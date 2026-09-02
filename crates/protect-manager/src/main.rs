//! protect-manager — a management layer for a `unifi-protect-backup` deployment.
//!
//! One container serves the SPA, the JSON API and the WebSockets from a single
//! origin. Same-origin is deliberate: no CORS, and the WebSocket authenticates
//! with the page's own cookie rather than a token in a query string.
//!
//! See the roadmap in README.md for what is built and what is planned.

mod archive;
mod auth;
mod config;
mod db;
mod docker;
mod error;
mod events;
mod health;
mod media;
mod ratelimit;
mod setup;
mod storage;
mod trace;
mod upb;
mod watchdog;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use futures_util::StreamExt;
use axum::extract::Path as UrlPath;
use axum::http::HeaderMap;
use protect_api_types::{
    CameraMonth, Check, ClipInfo, DiscoveryResult, EventQuery, Health, Schedule, SetupState,
    Settings, StartArchiveRequest, WatchdogConfig,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use tower_http::services::{ServeDir, ServeFile};

use crate::auth::authenticated;
use crate::config::Config;
use crate::error::ApiError;
use crate::health::{check_archive_dir, check_backup_dir};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: SqlitePool,
    pub docker: Option<Arc<bollard::Docker>>,
    pub jobs: archive::run::Jobs,
    pub media: media::Media,
    /// Sign-in backoff. In memory rather than in the database: it protects
    /// against a burst, and a process restart is not a burst.
    pub limiter: Arc<ratelimit::Limiter>,
}

impl AppState {
    fn job_context(&self) -> archive::run::JobContext {
        archive::run::JobContext {
            pool: self.pool.clone(),
            jobs: self.jobs.clone(),
            backup_dir: self.config.backup_dir.clone(),
            archive_dir: self.config.archive_dir.clone(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PM_LOG")
                .unwrap_or_else(|_| "protect_manager=info,tower_http=warn".into()),
        )
        .init();

    // Subcommands live here so setup and diagnosis need no separate tool, and
    // so they can be run *inside* the deployed container — which is the only
    // place that sees the environment the server actually got.
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("hash-password") => {
            let Some(password) = args.get(2) else {
                eprintln!("usage: protect-manager hash-password <password>");
                std::process::exit(2);
            };
            println!("{}", auth::hash_password(password)?);
            return Ok(());
        }

        // Reports the structure of PM_PASSWORD_HASH as this process sees it.
        Some("check-hash") => {
            let raw = std::env::var("PM_PASSWORD_HASH").unwrap_or_default();
            if raw.is_empty() {
                println!("PM_PASSWORD_HASH is not set in this environment.");
                std::process::exit(1);
            }
            println!("raw length:        {} characters", raw.len());
            println!("raw '$' count:     {}", raw.matches('$').count());
            let hash = auth::normalise_hash(&raw);
            if hash != raw {
                println!("after cleaning:    {} characters", hash.len());
            }
            match auth::diagnose_hash(&hash) {
                None => println!("\nOK — this is a valid argon2 hash."),
                Some(problem) => {
                    println!("\nNOT USABLE: {problem}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        // Tests a password against the configured hash, isolating "the hash is
        // wrong" from "the password is wrong" without touching the web layer.
        Some("verify-password") => {
            let Some(password) = args.get(2) else {
                eprintln!("usage: protect-manager verify-password <password>");
                std::process::exit(2);
            };
            let raw = std::env::var("PM_PASSWORD_HASH").unwrap_or_default();
            if raw.is_empty() {
                println!("PM_PASSWORD_HASH is not set in this environment.");
                std::process::exit(1);
            }
            let hash = auth::normalise_hash(&raw);
            if let Some(problem) = auth::diagnose_hash(&hash) {
                println!("The hash itself is unusable: {problem}");
                std::process::exit(1);
            }
            if auth::password_matches(&hash, password) {
                println!("MATCH — this password works with the configured hash.");
                return Ok(());
            }
            println!(
                "NO MATCH — the hash is valid, but this password does not produce it. \
                 Regenerate with 'hash-password' and set the new value."
            );
            std::process::exit(1);
        }

        _ => {}
    }

    let config = Arc::new(Config::from_env()?);
    match config.password_hash.as_deref() {
        None => tracing::warn!(
            "PM_PASSWORD_HASH is not set — every authenticated route will refuse. \
             Generate one with: protect-manager hash-password <password>"
        ),
        Some(hash) => auth::check_configured_hash(hash),
    }
    if !config.cookie_secure {
        tracing::warn!(
            "PM_COOKIE_SECURE=0 — session cookie will be sent over plain HTTP. \
             Development only; the proxy must terminate TLS in deployment."
        );
    }

    let pool = db::connect(&config.state_dir).await?;
    db::purge_expired_sessions(&pool).await;
    // A run recorded as in-progress cannot be one: this process just started.
    db::reconcile_interrupted_runs(&pool).await;

    // A missing Docker socket is reported through /api/health rather than being
    // fatal: an app that explains what's wrong beats a crash loop.
    let docker = match docker::connect() {
        Ok(d) => match d.ping().await {
            Ok(_) => Some(Arc::new(d)),
            Err(e) => {
                tracing::error!("docker socket present but not responding: {e}");
                None
            }
        },
        Err(e) => {
            tracing::error!("cannot connect to docker: {e}");
            None
        }
    };

    let state = AppState {
        config: config.clone(),
        pool,
        docker,
        jobs: archive::run::Jobs::default(),
        media: media::Media::new(config.state_dir.join("media")),
        limiter: Arc::new(ratelimit::Limiter::default()),
    };

    // The index syncs on a timer rather than on request. Reading the backup
    // service's database can block briefly while it writes, which is fine on a
    // background task and not fine while someone waits for a page.
    tokio::spawn(sync_loop(state.clone()));
    tokio::spawn(schedule_loop(state.clone()));
    tokio::spawn(sample_loop(state.clone()));
    tokio::spawn(watchdog_loop(state.clone()));

    let static_dir = config.static_dir.clone();
    let index = ServeFile::new(static_dir.join("index.html"));

    let app = Router::new()
        .route("/api/auth/status", get(auth::status))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/sessions", get(auth::sessions))
        .route("/api/auth/sessions/revoke-others", post(auth::revoke_others))
        .route("/api/health", get(health_handler))
        .route("/api/setup", get(setup_state_handler))
        .route("/api/setup/discover", get(discover_handler))
        .route("/api/settings", put(save_settings_handler))
        .route("/api/events", get(events_handler))
        .route("/api/cameras", get(cameras_handler))
        .route("/api/index/stats", get(index_stats_handler))
        .route("/api/index/sync", post(sync_now_handler))
        .route("/api/archive", get(archive_overview_handler))
        .route("/api/archive/runs", get(archive_runs_handler).post(start_archive_handler))
        .route("/api/archive/restore", post(restore_handler))
        .route("/api/archive/verify", post(verify_handler))
        .route("/api/archive/pin", post(pin_handler))
        .route("/api/schedule", get(get_schedule_handler).put(put_schedule_handler))
        .route("/ws/progress", get(progress_ws))
        .route("/api/storage", get(storage_handler))
        .route("/api/storage/history", get(storage_history_handler))
        .route("/api/watchdog", get(watchdog_handler))
        .route("/api/watchdog/config", put(watchdog_config_handler))
        .route("/api/media/{id}/info", get(clip_info_handler))
        .route("/api/media/{id}/thumb", get(thumb_handler))
        .route("/api/media/{id}/clip", get(clip_handler))
        .route("/api/media/{id}/original", get(original_handler))
        .route("/api/upb/containers", get(containers_handler))
        .route("/api/upb/inspect", get(inspect_handler))
        .route("/ws/logs", get(logs_ws))
        // Client-side routing: unknown paths fall back to index.html so a deep
        // link or a refresh lands on the app rather than a 404.
        .fallback_service(ServeDir::new(&static_dir).fallback(index))
        .layer(axum::middleware::from_fn(trace::middleware))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!("listening on http://{}", config.bind);
    // Connect info so sign-in backoff can tell clients apart when there is no
    // proxy in front to say who they are.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// Periodically rebuild the event index.
///
/// Errors are recorded and retried rather than escalated: the backup service
/// may be mid-write, the mount may be briefly unavailable, or setup may not
/// have happened yet. In all of those cases the previous index stays queryable
/// and the reason is visible in the UI.
async fn sync_loop(state: AppState) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        if let Err(e) = sync_once(&state).await {
            // Debug level: before setup is finished this is expected on every
            // tick, and a log full of it would bury real problems.
            tracing::debug!("index sync skipped: {e}");
            db::record_sync_error(&state.pool, &e.to_string()).await;
        }
    }
}

async fn sync_once(state: &AppState) -> anyhow::Result<upb::reconcile::SyncOutcome> {
    let settings = db::load_settings(&state.pool).await?;
    if !settings.setup_complete {
        anyhow::bail!("setup is not complete");
    }

    // How far back the backup service still backfills gaps decides whether an
    // un-captured event is recoverable or permanently lost. Read it from the
    // container rather than assuming a value.
    let missing_range = match state.docker.as_ref() {
        Some(docker) => match current_container(state, docker).await {
            Ok(Some(c)) => docker::inspect(docker, c, &state.config.backup_dir)
                .await
                .ok()
                .and_then(|i| i.missing_range)
                .and_then(|m| upb::reconcile::parse_duration_secs(&m)),
            _ => None,
        },
        None => None,
    };

    let outcome = upb::reconcile::sync(
        &state.pool,
        &settings,
        &state.config.backup_dir,
        missing_range,
    )
    .await?;

    tracing::debug!(
        "indexed {} events across {} cameras ({} clips checked on disk)",
        outcome.events,
        outcome.cameras,
        outcome.statted
    );

    // Thumbnails and transcodes only make sense for clips still on disk, so a
    // month being archived is what makes its cache entries garbage.
    let live: std::collections::HashSet<String> =
        sqlx::query_scalar("SELECT id FROM events WHERE status = 'live'")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
    let removed = state.media.evict(&live);
    if removed > 0 {
        tracing::info!("removed {removed} cached thumbnails/transcodes for clips no longer live");
    }

    Ok(outcome)
}

/// Fire scheduled archive runs, and say so loudly when one fails.
async fn schedule_loop(state: AppState) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        let schedule = archive::schedule::load(&state.pool).await;
        let now = upb::reconcile::now_secs();
        if !archive::schedule::is_due(&schedule, now) {
            continue;
        }

        let settings = match db::load_settings(&state.pool).await {
            Ok(s) if s.setup_complete => s,
            _ => continue,
        };

        tracing::info!("scheduled archive run starting");
        // Recorded as attempted before it runs. If it fails, the next tick
        // must not retry immediately in a loop — the failure is visible in
        // run history and, if configured, pushed to a webhook.
        archive::schedule::mark_ran(&state.pool, now).await;

        match archive::run::run_archive(state.job_context(), settings, Vec::new(), false, true)
            .await
        {
            Ok(run_id) => watch_scheduled_run(state.clone(), schedule, run_id),
            Err(e) => {
                // "Nothing to archive" is the normal state most of the time,
                // not a failure worth notifying anyone about.
                tracing::info!("scheduled run did not start: {e}");
            }
        }
    }
}

/// Watch a scheduled run to completion so a failure can be pushed outward.
fn watch_scheduled_run(state: AppState, schedule: Schedule, run_id: i64) {
    let Some(url) = schedule.webhook_url.clone() else { return };
    let mut rx = state.jobs.progress.subscribe();

    tokio::spawn(async move {
        while let Ok(p) = rx.recv().await {
            if p.run_id != run_id || !p.finished {
                continue;
            }
            if matches!(
                p.status,
                Some(protect_api_types::RunStatus::Failed)
                    | Some(protect_api_types::RunStatus::Interrupted)
            ) {
                archive::schedule::notify_failure(
                    &url,
                    run_id,
                    p.message.as_deref().unwrap_or("archive run failed"),
                )
                .await;
            }
            return;
        }
    });
}

/// Record storage usage periodically, so the trend has something to plot.
///
/// Half-hourly: often enough that a day of history is a usable line, rare
/// enough that a year of samples stays trivially small.
async fn sample_loop(state: AppState) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1800));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        let Ok(settings) = db::load_settings(&state.pool).await else { continue };
        if !settings.setup_complete {
            continue;
        }
        if let Err(e) = storage::take_sample(
            &state.pool,
            &settings,
            &state.config.backup_dir,
            &state.config.archive_dir,
        )
        .await
        {
            tracing::warn!("could not record a storage sample: {e}");
        }
    }
}

/// Watch for the backup service recording events but not downloading them.
///
/// Every two minutes: often enough to notice within the grace period, rare
/// enough to be free. The judgement itself is two SQL aggregates.
async fn watchdog_loop(state: AppState) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        match db::load_settings(&state.pool).await {
            Ok(s) if s.setup_complete => watchdog::tick(&state).await,
            _ => {}
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

/// Guard for everything that isn't the login flow itself.
///
/// Every authenticated handler opens with `require_auth(&state, &jar).await?`,
/// which is short enough that forgetting it is visible in review.
async fn require_auth(state: &AppState, jar: &CookieJar) -> Result<(), ApiError> {
    if authenticated(state, jar).await {
        Ok(())
    } else {
        Err(ApiError::unauthenticated())
    }
}

/// Resolve the container the user picked during setup.
///
/// Container IDs do not survive `docker compose up` — the container is
/// replaced and gets a new one. Pinning the ID chosen during setup would mean
/// the app breaks every time the backup service is redeployed, which for a
/// homelab tool is often. So a stale ID falls back to discovery by image, and
/// the new ID is written back.
pub async fn current_container(
    state: &AppState,
    docker: &bollard::Docker,
) -> anyhow::Result<Option<protect_api_types::ContainerRef>> {
    // An operator-supplied override is honoured as given: if it is wrong, the
    // error should say so rather than being quietly worked around.
    if let Some(explicit) = state.config.upb_container.as_deref() {
        return docker::resolve(docker, Some(explicit), &state.config.upb_image).await;
    }

    let mut settings = db::load_settings(&state.pool).await.unwrap_or_default();

    if let Some(saved) = settings.upb_container_id.clone() {
        match docker::resolve(docker, Some(&saved), &state.config.upb_image).await {
            Ok(Some(found)) => return Ok(Some(found)),
            _ => tracing::info!(
                "configured container {saved} is gone — it was probably recreated; \
                 re-discovering by image"
            ),
        }
    }

    let found = docker::resolve(docker, None, &state.config.upb_image).await?;

    if let Some(c) = &found {
        if settings.upb_container_id.as_deref() != Some(c.id.as_str())
            && settings.upb_container_id.is_some()
        {
            settings.upb_container_id = Some(c.id.clone());
            if let Err(e) = db::save_settings(&state.pool, &settings).await {
                tracing::error!("could not record the new container id: {e}");
            }
        }
    }

    Ok(found)
}

async fn health_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Health>, ApiError> {
    require_auth(&state, &jar).await?;

    let mut warnings = Vec::new();
    let mut info = Vec::new();

    let (docker_check, container_check) = match state.docker.as_ref() {
        None => (
            Check {
                ok: false,
                detail: "docker socket unavailable — is /var/run/docker.sock mounted?".into(),
            },
            Check { ok: false, detail: "not checked".into() },
        ),
        Some(docker) => {
            let d = Check { ok: true, detail: "socket reachable".into() };
            let c = match current_container(&state, docker).await {
                Ok(Some(found)) => {
                    // Finding the container is not the same as it working.
                    // Health must go red when it is stopped, or the dashboard
                    // reports "fine" for the exact failure it exists to catch.
                    let mut running = false;
                    if let Ok(insp) =
                        docker::inspect(docker, found.clone(), &state.config.backup_dir).await
                    {
                        running = insp.running;

                        // If UPB's retention is ever shorter than the age a
                        // month must reach to be archived, it deletes clips
                        // before we ever pack them — silent loss nothing else
                        // would notice. Compared against the archiving
                        // threshold rather than the live window: the live
                        // window is about where a clip is expected to be, and
                        // using it here made this warning fire, or stay quiet,
                        // for the wrong number.
                        let archive_after_days = db::load_settings(&state.pool)
                            .await
                            .map(|s| s.archive_after_days)
                            .unwrap_or(0);
                        match (&insp.retention, archive_after_days) {
                            (Some(r), d) if d > 0 => match parse_retention_days(r) {
                                Some(days) if days < d as u64 => warnings.push(format!(
                                    "UPB retention is {r}, shorter than the {d}-day archiving \
                                     threshold — clips will be deleted before they are archived"
                                )),
                                Some(_) => {}
                                None => info.push(format!(
                                    "UPB retention is {r}, which could not be compared to the \
                                     archiving threshold"
                                )),
                            },
                            (Some(r), _) => info.push(format!(
                                "UPB retention is {r} — will be checked once the archiving \
                                 threshold is set"
                            )),
                            _ => {}
                        }

                        if let Some(m) = &insp.missing_range {
                            info.push(format!(
                                "UPB backfills missing events within {m}; gaps older than that \
                                 are permanent"
                            ));

                            // Both containers share the clip directory, and age
                            // is what keeps them apart: we only archive months
                            // the backup service has finished with. A backfill
                            // window that reaches into archivable months breaks
                            // that assumption — it can write into a month we
                            // are about to pack and delete.
                            if let (Some(backfill), d @ 1..) =
                                (upb::reconcile::parse_duration_secs(m), archive_after_days)
                            {
                                let archivable_after = (d as f64) * 86_400.0;
                                if backfill >= archivable_after {
                                    warnings.push(format!(
                                        "UPB backfills up to {m}, which reaches into months old \
                                         enough to archive ({d}-day threshold). It could write \
                                         into a month while it is being packed — raise the \
                                         archiving threshold above the backfill range."
                                    ));
                                }
                            }
                        }
                        if !insp.running {
                            warnings.push("backup container is not running".into());
                        }
                        if insp.restart_count > 0 {
                            warnings.push(format!(
                                "backup container has restarted {} times",
                                insp.restart_count
                            ));
                        }
                    }
                    Check {
                        ok: running,
                        detail: format!(
                            "{} ({}) — {}",
                            found.name,
                            found.image,
                            if running { "running" } else { "not running" }
                        ),
                    }
                }
                Ok(None) => Check {
                    ok: false,
                    detail: format!("no container matching image {}", state.config.upb_image),
                },
                Err(e) => Check { ok: false, detail: format!("{e}") },
            };
            (d, c)
        }
    };

    let backup = check_backup_dir(&state.config.backup_dir);
    let archive = check_archive_dir(&state.config.archive_dir);

    Ok(Json(Health {
        ok: docker_check.ok && container_check.ok && backup.ok && archive.ok,
        docker: docker_check,
        container: container_check,
        backup_dir: backup,
        archive_dir: archive,
        warnings,
        info,
    }))
}

/// Parse UPB's retention argument (`36500d`, `72h`, `90m`) into whole days.
///
/// Returns `None` for anything unrecognised, so an unfamiliar format becomes
/// "could not compare" rather than a confidently wrong warning.
fn parse_retention_days(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let (value, unit) = raw.split_at(raw.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = value.parse().ok()?;
    match unit {
        "d" => Some(n),
        "h" => Some(n / 24),
        "m" => Some(n / (24 * 60)),
        "s" => Some(n / (24 * 3600)),
        _ => None,
    }
}

async fn setup_state_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<SetupState>, ApiError> {
    require_auth(&state, &jar).await?;
    let settings = db::load_settings(&state.pool).await.unwrap_or_default();
    let checks = setup::validate(&settings, &state.config.backup_dir);
    Ok(Json(SetupState {
        complete: settings.setup_complete && checks.iter().all(|c| c.ok),
        settings,
        checks,
    }))
}

async fn discover_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<DiscoveryResult>, ApiError> {
    require_auth(&state, &jar).await?;

    let mut notes = Vec::new();
    let (containers, inspection) = match state.docker.as_ref() {
        None => {
            notes.push("Docker is unavailable, so nothing could be detected automatically.".into());
            (Vec::new(), None)
        }
        Some(docker) => {
            let containers = docker::discover(docker, &state.config.upb_image)
                .await
                .unwrap_or_else(|e| {
                    notes.push(format!("container discovery failed: {e}"));
                    Vec::new()
                });
            if containers.is_empty() {
                notes.push(format!(
                    "No container matching image {}. Set PM_UPB_IMAGE if you run a fork or a \
                     retagged build.",
                    state.config.upb_image
                ));
            }
            let inspection = match current_container(&state, docker).await {
                Ok(Some(c)) => docker::inspect(docker, c, &state.config.backup_dir).await.ok(),
                _ => None,
            };
            (containers, inspection)
        }
    };

    let (cameras, cam_notes) = setup::find_cameras(&state.config.backup_dir);
    notes.extend(cam_notes);
    if let Some(i) = &inspection {
        notes.extend(i.proposed.notes.clone());
    }

    Ok(Json(DiscoveryResult { containers, inspection, cameras, notes }))
}

async fn save_settings_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(mut body): Json<Settings>,
) -> Result<Json<SetupState>, ApiError> {
    require_auth(&state, &jar).await?;

    // Validate before storing. Refusing to save a configuration we already know
    // is broken keeps "saved" meaning something.
    let checks = setup::validate(&body, &state.config.backup_dir);
    if body.setup_complete && !checks.iter().all(|c| c.ok) {
        return Err(ApiError::invalid_settings(
            checks.into_iter().filter(|c| !c.ok).collect(),
        ));
    }

    body.camera_dirs.sort();
    body.camera_dirs.dedup();

    db::save_settings(&state.pool, &body).await.map_err(ApiError::internal)?;

    // Index immediately so the app has data the moment setup finishes, rather
    // than an empty feed until the next tick.
    if body.setup_complete {
        let bg = state.clone();
        tokio::spawn(async move {
            if let Err(e) = sync_once(&bg).await {
                tracing::warn!("first index sync after setup failed: {e}");
                db::record_sync_error(&bg.pool, &e.to_string()).await;
            }
        });
    }

    let checks = setup::validate(&body, &state.config.backup_dir);
    Ok(Json(SetupState {
        complete: body.setup_complete && checks.iter().all(|c| c.ok),
        settings: body,
        checks,
    }))
}

async fn containers_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Vec<protect_api_types::ContainerRef>>, ApiError> {
    require_auth(&state, &jar).await?;
    let docker = state.docker.as_ref().ok_or_else(ApiError::docker_unavailable)?;
    docker::discover(docker, &state.config.upb_image)
        .await
        .map(Json)
        .map_err(ApiError::docker_failed)
}

async fn inspect_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<protect_api_types::UpbInspection>, ApiError> {
    require_auth(&state, &jar).await?;
    let docker = state.docker.as_ref().ok_or_else(ApiError::docker_unavailable)?;

    let container = current_container(&state, docker)
        .await
        .map_err(ApiError::docker_failed)?
        .ok_or_else(|| ApiError::container_not_found(&state.config.upb_image))?;

    docker::inspect(docker, container, &state.config.backup_dir)
        .await
        .map(Json)
        .map_err(ApiError::docker_failed)
}

async fn events_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<EventQuery>,
) -> Result<Json<protect_api_types::EventPage>, ApiError> {
    require_auth(&state, &jar).await?;
    Ok(Json(events::query(&state.pool, &q).await?))
}

async fn cameras_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Vec<protect_api_types::CameraInfo>>, ApiError> {
    require_auth(&state, &jar).await?;
    Ok(Json(events::cameras(&state.pool).await?))
}

async fn index_stats_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&state, &jar).await?;
    let stats = events::stats(&state.pool).await?;
    let types = events::event_types(&state.pool).await.unwrap_or_default();
    Ok(Json(serde_json::json!({ "stats": stats, "event_types": types })))
}

/// Force a sync now, so finishing setup shows results immediately instead of
/// leaving an empty page until the next tick.
async fn sync_now_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&state, &jar).await?;
    match sync_once(&state).await {
        Ok(o) => Ok(Json(serde_json::json!({
            "events": o.events, "cameras": o.cameras, "clips_checked": o.statted
        }))),
        Err(e) => {
            db::record_sync_error(&state.pool, &e.to_string()).await;
            // A sync that cannot run is almost always "setup is unfinished" or
            // "the mount went away" — the user's to fix, not a server fault.
            Err(ApiError::conflict(format!("The index could not be rebuilt: {e}")))
        }
    }
}

async fn archive_overview_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<protect_api_types::ArchiveOverview>, ApiError> {
    require_auth(&state, &jar).await?;
    let settings = db::load_settings(&state.pool).await.unwrap_or_default();
    Ok(Json(
        archive::run::overview(
            &state.pool,
            &settings,
            &state.config.backup_dir,
            &state.config.archive_dir,
        )
        .await?,
    ))
}

async fn archive_runs_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Vec<protect_api_types::ArchiveRun>>, ApiError> {
    require_auth(&state, &jar).await?;
    Ok(Json(archive::run::recent_runs(&state.pool, 50).await?))
}

async fn start_archive_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<StartArchiveRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&state, &jar).await?;
    let settings = match db::load_settings(&state.pool).await {
        Ok(s) if s.setup_complete => s,
        _ => return Err(ApiError::setup_incomplete()),
    };

    // Refusals here are all "not now": a job is already running, or nothing is
    // old enough to archive. Neither is a fault.
    let id = archive::run::run_archive(
        state.job_context(),
        settings,
        body.targets,
        body.dry_run,
        false,
    )
    .await
    .map_err(|e| ApiError::conflict(format!("{e}")))?;

    Ok(Json(serde_json::json!({ "run_id": id })))
}

async fn restore_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(target): Json<CameraMonth>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&state, &jar).await?;
    let id = archive::run::run_restore(state.job_context(), target)
        .await
        .map_err(|e| ApiError::conflict(format!("{e}")))?;
    Ok(Json(serde_json::json!({ "run_id": id })))
}

async fn verify_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(target): Json<CameraMonth>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&state, &jar).await?;
    let id = archive::run::run_verify(state.job_context(), target)
        .await
        .map_err(|e| ApiError::conflict(format!("{e}")))?;
    Ok(Json(serde_json::json!({ "run_id": id })))
}

#[derive(Deserialize)]
struct PinRequest {
    camera: String,
    month: String,
    pinned: bool,
}

/// Release (or re-apply) a pin, so a restored month can be archived again.
async fn pin_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<PinRequest>,
) -> Result<StatusCode, ApiError> {
    require_auth(&state, &jar).await?;
    sqlx::query("UPDATE archives SET pinned = ? WHERE camera = ? AND month = ?")
        .bind(body.pinned as i32)
        .bind(&body.camera)
        .bind(&body.month)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_schedule_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Schedule>, ApiError> {
    require_auth(&state, &jar).await?;
    Ok(Json(archive::schedule::load(&state.pool).await))
}

async fn put_schedule_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<Schedule>,
) -> Result<Json<Schedule>, ApiError> {
    require_auth(&state, &jar).await?;
    if let Some(problem) = archive::schedule::validation_error(&body) {
        return Err(ApiError::invalid(problem));
    }
    archive::schedule::save(&state.pool, &body).await?;
    Ok(Json(archive::schedule::load(&state.pool).await))
}

/// Live job progress. Same cookie auth as the log stream.
async fn progress_ws(
    State(state): State<AppState>,
    jar: CookieJar,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    require_auth(&state, &jar).await?;
    let mut rx = state.jobs.progress.subscribe();
    Ok(ws.on_upgrade(move |mut socket| async move {
        while let Ok(update) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&update) else { continue };
            if socket.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    }))
}

async fn storage_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<protect_api_types::StorageSnapshot>, ApiError> {
    require_auth(&state, &jar).await?;
    let settings = db::load_settings(&state.pool).await.unwrap_or_default();
    Ok(Json(
        storage::snapshot(
            &state.pool,
            &settings,
            &state.config.backup_dir,
            &state.config.archive_dir,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default = "default_history_days")]
    days: f64,
}

fn default_history_days() -> f64 {
    30.0
}

async fn storage_history_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<protect_api_types::StorageSample>>, ApiError> {
    require_auth(&state, &jar).await?;
    Ok(Json(storage::history(&state.pool, q.days.clamp(1.0, 365.0)).await?))
}

/// Resolve an event id to a clip we are allowed to open.
///
/// Two checks, both necessary: the index must say the clip is live (an
/// archived or missing clip has no file), and the path must actually be under
/// the backup directory — it is derived from another program's data, so it is
/// verified rather than trusted.
async fn resolve_clip(state: &AppState, id: &str) -> Result<std::path::PathBuf, ApiError> {
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT clip_path, status FROM events WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;

    let Some((path, status)) = row else {
        return Err(ApiError::not_found("There is no event with that id."));
    };
    let Some(path) = path else {
        return Err(ApiError::not_found("This event was never backed up."));
    };
    if status != "live" {
        return Err(ApiError::not_found(match status.as_str() {
            "archived" => "This clip has been archived.".to_string(),
            _ => format!("This clip is not in the live directory ({status})."),
        }));
    }

    let path = std::path::PathBuf::from(path);
    if !media::within(&state.config.backup_dir, &path) {
        // The path came out of another program's database, so it is checked
        // rather than trusted. Reaching here means the index disagrees with
        // the configuration, which is worth a log line.
        tracing::warn!(%id, "indexed clip path is outside the backup directory");
        return Err(ApiError::not_found("That clip is outside the backup directory."));
    }
    Ok(path)
}

async fn clip_info_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ClipInfo>, ApiError> {
    require_auth(&state, &jar).await?;
    // Unavailability is the answer here rather than an error: the timeline asks
    // about clips it already knows may be archived, and a 404 per tile would
    // be noise in the console for something entirely expected.
    let path = match resolve_clip(&state, &id).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(Json(ClipInfo {
                id,
                available: false,
                reason: Some(e.to_string()),
                codec: None,
                width: None,
                height: None,
                size_bytes: None,
                fps: None,
                direct: false,
                prepared: false,
            }))
        }
    };

    let probed = state.media.probe(&path).await.ok();
    let direct = probed
        .as_ref()
        .map(|p| media::Media::browser_can_play(&p.codec))
        .unwrap_or(false);
    let size_bytes = std::fs::metadata(&path).ok().map(|m| m.len() as i64);

    Ok(Json(ClipInfo {
        prepared: direct || state.media.is_prepared(&id),
        available: true,
        reason: None,
        codec: probed.as_ref().map(|p| p.codec.clone()),
        width: probed.as_ref().and_then(|p| p.width),
        height: probed.as_ref().and_then(|p| p.height),
        size_bytes,
        fps: probed.as_ref().and_then(|p| p.fps),
        direct,
        id,
    }))
}

async fn thumb_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_auth(&state, &jar).await?;
    let source = resolve_clip(&state, &id).await?;
    // A clip that will not decode is a truncated or in-progress download, not
    // a server fault — reporting it as a 500 sent people looking for a bug in
    // the app when the answer was to wait for the download to finish.
    let thumb = state
        .media
        .thumbnail(&id, &source)
        .await
        .map_err(ApiError::media_unreadable)?;
    Ok(media::range::serve_file(&thumb, "image/jpeg", &headers).await)
}

/// A clip the browser can play, transcoding first if it has to.
async fn clip_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_auth(&state, &jar).await?;
    let source = resolve_clip(&state, &id).await?;
    let clip = state
        .media
        .playable(&id, &source)
        .await
        .map_err(ApiError::media_unreadable)?;
    Ok(media::range::serve_file(&clip.path, "video/mp4", &headers).await)
}

/// The recording itself, untouched — for downloading the real thing.
async fn original_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_auth(&state, &jar).await?;
    let path = resolve_clip(&state, &id).await?;
    Ok(media::range::serve_file(&path, "video/mp4", &headers).await)
}

async fn watchdog_handler(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<protect_api_types::WatchdogState>, ApiError> {
    require_auth(&state, &jar).await?;
    Ok(Json(watchdog::state(&state.pool).await))
}

async fn watchdog_config_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<WatchdogConfig>,
) -> Result<Json<protect_api_types::WatchdogState>, ApiError> {
    require_auth(&state, &jar).await?;
    watchdog::save_config(&state.pool, &body).await?;
    Ok(Json(watchdog::state(&state.pool).await))
}

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default = "default_tail")]
    tail: String,
}

fn default_tail() -> String {
    "200".into()
}

/// Live container logs over a WebSocket, authenticated by the page's cookie.
async fn logs_ws(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<LogQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    require_auth(&state, &jar).await?;
    let docker = state.docker.clone().ok_or_else(ApiError::docker_unavailable)?;

    let container = current_container(&state, &docker)
        .await
        .map_err(ApiError::docker_failed)?
        .ok_or_else(|| ApiError::container_not_found(&state.config.upb_image))?;

    Ok(ws.on_upgrade(move |socket| stream_logs(socket, docker, container.id, q.tail)))
}

async fn stream_logs(
    mut socket: WebSocket,
    docker: Arc<bollard::Docker>,
    container_id: String,
    tail: String,
) {
    let mut stream = docker.logs(&container_id, Some(docker::log_options(&tail)));

    while let Some(item) = stream.next().await {
        let text = match item {
            Ok(out) => out.to_string(),
            Err(e) => format!("[stream error] {e}"),
        };
        let text = text.trim_end_matches(['\n', '\r']).to_string();
        if text.is_empty() {
            continue;
        }
        if socket.send(Message::Text(text.into())).await.is_err() {
            break; // client went away
        }
    }

    let _ = socket.send(Message::Text("[stream ended]".into())).await;
}

#[cfg(test)]
mod tests {
    use super::parse_retention_days;

    #[test]
    fn parses_retention_durations() {
        assert_eq!(parse_retention_days("36500d"), Some(36500));
        assert_eq!(parse_retention_days("72h"), Some(3));
        assert_eq!(parse_retention_days("30d"), Some(30));
        // Sub-day retention truncates to zero, which is correctly "shorter
        // than any live window" rather than an error.
        assert_eq!(parse_retention_days("6h"), Some(0));
    }

    #[test]
    fn unrecognised_retention_is_not_guessed_at() {
        assert_eq!(parse_retention_days("forever"), None);
        assert_eq!(parse_retention_days("30"), None);
        assert_eq!(parse_retention_days("30y"), None);
        assert_eq!(parse_retention_days(""), None);
    }
}

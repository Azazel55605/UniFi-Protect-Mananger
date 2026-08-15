//! protect-manager — a management layer for a `unifi-protect-backup` deployment.
//!
//! One container serves the SPA, the JSON API and the WebSockets from a single
//! origin. Same-origin is deliberate: no CORS, and the WebSocket authenticates
//! with the page's own cookie rather than a token in a query string.
//!
//! See the roadmap in README.md for what is built and what is planned.

mod auth;
mod config;
mod db;
mod docker;
mod health;
mod setup;

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use futures_util::StreamExt;
use protect_api_types::{Check, DiscoveryResult, Health, SetupState, Settings};
use serde::Deserialize;
use sqlx::SqlitePool;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::auth::authenticated;
use crate::config::Config;
use crate::health::check_backup_dir;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: SqlitePool,
    pub docker: Option<Arc<bollard::Docker>>,
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

    let state = AppState { config: config.clone(), pool, docker };

    let static_dir = config.static_dir.clone();
    let index = ServeFile::new(static_dir.join("index.html"));

    let app = Router::new()
        .route("/api/auth/status", get(auth::status))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/health", get(health_handler))
        .route("/api/setup", get(setup_state_handler))
        .route("/api/setup/discover", get(discover_handler))
        .route("/api/settings", put(save_settings_handler))
        .route("/api/upb/containers", get(containers_handler))
        .route("/api/upb/inspect", get(inspect_handler))
        .route("/ws/logs", get(logs_ws))
        // Client-side routing: unknown paths fall back to index.html so a deep
        // link or a refresh lands on the app rather than a 404.
        .fallback_service(ServeDir::new(&static_dir).fallback(index))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!("listening on http://{}", config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

/// Guard for everything that isn't the login flow itself.
///
/// Returns the rejection response itself so callers can `return` it directly.
/// `Response` is a large `Err` variant, but this never crosses a hot path and
/// boxing it would only add indirection to every handler.
#[allow(clippy::result_large_err)]
async fn require_auth(state: &AppState, jar: &CookieJar) -> Result<(), Response> {
    if authenticated(state, jar).await {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "authentication required").into_response())
    }
}

/// Resolve the container the user picked during setup.
///
/// Container IDs do not survive `docker compose up` — the container is
/// replaced and gets a new one. Pinning the ID chosen during setup would mean
/// the app breaks every time the backup service is redeployed, which for a
/// homelab tool is often. So a stale ID falls back to discovery by image, and
/// the new ID is written back.
async fn current_container(
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

async fn health_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Err(r) = require_auth(&state, &jar).await {
        return r;
    }

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

                        // If UPB's retention is ever shorter than the live
                        // window it deletes clips before we archive them —
                        // silent loss nothing else would notice.
                        let live_window = db::load_settings(&state.pool)
                            .await
                            .map(|s| s.live_window_months)
                            .unwrap_or(0);
                        match (&insp.retention, live_window) {
                            (Some(r), w) if w > 0 => match parse_retention_days(r) {
                                Some(days) if days < (w as u64) * 28 => warnings.push(format!(
                                    "UPB retention is {r}, shorter than the {w}-month live \
                                     window — clips will be deleted before they are archived"
                                )),
                                Some(_) => {}
                                None => info.push(format!(
                                    "UPB retention is {r}, which could not be compared to the \
                                     live window"
                                )),
                            },
                            (Some(r), _) => info.push(format!(
                                "UPB retention is {r} — will be checked once the live window is set"
                            )),
                            _ => {}
                        }

                        if let Some(m) = &insp.missing_range {
                            info.push(format!(
                                "UPB backfills missing events within {m}; gaps older than that \
                                 are permanent"
                            ));
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

    Json(Health {
        ok: docker_check.ok && container_check.ok && backup.ok,
        docker: docker_check,
        container: container_check,
        backup_dir: backup,
        warnings,
        info,
    })
    .into_response()
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

async fn setup_state_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Err(r) = require_auth(&state, &jar).await {
        return r;
    }
    let settings = db::load_settings(&state.pool).await.unwrap_or_default();
    let checks = setup::validate(&settings, &state.config.backup_dir);
    Json(SetupState {
        complete: settings.setup_complete && checks.iter().all(|c| c.ok),
        settings,
        checks,
    })
    .into_response()
}

async fn discover_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Err(r) = require_auth(&state, &jar).await {
        return r;
    }

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

    Json(DiscoveryResult { containers, inspection, cameras, notes }).into_response()
}

async fn save_settings_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(mut body): Json<Settings>,
) -> Response {
    if let Err(r) = require_auth(&state, &jar).await {
        return r;
    }

    // Validate before storing. Refusing to save a configuration we already know
    // is broken keeps "saved" meaning something.
    let checks = setup::validate(&body, &state.config.backup_dir);
    if body.setup_complete && !checks.iter().all(|c| c.ok) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(checks.into_iter().filter(|c| !c.ok).collect::<Vec<_>>()),
        )
            .into_response();
    }

    body.camera_dirs.sort();
    body.camera_dirs.dedup();

    match db::save_settings(&state.pool, &body).await {
        Ok(_) => {
            let checks = setup::validate(&body, &state.config.backup_dir);
            Json(SetupState {
                complete: body.setup_complete && checks.iter().all(|c| c.ok),
                settings: body,
                checks,
            })
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn containers_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Err(r) = require_auth(&state, &jar).await {
        return r;
    }
    let Some(docker) = state.docker.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "docker unavailable").into_response();
    };
    match docker::discover(docker, &state.config.upb_image).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e}")).into_response(),
    }
}

async fn inspect_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Err(r) = require_auth(&state, &jar).await {
        return r;
    }
    let Some(docker) = state.docker.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "docker unavailable").into_response();
    };
    match current_container(&state, docker).await {
        Ok(Some(c)) => match docker::inspect(docker, c, &state.config.backup_dir).await {
            Ok(i) => Json(i).into_response(),
            Err(e) => (StatusCode::BAD_GATEWAY, format!("{e}")).into_response(),
        },
        Ok(None) => (StatusCode::NOT_FOUND, "no backup container found").into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e}")).into_response(),
    }
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
) -> Response {
    if let Err(r) = require_auth(&state, &jar).await {
        return r;
    }
    let Some(docker) = state.docker.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "docker unavailable").into_response();
    };

    let container = match current_container(&state, &docker).await {
        Ok(Some(c)) => c,
        Ok(None) => return (StatusCode::NOT_FOUND, "no backup container found").into_response(),
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("{e}")).into_response(),
    };

    ws.on_upgrade(move |socket| stream_logs(socket, docker, container.id, q.tail))
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

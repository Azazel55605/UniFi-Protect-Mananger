//! Noticing when the backup service stops downloading.
//!
//! Its known failure is to keep running while quietly fetching nothing — the
//! event subscription drops without a clean close, so the process looks
//! healthy and only a restart fixes it. Uptime, restart count and container
//! state all say "fine", which is why none of them catch it.
//!
//! The signal that does catch it comes from the service's own database. It
//! writes an event row when it *sees* an event and a backup row when it
//! *downloads* one, so the two clocks can be compared:
//!
//! * events arriving, downloads not → stalled;
//! * no events arriving at all → a quiet night, which is not a fault.
//!
//! A plain "no clips for N hours" timeout cannot tell those apart and would
//! restart the service every quiet night.

use protect_api_types::{WatchdogConfig, WatchdogEvent, WatchdogState};
use sqlx::{Row, SqlitePool};

use crate::upb::reconcile::now_secs;

pub const DEFAULT: WatchdogConfig = WatchdogConfig {
    enabled: true,
    grace_minutes: 30,
    auto_restart: false,
    restart_cooldown_minutes: 30,
    webhook_url: None,
};

/// At least this many events must be waiting before a gap counts as a stall.
///
/// One clip failing to download is a failed clip, not a stalled service, and
/// restarting for it would be an overreaction.
const MIN_UNCAPTURED: i64 = 2;

pub struct Assessment {
    pub symptom: bool,
    pub newest_event: Option<f64>,
    pub newest_captured: Option<f64>,
    pub gap_secs: Option<f64>,
    pub uncaptured: i64,
    pub summary: String,
}

/// Decide, from the index alone, whether downloads have stalled.
pub fn assess(
    newest_event: Option<f64>,
    newest_captured: Option<f64>,
    uncaptured: i64,
    grace_secs: f64,
) -> Assessment {
    let gap = match (newest_event, newest_captured) {
        (Some(e), Some(c)) => Some(e - c),
        // Events recorded but nothing ever downloaded: the gap is the age of
        // the oldest thing we are waiting on, which we approximate with the
        // newest event's age.
        (Some(e), None) => Some(now_secs() - e),
        _ => None,
    };

    let symptom = matches!(gap, Some(g) if g > grace_secs) && uncaptured >= MIN_UNCAPTURED;

    let summary = match (newest_event, gap) {
        (None, _) => "No events indexed yet.".to_string(),
        (Some(_), Some(g)) if symptom => format!(
            "{uncaptured} events recorded but not downloaded; the newest clip is {} behind \
             the newest event.",
            human(g)
        ),
        (Some(_), Some(g)) if uncaptured > 0 => format!(
            "{uncaptured} event(s) waiting to download, {} behind. Within normal delay.",
            human(g)
        ),
        _ => "Downloads are keeping up with events.".to_string(),
    };

    Assessment { symptom, newest_event, newest_captured, gap_secs: gap, uncaptured, summary }
}

fn human(secs: f64) -> String {
    if secs < 90.0 {
        format!("{}s", secs.round())
    } else if secs < 5400.0 {
        format!("{}m", (secs / 60.0).round())
    } else {
        format!("{}h", (secs / 3600.0).round())
    }
}

// ------------------------------------------------------------------ state

pub async fn load_config(pool: &SqlitePool) -> WatchdogConfig {
    sqlx::query("SELECT json FROM watchdog WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|r| serde_json::from_str(&r.get::<String, _>("json")).ok())
        .unwrap_or(DEFAULT)
}

pub async fn save_config(pool: &SqlitePool, cfg: &WatchdogConfig) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO watchdog (id, json) VALUES (1, ?)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json",
    )
    .bind(serde_json::to_string(cfg)?)
    .execute(pool)
    .await?;
    Ok(())
}

async fn read_marks(pool: &SqlitePool) -> (Option<f64>, Option<f64>) {
    sqlx::query("SELECT stalled_since, last_restart FROM watchdog WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|r| (r.get("stalled_since"), r.get("last_restart")))
        .unwrap_or((None, None))
}

async fn set_stalled_since(pool: &SqlitePool, at: Option<f64>) {
    let _ = sqlx::query(
        "INSERT INTO watchdog (id, json, stalled_since) VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET stalled_since = excluded.stalled_since",
    )
    .bind(serde_json::to_string(&DEFAULT).unwrap_or_default())
    .bind(at)
    .execute(pool)
    .await;
}

async fn set_last_restart(pool: &SqlitePool, at: f64) {
    let _ = sqlx::query(
        "INSERT INTO watchdog (id, json, last_restart) VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET last_restart = excluded.last_restart",
    )
    .bind(serde_json::to_string(&DEFAULT).unwrap_or_default())
    .bind(at)
    .execute(pool)
    .await;
}

pub async fn record(pool: &SqlitePool, action: &str, detail: &str) {
    let _ = sqlx::query("INSERT INTO watchdog_log (at, action, detail) VALUES (?, ?, ?)")
        .bind(now_secs())
        .bind(action)
        .bind(detail)
        .execute(pool)
        .await;

    // A watchdog that keeps unbounded history becomes its own storage problem.
    let _ = sqlx::query(
        "DELETE FROM watchdog_log WHERE at < (
            SELECT MIN(at) FROM (SELECT at FROM watchdog_log ORDER BY at DESC LIMIT 100)
         )",
    )
    .execute(pool)
    .await;
}

pub async fn log(pool: &SqlitePool, limit: i64) -> Vec<WatchdogEvent> {
    sqlx::query("SELECT at, action, detail FROM watchdog_log ORDER BY at DESC LIMIT ?")
        .bind(limit)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| WatchdogEvent {
            at: r.get("at"),
            action: r.get("action"),
            detail: r.get("detail"),
        })
        .collect()
}

/// Read the two clocks out of the index.
pub async fn marks(pool: &SqlitePool) -> (Option<f64>, Option<f64>, i64) {
    let row = sqlx::query(
        "SELECT MAX(start) AS newest,
                MAX(CASE WHEN clip_path IS NOT NULL THEN start END) AS captured
           FROM events",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let newest: Option<f64> = row.as_ref().and_then(|r| r.get("newest"));
    let captured: Option<f64> = row.as_ref().and_then(|r| r.get("captured"));

    // Events after the last successful download — the backlog, ignoring old
    // gaps that are permanently unrecoverable.
    let uncaptured: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE clip_path IS NULL AND start > COALESCE(?, 0)",
    )
    .bind(captured)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    (newest, captured, uncaptured)
}

pub async fn state(pool: &SqlitePool) -> WatchdogState {
    let config = load_config(pool).await;
    let (newest, captured, uncaptured) = marks(pool).await;
    let a = assess(newest, captured, uncaptured, config.grace_minutes as f64 * 60.0);
    let (stalled_since, last_restart) = read_marks(pool).await;

    WatchdogState {
        stalled: a.symptom && config.enabled,
        stalled_since,
        newest_event: a.newest_event,
        newest_captured: a.newest_captured,
        gap_secs: a.gap_secs,
        uncaptured: a.uncaptured,
        summary: a.summary,
        last_restart,
        log: log(pool, 20).await,
        config,
    }
}

/// One watchdog tick: assess, and act if the symptom has persisted.
pub async fn tick(state: &crate::AppState) {
    let config = load_config(&state.pool).await;
    if !config.enabled {
        return;
    }

    let (newest, captured, uncaptured) = marks(&state.pool).await;
    let grace = config.grace_minutes as f64 * 60.0;
    let a = assess(newest, captured, uncaptured, grace);
    let (stalled_since, last_restart) = read_marks(&state.pool).await;
    let now = now_secs();

    if !a.symptom {
        if stalled_since.is_some() {
            set_stalled_since(&state.pool, None).await;
            record(&state.pool, "recovered", &a.summary).await;
            tracing::info!("backup service is downloading again");
        }
        return;
    }

    // First sighting: note it and wait. The gap already exceeds the grace
    // period, but a single reading during a slow catch-up should not act.
    let Some(since) = stalled_since else {
        set_stalled_since(&state.pool, Some(now)).await;
        record(&state.pool, "detected", &a.summary).await;
        tracing::warn!("backup service appears stalled: {}", a.summary);
        notify(state, &config, &a.summary).await;
        return;
    };

    if !config.auto_restart {
        return;
    }

    // Confirmed only once the symptom has survived a second grace period —
    // restarting a container is not something to do on one reading.
    if now - since < grace {
        return;
    }
    if let Some(last) = last_restart {
        if now - last < config.restart_cooldown_minutes as f64 * 60.0 {
            return;
        }
    }

    match restart_backup(state).await {
        Ok(name) => {
            set_last_restart(&state.pool, now).await;
            // Cleared so the next tick judges the restart on fresh evidence
            // rather than immediately restarting again.
            set_stalled_since(&state.pool, None).await;
            record(&state.pool, "restarted", &format!("restarted {name}: {}", a.summary)).await;
            tracing::warn!("restarted {name} because downloads had stalled");
            notify(state, &config, &format!("Restarted {name}: {}", a.summary)).await;
        }
        Err(e) => {
            record(&state.pool, "failed", &format!("could not restart: {e}")).await;
            tracing::error!("watchdog could not restart the backup container: {e}");
        }
    }
}

async fn restart_backup(state: &crate::AppState) -> anyhow::Result<String> {
    let Some(docker) = state.docker.as_ref() else {
        anyhow::bail!("docker is unavailable");
    };
    let Some(container) = crate::current_container(state, docker).await? else {
        anyhow::bail!("no backup container found");
    };
    docker
        .restart_container(
            &container.id,
            None::<bollard::query_parameters::RestartContainerOptions>,
        )
        .await?;
    Ok(container.name)
}

/// Falls back to the archive schedule's webhook, so one setting covers both.
async fn notify(state: &crate::AppState, config: &WatchdogConfig, message: &str) {
    let url = match config.webhook_url.clone() {
        Some(u) if !u.trim().is_empty() => Some(u),
        _ => crate::archive::schedule::load(&state.pool)
            .await
            .webhook_url
            .filter(|u| !u.trim().is_empty()),
    };
    let Some(url) = url else { return };

    crate::archive::schedule::notify_failure(&url, 0, message).await;
    record(&state.pool, "notified", message).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: f64 = 1800.0; // 30 minutes

    #[test]
    fn a_quiet_night_is_not_a_stall() {
        // No new events for hours, and everything that was recorded was
        // downloaded. A timeout-based check would restart the service here;
        // this one must not.
        let now = now_secs();
        let a = assess(Some(now - 6.0 * 3600.0), Some(now - 6.0 * 3600.0), 0, GRACE);
        assert!(!a.symptom);
        assert!(a.summary.contains("keeping up"), "{}", a.summary);
    }

    #[test]
    fn events_arriving_without_downloads_is_a_stall() {
        // The failure this exists for: the service keeps seeing events and
        // stops fetching them.
        let now = now_secs();
        let a = assess(Some(now - 60.0), Some(now - 3.0 * 3600.0), 12, GRACE);
        assert!(a.symptom);
        assert!(a.summary.contains("not downloaded"), "{}", a.summary);
    }

    #[test]
    fn a_single_stuck_clip_is_not_a_stall() {
        // One clip failing is a failed clip. Restarting for it would be an
        // overreaction, and the restart would not fix it either.
        let now = now_secs();
        let a = assess(Some(now - 60.0), Some(now - 2.0 * 3600.0), 1, GRACE);
        assert!(!a.symptom);
    }

    #[test]
    fn a_normal_download_delay_is_not_a_stall() {
        // Clips are fetched shortly after the event ends, so the newest event
        // is routinely a few minutes ahead of the newest clip.
        let now = now_secs();
        let a = assess(Some(now - 30.0), Some(now - 300.0), 3, GRACE);
        assert!(!a.symptom);
        assert!(a.summary.contains("Within normal delay"), "{}", a.summary);
    }

    #[test]
    fn recording_but_never_downloading_anything_is_a_stall() {
        // A service that has never managed a single download — misconfigured
        // credentials, say — has no "newest captured" to compare against.
        let now = now_secs();
        let a = assess(Some(now - 2.0 * 3600.0), None, 20, GRACE);
        assert!(a.symptom);
    }

    #[test]
    fn an_empty_index_claims_nothing() {
        let a = assess(None, None, 0, GRACE);
        assert!(!a.symptom);
        assert!(a.summary.contains("No events"), "{}", a.summary);
    }
}

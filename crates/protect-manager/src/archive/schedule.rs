//! Running archives on a schedule, and failing loudly when they don't.
//!
//! This is the part that replaces cron, so the bar is not "it usually fires".
//! A schedule that silently stops is the failure this whole app exists to
//! prevent, which is why the app records every run, catches up after downtime,
//! and can push a failure somewhere you'll actually see it.
//!
//! **Times are UTC.** Deriving a machine's local zone reliably inside a
//! container is more trouble than it is worth, so the browser — which knows
//! the viewer's zone for certain — converts when showing and saving. The
//! consequence is that a scheduled hour drifts by one across daylight saving;
//! for a monthly job that is not worth engineering around.

use protect_api_types::{Schedule, ScheduleKind};
use sqlx::{Row, SqlitePool};

use super::plan::civil_from_days;

pub const DEFAULT: Schedule = Schedule {
    kind: ScheduleKind::Off,
    day: 1,
    hour: 3,
    catch_up: true,
    webhook_url: None,
    next_run: None,
    last_run: None,
};

pub async fn load(pool: &SqlitePool) -> Schedule {
    let row = sqlx::query("SELECT json, last_run FROM schedule WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    let mut schedule = row
        .as_ref()
        .and_then(|r| serde_json::from_str::<Schedule>(&r.get::<String, _>("json")).ok())
        .unwrap_or(DEFAULT);

    schedule.last_run = row.as_ref().and_then(|r| r.get("last_run"));
    schedule.next_run = next_run_after(&schedule, crate::upb::reconcile::now_secs());
    schedule
}

pub async fn save(pool: &SqlitePool, schedule: &Schedule) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO schedule (id, json) VALUES (1, ?)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json",
    )
    .bind(serde_json::to_string(schedule)?)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_ran(pool: &SqlitePool, at: f64) {
    let _ = sqlx::query(
        "INSERT INTO schedule (id, json, last_run) VALUES (1, '{}', ?)
         ON CONFLICT(id) DO UPDATE SET last_run = excluded.last_run",
    )
    .bind(at)
    .execute(pool)
    .await;
}

/// Whether a run is owed right now.
///
/// Owed rather than "is it exactly the scheduled minute": if the app was down
/// when the slot passed, the work is still due. A late archive is strictly
/// better than a skipped one, and skipping is what cron was doing.
pub fn is_due(schedule: &Schedule, now: f64) -> bool {
    if matches!(schedule.kind, ScheduleKind::Off) {
        return false;
    }
    let Some(slot) = last_slot_before(schedule, now) else {
        return false;
    };
    match schedule.last_run {
        Some(last) => last < slot,
        // Never run before. With catch-up on, the first slot that has already
        // passed counts; otherwise wait for the next one.
        None => schedule.catch_up,
    }
}

/// Most recent scheduled moment at or before `now`.
fn last_slot_before(schedule: &Schedule, now: f64) -> Option<f64> {
    let hour_secs = (schedule.hour.min(23) as f64) * 3600.0;
    let day = (now / 86_400.0).floor();

    match schedule.kind {
        ScheduleKind::Off => None,
        ScheduleKind::Daily => {
            let today = day * 86_400.0 + hour_secs;
            Some(if today <= now { today } else { today - 86_400.0 })
        }
        ScheduleKind::Monthly => {
            let (y, m) = civil_from_days(day as i64);
            let candidate = month_slot(y, m, schedule.day, hour_secs);
            if candidate <= now {
                Some(candidate)
            } else {
                let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
                Some(month_slot(py, pm, schedule.day, hour_secs))
            }
        }
    }
}

/// Next scheduled moment strictly after `now`.
pub fn next_run_after(schedule: &Schedule, now: f64) -> Option<f64> {
    let hour_secs = (schedule.hour.min(23) as f64) * 3600.0;
    let day = (now / 86_400.0).floor();

    match schedule.kind {
        ScheduleKind::Off => None,
        ScheduleKind::Daily => {
            let today = day * 86_400.0 + hour_secs;
            Some(if today > now { today } else { today + 86_400.0 })
        }
        ScheduleKind::Monthly => {
            let (y, m) = civil_from_days(day as i64);
            let this = month_slot(y, m, schedule.day, hour_secs);
            if this > now {
                Some(this)
            } else {
                let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                Some(month_slot(ny, nm, schedule.day, hour_secs))
            }
        }
    }
}

/// A day-of-month slot, clamped to months that are shorter than the chosen
/// day — day 31 in February means the last day of February, not a skipped run.
fn month_slot(year: i64, month: u32, day: u32, hour_secs: f64) -> f64 {
    let dim = days_in_month(year, month);
    let d = day.clamp(1, dim);
    super::plan::days_from_civil(year, month, d) as f64 * 86_400.0 + hour_secs
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
    }
}

/// Tell someone a scheduled run failed, without needing a browser open.
///
/// A plain POST, so it works with Home Assistant, ntfy, a Discord webhook, or
/// anything else that accepts JSON. Failures here are logged and dropped: a
/// notification that can't be delivered must not turn into a second failure.
pub async fn notify_failure(url: &str, run_id: i64, message: &str) {
    let body = serde_json::json!({
        "source": "protect-manager",
        "event": "archive_failed",
        "run_id": run_id,
        "message": message,
    });

    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        path = url_path(url),
        host = url_host(url),
        len = body.to_string().len(),
        body = body,
    );

    let Some(addr) = url_addr(url) else {
        tracing::error!("webhook url is not usable: {url}");
        return;
    };

    match tokio::net::TcpStream::connect(&addr).await {
        Ok(mut stream) => {
            use tokio::io::AsyncWriteExt;
            if let Err(e) = stream.write_all(request.as_bytes()).await {
                tracing::error!("could not send failure webhook: {e}");
            }
        }
        Err(e) => tracing::error!("could not reach webhook {addr}: {e}"),
    }
}

fn url_host(url: &str) -> &str {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    rest.split('/').next().unwrap_or(rest)
}

fn url_path(url: &str) -> &str {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    match rest.find('/') {
        Some(i) => &rest[i..],
        None => "/",
    }
}

fn url_addr(url: &str) -> Option<String> {
    if !url.starts_with("http://") {
        // Plain HTTP only, deliberately: this runs on a private network, and
        // implementing TLS here would mean pulling in a client stack for one
        // fire-and-forget request.
        return None;
    }
    let host = url_host(url);
    Some(if host.contains(':') { host.to_string() } else { format!("{host}:80") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monthly(day: u32, hour: u32) -> Schedule {
        Schedule { kind: ScheduleKind::Monthly, day, hour, ..DEFAULT }
    }

    /// 2026-08-15 12:00 UTC
    const NOW: f64 = 1_786_881_600.0;

    #[test]
    fn a_monthly_schedule_lands_on_the_right_day() {
        let next = next_run_after(&monthly(1, 3), NOW).unwrap();
        // Next 1st at 03:00 is September.
        assert_eq!(next, days(2026, 9, 1) * 86_400.0 + 3.0 * 3600.0);

        // A day later this month is still this month.
        let next = next_run_after(&monthly(20, 3), NOW).unwrap();
        assert_eq!(next, days(2026, 8, 20) * 86_400.0 + 3.0 * 3600.0);
    }

    #[test]
    fn day_31_still_runs_in_a_short_month() {
        // February has no 31st; the run must happen on the last day rather
        // than being skipped for the month.
        let feb = days(2026, 2, 10) * 86_400.0;
        let next = next_run_after(&monthly(31, 3), feb).unwrap();
        assert_eq!(next, days(2026, 2, 28) * 86_400.0 + 3.0 * 3600.0);
    }

    #[test]
    fn a_missed_slot_is_still_owed() {
        // The defining behaviour: the app was down when the slot passed, so
        // the work is due now rather than in another month.
        let mut s = monthly(1, 3);
        s.last_run = Some(days(2026, 7, 1) * 86_400.0);
        assert!(is_due(&s, NOW), "August's run was missed and is still owed");

        s.last_run = Some(days(2026, 8, 1) * 86_400.0 + 4.0 * 3600.0);
        assert!(!is_due(&s, NOW), "August already ran");
    }

    #[test]
    fn catch_up_decides_what_happens_on_a_first_run() {
        let mut s = monthly(1, 3);
        s.last_run = None;
        assert!(is_due(&s, NOW), "with catch-up, a passed slot counts");

        s.catch_up = false;
        assert!(!is_due(&s, NOW), "without catch-up, wait for the next slot");
    }

    #[test]
    fn an_off_schedule_is_never_due() {
        let s = Schedule { kind: ScheduleKind::Off, ..DEFAULT };
        assert!(!is_due(&s, NOW));
        assert_eq!(next_run_after(&s, NOW), None);
    }

    #[test]
    fn a_daily_schedule_advances_by_a_day() {
        let s = Schedule { kind: ScheduleKind::Daily, hour: 3, ..DEFAULT };
        let next = next_run_after(&s, NOW).unwrap();
        // 03:00 already passed at 12:00, so it's tomorrow.
        assert_eq!(next, (NOW / 86_400.0).floor() * 86_400.0 + 86_400.0 + 3.0 * 3600.0);
    }

    #[test]
    fn webhook_urls_are_parsed_or_rejected() {
        assert_eq!(url_addr("http://ha.local:8123/api/webhook/x").as_deref(), Some("ha.local:8123"));
        assert_eq!(url_path("http://ha.local:8123/api/webhook/x"), "/api/webhook/x");
        assert_eq!(url_addr("http://ha.local/hook").as_deref(), Some("ha.local:80"));
        assert_eq!(url_addr("https://ha.local/hook"), None);
    }

    fn days(y: i64, m: u32, d: u32) -> f64 {
        super::super::plan::days_from_civil(y, m, d) as f64
    }
}

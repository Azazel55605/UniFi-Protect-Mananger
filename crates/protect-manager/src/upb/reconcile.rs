//! Keeping our index in step with the backup service's database.
//!
//! We hold our own copy of the events rather than querying theirs per request.
//! That is not caching for speed — it is because the useful fields don't exist
//! upstream. Camera names, detection subtypes, clip presence and duration all
//! have to be derived, and derived data needs somewhere to live that we can
//! index and filter on.

use std::path::Path;

use protect_api_types::{ClipStatus, Settings};
use sqlx::SqlitePool;

use crate::upb::{parse, read};

pub struct SyncOutcome {
    pub events: usize,
    pub cameras: usize,
    pub statted: usize,
}

/// Read the upstream database whole and rewrite our index from it.
pub async fn sync(
    ours: &SqlitePool,
    settings: &Settings,
    backup_dir: &Path,
    missing_range_secs: Option<f64>,
) -> anyhow::Result<SyncOutcome> {
    let Some(db_path) = settings.events_db_path.as_deref() else {
        anyhow::bail!("no event database configured");
    };
    let prefix = settings.clip_path_prefix.as_deref().unwrap_or("");

    let upstream = read::open(Path::new(db_path)).await?;
    let rows = read::read_all(&upstream).await?;
    upstream.close().await;

    let now = now_secs();
    // Only clips inside the live window can still be on disk; everything older
    // has been archived by definition. Checking the whole history would mean a
    // stat() per event on every sync, which grows without bound for an answer
    // we already know.
    let live_cutoff = live_window_cutoff(now, settings.live_window_months);

    let mut statted = 0usize;
    let mut records = Vec::with_capacity(rows.len());

    for row in &rows {
        let parsed = row.path.as_deref().map(|p| parse::parse(p, prefix));

        let local_path = row
            .path
            .as_deref()
            .and_then(|p| parse::strip_prefix(p, prefix))
            .map(|rel| backup_dir.join(rel));

        let (status, size) = match (&row.path, &local_path) {
            // Never backed up. Whether that is still recoverable depends on how
            // far back the backup service will backfill — two very different
            // pieces of news that a single "missing" count would blur.
            (None, _) => {
                let recoverable = missing_range_secs
                    .map(|w| now - row.start <= w)
                    .unwrap_or(false);
                (
                    if recoverable {
                        ClipStatus::PendingBackfill
                    } else {
                        ClipStatus::NeverBackedUp
                    },
                    None,
                )
            }
            (Some(_), Some(path)) if row.start >= live_cutoff => {
                statted += 1;
                match std::fs::metadata(path) {
                    Ok(m) => (ClipStatus::Live, Some(m.len() as i64)),
                    // Inside the live window and absent: something removed it
                    // out of band. Not the same as archived, and worth saying.
                    Err(_) => (ClipStatus::Vanished, None),
                }
            }
            (Some(_), _) => (ClipStatus::Archived, None),
        };

        records.push(IndexRow {
            id: row.id.clone(),
            camera_id: row.camera_id.clone(),
            camera_name: parsed.as_ref().and_then(|p| p.camera_name.clone()),
            // The database's own type is authoritative; the filename's copy is
            // only a fallback for a row whose type column is empty.
            event_type: if row.event_type.is_empty() {
                parsed.as_ref().and_then(|p| p.event_type.clone()).unwrap_or_default()
            } else {
                row.event_type.clone()
            },
            subtypes: parsed.map(|p| p.subtypes).unwrap_or_default(),
            start: row.start,
            end: row.end,
            status,
            clip_path: local_path.map(|p| p.to_string_lossy().to_string()),
            size_bytes: size,
        });
    }

    let cameras = write_index(ours, &records, now).await?;
    Ok(SyncOutcome { events: records.len(), cameras, statted })
}

struct IndexRow {
    id: String,
    camera_id: String,
    camera_name: Option<String>,
    event_type: String,
    subtypes: Vec<String>,
    start: f64,
    end: f64,
    status: ClipStatus,
    clip_path: Option<String>,
    size_bytes: Option<i64>,
}

/// Replace the index contents in one transaction.
///
/// A full replace rather than an incremental merge: it is the only way to
/// notice rows that disappeared upstream, and at this data volume it costs
/// milliseconds. The transaction means readers never see a half-built index.
async fn write_index(
    pool: &SqlitePool,
    rows: &[IndexRow],
    now: f64,
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM events").execute(&mut *tx).await?;

    for r in rows {
        sqlx::query(
            "INSERT INTO events
                (id, camera_id, camera_name, event_type, subtypes, start, end,
                 duration, status, clip_path, size_bytes)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&r.id)
        .bind(&r.camera_id)
        .bind(&r.camera_name)
        .bind(&r.event_type)
        // Space-padded so `LIKE '% person %'` matches a whole token and never
        // a prefix of a longer one.
        .bind(format!(" {} ", r.subtypes.join(" ")))
        .bind(r.start)
        .bind(r.end)
        .bind((r.end - r.start).max(0.0))
        .bind(status_str(r.status))
        .bind(&r.clip_path)
        .bind(r.size_bytes)
        .execute(&mut *tx)
        .await?;
    }

    // Camera identities are derived from paths, so they are rebuilt from the
    // same pass — but any display name the user set is theirs and survives.
    sqlx::query(
        "INSERT INTO cameras (camera_id, derived_name)
         SELECT camera_id, MAX(camera_name) FROM events
          WHERE camera_id != '' GROUP BY camera_id
         ON CONFLICT(camera_id) DO UPDATE
            SET derived_name = COALESCE(excluded.derived_name, cameras.derived_name)",
    )
    .execute(&mut *tx)
    .await?;

    let cameras: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras")
        .fetch_one(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO index_state (id, last_sync, last_error) VALUES (1, ?, NULL)
         ON CONFLICT(id) DO UPDATE SET last_sync = excluded.last_sync, last_error = NULL",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(cameras as usize)
}

pub fn status_str(s: ClipStatus) -> &'static str {
    match s {
        ClipStatus::Live => "live",
        ClipStatus::Archived => "archived",
        ClipStatus::Vanished => "vanished",
        ClipStatus::PendingBackfill => "pending_backfill",
        ClipStatus::NeverBackedUp => "never_backed_up",
    }
}

pub fn status_from_str(s: &str) -> ClipStatus {
    match s {
        "live" => ClipStatus::Live,
        "archived" => ClipStatus::Archived,
        "vanished" => ClipStatus::Vanished,
        "pending_backfill" => ClipStatus::PendingBackfill,
        _ => ClipStatus::NeverBackedUp,
    }
}

pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Approximate start of the live window.
///
/// Archives are whole calendar months, so the boundary is fuzzy by design —
/// "N months" is a floor, not an exact retention period. 30.5 days per month
/// is close enough for deciding whether a file is worth looking for on disk.
fn live_window_cutoff(now: f64, months: u32) -> f64 {
    if months == 0 {
        return f64::NEG_INFINITY;
    }
    now - (months as f64 * 30.5 * 86_400.0)
}

/// Convert the backup service's `--missing-range` (`30d`, `12h`) to seconds.
pub fn parse_duration_secs(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    let split = raw.find(|c: char| !c.is_ascii_digit())?;
    let n: f64 = raw[..split].parse().ok()?;
    match &raw[split..] {
        "d" => Some(n * 86_400.0),
        "h" => Some(n * 3_600.0),
        "m" => Some(n * 60.0),
        "s" => Some(n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_backfill_windows() {
        assert_eq!(parse_duration_secs("30d"), Some(2_592_000.0));
        assert_eq!(parse_duration_secs("12h"), Some(43_200.0));
        assert_eq!(parse_duration_secs("90"), None);
        assert_eq!(parse_duration_secs("forever"), None);
    }

    #[test]
    fn a_zero_live_window_never_treats_a_clip_as_archived() {
        // Before setup completes the window is 0. Reporting every clip as
        // archived at that point would be alarming and wrong.
        assert_eq!(live_window_cutoff(1_000_000.0, 0), f64::NEG_INFINITY);
        assert!(live_window_cutoff(1_000_000.0, 2) < 1_000_000.0);
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use crate::upb::read::fixture;
    use protect_api_types::Settings;

    /// A backup root with two cameras, a service directory that must not be
    /// mistaken for one, and one clip deliberately absent from disk.
    async fn scaffold(name: &str) -> (std::path::PathBuf, SqlitePool, Settings) {
        let root = std::env::temp_dir().join(format!("pm-sync-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let backup = root.join("backup");
        std::fs::create_dir_all(backup.join("database")).unwrap();

        let now = now_secs();
        let day = "2026-08-15";
        let mut rows: Vec<(String, String)> = Vec::new();
        for (cam, file) in [
            ("Front Door", "2026-08-15T10-00-00 smartDetectZone (person).mp4"),
            ("Gartenhäuschen", "2026-08-15T10-05-00 smartDetectZone (animal person).mp4"),
            ("Front Door", "2026-08-15T10-10-00 smartAudioDetect (alrmSpeak).mp4"),
        ] {
            let dir = backup.join(cam).join(day);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), b"clip").unwrap();
            rows.push((cam.to_string(), file.to_string()));
        }

        let db = backup.join("database").join("events.sqlite");
        let p = |i: usize| format!("/data/{}/{}/{}", rows[i].0, day, rows[i].1);
        fixture::create(
            &db,
            &[
                ("e1", "smartDetectZone", "cam1", now - 60.0, now - 45.0, Some(&p(0))),
                ("e2", "smartDetectZone", "cam2", now - 120.0, now - 100.0, Some(&p(1))),
                ("e3", "smartAudioDetect", "cam1", now - 180.0, now - 160.0, Some(&p(2))),
                // Backed up according to the database, but the file is not
                // there — removed out of band.
                ("e4", "smartDetectZone", "cam1", now - 240.0, now - 230.0,
                 Some("/data/Front Door/2026-08-15/2026-08-15T09-00-00 smartDetectZone (person).mp4")),
                // Never captured, recent: the backup service may still fetch it.
                ("e5", "smartDetectZone", "cam1", now - 300.0, now - 280.0, None),
                // Never captured, long ago: past any backfill window.
                ("e6", "smartDetectZone", "cam2", now - 400.0 * 86_400.0, now - 400.0 * 86_400.0 + 10.0, None),
            ],
        )
        .await;

        let ours = crate::db::connect(&root.join("state")).await.unwrap();
        let settings = Settings {
            events_db_path: Some(db.to_string_lossy().to_string()),
            clip_path_prefix: Some("/data".into()),
            live_window_months: 2,
            setup_complete: true,
            ..Default::default()
        };
        (backup, ours, settings)
    }

    #[tokio::test]
    async fn indexes_events_and_classifies_every_clip() {
        let (backup, ours, settings) = scaffold("classify").await;

        // A 30-day backfill window, as the backup service commonly runs with.
        let outcome = sync(&ours, &settings, &backup, Some(30.0 * 86_400.0)).await.unwrap();
        assert_eq!(outcome.events, 6);
        assert_eq!(outcome.cameras, 2);

        let stats = crate::events::stats(&ours).await.unwrap();
        assert_eq!(stats.total_events, 6);
        assert_eq!(stats.live_clips, 3);
        assert_eq!(stats.vanished, 1, "a clip missing inside the live window");
        assert_eq!(stats.pending_backfill, 1, "recent, still recoverable");
        assert_eq!(stats.never_backed_up, 1, "old enough that it never will be");

        // Detection types come only from filenames; the database has none.
        assert_eq!(stats.distinct_subtypes, vec!["alrmSpeak", "animal", "person"]);
    }

    #[tokio::test]
    async fn filters_by_camera_and_detection_type() {
        let (backup, ours, settings) = scaffold("filter").await;
        sync(&ours, &settings, &backup, Some(30.0 * 86_400.0)).await.unwrap();

        let by_camera = crate::events::query(
            &ours,
            &protect_api_types::EventQuery {
                camera_id: Some("cam2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(by_camera.total, 2);
        // Camera names are derived from the clip path, not the database.
        assert_eq!(by_camera.events[0].camera, "Gartenhäuschen");

        let animals = crate::events::query(
            &ours,
            &protect_api_types::EventQuery { subtype: Some("animal".into()), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(animals.total, 1);
        assert_eq!(animals.events[0].subtypes, vec!["animal", "person"]);

        // "person" must match the multi-detection event too, not just the
        // events whose only detection is person.
        let people = crate::events::query(
            &ours,
            &protect_api_types::EventQuery { subtype: Some("person".into()), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(people.total, 3);
    }

    #[tokio::test]
    async fn resyncing_is_idempotent_and_keeps_user_chosen_names() {
        let (backup, ours, settings) = scaffold("idempotent").await;
        sync(&ours, &settings, &backup, None).await.unwrap();

        sqlx::query("UPDATE cameras SET display_name = 'Porch' WHERE camera_id = 'cam1'")
            .execute(&ours)
            .await
            .unwrap();

        sync(&ours, &settings, &backup, None).await.unwrap();

        let stats = crate::events::stats(&ours).await.unwrap();
        assert_eq!(stats.total_events, 6, "a resync must not duplicate rows");

        let cams = crate::events::cameras(&ours).await.unwrap();
        let cam1 = cams.iter().find(|c| c.camera_id == "cam1").unwrap();
        assert_eq!(cam1.display_name, "Porch", "a chosen name must survive a resync");
        assert_eq!(cam1.derived_name.as_deref(), Some("Front Door"));
    }

    #[tokio::test]
    async fn backup_lag_reflects_the_newest_captured_clip() {
        let (backup, ours, settings) = scaffold("lag").await;
        sync(&ours, &settings, &backup, None).await.unwrap();

        let stats = crate::events::stats(&ours).await.unwrap();
        // The newest event with a clip is ~60s old; the newest event overall
        // is the same one. Lag should be small, and never negative.
        let lag = stats.backup_lag_secs.expect("lag is computable");
        assert!((50.0..300.0).contains(&lag), "unexpected lag: {lag}");
    }
}

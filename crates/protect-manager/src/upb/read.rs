//! Reading the backup service's own database.
//!
//! That file belongs to another process which is actively writing it, so the
//! rules here are narrow and deliberate:
//!
//! * **Read-only, always.** We never write to it, and open it as `mode=ro` so
//!   a bug can't.
//! * **Never on the request path.** It uses a rollback journal rather than
//!   WAL, so a reader can block briefly during a write, and a read-only
//!   connection cannot recover a hot journal left by a crash. Both are fine on
//!   a background timer with retries; neither is fine while a user waits.
//! * **Read it whole, diff afterwards.** See `reconcile`.

use std::path::Path;

use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

/// One row of the backup service's `events`, joined to its backup path.
#[derive(Debug, Clone, Serialize)]
pub struct UpbEvent {
    pub id: String,
    pub event_type: String,
    pub camera_id: String,
    pub start: f64,
    pub end: f64,
    /// Absent when the event was recorded but never backed up.
    pub path: Option<String>,
}

pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    if !path.is_file() {
        anyhow::bail!("{} is not a file", path.display());
    }

    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        // Long enough to outlast a write, short enough that a stuck sync is
        // reported rather than hanging the background task forever.
        .busy_timeout(std::time::Duration::from_secs(10));

    // A single connection: we read this file on a timer, and concurrency
    // against someone else's database buys nothing.
    Ok(SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?)
}

/// Read every event and its backup path.
///
/// A left join, deliberately: an event with no backup row is not noise, it is
/// the signal that footage was recorded and never captured. Dropping those
/// rows would hide the most important failure this app can report.
///
/// Reading the whole table rather than only new rows is also deliberate. The
/// upstream schema has no index on `start`, so a "newer than" query scans
/// every row anyway — and backup rows are written *after* their event, so a
/// high-water mark would permanently miss clips that arrived late.
pub async fn read_all(pool: &SqlitePool) -> anyhow::Result<Vec<UpbEvent>> {
    let rows = sqlx::query(
        "SELECT e.id, e.type, e.camera_id, e.start, e.end, b.path
           FROM events e
           LEFT JOIN backups b ON b.id = e.id
          ORDER BY e.start DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UpbEvent {
            id: r.get::<String, _>("id"),
            // Every column but `id` is untyped in the upstream schema, so a
            // NULL is possible in principle. Default rather than fail the
            // whole sync over one odd row.
            event_type: r.try_get::<String, _>("type").unwrap_or_default(),
            camera_id: r.try_get::<String, _>("camera_id").unwrap_or_default(),
            start: r.try_get::<f64, _>("start").unwrap_or(0.0),
            end: r.try_get::<f64, _>("end").unwrap_or(0.0),
            path: r.try_get::<Option<String>, _>("path").unwrap_or(None),
        })
        .collect())
}

#[cfg(test)]
pub mod fixture {
    //! Builds a database shaped like the backup service's own, so tests run
    //! against the real schema without shipping anyone's actual recordings.
    use super::*;

    /// `(id, type, camera_id, start, end, backup path)`
    pub type Row<'a> = (&'a str, &'a str, &'a str, f64, f64, Option<&'a str>);

    pub async fn create(path: &Path, rows: &[Row<'_>]) {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();

        // Verbatim upstream schema, including the absence of any index.
        sqlx::query("CREATE TABLE events(id PRIMARY KEY, type, camera_id, start REAL, end REAL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE backups(id REFERENCES events(id) ON DELETE CASCADE, remote, path,
                                  PRIMARY KEY (id, remote))",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (id, ty, cam, start, end, path) in rows {
            sqlx::query("INSERT INTO events VALUES (?, ?, ?, ?, ?)")
                .bind(id)
                .bind(ty)
                .bind(cam)
                .bind(start)
                .bind(end)
                .execute(&pool)
                .await
                .unwrap();
            if let Some(p) = path {
                sqlx::query("INSERT INTO backups VALUES (?, 'local', ?)")
                    .bind(id)
                    .bind(p)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_events_including_ones_never_backed_up() {
        let dir = std::env::temp_dir().join(format!("pm-upb-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("events.sqlite");

        fixture::create(
            &db,
            &[
                ("a", "smartDetectZone", "cam1", 100.0, 110.0, Some("/data/Cam/d/a.mp4")),
                // No backup row: recorded, never captured.
                ("b", "smartDetectZone", "cam1", 200.0, 215.0, None),
                // Upstream uses two id formats; neither is a UUID we can rely on.
                ("6a6c437503171103e41d0007", "smartAudioDetect", "cam2", 300.0, 316.0, None),
            ],
        )
        .await;

        let pool = open(&db).await.unwrap();
        let events = read_all(&pool).await.unwrap();

        assert_eq!(events.len(), 3);
        // Newest first.
        assert_eq!(events[0].id, "6a6c437503171103e41d0007");
        assert_eq!(events[2].path.as_deref(), Some("/data/Cam/d/a.mp4"));
        assert!(events[1].path.is_none(), "un-backed-up events must survive the read");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_a_path_that_is_not_a_file() {
        let missing = std::env::temp_dir().join("pm-definitely-not-here.sqlite");
        assert!(open(&missing).await.is_err());
    }
}

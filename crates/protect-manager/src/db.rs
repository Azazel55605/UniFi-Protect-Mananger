//! Our own SQLite: settings and sessions.
//!
//! Deliberately separate from UPB's `events.sqlite`, which we only ever open
//! read-only. This is the database we own and write.

use std::path::Path;

use protect_api_types::Settings;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

pub async fn connect(state_dir: &Path) -> anyhow::Result<SqlitePool> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join("protect-manager.sqlite");

    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        // WAL so a long read never blocks a write. (UPB's own database uses a
        // rollback journal, which is one reason we never query it directly on
        // the request path — but that is its choice, not ours.)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));

    let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
    migrate(&pool).await?;
    tracing::info!("state database at {}", path.display());
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    // Settings are stored as one JSON document rather than a column per
    // setting: the shape is still moving, and `Settings` in the API crate is
    // the single definition of it. Columns arrive when something needs to be
    // queried by, which nothing here does.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS settings (
            id      INTEGER PRIMARY KEY CHECK (id = 1),
            json    TEXT NOT NULL,
            updated INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sessions (
            token   TEXT PRIMARY KEY,
            created INTEGER NOT NULL,
            expires INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS sessions_expires ON sessions (expires)")
        .execute(pool)
        .await?;

    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ------------------------------------------------------------- settings

pub async fn load_settings(pool: &SqlitePool) -> anyhow::Result<Settings> {
    let row = sqlx::query("SELECT json FROM settings WHERE id = 1")
        .fetch_optional(pool)
        .await?;

    match row {
        Some(r) => {
            let raw: String = r.get("json");
            match serde_json::from_str(&raw) {
                Ok(s) => Ok(s),
                Err(e) => {
                    // A settings document we can't parse is a bug in us, not a
                    // reason to refuse to start — the setup flow can rebuild it.
                    tracing::error!("stored settings are unreadable, falling back to empty: {e}");
                    Ok(Settings::default())
                }
            }
        }
        None => Ok(Settings::default()),
    }
}

pub async fn save_settings(pool: &SqlitePool, settings: &Settings) -> anyhow::Result<()> {
    let json = serde_json::to_string(settings)?;
    sqlx::query(
        "INSERT INTO settings (id, json, updated) VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json, updated = excluded.updated",
    )
    .bind(json)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

// ------------------------------------------------------------- sessions

pub async fn create_session(pool: &SqlitePool, token: &str, ttl_secs: i64) -> anyhow::Result<()> {
    let now = now();
    sqlx::query("INSERT INTO sessions (token, created, expires) VALUES (?, ?, ?)")
        .bind(token)
        .bind(now)
        .bind(now + ttl_secs)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn session_valid(pool: &SqlitePool, token: &str) -> bool {
    match sqlx::query("SELECT expires FROM sessions WHERE token = ?")
        .bind(token)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(row)) => {
            let expires: i64 = row.get("expires");
            expires > now()
        }
        Ok(None) => false,
        Err(e) => {
            // Fail closed: a database error must not become an open door.
            tracing::error!("session lookup failed: {e}");
            false
        }
    }
}

pub async fn revoke_session(pool: &SqlitePool, token: &str) {
    if let Err(e) = sqlx::query("DELETE FROM sessions WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await
    {
        tracing::error!("failed to revoke session: {e}");
    }
}

/// Drop expired sessions. Called at startup; cheap enough not to schedule.
pub async fn purge_expired_sessions(pool: &SqlitePool) {
    match sqlx::query("DELETE FROM sessions WHERE expires <= ?")
        .bind(now())
        .execute(pool)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!("purged {} expired sessions", r.rows_affected())
        }
        Ok(_) => {}
        Err(e) => tracing::error!("failed to purge sessions: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One database per test. Tests run concurrently in the same process, so a
    /// shared path means they contend for SQLite's write lock and fail in a way
    /// that looks like a bug in the code rather than in the test setup.
    async fn pool(name: &str) -> SqlitePool {
        let dir = std::env::temp_dir().join(format!("pm-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        connect(&dir).await.unwrap()
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let pool = pool("settings").await;
        assert_eq!(load_settings(&pool).await.unwrap().live_window_months, 0);

        let mut s = Settings {
            live_window_months: 3,
            camera_dirs: vec!["Front Door".into(), "Gartenhäuschen".into()],
            ..Default::default()
        };
        save_settings(&pool, &s).await.unwrap();

        let loaded = load_settings(&pool).await.unwrap();
        assert_eq!(loaded.live_window_months, 3);
        // Non-ASCII camera names must survive the round trip.
        assert_eq!(loaded.camera_dirs[1], "Gartenhäuschen");

        // Saving again updates rather than inserting a second row.
        s.live_window_months = 6;
        save_settings(&pool, &s).await.unwrap();
        assert_eq!(load_settings(&pool).await.unwrap().live_window_months, 6);
    }

    #[tokio::test]
    async fn sessions_expire_and_revoke() {
        let pool = pool("sessions").await;

        create_session(&pool, "live", 3600).await.unwrap();
        assert!(session_valid(&pool, "live").await);

        create_session(&pool, "stale", -1).await.unwrap();
        assert!(!session_valid(&pool, "stale").await);

        revoke_session(&pool, "live").await;
        assert!(!session_valid(&pool, "live").await);

        assert!(!session_valid(&pool, "never-existed").await);
    }
}

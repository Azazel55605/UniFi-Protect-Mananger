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

    // Added after the table existed in the field, so they arrive by ALTER
    // rather than by changing the CREATE above — which SQLite would ignore for
    // anyone who already has the database.
    for (table, column, decl) in [
        ("sessions", "last_seen", "INTEGER"),
        ("sessions", "user_agent", "TEXT"),
        ("sessions", "address", "TEXT"),
    ] {
        add_column_if_missing(pool, table, column, decl).await?;
    }

    // The event index. This is a derived copy of the backup service's data,
    // enriched with everything it doesn't store: camera names, detection
    // subtypes, clip presence and size. Safe to drop and rebuild at any time.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS events (
            id          TEXT PRIMARY KEY,
            camera_id   TEXT NOT NULL,
            camera_name TEXT,
            event_type  TEXT NOT NULL,
            subtypes    TEXT NOT NULL DEFAULT '',
            start       REAL NOT NULL,
            end         REAL NOT NULL,
            duration    REAL NOT NULL,
            status      TEXT NOT NULL,
            clip_path   TEXT,
            size_bytes  INTEGER
        )",
    )
    .execute(pool)
    .await?;

    // The upstream database has no index at all, which is why time-range
    // queries there scan the table. Ours does not have that excuse.
    for stmt in [
        "CREATE INDEX IF NOT EXISTS events_start ON events (start DESC)",
        "CREATE INDEX IF NOT EXISTS events_camera_start ON events (camera_id, start DESC)",
        "CREATE INDEX IF NOT EXISTS events_type_start ON events (event_type, start DESC)",
        "CREATE INDEX IF NOT EXISTS events_status ON events (status)",
    ] {
        sqlx::query(stmt).execute(pool).await?;
    }

    // Camera identity is derived from clip paths, but a display name the user
    // chose is theirs — kept in a separate column so a resync cannot erase it.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cameras (
            camera_id    TEXT PRIMARY KEY,
            derived_name TEXT,
            display_name TEXT
        )",
    )
    .execute(pool)
    .await?;

    // One row per archive we know about, keyed by the camera-month it holds.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS archives (
            camera      TEXT NOT NULL,
            month       TEXT NOT NULL,
            path        TEXT NOT NULL,
            size_bytes  INTEGER NOT NULL DEFAULT 0,
            file_count  INTEGER NOT NULL DEFAULT 0,
            created     REAL,
            verified_at REAL,
            verify_ok   INTEGER,
            -- Restored back to live. The scheduler skips pinned months, or it
            -- would re-archive them on its next pass and undo the restore.
            pinned      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (camera, month)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS archive_runs (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            kind         TEXT NOT NULL,
            status       TEXT NOT NULL,
            camera       TEXT,
            month        TEXT,
            started      REAL NOT NULL,
            finished     REAL,
            dry_run      INTEGER NOT NULL DEFAULT 0,
            scheduled    INTEGER NOT NULL DEFAULT 0,
            files_total  INTEGER NOT NULL DEFAULT 0,
            files_done   INTEGER NOT NULL DEFAULT 0,
            bytes_total  INTEGER NOT NULL DEFAULT 0,
            message      TEXT,
            failed_files TEXT NOT NULL DEFAULT ''
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS runs_started ON archive_runs (started DESC)")
        .execute(pool)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schedule (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            json        TEXT NOT NULL,
            last_run    REAL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS watchdog (
            id            INTEGER PRIMARY KEY CHECK (id = 1),
            json          TEXT NOT NULL,
            stalled_since REAL,
            last_restart  REAL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS watchdog_log (
            at     REAL NOT NULL,
            action TEXT NOT NULL,
            detail TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS storage_samples (
            at            REAL PRIMARY KEY,
            live_bytes    INTEGER NOT NULL,
            archive_bytes INTEGER NOT NULL,
            free_bytes    INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS index_state (
            id         INTEGER PRIMARY KEY CHECK (id = 1),
            last_sync  REAL,
            last_error TEXT
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Add a column to an existing table, if it is not already there.
///
/// SQLite has no `ADD COLUMN IF NOT EXISTS`, and `CREATE TABLE IF NOT EXISTS`
/// silently does nothing when the table exists — so a column added to the
/// schema above would never appear for anyone who already ran an older build.
/// Asking `PRAGMA table_info` first is what makes the migration idempotent.
async fn add_column_if_missing(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    decl: &str,
) -> anyhow::Result<()> {
    // `AssertSqlSafe` because the statement is assembled at runtime, which sqlx
    // refuses by default. The assertion holds: every `table`, `column` and
    // `decl` passed here is a literal written a few lines above, and none of
    // them comes from a request. This is the one place in the file where SQL is
    // built rather than written out, and the wrapper marks it as such.
    let sql = format!("PRAGMA table_info({table})");
    let existing: Vec<String> = sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();

    if existing.iter().any(|c| c == column) {
        return Ok(());
    }

    let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {decl}");
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(pool).await?;
    tracing::info!("migrated: added {table}.{column}");
    Ok(())
}

/// Mark any run still recorded as in-progress as interrupted.
///
/// Called once at startup. A process that dies mid-archive leaves a row that
/// says "running" forever, and a history that lies about the present is worse
/// than one that admits a gap. Sources are never deleted before verification,
/// so an interrupted run leaves the originals intact.
pub async fn reconcile_interrupted_runs(pool: &SqlitePool) {
    match sqlx::query(
        "UPDATE archive_runs
            SET status = 'interrupted',
                finished = ?,
                message = COALESCE(message, 'stopped when the app restarted')
          WHERE status = 'running'",
    )
    .bind(now() as f64)
    .execute(pool)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::warn!("marked {} interrupted run(s) from a previous start", r.rows_affected())
        }
        Ok(_) => {}
        Err(e) => tracing::error!("could not reconcile interrupted runs: {e}"),
    }
}

/// Record a failed sync without disturbing the index it failed to replace.
///
/// A stale index plus a visible error is more useful than an empty one: the
/// events you could see a minute ago are still true.
pub async fn record_sync_error(pool: &SqlitePool, message: &str) {
    let _ = sqlx::query(
        "INSERT INTO index_state (id, last_error) VALUES (1, ?)
         ON CONFLICT(id) DO UPDATE SET last_error = excluded.last_error",
    )
    .bind(message)
    .execute(pool)
    .await;
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

pub async fn create_session(
    pool: &SqlitePool,
    token: &str,
    ttl_secs: i64,
    user_agent: Option<&str>,
    address: Option<&str>,
) -> anyhow::Result<()> {
    let now = now();
    sqlx::query(
        "INSERT INTO sessions (token, created, expires, last_seen, user_agent, address)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(token)
    .bind(now)
    .bind(now + ttl_secs)
    .bind(now)
    .bind(user_agent)
    .bind(address)
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

/// Note that a session was used, at a resolution of one minute.
///
/// The point of `last_seen` is to let you recognise a row in the session list
/// — "this one was active a minute ago, that one three weeks ago". Minute
/// resolution is plenty for that, and it turns a write on every single request
/// into a write once a minute per session.
pub async fn touch_session(pool: &SqlitePool, token: &str) {
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET last_seen = ?
          WHERE token = ? AND (last_seen IS NULL OR last_seen < ?)",
    )
    .bind(now())
    .bind(token)
    .bind(now() - 60)
    .execute(pool)
    .await
    {
        tracing::debug!("could not update session activity: {e}");
    }
}

/// Every live session, newest first. The tokens themselves never leave here.
pub async fn list_sessions(
    pool: &SqlitePool,
    current_token: &str,
) -> anyhow::Result<Vec<protect_api_types::SessionInfo>> {
    let rows = sqlx::query(
        "SELECT token, created, expires, last_seen, user_agent, address
           FROM sessions WHERE expires > ? ORDER BY created DESC",
    )
    .bind(now())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let token: String = r.get("token");
            let created: i64 = r.get("created");
            protect_api_types::SessionInfo {
                // A prefix, not the token: enough to tell two rows apart, and
                // far too short to sign in with.
                id: token.chars().take(8).collect(),
                current: token == current_token,
                created: created as f64,
                expires: r.get::<i64, _>("expires") as f64,
                last_seen: r.get::<Option<i64>, _>("last_seen").unwrap_or(created) as f64,
                user_agent: r.get::<Option<String>, _>("user_agent"),
                address: r.get::<Option<String>, _>("address"),
            }
        })
        .collect())
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

/// Sign out everywhere except here.
///
/// The one recovery action available with a single account: a session on a
/// device you no longer have — a phone, a borrowed laptop — is otherwise valid
/// for the full fourteen days, and changing the password does not touch it,
/// because sessions are independent of the hash that created them.
pub async fn revoke_other_sessions(pool: &SqlitePool, keep: &str) -> anyhow::Result<u64> {
    let result = sqlx::query("DELETE FROM sessions WHERE token != ?")
        .bind(keep)
        .execute(pool)
        .await?;
    if result.rows_affected() > 0 {
        tracing::info!("revoked {} other session(s)", result.rows_affected());
    }
    Ok(result.rows_affected())
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

    /// Settings saved before the archiving threshold existed must still load,
    /// and must not come back as zero — zero would read as "archive footage
    /// from this morning", which is the one answer an upgrade must never
    /// invent on the user's behalf.
    #[tokio::test]
    async fn settings_stored_before_the_threshold_existed_get_the_default() {
        let pool = pool("settings-upgrade").await;
        let legacy = r#"{"upb_container_id":"abc","events_db_path":"/backup/database/events.sqlite",
            "clip_path_prefix":"/data","camera_dirs":["Front Door"],"live_window_months":2,
            "setup_complete":true}"#;

        sqlx::query("INSERT INTO settings (id, json, updated) VALUES (1, ?, 0)")
            .bind(legacy)
            .execute(&pool)
            .await
            .unwrap();

        let loaded = load_settings(&pool).await.unwrap();
        assert_eq!(loaded.live_window_months, 2);
        assert_eq!(
            loaded.archive_after_days,
            protect_api_types::DEFAULT_ARCHIVE_AFTER_DAYS
        );
        assert!(loaded.setup_complete, "an upgrade must not send anyone back to the wizard");
    }

    #[tokio::test]
    async fn sessions_expire_and_revoke() {
        let pool = pool("sessions").await;

        create_session(&pool, "live", 3600, None, None).await.unwrap();
        assert!(session_valid(&pool, "live").await);

        create_session(&pool, "stale", -1, None, None).await.unwrap();
        assert!(!session_valid(&pool, "stale").await);

        revoke_session(&pool, "live").await;
        assert!(!session_valid(&pool, "live").await);

        assert!(!session_valid(&pool, "never-existed").await);
    }

    #[tokio::test]
    async fn the_session_list_describes_sessions_without_exposing_them() {
        let pool = pool("session-list").await;
        let token = "0123456789abcdef0123456789abcdef";

        create_session(&pool, token, 3600, Some("Firefox on Linux"), Some("10.0.0.4"))
            .await
            .unwrap();
        create_session(&pool, "other-token", 3600, None, None).await.unwrap();
        create_session(&pool, "expired-token", -1, None, None).await.unwrap();

        let list = list_sessions(&pool, token).await.unwrap();

        // Expired sessions are not live sessions, so they are not listed.
        assert_eq!(list.len(), 2);

        let current = list.iter().find(|s| s.current).expect("one row is this session");
        assert_eq!(current.user_agent.as_deref(), Some("Firefox on Linux"));
        assert_eq!(current.address.as_deref(), Some("10.0.0.4"));

        // The identifier must be useless for signing in.
        assert_eq!(current.id, "01234567");
        for s in &list {
            assert!(s.id.len() <= 8, "an id long enough to be a token leaked");
        }
    }

    #[tokio::test]
    async fn signing_out_everywhere_keeps_the_session_doing_it() {
        let pool = pool("revoke-others").await;
        for token in ["here", "phone", "old-laptop"] {
            create_session(&pool, token, 3600, None, None).await.unwrap();
        }

        assert_eq!(revoke_other_sessions(&pool, "here").await.unwrap(), 2);
        assert!(session_valid(&pool, "here").await);
        assert!(!session_valid(&pool, "phone").await);
        assert!(!session_valid(&pool, "old-laptop").await);
    }

    #[tokio::test]
    async fn activity_is_recorded_but_not_on_every_request() {
        let pool = pool("touch").await;
        create_session(&pool, "t", 3600, None, None).await.unwrap();

        let seen_at = |pool: SqlitePool| async move {
            sqlx::query_scalar::<_, i64>("SELECT last_seen FROM sessions WHERE token = 't'")
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        let before = seen_at(pool.clone()).await;

        // A fresh session was just seen, so a request now must not write.
        touch_session(&pool, "t").await;
        assert_eq!(seen_at(pool.clone()).await, before);

        // Backdated past the interval, it does.
        sqlx::query("UPDATE sessions SET last_seen = ? WHERE token = 't'")
            .bind(before - 3600)
            .execute(&pool)
            .await
            .unwrap();
        touch_session(&pool, "t").await;
        assert!(seen_at(pool).await >= before);
    }
}

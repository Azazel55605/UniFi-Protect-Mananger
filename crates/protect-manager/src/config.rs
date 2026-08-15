//! Runtime configuration.
//!
//! Only deployment-level wiring lives here. Paths, cadence and camera settings
//! belong to the in-app setup flow instead, because someone deploying this for
//! the first time should never have to hand-edit a config file.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Default image used to discover the backup container.
///
/// The container *name* is deliberately not configured: any compose deployment
/// that doesn't set `container_name` generates its own, so discovery is by
/// image instead.
pub const DEFAULT_UPB_IMAGE: &str = "ghcr.io/ep1cman/unifi-protect-backup";

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    /// Argon2 PHC string. Generate with `protect-manager hash-password`.
    pub password_hash: Option<String>,
    /// Mount point of the backup root inside *this* container.
    pub backup_dir: PathBuf,
    /// Where archives are written. Must be writable, unlike the backup root.
    pub archive_dir: PathBuf,
    /// Image substring used to find the backup container.
    pub upb_image: String,
    /// Explicit container id/name, bypassing discovery. Escape hatch only.
    pub upb_container: Option<String>,
    /// Whether to set `Secure` on the session cookie.
    ///
    /// Must be true in deployment — the reverse proxy terminates TLS and the
    /// session cookie is worthless without it. Only turned off to exercise the
    /// login flow over plain HTTP on a dev box.
    pub cookie_secure: bool,
    pub session_ttl_secs: u64,
    /// Where our own database and caches live.
    pub state_dir: PathBuf,
    /// Where the built frontend is served from.
    pub static_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("PM_BIND")
            .unwrap_or_else(|_| "0.0.0.0:8642".into())
            .parse()?;

        // Secure unless explicitly disabled: an unset variable must not
        // silently produce a cookie that survives plain HTTP.
        let cookie_secure = !matches!(
            std::env::var("PM_COOKIE_SECURE").as_deref(),
            Ok("0") | Ok("false")
        );

        Ok(Self {
            bind,
            password_hash: std::env::var("PM_PASSWORD_HASH")
                .ok()
                .map(|s| crate::auth::normalise_hash(&s))
                .filter(|s| !s.is_empty()),
            backup_dir: std::env::var("PM_BACKUP_DIR")
                .unwrap_or_else(|_| "/backup".into())
                .into(),
            archive_dir: std::env::var("PM_ARCHIVE_DIR")
                .unwrap_or_else(|_| "/archive".into())
                .into(),
            upb_image: std::env::var("PM_UPB_IMAGE")
                .unwrap_or_else(|_| DEFAULT_UPB_IMAGE.into()),
            upb_container: std::env::var("PM_UPB_CONTAINER").ok().filter(|s| !s.is_empty()),
            cookie_secure,
            session_ttl_secs: std::env::var("PM_SESSION_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60 * 60 * 24 * 14),
            state_dir: std::env::var("PM_STATE_DIR")
                .unwrap_or_else(|_| "/var/lib/protect-manager".into())
                .into(),
            static_dir: std::env::var("PM_STATIC_DIR")
                .unwrap_or_else(|_| "web/dist".into())
                .into(),
        })
    }
}

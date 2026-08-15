//! Password login and cookie sessions.
//!
//! The session cookie is `HttpOnly; Secure; SameSite=Strict`. `SameSite=Strict`
//! is what covers CSRF here — the app has no cross-site flows and is never
//! embedded, so a separate CSRF token would be ceremony. The WebSocket
//! authenticates with the same cookie because it is same-origin, which keeps
//! credentials out of query strings (and therefore out of proxy logs).
//!
//! Sessions live in our SQLite, so they survive a restart and can be revoked.
//! An unattended archive scheduler that logs you out every time it redeploys
//! would be its own small annoyance.

use password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use protect_api_types::{AuthStatus, LoginRequest};

use crate::{db, AppState};

pub const COOKIE_NAME: &str = "pm_session";

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hash a password into a PHC string for `PM_PASSWORD_HASH`.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Clean up a configured hash, repairing the mangling we can recognise.
///
/// Two environments handle `$` differently and both are reachable here: a
/// Compose file interpolates `$`, so the hash must be written with `$$`; an
/// environment field does not, so the same `$$` arrives literally. Doubling is
/// unambiguous to undo — a valid PHC string never contains `$$` — so rather
/// than making the user guess which convention applies, accept both.
pub fn normalise_hash(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(['"', '\''].as_ref()).to_string();

    if trimmed.contains("$$") {
        tracing::warn!(
            "PM_PASSWORD_HASH contains doubled '$'. That is the correct form for a \
             Compose file, but this value was passed through literally — it was \
             probably set in an environment field instead. Un-doubling it."
        );
        return trimmed.replace("$$", "$");
    }

    trimmed
}

/// Describe what is wrong with a hash, in terms of its structure.
///
/// A valid argon2 PHC string splits on `$` into exactly six parts:
/// `["", "argon2id", "v=19", "m=..,t=..,p=..", salt, hash]`. Reporting which
/// parts arrived turns "invalid password" into something diagnosable without
/// ever printing the hash itself.
pub fn diagnose_hash(hash: &str) -> Option<String> {
    // Parsing successfully is not the same as being usable. A PHC string is
    // allowed to carry parameters and a salt with no output field, so a hash
    // truncated at the last `$` parses cleanly here and only fails later, at
    // verification, as "password hash string missing field". Checking for the
    // output field is what moves that failure from login time to startup.
    match PasswordHash::new(hash) {
        Ok(parsed) if parsed.hash.is_some() => return None,
        Ok(_) => {
            return Some(format!(
                "the final hash field is missing — the value is truncated. This is what \
                 Compose interpolation does when the last segment begins with a letter: \
                 it is read as a variable name and removed. Double every '$' in the YAML \
                 ($$argon2id$$v=19$$...), or set the value where interpolation does not \
                 apply. Received {} field(s)",
                hash.split('$').count()
            ));
        }
        Err(_) => {}
    }

    let parts: Vec<&str> = hash.split('$').collect();
    let shape: Vec<String> = parts
        .iter()
        .enumerate()
        .map(|(i, p)| match i {
            // The salt and hash are the only parts worth withholding, and only
            // out of habit — neither is a secret. Length is what matters here.
            4 | 5 => format!("<{} chars>", p.len()),
            _ => format!("{p:?}"),
        })
        .collect();

    let cause = if hash.is_empty() {
        "the value is empty"
    } else if !hash.starts_with('$') {
        "it does not start with '$' — Compose interpolation consumed the '$' \
         characters. Double every '$' in the YAML: $$argon2id$$v=19$$..."
    } else if parts.len() < 6 {
        "fields are missing. The most common cause is a value truncated when \
         copied, or Compose interpolation eating part of it. Regenerate with \
         'hash-password', copy the whole single line, and double every '$' if \
         it goes into a Compose file"
    } else if parts.len() > 6 {
        "there are too many fields — the '$' characters are probably doubled. \
         Use single '$' when setting this in an environment field"
    } else {
        "the fields are present but not valid argon2 — regenerate it with \
         'hash-password'"
    };

    Some(format!(
        "{cause}. Received {} field(s): [{}]",
        parts.len(),
        shape.join(", ")
    ))
}

/// Check the configured hash at startup and explain a mangled one.
pub fn check_configured_hash(hash: &str) {
    if let Some(problem) = diagnose_hash(hash) {
        tracing::error!("PM_PASSWORD_HASH is unusable: {problem}");
        tracing::error!(
            "Run 'protect-manager check-hash' inside this container to see the same \
             diagnosis, or 'protect-manager verify-password <password>' to test a \
             password against the configured hash."
        );
    }
}

/// Public so the `verify-password` subcommand can reuse exactly the check the
/// login route performs — a diagnostic that tests something slightly different
/// from the real thing is worse than no diagnostic.
pub fn password_matches(hash: &str, password: &str) -> bool {
    verify(hash, password)
}

fn verify(hash: &str, password: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(e) => {
            tracing::error!("PM_PASSWORD_HASH is not a valid argon2 PHC string: {e}");
            false
        }
    }
}

/// True when the request carries a live session cookie.
pub async fn authenticated(state: &AppState, jar: &CookieJar) -> bool {
    match jar.get(COOKIE_NAME) {
        Some(c) => db::session_valid(&state.pool, c.value()).await,
        None => false,
    }
}

pub async fn status(State(state): State<AppState>, jar: CookieJar) -> Json<AuthStatus> {
    Json(AuthStatus {
        authenticated: authenticated(&state, &jar).await,
        configured: state.config.password_hash.is_some(),
    })
}

/// Warn when a `Secure` cookie is about to be issued to a page served over
/// plain HTTP.
///
/// The browser accepts the login and then silently discards the cookie, so the
/// next request is unauthenticated and the user is bounced back to the login
/// screen with the password they just typed correctly. From the server's side
/// the login succeeded, so nothing would otherwise appear in the logs.
///
/// `Origin` is what tells us: it carries the scheme of the page the request
/// came from. A reverse proxy terminating TLS produces `https://` here even
/// though the hop to this process is plain HTTP — which is correct, because
/// the flag only ever concerned the browser's own connection.
fn warn_if_cookie_will_be_dropped(headers: &HeaderMap, cookie_secure: bool) {
    if !cookie_secure {
        return;
    }
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return;
    };
    if origin.starts_with("http://") {
        tracing::warn!(
            "Login succeeded but the page was served over plain HTTP ({origin}). The \
             session cookie is marked Secure, so the browser will discard it and the \
             login will appear to fail. Either terminate TLS at your reverse proxy \
             (the hop from the proxy to this container can stay HTTP), or set \
             PM_COOKIE_SECURE=0 while testing."
        );
    }
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let Some(hash) = state.config.password_hash.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no password configured; set PM_PASSWORD_HASH",
        )
            .into_response();
    };

    if !verify(hash, &body.password) {
        // Deliberately vague, and deliberately not logged with the attempt:
        // there is exactly one account, so a failure is either you or someone
        // who is already inside the VPN.
        return (StatusCode::UNAUTHORIZED, "invalid password").into_response();
    }

    warn_if_cookie_will_be_dropped(&headers, state.config.cookie_secure);

    let token = new_token();
    if let Err(e) = db::create_session(&state.pool, &token, state.config.session_ttl_secs as i64).await
    {
        tracing::error!("could not persist session: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "could not create session").into_response();
    }

    let mut cookie = Cookie::new(COOKIE_NAME, token);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Strict);
    cookie.set_secure(state.config.cookie_secure);
    cookie.set_path("/");
    cookie.set_max_age(time::Duration::seconds(state.config.session_ttl_secs as i64));

    (jar.add(cookie), StatusCode::NO_CONTENT).into_response()
}

pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    if let Some(c) = jar.get(COOKIE_NAME) {
        db::revoke_session(&state.pool, c.value()).await;
    }
    let mut removal = Cookie::from(COOKIE_NAME);
    removal.set_path("/");
    (jar.remove(removal), StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> String {
        hash_password("correct horse").unwrap()
    }

    #[test]
    fn a_valid_hash_has_no_complaints() {
        assert!(diagnose_hash(&good()).is_none());
        assert!(password_matches(&good(), "correct horse"));
    }

    #[test]
    fn compose_interpolation_is_recognised() {
        // What Compose actually produces: leading `$name` segments are
        // replaced with empty strings, so the string loses its `$` prefix.
        let mangled = "=19=19456,t=2,p=1$9Ja3K9c8MfT20JgepllNFjVxAihDHTtkuXer4RmqKDE";
        let problem = diagnose_hash(mangled).expect("must be rejected");
        assert!(problem.contains("Double every '$'"), "{problem}");
    }

    #[test]
    fn a_literal_doubled_hash_is_repaired() {
        let doubled = good().replace('$', "$$");
        let repaired = normalise_hash(&doubled);
        assert!(diagnose_hash(&repaired).is_none());
        assert!(password_matches(&repaired, "correct horse"));
    }

    #[test]
    fn a_truncated_hash_reports_missing_fields() {
        // Dropping the trailing hash field — what a partial copy/paste leaves.
        let g = good();
        let truncated = &g[..g.rfind('$').unwrap()];
        let problem = diagnose_hash(truncated).expect("must be rejected");
        assert!(problem.contains("final hash field is missing"), "{problem}");
    }

    #[test]
    fn surrounding_whitespace_and_quotes_are_tolerated() {
        // A YAML value copied with quotes, or a trailing newline from a shell.
        let g = good();
        for raw in [format!("  {g}\n"), format!("\"{g}\""), format!("'{g}'")] {
            let cleaned = normalise_hash(&raw);
            assert!(diagnose_hash(&cleaned).is_none(), "failed for {raw:?}");
            assert!(password_matches(&cleaned, "correct horse"));
        }
    }

    #[test]
    fn the_diagnosis_never_prints_the_hash_itself() {
        let g = good();
        let truncated = &g[..g.rfind('$').unwrap()];
        let problem = diagnose_hash(truncated).unwrap();
        let salt = g.split('$').nth(4).unwrap();
        assert!(!problem.contains(salt), "salt leaked into the diagnosis");
    }
}

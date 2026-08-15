//! Backoff on failed sign-ins.
//!
//! There is exactly one password and access is over a VPN, so this is
//! defence-in-depth rather than the thing standing between an attacker and the
//! footage. What it actually buys: an argon2 verification is deliberately
//! expensive, so an unthrottled login endpoint is a CPU exhaustion primitive
//! long before it is a way in. Backoff closes that.
//!
//! Two buckets are consulted, and the longer wait wins:
//!
//! * **Per client.** Generous, because the client address is only as
//!   trustworthy as the proxy in front of us — and behind a reverse proxy that
//!   does not forward the original address, *every* client shares one bucket.
//! * **Global.** Ignores the key entirely, so rotating addresses cannot walk
//!   around the limit. Capped much lower than the per-client block, because
//!   this one can lock out the legitimate user: with a single account, someone
//!   else's noise must not cost you the evening.
//!
//! A rejected attempt costs nothing and counts for nothing: the check happens
//! before the hash is verified, so a client that keeps hammering during its own
//! block does not deepen it. That is deliberate. The delay only doubles for a
//! client patient enough to wait each block out, and an impatient one is
//! already getting no hashing done — which was the thing being rationed.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

/// The peer address, when the server was built with connect info.
///
/// `Option<ConnectInfo<_>>` is not an extractor in axum 0.8, and the plain
/// `ConnectInfo` extractor rejects with a 500 when it is missing — which would
/// turn a server built without connect info into one where *login itself* is
/// broken. Sign-in must not depend on knowing where the client is: not knowing
/// simply means everyone shares a bucket, which is already the case behind a
/// proxy that does not forward the address.
pub struct ClientAddr(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0),
        ))
    }
}

/// Failures forgiven before any delay is imposed. Typos are normal.
const FREE_ATTEMPTS: u32 = 5;
const BASE_DELAY_SECS: f64 = 5.0;
const MAX_DELAY_SECS: f64 = 300.0;

/// The global bucket forgives more and punishes less: it exists to bound total
/// work, not to keep anyone out.
const GLOBAL_FREE_ATTEMPTS: u32 = 15;
const GLOBAL_MAX_DELAY_SECS: f64 = 30.0;

/// A bucket untouched for this long is forgotten, so the map cannot grow
/// without bound and yesterday's typos are not held against you.
const FORGET_AFTER_SECS: f64 = 3600.0;

const GLOBAL_KEY: &str = "\0global";

#[derive(Debug, Default, Clone, Copy)]
struct Bucket {
    failures: u32,
    /// When the current block expires. Zero when not blocked.
    until: f64,
    last_seen: f64,
}

#[derive(Debug, Default)]
pub struct Limiter {
    // A mutex rather than a lock-free structure: this is touched once per
    // login attempt, and login attempts are rare by construction.
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl Limiter {
    /// Seconds the caller must wait, or `None` if it may proceed.
    pub fn retry_after(&self, key: &str, now: f64) -> Option<f64> {
        let buckets = self.buckets.lock().unwrap();
        let wait = |k: &str| {
            buckets
                .get(k)
                .map(|b| b.until - now)
                .filter(|remaining| *remaining > 0.0)
        };

        // The longer of the two, so neither bucket can be talked out of by the
        // other having expired.
        match (wait(key), wait(GLOBAL_KEY)) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn record_failure(&self, key: &str, now: f64) {
        let mut buckets = self.buckets.lock().unwrap();
        buckets.retain(|_, b| now - b.last_seen < FORGET_AFTER_SECS);

        for (k, free, cap) in [
            (key, FREE_ATTEMPTS, MAX_DELAY_SECS),
            (GLOBAL_KEY, GLOBAL_FREE_ATTEMPTS, GLOBAL_MAX_DELAY_SECS),
        ] {
            let bucket = buckets.entry(k.to_string()).or_default();
            bucket.failures += 1;
            bucket.last_seen = now;
            if bucket.failures > free {
                // Doubling from the first failure past the allowance: 5s, 10s,
                // 20s… A scripted attempt hits the cap almost immediately; a
                // person who mistyped twice more never notices.
                let steps = bucket.failures - free - 1;
                let delay = (BASE_DELAY_SECS * 2f64.powi(steps.min(16) as i32)).min(cap);
                bucket.until = now + delay;
            }
        }
    }

    /// Forget a client's failures. The global bucket is deliberately *not*
    /// cleared: one success would otherwise reset a limit that exists to bound
    /// what everyone else can do.
    pub fn record_success(&self, key: &str) {
        self.buckets.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> Limiter {
        Limiter::default()
    }

    #[test]
    fn a_few_typos_cost_nothing() {
        let l = limiter();
        for _ in 0..FREE_ATTEMPTS {
            l.record_failure("client", 0.0);
        }
        assert_eq!(l.retry_after("client", 0.0), None);
    }

    #[test]
    fn further_failures_back_off_and_then_stop_growing() {
        let l = limiter();
        for _ in 0..FREE_ATTEMPTS + 1 {
            l.record_failure("client", 0.0);
        }
        assert_eq!(l.retry_after("client", 0.0), Some(BASE_DELAY_SECS));

        l.record_failure("client", 0.0);
        assert_eq!(l.retry_after("client", 0.0), Some(BASE_DELAY_SECS * 2.0));

        for _ in 0..60 {
            l.record_failure("client", 0.0);
        }
        assert_eq!(l.retry_after("client", 0.0), Some(MAX_DELAY_SECS));
    }

    #[test]
    fn a_block_expires_on_its_own() {
        let l = limiter();
        for _ in 0..FREE_ATTEMPTS + 1 {
            l.record_failure("client", 0.0);
        }
        assert!(l.retry_after("client", BASE_DELAY_SECS - 0.1).is_some());
        assert_eq!(l.retry_after("client", BASE_DELAY_SECS + 0.1), None);
    }

    #[test]
    fn signing_in_clears_the_failures() {
        let l = limiter();
        for _ in 0..FREE_ATTEMPTS + 1 {
            l.record_failure("client", 0.0);
        }
        l.record_success("client");
        assert_eq!(l.retry_after("client", 0.0), None);
    }

    #[test]
    fn rotating_the_address_does_not_walk_around_the_limit() {
        let l = limiter();
        // Every attempt from a fresh address, so no per-client bucket ever
        // reaches its allowance. The global one still does.
        for i in 0..GLOBAL_FREE_ATTEMPTS + 1 {
            l.record_failure(&format!("client-{i}"), 0.0);
        }
        let wait = l.retry_after("client-new", 0.0).expect("global limit must apply");
        assert!(wait > 0.0 && wait <= GLOBAL_MAX_DELAY_SECS);
    }

    #[test]
    fn one_clients_noise_cannot_lock_everyone_out_for_long() {
        let l = limiter();
        for _ in 0..500 {
            l.record_failure("noisy", 0.0);
        }
        // The noisy client is held for the full block; everyone else is held
        // only by the far shorter global cap.
        assert_eq!(l.retry_after("noisy", 0.0), Some(MAX_DELAY_SECS));
        assert_eq!(l.retry_after("someone-else", 0.0), Some(GLOBAL_MAX_DELAY_SECS));
    }

    #[test]
    fn hammering_during_a_block_neither_helps_nor_deepens_it() {
        let l = limiter();
        for _ in 0..FREE_ATTEMPTS + 1 {
            l.record_failure("client", 0.0);
        }
        let first = l.retry_after("client", 0.0).unwrap();

        // What the server does with a blocked request: reject it, and do not
        // reach `record_failure` at all.
        for t in [0.5, 1.0, 2.0] {
            assert!(l.retry_after("client", t).is_some());
        }
        assert_eq!(l.retry_after("client", 0.0), Some(first));
    }

    #[test]
    fn old_buckets_are_forgotten() {
        let l = limiter();
        for _ in 0..FREE_ATTEMPTS + 1 {
            l.record_failure("client", 0.0);
        }
        // A later failure from someone else prunes the stale entry rather than
        // leaving it to accumulate for the life of the process.
        l.record_failure("other", FORGET_AFTER_SECS + 1.0);
        assert!(!l.buckets.lock().unwrap().contains_key("client"));
    }
}

//! Request identity and per-request logging.
//!
//! Every request gets a short id. It goes into the tracing span, so every log
//! line the request produces carries it; into the `x-request-id` response
//! header; and, on a server fault, into the error body. A screenshot of "and
//! then it said 500" is then enough to find the exact line that explains it —
//! which matters for something that runs unattended on someone else's homelab
//! and is debugged over a chat message.
//!
//! It replaces `TraceLayer`, which logs a fine generic span but cannot put an
//! id into the response body, and had no opinion about which requests are
//! worth a log line at all.

use std::time::Instant;

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use password_hash::rand_core::{OsRng, RngCore};
use tracing::Instrument;

tokio::task_local! {
    static REQUEST_ID: String;
}

/// The id of the request being handled, if we are inside one.
///
/// Background loops — the indexer, the scheduler, the watchdog — are not
/// requests and correctly get `None`.
pub fn current_request_id() -> Option<String> {
    REQUEST_ID.try_with(|id| id.clone()).ok()
}

fn new_id() -> String {
    // Eight hex characters. This is a correlation id inside one process's
    // logs, not a globally unique one, and a short id is a readable id.
    let mut bytes = [0u8; 4];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How long a request has to take before it is worth mentioning on its own.
///
/// Transcoding and archiving are genuinely slow, and they report progress
/// elsewhere; this is for the requests that are *supposed* to be quick.
const SLOW_REQUEST_SECS: f64 = 5.0;

pub async fn middleware(request: Request, next: Next) -> Response {
    let id = new_id();
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    // Static assets are the majority of requests and none of the interest.
    let boring = !path.starts_with("/api/") && !path.starts_with("/ws/");

    let span = tracing::info_span!("request", id = %id, %method, path = %path);
    let started = Instant::now();

    let mut response = REQUEST_ID
        .scope(id.clone(), next.run(request).instrument(span.clone()))
        .await;

    let elapsed = started.elapsed().as_secs_f64();
    let status = response.status().as_u16();
    let _entered = span.enter();

    if response.status().is_server_error() {
        tracing::error!(status, elapsed_secs = elapsed, "request failed");
    } else if elapsed > SLOW_REQUEST_SECS && !boring {
        tracing::warn!(status, elapsed_secs = elapsed, "slow request");
    } else if !boring {
        tracing::debug!(status, elapsed_secs = elapsed, "handled");
    }

    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route(
                "/api/echo",
                // Reads the id the middleware installed, so the test proves the
                // handler and the header agree rather than that a header exists.
                get(|| async { current_request_id().unwrap_or_else(|| "none".into()) }),
            )
            .layer(axum::middleware::from_fn(middleware))
    }

    async fn call(app: Router) -> (String, String) {
        let res = app
            .oneshot(Request::builder().uri("/api/echo").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let header = res.headers()["x-request-id"].to_str().unwrap().to_string();
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        (header, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn the_handler_sees_the_id_that_the_client_is_told() {
        let (header, seen) = call(app()).await;
        assert_eq!(header, seen);
        assert_eq!(header.len(), 8);
    }

    #[tokio::test]
    async fn each_request_gets_its_own_id() {
        let (first, _) = call(app()).await;
        let (second, _) = call(app()).await;
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn background_work_has_no_request_id() {
        assert!(current_request_id().is_none());
    }
}

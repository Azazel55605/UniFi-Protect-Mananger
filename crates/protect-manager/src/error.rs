//! One error type for the whole API.
//!
//! Before this, every handler decided its own status code and wrote its own
//! body — sometimes JSON, usually a bare string, occasionally a raw
//! `anyhow::Error` rendered straight to the client. That leaked internals
//! outward and gave the frontend nothing to branch on but the status code,
//! which is too coarse: a 409 might mean "setup is unfinished" or "a job is
//! already running", and the UI wants to answer those differently.
//!
//! So: a classified code the frontend switches on, a sentence for the person
//! reading it, and an optional hint saying what to do. Internal detail is
//! logged with a request id and *not* sent — the client gets the id instead,
//! which is enough to find the log line and nothing else.

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use protect_api_types::{ApiErrorBody, ErrorCode, NamedCheck};

use crate::trace::current_request_id;

#[derive(Debug)]
pub struct ApiError {
    code: ErrorCode,
    message: String,
    hint: Option<String>,
    checks: Option<Vec<NamedCheck>>,
    retry_after_secs: Option<f64>,
    /// The underlying cause. Logged, never serialised.
    detail: Option<String>,
}

impl ApiError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            checks: None,
            retry_after_secs: None,
            detail: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn unauthenticated() -> Self {
        Self::new(ErrorCode::Unauthenticated, "You are not signed in.")
    }

    pub fn rate_limited(retry_after_secs: f64) -> Self {
        Self {
            retry_after_secs: Some(retry_after_secs),
            ..Self::new(
                ErrorCode::RateLimited,
                "Too many failed sign-in attempts.",
            )
        }
        .hint("Wait for the countdown to finish, then try again.")
    }

    pub fn setup_incomplete() -> Self {
        Self::new(
            ErrorCode::SetupIncomplete,
            "Setup has not been finished yet.",
        )
        .hint("Finish the setup wizard so the app knows where your footage lives.")
    }

    pub fn docker_unavailable() -> Self {
        Self::new(
            ErrorCode::DockerUnavailable,
            "The Docker socket is not available.",
        )
        .hint("Mount /var/run/docker.sock into this container, read-only is enough.")
    }

    pub fn container_not_found(image: &str) -> Self {
        Self::new(
            ErrorCode::ContainerNotFound,
            format!("No running container matches the image {image}."),
        )
        .hint("Set PM_UPB_IMAGE if you run a fork or a retagged build.")
    }

    pub fn docker_failed(e: impl std::fmt::Display) -> Self {
        Self {
            detail: Some(e.to_string()),
            ..Self::new(ErrorCode::DockerFailed, "Docker refused the request.")
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }

    /// Settings that failed validation, carrying the failures themselves so
    /// the wizard can point at the step at fault.
    pub fn invalid_settings(checks: Vec<NamedCheck>) -> Self {
        Self {
            checks: Some(checks),
            ..Self::new(ErrorCode::Invalid, "Some settings are not valid.")
        }
    }

    /// A clip that exists but will not decode. Not a server fault — a
    /// truncated download or a file still being written looks exactly like
    /// this, and both resolve themselves.
    pub fn media_unreadable(e: impl std::fmt::Display) -> Self {
        Self {
            detail: Some(e.to_string()),
            ..Self::new(
                ErrorCode::MediaUnreadable,
                "This clip could not be read.",
            )
        }
        .hint("It may be a partial download that the backup service has not finished.")
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self {
            detail: Some(e.to_string()),
            ..Self::new(ErrorCode::Internal, "Something went wrong on the server.")
        }
    }

    fn status(&self) -> StatusCode {
        match self.code {
            ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::SetupIncomplete | ErrorCode::Conflict => StatusCode::CONFLICT,
            ErrorCode::DockerUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ContainerNotFound | ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::DockerFailed => StatusCode::BAD_GATEWAY,
            ErrorCode::Invalid | ErrorCode::MediaUnreadable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// The client-facing sentence, so an error can be embedded in a response that
/// is not itself an error — a clip that is archived rather than missing, say.
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// Anything that fails with an `anyhow::Error` and has no better classification
/// is a server fault, so `?` works in handlers without ceremony.
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        // `{:#}` walks the context chain, which is where the useful part of an
        // anyhow error usually lives.
        Self::internal(format!("{e:#}"))
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let request_id = current_request_id();

        // The cause is logged rather than sent. A 5xx is our problem and the
        // detail is ours to read; the client gets the id that finds it.
        if let Some(detail) = &self.detail {
            if status.is_server_error() {
                tracing::error!(code = ?self.code, detail, "request failed");
            } else {
                tracing::debug!(code = ?self.code, detail, "request rejected");
            }
        }

        let retry_after = self.retry_after_secs;
        let body = ApiErrorBody {
            code: self.code,
            message: self.message,
            hint: self.hint,
            checks: self.checks,
            retry_after_secs: retry_after,
            // Only on server faults: on a 404 it is noise, and it would invite
            // people to report ordinary rejections as bugs.
            request_id: status.is_server_error().then_some(request_id).flatten(),
        };

        let mut response = (status, Json(body)).into_response();

        // Retry-After is the standard way to say this, and something other
        // than our own frontend may eventually be reading these responses.
        if let Some(secs) = retry_after {
            if let Ok(v) = HeaderValue::from_str(&secs.ceil().to_string()) {
                response.headers_mut().insert("retry-after", v);
            }
        }

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(e: ApiError) -> (StatusCode, ApiErrorBody) {
        let res = e.into_response();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn the_cause_of_a_server_fault_never_reaches_the_client() {
        let (status, body) =
            body_of(ApiError::internal("connection string: user=admin password=hunter2")).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.code, ErrorCode::Internal);
        assert!(
            !body.message.contains("hunter2"),
            "the underlying error leaked into the response: {}",
            body.message
        );
        assert!(body.hint.is_none());
    }

    #[tokio::test]
    async fn a_rate_limit_says_when_to_come_back() {
        let res = ApiError::rate_limited(42.3).into_response();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        // Rounded up: telling a client to retry in 42s when 42.3 remain would
        // have it rejected a second time.
        assert_eq!(res.headers()["retry-after"], "43");
    }

    #[tokio::test]
    async fn failed_settings_travel_with_the_error() {
        let (status, body) = body_of(ApiError::invalid_settings(vec![NamedCheck {
            name: "clip_prefix".into(),
            ok: false,
            detail: "no such directory".into(),
        }]))
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body.checks.unwrap()[0].name, "clip_prefix");
    }

    #[tokio::test]
    async fn a_rejection_carries_no_request_id() {
        // The id exists to find a log line for a fault. Attaching one to an
        // ordinary 404 would suggest there is something to look up.
        let (_, body) = body_of(ApiError::not_found("no such event")).await;
        assert!(body.request_id.is_none());
    }

    #[test]
    fn every_code_maps_to_its_own_status_family() {
        use ErrorCode::*;
        for (code, expected) in [
            (Unauthenticated, 401),
            (RateLimited, 429),
            (SetupIncomplete, 409),
            (DockerUnavailable, 503),
            (ContainerNotFound, 404),
            (DockerFailed, 502),
            (NotFound, 404),
            (Conflict, 409),
            (Invalid, 422),
            (MediaUnreadable, 422),
            (Internal, 500),
        ] {
            let e = ApiError::new(code, "x");
            assert_eq!(e.status().as_u16(), expected, "{code:?}");
        }
    }
}

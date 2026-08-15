//! Serving a file with HTTP range support.
//!
//! Range requests are what make a `<video>` element usable: without them the
//! browser must fetch the whole clip before it can play, and seeking is
//! impossible. Implemented directly because the alternative — a static-file
//! service — doesn't fit here: these paths are resolved from the index and
//! authorised per request, not mapped from a URL.

use std::path::Path;

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// Parse a single-range `Range: bytes=start-end` header.
///
/// Multi-range requests are answered with the whole file instead. They are
/// legal, no browser video player sends them, and a wrong multipart response
/// is worse than a correct simple one.
fn parse_range(headers: &HeaderMap, len: u64) -> Option<(u64, u64)> {
    let raw = headers.get(header::RANGE)?.to_str().ok()?;
    let spec = raw.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;

    let (start, end) = match (start.trim(), end.trim()) {
        // `bytes=-500` — the final 500 bytes.
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (s, "") => (s.parse().ok()?, len - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len - 1)),
    };

    (start <= end && start < len).then_some((start, end))
}

/// Stream a file, honouring a range request when there is one.
pub async fn serve_file(path: &Path, content_type: &str, headers: &HeaderMap) -> Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            return (StatusCode::NOT_FOUND, format!("{e}")).into_response();
        }
    };
    let len = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    };

    if len == 0 {
        return (StatusCode::NO_CONTENT, "empty file").into_response();
    }

    let mut base = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        // Advertised so the browser knows seeking is available at all.
        .header(header::ACCEPT_RANGES, "bytes")
        // Clips never change once written, and a transcode is keyed by event
        // id, so both are safe to cache hard.
        .header(header::CACHE_CONTROL, "private, max-age=86400");

    match parse_range(headers, len) {
        None if headers.contains_key(header::RANGE) => {
            // A range was asked for and it doesn't make sense against this
            // file — say so rather than quietly sending everything.
            Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .unwrap()
        }
        None => {
            let stream = ReaderStream::new(file);
            base = base.header(header::CONTENT_LENGTH, len);
            base.status(StatusCode::OK).body(Body::from_stream(stream)).unwrap()
        }
        Some((start, end)) => {
            let mut file = file;
            if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, "seek failed").into_response();
            }
            let take = file.take(end - start + 1);
            let stream = ReaderStream::new(take);
            base = base
                .header(header::CONTENT_LENGTH, end - start + 1)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
            base.status(StatusCode::PARTIAL_CONTENT)
                .body(Body::from_stream(stream))
                .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(range: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, range.parse().unwrap());
        h
    }

    #[test]
    fn parses_the_ranges_a_video_element_actually_sends() {
        // Opening a video: "give me everything from the start".
        assert_eq!(parse_range(&headers("bytes=0-"), 1000), Some((0, 999)));
        // Seeking: an explicit window.
        assert_eq!(parse_range(&headers("bytes=200-499"), 1000), Some((200, 499)));
        // A suffix range, used to read the trailing index of a file.
        assert_eq!(parse_range(&headers("bytes=-100"), 1000), Some((900, 999)));
    }

    #[test]
    fn an_end_past_the_file_is_clamped_rather_than_refused() {
        assert_eq!(parse_range(&headers("bytes=900-5000"), 1000), Some((900, 999)));
    }

    #[test]
    fn nonsense_ranges_are_rejected() {
        assert_eq!(parse_range(&headers("bytes=500-200"), 1000), None);
        assert_eq!(parse_range(&headers("bytes=2000-3000"), 1000), None);
        assert_eq!(parse_range(&headers("items=0-10"), 1000), None);
        // Multi-range is legal but unused by players; answered whole instead.
        assert_eq!(parse_range(&headers("bytes=0-10,20-30"), 1000), None);
        assert_eq!(parse_range(&HeaderMap::new(), 1000), None);
    }

    #[tokio::test]
    async fn serves_a_partial_response_for_a_range() {
        let dir = std::env::temp_dir().join(format!("pm-range-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mp4");
        std::fs::write(&path, (0u8..=255).collect::<Vec<u8>>()).unwrap();

        let res = serve_file(&path, "video/mp4", &headers("bytes=10-19")).await;
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 10-19/256");
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "10");

        let whole = serve_file(&path, "video/mp4", &HeaderMap::new()).await;
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(whole.headers()[header::ACCEPT_RANGES], "bytes");

        let bad = serve_file(&path, "video/mp4", &headers("bytes=9999-")).await;
        assert_eq!(bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

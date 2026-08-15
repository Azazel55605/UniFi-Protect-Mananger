//! Reading meaning out of clip paths.
//!
//! The backup service's database records an event's id, type, camera id and
//! timing — but not the camera's *name*, and not which detections triggered it.
//! Both of those only exist in the path it wrote the clip to:
//!
//! ```text
//! /data/Front Door/2026-08-15/2026-08-15T13-57-59 smartDetectZone (person).mp4
//!       ^camera    ^date       ^timestamp         ^type            ^subtypes
//! ```
//!
//! So this module is the only source of the most useful filter in the app, and
//! the part most likely to break if the upstream file-structure template
//! changes. It parses defensively: every field is optional, an unrecognised
//! shape yields whatever could be read rather than an error, and the tests pin
//! the real-world cases including multi-valued detections and non-ASCII names.

/// What a clip path can tell us.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedClip {
    /// First path segment below the clip root — the camera's directory.
    pub camera_name: Option<String>,
    /// Event type as it appears in the filename, e.g. `smartDetectZone`.
    pub event_type: Option<String>,
    /// Detection types from the parenthesised group. Multiple detections on
    /// one event are space-separated, e.g. `(animal person)`.
    pub subtypes: Vec<String>,
}

/// Strip the prefix the backup service recorded, leaving a path relative to
/// the clip root.
///
/// Returns `None` when the path isn't under the prefix at all — a silent
/// mismatch here would produce paths that look plausible and don't exist.
pub fn strip_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return Some(path.trim_start_matches('/'));
    }
    // The remainder must begin at a path boundary. Without this check `/data`
    // also matches `/database`, which is a real sibling directory in this
    // layout — the backup service keeps its own database next to the clips.
    // Files under it would then be indexed as footage.
    path.strip_prefix(prefix)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .map(|rest| rest.trim_start_matches('/'))
        .filter(|rest| !rest.is_empty())
}

/// Parse a recorded clip path.
///
/// `path` is as the backup service recorded it; `prefix` is what to remove
/// from the front. Anything that cannot be determined is left as `None`
/// instead of guessed.
pub fn parse(path: &str, prefix: &str) -> ParsedClip {
    let relative = strip_prefix(path, prefix).unwrap_or(path.trim_start_matches('/'));
    let mut segments = relative.split('/');

    // The camera directory is the first segment, but only when there is a
    // deeper path — a bare filename tells us nothing about a camera.
    let first = segments.next().filter(|s| !s.is_empty());
    let remainder: Vec<&str> = segments.collect();
    let camera_name = if remainder.is_empty() {
        None
    } else {
        first.map(str::to_string)
    };

    let file = remainder.last().copied().or(first).unwrap_or_default();
    let (event_type, subtypes) = parse_filename(file);

    ParsedClip { camera_name, event_type, subtypes }
}

/// Pull the event type and detections out of a filename.
fn parse_filename(file: &str) -> (Option<String>, Vec<String>) {
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);

    // Detections live in the last parenthesised group. Searching from the end
    // means a camera or timestamp containing parentheses can't be mistaken
    // for one.
    let (before, subtypes) = match (stem.rfind('('), stem.rfind(')')) {
        (Some(open), Some(close)) if close > open => {
            let inner = &stem[open + 1..close];
            (
                &stem[..open],
                inner
                    .split_whitespace()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            )
        }
        _ => (stem, Vec::new()),
    };

    // The type is the token between the timestamp and the detections. The
    // timestamp itself contains no spaces, so the last whitespace-separated
    // token that isn't the timestamp is what we want.
    let event_type = before
        .split_whitespace()
        .rfind(|t| !looks_like_timestamp(t))
        .map(str::to_string);

    (event_type, subtypes)
}

/// `2026-08-15T13-57-59` — date, `T`, time. Recognised so it is never
/// mistaken for the event type.
fn looks_like_timestamp(token: &str) -> bool {
    let Some((date, time)) = token.split_once('T') else {
        return false;
    };
    date.len() == 10
        && date.as_bytes().iter().all(|b| b.is_ascii_digit() || *b == b'-')
        && time.len() >= 8
        && time.as_bytes().iter().all(|b| b.is_ascii_digit() || *b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "/data";

    fn subtypes(path: &str) -> Vec<String> {
        parse(path, PREFIX).subtypes
    }

    #[test]
    fn reads_camera_type_and_detection() {
        let p = parse(
            "/data/Front Door/2026-08-15/2026-08-15T13-57-59 smartDetectZone (person).mp4",
            PREFIX,
        );
        assert_eq!(p.camera_name.as_deref(), Some("Front Door"));
        assert_eq!(p.event_type.as_deref(), Some("smartDetectZone"));
        assert_eq!(p.subtypes, vec!["person"]);
    }

    #[test]
    fn detections_can_be_multiple() {
        // One event can trigger several detections; the filter UI has to treat
        // subtypes as a set rather than a single value because of this.
        assert_eq!(
            subtypes("/data/Cam/2026-06-15/2026-06-15T06-50-18 smartDetectZone (animal person).mp4"),
            vec!["animal", "person"]
        );
        assert_eq!(
            subtypes("/data/Cam/2026-06-19/2026-06-19T21-08-31 smartDetectZone (person vehicle).mp4"),
            vec!["person", "vehicle"]
        );
    }

    #[test]
    fn handles_audio_detections() {
        let p = parse(
            "/data/Cam/2026-05-29/2026-05-29T12-00-19 smartAudioDetect (alrmSpeak).mp4",
            PREFIX,
        );
        assert_eq!(p.event_type.as_deref(), Some("smartAudioDetect"));
        assert_eq!(p.subtypes, vec!["alrmSpeak"]);
    }

    #[test]
    fn non_ascii_camera_names_survive() {
        let p = parse(
            "/data/Gartenhäuschen/2026-08-15/2026-08-15T10-20-25 smartDetectZone (person).mp4",
            PREFIX,
        );
        assert_eq!(p.camera_name.as_deref(), Some("Gartenhäuschen"));
    }

    #[test]
    fn a_missing_detection_group_is_not_an_error() {
        // Motion and ring events have no detection subtype. They must still
        // parse, since the app is meant to keep working if those appear.
        let p = parse("/data/Cam/2026-08-15/2026-08-15T10-20-25 motion.mp4", PREFIX);
        assert_eq!(p.event_type.as_deref(), Some("motion"));
        assert!(p.subtypes.is_empty());
        assert_eq!(p.camera_name.as_deref(), Some("Cam"));
    }

    #[test]
    fn a_camera_name_containing_parentheses_is_not_read_as_a_detection() {
        let p = parse(
            "/data/Side (rear)/2026-08-15/2026-08-15T10-20-25 smartDetectZone (person).mp4",
            PREFIX,
        );
        assert_eq!(p.camera_name.as_deref(), Some("Side (rear)"));
        assert_eq!(p.subtypes, vec!["person"]);
    }

    #[test]
    fn unfamiliar_layouts_yield_what_they_can() {
        // A custom file-structure template. We should not invent a camera, but
        // the filename still carries a type and a detection.
        let p = parse("/data/2026-08-15T10-20-25 smartDetectZone (person).mp4", PREFIX);
        assert_eq!(p.camera_name, None);
        assert_eq!(p.event_type.as_deref(), Some("smartDetectZone"));
        assert_eq!(p.subtypes, vec!["person"]);
    }

    #[test]
    fn prefix_handling_is_strict_about_boundaries() {
        assert_eq!(strip_prefix("/data/Cam/x.mp4", "/data"), Some("Cam/x.mp4"));
        assert_eq!(strip_prefix("/data/Cam/x.mp4", "/data/"), Some("Cam/x.mp4"));
        // A different directory that merely starts with the same characters is
        // not inside the prefix.
        assert_eq!(strip_prefix("/database/x.mp4", "/data"), None);
        assert_eq!(strip_prefix("/data", "/data"), None);
    }

    #[test]
    fn a_timestamp_is_never_mistaken_for_the_event_type() {
        // If the type were ever absent, we must not fall back to reporting the
        // timestamp as the type.
        let (event_type, subs) = parse_filename("2026-08-15T13-57-59 (person).mp4");
        assert_eq!(event_type, None);
        assert_eq!(subs, vec!["person"]);
    }
}

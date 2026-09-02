//! First-run setup: discovery and validation.
//!
//! The design goal is that a user confirms rather than composes. Everything we
//! can derive from the backup container or the filesystem is proposed with the
//! evidence behind it; the user's job is to say yes or correct it.

use std::path::{Path, PathBuf};

use protect_api_types::{CameraCandidate, NamedCheck, Settings};

/// A directory name shaped like `YYYY-MM-DD`.
///
/// This is how camera directories are identified. The obvious alternative —
/// excluding a list of known non-camera names like `database`, `rclone`, `ufp`
/// — only describes one deployment, and would silently swallow a camera that
/// happened to be named `ufp`. Evidence generalises; a blocklist does not.
fn is_date_dir(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..].iter().all(u8::is_ascii_digit)
}

fn is_clip(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".mp4") || lower.ends_with(".mkv") || lower.ends_with(".mov")
}

/// Inspect one directory for evidence that it holds a camera's clips.
///
/// Only the first few date directories are opened. A camera with two years of
/// history has hundreds of them, and counting every clip would turn setup into
/// a filesystem walk of the whole archive.
fn examine(dir: &Path) -> (usize, usize, Option<String>) {
    const SAMPLE: usize = 5;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0, Some("not readable".into()));
    };

    let mut date_dirs = 0usize;
    let mut sampled = 0usize;
    let mut clips = 0usize;

    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_date_dir(&name) {
            continue;
        }
        date_dirs += 1;

        if sampled < SAMPLE {
            sampled += 1;
            if let Ok(day) = std::fs::read_dir(entry.path()) {
                clips += day
                    .filter_map(Result::ok)
                    .filter(|e| is_clip(&e.file_name().to_string_lossy()))
                    .count();
            }
        }
    }

    let note = (date_dirs > sampled).then(|| {
        format!("{clips} clips counted across the first {sampled} of {date_dirs} day folders")
    });

    (date_dirs, clips, note)
}

/// Find directories under the backup root that look like cameras.
pub fn find_cameras(backup_dir: &Path) -> (Vec<CameraCandidate>, Vec<String>) {
    let mut notes = Vec::new();

    let entries = match std::fs::read_dir(backup_dir) {
        Ok(e) => e,
        Err(e) => {
            notes.push(format!("cannot read {}: {e}", backup_dir.display()));
            return (Vec::new(), notes);
        }
    };

    let mut candidates: Vec<CameraCandidate> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| {
            let dir_name = e.file_name().to_string_lossy().to_string();
            let (date_dirs, clip_count, note) = examine(&e.path());
            CameraCandidate {
                looks_like_camera: date_dirs > 0,
                dir_name,
                date_dirs,
                clip_count,
                note,
            }
        })
        .collect();

    // Likely cameras first, then by volume — the user scans a short list of
    // plausible things rather than an alphabetical mix.
    candidates.sort_by(|a, b| {
        b.looks_like_camera
            .cmp(&a.looks_like_camera)
            .then(b.date_dirs.cmp(&a.date_dirs))
            .then(a.dir_name.cmp(&b.dir_name))
    });

    if candidates.iter().all(|c| !c.looks_like_camera) && !candidates.is_empty() {
        notes.push(
            "No directory contains YYYY-MM-DD subfolders. If the backup container uses a custom \
             file-structure format, select the camera directories manually."
                .into(),
        );
    }

    (candidates, notes)
}

/// Translate a host path into one this container can open.
///
/// The backup container reports paths as *it* sees them; we resolve those to
/// host paths through its mounts, then back into our own mount. If the file
/// isn't under our mount, no amount of guessing helps — say so instead.
pub fn host_to_local(
    host_path: &str,
    host_backup_dir: &str,
    local_backup_dir: &Path,
) -> Option<PathBuf> {
    let base = host_backup_dir.trim_end_matches('/');
    let rest = if host_path == base {
        ""
    } else {
        host_path.strip_prefix(&format!("{base}/"))?
    };
    Some(local_backup_dir.join(rest))
}

/// Re-validate settings against the filesystem on every request.
///
/// Configuration that was true when saved is not necessarily true now: a mount
/// can vanish, permissions can change, the backup container can be recreated.
/// Storing "validated: true" would make the app confidently wrong.
pub fn validate(settings: &Settings, backup_dir: &Path) -> Vec<NamedCheck> {
    let mut checks = Vec::new();

    checks.push(match &settings.upb_container_id {
        Some(id) => NamedCheck {
            name: "Backup container".into(),
            ok: true,
            detail: format!("selected ({})", &id[..id.len().min(12)]),
        },
        None => NamedCheck {
            name: "Backup container".into(),
            ok: false,
            detail: "not selected".into(),
        },
    });

    checks.push(match &settings.events_db_path {
        Some(p) => {
            let path = Path::new(p);
            match std::fs::metadata(path) {
                Ok(m) if m.is_file() => match std::fs::File::open(path) {
                    Ok(_) => NamedCheck {
                        name: "Event database".into(),
                        ok: true,
                        detail: format!("{p} ({} KB)", m.len() / 1024),
                    },
                    Err(e) => NamedCheck {
                        name: "Event database".into(),
                        ok: false,
                        detail: format!("{p} exists but cannot be opened: {e}"),
                    },
                },
                Ok(_) => NamedCheck {
                    name: "Event database".into(),
                    ok: false,
                    detail: format!("{p} is not a file"),
                },
                Err(e) => NamedCheck {
                    name: "Event database".into(),
                    ok: false,
                    detail: format!("{p}: {e}"),
                },
            }
        }
        None => NamedCheck {
            name: "Event database".into(),
            ok: false,
            detail: "not located".into(),
        },
    });

    checks.push(match &settings.clip_path_prefix {
        Some(p) => NamedCheck {
            name: "Clip path prefix".into(),
            ok: true,
            detail: format!("stripping {p} from recorded paths"),
        },
        None => NamedCheck {
            name: "Clip path prefix".into(),
            ok: false,
            detail: "not set — clips cannot be located".into(),
        },
    });

    let missing: Vec<&String> = settings
        .camera_dirs
        .iter()
        .filter(|d| !backup_dir.join(d).is_dir())
        .collect();
    checks.push(if settings.camera_dirs.is_empty() {
        NamedCheck {
            name: "Cameras".into(),
            ok: false,
            detail: "none selected".into(),
        }
    } else if missing.is_empty() {
        NamedCheck {
            name: "Cameras".into(),
            ok: true,
            detail: format!("{} selected", settings.camera_dirs.len()),
        }
    } else {
        NamedCheck {
            name: "Cameras".into(),
            ok: false,
            detail: format!(
                "{} of {} no longer exist: {}",
                missing.len(),
                settings.camera_dirs.len(),
                missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ),
        }
    });

    checks.push(if settings.live_window_months == 0 {
        NamedCheck {
            name: "Live window".into(),
            ok: false,
            detail: "not set".into(),
        }
    } else {
        NamedCheck {
            name: "Live window".into(),
            ok: true,
            detail: format!(
                "clips are expected on disk for {} months, which bounds what the index looks for",
                settings.live_window_months
            ),
        }
    });

    // Reported separately from the live window because they answer different
    // questions, and conflating them is what made archiving look broken: the
    // window is about where a clip is expected to be, this is about when we
    // are allowed to move it.
    checks.push(if settings.archive_after_days == 0 {
        NamedCheck {
            name: "Archive after".into(),
            ok: false,
            detail: "not set".into(),
        }
    } else {
        NamedCheck {
            name: "Archive after".into(),
            ok: true,
            detail: format!(
                "a month is offered for archiving once its newest clip is {} days old",
                settings.archive_after_days
            ),
        }
    });

    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_date_directories() {
        assert!(is_date_dir("2026-08-15"));
        assert!(!is_date_dir("2026-8-15"));
        assert!(!is_date_dir("database"));
        assert!(!is_date_dir("2026-08-155"));
        assert!(!is_date_dir("rclone"));
        // Right shape, wrong content.
        assert!(!is_date_dir("20xx-08-15"));
    }

    #[test]
    fn camera_detection_uses_evidence_not_a_name_blocklist() {
        let root = std::env::temp_dir().join("pm-cams-test");
        let _ = std::fs::remove_dir_all(&root);

        // Three cameras: one plain, one non-ASCII, and one whose name collides
        // with a service directory a blocklist would have excluded.
        for cam in ["Front Door", "Gartenhäuschen", "ufp"] {
            let day = root.join(cam).join("2026-08-15");
            std::fs::create_dir_all(&day).unwrap();
            std::fs::write(day.join("clip.mp4"), b"x").unwrap();
        }
        // Genuine non-camera directories, with no date folders.
        std::fs::create_dir_all(root.join("database")).unwrap();
        std::fs::create_dir_all(root.join("rclone")).unwrap();

        let (cams, _) = find_cameras(&root);
        let picked: Vec<&str> = cams
            .iter()
            .filter(|c| c.looks_like_camera)
            .map(|c| c.dir_name.as_str())
            .collect();

        assert_eq!(picked.len(), 3);
        assert!(picked.contains(&"ufp"), "a camera named ufp must survive");
        assert!(picked.contains(&"Gartenhäuschen"));

        let skipped: Vec<&str> = cams
            .iter()
            .filter(|c| !c.looks_like_camera)
            .map(|c| c.dir_name.as_str())
            .collect();
        assert!(skipped.contains(&"database"));
        assert!(skipped.contains(&"rclone"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn maps_host_paths_into_our_mount() {
        let local = PathBuf::from("/backup");
        assert_eq!(
            host_to_local(
                "/srv/pool/protect/backup-service/database/events.sqlite",
                "/srv/pool/protect/backup-service",
                &local
            ),
            Some(PathBuf::from("/backup/database/events.sqlite"))
        );

        // Trailing slash on the mount source must not break the join.
        assert_eq!(
            host_to_local("/host/dir/db.sqlite", "/host/dir/", &local),
            Some(PathBuf::from("/backup/db.sqlite"))
        );

        // Outside our mount: unreachable, and we must not pretend otherwise.
        assert_eq!(host_to_local("/elsewhere/db.sqlite", "/host/dir", &local), None);

        // A sibling directory sharing a name prefix is not inside the mount.
        assert_eq!(host_to_local("/host/dir2/db.sqlite", "/host/dir", &local), None);
    }

    #[test]
    fn validation_rejects_a_camera_directory_that_disappeared() {
        let root = std::env::temp_dir().join("pm-validate-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("present")).unwrap();

        let settings = Settings {
            camera_dirs: vec!["present".into(), "vanished".into()],
            ..Default::default()
        };
        let checks = validate(&settings, &root);
        let cameras = checks.iter().find(|c| c.name == "Cameras").unwrap();

        assert!(!cameras.ok);
        assert!(cameras.detail.contains("vanished"));

        std::fs::remove_dir_all(&root).unwrap();
    }
}

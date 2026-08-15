//! Working out what should be archived, and where it would go.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use protect_api_types::CameraMonth;

/// The clips belonging to one camera-month, with their day directories.
#[derive(Debug, Clone)]
pub struct MonthContents {
    pub camera: String,
    pub month: String,
    /// The `YYYY-MM-DD` directories that make up this month.
    pub day_dirs: Vec<PathBuf>,
    /// Every clip in those directories, relative to the camera directory.
    pub files: Vec<(PathBuf, String)>,
    pub bytes: i64,
    /// When the most recently written clip in this month was modified.
    ///
    /// Two containers share this directory: the backup service writes here
    /// and we delete from it. Age alone is meant to keep them apart, but a
    /// misconfigured backfill window can put the other process back inside a
    /// month we consider finished — and archiving a file mid-write would
    /// capture a truncated clip and then delete the original.
    pub newest_write: Option<f64>,
}

impl MonthContents {
    pub fn key(&self) -> CameraMonth {
        CameraMonth { camera: self.camera.clone(), month: self.month.clone() }
    }
}

fn is_date_dir(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..].iter().all(u8::is_ascii_digit)
}

/// Where a camera-month's archive lives: `<archive>/<camera>/<YYYY-MM>.tar`.
///
/// Deliberately identical to the layout produced by the shell script this
/// replaces, so existing archives are recognised rather than orphaned.
pub fn archive_path(archive_dir: &Path, camera: &str, month: &str) -> PathBuf {
    archive_dir.join(camera).join(format!("{month}.tar"))
}

/// Scan one camera directory and group its clips by month.
pub fn months_for_camera(backup_dir: &Path, camera: &str) -> Vec<MonthContents> {
    let camera_dir = backup_dir.join(camera);
    let Ok(entries) = std::fs::read_dir(&camera_dir) else {
        return Vec::new();
    };

    let mut months: BTreeMap<String, MonthContents> = BTreeMap::new();

    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let day = entry.file_name().to_string_lossy().to_string();
        if !is_date_dir(&day) {
            continue;
        }
        let month = day[..7].to_string();

        let slot = months.entry(month.clone()).or_insert_with(|| MonthContents {
            camera: camera.to_string(),
            month,
            day_dirs: Vec::new(),
            files: Vec::new(),
            bytes: 0,
            newest_write: None,
        });
        slot.day_dirs.push(entry.path());

        if let Ok(files) = std::fs::read_dir(entry.path()) {
            for f in files.filter_map(Result::ok) {
                if !f.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let name = f.file_name().to_string_lossy().to_string();
                if let Ok(meta) = f.metadata() {
                    slot.bytes += meta.len() as i64;
                    if let Some(modified) = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64())
                    {
                        slot.newest_write =
                            Some(slot.newest_write.map_or(modified, |n: f64| n.max(modified)));
                    }
                }
                // Stored as `<day>/<file>` so unpacking recreates the layout
                // the backup service produced.
                slot.files.push((f.path(), format!("{day}/{name}")));
            }
        }
    }

    let mut out: Vec<MonthContents> = months.into_values().collect();
    for m in &mut out {
        m.files.sort_by(|a, b| a.1.cmp(&b.1));
        m.day_dirs.sort();
    }
    out
}

/// The most recent month that must not be touched.
///
/// Archiving works in whole calendar months, so the boundary is a month
/// rather than a date: a month is eligible only once every day in it is older
/// than the live window. `live_window_months` counted back from the current
/// month gives the first month still protected.
pub fn cutoff_month(now: f64, live_window_months: u32) -> String {
    let days = now / 86_400.0;
    let (mut y, mut m) = civil_from_days(days as i64);
    // Step back one month at a time; month arithmetic has no shortcut that
    // stays correct across year boundaries.
    for _ in 0..live_window_months {
        if m == 1 {
            m = 12;
            y -= 1;
        } else {
            m -= 1;
        }
    }
    format!("{y:04}-{m:02}")
}

/// Days since the Unix epoch to `(year, month)`, via Howard Hinnant's
/// civil-from-days algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32)
}

/// `(year, month, day)` to days since the Unix epoch — the inverse of
/// `civil_from_days`, used to build scheduled timestamps.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_clips_by_calendar_month() {
        let root = std::env::temp_dir().join(format!("pm-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        for (day, count) in [("2026-06-29", 2), ("2026-06-30", 1), ("2026-07-01", 3)] {
            let d = root.join("Front Door").join(day);
            std::fs::create_dir_all(&d).unwrap();
            for i in 0..count {
                std::fs::write(d.join(format!("clip{i}.mp4")), b"xxxx").unwrap();
            }
        }
        // A stray non-date directory must not become a month.
        std::fs::create_dir_all(root.join("Front Door").join("thumbnails")).unwrap();

        let months = months_for_camera(&root, "Front Door");
        assert_eq!(months.len(), 2);
        assert_eq!(months[0].month, "2026-06");
        assert_eq!(months[0].files.len(), 3);
        assert_eq!(months[0].day_dirs.len(), 2);
        assert_eq!(months[1].month, "2026-07");
        assert_eq!(months[1].files.len(), 3);
        // Entry names keep the day directory, so a restore rebuilds the tree.
        assert!(months[0].files[0].1.starts_with("2026-06-29/"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_cutoff_month_steps_back_across_a_year_boundary() {
        // 2026-02-15
        let feb = 1_771_113_600.0;
        assert_eq!(cutoff_month(feb, 0), "2026-02");
        assert_eq!(cutoff_month(feb, 1), "2026-01");
        assert_eq!(cutoff_month(feb, 2), "2025-12");
        assert_eq!(cutoff_month(feb, 14), "2024-12");
    }

    #[test]
    fn civil_date_conversion_round_trips() {
        for (y, m, d) in [(2026, 8, 15), (2024, 2, 29), (1999, 12, 31), (2000, 1, 1)] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m), "failed for {y}-{m}-{d}");
        }
        // The epoch itself.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn archive_paths_match_the_layout_they_replace() {
        assert_eq!(
            archive_path(Path::new("/archive"), "Front Door", "2026-06"),
            PathBuf::from("/archive/Front Door/2026-06.tar")
        );
    }
}

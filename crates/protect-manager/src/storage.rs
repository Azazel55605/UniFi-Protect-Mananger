//! Capacity: what is on disk, and how fast it is filling.
//!
//! Read from the filesystem rather than from a storage appliance's API. That
//! is deliberate: `statvfs` works on any host this container can run on,
//! needs no credentials, and cannot go stale against a vendor's API version.
//! The trade-off is that we see the filesystem as mounted, not the pool
//! underneath it — for "will archiving fit", that is the number that matters.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use protect_api_types::{
    CameraUsage, FilesystemUsage, Settings, StorageSample, StorageSnapshot,
};
use sqlx::{Row, SqlitePool};

/// How long to keep samples. A year of half-hourly points is tiny, and the
/// trend is only interesting over months.
const HISTORY_DAYS: f64 = 365.0;

pub fn usage(path: &Path) -> Option<FilesystemUsage> {
    let stat = statvfs_of(path)?;
    let device = std::fs::metadata(path).map(|m| m.dev() as i64).unwrap_or(0);
    Some(FilesystemUsage {
        path: path.to_string_lossy().to_string(),
        // `f_bavail` rather than `f_bfree`: blocks reserved for root are not
        // space we can actually use.
        total_bytes: (stat.f_blocks.saturating_mul(stat.f_frsize)) as i64,
        free_bytes: (stat.f_bavail.saturating_mul(stat.f_frsize)) as i64,
        device,
    })
}

pub async fn snapshot(
    pool: &SqlitePool,
    settings: &Settings,
    backup_dir: &Path,
    archive_dir: &Path,
) -> anyhow::Result<StorageSnapshot> {
    let backup = usage(backup_dir);
    let archive = usage(archive_dir);
    let same_filesystem = match (&backup, &archive) {
        (Some(b), Some(a)) => b.device == a.device && b.device != 0,
        _ => false,
    };

    // Live bytes come from the event index, which already knows each clip's
    // size — cheaper and more accurate than walking the tree again, and it
    // only counts clips we actually resolved.
    let live: (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(size_bytes), 0), COUNT(*) FROM events WHERE status = 'live'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or((0, 0));

    let archive_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM archives")
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    let cameras = per_camera(pool, settings).await?;
    let history = self::history(pool, 30.0).await.unwrap_or_default();
    let growth = growth_per_day(&history);

    let free = [backup.as_ref(), archive.as_ref()]
        .into_iter()
        .flatten()
        .map(|u| u.free_bytes)
        .min();

    let days_until_full = match (growth, free) {
        (Some(rate), Some(free)) if rate > 0.0 => Some(free as f64 / rate),
        _ => None,
    };

    Ok(StorageSnapshot {
        backup,
        archive,
        same_filesystem,
        live_bytes: live.0,
        archive_bytes,
        cameras,
        growth_bytes_per_day: growth,
        days_until_full,
    })
}

async fn per_camera(pool: &SqlitePool, settings: &Settings) -> anyhow::Result<Vec<CameraUsage>> {
    let live = sqlx::query(
        "SELECT COALESCE(c.display_name, c.derived_name, e.camera_id) AS name,
                COALESCE(SUM(e.size_bytes), 0) AS bytes,
                COUNT(*) AS clips
           FROM events e
           LEFT JOIN cameras c ON c.camera_id = e.camera_id
          WHERE e.status = 'live'
          GROUP BY e.camera_id",
    )
    .fetch_all(pool)
    .await?;

    let archived = sqlx::query(
        "SELECT camera, COALESCE(SUM(size_bytes), 0) AS bytes, COUNT(*) AS months
           FROM archives GROUP BY camera",
    )
    .fetch_all(pool)
    .await?;

    // Cameras are keyed by name here rather than id, because an archive is
    // named after the directory it came from and has no camera id to join on.
    let mut by_name: std::collections::BTreeMap<String, CameraUsage> = Default::default();

    for r in &live {
        let camera: String = r.get("name");
        by_name.insert(
            camera.clone(),
            CameraUsage {
                camera,
                live_bytes: r.get("bytes"),
                live_clips: r.get("clips"),
                archive_bytes: 0,
                archived_months: 0,
            },
        );
    }

    for r in &archived {
        let camera: String = r.get("camera");
        let entry = by_name.entry(camera.clone()).or_insert(CameraUsage {
            camera,
            live_bytes: 0,
            live_clips: 0,
            archive_bytes: 0,
            archived_months: 0,
        });
        entry.archive_bytes = r.get("bytes");
        entry.archived_months = r.get("months");
    }

    // A configured camera with nothing on disk still belongs in the list —
    // an empty row is information, and hiding it looks like the camera is
    // simply unknown.
    for name in &settings.camera_dirs {
        by_name.entry(name.clone()).or_insert(CameraUsage {
            camera: name.clone(),
            live_bytes: 0,
            live_clips: 0,
            archive_bytes: 0,
            archived_months: 0,
        });
    }

    let mut out: Vec<CameraUsage> = by_name.into_values().collect();
    out.sort_by(|a, b| {
        (b.live_bytes + b.archive_bytes).cmp(&(a.live_bytes + a.archive_bytes))
    });
    Ok(out)
}

/// Record where things stand, for the trend.
pub async fn take_sample(
    pool: &SqlitePool,
    settings: &Settings,
    backup_dir: &Path,
    archive_dir: &Path,
) -> anyhow::Result<()> {
    let snap = snapshot(pool, settings, backup_dir, archive_dir).await?;
    let free = snap
        .backup
        .as_ref()
        .or(snap.archive.as_ref())
        .map(|u| u.free_bytes)
        .unwrap_or(0);

    sqlx::query("INSERT INTO storage_samples (at, live_bytes, archive_bytes, free_bytes) VALUES (?, ?, ?, ?)")
        .bind(crate::upb::reconcile::now_secs())
        .bind(snap.live_bytes)
        .bind(snap.archive_bytes)
        .bind(free)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM storage_samples WHERE at < ?")
        .bind(crate::upb::reconcile::now_secs() - HISTORY_DAYS * 86_400.0)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn history(pool: &SqlitePool, days: f64) -> anyhow::Result<Vec<StorageSample>> {
    let since = crate::upb::reconcile::now_secs() - days * 86_400.0;
    let rows = sqlx::query(
        "SELECT at, live_bytes, archive_bytes, free_bytes
           FROM storage_samples WHERE at >= ? ORDER BY at",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| StorageSample {
            at: r.get("at"),
            live_bytes: r.get("live_bytes"),
            archive_bytes: r.get("archive_bytes"),
            free_bytes: r.get("free_bytes"),
        })
        .collect())
}

/// Bytes added per day, from the oldest and newest samples.
///
/// A straight line between the ends rather than a fitted curve: with points
/// every half hour and a question as coarse as "roughly when do I run out",
/// anything cleverer would be false precision. Returns `None` until there is
/// a day of history, because a rate extrapolated from minutes is noise.
pub fn growth_per_day(history: &[StorageSample]) -> Option<f64> {
    let first = history.first()?;
    let last = history.last()?;
    let span_days = (last.at - first.at) / 86_400.0;
    if span_days < 1.0 {
        return None;
    }
    let grew = (last.live_bytes + last.archive_bytes) - (first.live_bytes + first.archive_bytes);
    Some(grew as f64 / span_days)
}

#[repr(C)]
#[allow(non_camel_case_types)]
struct Statvfs {
    f_bsize: u64,
    f_frsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: u64,
    f_flag: u64,
    f_namemax: u64,
    f_spare: [i32; 6],
}

extern "C" {
    fn statvfs(path: *const std::ffi::c_char, buf: *mut Statvfs) -> i32;
}

fn statvfs_of(path: &Path) -> Option<Statvfs> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: Statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { statvfs(c.as_ptr(), &mut stat) };
    (rc == 0).then_some(stat)
}

/// Free bytes on the filesystem holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let stat = statvfs_of(path)?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_usage_for_a_real_directory() {
        let u = usage(&std::env::temp_dir()).expect("temp dir is on a filesystem");
        assert!(u.total_bytes > 0);
        assert!(u.free_bytes <= u.total_bytes);
        assert!(u.device != 0);
    }

    #[test]
    fn a_missing_path_reports_nothing_rather_than_zero() {
        // Zero would render as a full disk, which is a worse lie than "unknown".
        assert!(usage(Path::new("/definitely/not/here")).is_none());
    }

    fn sample(at: f64, live: i64, archive: i64) -> StorageSample {
        StorageSample { at, live_bytes: live, archive_bytes: archive, free_bytes: 0 }
    }

    #[test]
    fn growth_needs_a_day_before_it_will_guess() {
        let hour = vec![sample(0.0, 0, 0), sample(3600.0, 1_000_000, 0)];
        assert_eq!(growth_per_day(&hour), None, "an hour cannot predict a month");

        let week = vec![sample(0.0, 0, 0), sample(7.0 * 86_400.0, 700, 0)];
        assert_eq!(growth_per_day(&week), Some(100.0));
    }

    #[test]
    fn archiving_that_frees_space_shows_as_negative_growth() {
        // Live shrinks by more than the archive grows, because a tar of many
        // clips is smaller than the directory tree it replaced.
        let h = vec![
            sample(0.0, 10_000, 0),
            sample(2.0 * 86_400.0, 2_000, 6_000),
        ];
        assert_eq!(growth_per_day(&h), Some(-1000.0));
    }

    #[test]
    fn no_history_means_no_claim() {
        assert_eq!(growth_per_day(&[]), None);
        assert_eq!(growth_per_day(&[sample(0.0, 1, 1)]), None);
    }
}

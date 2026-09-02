//! Running archive, restore and verify jobs, and recording what happened.
//!
//! The order of operations is the whole point: pack, verify, *then* delete.
//! Nothing removes a source file until the archive holding it has been read
//! back and compared byte for byte.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use protect_api_types::{
    ArchiveEntry, ArchiveOverview, ArchiveRun, CameraMonth, DueEntry, RunKind, RunProgress,
    RunStatus, Settings,
};
use sqlx::{Row, SqlitePool};
use tokio::sync::{broadcast, Mutex};

use super::{pack, plan};

/// How recently a month must have been written to for archiving to hold off.
///
/// The backup service and this app share the clip directory, and age is what
/// normally keeps them apart. This is the belt to that braces: whatever the
/// configuration says, a month that was written to minutes ago is not
/// finished, and packing a file mid-write would archive a truncated clip and
/// then delete the only other copy.
const RECENT_WRITE_SECS: f64 = 3600.0;

/// Only one job at a time. Two archives writing the same tar, or a restore
/// racing an archive over the same month, is not a situation worth handling
/// gracefully — it is one worth preventing.
pub type JobLock = Arc<Mutex<()>>;

#[derive(Clone)]
pub struct Jobs {
    pub lock: JobLock,
    pub progress: broadcast::Sender<RunProgress>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self { lock: Arc::new(Mutex::new(())), progress: broadcast::channel(256).0 }
    }
}

fn now() -> f64 {
    crate::upb::reconcile::now_secs()
}

/// Say that a job has started before doing anything slow.
///
/// Reading an archive's index means walking every header, which on a large tar
/// takes long enough that the UI would otherwise sit blank and look broken.
/// The first thing a job does is announce itself.
fn announce(jobs: &Jobs, run_id: i64, kind: RunKind, target: Option<&CameraMonth>, phase: &str) {
    let _ = jobs.progress.send(RunProgress {
        run_id,
        kind,
        camera: target.map(|t| t.camera.clone()),
        month: target.map(|t| t.month.clone()),
        phase: phase.into(),
        current_file: None,
        files_done: 0,
        files_total: 0,
        overall_done: 0,
        overall_total: 0,
        finished: false,
        status: None,
        message: None,
    });
}

fn kind_str(k: RunKind) -> &'static str {
    match k {
        RunKind::Archive => "archive",
        RunKind::Restore => "restore",
        RunKind::Verify => "verify",
    }
}

fn kind_from(s: &str) -> RunKind {
    match s {
        "restore" => RunKind::Restore,
        "verify" => RunKind::Verify,
        _ => RunKind::Archive,
    }
}

fn status_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Running => "running",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Interrupted => "interrupted",
    }
}

fn status_from(s: &str) -> RunStatus {
    match s {
        "running" => RunStatus::Running,
        "succeeded" => RunStatus::Succeeded,
        "interrupted" => RunStatus::Interrupted,
        _ => RunStatus::Failed,
    }
}

// ------------------------------------------------------------ run records

async fn start_run(
    pool: &SqlitePool,
    kind: RunKind,
    target: Option<&CameraMonth>,
    dry_run: bool,
    scheduled: bool,
    files_total: i64,
) -> anyhow::Result<i64> {
    let id = sqlx::query(
        "INSERT INTO archive_runs (kind, status, camera, month, started, dry_run, scheduled, files_total)
         VALUES (?, 'running', ?, ?, ?, ?, ?, ?)",
    )
    .bind(kind_str(kind))
    .bind(target.map(|t| t.camera.clone()))
    .bind(target.map(|t| t.month.clone()))
    .bind(now())
    .bind(dry_run as i32)
    .bind(scheduled as i32)
    .bind(files_total)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

async fn finish_run(
    pool: &SqlitePool,
    id: i64,
    status: RunStatus,
    message: Option<String>,
    files_done: i64,
    bytes: i64,
    failed: &[String],
) {
    let _ = sqlx::query(
        "UPDATE archive_runs
            SET status = ?, finished = ?, message = ?, files_done = ?, bytes_total = ?,
                failed_files = ?
          WHERE id = ?",
    )
    .bind(status_str(status))
    .bind(now())
    .bind(message)
    .bind(files_done)
    .bind(bytes)
    .bind(failed.join("\n"))
    .bind(id)
    .execute(pool)
    .await;
}

pub async fn recent_runs(pool: &SqlitePool, limit: i64) -> anyhow::Result<Vec<ArchiveRun>> {
    let rows = sqlx::query("SELECT * FROM archive_runs ORDER BY started DESC LIMIT ?")
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_run).collect())
}

fn row_to_run(r: &sqlx::sqlite::SqliteRow) -> ArchiveRun {
    let failed: String = r.get("failed_files");
    ArchiveRun {
        id: r.get("id"),
        kind: kind_from(&r.get::<String, _>("kind")),
        status: status_from(&r.get::<String, _>("status")),
        camera: r.get("camera"),
        month: r.get("month"),
        started: r.get("started"),
        finished: r.get("finished"),
        dry_run: r.get::<i64, _>("dry_run") != 0,
        scheduled: r.get::<i64, _>("scheduled") != 0,
        files_total: r.get("files_total"),
        files_done: r.get("files_done"),
        bytes_total: r.get("bytes_total"),
        message: r.get("message"),
        failed_files: failed.lines().map(str::to_string).filter(|s| !s.is_empty()).collect(),
    }
}

// ------------------------------------------------------------- inventory

/// What exists, what is due, and what has gone missing.
pub async fn overview(
    pool: &SqlitePool,
    settings: &Settings,
    backup_dir: &Path,
    archive_dir: &Path,
) -> anyhow::Result<ArchiveOverview> {
    let recorded = sqlx::query("SELECT * FROM archives ORDER BY camera, month")
        .fetch_all(pool)
        .await?;

    let mut archives: Vec<ArchiveEntry> = Vec::new();
    let mut missing_archives = Vec::new();

    for r in &recorded {
        let camera: String = r.get("camera");
        let month: String = r.get("month");
        let path: String = r.get("path");
        if !Path::new(&path).is_file() {
            // Recorded as archived, but the tar is gone. Silently dropping
            // this would hide the loss of a whole month of footage.
            missing_archives.push(CameraMonth { camera, month });
            continue;
        }
        archives.push(ArchiveEntry {
            camera,
            month,
            path,
            size_bytes: r.get("size_bytes"),
            file_count: r.get("file_count"),
            created: r.get("created"),
            verified_at: r.get("verified_at"),
            verify_ok: r.get::<Option<i64>, _>("verify_ok").map(|v| v != 0),
            pinned: r.get::<i64, _>("pinned") != 0,
            unrecorded: false,
        });
    }

    // Tars on disk we have no record of: made by hand, or by whatever handled
    // archiving before this app did. They are real archives and belong in the
    // list, flagged so the difference is visible.
    if let Ok(cameras) = std::fs::read_dir(archive_dir) {
        for cam in cameras.filter_map(Result::ok) {
            if !cam.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let camera = cam.file_name().to_string_lossy().to_string();
            let Ok(files) = std::fs::read_dir(cam.path()) else { continue };
            for f in files.filter_map(Result::ok) {
                let name = f.file_name().to_string_lossy().to_string();
                let Some(month) = name.strip_suffix(".tar") else { continue };
                if archives.iter().any(|a| a.camera == camera && a.month == month) {
                    continue;
                }
                let meta = f.metadata().ok();
                archives.push(ArchiveEntry {
                    camera: camera.clone(),
                    month: month.to_string(),
                    path: f.path().to_string_lossy().to_string(),
                    size_bytes: meta.as_ref().map(|m| m.len() as i64).unwrap_or(0),
                    // Counting entries means a header walk per archive; left
                    // for the on-demand verify rather than done on every list.
                    file_count: 0,
                    created: None,
                    verified_at: None,
                    verify_ok: None,
                    pinned: false,
                    unrecorded: true,
                });
            }
        }
    }
    archives.sort_by(|a, b| a.camera.cmp(&b.camera).then(b.month.cmp(&a.month)));

    let due = due_months(pool, settings, backup_dir, &archives).await?;
    let total_bytes = archives.iter().map(|a| a.size_bytes).sum();

    let running = sqlx::query("SELECT * FROM archive_runs WHERE status = 'running' LIMIT 1")
        .fetch_optional(pool)
        .await?
        .map(|r| row_to_run(&r));

    Ok(ArchiveOverview { archives, due, missing_archives, total_bytes, running })
}

/// Camera-months old enough to archive that haven't been.
pub async fn due_months(
    pool: &SqlitePool,
    settings: &Settings,
    backup_dir: &Path,
    archives: &[ArchiveEntry],
) -> anyhow::Result<Vec<DueEntry>> {
    let now = now();
    // The current calendar month is never eligible, whatever the threshold
    // says: a month is archived whole or not at all, and this one is still
    // being written to.
    let current_month = plan::cutoff_month(now, 0);
    // Clamped to a day, because zero would mean "archive footage recorded this
    // morning" and nothing below would catch it — `RECENT_WRITE_SECS` guards
    // an hour, not a day.
    let min_age = settings.archive_after_days.max(1) as f64 * 86_400.0;
    let pinned: Vec<(String, String)> =
        sqlx::query("SELECT camera, month FROM archives WHERE pinned = 1")
            .fetch_all(pool)
            .await?
            .iter()
            .map(|r| (r.get("camera"), r.get("month")))
            .collect();

    let mut due = Vec::new();
    for camera in &settings.camera_dirs {
        for month in plan::months_for_camera(backup_dir, camera) {
            // Whole months only, and never the one still being written to.
            if month.month >= current_month || month.files.is_empty() {
                continue;
            }
            let written_recently = month
                .newest_write
                .map(|at| now - at < RECENT_WRITE_SECS)
                .unwrap_or(false);
            // A month with no readable day directory has no age we can trust,
            // so it is held rather than archived on a guess.
            let age = month.newest_day_end().map(|end| now - end);

            let blocked = if pinned.iter().any(|(c, m)| c == camera && *m == month.month) {
                Some("restored and pinned — release it to allow archiving".to_string())
            } else if archives
                .iter()
                .any(|a| a.camera == *camera && a.month == month.month)
            {
                Some("an archive already exists for this month".to_string())
            } else if written_recently {
                Some(
                    "the backup service wrote to this month within the last hour —                      archiving would risk capturing a clip mid-write"
                        .to_string(),
                )
            } else if let Some(days) = age
                .filter(|a| *a < min_age)
                .map(|a| (a / 86_400.0).floor() as i64)
            {
                // Said as a countdown rather than a rule: the question being
                // asked is "why not this one", and the answer that helps is
                // when it changes.
                Some(format!(
                    "{days} days old — archived at {}",
                    plural_days(settings.archive_after_days.max(1)),
                ))
            } else if age.is_none() {
                Some("no dated clip folders, so its age is unknown".to_string())
            } else {
                None
            };

            due.push(DueEntry {
                camera: camera.clone(),
                month: month.month.clone(),
                file_count: month.files.len() as i64,
                bytes: month.bytes,
                blocked,
            });
        }
    }
    due.sort_by(|a, b| a.month.cmp(&b.month).then(a.camera.cmp(&b.camera)));
    Ok(due)
}

fn plural_days(n: u32) -> String {
    if n == 1 { "1 day".into() } else { format!("{n} days") }
}

/// The oldest camera-month present on disk, whether or not it is due.
fn oldest_month(settings: &Settings, backup_dir: &Path) -> Option<CameraMonth> {
    settings
        .camera_dirs
        .iter()
        .flat_map(|c| plan::months_for_camera(backup_dir, c))
        .filter(|m| !m.files.is_empty())
        .min_by(|a, b| a.month.cmp(&b.month))
        .map(|m| m.key())
}

// ------------------------------------------------------------------ jobs

pub struct JobContext {
    pub pool: SqlitePool,
    pub jobs: Jobs,
    pub backup_dir: PathBuf,
    pub archive_dir: PathBuf,
}

/// Archive the given camera-months, or everything due when none are given.
pub async fn run_archive(
    ctx: JobContext,
    settings: Settings,
    targets: Vec<CameraMonth>,
    dry_run: bool,
    scheduled: bool,
) -> anyhow::Result<i64> {
    let guard = ctx.jobs.lock.clone().try_lock_owned();
    let Ok(guard) = guard else {
        anyhow::bail!("another job is already running");
    };

    let overview = overview(&ctx.pool, &settings, &ctx.backup_dir, &ctx.archive_dir).await?;
    let selected: Vec<CameraMonth> = if targets.is_empty() {
        overview
            .due
            .iter()
            .filter(|d| d.blocked.is_none())
            .map(|d| CameraMonth { camera: d.camera.clone(), month: d.month.clone() })
            .collect()
    } else {
        targets
    };

    // A dry run with nothing due would otherwise do nothing at all, which is
    // useless precisely when you want to test the mechanism — most of the
    // time nothing *is* due. Preview the oldest month instead; a dry run
    // writes and deletes nothing either way.
    let mut previewing_not_due = false;
    let selected = if selected.is_empty() && dry_run {
        match oldest_month(&settings, &ctx.backup_dir) {
            Some(m) => {
                previewing_not_due = true;
                vec![m]
            }
            None => anyhow::bail!("there are no clips to preview"),
        }
    } else {
        selected
    };

    if selected.is_empty() {
        anyhow::bail!("nothing to archive");
    }

    // Check we can write before claiming a run. A permission problem found
    // here is a sentence naming the directory and the fix; the same problem
    // found inside `pack` is a bare "Permission denied (os error 13)" attached
    // to a failed run, halfway down the history. A dry run writes nothing, so
    // it is deliberately allowed through — being able to preview without a
    // writable archive mount is useful while you are still fixing the mount.
    if !dry_run {
        let check = crate::health::check_archive_dir(&ctx.archive_dir);
        if !check.ok {
            anyhow::bail!("{}", check.detail);
        }
    }

    let months: Vec<plan::MonthContents> = selected
        .iter()
        .filter_map(|t| {
            plan::months_for_camera(&ctx.backup_dir, &t.camera)
                .into_iter()
                .find(|m| m.month == t.month)
        })
        .collect();

    let overall_total: i64 = months.iter().map(|m| m.files.len() as i64).sum();
    let single = (months.len() == 1).then(|| months[0].key());
    let run_id = start_run(
        &ctx.pool,
        RunKind::Archive,
        single.as_ref(),
        dry_run,
        scheduled,
        overall_total,
    )
    .await?;

    announce(&ctx.jobs, run_id, RunKind::Archive, single.as_ref(), "preparing");

    tokio::spawn(async move {
        let _guard = guard;
        let result =
            archive_months(&ctx, &settings, run_id, months, overall_total, dry_run).await;

        let (status, message, done, bytes, failed) = match result {
            Ok(o) => {
                let mut message = o.message;
                if previewing_not_due {
                    message.push_str(
                        " (nothing is due yet, so this previewed the oldest month)",
                    );
                }
                (
                    if o.failed.is_empty() { RunStatus::Succeeded } else { RunStatus::Failed },
                    Some(message),
                    o.done,
                    o.bytes,
                    o.failed,
                )
            }
            Err(e) => (RunStatus::Failed, Some(e.to_string()), 0, 0, Vec::new()),
        };

        finish_run(&ctx.pool, run_id, status, message.clone(), done, bytes, &failed).await;
        let _ = ctx.jobs.progress.send(RunProgress {
            run_id,
            kind: RunKind::Archive,
            camera: None,
            month: None,
            phase: "done".into(),
            current_file: None,
            files_done: done,
            files_total: overall_total,
            overall_done: done,
            overall_total,
            finished: true,
            status: Some(status),
            message,
        });
    });

    Ok(run_id)
}

struct ArchiveOutcome {
    done: i64,
    bytes: i64,
    failed: Vec<String>,
    message: String,
}

async fn archive_months(
    ctx: &JobContext,
    settings: &Settings,
    run_id: i64,
    months: Vec<plan::MonthContents>,
    overall_total: i64,
    dry_run: bool,
) -> anyhow::Result<ArchiveOutcome> {
    let mut overall_done = 0i64;
    let mut bytes = 0i64;
    let mut failed = Vec::new();
    let mut archived = 0usize;
    let mut skipped = Vec::new();

    for month in months {
        let dest = plan::archive_path(&ctx.archive_dir, &month.camera, &month.month);
        if dest.exists() {
            skipped.push(format!("{}/{}", month.camera, month.month));
            overall_done += month.files.len() as i64;
            continue;
        }

        if dry_run {
            overall_done += month.files.len() as i64;
            bytes += month.bytes;
            archived += 1;
            emit(
                ctx,
                run_id,
                &month,
                "would archive",
                None,
                Counts {
                    done: month.files.len() as i64,
                    total: month.files.len() as i64,
                    overall_done,
                    overall_total,
                },
            );
            continue;
        }

        // Packing and verifying are long, blocking, I/O-heavy jobs; they must
        // not sit on the async runtime's worker threads.
        let progress = ctx.jobs.progress.clone();
        let month_for_task = month.clone();
        let dest_for_task = dest.clone();
        let base = overall_done;

        let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let dest_label = dest_for_task.display().to_string();
            let packed = pack::pack(&month_for_task, &dest_for_task, |p| {
                let _ = progress.send(RunProgress {
                    run_id,
                    kind: RunKind::Archive,
                    camera: Some(month_for_task.camera.clone()),
                    month: Some(month_for_task.month.clone()),
                    phase: "packing".into(),
                    current_file: Some(p.name.to_string()),
                    files_done: p.index as i64 + 1,
                    files_total: p.total as i64,
                    overall_done: base + p.index as i64 + 1,
                    overall_total,
                    finished: false,
                    status: None,
                    message: None,
                });
            })
            .map_err(|e| e.context(format!("writing {dest_label}")))?;

            let verified = pack::verify(&dest_for_task, &packed.hashes, |p| {
                let _ = progress.send(RunProgress {
                    run_id,
                    kind: RunKind::Archive,
                    camera: Some(month_for_task.camera.clone()),
                    month: Some(month_for_task.month.clone()),
                    phase: "verifying".into(),
                    current_file: Some(p.name.to_string()),
                    files_done: p.index as i64 + 1,
                    files_total: p.total as i64,
                    overall_done: base + p.total as i64,
                    overall_total,
                    finished: false,
                    status: None,
                    message: None,
                });
            })
            .map_err(|e| e.context(format!("verifying {dest_label}")))?;

            Ok((packed.bytes_written, packed.hashes.len(), verified))
        })
        .await??;

        let (written, count, verified) = outcome;
        overall_done += month.files.len() as i64;
        bytes += written as i64;

        if !verified.ok() {
            // The archive is not a faithful copy, so the originals stay put
            // and the bad tar is removed rather than left to look finished.
            let _ = std::fs::remove_file(&dest);
            failed.extend(verified.failed_files());
            tracing::error!(
                "verification failed for {}/{}: {} — sources left in place",
                month.camera,
                month.month,
                verified.summary()
            );
            continue;
        }

        record_archive(&ctx.pool, &month, &dest, written as i64, count as i64).await;

        if settings.keep_sources_after_archive {
            tracing::info!("{}/{} archived; sources kept by configuration", month.camera, month.month);
        } else {
            emit(
                ctx,
                run_id,
                &month,
                "removing originals",
                None,
                Counts {
                    done: count as i64,
                    total: count as i64,
                    overall_done,
                    overall_total,
                },
            );
            for dir in &month.day_dirs {
                if let Err(e) = std::fs::remove_dir_all(dir) {
                    tracing::error!("archived {}/{} but could not remove {}: {e}",
                        month.camera, month.month, dir.display());
                }
            }
        }
        archived += 1;
    }

    let mut message = if dry_run {
        format!("dry run: would archive {archived} camera-month(s)")
    } else {
        format!("archived {archived} camera-month(s)")
    };
    if !skipped.is_empty() {
        message.push_str(&format!("; skipped {} that already exist", skipped.len()));
    }
    if !failed.is_empty() {
        message.push_str(&format!("; {} file(s) failed verification", failed.len()));
    }

    Ok(ArchiveOutcome { done: overall_done, bytes, failed, message })
}

/// Counters for one progress update, grouped because nine positional
/// arguments is a call site nobody can read.
struct Counts {
    done: i64,
    total: i64,
    overall_done: i64,
    overall_total: i64,
}

fn emit(
    ctx: &JobContext,
    run_id: i64,
    month: &plan::MonthContents,
    phase: &str,
    file: Option<String>,
    counts: Counts,
) {
    let Counts { done, total, overall_done, overall_total } = counts;
    let _ = ctx.jobs.progress.send(RunProgress {
        run_id,
        kind: RunKind::Archive,
        camera: Some(month.camera.clone()),
        month: Some(month.month.clone()),
        phase: phase.into(),
        current_file: file,
        files_done: done,
        files_total: total,
        overall_done,
        overall_total,
        finished: false,
        status: None,
        message: None,
    });
}

async fn record_archive(
    pool: &SqlitePool,
    month: &plan::MonthContents,
    dest: &Path,
    size: i64,
    files: i64,
) {
    let _ = sqlx::query(
        "INSERT INTO archives (camera, month, path, size_bytes, file_count, created,
                               verified_at, verify_ok, pinned)
         VALUES (?, ?, ?, ?, ?, ?, ?, 1, 0)
         ON CONFLICT(camera, month) DO UPDATE SET
            path = excluded.path, size_bytes = excluded.size_bytes,
            file_count = excluded.file_count, verified_at = excluded.verified_at,
            verify_ok = 1, pinned = 0",
    )
    .bind(&month.camera)
    .bind(&month.month)
    .bind(dest.to_string_lossy().to_string())
    .bind(size)
    .bind(files)
    .bind(now())
    .bind(now())
    .execute(pool)
    .await;
}

/// Restore an archived camera-month back into the live directory.
pub async fn run_restore(ctx: JobContext, target: CameraMonth) -> anyhow::Result<i64> {
    let guard = ctx.jobs.lock.clone().try_lock_owned();
    let Ok(guard) = guard else {
        anyhow::bail!("another job is already running");
    };

    let archive = plan::archive_path(&ctx.archive_dir, &target.camera, &target.month);
    if !archive.is_file() {
        anyhow::bail!("no archive at {}", archive.display());
    }

    let run_id = start_run(&ctx.pool, RunKind::Restore, Some(&target), false, false, 0).await?;
    announce(&ctx.jobs, run_id, RunKind::Restore, Some(&target), "reading archive index");

    // Restoring writes back everything the archive holds, so refusing early
    // beats filling the disk halfway through. Walking the headers of a large
    // tar is slow, so it happens off the async runtime — doing it inline
    // stalls every other request while it runs.
    let index_path = archive.clone();
    let entries = tokio::task::spawn_blocking(move || pack::list(&index_path)).await??;
    let needed: u64 = entries.iter().map(|(_, size)| size).sum();
    if let Some(free) = crate::storage::free_space(&ctx.backup_dir) {
        if free < needed + needed / 10 {
            let msg = format!(
                "restoring needs about {} MB but only {} MB is free",
                needed / 1_048_576,
                free / 1_048_576
            );
            finish_run(&ctx.pool, run_id, RunStatus::Failed, Some(msg.clone()), 0, 0, &[]).await;
            anyhow::bail!(msg);
        }
    }

    let _ = sqlx::query("UPDATE archive_runs SET files_total = ? WHERE id = ?")
        .bind(entries.len() as i64)
        .bind(run_id)
        .execute(&ctx.pool)
        .await;

    tokio::spawn(async move {
        let _guard = guard;
        let dest = ctx.backup_dir.join(&target.camera);
        let progress = ctx.jobs.progress.clone();
        let total = entries.len() as i64;
        let t = target.clone();

        let result = tokio::task::spawn_blocking(move || {
            pack::unpack(&archive, &dest, |p| {
                let _ = progress.send(RunProgress {
                    run_id,
                    kind: RunKind::Restore,
                    camera: Some(t.camera.clone()),
                    month: Some(t.month.clone()),
                    phase: "restoring".into(),
                    current_file: Some(p.name.to_string()),
                    files_done: p.index as i64 + 1,
                    files_total: p.total as i64,
                    overall_done: p.index as i64 + 1,
                    overall_total: p.total as i64,
                    finished: false,
                    status: None,
                    message: None,
                });
            })
        })
        .await;

        let (status, message, done) = match result {
            Ok(Ok(count)) => {
                // Pin it, or the next scheduled run archives it straight back
                // — the month is older than the live window by definition.
                let _ = sqlx::query(
                    "UPDATE archives SET pinned = 1 WHERE camera = ? AND month = ?",
                )
                .bind(&target.camera)
                .bind(&target.month)
                .execute(&ctx.pool)
                .await;
                (
                    RunStatus::Succeeded,
                    format!("restored {count} files; month pinned so it will not be re-archived"),
                    count as i64,
                )
            }
            Ok(Err(e)) => (RunStatus::Failed, e.to_string(), 0),
            Err(e) => (RunStatus::Failed, e.to_string(), 0),
        };

        finish_run(&ctx.pool, run_id, status, Some(message.clone()), done, 0, &[]).await;
        let _ = ctx.jobs.progress.send(RunProgress {
            run_id,
            kind: RunKind::Restore,
            camera: Some(target.camera),
            month: Some(target.month),
            phase: "done".into(),
            current_file: None,
            files_done: done,
            files_total: total,
            overall_done: done,
            overall_total: total,
            finished: true,
            status: Some(status),
            message: Some(message),
        });
    });

    Ok(run_id)
}

/// Re-verify an archive that already exists, without touching anything else.
pub async fn run_verify(ctx: JobContext, target: CameraMonth) -> anyhow::Result<i64> {
    let guard = ctx.jobs.lock.clone().try_lock_owned();
    let Ok(guard) = guard else {
        anyhow::bail!("another job is already running");
    };

    let archive = plan::archive_path(&ctx.archive_dir, &target.camera, &target.month);
    if !archive.is_file() {
        anyhow::bail!("no archive at {}", archive.display());
    }

    let run_id = start_run(&ctx.pool, RunKind::Verify, Some(&target), false, false, 0).await?;
    announce(&ctx.jobs, run_id, RunKind::Verify, Some(&target), "reading archive index");

    tokio::spawn(async move {
        let _guard = guard;
        let progress = ctx.jobs.progress.clone();
        let t = target.clone();

        // Without stored hashes this can only confirm the archive reads back
        // cleanly end to end — structure, not content. Said plainly rather
        // than reported as a full verification.
        let result = tokio::task::spawn_blocking(move || {
            let entries = pack::list(&archive)?;
            let total = entries.len();
            let expected = std::collections::BTreeMap::new();
            let r = pack::verify(&archive, &expected, |p| {
                let _ = progress.send(RunProgress {
                    run_id,
                    kind: RunKind::Verify,
                    camera: Some(t.camera.clone()),
                    month: Some(t.month.clone()),
                    phase: "reading".into(),
                    current_file: Some(p.name.to_string()),
                    files_done: p.index as i64 + 1,
                    files_total: total as i64,
                    overall_done: p.index as i64 + 1,
                    overall_total: total as i64,
                    finished: false,
                    status: None,
                    message: None,
                });
            })?;
            Ok::<_, anyhow::Error>((total, r.checked))
        })
        .await;

        let (status, message, done) = match result {
            Ok(Ok((total, read))) => (
                RunStatus::Succeeded,
                format!("{read} of {total} entries read back without error"),
                read as i64,
            ),
            Ok(Err(e)) => (RunStatus::Failed, format!("archive is unreadable: {e}"), 0),
            Err(e) => (RunStatus::Failed, e.to_string(), 0),
        };

        let ok = matches!(status, RunStatus::Succeeded);
        let _ = sqlx::query(
            "UPDATE archives SET verified_at = ?, verify_ok = ? WHERE camera = ? AND month = ?",
        )
        .bind(now())
        .bind(ok as i32)
        .bind(&target.camera)
        .bind(&target.month)
        .execute(&ctx.pool)
        .await;

        finish_run(&ctx.pool, run_id, status, Some(message.clone()), done, 0, &[]).await;
        let _ = ctx.jobs.progress.send(RunProgress {
            run_id,
            kind: RunKind::Verify,
            camera: Some(target.camera),
            month: Some(target.month),
            phase: "done".into(),
            current_file: None,
            files_done: done,
            files_total: done,
            overall_done: done,
            overall_total: done,
            finished: true,
            status: Some(status),
            message: Some(message),
        });
    });

    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env {
        root: PathBuf,
        pool: SqlitePool,
        settings: Settings,
    }

    /// Two months of clips: one old enough to archive, one still live.
    async fn env(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("pm-run-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let backup = root.join("backup");

        // `now` is inside the current month, so an old month is safely past a
        // 1-month live window whatever today's date happens to be.
        let old = plan::cutoff_month(now(), 3);
        let current = plan::cutoff_month(now(), 0);

        for (month, days) in [(&old, ["01", "02"]), (&current, ["01", "02"])] {
            for day in days {
                let d = backup.join("Front Door").join(format!("{month}-{day}"));
                std::fs::create_dir_all(&d).unwrap();
                for i in 0..3 {
                    let path = d.join(format!("clip{i}.mp4"));
                    std::fs::write(&path, format!("{month}{day}{i}").repeat(50)).unwrap();
                    // Backdate, because a real clip in an old month was
                    // written back then — and archiving deliberately holds off
                    // on months touched in the last hour.
                    backdate(&path, 40.0 * 86_400.0);
                }
            }
        }

        // The archive root exists but is empty, which is what a deployment
        // looks like: it is a bind mount, so it is always there before the
        // first run.
        std::fs::create_dir_all(root.join("archive")).unwrap();

        let pool = crate::db::connect(&root.join("state")).await.unwrap();
        let settings = Settings {
            camera_dirs: vec!["Front Door".into()],
            live_window_months: 1,
            archive_after_days: 30,
            setup_complete: true,
            ..Default::default()
        };
        Env { root, pool, settings }
    }

    fn is_empty(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir).map(|mut e| e.next().is_none()).unwrap_or(false)
    }

    /// Set a file's modification time to `age` seconds ago.
    fn backdate(path: &std::path::Path, age: f64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs_f64(age);
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(when).unwrap();
    }

    fn ctx(e: &Env, jobs: &Jobs) -> JobContext {
        JobContext {
            pool: e.pool.clone(),
            jobs: jobs.clone(),
            backup_dir: e.root.join("backup"),
            archive_dir: e.root.join("archive"),
        }
    }

    async fn wait_for_finish(jobs: &Jobs, mut rx: broadcast::Receiver<RunProgress>) -> RunProgress {
        let _ = jobs;
        loop {
            let p = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
                .await
                .expect("job did not finish in time")
                .expect("progress channel closed");
            if p.finished {
                return p;
            }
        }
    }

    #[tokio::test]
    async fn the_current_month_is_never_due() {
        let e = env("due").await;
        let overview =
            overview(&e.pool, &e.settings, &e.root.join("backup"), &e.root.join("archive"))
                .await
                .unwrap();

        // Two months exist on disk; only the older one is eligible.
        assert_eq!(overview.due.len(), 1, "the current month must not be archived");
        assert_eq!(overview.due[0].file_count, 6);

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    /// The bug this replaced: the threshold was a whole-month count stepped
    /// back from the current month, so its minimum was somewhere between one
    /// and two months depending on today's date, and nothing shorter could be
    /// asked for. A deployment a few months old had nothing due, ever.
    #[tokio::test]
    async fn the_age_threshold_is_in_days_and_can_be_short() {
        let mut e = env("days").await;

        // The fixture's old month is three months back, so it clears any
        // short threshold whatever today's date is.
        e.settings.archive_after_days = 14;
        let short =
            overview(&e.pool, &e.settings, &e.root.join("backup"), &e.root.join("archive"))
                .await
                .unwrap();
        assert_eq!(short.due.iter().filter(|d| d.blocked.is_none()).count(), 1);

        // And a threshold longer than the footage holds it back with a reason
        // that says when it changes, rather than hiding it.
        e.settings.archive_after_days = 3650;
        let long =
            overview(&e.pool, &e.settings, &e.root.join("backup"), &e.root.join("archive"))
                .await
                .unwrap();
        assert_eq!(long.due.iter().filter(|d| d.blocked.is_none()).count(), 0);
        let held = long.due.iter().find(|d| d.blocked.is_some()).expect("held, not hidden");
        assert!(
            held.blocked.as_deref().unwrap().contains("archived at 3650 days"),
            "{:?}",
            held.blocked
        );

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn a_dry_run_writes_and_deletes_nothing() {
        let e = env("dryrun").await;
        let jobs = Jobs::default();
        let rx = jobs.progress.subscribe();

        run_archive(ctx(&e, &jobs), e.settings.clone(), Vec::new(), true, false)
            .await
            .unwrap();
        let done = wait_for_finish(&jobs, rx).await;

        assert_eq!(done.status, Some(RunStatus::Succeeded));
        assert!(done.message.unwrap().contains("dry run"));
        assert!(is_empty(&e.root.join("archive")), "a dry run must not write an archive");
        let old = plan::cutoff_month(now(), 3);
        assert!(e.root.join("backup/Front Door").join(format!("{old}-01")).exists());

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn archiving_verifies_then_removes_the_originals() {
        let e = env("archive").await;
        let jobs = Jobs::default();
        let rx = jobs.progress.subscribe();
        let old = plan::cutoff_month(now(), 3);

        run_archive(ctx(&e, &jobs), e.settings.clone(), Vec::new(), false, false)
            .await
            .unwrap();
        let done = wait_for_finish(&jobs, rx).await;
        assert_eq!(done.status, Some(RunStatus::Succeeded), "{:?}", done.message);

        let tar = e.root.join("archive/Front Door").join(format!("{old}.tar"));
        assert!(tar.is_file(), "archive written");

        // The source days for the archived month are gone; the live month is not.
        assert!(!e.root.join("backup/Front Door").join(format!("{old}-01")).exists());
        let current = plan::cutoff_month(now(), 0);
        assert!(e.root.join("backup/Front Door").join(format!("{current}-01")).exists());

        let entry = sqlx::query("SELECT verify_ok, file_count FROM archives WHERE month = ?")
            .bind(&old)
            .fetch_one(&e.pool)
            .await
            .unwrap();
        assert_eq!(entry.get::<i64, _>("verify_ok"), 1);
        assert_eq!(entry.get::<i64, _>("file_count"), 6);

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn keeping_sources_leaves_the_originals_in_place() {
        let e = env("keep").await;
        let mut settings = e.settings.clone();
        settings.keep_sources_after_archive = true;
        let jobs = Jobs::default();
        let rx = jobs.progress.subscribe();
        let old = plan::cutoff_month(now(), 3);

        run_archive(ctx(&e, &jobs), settings, Vec::new(), false, false).await.unwrap();
        wait_for_finish(&jobs, rx).await;

        assert!(e.root.join("archive/Front Door").join(format!("{old}.tar")).is_file());
        assert!(
            e.root.join("backup/Front Door").join(format!("{old}-01")).exists(),
            "sources must survive when the setting says to keep them"
        );

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn restoring_brings_a_month_back_and_pins_it() {
        let e = env("restore").await;
        let jobs = Jobs::default();
        let old = plan::cutoff_month(now(), 3);

        let rx = jobs.progress.subscribe();
        run_archive(ctx(&e, &jobs), e.settings.clone(), Vec::new(), false, false)
            .await
            .unwrap();
        wait_for_finish(&jobs, rx).await;
        assert!(!e.root.join("backup/Front Door").join(format!("{old}-01")).exists());

        let rx = jobs.progress.subscribe();
        run_restore(
            ctx(&e, &jobs),
            CameraMonth { camera: "Front Door".into(), month: old.clone() },
        )
        .await
        .unwrap();
        let done = wait_for_finish(&jobs, rx).await;
        assert_eq!(done.status, Some(RunStatus::Succeeded), "{:?}", done.message);

        let restored = e.root.join("backup/Front Door").join(format!("{old}-01/clip0.mp4"));
        assert!(restored.is_file());
        assert_eq!(
            std::fs::read(&restored).unwrap(),
            format!("{old}010").repeat(50).into_bytes()
        );

        let overview =
            overview(&e.pool, &e.settings, &e.root.join("backup"), &e.root.join("archive"))
                .await
                .unwrap();
        assert!(overview.archives.iter().any(|a| a.month == old && a.pinned));
        let due = overview.due.iter().find(|d| d.month == old).unwrap();
        assert!(due.blocked.as_deref().unwrap().contains("pinned"));

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn an_existing_archive_is_never_overwritten() {
        let e = env("existing").await;
        let jobs = Jobs::default();
        let old = plan::cutoff_month(now(), 3);

        // A tar already there, as if a previous tool wrote it.
        let dest = e.root.join("archive/Front Door").join(format!("{old}.tar"));
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"not really a tar").unwrap();

        let rx = jobs.progress.subscribe();
        run_archive(
            ctx(&e, &jobs),
            e.settings.clone(),
            vec![CameraMonth { camera: "Front Door".into(), month: old.clone() }],
            false,
            false,
        )
        .await
        .unwrap();
        let done = wait_for_finish(&jobs, rx).await;

        assert!(done.message.unwrap().contains("skipped"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"not really a tar");
        // Crucially, the sources were not deleted on the strength of an
        // archive we did not write and have not verified.
        assert!(e.root.join("backup/Front Door").join(format!("{old}-01")).exists());

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn a_dry_run_previews_the_oldest_month_when_nothing_is_due() {
        // Most of the time nothing is due, and a dry run that did nothing then
        // would be useless exactly when you want to test the mechanism.
        let e = env("preview").await;
        let mut settings = e.settings.clone();
        settings.archive_after_days = 3650; // nothing can possibly be due

        let jobs = Jobs::default();
        let rx = jobs.progress.subscribe();
        run_archive(ctx(&e, &jobs), settings, Vec::new(), true, false).await.unwrap();
        let done = wait_for_finish(&jobs, rx).await;

        assert_eq!(done.status, Some(RunStatus::Succeeded));
        let message = done.message.unwrap();
        assert!(message.contains("previewed the oldest month"), "{message}");
        assert!(is_empty(&e.root.join("archive")));
        let old = plan::cutoff_month(now(), 3);
        assert!(e.root.join("backup/Front Door").join(format!("{old}-01")).exists());

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn a_real_run_with_nothing_due_does_not_invent_work() {
        // The fallback is a dry-run affordance only. A real run must never
        // archive a month that has not reached the age threshold.
        let e = env("nodue").await;
        let mut settings = e.settings.clone();
        settings.archive_after_days = 3650;

        let jobs = Jobs::default();
        let result = run_archive(ctx(&e, &jobs), settings, Vec::new(), false, false).await;
        assert!(result.is_err(), "a real run must refuse rather than pick a month itself");

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn a_run_refuses_up_front_when_the_archive_directory_cannot_be_written() {
        // Root writes through any mode, so there is nothing to test as root.
        if unsafe { crate::health::geteuid_for_tests() } == 0 {
            return;
        }

        let e = env("readonly").await;
        let archive = e.root.join("archive");
        let mut perms = std::fs::metadata(&archive).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        std::fs::set_permissions(&archive, perms).unwrap();

        let jobs = Jobs::default();
        let err = run_archive(ctx(&e, &jobs), e.settings.clone(), Vec::new(), false, false)
            .await
            .expect_err("must refuse rather than start a run it cannot finish");

        // The message has to name the fix, not just the errno — the whole
        // point of checking here rather than failing inside `pack`.
        let message = err.to_string();
        assert!(message.contains("not writable"), "{message}");
        assert!(message.contains("group_add"), "{message}");

        // Nothing was recorded: a refusal is not a failed run.
        let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM archive_runs")
            .fetch_one(&e.pool)
            .await
            .unwrap();
        assert_eq!(runs, 0, "a refusal must not leave a run in the history");

        let mut perms = std::fs::metadata(&archive).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        std::fs::set_permissions(&archive, perms).unwrap();
        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn a_job_announces_itself_before_the_slow_part() {
        // Reading a large archive's index takes long enough that silence looks
        // like a hang; the first message must arrive immediately.
        let e = env("announce").await;
        let jobs = Jobs::default();
        let mut rx = jobs.progress.subscribe();

        run_archive(ctx(&e, &jobs), e.settings.clone(), Vec::new(), true, false)
            .await
            .unwrap();

        let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("no progress arrived")
            .unwrap();
        assert_eq!(first.phase, "preparing");
        assert!(!first.finished);

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn a_month_still_being_written_to_is_held_back() {
        // The backup service and this app share the clip directory. If the
        // other container is still writing into a month, archiving it would
        // capture a clip mid-write and then delete the original.
        let e = env("fresh").await;
        let old = plan::cutoff_month(now(), 3);

        // Touch one clip, as the backup service would when backfilling.
        let touched = e
            .root
            .join("backup/Front Door")
            .join(format!("{old}-01"))
            .join("clip0.mp4");
        backdate(&touched, 60.0);

        let overview =
            overview(&e.pool, &e.settings, &e.root.join("backup"), &e.root.join("archive"))
                .await
                .unwrap();
        let due = overview.due.iter().find(|d| d.month == old).unwrap();
        assert!(
            due.blocked.as_deref().unwrap_or("").contains("mid-write"),
            "expected a hold, got {:?}",
            due.blocked
        );

        // And "archive everything due" must not quietly include it.
        let jobs = Jobs::default();
        let result =
            run_archive(ctx(&e, &jobs), e.settings.clone(), Vec::new(), false, false).await;
        assert!(result.is_err(), "a blocked month must not be archived by an 'all' run");

        std::fs::remove_dir_all(&e.root).unwrap();
    }

    #[tokio::test]
    async fn only_one_job_runs_at_a_time() {
        let e = env("lock").await;
        let jobs = Jobs::default();
        let held = jobs.lock.clone().lock_owned().await;

        let result =
            run_archive(ctx(&e, &jobs), e.settings.clone(), Vec::new(), false, false).await;
        assert!(result.is_err(), "a second job must be refused, not queued");
        drop(held);

        std::fs::remove_dir_all(&e.root).unwrap();
    }
}

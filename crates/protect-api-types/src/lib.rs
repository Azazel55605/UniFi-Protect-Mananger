//! The API contract between the Rust service and the React frontend.
//!
//! Every type here derives `TS`, and `cargo test -p protect-api-types` writes
//! the TypeScript equivalents to `web/src/lib/types.gen.ts`. That file is
//! generated, never hand-edited: a change to a response shape that breaks the
//! frontend then breaks `tsc`, instead of surfacing as `undefined` at runtime.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

const OUT: &str = "../../../web/src/lib/types.gen.ts";

// Note: `i64`/`usize` fields carry `#[ts(type = "number")]`. ts-rs would
// otherwise generate `bigint`, but `serde_json` writes them as ordinary JSON
// numbers — the annotation keeps the generated type honest about the wire.

// ---------------------------------------------------------------- auth

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct AuthStatus {
    pub authenticated: bool,
    /// False when no password is configured — the UI explains the fix rather
    /// than showing a login form that cannot succeed.
    pub configured: bool,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct LoginRequest {
    pub password: String,
}

// -------------------------------------------------------------- health

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct Check {
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct Health {
    pub ok: bool,
    pub docker: Check,
    pub container: Check,
    pub backup_dir: Check,
    /// Things that are wrong and need attention.
    pub warnings: Vec<String>,
    /// Things worth stating that are not, by themselves, problems. Kept
    /// separate so the warning list stays meaningful when it is non-empty.
    pub info: Vec<String>,
}

// -------------------------------------------------------------- docker

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct ContainerRef {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct MountInfo {
    pub source: Option<String>,
    pub destination: String,
    pub rw: bool,
}

/// Configuration proposed from the backup container itself, so setup is a
/// confirmation rather than a typing exercise.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct ProposedConfig {
    pub sqlite_container_path: Option<String>,
    pub sqlite_host_path: Option<String>,
    pub backup_host_dir: Option<String>,
    /// Prefix to strip from `backups.path` when resolving a clip.
    pub clip_path_prefix: Option<String>,
    /// Where the database lands inside *our* mount — the only form we can open.
    pub events_db_local_path: Option<String>,
    /// Why any of the above is missing, in words the user can act on.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct UpbInspection {
    pub container: ContainerRef,
    pub running: bool,
    pub started_at: Option<String>,
    #[ts(type = "number")]
    pub restart_count: i64,
    /// Docker health is unavailable when the container disables its
    /// healthcheck, which the backup service's image does by default.
    pub health_available: bool,
    pub mounts: Vec<MountInfo>,
    /// Only an allowlist is read; the container's environment holds NVR
    /// credentials in plaintext.
    pub env: Vec<(String, String)>,
    #[ts(type = "number")]
    pub env_withheld: usize,
    pub command: Option<String>,
    pub retention: Option<String>,
    pub missing_range: Option<String>,
    pub proposed: ProposedConfig,
}

// --------------------------------------------------------------- setup

/// A directory that looks like it holds a camera's clips.
///
/// Candidates are identified by evidence — date-shaped subdirectories and
/// video files — rather than by excluding a hardcoded list of known
/// non-camera directory names, which would only ever match one user's layout.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct CameraCandidate {
    pub dir_name: String,
    /// Subdirectories shaped like `YYYY-MM-DD`.
    pub date_dirs: usize,
    pub clip_count: usize,
    /// Whether we would pre-select this one.
    pub looks_like_camera: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct Settings {
    pub upb_container_id: Option<String>,
    /// `events.sqlite` as *this* container can open it.
    pub events_db_path: Option<String>,
    /// Prefix to strip from `backups.path`.
    pub clip_path_prefix: Option<String>,
    /// Camera directories the user confirmed.
    pub camera_dirs: Vec<String>,
    /// How long clips stay viewable. The same number decides when they are
    /// archived — one knob, phrased the way the user thinks about it.
    #[ts(type = "number")]
    pub live_window_months: u32,
    /// Keep the originals after an archive verifies clean.
    ///
    /// Defaults to false — the sources are removed — because the backup
    /// service never deletes anything itself, so nothing else on the system
    /// bounds disk growth. Turning this on means the live directory grows
    /// forever, which is a legitimate choice but has to be a deliberate one.
    #[serde(default)]
    pub keep_sources_after_archive: bool,
    pub setup_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct SetupState {
    pub settings: Settings,
    /// Whether enough is configured for the app to do anything useful.
    pub complete: bool,
    /// Validation of the current settings, re-run on each request so a path
    /// that disappears is noticed rather than remembered as good.
    pub checks: Vec<NamedCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct NamedCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct DiscoveryResult {
    pub containers: Vec<ContainerRef>,
    pub inspection: Option<UpbInspection>,
    pub cameras: Vec<CameraCandidate>,
    pub notes: Vec<String>,
}

// --------------------------------------------------------------- events

/// Where a clip's bytes are, which is not the same as whether an event exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub enum ClipStatus {
    /// The file is on disk and playable.
    Live,
    /// Backed up once, but no longer in the live directory — it has aged out
    /// and been archived. The archive that holds it is named once archiving
    /// is implemented.
    Archived,
    /// Backed up, still inside the live window, but the file is not there.
    /// Something removed it out of band; worth surfacing rather than hiding.
    Vanished,
    /// The event was recorded but never backed up, and still could be — the
    /// backup service backfills gaps within a window.
    PendingBackfill,
    /// Never backed up and now outside that window. The footage is gone.
    NeverBackedUp,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct EventRecord {
    pub id: String,
    pub camera_id: String,
    /// Resolved display name, falling back to the raw id when unknown.
    pub camera: String,
    /// The backup service's own event type, e.g. `smartDetectZone`.
    pub event_type: String,
    /// Detection types, which live in the clip's filename rather than the
    /// database, and can be multiple for one event.
    pub subtypes: Vec<String>,
    /// Unix seconds.
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub status: ClipStatus,
    /// Path as this container would open it. Absent when never backed up.
    pub clip_path: Option<String>,
    #[ts(type = "number")]
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct EventPage {
    pub events: Vec<EventRecord>,
    /// Total matching the filter, for pagination.
    #[ts(type = "number")]
    pub total: i64,
    #[ts(type = "number")]
    pub offset: i64,
    #[ts(type = "number")]
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct CameraInfo {
    pub camera_id: String,
    /// Name derived from the clip path.
    pub derived_name: Option<String>,
    /// What to show. Falls back to the derived name, then the raw id.
    pub display_name: String,
    #[ts(type = "number")]
    pub event_count: i64,
    /// Unix seconds of the most recent event, if any.
    pub last_event: Option<f64>,
}

/// Health of the index itself, and of the backup pipeline feeding it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct IndexStats {
    #[ts(type = "number")]
    pub total_events: i64,
    #[ts(type = "number")]
    pub live_clips: i64,
    #[ts(type = "number")]
    pub archived: i64,
    #[ts(type = "number")]
    pub vanished: i64,
    #[ts(type = "number")]
    pub pending_backfill: i64,
    #[ts(type = "number")]
    pub never_backed_up: i64,
    /// Seconds since the newest event that has a clip — the single best
    /// "is the pipeline working" signal, and free to compute.
    pub backup_lag_secs: Option<f64>,
    pub newest_event: Option<f64>,
    pub oldest_event: Option<f64>,
    /// When the index last synced, and whether that attempt worked.
    pub last_sync: Option<f64>,
    pub last_sync_error: Option<String>,
    pub distinct_subtypes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct EventQuery {
    #[ts(optional)]
    pub camera_id: Option<String>,
    #[ts(optional)]
    pub event_type: Option<String>,
    #[ts(optional)]
    pub subtype: Option<String>,
    #[ts(optional)]
    pub status: Option<ClipStatus>,
    #[ts(optional)]
    pub from: Option<f64>,
    #[ts(optional)]
    pub to: Option<f64>,
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub limit: Option<i64>,
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub offset: Option<i64>,
}

// -------------------------------------------------------------- archive

/// A camera-month: the unit everything here operates on, because archives are
/// one tar per camera per calendar month.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct CameraMonth {
    pub camera: String,
    /// `YYYY-MM`.
    pub month: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub enum RunKind {
    Archive,
    Restore,
    Verify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    /// The process stopped mid-run. Detected at startup, never left as
    /// "running" — a row that claims to be in progress forever is a lie.
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct ArchiveRun {
    #[ts(type = "number")]
    pub id: i64,
    pub kind: RunKind,
    pub status: RunStatus,
    /// Absent for a run that covered several camera-months.
    pub camera: Option<String>,
    pub month: Option<String>,
    pub started: f64,
    pub finished: Option<f64>,
    /// True when nothing was written or deleted.
    pub dry_run: bool,
    /// Whether a schedule started this, rather than a person.
    pub scheduled: bool,
    #[ts(type = "number")]
    pub files_total: i64,
    #[ts(type = "number")]
    pub files_done: i64,
    #[ts(type = "number")]
    pub bytes_total: i64,
    pub message: Option<String>,
    /// Files whose content did not survive the round trip. Empty on success,
    /// and the reason sources are still on disk when it isn't.
    pub failed_files: Vec<String>,
}

/// An archive on disk, as far as we know it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct ArchiveEntry {
    pub camera: String,
    pub month: String,
    pub path: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub file_count: i64,
    pub created: Option<f64>,
    pub verified_at: Option<f64>,
    pub verify_ok: Option<bool>,
    /// Restored back to live, so the scheduler must leave it alone until
    /// released — otherwise the next run would immediately re-archive it.
    pub pinned: bool,
    /// On disk but with no run history: made by hand, or by whatever managed
    /// archiving before this app did.
    pub unrecorded: bool,
}

/// A camera-month old enough to archive that hasn't been.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct DueEntry {
    pub camera: String,
    pub month: String,
    #[ts(type = "number")]
    pub file_count: i64,
    #[ts(type = "number")]
    pub bytes: i64,
    /// Why it hasn't been archived, when the answer isn't "not yet".
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct ArchiveOverview {
    pub archives: Vec<ArchiveEntry>,
    pub due: Vec<DueEntry>,
    /// Runs whose archive is no longer on disk.
    pub missing_archives: Vec<CameraMonth>,
    #[ts(type = "number")]
    pub total_bytes: i64,
    pub running: Option<ArchiveRun>,
}

/// Live progress for a running job.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct RunProgress {
    #[ts(type = "number")]
    pub run_id: i64,
    pub kind: RunKind,
    /// What is being worked on right now.
    pub camera: Option<String>,
    pub month: Option<String>,
    pub phase: String,
    pub current_file: Option<String>,
    /// Progress through the current camera-month.
    #[ts(type = "number")]
    pub files_done: i64,
    #[ts(type = "number")]
    pub files_total: i64,
    /// Progress across the whole run, which may span several camera-months.
    #[ts(type = "number")]
    pub overall_done: i64,
    #[ts(type = "number")]
    pub overall_total: i64,
    pub finished: bool,
    pub status: Option<RunStatus>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct StartArchiveRequest {
    /// Empty means everything currently due.
    #[serde(default)]
    pub targets: Vec<CameraMonth>,
    /// Report what would happen, writing and deleting nothing.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub enum ScheduleKind {
    Off,
    /// On a given day of the month.
    Monthly,
    /// Every day. Cheap when nothing is due, and keeps a missed month from
    /// waiting another four weeks.
    Daily,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct Schedule {
    pub kind: ScheduleKind,
    /// Day of month for `Monthly`, ignored otherwise.
    #[ts(type = "number")]
    pub day: u32,
    /// Local hour, 0–23.
    #[ts(type = "number")]
    pub hour: u32,
    /// Run as soon as the app starts if the scheduled time was missed while it
    /// was down. A late archive beats a skipped one.
    pub catch_up: bool,
    /// Optional POST on failure, so a silent failure can't hide behind a
    /// closed browser tab.
    pub webhook_url: Option<String>,
    pub next_run: Option<f64>,
    pub last_run: Option<f64>,
}

// -------------------------------------------------------------- storage

/// Usage of one filesystem, as the kernel reports it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct FilesystemUsage {
    /// The path we asked about, as mounted in this container.
    pub path: String,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub free_bytes: i64,
    /// Device id, so two paths on the same filesystem can be recognised as one
    /// — otherwise their free space looks like twice what exists.
    #[ts(type = "number")]
    pub device: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct CameraUsage {
    pub camera: String,
    #[ts(type = "number")]
    pub live_bytes: i64,
    #[ts(type = "number")]
    pub archive_bytes: i64,
    #[ts(type = "number")]
    pub live_clips: i64,
    #[ts(type = "number")]
    pub archived_months: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct StorageSnapshot {
    pub backup: Option<FilesystemUsage>,
    pub archive: Option<FilesystemUsage>,
    /// True when both directories live on the same filesystem, which changes
    /// what "free space" means for archiving: packing then deleting frees
    /// nothing until the sources go.
    pub same_filesystem: bool,
    #[ts(type = "number")]
    pub live_bytes: i64,
    #[ts(type = "number")]
    pub archive_bytes: i64,
    pub cameras: Vec<CameraUsage>,
    /// Bytes per day, measured over the sampled history. `None` until there
    /// is enough history to say anything honest.
    pub growth_bytes_per_day: Option<f64>,
    /// Days until the fuller of the two filesystems runs out at that rate.
    pub days_until_full: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct StorageSample {
    pub at: f64,
    #[ts(type = "number")]
    pub live_bytes: i64,
    #[ts(type = "number")]
    pub archive_bytes: i64,
    #[ts(type = "number")]
    pub free_bytes: i64,
}

/// What playing a clip will involve, so the UI can be honest about a wait.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = OUT)]
pub struct ClipInfo {
    pub id: String,
    pub available: bool,
    /// Why it isn't, when it isn't.
    pub reason: Option<String>,
    pub codec: Option<String>,
    /// True when the browser can play the recording as-is.
    pub direct: bool,
    /// True when a transcode already exists, so playback starts immediately.
    pub prepared: bool,
}

#[cfg(test)]
mod tests {
    /// `cargo test -p protect-api-types` regenerates the TypeScript bindings;
    /// ts-rs writes them as a side effect of its own export tests.
    #[test]
    fn bindings_are_generated() {
        // The derive macro's generated tests do the writing. This exists so a
        // bare `cargo test` in CI fails loudly if the crate stops compiling.
    }
}

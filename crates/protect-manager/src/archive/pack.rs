//! Writing an archive, and proving it before anything is deleted.
//!
//! The routine this replaces verified with `tar -tf`, which walks the headers
//! and catches truncation — but never compares content against the source.
//! A structurally valid tar holding wrong bytes passed, and the originals were
//! deleted straight afterwards. Checksumming filesystems make that unlikely,
//! but the consequence is permanent loss of footage, and the fix is cheap:
//! hash every file on the way in, read the archive back, and compare.
//!
//! Nothing here deletes anything. Deletion is the caller's decision, and only
//! after `verify` has returned a clean result.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};


/// Reported per file so a caller can drive a progress bar without knowing how
/// archiving works.
pub struct FileProgress<'a> {
    pub index: usize,
    pub total: usize,
    pub name: &'a str,
}

fn hash_reader<R: Read>(mut r: R, sink: Option<&mut dyn Write>) -> std::io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    let mut sink = sink;

    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        if let Some(w) = sink.as_deref_mut() {
            w.write_all(&buf[..n])?;
        }
        total += n as u64;
    }
    let digest = hasher.finalize();
    let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok((hex, total))
}

/// Where a camera-month's archive is assembled before it is complete.
///
/// A separate name so an interrupted run never leaves something that looks
/// like a finished archive — and, now that runs resume, so the work in
/// progress is findable again rather than being anonymous rubble.
pub fn partial_path(dest: &Path) -> PathBuf {
    dest.with_extension("tar.partial")
}

/// What one appended group of files added to a partial archive.
pub struct DayPack {
    /// Byte offset of the end of the last entry, *before* the tar trailer.
    ///
    /// This is the resume point, and it is a position we wrote rather than one
    /// we searched for: appending later means truncating back to exactly here,
    /// which is by construction a record boundary.
    pub end_offset: u64,
    pub hashes: BTreeMap<String, String>,
}

/// Append files to a partial archive, hashing each as it goes.
///
/// Uncompressed, matching the existing archives — and worth keeping that way:
/// clips are already compressed video, so gzip would burn CPU for nothing.
///
/// Everything past `start_offset` is discarded first. On a fresh start that is
/// a no-op; on a resume it removes both the previous trailer and any partial
/// record from the write that was interrupted. Either way the archive is
/// extended from a known-good boundary.
///
/// The trailer is rewritten after every call, so the partial is a *valid tar*
/// at every checkpoint rather than only at the end — an interrupted run leaves
/// a readable archive of the days that made it, not a torn file.
pub fn append_day(
    partial: &Path,
    start_offset: u64,
    files: &[(PathBuf, String)],
    mut on_file: impl FnMut(FileProgress<'_>),
) -> anyhow::Result<DayPack> {
    if let Some(parent) = partial.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(partial)?;
    file.set_len(start_offset)?;
    file.seek(SeekFrom::Start(start_offset))?;

    let mut builder = tar::Builder::new(file);
    let mut hashes = BTreeMap::new();

    let total = files.len();
    for (i, (source, entry_name)) in files.iter().enumerate() {
        on_file(FileProgress { index: i, total, name: entry_name });

        let mut header = tar::Header::new_gnu();
        let meta = std::fs::metadata(source)?;
        header.set_metadata(&meta);
        header.set_size(meta.len());
        header.set_cksum();

        let mut src = File::open(source)?;
        // Hash and write in one pass: reading these files twice would double
        // the I/O on the slowest part of the job.
        let mut buf = Vec::with_capacity(meta.len() as usize);
        let (hash, _) = hash_reader(&mut src, Some(&mut buf))?;
        builder.append_data(&mut header, entry_name, &buf[..])?;
        hashes.insert(entry_name.clone(), hash);
    }

    // Read the position before `into_inner` writes the trailer, so the
    // recorded offset is where the next append must resume from.
    let end_offset = builder.get_mut().stream_position()?;

    let mut file = builder.into_inner()?;
    file.flush()?;
    // The point of verification is that the bytes reached the disk, so make
    // sure they have before we read them back — and before a checkpoint claims
    // this day is safely stored.
    file.sync_all()?;
    drop(file);

    Ok(DayPack { end_offset, hashes })
}

/// Promote a finished partial to the archive it was building.
pub fn finish(partial: &Path, dest: &Path) -> anyhow::Result<u64> {
    let bytes = std::fs::metadata(partial)?.len();
    std::fs::rename(partial, dest)?;
    Ok(bytes)
}

pub struct VerifyResult {
    pub checked: usize,
    /// Entry names whose content did not match. Empty means the archive is a
    /// faithful copy and the sources are safe to remove.
    pub mismatched: Vec<String>,
    pub missing: Vec<String>,
    pub extra: Vec<String>,
}

impl VerifyResult {
    pub fn ok(&self) -> bool {
        self.mismatched.is_empty() && self.missing.is_empty() && self.extra.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.ok() {
            return format!("{} files verified", self.checked);
        }
        let mut parts = Vec::new();
        if !self.mismatched.is_empty() {
            parts.push(format!("{} corrupt", self.mismatched.len()));
        }
        if !self.missing.is_empty() {
            parts.push(format!("{} missing from archive", self.missing.len()));
        }
        if !self.extra.is_empty() {
            parts.push(format!("{} unexpected in archive", self.extra.len()));
        }
        parts.join(", ")
    }

    /// Everything that went wrong, for the run record.
    pub fn failed_files(&self) -> Vec<String> {
        self.mismatched
            .iter()
            .chain(self.missing.iter())
            .chain(self.extra.iter())
            .cloned()
            .collect()
    }
}

/// Read an archive back and compare every file against the expected hashes.
pub fn verify(
    archive: &Path,
    expected: &BTreeMap<String, String>,
    mut on_file: impl FnMut(FileProgress<'_>),
) -> anyhow::Result<VerifyResult> {
    let file = File::open(archive)?;
    let mut tar = tar::Archive::new(file);

    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let total = expected.len();

    for entry in tar.entries()? {
        let entry = entry?;
        let name = entry.path()?.to_string_lossy().to_string();
        on_file(FileProgress { index: seen.len(), total, name: &name });
        let (hash, _) = hash_reader(entry, None)?;
        seen.insert(name, hash);
    }

    let mut mismatched = Vec::new();
    let mut missing = Vec::new();
    for (name, want) in expected {
        match seen.get(name) {
            Some(got) if got == want => {}
            Some(_) => mismatched.push(name.clone()),
            None => missing.push(name.clone()),
        }
    }
    let extra: Vec<String> = seen
        .keys()
        .filter(|k| !expected.contains_key(*k))
        .cloned()
        .collect();

    Ok(VerifyResult { checked: seen.len(), mismatched, missing, extra })
}

/// List an archive's entries without reading their content.
///
/// A header walk, so "what's in this archive" stays cheap even for a tar of
/// many gigabytes.
pub fn list(archive: &Path) -> anyhow::Result<Vec<(String, u64)>> {
    let file = File::open(archive)?;
    let mut tar = tar::Archive::new(file);
    let mut out = Vec::new();
    for entry in tar.entries()? {
        let entry = entry?;
        out.push((entry.path()?.to_string_lossy().to_string(), entry.size()));
    }
    Ok(out)
}

/// Unpack an archive back into a camera directory.
pub fn unpack(
    archive: &Path,
    dest: &Path,
    mut on_file: impl FnMut(FileProgress<'_>),
) -> anyhow::Result<usize> {
    let total = list(archive)?.len();
    let file = File::open(archive)?;
    let mut tar = tar::Archive::new(file);
    let mut count = 0usize;

    std::fs::create_dir_all(dest)?;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().to_string();

        // Never write outside the destination, whatever the archive claims.
        // Our own archives are safe by construction; one edited by hand or
        // produced by another tool is not, and this is the classic tar bug.
        if name.contains("..") || name.starts_with('/') {
            anyhow::bail!("archive contains an unsafe path: {name}");
        }

        on_file(FileProgress { index: count, total, name: &name });
        entry.unpack_in(dest)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::plan::{months_for_camera, MonthContents};

    /// Pack a whole month the way a run does — day by day, then promote.
    fn pack_all(
        month: &MonthContents,
        dest: &Path,
        mut on_file: impl FnMut(FileProgress<'_>),
    ) -> anyhow::Result<BTreeMap<String, String>> {
        let partial = partial_path(dest);
        let mut offset = 0u64;
        let mut hashes = BTreeMap::new();

        let mut by_day: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
        for (path, entry) in &month.files {
            let day = entry.split('/').next().unwrap_or("").to_string();
            by_day.entry(day).or_default().push((path.clone(), entry.clone()));
        }

        for (_, files) in by_day {
            let packed = append_day(&partial, offset, &files, &mut on_file)?;
            offset = packed.end_offset;
            hashes.extend(packed.hashes);
        }
        finish(&partial, dest)?;
        Ok(hashes)
    }

    fn scaffold(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("pm-pack-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (day, n) in [("2026-06-01", 2), ("2026-06-02", 2)] {
            let d = root.join("backup").join("Front Door").join(day);
            std::fs::create_dir_all(&d).unwrap();
            for i in 0..n {
                std::fs::write(d.join(format!("clip{i}.mp4")), format!("{day}-{i}").repeat(400))
                    .unwrap();
            }
        }
        root
    }

    #[test]
    fn packs_and_verifies_a_month() {
        let root = scaffold("roundtrip");
        let month = months_for_camera(&root.join("backup"), "Front Door").remove(0);
        let dest = root.join("archive").join("Front Door").join("2026-06.tar");

        let mut seen = 0;
        let hashes = pack_all(&month, &dest, |_| seen += 1).unwrap();
        assert_eq!(seen, 4);
        assert_eq!(hashes.len(), 4);
        assert!(dest.is_file());
        // The temporary file must not survive a successful run.
        assert!(!dest.with_extension("tar.partial").exists());

        let result = verify(&dest, &hashes, |_| {}).unwrap();
        assert!(result.ok(), "{}", result.summary());
        assert_eq!(result.checked, 4);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn appending_discards_a_torn_tail_and_still_produces_a_valid_archive() {
        // What a killed process leaves: bytes past the last checkpoint that are
        // half a record, plus the trailer written at the previous checkpoint.
        // Resuming truncates back to the recorded offset — a boundary we wrote,
        // never one we went looking for.
        let root = scaffold("torn");
        let month = months_for_camera(&root.join("backup"), "Front Door").remove(0);
        let dest = root.join("archive").join("Front Door").join("2026-06.tar");
        let partial = partial_path(&dest);

        let day1: Vec<_> = month
            .files
            .iter()
            .filter(|(_, e)| e.starts_with("2026-06-01"))
            .cloned()
            .collect();
        let day2: Vec<_> = month
            .files
            .iter()
            .filter(|(_, e)| e.starts_with("2026-06-02"))
            .cloned()
            .collect();

        let first = append_day(&partial, 0, &day1, |_| {}).unwrap();
        // At a checkpoint the partial is already a valid tar of what it holds.
        assert_eq!(list(&partial).unwrap().len(), 2);

        // Now scribble a half-written record onto the end.
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new().append(true).open(&partial).unwrap();
            f.write_all(&[0xAB; 900]).unwrap();
        }

        let second = append_day(&partial, first.end_offset, &day2, |_| {}).unwrap();
        finish(&partial, &dest).unwrap();

        let mut hashes = first.hashes.clone();
        hashes.extend(second.hashes.clone());
        assert_eq!(hashes.len(), 4);

        let entries = list(&dest).unwrap();
        assert_eq!(entries.len(), 4, "no torn record survived: {entries:?}");
        let result = verify(&dest, &hashes, |_| {}).unwrap();
        assert!(result.ok(), "{}", result.summary());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn verification_catches_content_that_does_not_match() {
        // The failure the previous `tar -tf` check could not see: a structurally
        // valid archive whose bytes are wrong. This is what stands between a
        // corrupt archive and deleting the only other copy.
        let root = scaffold("corrupt");
        let month = months_for_camera(&root.join("backup"), "Front Door").remove(0);
        let dest = root.join("archive").join("2026-06.tar");
        let hashes = pack_all(&month, &dest, |_| {}).unwrap();

        let mut wrong = hashes.clone();
        let key = wrong.keys().next().unwrap().clone();
        wrong.insert(key.clone(), "0".repeat(64));

        let result = verify(&dest, &wrong, |_| {}).unwrap();
        assert!(!result.ok());
        assert_eq!(result.mismatched, vec![key]);
        assert!(result.summary().contains("corrupt"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn verification_notices_a_file_absent_from_the_archive() {
        let root = scaffold("missing");
        let month = months_for_camera(&root.join("backup"), "Front Door").remove(0);
        let dest = root.join("archive").join("2026-06.tar");
        let mut hashes = pack_all(&month, &dest, |_| {}).unwrap();

        hashes.insert("2026-06-03/never-written.mp4".into(), "abc".into());
        let result = verify(&dest, &hashes, |_| {}).unwrap();
        assert!(!result.ok());
        assert_eq!(result.missing, vec!["2026-06-03/never-written.mp4"]);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn round_trips_through_unpack() {
        let root = scaffold("unpack");
        let month = months_for_camera(&root.join("backup"), "Front Door").remove(0);
        let dest = root.join("archive").join("2026-06.tar");
        pack_all(&month, &dest, |_| {}).unwrap();

        let restored = root.join("restored");
        let count = unpack(&dest, &restored, |_| {}).unwrap();
        assert_eq!(count, 4);

        let original = std::fs::read(root.join("backup/Front Door/2026-06-01/clip0.mp4")).unwrap();
        let back = std::fs::read(restored.join("2026-06-01/clip0.mp4")).unwrap();
        assert_eq!(original, back, "restored content must be byte-identical");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn listing_an_archive_does_not_read_its_content() {
        let root = scaffold("list");
        let month = months_for_camera(&root.join("backup"), "Front Door").remove(0);
        let dest = root.join("archive").join("2026-06.tar");
        pack_all(&month, &dest, |_| {}).unwrap();

        let entries = list(&dest).unwrap();
        assert_eq!(entries.len(), 4);
        assert!(entries.iter().all(|(_, size)| *size > 0));

        std::fs::remove_dir_all(&root).unwrap();
    }
}

//! Health: can this container actually see everything it needs?
//!
//! These checks answer four questions about the real deployment — can we reach
//! the Docker socket, can we find the backup container without knowing its
//! name, can we read the clip directory, and can we write to the archive
//! directory. A permission problem is obvious here; the same problem
//! discovered during an archive run shows up as a failure halfway through.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use protect_api_types::Check;

fn ok(detail: impl Into<String>) -> Check {
    Check { ok: true, detail: detail.into() }
}

fn fail(detail: impl Into<String>) -> Check {
    Check { ok: false, detail: detail.into() }
}

/// Verify we can read the backup root, and say something useful when we can't.
///
/// The backup service writes clips as its own uid/gid, so the likely failure is
/// group membership rather than a wrong path. Reporting both sides — "cannot
/// read /backup, owned by 950:568, we run as 1000:1000" — is fixable; an empty
/// event list is not.
pub fn check_backup_dir(dir: &Path) -> Check {
    let meta = match std::fs::metadata(dir) {
        Ok(m) => m,
        Err(e) => return fail(format!("{}: {e}", dir.display())),
    };
    if !meta.is_dir() {
        return fail(format!("{} is not a directory", dir.display()));
    }

    let ids = format!(
        "owned by {}:{}, we run as {}:{}",
        meta.uid(),
        meta.gid(),
        unsafe { libc_geteuid() },
        unsafe { libc_getegid() },
    );

    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let subdirs: Vec<String> = entries
                .filter_map(Result::ok)
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            ok(format!(
                "{} readable, {} subdirectories ({})",
                dir.display(),
                subdirs.len(),
                ids
            ))
        }
        Err(e) => fail(format!(
            "{} exists but is not readable: {e} — {ids}",
            dir.display()
        )),
    }
}

/// Verify we can actually write into the archive root.
///
/// Checked by writing, not by reading the mode bits. The image runs as an
/// unprivileged uid with the clip gid added through `group_add`, so the
/// permissions that matter come from supplementary groups — and mode bits plus
/// an owner id cannot tell you whether *this* process is in the group, let
/// alone what an ACL or a read-only bind mount will do. The only honest test
/// is the one archiving itself performs.
pub fn check_archive_dir(dir: &Path) -> Check {
    let meta = match std::fs::metadata(dir) {
        Ok(m) => m,
        // Not an error to be smoothed over by creating it: the directory comes
        // from a bind mount, and if it is missing the mount is wrong. Creating
        // it here would put archives inside the container, where they are lost
        // on the next `docker compose up`.
        Err(e) => {
            return fail(format!(
                "{}: {e} — archiving cannot run. Check the volume is mounted.",
                dir.display()
            ))
        }
    };
    if !meta.is_dir() {
        return fail(format!("{} is not a directory", dir.display()));
    }

    let ids = format!(
        "owned by {}:{}, we run as {}:{}",
        meta.uid(),
        meta.gid(),
        unsafe { libc_geteuid() },
        unsafe { libc_getegid() },
    );

    if let Err(e) = probe_writable(dir) {
        return fail(unwritable(dir, &e, &ids));
    }

    // The root being writable is not enough, and assuming it was is what let a
    // permission failure through to a run. Archives land in
    // `<archive>/<camera>/`, and this app writes the same layout as the shell
    // script it replaces — so on any deployment that ran that script, those
    // per-camera directories already exist and are owned by whoever ran it.
    // A writable root beside an unwritable camera directory is the normal
    // shape of this failure.
    match unwritable_subdir(dir) {
        Err(detail) => fail(detail),
        Ok(checked) => ok(format!(
            "{} writable, {checked} camera directories checked ({ids})",
            dir.display(),
        )),
    }
}

/// Probe every existing camera directory, naming the first that refuses.
///
/// Returns how many were checked, so a green light says what it actually
/// verified rather than implying more than it looked at.
fn unwritable_subdir(dir: &Path) -> Result<usize, String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(0) };
    let mut checked = 0usize;

    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        checked += 1;
        let path = entry.path();
        if let Err(e) = probe_writable(&path) {
            let ids = match std::fs::metadata(&path) {
                Ok(m) => format!(
                    "owned by {}:{}, we run as {}:{}",
                    m.uid(),
                    m.gid(),
                    unsafe { libc_geteuid() },
                    unsafe { libc_getegid() },
                ),
                Err(_) => "owner unknown".into(),
            };
            return Err(unwritable(&path, &e, &ids));
        }
    }
    Ok(checked)
}

/// Can we create a file here? Asked by doing it.
///
/// A fixed probe name, removed straight away. A pid-suffixed one would leave
/// litter behind if the process were killed between create and remove, and
/// health runs often enough for that to accumulate.
pub fn probe_writable(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(".protect-manager-write-test");
    let _ = std::fs::remove_file(&probe);
    std::fs::write(&probe, b"")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// One sentence for an unwritable directory, naming the fix rather than only
/// the fault — the errno alone is what made this hard to act on.
pub fn unwritable(dir: &Path, e: &std::io::Error, ids: &str) -> String {
    format!(
        "{} is not writable: {e} — archiving will fail. Add the group owning it to \
         `group_add`, or chown it to {}:{}. ({ids})",
        dir.display(),
        unsafe { libc_geteuid() },
        unsafe { libc_getegid() },
    )
}

/// The effective uid, for tests that must skip when running as root.
#[cfg(test)]
pub unsafe fn geteuid_for_tests() -> u32 {
    unsafe { libc_geteuid() }
}

// Avoiding a dependency on `libc` for two calls.
unsafe fn libc_geteuid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    geteuid()
}

unsafe fn libc_getegid() -> u32 {
    extern "C" {
        fn getegid() -> u32;
    }
    getegid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unwritable_archive_directory_is_caught_before_a_run_is_started() {
        let root = std::env::temp_dir().join(format!("pm-health-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        assert!(check_archive_dir(&root).ok, "a normal directory is writable");
        // The probe must not survive the check that wrote it.
        assert!(!root.join(".protect-manager-write-test").exists());

        // A missing mount reads as a failure, and is never created for you.
        let missing = root.join("not-mounted");
        let check = check_archive_dir(&missing);
        assert!(!check.ok);
        assert!(!missing.exists(), "a missing archive root must not be created");

        // Root runs as uid 0 and can write through any mode, so the read-only
        // case is only meaningful unprivileged.
        if unsafe { libc_geteuid() } != 0 {
            let locked = root.join("locked");
            std::fs::create_dir_all(&locked).unwrap();
            lock(&locked);

            let check = check_archive_dir(&locked);
            assert!(!check.ok, "{}", check.detail);
            assert!(check.detail.contains("not writable"), "{}", check.detail);
            assert!(check.detail.contains("group_add"), "the fix, not just the fault");
            unlock(&locked);
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The failure that actually reached a user: the archive root is writable,
    /// so nothing complained, but the per-camera directory inside it was made
    /// by the shell script this app replaces and is owned by someone else.
    #[test]
    fn an_unwritable_camera_directory_fails_the_check_even_when_the_root_is_fine() {
        if unsafe { libc_geteuid() } == 0 {
            return;
        }

        let root = std::env::temp_dir().join(format!("pm-health-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let camera = root.join("G4 Instant Wäscheplatz");
        std::fs::create_dir_all(&camera).unwrap();

        assert!(check_archive_dir(&root).ok, "a writable tree passes");

        lock(&camera);
        let check = check_archive_dir(&root);
        assert!(!check.ok, "the root is writable, but the camera directory is not");
        // The message has to name the directory that refused, not the root we
        // happened to start from.
        assert!(check.detail.contains("G4 Instant Wäscheplatz"), "{}", check.detail);
        assert!(check.detail.contains("not writable"), "{}", check.detail);

        unlock(&camera);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn lock(dir: &Path) {
        let mut perms = std::fs::metadata(dir).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        std::fs::set_permissions(dir, perms).unwrap();
    }

    fn unlock(dir: &Path) {
        let mut perms = std::fs::metadata(dir).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        std::fs::set_permissions(dir, perms).unwrap();
    }
}

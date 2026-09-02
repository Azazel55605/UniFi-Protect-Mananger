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

    // A fixed name, removed straight away. A pid-suffixed one would leave
    // litter behind if the process were killed between create and remove, and
    // health runs often enough for that to accumulate.
    let probe = dir.join(".protect-manager-write-test");
    let _ = std::fs::remove_file(&probe);
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            ok(format!("{} writable ({ids})", dir.display()))
        }
        Err(e) => fail(format!(
            "{} is not writable: {e} — archiving will fail. Add the group owning it to \
             `group_add`, or chown it to {}:{}. ({ids})",
            dir.display(),
            unsafe { libc_geteuid() },
            unsafe { libc_getegid() },
        )),
    }
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
            let mut perms = std::fs::metadata(&locked).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
            std::fs::set_permissions(&locked, perms).unwrap();

            let check = check_archive_dir(&locked);
            assert!(!check.ok, "{}", check.detail);
            assert!(check.detail.contains("not writable"), "{}", check.detail);
            assert!(check.detail.contains("group_add"), "the fix, not just the fault");
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}

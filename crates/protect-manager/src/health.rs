//! Health: can this container actually see everything it needs?
//!
//! These checks answer three questions about the real deployment — can we
//! reach the Docker socket, can we find the backup container without knowing
//! its name, and can we read the clip directory. A permission problem is
//! obvious here; the same problem discovered during an archive run shows up as
//! a failure halfway through.

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

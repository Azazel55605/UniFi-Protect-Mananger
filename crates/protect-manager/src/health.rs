//! Health: can this container actually see everything it needs?
//!
//! These checks answer four questions about the real deployment — can we reach
//! the Docker socket, can we find the backup container without knowing its
//! name, can we read the clip directory, and can we write to the archive
//! directory. A permission problem is obvious here; the same problem
//! discovered during an archive run shows up as a failure halfway through.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

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

    let root = describe(dir);
    if !root.writable() {
        return fail(root.summary());
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
            let _ = e;
            return Err(describe(&path).summary());
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

/// The effective uid, for tests that must skip when running as root.
#[cfg(test)]
pub unsafe fn geteuid_for_tests() -> u32 {
    unsafe { libc_geteuid() }
}

// ------------------------------------------------------- describing a path

/// A directory's ownership and mode, as three plain numbers.
#[derive(Clone, Copy)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
}

/// Why a write was refused, and what to change.
///
/// Pure, and separate from the filesystem, because the interesting case cannot
/// be staged as a test any other way: a mode that plainly permits the write and
/// a refusal anyway needs a directory owned by someone else, which an
/// unprivileged test process cannot create.
fn explain_denial(
    e: &std::io::Error,
    dir: Owner,
    me: &Identity,
    read_only: bool,
    filesystem: Option<&str>,
    path: &Path,
) -> String {
    if read_only {
        return format!(
            "the mount is read-only. Drop `:ro` from the {} volume in your compose file.",
            path.display()
        );
    }

    if e.kind() != std::io::ErrorKind::PermissionDenied {
        return format!("{e}");
    }

    // Exactly one of the three bit groups is consulted, and which one decides
    // the fix. Naming the wrong one sends someone to change a permission that
    // was never being looked at.
    let permitted = if me.uid == dir.uid {
        dir.mode & 0o200 != 0
    } else if me.gid == dir.gid || me.groups.contains(&dir.gid) {
        dir.mode & 0o020 != 0
    } else {
        dir.mode & 0o002 != 0
    };

    if permitted {
        let fs = filesystem.unwrap_or("this filesystem");
        return format!(
            "the mode ({:04o}) permits this write and it was refused anyway, so the mode is \
             not what is denying it. On {fs} that is usually a layer above it: an ACL — on \
             ZFS/TrueNAS an NFSv4 ACL on the dataset overrides the POSIX bits, and `chmod` \
             does not touch it — or a security module (SELinux, AppArmor) denying the \
             container. Check the dataset's ACL and give uid {} write.",
            dir.mode, me.uid,
        );
    }

    if me.uid == dir.uid {
        return format!(
            "we own the directory, but the owner bits do not include write ({:04o}). \
             `chmod u+w` on it.",
            dir.mode
        );
    }
    if me.gid == dir.gid || me.groups.contains(&dir.gid) {
        return format!(
            "we are in the owning group {}, but its mode bits do not include write ({:04o}). \
             `chmod g+w` on the directory, or chown it to {}.",
            dir.gid, dir.mode, me.uid
        );
    }
    format!(
        "we are not the owner ({}) and not in the owning group ({}), and the other bits do \
         not include write ({:04o}). Add {} to `group_add` and recreate the container — a \
         restart does not change supplementary groups — or chown the directory to {}.",
        dir.uid, dir.gid, dir.mode, dir.gid, me.uid
    )
}

/// Who this process is, as the kernel sees it.
///
/// The supplementary groups matter as much as the uid: `group_add` in compose
/// is exactly a supplementary group, and the only way to know it took effect
/// is to look at the list. A compose file that lists a gid and a container that
/// was restarted rather than recreated disagree, and nothing else shows it.
pub struct Identity {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

impl Identity {
    pub fn current() -> Self {
        Self {
            uid: unsafe { libc_geteuid() },
            gid: unsafe { libc_getegid() },
            groups: unsafe { libc_getgroups() },
        }
    }
}

impl std::fmt::Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "uid {}, gid {}", self.uid, self.gid)?;
        if self.groups.is_empty() {
            write!(f, ", no supplementary groups")
        } else {
            let list: Vec<String> = self.groups.iter().map(u32::to_string).collect();
            write!(f, ", groups {}", list.join(","))
        }
    }
}

/// The mount a path actually lives on, from `/proc/self/mountinfo`.
///
/// Read rather than derived: inside a container the interesting facts — that
/// the mount is read-only, that it is a bind of something else, what
/// filesystem is underneath — are invisible from the path alone, and they are
/// the ones that explain a refusal the mode bits say should not happen.
pub struct MountInfo {
    pub mount_point: String,
    pub source: String,
    pub fstype: String,
    pub options: String,
    pub read_only: bool,
}

pub fn mount_for(path: &Path) -> Option<MountInfo> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let raw = std::fs::read_to_string("/proc/self/mountinfo").ok()?;

    let mut best: Option<MountInfo> = None;
    for line in raw.lines() {
        // `<id> <parent> <maj:min> <root> <point> <opts> [tags...] - <fs> <src> <sopts>`
        let (left, right) = line.split_once(" - ")?;
        let left: Vec<&str> = left.split_whitespace().collect();
        let right: Vec<&str> = right.split_whitespace().collect();
        if left.len() < 6 || right.len() < 2 {
            continue;
        }
        // Mount points are octal-escaped for spaces, which camera directories
        // very much have.
        let point = unescape_mount(left[4]);
        if !target.starts_with(&point) {
            continue;
        }
        if best.as_ref().is_some_and(|b| b.mount_point.len() >= point.len()) {
            continue;
        }

        let options = left[5].to_string();
        let super_options = right.get(2).copied().unwrap_or("");
        best = Some(MountInfo {
            read_only: has_option(&options, "ro") || has_option(super_options, "ro"),
            mount_point: point,
            fstype: right[0].to_string(),
            source: unescape_mount(right[1]),
            options,
        });
    }
    best
}

fn has_option(options: &str, want: &str) -> bool {
    options.split(',').any(|o| o == want)
}

/// `\040` and friends back into the characters they stand for.
fn unescape_mount(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Some(c) = std::str::from_utf8(&bytes[i + 1..i + 4])
                .ok()
                .and_then(|o| u8::from_str_radix(o, 8).ok())
            {
                out.push(c as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Everything known about one directory we depend on.
pub struct DirReport {
    pub path: PathBuf,
    pub meta: Option<std::fs::Metadata>,
    pub mount: Option<MountInfo>,
    pub write: Option<std::io::Error>,
}

pub fn describe(path: &Path) -> DirReport {
    let meta = std::fs::metadata(path).ok();
    let write = match &meta {
        Some(m) if m.is_dir() => probe_writable(path).err(),
        _ => None,
    };
    DirReport { path: path.to_path_buf(), meta, mount: mount_for(path), write }
}

impl DirReport {
    pub fn writable(&self) -> bool {
        self.meta.as_ref().is_some_and(|m| m.is_dir()) && self.write.is_none()
    }

    fn ids(&self) -> String {
        match &self.meta {
            Some(m) => format!("owned by {}:{}, mode {:04o}", m.uid(), m.gid(), m.mode() & 0o7777),
            None => "does not exist".into(),
        }
    }

    /// Why the write was refused, in terms of the thing to go and change.
    pub fn explain(&self) -> Option<String> {
        let e = self.write.as_ref()?;
        let (uid, gid, mode) = match &self.meta {
            Some(m) => (m.uid(), m.gid(), m.mode() & 0o777),
            None => return None,
        };
        let filesystem = self
            .mount
            .as_ref()
            .map(|m| format!("{} mounted from {}", m.fstype, m.source));

        Some(explain_denial(
            e,
            Owner { uid, gid, mode },
            &Identity::current(),
            self.mount.as_ref().is_some_and(|m| m.read_only),
            filesystem.as_deref(),
            &self.path,
        ))
    }

    /// One line, for a health check.
    pub fn summary(&self) -> String {
        match (&self.meta, &self.write) {
            (None, _) => format!(
                "{}: does not exist — check the volume is mounted.",
                self.path.display()
            ),
            (Some(m), _) if !m.is_dir() => format!("{} is not a directory", self.path.display()),
            (Some(_), None) => format!("{} writable ({})", self.path.display(), self.ids()),
            (Some(_), Some(e)) => {
                let why = self
                    .explain()
                    .map(|w| format!(" — {w}"))
                    .unwrap_or_default();
                format!(
                    "{} is not writable: {e} ({}); we run as {}{why}",
                    self.path.display(),
                    self.ids(),
                    Identity::current(),
                )
            }
        }
    }

    /// The long form, for `protect-manager doctor`.
    pub fn report(&self) -> String {
        let mut out = format!("{}\n", self.path.display());
        match &self.meta {
            None => out.push_str("  exists:      no\n"),
            Some(m) => {
                out.push_str("  exists:      yes\n");
                out.push_str(&format!("  kind:        {}\n", if m.is_dir() { "directory" } else { "not a directory" }));
                out.push_str(&format!("  owner:       {}:{}\n", m.uid(), m.gid()));
                out.push_str(&format!("  mode:        {:04o}\n", m.mode() & 0o7777));
            }
        }
        match &self.mount {
            None => out.push_str("  mount:       not found in /proc/self/mountinfo\n"),
            Some(mi) => {
                out.push_str(&format!("  mount point: {}\n", mi.mount_point));
                out.push_str(&format!("  source:      {}\n", mi.source));
                out.push_str(&format!("  filesystem:  {}\n", mi.fstype));
                out.push_str(&format!("  options:     {}\n", mi.options));
                out.push_str(&format!("  read-only:   {}\n", if mi.read_only { "YES" } else { "no" }));
            }
        }
        match &self.write {
            None if self.meta.is_some() => out.push_str("  write test:  OK\n"),
            None => {}
            Some(e) => {
                // `{e}` already carries the errno, so nothing is added here.
                out.push_str(&format!("  write test:  FAILED — {e}\n"));
                if let Some(why) = self.explain() {
                    out.push_str(&format!("  diagnosis:   {why}\n"));
                }
            }
        }
        out
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

/// The supplementary groups this process belongs to.
unsafe fn libc_getgroups() -> Vec<u32> {
    extern "C" {
        fn getgroups(size: i32, list: *mut u32) -> i32;
    }
    // A first call with size 0 asks how many there are without writing any.
    let n = getgroups(0, std::ptr::null_mut());
    if n <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u32; n as usize];
    let got = getgroups(n, buf.as_mut_ptr());
    if got < 0 {
        return Vec::new();
    }
    buf.truncate(got as usize);
    buf
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
            // We own the temp directory, so the fix is the owner bit — naming
            // `group_add` here would send someone to change the wrong thing.
            assert!(check.detail.contains("chmod u+w"), "{}", check.detail);
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

    fn denied() -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::PermissionDenied)
    }

    fn me(uid: u32, gid: u32, groups: &[u32]) -> Identity {
        Identity { uid, gid, groups: groups.to_vec() }
    }

    /// The shape that is impossible to stage with real files unprivileged, and
    /// the one that actually confuses people: permissive bits, refused anyway.
    #[test]
    fn a_mode_that_permits_the_write_points_above_the_mode() {
        let why = explain_denial(
            &denied(),
            Owner { uid: 950, gid: 950, mode: 0o777 },
            &me(10001, 10001, &[950]),
            false,
            Some("zfs mounted from tank/archive"),
            Path::new("/archive"),
        );
        assert!(why.contains("permits this write"), "{why}");
        assert!(why.contains("ACL"), "names the layer that is actually denying: {why}");
        assert!(why.contains("zfs"), "names the filesystem it saw: {why}");
        // Must not send anyone to chmod — that is the thing already proven
        // not to be the problem.
        assert!(!why.contains("chmod g+w"), "{why}");
    }

    #[test]
    fn each_bit_group_names_its_own_fix() {
        // Owner, no write bit.
        let why = explain_denial(
            &denied(),
            Owner { uid: 10001, gid: 10001, mode: 0o500 },
            &me(10001, 10001, &[]),
            false,
            None,
            Path::new("/archive"),
        );
        assert!(why.contains("chmod u+w"), "{why}");

        // In the owning group, group bits lack write. This is the case
        // `group_add` cannot fix, and saying so is the whole point.
        let why = explain_denial(
            &denied(),
            Owner { uid: 950, gid: 950, mode: 0o755 },
            &me(10001, 10001, &[950]),
            false,
            None,
            Path::new("/archive"),
        );
        assert!(why.contains("chmod g+w"), "{why}");
        assert!(!why.contains("group_add"), "already in the group: {why}");

        // Neither owner nor group — the one `group_add` does fix.
        let why = explain_denial(
            &denied(),
            Owner { uid: 950, gid: 950, mode: 0o750 },
            &me(10001, 10001, &[]),
            false,
            None,
            Path::new("/archive"),
        );
        assert!(why.contains("group_add"), "{why}");
        assert!(why.contains("recreate"), "a restart does not apply group_add: {why}");
    }

    #[test]
    fn a_read_only_mount_is_reported_as_the_mount_not_the_mode() {
        let why = explain_denial(
            &denied(),
            Owner { uid: 10001, gid: 10001, mode: 0o777 },
            &me(10001, 10001, &[]),
            true,
            None,
            Path::new("/archive"),
        );
        assert!(why.contains("read-only"), "{why}");
        assert!(why.contains(":ro"), "names the compose change: {why}");
    }

    #[test]
    fn mount_points_with_spaces_survive_the_octal_escaping() {
        // Camera directories have spaces, and mountinfo escapes them.
        assert_eq!(unescape_mount(r"/archive/G4\040Instant"), "/archive/G4 Instant");
        assert_eq!(unescape_mount("/archive"), "/archive");
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

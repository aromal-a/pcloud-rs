//! **PLATFORM: FreeBSD, NetBSD, OpenBSD, DragonFlyBSD.**
//! **GATING: `#[cfg(any(target_os = "freebsd", target_os = "netbsd",
//! target_os = "openbsd", target_os = "dragonfly"))]`** -- gated at the `mod bsd;` line in
//! `platform/mod.rs`.
//!
//! All four targets use their native libfuse/refuse ABI through `fuser`.
//! `MountinfoReader` wraps `getmntinfo(3)` for live mount discovery and
//! orphan cleanup.
//!
//! This module implements mountpoint validation (stat + getmntinfo
//! cross-check), runtime
//! probe (presence of each OS's native FUSE device), conservative defaults,
//! the native `fuser` session lifecycle, and signal-driven cleanup.

use std::io;
use std::path::Path;

use crate::mount_orphan::MountinfoReader;
use crate::mount_service::{MountError, MountOptions};
use crate::platform::PlatformMount;

// `getmntinfo(3)` returns a kernel mount-table snapshot whose entry
// type differs across BSD flavours:
//
// * **FreeBSD / OpenBSD / DragonFly** expose the historical BSD
//   `struct statfs` (with `f_mntonname` + `f_fstypename` arrays).
// * **NetBSD** dropped `statfs` and exposes only POSIX `struct statvfs`
//   (also with `f_mntonname` + `f_fstypename` — NetBSD extended POSIX
//   to keep the BSD field names). Same `getmntinfo` signature, same
//   field accesses; only the struct name differs.
//
// Alias to the right type per target so the rest of this module can
// access the fields uniformly.
#[cfg(target_os = "netbsd")]
type GetmntinfoStat = libc::statvfs;
#[cfg(not(target_os = "netbsd"))]
type GetmntinfoStat = libc::statfs;

/// Native BSD platform-mount implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct BsdPlatformMount;

impl PlatformMount for BsdPlatformMount {
    /// Validate `mountpoint` using BSD `stat(2)` + `getmntinfo(3)`.
    ///
    /// Rejections (in order):
    /// 1. path does not exist                       -> `MountpointMissing`
    /// 2. path is not a directory                   -> `MountpointNotDirectory`
    /// 3. directory is non-empty (first 3 entries)  -> `MountpointNotEmpty`
    /// 4. group- or world-writable (mode & 0o022)   -> `MountpointWorldWritable`
    /// 5. path already appears in `getmntinfo`      -> `Unsupported("already mounted: ...")`
    ///
    /// Note: the `MountError` enum does not have a dedicated
    /// `MountpointAlreadyMounted` variant, so "already mounted" is
    /// reported as `MountError::Unsupported` with a stable diagnostic
    /// prefix (`"already mounted: "`). Callers that need to match on
    /// this case can string-match the prefix. See `mount_service.rs`.
    fn validate_mountpoint(&self, mountpoint: &Path) -> Result<(), MountError> {
        use std::os::unix::fs::MetadataExt;

        let meta = match std::fs::symlink_metadata(mountpoint) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(MountError::MountpointMissing(mountpoint.to_path_buf()));
            }
            Err(e) => return Err(MountError::Io(e)),
        };

        if meta.file_type().is_symlink() {
            return Err(MountError::MountpointSymlink(mountpoint.to_path_buf()));
        }

        if !meta.is_dir() {
            return Err(MountError::MountpointNotDirectory(mountpoint.to_path_buf()));
        }

        // Fail fast on the first entry; we do not enumerate the directory.
        let mut rd = std::fs::read_dir(mountpoint)?;
        match rd.next() {
            Some(Ok(_)) => {
                return Err(MountError::MountpointNotEmpty(mountpoint.to_path_buf()));
            }
            Some(Err(e)) => return Err(MountError::Io(e)),
            None => {}
        }

        // A per-user mount must remain owned by the invoking effective UID.
        let current_uid = unsafe { libc::geteuid() };
        if meta.uid() != current_uid {
            return Err(MountError::MountpointNotOwned {
                path: mountpoint.to_path_buf(),
                owner: meta.uid(),
                current: current_uid,
            });
        }

        let mode = meta.mode();
        // Reject group- or world-writable directories. A pCloud mount
        // must be privately owned by the invoking user; a wider mode
        // permits other local accounts to race the staging area.
        if mode & 0o022 != 0 {
            return Err(MountError::MountpointWorldWritable {
                path: mountpoint.to_path_buf(),
                mode: mode & 0o7777,
            });
        }

        // Cross-check the kernel mount table: if `getmntinfo(3)`
        // already reports `mountpoint` as an active mount, refuse to
        // proceed (mounting on top would shadow an existing
        // filesystem).
        if path_is_current_mount(mountpoint)? {
            return Err(MountError::Unsupported(format!(
                "already mounted: {}",
                mountpoint.display()
            )));
        }

        Ok(())
    }

    /// Runtime probe.
    ///
    /// Succeeds when the native userspace-filesystem device exists:
    /// `/dev/fuse` (FreeBSD), `/dev/puffs` or the perfuse compatibility
    /// endpoint (NetBSD), `/dev/fuse0` (OpenBSD), and `/dev/fuse`
    /// (DragonFlyBSD).
    fn probe_supported(&self) -> Result<(), MountError> {
        #[cfg(target_os = "freebsd")]
        {
            if Path::new("/dev/fuse").exists() {
                return Ok(());
            }
            return Err(MountError::Unsupported(
                "/dev/fuse missing; load the fusefs kernel module (kldload fusefs)".to_string(),
            ));
        }

        #[cfg(target_os = "netbsd")]
        {
            if Path::new("/dev/puffs").exists() || Path::new("/dev/fuse").exists() {
                return Ok(());
            }
            return Err(MountError::Unsupported(
                "/dev/puffs and /dev/fuse are missing; enable PUFFS/refuse or perfused".to_string(),
            ));
        }

        #[cfg(target_os = "openbsd")]
        {
            if Path::new("/dev/fuse0").exists() {
                return Ok(());
            }
            return Err(MountError::Unsupported(
                "/dev/fuse0 missing; boot a kernel with the FUSE pseudo-device enabled".to_string(),
            ));
        }

        #[cfg(target_os = "dragonfly")]
        {
            if Path::new("/dev/fuse").exists() {
                return Ok(());
            }
            return Err(MountError::Unsupported(
                "/dev/fuse missing; load the DragonFly FUSE kernel module".to_string(),
            ));
        }

        // Unreachable under the module cfg-gate, but kept so the
        // function remains total if the gate ever widens. Surface the
        // concrete remediation hint so operators do not see an opaque
        // "UnsupportedPlatform" with no guidance.
        #[allow(unreachable_code)]
        Err(MountError::Unsupported(
            "BSD FUSE support requires fusefs-libs (pkg install fusefs-libs), \
             the 'fuse' kernel module loaded (kldload fuse), and \
             sysctl vfs.usermount=1 for non-root mounts"
                .to_string(),
        ))
    }

    /// Conservative defaults for BSD: no cross-user access, a stable
    /// `fs_name` for `/proc`-equivalent listings, and writable (the
    /// cross-platform `MountService` still enforces its own policy).
    fn default_options(&self) -> MountOptions {
        MountOptions {
            read_only: false,
            fs_name: Some("pcloud-rs".to_string()),
            allow_other: false,
            attr_timeout_secs: 1.0,
            entry_timeout_secs: 1.0,
            max_readahead: 128 * 1024,
        }
    }

    fn mount_adapter(
        &self,
        adapter: Box<dyn crate::fuse_adapter::FuseAdapter>,
        mountpoint: &Path,
        options: MountOptions,
    ) -> Result<crate::mount_service::MountHandle, MountError> {
        let shim = crate::platform::fuser_shim::BoxedFuserShim::new(adapter);
        mount_fuser_filesystem(mountpoint, shim, options)
    }
}

// -----------------------------------------------------------------------------
// BSD kernel mount lifecycle.
// -----------------------------------------------------------------------------

/// RAII owner for a live BSD `fuser` background session.
///
/// Dropping the session asks libfuse to unmount. Explicit teardown verifies
/// the kernel mount table for up to two seconds and reports a lingering mount
/// instead of silently claiming success.
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub struct BsdMountHandle {
    mountpoint: std::path::PathBuf,
    session: Option<fuser::BackgroundSession>,
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
impl BsdMountHandle {
    /// Drop the userspace session and verify that `getmntinfo(3)` no longer
    /// contains the mountpoint.
    pub fn unmount(mut self) -> Result<(), MountError> {
        drop(self.session.take());

        let result = (|| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if !path_is_current_mount(&self.mountpoint)? {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }

            Err(MountError::Io(io::Error::other(format!(
                "BSD FUSE mount still present after session teardown: {}",
                self.mountpoint.display()
            ))))
        })();
        reaper::unregister_mount(&self.mountpoint);
        result
    }
}

/// Mount a fully composed filesystem using the native BSD libfuse/refuse
/// bridge exposed through `fuser`.
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub fn mount_fuser_filesystem<F>(
    mountpoint: &Path,
    filesystem: F,
    options: MountOptions,
) -> Result<crate::mount_service::MountHandle, MountError>
where
    F: fuser::Filesystem + Send + 'static,
{
    BsdPlatformMount.probe_supported()?;
    BsdPlatformMount.validate_mountpoint(mountpoint)?;
    reaper::install_bsd_signal_reaper();

    let fuse_options = crate::platform::fuser_shim::build_fuse_options(&options);

    let session = fuser::spawn_mount2(filesystem, mountpoint, &fuse_options)
        .map_err(|error| MountError::Fuser(error.to_string()))?;
    reaper::register_mount(mountpoint);
    Ok(crate::mount_service::MountHandle::from_bsd(
        BsdMountHandle {
            mountpoint: mountpoint.to_path_buf(),
            session: Some(session),
        },
    ))
}

/// Return `Ok(true)` if `path` currently appears as a mountpoint in
/// `getmntinfo(3)`.
fn path_is_current_mount(path: &Path) -> io::Result<bool> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    // SAFETY: `getmntinfo` accepts a non-null out-pointer and returns
    // the number of entries it wrote. On success `mntbuf` points at a
    // libc-owned static array (we do not free it). On failure it
    // returns 0 and sets `errno`.
    let mut mntbuf: *mut GetmntinfoStat = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut mntbuf, libc::MNT_NOWAIT) };
    if count <= 0 || mntbuf.is_null() {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `mntbuf` points to `count` initialized `statfs` structs
    // owned by libc. We read through the slice only for the lifetime
    // of this call and never retain the pointer.
    let entries = unsafe { std::slice::from_raw_parts(mntbuf, count as usize) };
    for entry in entries {
        let mountpoint = cstr_to_string(entry.f_mntonname.as_ptr());
        if mountpoint.is_empty() {
            continue;
        }
        if Path::new(&mountpoint) == canonical || Path::new(&mountpoint) == path {
            return Ok(true);
        }
    }
    Ok(false)
}

/// BSD mountinfo reader backed by `getmntinfo(3)`.
///
/// Enumerates the kernel mount table and emits a
/// `/proc/self/mountinfo`-shaped payload containing only FUSE-backed
/// entries whose filesystem type is FUSE-backed and whose filesystem source
/// identifies pCloud. Foreign sshfs/rclone/other FUSE mounts are deliberately
/// omitted so orphan cleanup can never claim them.
#[derive(Debug, Default, Clone, Copy)]
pub struct BsdMountinfoReader;

impl MountinfoReader for BsdMountinfoReader {
    fn read(&self) -> io::Result<String> {
        read_getmntinfo()
    }
}

/// Shared `getmntinfo(3)` enumeration used by BSD and macOS.
///
/// Returns a `/proc/self/mountinfo`-compatible payload (one line per
/// matching mount) suitable for
/// [`crate::mount_orphan::parse_pcloud_mounts`].
pub(crate) fn read_getmntinfo() -> io::Result<String> {
    // SAFETY: `getmntinfo` accepts a non-null out-pointer in which the
    // kernel stores a pointer to a statically-allocated array owned by
    // libc. On success it returns the number of entries (>0). On failure
    // it returns 0 and sets `errno`. We do not free the returned buffer
    // (libc owns it) and we do not retain the pointer past this call.
    let mut mntbuf: *mut GetmntinfoStat = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut mntbuf, libc::MNT_NOWAIT) };
    if count <= 0 || mntbuf.is_null() {
        return Err(io::Error::last_os_error());
    }

    let mut out = String::new();
    // SAFETY: `mntbuf` points to `count` initialized `statfs` structs
    // owned by libc; we only read through the slice and never outlive
    // this function scope. `count` is positive (checked above) and we
    // cast via `as usize` which is the canonical idiom here.
    let entries = unsafe { std::slice::from_raw_parts(mntbuf, count as usize) };
    for entry in entries {
        let fstype = cstr_to_string(entry.f_fstypename.as_ptr());
        if !fstype.contains("fuse") {
            continue;
        }
        let src = cstr_to_string(entry.f_mntfromname.as_ptr());
        let identity = format!(
            "{} {}",
            fstype.to_ascii_lowercase(),
            src.to_ascii_lowercase()
        );
        if !identity.contains("pcloud-rs") {
            continue;
        }
        let mountpoint = cstr_to_string(entry.f_mntonname.as_ptr());
        if mountpoint.is_empty() {
            continue;
        }

        // Emit a minimal `/proc/self/mountinfo`-shaped line. The parser
        // needs at least 5 space-delimited fields on the left of " - "
        // (mountpoint is field index 4) and the first right-hand field
        // must match `PCLOUD_FUSE_TYPES`.
        //
        // Fields: id parent_id major:minor root mountpoint - fstype src opts
        out.push_str("0 0 0:0 / ");
        out.push_str(&escape_mountinfo(&mountpoint));
        out.push_str(" - fuse.pcloud-rs ");
        if src.is_empty() {
            out.push_str("pcloud");
        } else {
            out.push_str(&escape_mountinfo(&src));
        }
        out.push_str(" rw\n");
    }
    Ok(out)
}

/// Read a NUL-terminated C string and lossily convert to a Rust `String`.
/// Returns an empty string when the pointer is null.
fn cstr_to_string(ptr: *const libc::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: `ptr` is a non-null pointer into a libc-owned `statfs`
    // struct whose character arrays are guaranteed NUL-terminated by
    // the kernel (see statfs(2)). We do not retain the CStr reference
    // beyond this scope.
    let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
    cstr.to_string_lossy().into_owned()
}

/// Escape whitespace characters in a path so the `/proc/self/mountinfo`
/// parser can recover the original bytes. Mirrors the octal escaping
/// documented in `proc(5)` for space (`\040`), tab (`\011`), newline
/// (`\012`) and backslash (`\134`).
fn escape_mountinfo(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            ' ' => out.push_str("\\040"),
            '\t' => out.push_str("\\011"),
            '\n' => out.push_str("\\012"),
            '\\' => out.push_str("\\134"),
            other => out.push(other),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// M-5.1: Signal-driven BSD reaper. The signal handler performs only an
// atomic store; a worker thread drains ACTIVE_MOUNTS with unmount(2).
// ---------------------------------------------------------------------------

/// Signal-safe shutdown coordination and active-mount registry for BSD.
///
/// The signal trampoline only flips an atomic flag; a normal worker thread
/// drains registered mounts with `unmount(2)`, keeping unsafe work outside
/// the signal context.
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub mod reaper {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
    static SIGNAL_HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();
    static REAPER_INSTALLED: OnceLock<()> = OnceLock::new();

    /// Process-wide registry of BSD mountpoints that the daemon currently
    /// owns. Mirrors the Linux `ACTIVE_MOUNTS` set
    /// (`platform/linux.rs::registry`); the reaper drains this set on
    /// SIGTERM/SIGINT and issues `libc::unmount(path, MNT_FORCE)` per
    /// entry so a process abort or service stop does not leave a stale
    /// kernel mount that the operator must clean up by hand.
    ///
    /// Every successful native BSD mount is registered here and removed
    /// during explicit RAII teardown.
    static ACTIVE_MOUNTS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();

    fn registry() -> &'static Mutex<BTreeSet<PathBuf>> {
        ACTIVE_MOUNTS.get_or_init(|| Mutex::new(BTreeSet::new()))
    }

    /// Canonicalise a mount path to a stable key. Mirrors the Linux
    /// `canonical_key` derivation so register/unregister round-trip
    /// even when the path no longer exists (mid-teardown).
    fn canonical_key(path: &Path) -> PathBuf {
        if let Ok(c) = std::fs::canonicalize(path) {
            return c;
        }
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        let mut normalised = PathBuf::new();
        for comp in abs.components() {
            normalised.push(comp.as_os_str());
        }
        normalised
    }

    /// Register `path` in the BSD active-mount set. Idempotent on the
    /// canonical key; a double-register logs `error!` (lifecycle bug).
    pub fn register_mount(path: &Path) {
        let key = canonical_key(path);
        if let Ok(mut guard) = registry().lock() {
            if !guard.insert(key) {
                log::error!(
                    "ACTIVE_MOUNTS (BSD) double-register: {path:?}; \
                     this indicates a mount lifecycle bug"
                );
            }
        }
    }

    /// Remove `path` from the active-mount set. Logs `error!` on miss.
    pub fn unregister_mount(path: &Path) {
        let key = canonical_key(path);
        if let Ok(mut guard) = registry().lock() {
            if !guard.remove(&key) {
                log::error!(
                    "ACTIVE_MOUNTS (BSD) unregister miss: {path:?} (key={key:?}); \
                     unbalanced mount/unmount lifecycle bug"
                );
            }
        }
    }

    /// Snapshot the current registry. Test-only helper that lets unit
    /// tests assert pre/post state around the reaper without exposing
    /// the `Mutex<BTreeSet<_>>` directly.
    #[cfg(test)]
    pub(super) fn snapshot_registry() -> Vec<PathBuf> {
        registry()
            .lock()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Returns `true` when SIGTERM or SIGINT has been received.
    #[allow(dead_code)]
    pub fn shutdown_requested() -> bool {
        SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
    }

    /// Test-only: simulate a SIGTERM by flipping the shutdown flag and
    /// invoking the reap routine inline so unit tests can assert that
    /// the registry is drained without spawning a thread or sending a
    /// real signal.
    #[cfg(test)]
    pub(super) fn force_reap_for_tests() {
        SHUTDOWN_REQUESTED.store(true, Ordering::Release);
        reap_all_mounts();
    }

    extern "C" fn signal_trampoline(_sig: libc::c_int) {
        SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
    }

    /// Install BSD signal handler and reaper thread.
    ///
    /// Drains a process-wide [`ACTIVE_MOUNTS`] registry and issues
    /// `libc::unmount(path, MNT_FORCE)` per entry, mirroring the Linux
    /// `umount2(MNT_DETACH)` reaper.
    ///
    /// M-5.1.
    pub fn install_bsd_signal_reaper() {
        SIGNAL_HANDLER_INSTALLED.get_or_init(|| {
            // SAFETY: sigaction is called once per signal during process
            // lifetime with a static handler. The handler stores only to
            // an AtomicBool, which is async-signal-safe.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = signal_trampoline as *const () as usize;
                sa.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                let _ = libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
                let _ = libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
            }
        });

        REAPER_INSTALLED.get_or_init(|| {
            // M-5.1 / M-5.3: surface spawn failure via log::error!.
            if let Err(e) = std::thread::Builder::new()
                .name("pcloudfs-bsd-reaper".to_string())
                .spawn(bsd_reaper_main)
            {
                log::error!(
                    "pcloud-fs (BSD): failed to spawn reaper thread: {e}; \
                     SIGTERM/SIGINT will not trigger mount cleanup"
                );
            }
        });
    }

    fn bsd_reaper_main() {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
                reap_all_mounts();
                return;
            }
        }
    }

    /// Drain the active-mount registry and issue
    /// `libc::unmount(path, MNT_FORCE)` per entry. Logs at `warn!` on
    /// each individual `unmount` failure so an operator can act, but
    /// continues draining so a single hung mount cannot hold up the
    /// rest of the process exit.
    ///
    /// The unit test in this module simulates a registered mount and
    /// asserts the registry empties after `force_reap_for_tests`; the
    /// `unmount(2)` syscall itself returns `ENOENT` for the simulated
    /// path and we tolerate that error by design (the test does not
    /// assert any kernel side-effect).
    fn reap_all_mounts() {
        let paths: Vec<PathBuf> = registry()
            .lock()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default();
        if paths.is_empty() {
            log::warn!(
                "pcloud-fs (BSD): shutdown signal received; \
                 ACTIVE_MOUNTS is empty (no live mounts to reap)"
            );
            return;
        }
        log::warn!(
            "pcloud-fs (BSD): shutdown signal received; \
             reaping {} active mount(s) via libc::unmount(MNT_FORCE)",
            paths.len()
        );
        for path in paths {
            if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
                // SAFETY: `unmount(2)` is a direct syscall taking a
                // NUL-terminated path and a flags integer. `c` owns the
                // path bytes for the duration of the call. `MNT_FORCE`
                // requests the kernel to detach even if there are
                // open file references (matches the Linux
                // `MNT_DETACH` semantics). The flag value is
                // architecture-stable across the BSD targets we
                // gate on.
                let rc = unsafe { libc::unmount(c.as_ptr(), libc::MNT_FORCE) };
                if rc != 0 {
                    let e = std::io::Error::last_os_error();
                    log::warn!(
                        "pcloud-fs (BSD) reaper: unmount({}) failed: {}",
                        path.display(),
                        e
                    );
                }
            }
            if let Ok(mut guard) = registry().lock() {
                guard.remove(&path);
            }
        }
    }
}

#[cfg(all(
    test,
    any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )
))]
mod tests {
    use super::*;

    /// Smoke test: construct the platform type and exercise the
    /// trait-method signatures we implement. Tests are difficult
    /// without a real BSD host (getmntinfo, /dev/fuse, directory
    /// layout), so this test only proves the BSD path compiles and
    /// the signatures line up with the trait.
    #[test]
    fn validate_mountpoint_paths_compile() {
        let plat = BsdPlatformMount;
        let _ = plat.default_options();

        // Non-existent path: must yield MountpointMissing.
        let missing = Path::new("/nonexistent/pcloud/mount/__bsd_smoke__");
        let err = plat.validate_mountpoint(missing).unwrap_err();
        assert!(matches!(err, MountError::MountpointMissing(_)));

        // probe_supported is platform-policy; we only assert it returns.
        let _ = plat.probe_supported();
    }

    /// Audit-06 stream E (bd-xplat-bsd, CLAUDE.md "Signal-driven mount
    /// cleanup posture"): simulate a registered mount, invoke the
    /// reaper inline, and assert that the registry is drained. Live
    /// `unmount(2)` against real BSD hardware is out of scope (the
    /// reaper internally tolerates a non-zero return for the simulated
    /// path); the contract this test enforces is the in-process
    /// registry drain.
    #[test]
    fn reaper_drains_registry_on_simulated_signal() {
        let path = std::path::PathBuf::from("/tmp/__pcloud_bsd_reaper_smoke__");
        // The file does not need to exist for register_mount to
        // accept it: canonical_key falls back to component
        // normalisation when canonicalize fails.
        super::reaper::register_mount(&path);

        // Confirm the registry now contains the path. We look for any
        // suffix match because canonical_key may have rewritten the
        // path through CWD / canonicalize.
        let before = super::reaper::snapshot_registry();
        assert!(
            before
                .iter()
                .any(|p| p.ends_with("__pcloud_bsd_reaper_smoke__")),
            "expected registered mount in BSD ACTIVE_MOUNTS, got {before:?}"
        );

        // Simulate SIGTERM. The reaper internally calls
        // `libc::unmount(MNT_FORCE)` — this returns ENOENT for the
        // simulated path; we tolerate that and assert only the
        // registry is empty afterwards.
        super::reaper::force_reap_for_tests();

        let after = super::reaper::snapshot_registry();
        assert!(
            !after
                .iter()
                .any(|p| p.ends_with("__pcloud_bsd_reaper_smoke__")),
            "expected registry drained after reap, got {after:?}"
        );
    }
}

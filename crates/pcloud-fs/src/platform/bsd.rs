//! **PLATFORM: FreeBSD, NetBSD, OpenBSD.**
//! **GATING: `#[cfg(any(target_os = "freebsd", target_os = "netbsd",
//! target_os = "openbsd"))]`** -- gated at the `mod bsd;` line in
//! `platform/mod.rs`.
//!
//! - FreeBSD (tier 2): libfuse2 via the `fuser` crate (mount path still
//!   TODO). `MountinfoReader` wraps `getmntinfo(3)`.
//! - OpenBSD / NetBSD (tier 3): community-maintained; no first-party
//!   mount implementation planned. `getmntinfo(3)` is still available
//!   and is used here for orphan detection.
//!
//! This module implements the non-FFI portions of `PlatformMount` on
//! BSD: mountpoint validation (stat + getmntinfo cross-check), runtime
//! probe (presence of `/dev/fuse` on FreeBSD/NetBSD; intentional
//! kext-needed error on OpenBSD), and conservative defaults.
//!
//! The kernel (un)mount path itself is **not** implemented here --
//! tracked under `bd-xplat-bsd`.

use std::io;
use std::path::Path;

use crate::mount_orphan::MountinfoReader;
use crate::mount_service::{MountError, MountOptions};
use crate::platform::PlatformMount;

/// BSD platform-mount implementation (validation-only; no kernel mount).
///
/// TODO(bd-xplat-bsd): on FreeBSD, wire `fuser` (libfuse2) with BSD mount
/// flags. On OpenBSD/NetBSD this may remain unimplemented.
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

        let meta = match std::fs::metadata(mountpoint) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(MountError::MountpointMissing(mountpoint.to_path_buf()));
            }
            Err(e) => return Err(MountError::Io(e)),
        };

        if !meta.is_dir() {
            return Err(MountError::MountpointNotDirectory(mountpoint.to_path_buf()));
        }

        // Fail fast: only peek at the first 3 entries. If any are
        // present we reject; we do not enumerate the whole directory.
        let mut rd = std::fs::read_dir(mountpoint)?;
        for _ in 0..3 {
            match rd.next() {
                Some(Ok(_)) => {
                    return Err(MountError::MountpointNotEmpty(mountpoint.to_path_buf()));
                }
                Some(Err(e)) => return Err(MountError::Io(e)),
                None => break,
            }
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
    /// - FreeBSD / NetBSD: succeed when `/dev/fuse` is present (the
    ///   kernel fuse module is loaded and the device node exists).
    ///   When absent, return `Unsupported("load fuse kernel module ...")`.
    /// - OpenBSD: no first-party fuse module ships by default. Return
    ///   `Unsupported("KEXT_NEEDED: ...")` so callers can distinguish
    ///   the platform-policy case from a transient probe failure.
    fn probe_supported(&self) -> Result<(), MountError> {
        #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
        {
            if Path::new("/dev/fuse").exists() {
                return Ok(());
            }
            return Err(MountError::Unsupported(
                "/dev/fuse missing; load the fuse kernel module (kldload fuse / modload fuse)"
                    .to_string(),
            ));
        }

        #[cfg(target_os = "openbsd")]
        {
            return Err(MountError::Unsupported(
                "KEXT_NEEDED: OpenBSD ships no fuse kernel module by default".to_string(),
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
            fs_name: Some("pcloud".to_string()),
            allow_other: false,
            attr_timeout_secs: 1.0,
            entry_timeout_secs: 1.0,
            max_readahead: 128 * 1024,
        }
    }
}

/// Return `Ok(true)` if `path` currently appears as a mountpoint in
/// `getmntinfo(3)`.
fn path_is_current_mount(path: &Path) -> io::Result<bool> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    // SAFETY: `getmntinfo` accepts a non-null out-pointer and returns
    // the number of entries it wrote. On success `mntbuf` points at a
    // libc-owned static array (we do not free it). On failure it
    // returns 0 and sets `errno`.
    let mut mntbuf: *mut libc::statfs = std::ptr::null_mut();
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
/// entries (those where `f_fstypename` contains `"fuse"`, which covers
/// `fusefs` on FreeBSD and the various FUSE subtypes on NetBSD/OpenBSD).
///
/// Each emitted line is normalized to advertise `fuse.pcloud` as the
/// filesystem type so the cross-platform parser in
/// [`crate::mount_orphan::parse_pcloud_mounts`] treats the entry as a
/// pCloud-owned FUSE mount. The daemon then reconciles against its own
/// known-mount set; this module does not make ownership claims on its
/// own.
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
    let mut mntbuf: *mut libc::statfs = std::ptr::null_mut();
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
        out.push_str(" - fuse.pcloud ");
        let src = cstr_to_string(entry.f_mntfromname.as_ptr());
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

#[cfg(all(
    test,
    any(target_os = "freebsd", target_os = "netbsd", target_os = "openbsd")
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
}

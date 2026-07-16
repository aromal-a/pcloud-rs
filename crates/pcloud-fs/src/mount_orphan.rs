//! **PLATFORM: all** (Linux | FreeBSD | macOS | Windows).
//! **GATING: none** at the trait/parser level; the Linux-only concrete
//! reader (`ProcMountinfoReader`) is re-exported under
//! `#[cfg(target_os = "linux")]`.
//!
//! Orphan-mount detection and recovery for P1.4.
//!
//! The pCloud daemon can crash, get SIGKILLed, or be taken down out of
//! order while a FUSE mount is still live. When that happens the kernel
//! keeps the `fuse.pcloud*` mount entry around even though no userspace
//! daemon owns it. A subsequent daemon start must notice those orphan
//! mounts and refuse to start (or, with an explicit operator override,
//! tear them down) rather than silently layering a second mount on top.
//!
//! This module provides:
//!
//! * [`parse_pcloud_mounts`] — extract mounts carrying pcloud-rs's private
//!   filesystem marker from a `/proc/self/mountinfo` payload.
//! * [`detect_orphans`] — filter those to the ones the daemon doesn't
//!   currently own.
//! * [`MountinfoReader`] — dependency-injection seam so tests can feed a
//!   canned payload instead of hitting `/proc`.
//! * [`fusermount_unmount`] — call `fusermount3 -u` (falling back to
//!   `fusermount -u`) to release a single mount.
//!
//! All I/O is kept here so the higher-level mount runtime can stay
//! platform-agnostic in its core logic.
//!
//! # `/proc/self/mountinfo` parser (Linux)
//!
//! Each line of `/proc/self/mountinfo` is a space-separated record
//! documented in `Documentation/filesystems/proc.rst`:
//!
//! ```text
//! 36 35 98:0 /mnt1 /mnt/cloud rw,noatime master:1 - fuse.pcloud-rs pcloud-rs rw
//! (1) (2) (3)  (4)   (5)      (6)         (7)      (8)   (9)          (10) (11)
//! ```
//!
//! Field 5 is the mount point (after octal escape decoding of `\040`
//! `\011` `\012` `\134` for space/tab/newline/backslash). Field 9 —
//! the text after the ` - ` separator — is the filesystem type and is
//! matched against [`PCLOUD_FUSE_TYPES`]. Fields 6 and 7 are parsed to
//! ignore autofs indirection. Malformed lines are skipped silently; a
//! hostile `/proc` payload cannot crash the parser.
//!
//! # BSD / macOS equivalent: `getmntinfo(3)`
//!
//! On FreeBSD, OpenBSD, NetBSD, and macOS there is no `/proc/self/
//! mountinfo`. The equivalent is the libc call `getmntinfo(3)`, which
//! returns a `struct statfs *` array (FreeBSD) or
//! `struct statfs64 *` (macOS) covering every live mount. The BSD/
//! macOS `MountinfoReader` implementations:
//!
//! 1. call `getmntinfo(&mut buf, MNT_NOWAIT)`,
//! 2. for each entry, format `f_mntfromname`, `f_mntonname`, and
//!    `f_fstypename` into a synthetic line matching the Linux schema
//!    so the same [`parse_pcloud_mounts`] routine can consume it,
//! 3. require both a FUSE-family type and a pCloud source identity before
//!    normalizing the entry to the private `fuse.pcloud-rs` marker.
//!
//! Keeping the parser string-based means all OSes share one
//! well-tested code path.
//!
//! # Windows volume enumeration
//!
//! `crate::platform::windows::WindowsMountinfoReader` enumerates Win32
//! volumes, selects the private `pcloud-rs` filesystem marker, and expands
//! every volume path through `GetVolumePathNamesForVolumeNameW`. This covers
//! drive letters and directory mount points without classifying unrelated
//! WinFSP filesystems—or the official pCloud client—as daemon-owned mounts.
//! Windows mountpoint comparisons also tolerate drive-letter/path case.

use std::io;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

/// Filesystem types that prove ownership by this pcloud-rs daemon.
///
/// Legacy `fuse.pcloud` and `fuse.pclsync` names are deliberately excluded:
/// they may belong to the official pCloud client or another implementation.
/// Sync-root discovery recognizes those drives separately, but orphan cleanup
/// must never refuse startup or unmount a volume it cannot prove it owns.
pub const PCLOUD_FUSE_TYPES: &[&str] = &["fuse.pcloud-rs"];

/// A single pCloud-owned FUSE mount, as observed in `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcloudMountEntry {
    /// Mount point within the current mount namespace.
    pub mount_point: PathBuf,
    /// Filesystem type (one of [`PCLOUD_FUSE_TYPES`]).
    pub fs_type: String,
}

/// Abstraction over "read `/proc/self/mountinfo`" so tests can inject a
/// fixture payload without touching the real `/proc`.
pub trait MountinfoReader: Send + Sync {
    /// Return the current mountinfo payload as a single UTF-8 string.
    /// Implementations on Linux read `/proc/self/mountinfo`; tests return
    /// a canned fixture.
    fn read(&self) -> io::Result<String>;
}

/// Default reader that reads `/proc/self/mountinfo` directly (Linux).
///
/// Re-exported from [`crate::platform::linux`] for backward compatibility
/// with pre-refactor call-sites.
#[cfg(target_os = "linux")]
pub use crate::platform::linux::ProcMountinfoReader;

/// Non-Linux stub of `ProcMountinfoReader`. Returns an empty payload so
/// orphan detection becomes a no-op on platforms that do not expose
/// `/proc/self/mountinfo`. Real BSD/macOS/Windows readers live in the
/// platform-specific modules under `crate::platform`.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcMountinfoReader;

#[cfg(not(target_os = "linux"))]
impl MountinfoReader for ProcMountinfoReader {
    fn read(&self) -> io::Result<String> {
        Ok(String::new())
    }
}

/// In-memory reader used by tests. Wraps a `String`.
#[derive(Debug, Clone)]
pub struct StaticMountinfoReader {
    payload: String,
}

impl StaticMountinfoReader {
    /// Construct a reader that returns `payload` verbatim on every
    /// [`MountinfoReader::read`] call. Used by tests to exercise
    /// mount-orphan detection without touching `/proc/self/mountinfo`.
    #[must_use]
    pub fn new(payload: impl Into<String>) -> Self {
        Self {
            payload: payload.into(),
        }
    }
}

impl MountinfoReader for StaticMountinfoReader {
    fn read(&self) -> io::Result<String> {
        Ok(self.payload.clone())
    }
}

/// Parse a `/proc/self/mountinfo` payload and return only entries carrying
/// pcloud-rs's private filesystem type (see [`PCLOUD_FUSE_TYPES`]).
///
/// The `/proc/self/mountinfo` format is documented in `proc(5)`; fields
/// on the left of `" - "` are space-delimited, field index 4 is the
/// mountpoint, and the first field on the right of `" - "` is the
/// filesystem type. Malformed lines are skipped.
#[must_use]
pub fn parse_pcloud_mounts(payload: &str) -> Vec<PcloudMountEntry> {
    let mut out = Vec::new();
    for line in payload.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(sep) = line.find(" - ") else {
            continue;
        };
        let (left, right) = (&line[..sep], &line[sep + 3..]);
        let left_fields: Vec<&str> = left.split_ascii_whitespace().collect();
        let right_fields: Vec<&str> = right.split_ascii_whitespace().collect();
        if left_fields.len() < 5 || right_fields.is_empty() {
            continue;
        }
        let fs_type = right_fields[0];
        if !PCLOUD_FUSE_TYPES.contains(&fs_type) {
            continue;
        }
        let mount_point = unescape_mountinfo(left_fields[4]);
        out.push(PcloudMountEntry {
            mount_point: PathBuf::from(mount_point),
            fs_type: fs_type.to_owned(),
        });
    }
    out
}

/// Detect orphan pcloud-rs mounts: entries present on the kernel mount
/// table but not listed in the daemon's known-mount set.
///
/// `known` is intended to be the mountpoint set the daemon remembers
/// (typically at most one, but a vector keeps the shape future-proof).
/// Comparison is by exact `PathBuf` equality.
pub fn detect_orphans(
    reader: &dyn MountinfoReader,
    known: &[PathBuf],
) -> io::Result<Vec<PcloudMountEntry>> {
    let payload = reader.read()?;
    let all = parse_pcloud_mounts(&payload);
    Ok(all
        .into_iter()
        .filter(|entry| {
            !known
                .iter()
                .any(|known_path| mount_paths_equal(known_path, &entry.mount_point))
        })
        .collect())
}

/// Returns `Some(fs_type)` when the given `mountpoint` is already
/// present in the kernel's mount table (not just for `fuse.pcloud*`
/// entries — we refuse to mount on top of *any* existing mount to
/// avoid shadowing state from another filesystem).
///
/// This parses the full `/proc/self/mountinfo` payload rather than
/// delegating to [`parse_pcloud_mounts`] because concurrent-mount
/// pre-checks must also reject non-pCloud mounts already occupying the
/// requested path (e.g. a stale `fuse.sshfs` the operator forgot to
/// unmount before launching `pcloudc mount`).
#[must_use]
pub fn mountpoint_is_already_mounted(
    reader: &dyn MountinfoReader,
    mountpoint: &Path,
) -> Option<String> {
    let payload = reader.read().ok()?;
    for line in payload.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(sep) = line.find(" - ") else {
            continue;
        };
        let (left, right) = (&line[..sep], &line[sep + 3..]);
        let left_fields: Vec<&str> = left.split_ascii_whitespace().collect();
        let right_fields: Vec<&str> = right.split_ascii_whitespace().collect();
        if left_fields.len() < 5 || right_fields.is_empty() {
            continue;
        }
        let mp = unescape_mountinfo(left_fields[4]);
        if mount_paths_equal(Path::new(&mp), mountpoint) {
            return Some(right_fields[0].to_owned());
        }
    }
    None
}

#[cfg(windows)]
fn mount_paths_equal(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

#[cfg(not(windows))]
fn mount_paths_equal(left: &Path, right: &Path) -> bool {
    left == right
}

/// Attempt to unmount `path` via the appropriate platform helper.
///
/// - **Linux**: shells out to `fusermount3 -u` (preferred) or `fusermount -u`.
///   The `fusermount` path is the libfuse-blessed release path and cleans up
///   auxiliary state (lock files, `/etc/mtab`-equivalent entries) that a raw
///   `umount2` leaves behind.
/// - **macOS / FreeBSD / DragonFlyBSD**: calls `unmount(2)` directly
///   through libc.
///
/// Returns `Ok(())` on success. The `timeout` parameter is honoured on Linux
/// (bounds the external command wait); on macOS/FreeBSD/DragonFlyBSD
/// `unmount(2)` is synchronous so the parameter is accepted but unused.
pub fn fusermount_unmount(path: &Path, timeout: Duration) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let candidates = ["fusermount3", "fusermount"];
        let mut last_err: Option<io::Error> = None;
        for bin in candidates {
            match spawn_and_wait(bin, path, timeout) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("no fusermount binary available")))
    }

    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "dragonfly"))]
    {
        let _ = timeout;
        use std::ffi::CString;
        let path_cstr = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|e| io::Error::other(format!("path contains NUL: {e}")))?;
        // SAFETY: `path_cstr` is a valid NUL-terminated C string. `umount(2)`
        // on these targets accepts flags = 0 for a normal unmount.
        let ret = unsafe { libc::unmount(path_cstr.as_ptr(), 0) };
        if ret == 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "dragonfly"
    )))]
    {
        let _ = (path, timeout);
        Err(io::Error::other("fusermount_unmount: unsupported platform"))
    }
}

#[cfg(target_os = "linux")]
fn spawn_and_wait(bin: &str, path: &Path, timeout: Duration) -> io::Result<()> {
    let mut child = Command::new(bin).arg("-u").arg(path).spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(io::Error::other(format!(
                    "{bin} -u {} exited with {status}",
                    path.display()
                )));
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("{bin} -u {} timed out", path.display()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn unescape_mountinfo(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let a = bytes[i + 1];
            let b = bytes[i + 2];
            let c = bytes[i + 3];
            if a.is_ascii_digit() && b.is_ascii_digit() && c.is_ascii_digit() {
                let v = ((a - b'0') * 64) + ((b - b'0') * 8) + (c - b'0');
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        "22 28 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:13 - proc proc rw\n",
        "23 28 0:22 / /sys rw,nosuid,nodev,noexec,relatime shared:14 - sysfs sysfs rw\n",
        "24 28 8:2 / /home rw,relatime shared:30 - ext4 /dev/sda2 rw\n",
        "25 28 0:44 / /home/user/official-pCloud rw,nosuid,nodev,relatime shared:77 - fuse.pcloud pcloud rw,user_id=1000,group_id=1000\n",
        "27 28 0:45 / /mnt/legacy-client rw,nosuid,nodev,relatime shared:78 - fuse.pclsync pclsync rw\n",
        "28 28 0:46 / /mnt/fork rw,nosuid,nodev,relatime shared:79 - fuse.pcloud-rs pcloud-rs rw\n",
        "29 28 0:99 / /home/user/otherfuse rw,nosuid,nodev,relatime shared:80 - fuse.sshfs sshfs rw\n",
        "30 28 0:55 / /mnt/with\\040space rw,relatime - fuse.pcloud-rs pcloud-rs rw\n",
        "malformed line without dash separator\n",
        "\n",
    );

    #[test]
    fn parse_pcloud_mounts_identifies_only_private_pcloud_rs_type() {
        let entries = parse_pcloud_mounts(FIXTURE);
        let points: Vec<&Path> = entries.iter().map(|e| e.mount_point.as_path()).collect();
        assert!(points.contains(&Path::new("/mnt/fork")));
        assert!(points.contains(&Path::new("/mnt/with space")));
        assert!(!points.contains(&Path::new("/home/user/official-pCloud")));
        assert!(!points.contains(&Path::new("/mnt/legacy-client")));
        assert!(!points.contains(&Path::new("/home/user/otherfuse")));
        assert!(!points.contains(&Path::new("/home")));
        assert!(!points.contains(&Path::new("/proc")));
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn parse_pcloud_mounts_skips_malformed_lines() {
        let entries = parse_pcloud_mounts("garbage\n\n   \n");
        assert!(entries.is_empty());
    }

    #[test]
    fn detect_orphans_filters_known_mounts() {
        let reader = StaticMountinfoReader::new(FIXTURE);
        let known = vec![PathBuf::from("/mnt/fork")];
        let orphans = detect_orphans(&reader, &known).expect("reader must succeed");
        let points: Vec<&Path> = orphans.iter().map(|e| e.mount_point.as_path()).collect();
        assert!(!points.contains(&Path::new("/mnt/fork")));
        assert!(points.contains(&Path::new("/mnt/with space")));
        assert_eq!(orphans.len(), 1);
    }

    #[test]
    fn detect_orphans_returns_all_when_nothing_known() {
        let reader = StaticMountinfoReader::new(FIXTURE);
        let orphans = detect_orphans(&reader, &[]).unwrap();
        assert_eq!(orphans.len(), 2);
    }

    #[test]
    fn mountpoint_is_already_mounted_finds_any_fs_type() {
        let reader = StaticMountinfoReader::new(FIXTURE);
        // pCloud type
        let ft = mountpoint_is_already_mounted(&reader, Path::new("/home/user/official-pCloud"))
            .expect("must detect pcloud mount");
        assert_eq!(ft, "fuse.pcloud");
        // Non-pCloud type (sshfs) must also be reported — operator
        // cannot layer on top of a foreign mount.
        let ft = mountpoint_is_already_mounted(&reader, Path::new("/home/user/otherfuse"))
            .expect("must detect foreign mount");
        assert_eq!(ft, "fuse.sshfs");
        // Space-escaped path round-trips through the parser.
        let ft = mountpoint_is_already_mounted(&reader, Path::new("/mnt/with space"))
            .expect("must detect escaped-space mount");
        assert_eq!(ft, "fuse.pcloud-rs");
        // Unknown path is absent.
        assert!(mountpoint_is_already_mounted(&reader, Path::new("/nowhere")).is_none());
    }

    #[test]
    fn detect_orphans_empty_when_no_pcloud_fs_present() {
        let payload = concat!(
            "24 28 8:2 / /home rw,relatime shared:30 - ext4 /dev/sda2 rw\n",
            "22 28 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:13 - proc proc rw\n",
        );
        let reader = StaticMountinfoReader::new(payload);
        let orphans = detect_orphans(&reader, &[]).unwrap();
        assert!(orphans.is_empty());
    }

    /// F-10 regression: the macOS/BSD getmntinfo reader historically emitted
    /// `fuse.pcloud` for every FUSE-type mount, including unrelated sshfs,
    /// macFUSE, and other FUSE volumes. This test verifies that a synthetic
    /// mountinfo payload containing non-pCloud FUSE entries does NOT yield
    /// any pCloud orphan entries, preventing false-positive force-unmounts.
    #[test]
    fn foreign_fuse_mounts_not_classified_as_pcloud_orphans() {
        // Simulate a corrected macOS/BSD reader: sshfs and generic fuse are
        // excluded; only genuine fuse.pcloud* entries appear.
        let payload = concat!(
            // sshfs — NOT a pCloud mount (fuse.sshfs)
            "0 0 0:0 / /home/user/remote - fuse.sshfs sshfs@192.168.1.1:/data rw\n",
            // generic fuse type — NOT pCloud
            "0 0 0:0 / /mnt/macfuse-vol - fuse macfuse-dev rw\n",
            // Genuine pCloud mount
            "0 0 0:0 / /home/user/pCloudDrive - fuse.pcloud-rs pcloud-rs rw\n",
        );
        let reader = StaticMountinfoReader::new(payload);
        let orphans = detect_orphans(&reader, &[]).unwrap();
        let points: Vec<&Path> = orphans.iter().map(|e| e.mount_point.as_path()).collect();
        assert_eq!(orphans.len(), 1, "expected 1 pCloud orphan, got {points:?}");
        assert!(points.contains(&Path::new("/home/user/pCloudDrive")));
        assert!(
            !points.contains(&Path::new("/home/user/remote")),
            "sshfs must not be classified as pCloud orphan"
        );
        assert!(
            !points.contains(&Path::new("/mnt/macfuse-vol")),
            "generic fuse must not be classified as pCloud orphan"
        );
    }
}

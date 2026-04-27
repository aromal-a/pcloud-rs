//! Mount-point and ignore-path auto-discovery for folder syncability checks.
//!
//! The C `psync_is_folder_syncable` implementation in
//! `pclsync/psynclib.c` consults two pieces of ambient system state that
//! the Rust `classify_folder_syncability` originally delegated to the
//! caller:
//!
//! 1. `pfs_getmountpoint()` — the current pCloud-drive mount point. This
//!    has to be discovered from the live kernel mount table so that a
//!    folder living on a FUSE-mounted pCloud drive is rejected even if
//!    the daemon is not the mount owner.
//! 2. `psync_setting_get_string(_PS(ignorepaths))` — a semicolon-delimited
//!    list of user-configured ignore paths.
//!
//! This module adds real Linux-only auto-discovery for both:
//!
//! * [`MountDiscovery`] parses `/proc/self/mountinfo` (the stable,
//!   per-namespace view of mounts) and returns the list of mount points
//!   whose filesystem type implies a pCloud drive, a virtual/pseudo
//!   filesystem, or any other filesystem that must never be adopted as a
//!   sync root. Results are cached with a TTL so that repeated syncability
//!   checks do not re-read the file for each call.
//! * [`default_ignore_patterns`] returns the hard-coded list of well-known
//!   system directories that the C client historically rejects through
//!   `ignorepaths` and that make no sense as sync roots (e.g. `/proc`,
//!   `/sys`, `/dev`, `/run`, snap/flatpak runtime bind mounts).
//!
//! All public types are `Send + Sync` and avoid interior-mutability
//! leaks: the TTL cache uses a plain `Mutex` around owned data.

// **PLATFORM:** Linux
// **GATING:** #[cfg(target_os = "linux")].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Virtual / pseudo filesystems that should never host a sync root.
///
/// These are matched against the `fstype` column of
/// `/proc/self/mountinfo`. They mirror the pCloud C client's implicit
/// assumption that sync roots live on real persistent filesystems only.
pub const VIRTUAL_FS_TYPES: &[&str] = &[
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "devpts",
    "devtmpfs",
    "mqueue",
    "hugetlbfs",
    "pstore",
    "bpf",
    "tracefs",
    "debugfs",
    "securityfs",
    "configfs",
    "fusectl",
    "autofs",
    "binfmt_misc",
    "rpc_pipefs",
    "nsfs",
    "ramfs",
];

/// Filesystem types that are likely a pCloud drive surface.
///
/// `fuse.pcloud` is what the legacy C client registers; `fuse.pclsync` is
/// the alternative name used by some forks. Any mount with one of these
/// types is treated as a pCloud-drive mount and rejected as a sync root
/// location.
pub const PCLOUD_FS_TYPES: &[&str] = &["fuse.pcloud", "fuse.pclsync", "fuse.pcloud-rs"];

/// Ignore-path prefixes installed by default on Linux.
///
/// The C client's shipped `ignorepaths` default plus conservative
/// additions for snap/flatpak/systemd runtime trees. A sync candidate
/// nested under any of these is rejected up front.
#[cfg(target_os = "linux")]
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/var/run",
    "/var/lock",
    "/tmp/.X11-unix",
    "/snap",
    "/var/lib/snapd",
    "/var/lib/flatpak",
    "/run/user",
    "/run/media",
    "/boot",
    "/lost+found",
];

/// Ignore-path prefixes installed by default on macOS.
///
/// Covers macOS system volumes, virtual filesystems, and paths that should
/// never be selected as sync roots. `/Volumes` contains all mounted drives
/// (including the pCloud FUSE mount itself); `/System` is SIP-protected.
/// `/private/tmp` and `/private/var/vm` are transient system paths.
/// Note: `/private/var/folders` (temp files) and user home dirs are NOT
/// blocked here — they are valid sync root candidates.
#[cfg(target_os = "macos")]
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    "/System",
    "/Volumes",
    "/private/tmp",
    "/private/var/db",
    "/private/var/vm",
    "/private/var/run",
    "/private/etc",
    "/dev",
    "/cores",
    "/Network",
    "/automount",
];

/// Ignore-path prefixes installed by default on platforms other than Linux and macOS.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[];

/// One entry parsed from `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Mount point within the current mount namespace.
    pub mount_point: PathBuf,
    /// Filesystem type, e.g. `ext4`, `fuse.pcloud`, `tmpfs`.
    pub fs_type: String,
}

impl MountEntry {
    /// Whether this mount is a pCloud drive surface.
    #[must_use]
    pub fn is_pcloud_drive(&self) -> bool {
        PCLOUD_FS_TYPES.iter().any(|ty| self.fs_type == *ty)
    }

    /// Whether this mount is a pseudo / virtual filesystem.
    #[must_use]
    pub fn is_virtual(&self) -> bool {
        VIRTUAL_FS_TYPES.iter().any(|ty| self.fs_type == *ty)
    }
}

/// Parse a `/proc/self/mountinfo` payload.
///
/// The format is documented in `proc(5)`:
///
/// ```text
/// 36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 - ext3 /dev/root rw,errors=continue
///  0  1   2    3     4     5           6     7   8        9            10
/// ```
///
/// Fields after field 6 are terminated by `" - "`; field 8 is the
/// filesystem type. Malformed lines are skipped rather than returned as
/// errors so the caller sees a best-effort view.
#[must_use]
pub fn parse_mountinfo(payload: &str) -> Vec<MountEntry> {
    let mut out = Vec::new();
    for line in payload.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Split at " - " separator between optional-fields and fstype.
        let Some(sep) = line.find(" - ") else {
            continue;
        };
        let (left, right) = (&line[..sep], &line[sep + 3..]);
        let left_fields: Vec<&str> = left.split_ascii_whitespace().collect();
        let right_fields: Vec<&str> = right.split_ascii_whitespace().collect();
        // Field 4 (zero-indexed) is the mount point; right field 0 is fstype.
        if left_fields.len() < 5 || right_fields.is_empty() {
            continue;
        }
        let raw_mount = unescape_mountinfo(left_fields[4]);
        let fs_type = right_fields[0].to_string();
        out.push(MountEntry {
            mount_point: PathBuf::from(raw_mount),
            fs_type,
        });
    }
    out
}

/// Decode the octal-escape sequences that `mountinfo` uses for whitespace
/// and backslashes (e.g. `\040` for space, `\011` for tab).
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

/// TTL-cached reader for the live mount table.
///
/// On non-Linux targets [`MountDiscovery::current_mounts`] returns an
/// empty list so the syncability check degrades gracefully.
#[derive(Debug)]
pub struct MountDiscovery {
    ttl: Duration,
    cache: Mutex<Option<CachedMounts>>,
}

#[derive(Debug, Clone)]
struct CachedMounts {
    fetched_at: Instant,
    entries: Vec<MountEntry>,
}

impl Default for MountDiscovery {
    fn default() -> Self {
        // 5 seconds is enough to coalesce bursts of syncability checks
        // (e.g. CLI batch add) while still reflecting new mounts
        // quickly during interactive use.
        Self::with_ttl(Duration::from_secs(5))
    }
}

impl MountDiscovery {
    /// Build a [`MountDiscovery`] with a custom TTL.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            cache: Mutex::new(None),
        }
    }

    /// Return the cached mount list, refreshing if the TTL has elapsed.
    #[must_use]
    pub fn current_mounts(&self) -> Vec<MountEntry> {
        let now = Instant::now();
        if let Ok(guard) = self.cache.lock() {
            if let Some(ref cached) = *guard {
                if now.duration_since(cached.fetched_at) < self.ttl {
                    return cached.entries.clone();
                }
            }
        }
        let entries = read_mountinfo().unwrap_or_default();
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(CachedMounts {
                fetched_at: now,
                entries: entries.clone(),
            });
        }
        entries
    }

    /// Return the mount points identified as pCloud drive surfaces.
    #[must_use]
    pub fn pcloud_mount_points(&self) -> Vec<PathBuf> {
        self.current_mounts()
            .into_iter()
            .filter(MountEntry::is_pcloud_drive)
            .map(|e| e.mount_point)
            .collect()
    }

    /// Return virtual / pseudo mount points that are never syncable.
    #[must_use]
    pub fn virtual_mount_points(&self) -> Vec<PathBuf> {
        self.current_mounts()
            .into_iter()
            .filter(MountEntry::is_virtual)
            .map(|e| e.mount_point)
            .collect()
    }

    /// Force the next call to re-read `/proc/self/mountinfo`.
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = None;
        }
    }
}

#[cfg(target_os = "linux")]
fn read_mountinfo() -> io::Result<Vec<MountEntry>> {
    let payload = std::fs::read_to_string("/proc/self/mountinfo")?;
    Ok(parse_mountinfo(&payload))
}

#[cfg(not(target_os = "linux"))]
fn read_mountinfo() -> io::Result<Vec<MountEntry>> {
    Ok(Vec::new())
}

/// Default ignore pattern list, owned and with trailing slashes stripped.
#[must_use]
pub fn default_ignore_patterns() -> Vec<String> {
    DEFAULT_IGNORE_PATTERNS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Match `candidate` against an ignore-prefix entry using the same
/// semantics as the C `psyncer_str_starts_with`: equal, or
/// `candidate[len]` is a path separator.
///
/// Accepts both `/` and `\` as separators so Windows paths (where
/// `std::fs::canonicalize` returns `\\?\C:\...\name` with backslashes)
/// match correctly against both Unix-style and Windows-style prefixes.
#[must_use]
pub fn is_ignored_under(candidate: &Path, prefix: &str) -> bool {
    let cand = candidate.as_os_str().to_string_lossy();
    let trimmed = prefix.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return false;
    }
    if cand == trimmed {
        return true;
    }
    if let Some(rest) = cand.strip_prefix(trimmed) {
        rest.starts_with('/') || rest.starts_with('\\')
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        "22 28 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:13 - proc proc rw\n",
        "23 28 0:22 / /sys rw,nosuid,nodev,noexec,relatime shared:14 - sysfs sysfs rw\n",
        "24 28 8:2 / /home rw,relatime shared:30 - ext4 /dev/sda2 rw\n",
        "25 28 0:44 / /home/user/pCloudDrive rw,nosuid,nodev,relatime shared:77 - fuse.pcloud pcloud rw,user_id=1000,group_id=1000\n",
        "26 28 0:55 / /mnt/with\\040space rw,relatime - ext4 /dev/sdb1 rw\n",
        "malformed line without dash separator\n",
        "\n",
    );

    #[test]
    fn parse_mountinfo_extracts_mounts_and_types() {
        let entries = parse_mountinfo(FIXTURE);
        let types: Vec<&str> = entries.iter().map(|e| e.fs_type.as_str()).collect();
        assert!(types.contains(&"proc"));
        assert!(types.contains(&"sysfs"));
        assert!(types.contains(&"ext4"));
        assert!(types.contains(&"fuse.pcloud"));
        // malformed line is skipped, so count == 5
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn parse_mountinfo_unescapes_whitespace() {
        let entries = parse_mountinfo(FIXTURE);
        let has_space = entries
            .iter()
            .any(|e| e.mount_point == Path::new("/mnt/with space"));
        assert!(has_space, "expected decoded space in mount point");
    }

    #[test]
    fn classifies_pcloud_and_virtual_mounts() {
        let entries = parse_mountinfo(FIXTURE);
        let pcloud: Vec<_> = entries.iter().filter(|e| e.is_pcloud_drive()).collect();
        let virt: Vec<_> = entries.iter().filter(|e| e.is_virtual()).collect();
        assert_eq!(pcloud.len(), 1);
        assert_eq!(
            pcloud[0].mount_point,
            PathBuf::from("/home/user/pCloudDrive")
        );
        assert_eq!(virt.len(), 2);
    }

    #[test]
    fn ignored_matcher_matches_exact_and_nested() {
        assert!(is_ignored_under(Path::new("/proc"), "/proc"));
        assert!(is_ignored_under(Path::new("/proc/1"), "/proc"));
        assert!(is_ignored_under(Path::new("/proc/1"), "/proc/"));
        assert!(!is_ignored_under(Path::new("/procurement"), "/proc"));
        assert!(!is_ignored_under(Path::new("/home"), "/proc"));
        assert!(!is_ignored_under(Path::new("/"), ""));
    }

    #[test]
    fn ttl_cache_returns_stable_snapshot() {
        let mnt = MountDiscovery::with_ttl(Duration::from_secs(60));
        let a = mnt.current_mounts();
        let b = mnt.current_mounts();
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn invalidate_forces_refresh() {
        let mnt = MountDiscovery::with_ttl(Duration::from_secs(60));
        let _ = mnt.current_mounts();
        mnt.invalidate();
        // Should not panic and should return a (possibly empty) list.
        let _ = mnt.current_mounts();
    }

    #[test]
    fn default_ignore_patterns_contain_system_dirs() {
        let patterns = default_ignore_patterns();
        // Platform-specific expected entries.
        #[cfg(target_os = "linux")]
        let expected_entries = ["/proc", "/sys", "/dev", "/run", "/snap"];
        #[cfg(target_os = "macos")]
        let expected_entries = ["/System", "/Volumes", "/dev", "/cores", "/automount"];
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let expected_entries: [&str; 0] = [];
        for expected in expected_entries {
            assert!(
                patterns.iter().any(|p| p == expected),
                "missing default ignore {expected}"
            );
        }
    }
}

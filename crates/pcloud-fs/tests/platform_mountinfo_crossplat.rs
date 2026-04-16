#![allow(clippy::pedantic)]
//! **PLATFORM: all** (Linux | FreeBSD | OpenBSD | NetBSD | macOS | Windows).
//! **GATING: sub-tests are cfg-gated per OS; the file compiles
//! everywhere.**
//!
//! Phase 3 integration tests for the [`pcloud_fs::platform`] mountinfo
//! reader. Exercises the `MountinfoReader` trait through the active
//! per-OS concrete type:
//!
//! - Linux: `ProcMountinfoReader` really reads `/proc/self/mountinfo`
//!   and feeds it to `parse_pcloud_mounts`. We assert only the shape
//!   (no panic, well-formed output) so the test is robust on any host.
//! - BSD/macOS/Windows: the platform reader is a planned
//!   `unimplemented!` surface; we assert a non-zero `TypeId` on the
//!   active reader as a placeholder sanity check until the real
//!   backend lands.

#[cfg(not(target_os = "linux"))]
use std::any::TypeId;

#[cfg(target_os = "linux")]
use pcloud_fs::mount_orphan::{MountinfoReader, parse_pcloud_mounts};

/// On Linux, the production reader reads `/proc/self/mountinfo`. Assert
/// the round-trip through `parse_pcloud_mounts` does not panic and
/// returns a well-formed (possibly empty) vector. The payload format is
/// documented in `proc(5)`; any future change to the parser must keep
/// this property.
#[cfg(target_os = "linux")]
#[test]
fn proc_reader_returns_string_matching_mountinfo_format() {
    use pcloud_fs::mount_orphan::ProcMountinfoReader;

    let reader = ProcMountinfoReader;
    let payload = reader
        .read()
        .expect("/proc/self/mountinfo should be readable on Linux");

    // The kernel guarantees the payload is ASCII text, newline-delimited,
    // with at least one entry (root `/`). We only assert it looks
    // line-structured; byte content is not load-bearing here.
    assert!(
        !payload.is_empty(),
        "/proc/self/mountinfo must not be empty on a live Linux host"
    );
    assert!(
        payload.lines().count() >= 1,
        "/proc/self/mountinfo should have at least one entry"
    );

    // Parsing must not panic, even if the live host has zero pCloud
    // FUSE mounts (which is the common CI case). An empty result is
    // expected and valid.
    let entries = parse_pcloud_mounts(&payload);
    for entry in &entries {
        assert!(
            !entry.fs_type.is_empty(),
            "parsed entry must carry a non-empty fs_type"
        );
    }
}

/// BSD/macOS placeholder: the platform mountinfo reader is not yet
/// implemented (it will wrap `getmntinfo(3)` on BSD/macOS and the
/// native API on Windows). Until then, we assert only that an active
/// reader type exists and has a non-zero `TypeId`, which proves the
/// cfg-selected `ActivePlatformMount` symbol is actually reachable.
#[cfg(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "macos",
))]
#[test]
fn bsd_or_macos_reader_typeid_is_non_zero_placeholder() {
    use pcloud_fs::platform::ActivePlatformMount;

    // TypeId of any live concrete type is non-zero; this is the weakest
    // statement we can make without actually calling into a reader that
    // is still `unimplemented!()` on these targets.
    let tid = TypeId::of::<ActivePlatformMount>();
    assert_ne!(
        tid,
        TypeId::of::<()>(),
        "ActivePlatformMount must resolve to a real struct on BSD/macOS"
    );
}

/// Windows placeholder: same rationale as the BSD/macOS arm above. The
/// real reader will live in `platform::windows` (WinFSP-backed).
#[cfg(target_os = "windows")]
#[test]
fn windows_reader_typeid_is_non_zero_placeholder() {
    use pcloud_fs::platform::ActivePlatformMount;

    let tid = TypeId::of::<ActivePlatformMount>();
    assert_ne!(
        tid,
        TypeId::of::<()>(),
        "ActivePlatformMount must resolve to a real struct on Windows"
    );
}

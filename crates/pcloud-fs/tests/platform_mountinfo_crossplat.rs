#![allow(clippy::pedantic)]
//! **PLATFORM: all** (Linux | BSD | macOS | Windows).
//! **GATING: sub-tests are cfg-gated per OS; the file compiles
//! everywhere.**
//!
//! Native integration tests for the [`pcloud_fs::platform`] mountinfo
//! reader. Exercises the `MountinfoReader` trait through the active
//! per-OS concrete type:
//!
//! - Linux: `ProcMountinfoReader` really reads `/proc/self/mountinfo`
//!   and feeds it to `parse_pcloud_mounts`. We assert only the shape
//!   (no panic, well-formed output) so the test is robust on any host.
//! - BSD/macOS: call the production `getmntinfo(3)` reader.
//! - Windows: call the production Win32 volume enumerator.
//!
//! A host normally has no pCloud mount, so the portable assertion is that
//! native enumeration succeeds and any returned entries parse as private
//! pCloud filesystem types.

use pcloud_fs::mount_orphan::{MountinfoReader, parse_pcloud_mounts};

fn assert_pcloud_payload_is_well_formed(payload: &str) {
    for entry in parse_pcloud_mounts(payload) {
        assert!(!entry.mount_point.as_os_str().is_empty());
        assert!(
            entry.fs_type == "fuse.pcloud-rs",
            "native reader returned an unowned filesystem type: {}",
            entry.fs_type
        );
    }
}

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
    assert_pcloud_payload_is_well_formed(&payload);
}

/// BSD reader calls `getmntinfo(3)` and filters foreign FUSE volumes.
#[cfg(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "netbsd"
))]
#[test]
fn bsd_reader_enumerates_native_mount_table() {
    use pcloud_fs::platform::bsd::BsdMountinfoReader;

    let payload = BsdMountinfoReader
        .read()
        .expect("getmntinfo(3) must enumerate the BSD mount table");
    assert_pcloud_payload_is_well_formed(&payload);
}

/// macOS reader calls `getmntinfo(3)` and filters foreign FUSE volumes.
#[cfg(target_os = "macos")]
#[test]
fn macos_reader_enumerates_native_mount_table() {
    use pcloud_fs::platform::macos::MacosMountinfoReader;

    let payload = MacosMountinfoReader
        .read()
        .expect("getmntinfo(3) must enumerate the macOS mount table");
    assert_pcloud_payload_is_well_formed(&payload);
}

/// Windows reader enumerates every volume and selects only the private
/// `pcloud-rs` filesystem marker.
#[cfg(target_os = "windows")]
#[test]
fn windows_reader_enumerates_native_volume_table() {
    use pcloud_fs::platform::windows::WindowsMountinfoReader;

    let payload = WindowsMountinfoReader
        .read()
        .expect("Win32 must enumerate the native volume table");
    assert_pcloud_payload_is_well_formed(&payload);
}

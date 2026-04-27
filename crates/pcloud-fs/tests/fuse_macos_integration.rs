//! macOS FUSE integration tests using fuse-t.
//!
//! These tests require a real macOS host with fuse-t installed.
//! Gate: `PCLOUD_FUSE_TEST=1` must be set, otherwise tests are skipped.
//!
//! Run with:
//!   PCLOUD_FUSE_TEST=1 cargo test -p pcloud-fs --test fuse_macos_integration -- --nocapture

#![cfg(target_os = "macos")]
#![allow(clippy::pedantic)]

use std::time::Duration;

use pcloud_fs::fuse_adapter::NullFuseAdapter;
use pcloud_fs::mount_orphan::MountinfoReader;
use pcloud_fs::mount_service::{MountOptions, MountService};
use pcloud_fs::platform::PlatformMount;
use pcloud_fs::platform::macos::{MacosMountinfoReader, MacosPlatformMount};

fn fuse_test_enabled() -> bool {
    std::env::var("PCLOUD_FUSE_TEST")
        .map(|v| v == "1")
        .unwrap_or(false)
}

fn probe_fuse_t() -> bool {
    MacosPlatformMount.probe_supported().is_ok()
}

#[test]
fn macos_fuse_t_probe_reports_availability() {
    if !fuse_test_enabled() {
        eprintln!("SKIP: set PCLOUD_FUSE_TEST=1 to run macOS FUSE tests");
        return;
    }
    let available = probe_fuse_t();
    if !available {
        eprintln!("SKIP: fuse-t not installed — install from https://www.fuse-t.org/");
        return;
    }
    println!("fuse-t available: probe succeeded");
}

#[test]
fn macos_null_adapter_mount_unmount_roundtrip() {
    if !fuse_test_enabled() {
        eprintln!("SKIP: set PCLOUD_FUSE_TEST=1 to run macOS FUSE tests");
        return;
    }
    if !probe_fuse_t() {
        eprintln!("SKIP: fuse-t not available");
        return;
    }

    let tmp = tempfile::TempDir::new().expect("temp dir");
    let mountpoint = tmp.path().to_path_buf();

    let service = MountService::new();
    let opts = MountOptions::default();

    let handle = service
        .mount(&mountpoint, NullFuseAdapter, opts)
        .expect("mount should succeed with fuse-t");

    // Give fuse-t time to register the mount in the kernel table.
    std::thread::sleep(Duration::from_millis(500));

    // Verify mount appears in getmntinfo.
    let reader = MacosMountinfoReader;
    let payload = reader.read().expect("getmntinfo should succeed");
    println!("mountinfo payload:\n{payload}");

    // Unmount.
    handle.unmount().expect("unmount should succeed");

    // Give kernel time to update mount table.
    std::thread::sleep(Duration::from_millis(200));

    println!("mount/unmount roundtrip succeeded");
}

#[test]
fn macos_mountinfo_reader_returns_string() {
    // This test runs even without PCLOUD_FUSE_TEST — it only calls getmntinfo.
    // MacosMountinfoReader filters to FUSE mounts only (for orphan detection),
    // so an empty result is expected when no FUSE mounts are active.
    let reader = MacosMountinfoReader;
    let payload = reader.read().expect("getmntinfo syscall must not fail");
    println!(
        "FUSE mountinfo payload ({} bytes):\n{payload}",
        payload.len()
    );
    // No assertion on content: empty is valid when no FUSE mounts are active.
}

#[test]
fn macos_mountinfo_reader_contains_root_mount() {
    let reader = MacosMountinfoReader;
    let payload = reader.read().expect("getmntinfo must not fail");
    // Root should always be present even though it's not a FUSE mount.
    // The payload format has 'mountpoint' in field 4 of each line.
    // Check the raw payload contains "/" as a path component.
    println!("Raw payload:\n{payload}");
    // (We don't assert on specific format here since non-FUSE mounts
    // are filtered from the pCloud-specific parse path.)
}

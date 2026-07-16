#![allow(clippy::pedantic)]
//! **PLATFORM: macOS only.**
//! **GATING: `#[cfg(target_os = "macos")]`** — entire file.
//!
//! Integration tests for the macOS FUSE platform layer using public APIs:
//!
//! - `MountService::validate_mountpoint` cross-platform validation on macOS
//! - `MacosPlatformMount` public-API surface
//! - `MacosMountinfoReader` — format contract + orphan-detection integration
//! - `fusermount_unmount` on macOS — error path on an unmounted directory
//! - `parse_pcloud_mounts` with synthetically-formatted macOS payload
//!
//! Tests that require a real fuse-t install or a real kernel mount are in
//! `macos_mount_live.rs` and are gated behind `PCLOUD_FUSE_TEST=1`.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use pcloud_fs::mount_orphan::{
    MountinfoReader, StaticMountinfoReader, detect_orphans, fusermount_unmount,
    mountpoint_is_already_mounted, parse_pcloud_mounts,
};
use pcloud_fs::mount_service::{MountError, MountOptions, MountService};
use pcloud_fs::platform::macos::MacosMountinfoReader;
use pcloud_fs::platform::{ActivePlatformMount, PlatformMount};

// =============================================================================
// MountService::validate_mountpoint — macOS
// =============================================================================

#[test]
fn validate_mountpoint_missing_returns_error() {
    let result = MountService::validate_mountpoint(Path::new("/nonexistent/path/abc123"));
    assert!(result.is_err());
    match result.unwrap_err() {
        MountError::MountpointMissing(_) => {}
        other => panic!("expected MountpointMissing, got {other:?}"),
    }
}

#[test]
fn validate_mountpoint_file_returns_not_directory_error() {
    let tmp = tempfile::NamedTempFile::new().expect("named temp file");
    let result = MountService::validate_mountpoint(tmp.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        MountError::MountpointNotDirectory(_) => {}
        other => panic!("expected MountpointNotDirectory, got {other:?}"),
    }
}

#[test]
fn validate_mountpoint_non_empty_directory_returns_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Write a file into it so it's non-empty.
    std::fs::write(dir.path().join("file.txt"), b"x").expect("write");
    let result = MountService::validate_mountpoint(dir.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        MountError::MountpointNotEmpty(_) => {}
        other => panic!("expected MountpointNotEmpty, got {other:?}"),
    }
}

#[test]
fn validate_mountpoint_empty_directory_is_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        MountService::validate_mountpoint(dir.path()).is_ok(),
        "empty directory must be a valid mountpoint on macOS"
    );
}

// =============================================================================
// ActivePlatformMount == MacosPlatformMount on macOS
// =============================================================================

#[test]
fn active_platform_mount_is_macos_type() {
    use pcloud_fs::platform::macos::MacosPlatformMount;
    use std::any::TypeId;
    assert_eq!(
        TypeId::of::<ActivePlatformMount>(),
        TypeId::of::<MacosPlatformMount>(),
        "ActivePlatformMount must be MacosPlatformMount on macOS"
    );
}

// =============================================================================
// MacosPlatformMount::validate_mountpoint
// =============================================================================

#[test]
fn platform_validate_mountpoint_missing() {
    let mount = ActivePlatformMount::default();
    let err = mount
        .validate_mountpoint(Path::new("/nosuchpath/pcloud-xyz"))
        .unwrap_err();
    assert!(
        matches!(err, MountError::MountpointMissing(_)),
        "missing path must return MountpointMissing: {err:?}"
    );
}

#[test]
fn platform_validate_mountpoint_not_dir() {
    let file = tempfile::NamedTempFile::new().expect("named temp file");
    let mount = ActivePlatformMount::default();
    let err = mount.validate_mountpoint(file.path()).unwrap_err();
    assert!(
        matches!(err, MountError::MountpointNotDirectory(_)),
        "file path must return MountpointNotDirectory: {err:?}"
    );
}

#[test]
fn platform_validate_mountpoint_empty_dir_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mount = ActivePlatformMount::default();
    assert!(
        mount.validate_mountpoint(dir.path()).is_ok(),
        "empty directory must pass platform validation"
    );
}

// =============================================================================
// MacosPlatformMount::default_options
// =============================================================================

#[test]
fn default_options_fs_name_is_pcloud() {
    let mount = ActivePlatformMount::default();
    let opts = mount.default_options();
    assert_eq!(
        opts.fs_name.as_deref(),
        Some("pCloud"),
        "macOS default fs_name must be 'pCloud'"
    );
}

#[test]
fn default_options_read_only_flag_is_default() {
    let mount = ActivePlatformMount::default();
    let opts = mount.default_options();
    // The default read_only state is inherited from MountOptions::default.
    // Just confirm we get a well-formed struct back (no panic).
    let _read_only: bool = opts.read_only;
}

// =============================================================================
// MacosPlatformMount::probe_supported
// =============================================================================

#[test]
fn probe_supported_never_panics() {
    let mount = ActivePlatformMount::default();
    // Must return either Ok or a non-panic Err.
    let _ = mount.probe_supported();
}

#[test]
fn probe_supported_returns_ok_or_unsupported_with_nonempty_hint() {
    let mount = ActivePlatformMount::default();
    match mount.probe_supported() {
        Ok(()) => {}
        Err(MountError::Unsupported(hint)) => {
            assert!(!hint.is_empty(), "Unsupported hint must not be empty");
            assert!(
                hint.contains("fuse-t.org") || hint.contains("macfuse.github.io"),
                "hint must point to an install URL, got: {hint}"
            );
        }
        Err(other) => panic!("probe_supported must not return {other:?}"),
    }
}

// =============================================================================
// MacosMountinfoReader
// =============================================================================

#[test]
fn macos_mountinfo_reader_read_does_not_panic() {
    let reader = MacosMountinfoReader;
    let _ = reader.read();
}

#[test]
fn macos_mountinfo_reader_output_parses_without_panic() {
    let reader = MacosMountinfoReader;
    if let Ok(payload) = reader.read() {
        let entries = parse_pcloud_mounts(&payload);
        for entry in &entries {
            assert!(
                !entry.fs_type.is_empty(),
                "every parsed entry must have a non-empty fs_type"
            );
        }
    }
}

#[test]
fn macos_mountinfo_reader_entries_have_absolute_mount_points() {
    let reader = MacosMountinfoReader;
    if let Ok(payload) = reader.read() {
        let entries = parse_pcloud_mounts(&payload);
        for entry in &entries {
            assert!(
                entry.mount_point.is_absolute(),
                "mount point must be absolute: {:?}",
                entry.mount_point
            );
        }
    }
}

#[test]
fn macos_mountinfo_reader_entries_have_pcloud_fstype() {
    let reader = MacosMountinfoReader;
    if let Ok(payload) = reader.read() {
        let entries = parse_pcloud_mounts(&payload);
        for entry in &entries {
            assert!(
                entry.fs_type == "fuse.pcloud-rs",
                "every entry from MacosMountinfoReader must have the private fuse.pcloud-rs fstype, got: {}",
                entry.fs_type
            );
        }
    }
}

// =============================================================================
// detect_orphans via StaticMountinfoReader (simulates macOS-formatted payload)
// =============================================================================

const MACOS_STYLE_FIXTURE: &str = concat!(
    "0 0 0:0 / /Volumes/pCloud\\040Drive - fuse.pcloud-rs pcloud-rs rw\n",
    "0 0 0:0 / /Volumes/pCloudOld - fuse.pcloud-rs pcloud-rs rw\n",
);

#[test]
fn detect_orphans_identifies_unknown_macos_mounts() {
    let reader = StaticMountinfoReader::new(MACOS_STYLE_FIXTURE);
    let known: Vec<PathBuf> = vec![PathBuf::from("/Volumes/pCloud Drive")];
    let orphans = detect_orphans(&reader, &known).expect("detect_orphans must not error");

    assert_eq!(orphans.len(), 1, "only one mount should be an orphan");
    assert_eq!(
        orphans[0].mount_point,
        PathBuf::from("/Volumes/pCloudOld"),
        "orphan must be the unknown mount"
    );
}

#[test]
fn detect_orphans_returns_all_when_nothing_known() {
    let reader = StaticMountinfoReader::new(MACOS_STYLE_FIXTURE);
    let orphans = detect_orphans(&reader, &[]).expect("detect_orphans must not error");
    assert_eq!(
        orphans.len(),
        2,
        "all mounts must be orphans when nothing known"
    );
}

#[test]
fn detect_orphans_empty_when_all_known() {
    let reader = StaticMountinfoReader::new(MACOS_STYLE_FIXTURE);
    let known = vec![
        PathBuf::from("/Volumes/pCloud Drive"),
        PathBuf::from("/Volumes/pCloudOld"),
    ];
    let orphans = detect_orphans(&reader, &known).expect("detect_orphans must not error");
    assert!(orphans.is_empty(), "no orphans when all mounts are known");
}

#[test]
fn detect_orphans_empty_reader_returns_empty() {
    let reader = StaticMountinfoReader::new("");
    let orphans = detect_orphans(&reader, &[]).expect("detect_orphans must not error");
    assert!(orphans.is_empty(), "empty reader must produce no orphans");
}

// =============================================================================
// mountpoint_is_already_mounted via StaticMountinfoReader
// =============================================================================

#[test]
fn mountpoint_is_already_mounted_finds_macos_fuse_mount() {
    let reader = StaticMountinfoReader::new(MACOS_STYLE_FIXTURE);
    let ft = mountpoint_is_already_mounted(&reader, Path::new("/Volumes/pCloud Drive"))
        .expect("pCloud Drive must be detected as mounted");
    assert_eq!(ft, "fuse.pcloud-rs", "fstype must be fuse.pcloud-rs");
}

#[test]
fn mountpoint_is_already_mounted_not_found_returns_none() {
    let reader = StaticMountinfoReader::new(MACOS_STYLE_FIXTURE);
    let result = mountpoint_is_already_mounted(&reader, Path::new("/Volumes/NonExistent"));
    assert!(result.is_none(), "non-mounted path must return None");
}

#[test]
fn mountpoint_is_already_mounted_handles_space_in_path() {
    let payload = "0 0 0:0 / /Volumes/my\\040cloud - fuse.pcloud-rs pcloud-rs rw\n";
    let reader = StaticMountinfoReader::new(payload);
    let ft = mountpoint_is_already_mounted(&reader, Path::new("/Volumes/my cloud"))
        .expect("space-escaped path must be detected");
    assert_eq!(ft, "fuse.pcloud-rs");
}

// =============================================================================
// fusermount_unmount on macOS — error path
// =============================================================================

#[test]
fn fusermount_unmount_on_unmounted_dir_returns_err() {
    let dir = tempfile::tempdir().expect("tempdir");
    // This must not panic. It will return an IO error because nothing is mounted.
    let result = fusermount_unmount(dir.path(), Duration::from_secs(5));
    assert!(
        result.is_err(),
        "unmounting an unmounted dir must return an error"
    );
}

#[test]
fn fusermount_unmount_on_nonexistent_path_returns_err() {
    let result = fusermount_unmount(Path::new("/nonexistent/path/xyz"), Duration::from_secs(5));
    assert!(
        result.is_err(),
        "unmounting a nonexistent path must return an error"
    );
}

// =============================================================================
// MountOptions default shape on macOS
// =============================================================================

#[test]
fn mount_options_default_shape() {
    let opts = MountOptions::default();
    assert!(opts.read_only, "default MountOptions must be read-only");
    assert!(
        !opts.allow_other,
        "default MountOptions must not allow_other"
    );
    assert!(
        opts.fs_name.is_none(),
        "default MountOptions must have no fs_name"
    );
}

#[test]
fn mount_service_rejects_allow_other_on_macos() {
    let svc = MountService::new();
    let dir = tempfile::tempdir().expect("tempdir");
    // We use a NullFuseAdapter so we can test the allow_other rejection
    // without needing a real fuse-t install.
    use pcloud_fs::fuse_adapter::NullFuseAdapter;
    let result = svc.mount(
        dir.path(),
        NullFuseAdapter,
        MountOptions {
            allow_other: true,
            ..MountOptions::default()
        },
    );
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), MountError::AllowOtherRejected),
        "allow_other must be rejected by MountService on macOS"
    );
}

// =============================================================================
// parse_pcloud_mounts with macOS-formatted synthetic payload
// =============================================================================

#[test]
fn parse_pcloud_mounts_on_macos_formatted_lines() {
    let payload = concat!(
        "0 0 0:0 / /Volumes/pCloudRs - fuse.pcloud-rs pcloud-rs rw\n",
        "0 0 0:0 / /Volumes/OfficialPCloud - fuse.pcloud pcloud rw\n",
        "0 0 0:0 / /Volumes/other - ext4 /dev/disk1 rw\n",
    );
    let entries = parse_pcloud_mounts(payload);
    assert_eq!(entries.len(), 1, "only the private pcloud-rs type is owned");
    let paths: Vec<&std::path::Path> = entries.iter().map(|e| e.mount_point.as_path()).collect();
    assert!(paths.contains(&Path::new("/Volumes/pCloudRs")));
    assert!(!paths.contains(&Path::new("/Volumes/OfficialPCloud")));
}

#[test]
fn parse_pcloud_mounts_handles_spaces_in_macos_volume_names() {
    let payload = "0 0 0:0 / /Volumes/My\\040pCloud - fuse.pcloud-rs pcloud-rs rw\n";
    let entries = parse_pcloud_mounts(payload);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].mount_point, PathBuf::from("/Volumes/My pCloud"));
}

#[test]
fn parse_pcloud_mounts_skips_non_pcloud_macos_mounts() {
    let payload = concat!(
        "0 0 0:0 / /Volumes/Macintosh\\040HD - hfs /dev/disk0s2 rw\n",
        "0 0 0:0 / /private/var/folders - tmpfs tmpfs rw\n",
    );
    let entries = parse_pcloud_mounts(payload);
    assert!(entries.is_empty(), "non-pCloud mounts must be ignored");
}

// =============================================================================
// MountService::mount — probe path (graceful degradation without real mount)
// =============================================================================

#[test]
fn mount_service_probe_or_unsupported_never_panics() {
    // Verify that probe_supported either returns Ok (fuse-t is installed)
    // or Unsupported with a non-empty hint. Does NOT perform an actual
    // kernel mount so it cannot hang regardless of whether fuse-t is installed.
    use pcloud_fs::platform::PlatformMount;
    use pcloud_fs::platform::macos::MacosPlatformMount;

    match MacosPlatformMount.probe_supported() {
        Ok(()) => {}
        Err(MountError::Unsupported(hint)) => {
            assert!(
                !hint.is_empty(),
                "Unsupported hint must not be empty: {hint}"
            );
        }
        Err(other) => panic!("probe_supported must not return {other:?}"),
    }
}

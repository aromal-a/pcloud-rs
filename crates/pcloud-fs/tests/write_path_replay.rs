#![allow(clippy::pedantic)]
//! bd-1du.4.d integration test: write-path durability across remount.
//!
//! Simulates a mount-write-crash-remount sequence and verifies the journal
//! replay surfaces the pending mutations so the daemon can re-drive them.
//!
//! A real kernel FUSE mount is owned by the 4.e sub-bead. To keep 4.d
//! decoupled from mount lifecycle, this test exercises the durability
//! contract directly: a "mount" is represented by a `WritePathService`
//! instance; an "unmount" is a drop; a "remount" is re-opening the same
//! staging dir + journal from disk.

// **PLATFORM:** Linux
// **GATING:** #[cfg(target_os = "linux")].

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::{JournalOp, WriteJournal, replay_path};
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};

/// Minimal upload backend used by the integration test. Intentionally
/// always fails the upload call so that the on-disk journal becomes the
/// only record of durability at the "unmount" boundary.
struct AlwaysFailingBackend;

impl FileUploadBackend for AlwaysFailingBackend {
    fn upload_file(
        &self,
        _parent_path: &str,
        _name: &str,
        _staging_file: &Path,
    ) -> Result<(), WritePathError> {
        Err(WritePathError::Upload("injected failure".to_owned()))
    }
    fn unlink_remote(&self, _path: &str) -> Result<(), WritePathError> {
        Ok(())
    }
    fn rename_remote(&self, _from: &str, _to: &str) -> Result<(), WritePathError> {
        Ok(())
    }
}

#[test]
fn mount_write_fsync_unmount_remount_preserves_pending_records() {
    let tmp = tempfile::tempdir().unwrap();
    let stage_root = tmp.path().join("stage");

    // ----- Mount #1: create + write + fsync -----
    {
        let stage = StagingDir::open(&stage_root).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(AlwaysFailingBackend);
        let svc = WritePathService::new(
            stage,
            journal,
            backend,
            WritePathOptions {
                flush_threshold_bytes: 1024 * 1024,
                flush_interval: Duration::from_secs(3600),
            },
        );
        svc.create(42, "/", "doc.txt").unwrap();
        svc.write(42, 0, b"remount-survives-this").unwrap();
        // fsync fails because the rigged backend rejects upload; journal
        // entries before the failing upload must still be durable.
        let _ = svc.fsync(42);
        // Drop svc: "unmount".
    }

    // ----- "Remount": re-open journal from disk -----
    let replayed = replay_path(stage_root.join("journal.log")).unwrap();
    let has_create = replayed.iter().any(|r| {
        matches!(&r.op,
            JournalOp::Create { parent_path, name } if parent_path == "/" && name == "doc.txt")
    });
    let has_write = replayed.iter().any(|r| {
        matches!(&r.op,
            JournalOp::Write { path, len, .. } if path == "/doc.txt" && *len == 21)
    });
    let has_barrier = replayed
        .iter()
        .any(|r| matches!(&r.op, JournalOp::FlushBarrier { path } if path == "/doc.txt"));
    assert!(has_create, "Create must survive remount: {replayed:#?}");
    assert!(has_write, "Write must survive remount: {replayed:#?}");
    assert!(
        has_barrier,
        "FlushBarrier must survive remount: {replayed:#?}"
    );

    // Staging blob must also be present so the daemon can re-drive the
    // upload against a healthy backend on remount.
    let stage = StagingDir::open(&stage_root).unwrap();
    let blob = stage.read_blob("ino-42.blob").unwrap();
    assert_eq!(blob, b"remount-survives-this");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1; real FUSE mount integration is owned by 4.e"]
fn write_path_via_real_mount() {
    if std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() != Some("1") {
        return;
    }
    // Real FUSE-mount write integration belongs to 4.e
    // (mount_service::MountService needs the write-path wired through the
    // adapter trait). 4.d ships the durability primitives; 4.e ships the
    // kernel-facing glue. When 4.e lands this test becomes active.
    panic!("bd-1du.4.d: real FUSE mount write integration is owned by 4.e");
}

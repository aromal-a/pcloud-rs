#![allow(clippy::pedantic)]
//! Integration tests for the `WritePathService` — the staging-blob +
//! write-journal + chunked-upload engine that backs FUSE mutations.
//!
//! These tests are intentionally independent of any FUSE kernel mount:
//! they wire the public surface of `WritePathService` against
//! hand-rolled mock upload backends and exercise:
//!
//! * journal replay after a "crash" (drop mid-stream),
//! * chunked flush dispatch for large files,
//! * retry on a transient upload error.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::{JournalOp, WriteJournal, replay_path};
use pcloud_fs::write_path::{
    FileUploadBackend, UploadStatus, WritePathError, WritePathOptions, WritePathService,
};

// -----------------------------------------------------------------------------
// Helper: flakey upload backend for retry tests.
// -----------------------------------------------------------------------------

#[derive(Debug, Default)]
struct FlakeyUploadBackend {
    calls: AtomicU32,
    fail_first_n: u32,
    uploads: Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl FlakeyUploadBackend {
    fn new(fail_first_n: u32) -> Self {
        Self {
            calls: AtomicU32::new(0),
            fail_first_n,
            uploads: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn total_calls(&self) -> u32 {
        self.calls.load(Ordering::Relaxed)
    }
}

impl FileUploadBackend for FlakeyUploadBackend {
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &Path,
    ) -> Result<(), WritePathError> {
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        if n < self.fail_first_n {
            return Err(WritePathError::Upload(format!("transient #{n}")));
        }
        let bytes =
            std::fs::read(staging_file).map_err(|e| WritePathError::Upload(e.to_string()))?;
        let full = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        self.uploads.lock().unwrap().insert(full, bytes);
        Ok(())
    }

    fn unlink_remote(&self, _path: &str) -> Result<(), WritePathError> {
        Ok(())
    }

    fn rename_remote(&self, _from: &str, _to: &str) -> Result<(), WritePathError> {
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Chunked-upload recording backend for large-file tests.
// -----------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ChunkedRecordingBackend {
    calls: Mutex<Vec<String>>,
    in_progress: Mutex<std::collections::HashMap<u64, (String, String, Vec<u8>)>>,
    uploads: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    next_id: Mutex<u64>,
}

impl ChunkedRecordingBackend {
    fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
            ..Self::default()
        }
    }

    fn chunk_write_count(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("write:"))
            .count()
    }
}

impl FileUploadBackend for ChunkedRecordingBackend {
    fn upload_file(
        &self,
        _parent_path: &str,
        _name: &str,
        _staging_file: &Path,
    ) -> Result<(), WritePathError> {
        // If chunked flush is used, upload_file should NOT be called.
        self.calls
            .lock()
            .unwrap()
            .push("whole-file-fallback".into());
        Err(WritePathError::Upload(
            "whole-file fallback unexpected".into(),
        ))
    }

    fn unlink_remote(&self, _path: &str) -> Result<(), WritePathError> {
        Ok(())
    }

    fn rename_remote(&self, _from: &str, _to: &str) -> Result<(), WritePathError> {
        Ok(())
    }

    fn upload_create(&self, parent_path: &str, name: &str) -> Result<u64, WritePathError> {
        let mut id = self.next_id.lock().unwrap();
        let upload_id = *id;
        *id += 1;
        self.calls
            .lock()
            .unwrap()
            .push(format!("create:{parent_path}/{name}"));
        self.in_progress.lock().unwrap().insert(
            upload_id,
            (parent_path.to_owned(), name.to_owned(), Vec::new()),
        );
        Ok(upload_id)
    }

    fn upload_write(
        &self,
        upload_id: u64,
        offset: u64,
        chunk: &[u8],
    ) -> Result<(), WritePathError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("write:{upload_id}:{offset}:{}", chunk.len()));
        let mut inp = self.in_progress.lock().unwrap();
        let entry = inp.get_mut(&upload_id).ok_or_else(|| {
            WritePathError::Upload(format!("upload_write: unknown id {upload_id}"))
        })?;
        let end = (offset as usize) + chunk.len();
        if entry.2.len() < end {
            entry.2.resize(end, 0);
        }
        entry.2[offset as usize..end].copy_from_slice(chunk);
        Ok(())
    }

    fn upload_status(&self, upload_id: u64) -> Result<UploadStatus, WritePathError> {
        match self.in_progress.lock().unwrap().get(&upload_id) {
            Some((_, _, bytes)) => Ok(UploadStatus::Bytes(bytes.len() as u64)),
            None => Ok(UploadStatus::NotFound),
        }
    }

    fn upload_save(
        &self,
        upload_id: u64,
        parent_path: &str,
        name: &str,
        total_size: u64,
    ) -> Result<(), WritePathError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("save:{upload_id}:{total_size}"));
        let mut inp = self.in_progress.lock().unwrap();
        let (_, _, bytes) = inp
            .remove(&upload_id)
            .ok_or_else(|| WritePathError::Upload(format!("save: unknown id {upload_id}")))?;
        assert_eq!(bytes.len() as u64, total_size);
        let full = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        self.uploads.lock().unwrap().insert(full, bytes);
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Always-failing backend for journal replay tests.
// -----------------------------------------------------------------------------

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

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[test]
fn journal_replay_after_crash_applies_pending_operations() {
    let tmp = tempfile::tempdir().unwrap();
    let stage_root = tmp.path().join("stage");

    // Session 1 — create + write + fsync (fsync errors out due to the
    // rigged backend, but Create / Write / FlushBarrier must still be
    // journalled before the upload is attempted).
    {
        let stage = StagingDir::open(&stage_root).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(AlwaysFailingBackend);
        let svc = WritePathService::new(
            stage,
            journal,
            backend,
            WritePathOptions::default()
                .with_flush_threshold(64 * 1024 * 1024)
                .with_flush_interval(Duration::from_secs(3600)),
        );
        svc.create(42, "/", "pending.txt").unwrap();
        svc.write(42, 0, b"replayable").unwrap();
        let _ = svc.fsync(42); // fails, but records are already durable.
        // Drop: simulated crash / unmount.
    }

    // Session 2 — replay the on-disk journal without any daemon wiring.
    let records = replay_path(stage_root.join("journal.log")).unwrap();
    assert!(
        records
            .iter()
            .any(|r| matches!(&r.op, JournalOp::Create { name, .. } if name == "pending.txt")),
        "journal must contain Create: {records:#?}"
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(&r.op, JournalOp::Write { path, len, .. } if path == "/pending.txt" && *len == 10)),
        "journal must contain Write: {records:#?}"
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(&r.op, JournalOp::FlushBarrier { path } if path == "/pending.txt")),
        "journal must contain FlushBarrier: {records:#?}"
    );

    // Staging blob must also survive: the bytes that were durably
    // accepted by `write` are still addressable by ino-42.blob.
    let stage = StagingDir::open(&stage_root).unwrap();
    let blob = stage.read_blob("ino-42.blob").unwrap();
    assert_eq!(blob, b"replayable");
}

#[test]
fn chunked_flush_sends_correct_number_of_chunks_for_large_file() {
    let tmp = tempfile::tempdir().unwrap();
    let stage = StagingDir::open(tmp.path().join("stage")).unwrap();
    let journal = WriteJournal::open(stage.journal_path()).unwrap();
    let backend = Arc::new(ChunkedRecordingBackend::new());
    // Force size-based auto-flush on small payloads by configuring a tiny
    // threshold — then the mid-write path takes the chunked pipeline.
    let svc = WritePathService::new(
        stage,
        journal,
        Arc::clone(&backend),
        WritePathOptions::default()
            .with_flush_threshold(256 * 1024) // 256 KiB — small on purpose.
            .with_flush_interval(Duration::from_secs(3600)),
    );

    svc.create(7, "/", "big.bin").unwrap();

    // Write 12 MiB in a single call. The chunked pipeline uses 4 MiB
    // chunks (UPLOAD_CHUNK_BYTES), so we must see exactly 3 upload_write
    // calls plus one upload_create and one upload_save.
    let payload = vec![0xABu8; 12 * 1024 * 1024];
    svc.write(7, 0, &payload).unwrap();
    // A flush on top to guarantee any residual state is flushed (no-op if
    // the size-trigger path already fired).
    let _ = svc.flush(7);

    // Verify the chunked surface was used (not the whole-file fallback).
    let chunk_writes = backend.chunk_write_count();
    assert_eq!(
        chunk_writes, 3,
        "12 MiB must split into exactly three 4 MiB upload_write calls, got {chunk_writes}"
    );
    let uploads = backend.uploads.lock().unwrap();
    assert_eq!(uploads.get("/big.bin").unwrap().len(), 12 * 1024 * 1024);
}

#[test]
fn flush_retry_on_transient_error_succeeds_on_second_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    let stage = StagingDir::open(tmp.path().join("stage")).unwrap();
    let journal = WriteJournal::open(stage.journal_path()).unwrap();
    // Fail the first upload, succeed the second.
    let backend = Arc::new(FlakeyUploadBackend::new(1));
    let svc = WritePathService::new(
        stage,
        journal,
        Arc::clone(&backend),
        WritePathOptions::default(),
    );

    svc.create(3, "/", "retry.bin").unwrap();
    svc.write(3, 0, b"hello").unwrap();

    // First flush returns an error (transient #0).
    let first = svc.flush(3);
    assert!(first.is_err(), "first flush must fail (transient)");

    // Second flush must succeed.
    let second = svc.flush(3);
    assert!(second.is_ok(), "second flush must succeed, got {second:?}");

    assert_eq!(
        backend.total_calls(),
        2,
        "backend must have been called exactly twice"
    );
    let uploads = backend.uploads.lock().unwrap();
    assert_eq!(uploads.get("/retry.bin").unwrap(), b"hello");
}

#[test]
fn multiple_inodes_flush_independently() {
    let tmp = tempfile::tempdir().unwrap();
    let stage = StagingDir::open(tmp.path().join("stage")).unwrap();
    let journal = WriteJournal::open(stage.journal_path()).unwrap();
    let backend = Arc::new(FlakeyUploadBackend::new(0)); // never fail
    let svc = WritePathService::new(
        stage,
        journal,
        Arc::clone(&backend),
        WritePathOptions::default(),
    );
    svc.create(10, "/", "a.bin").unwrap();
    svc.create(11, "/", "b.bin").unwrap();
    svc.write(10, 0, b"aaa").unwrap();
    svc.write(11, 0, b"bbbb").unwrap();
    svc.flush(10).unwrap();
    svc.flush(11).unwrap();
    let uploads = backend.uploads.lock().unwrap();
    assert_eq!(uploads.get("/a.bin").unwrap(), b"aaa");
    assert_eq!(uploads.get("/b.bin").unwrap(), b"bbbb");
}

#[test]
fn unopened_inode_write_returns_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let stage = StagingDir::open(tmp.path().join("stage")).unwrap();
    let journal = WriteJournal::open(stage.journal_path()).unwrap();
    let backend = Arc::new(FlakeyUploadBackend::new(0));
    let svc = WritePathService::new(stage, journal, backend, WritePathOptions::default());

    let err = svc.write(999, 0, b"x").unwrap_err();
    assert_eq!(err.to_errno(), pcloud_fs::errors::EINVAL);
}

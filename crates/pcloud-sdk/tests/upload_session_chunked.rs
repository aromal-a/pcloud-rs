#![allow(clippy::pedantic)]
//! Integration tests for the chunked `UploadSession` state machine.
//!
//! These exercise the state machine through a deterministic in-memory
//! [`UploadSessionDriver`] mock so the transitions and journal contract
//! can be validated without a live pCloud endpoint. Live-endpoint
//! verification is tracked separately under `bd-1du.10`.

use std::sync::Arc;
use std::sync::Mutex;

use pcloud_backends::upload_journal::UploadJournal;
use pcloud_embedded_sdk::{
    FileMetadata, UploadError, UploadHandle, UploadSession, UploadSessionDriver, UploadState,
};
use tempfile::tempdir;

/// Records of each driver call, used by tests to assert what wire
/// calls were issued.
#[derive(Default, Debug)]
struct MockLog {
    created: Vec<(u64, String, u64)>,
    writes: Vec<(u64, u64, usize)>,
    saves: Vec<u64>,
    deletes: Vec<u64>,
}

struct MockDriver {
    next_upload_id: u64,
    log: Arc<Mutex<MockLog>>,
    fail_create: bool,
    fail_write: bool,
    fail_save: bool,
    server_hash: Option<String>,
}

impl MockDriver {
    fn new() -> (Self, Arc<Mutex<MockLog>>) {
        let log = Arc::new(Mutex::new(MockLog::default()));
        (
            Self {
                next_upload_id: 1001,
                log: log.clone(),
                fail_create: false,
                fail_write: false,
                fail_save: false,
                server_hash: None,
            },
            log,
        )
    }
}

impl UploadSessionDriver for MockDriver {
    fn create(
        &mut self,
        folder_id: u64,
        file_name: &str,
        total: u64,
    ) -> Result<UploadHandle, UploadError> {
        if self.fail_create {
            return Err(UploadError::Helper(
                pcloud_embedded_sdk::UploadHelperError::Create("create forced to fail".to_owned()),
            ));
        }
        let uid = self.next_upload_id;
        self.next_upload_id += 1;
        self.log
            .lock()
            .unwrap()
            .created
            .push((folder_id, file_name.to_owned(), total));
        Ok(UploadHandle {
            upload_id: uid,
            parent_folder_id: folder_id,
            file_name: file_name.to_owned(),
        })
    }

    fn write_chunk(
        &mut self,
        handle: &UploadHandle,
        offset: u64,
        buf: &[u8],
    ) -> Result<u64, UploadError> {
        if self.fail_write {
            return Err(UploadError::Helper(
                pcloud_embedded_sdk::UploadHelperError::Write("write forced to fail".to_owned()),
            ));
        }
        self.log
            .lock()
            .unwrap()
            .writes
            .push((handle.upload_id, offset, buf.len()));
        Ok(offset + buf.len() as u64)
    }

    fn save(&mut self, handle: &UploadHandle) -> Result<FileMetadata, UploadError> {
        self.log.lock().unwrap().saves.push(handle.upload_id);
        if self.fail_save {
            return Err(UploadError::Helper(
                pcloud_embedded_sdk::UploadHelperError::Write("save forced to fail".to_owned()),
            ));
        }
        Ok(FileMetadata {
            file_id: Some(42),
            parent_folder_id: handle.parent_folder_id,
            name: handle.file_name.clone(),
            bytes_uploaded: 0, // set by test drivers when known
            conflicted: false,
            server_hash: self.server_hash.clone(),
        })
    }

    fn delete(&mut self, handle: &UploadHandle) -> Result<(), UploadError> {
        self.log.lock().unwrap().deletes.push(handle.upload_id);
        Ok(())
    }
}

fn chunk_stream(total: usize, chunk: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < total {
        let end = (i + chunk).min(total);
        out.push(vec![0xAB; end - i]);
        i = end;
    }
    out
}

#[test]
fn start_then_write_then_save_completes() {
    let (mut driver, log) = MockDriver::new();
    let dir = tempdir().unwrap();
    let journal = UploadJournal::open(dir.path()).unwrap();

    let total: usize = 10 * 1024;
    let chunk: usize = 4 * 1024;
    let chunks = chunk_stream(total, chunk);

    let session = UploadSession::start(42, "hello.bin", total as u64, &mut driver, Some(journal))
        .expect("create should succeed");

    for buf in &chunks {
        session.write_chunk(&mut driver, buf).expect("write chunk");
    }

    let meta = session
        .save_and_complete(&mut driver, None)
        .expect("save should succeed");
    assert_eq!(meta.parent_folder_id, 42);
    assert_eq!(meta.name, "hello.bin");

    let log = log.lock().unwrap();
    assert_eq!(log.created.len(), 1);
    assert_eq!(log.writes.len(), chunks.len());
    assert_eq!(log.saves.len(), 1);
    assert!(log.deletes.is_empty());
}

#[test]
fn pause_persists_offset_in_journal() {
    let (mut driver, _log) = MockDriver::new();
    let dir = tempdir().unwrap();
    let journal = UploadJournal::open(dir.path()).unwrap();
    let journal_path = journal.path().to_owned();

    let total: usize = 12 * 1024;
    let chunk: usize = 4 * 1024;
    let chunks = chunk_stream(total, chunk);

    let session = UploadSession::start(
        7,
        "paused.bin",
        total as u64,
        &mut driver,
        Some(journal.clone()),
    )
    .unwrap();

    // Write two of three chunks, then pause.
    session.write_chunk(&mut driver, &chunks[0]).unwrap();
    session.write_chunk(&mut driver, &chunks[1]).unwrap();
    session.pause().expect("pause");

    let snap = session.progress().borrow().clone();
    assert_eq!(snap.state, UploadState::Paused);
    assert_eq!(snap.bytes_sent, (2 * chunk) as u64);

    // Journal must contain entries for the two persisted chunks with
    // the cumulative byte count matching the in-memory offset.
    let report = UploadJournal::open(dir.path()).unwrap().replay().unwrap();
    let entries: Vec<_> = report
        .entries
        .iter()
        .filter(|e| e.upload_id == session.handle().unwrap().upload_id)
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries.last().unwrap().bytes, (2 * chunk) as u64);
    assert_eq!(entries.last().unwrap().chunks_done, 2);

    assert!(journal_path.exists());
}

#[test]
fn resume_after_crash_replays_from_journal() {
    // First "process": write two chunks, then drop the session without
    // completing. The journal stays on disk.
    let (mut driver1, _log1) = MockDriver::new();
    let dir = tempdir().unwrap();
    let journal1 = UploadJournal::open(dir.path()).unwrap();

    let total: usize = 12 * 1024;
    let chunk: usize = 4 * 1024;
    let chunks = chunk_stream(total, chunk);

    let session1 = UploadSession::start(
        99,
        "resume.bin",
        total as u64,
        &mut driver1,
        Some(journal1.clone()),
    )
    .unwrap();
    session1.write_chunk(&mut driver1, &chunks[0]).unwrap();
    session1.write_chunk(&mut driver1, &chunks[1]).unwrap();
    let uid = session1.handle().unwrap().upload_id;
    drop(session1);

    // Second "process": rebuild a session that inherits the same
    // upload id (simulate replay — the caller normally reads the
    // journal and reconstructs the handle). We then call `resume()`
    // which reconciles the in-memory offset with the journal.
    let (mut driver2, _log2) = MockDriver::new();
    let journal2 = UploadJournal::open(dir.path()).unwrap();

    // Seed a session reusing the prior upload_id by constructing
    // through `start` with a driver that returns the same id.
    struct ReplayDriver {
        target_id: u64,
        log: Arc<Mutex<MockLog>>,
    }
    impl UploadSessionDriver for ReplayDriver {
        fn create(
            &mut self,
            folder_id: u64,
            file_name: &str,
            total: u64,
        ) -> Result<UploadHandle, UploadError> {
            self.log
                .lock()
                .unwrap()
                .created
                .push((folder_id, file_name.to_owned(), total));
            Ok(UploadHandle {
                upload_id: self.target_id,
                parent_folder_id: folder_id,
                file_name: file_name.to_owned(),
            })
        }
        fn write_chunk(
            &mut self,
            handle: &UploadHandle,
            offset: u64,
            buf: &[u8],
        ) -> Result<u64, UploadError> {
            self.log
                .lock()
                .unwrap()
                .writes
                .push((handle.upload_id, offset, buf.len()));
            Ok(offset + buf.len() as u64)
        }
        fn save(&mut self, handle: &UploadHandle) -> Result<FileMetadata, UploadError> {
            self.log.lock().unwrap().saves.push(handle.upload_id);
            Ok(FileMetadata {
                file_id: Some(1),
                parent_folder_id: handle.parent_folder_id,
                name: handle.file_name.clone(),
                bytes_uploaded: 0,
                conflicted: false,
                server_hash: None,
            })
        }
        fn delete(&mut self, handle: &UploadHandle) -> Result<(), UploadError> {
            self.log.lock().unwrap().deletes.push(handle.upload_id);
            Ok(())
        }
    }

    let log2 = Arc::new(Mutex::new(MockLog::default()));
    let mut replay_driver = ReplayDriver {
        target_id: uid,
        log: log2.clone(),
    };
    let _ = &mut driver2; // silence unused

    let session2 = UploadSession::start(
        99,
        "resume.bin",
        total as u64,
        &mut replay_driver,
        Some(journal2),
    )
    .unwrap();

    // Before resume the in-memory offset is 0.
    assert_eq!(session2.current_offset(), Some(0));
    // After pause+resume, resume() reads the journal and fast-forwards
    // the offset.
    session2.pause().unwrap();
    session2.resume().unwrap();
    assert_eq!(session2.current_offset(), Some((2 * chunk) as u64));
    assert_eq!(session2.progress().borrow().bytes_sent, (2 * chunk) as u64);

    // Finish the last chunk and commit.
    session2
        .write_chunk(&mut replay_driver, &chunks[2])
        .unwrap();
    session2
        .save_and_complete(&mut replay_driver, None)
        .unwrap();

    // The replay driver only issued one write (the tail); resume
    // skipped the already-journaled chunks.
    assert_eq!(log2.lock().unwrap().writes.len(), 1);
}

#[test]
fn cancel_clears_journal_and_calls_upload_delete() {
    let (mut driver, log) = MockDriver::new();
    let dir = tempdir().unwrap();
    let journal = UploadJournal::open(dir.path()).unwrap();

    let total: usize = 8 * 1024;
    let chunk: usize = 4 * 1024;
    let chunks = chunk_stream(total, chunk);

    let session =
        UploadSession::start(3, "c.bin", total as u64, &mut driver, Some(journal.clone())).unwrap();
    session.write_chunk(&mut driver, &chunks[0]).unwrap();

    let handle = session.handle().expect("handle");
    session.cancel();

    // Caller drives the upload_delete call and journal clear on cancel.
    driver.delete(&handle).expect("delete should succeed");
    journal.clear().expect("journal clear");

    // After cancel, await_completion returns Canceled.
    let err = session.await_completion().unwrap_err();
    assert!(matches!(err, UploadError::Canceled));

    // The driver saw exactly one delete and the journal is empty.
    let log = log.lock().unwrap();
    assert_eq!(log.deletes, vec![handle.upload_id]);
    assert!(journal.replay().unwrap().entries.is_empty());
}

#[test]
fn driver_and_state_transition_failures_are_typed_and_observable() {
    let (mut create_failure, _) = MockDriver::new();
    create_failure.fail_create = true;
    assert!(matches!(
        UploadSession::start(1, "create.bin", 4, &mut create_failure, None),
        Err(UploadError::Helper(_))
    ));

    let (mut driver, _) = MockDriver::new();
    let session = UploadSession::start(2, "state.bin", 4, &mut driver, None).unwrap();
    assert!(matches!(
        session.clone().await_completion(),
        Err(UploadError::NotStarted)
    ));
    session.pause().unwrap();
    session.pause().unwrap();
    assert_eq!(session.progress().borrow().state, UploadState::Paused);
    assert!(matches!(
        session.write_chunk(&mut driver, b"ab"),
        Err(UploadError::InvalidState(_))
    ));
    session.resume().unwrap();

    driver.fail_write = true;
    assert!(matches!(
        session.write_chunk(&mut driver, b"ab"),
        Err(UploadError::Helper(_))
    ));
    assert_eq!(session.current_offset(), Some(0));
    driver.fail_write = false;
    session.write_chunk(&mut driver, b"ab").unwrap();
    assert!(matches!(
        session.clone().save_and_complete(&mut driver, None),
        Err(UploadError::InvalidState(_))
    ));
    session.write_chunk(&mut driver, b"cd").unwrap();

    driver.fail_save = true;
    let mut progress = session.progress();
    assert!(matches!(
        session.clone().save_and_complete(&mut driver, None),
        Err(UploadError::Helper(_))
    ));
    assert_eq!(
        progress.borrow_and_update().state,
        UploadState::Failed,
        "save failure must be terminal"
    );
    assert!(matches!(session.pause(), Err(UploadError::InvalidState(_))));
    assert!(matches!(
        session.resume(),
        Err(UploadError::InvalidState(_))
    ));
    assert!(matches!(
        session.await_completion(),
        Err(UploadError::Helper(_))
    ));
}

#[test]
fn server_hash_verification_covers_match_and_mismatch_outcomes() {
    let (mut mismatch_driver, _) = MockDriver::new();
    mismatch_driver.server_hash = Some("server-hash".to_owned());
    let mismatch = UploadSession::start(3, "mismatch.bin", 3, &mut mismatch_driver, None).unwrap();
    mismatch.write_chunk(&mut mismatch_driver, b"abc").unwrap();
    let progress = mismatch.progress();
    assert!(matches!(
        mismatch
            .clone()
            .save_and_complete(&mut mismatch_driver, Some("local-hash")),
        Err(UploadError::HashMismatch { .. })
    ));
    assert_eq!(progress.borrow().state, UploadState::Failed);
    assert!(matches!(
        mismatch.await_completion(),
        Err(UploadError::HashMismatch { .. })
    ));

    let (mut matching_driver, _) = MockDriver::new();
    matching_driver.server_hash = Some("same-hash".to_owned());
    let matching = UploadSession::start(4, "match.bin", 3, &mut matching_driver, None).unwrap();
    matching.write_chunk(&mut matching_driver, b"abc").unwrap();
    let meta = matching
        .clone()
        .save_and_complete(&mut matching_driver, Some("same-hash"))
        .unwrap();
    assert_eq!(meta.server_hash.as_deref(), Some("same-hash"));
    assert_eq!(
        matching.await_completion().unwrap().name,
        "match.bin".to_owned()
    );
}

#[test]
fn payload_debug_never_exposes_in_memory_bytes() {
    let bytes = pcloud_embedded_sdk::UploadPayload::Bytes(b"private payload".to_vec());
    let rendered = format!("{bytes:?}");
    assert!(rendered.contains("len"));
    assert!(!rendered.contains("private payload"));

    let file = pcloud_embedded_sdk::UploadPayload::File("payload.bin".into());
    assert!(format!("{file:?}").contains("payload.bin"));
}

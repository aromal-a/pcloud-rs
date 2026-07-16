#![allow(clippy::pedantic)]
//! Crash-replay proof for the upload journal (PLAN_A_PLUS P1.2).
//!
//! These tests exercise the exact failure mode that motivated the
//! journal: SIGKILL between `write(2)` and `fsync(2)` leaves a line
//! without a trailing newline.  Replay must reject that line and keep
//! the earlier, well-formed entries.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

use pcloud_daemon::upload_journal::{JournalEntry, UploadJournal};
use tempfile::tempdir;

fn entry(id: u64, chunks: u64, bytes: u64) -> JournalEntry {
    JournalEntry {
        upload_id: id,
        chunks_done: chunks,
        bytes,
        sha_partial: Some(format!("sha-{id}")),
        descriptor: None,
        committed: false,
    }
}

#[test]
fn three_fully_written_entries_are_all_restored() {
    let dir = tempdir().unwrap();
    let journal = UploadJournal::open(dir.path()).unwrap();

    journal.append(&entry(1, 1, 1024)).unwrap();
    journal.append(&entry(2, 2, 2048)).unwrap();
    journal.append(&entry(3, 3, 4096)).unwrap();

    // Drop and reopen — simulates a daemon restart with an intact journal.
    drop(journal);
    let journal = UploadJournal::open(dir.path()).unwrap();
    let report = journal.replay().unwrap();

    assert_eq!(report.rejected_lines, 0);
    assert_eq!(report.entries.len(), 3);
    assert_eq!(report.entries[0].upload_id, 1);
    assert_eq!(report.entries[1].upload_id, 2);
    assert_eq!(report.entries[2].upload_id, 3);
}

#[test]
fn half_written_trailing_line_is_rejected_earlier_entries_survive() {
    let dir = tempdir().unwrap();
    let journal = UploadJournal::open(dir.path()).unwrap();

    journal.append(&entry(10, 1, 100)).unwrap();
    journal.append(&entry(20, 2, 200)).unwrap();
    journal.append(&entry(30, 3, 300)).unwrap();

    // Simulate SIGKILL mid-append: strip the final newline so the last
    // record looks half-flushed.
    let path = journal.path().to_path_buf();
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let len = f.metadata().unwrap().len();
    assert!(len > 0);
    f.set_len(len - 1).unwrap();
    f.seek(SeekFrom::End(0)).unwrap();
    drop(f);

    // Reopen and replay.
    let journal = UploadJournal::open(dir.path()).unwrap();
    let report = journal.replay().unwrap();

    assert_eq!(
        report.rejected_lines, 1,
        "expected exactly one partial line to be rejected"
    );
    assert_eq!(
        report.entries.len(),
        2,
        "earlier well-formed entries must be preserved",
    );
    assert_eq!(report.entries[0].upload_id, 10);
    assert_eq!(report.entries[1].upload_id, 20);
}

#[test]
fn half_written_then_clear_resets_journal() {
    let dir = tempdir().unwrap();
    let journal = UploadJournal::open(dir.path()).unwrap();
    journal.append(&entry(99, 1, 1)).unwrap();

    // Torn-write simulation.
    let mut f = OpenOptions::new()
        .append(true)
        .open(journal.path())
        .unwrap();
    f.write_all(b"{\"upload_id\":100,\"chunks_done\":1")
        .unwrap();
    drop(f);

    let report = journal.replay().unwrap();
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.rejected_lines, 1);

    // After clear, journal is empty.
    journal.clear().unwrap();
    let report = journal.replay().unwrap();
    assert!(report.entries.is_empty());
    assert_eq!(report.rejected_lines, 0);
}

#[test]
fn replay_reconciles_unknown_upload_ids_with_warning() {
    use pcloud_daemon::transfer_backend::TransferRuntime;

    let dir = tempdir().unwrap();
    let journal = UploadJournal::open(dir.path()).unwrap();
    journal.append(&entry(1, 1, 10)).unwrap();
    journal.append(&entry(2, 2, 20)).unwrap();
    journal.append(&entry(42, 3, 30)).unwrap();

    // Only uploads 1 and 2 are known to this session.  Upload 42 must
    // land in the "unknown" bucket so the caller can log a warning.
    let (known, unknown, report) =
        TransferRuntime::replay_upload_journal(&journal, &[1, 2]).unwrap();

    assert_eq!(report.entries.len(), 3);
    assert_eq!(report.rejected_lines, 0);
    assert_eq!(known.len(), 2);
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].upload_id, 42);
}

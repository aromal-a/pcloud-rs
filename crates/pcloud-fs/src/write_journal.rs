//! Write-ahead journal for the FUSE write path (bd-1du.4.d).
//!
//! The journal is an append-only file that records every mutation intended
//! for the remote backend: `create`, `write`, `truncate`, `unlink`, and
//! `rename`. It is used for crash-safe writeback: on remount, the daemon
//! replays any unflushed records against the staging dir and transport so
//! that a crash between `fsync` and remote upload does not silently lose
//! writes.
//!
//! # Durability contract
//!
//! * [`WriteJournal::append`] serialises one record, writes it to the
//!   underlying file with a length prefix and CRC, and (by default) calls
//!   `fsync(2)` on commit. A commit boundary is any `fsync`/`flush` from
//!   the FUSE layer, or an explicit [`WriteJournal::commit`] call.
//! * Truncated tails caused by a crash-during-append are detected by the
//!   length/CRC envelope and stop replay cleanly; earlier records remain
//!   valid.
//! * The journal file is created mode `0o600` and lives inside the
//!   staging directory (which itself is `0o700`).
//!
//! # Scope
//!
//! This is **not** the coarse [`crate::journal::WritebackJournal`] used by
//! the 4.a in-memory scaffold. That one tracks pending-byte bookkeeping
//! for flush coalescing. The two coexist: `WriteJournal` owns on-disk
//! durability, `WritebackJournal` owns in-memory pending accounting.

// **PLATFORM:** Unix (Linux, BSD, macOS)
// **GATING:** #[cfg(unix)].

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Envelope magic to detect garbage tails and foreign files.
const MAGIC: u32 = 0x50_43_4A_31; // "PCJ1"
/// Maximum record size accepted during replay. Records larger than this
/// are rejected rather than allocating unbounded memory.
const MAX_RECORD_BYTES: u32 = 16 * 1024 * 1024;

/// One durable operation the write path has agreed to replay on crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalOp {
    /// File creation under `parent_path` with `name`.
    Create {
        /// Absolute parent directory path.
        parent_path: String,
        /// Final path component to create.
        name: String,
    },
    /// Bytes written to staging at `path` covering `[offset, offset+len)`.
    ///
    /// Payload is referenced by content-addressed staging filename, not
    /// inlined — the journal records metadata only. The staging file is
    /// fsynced before the journal record is committed (write-ahead order).
    Write {
        /// Absolute logical path that was written.
        path: String,
        /// Starting byte offset of the write.
        offset: u64,
        /// Length in bytes of the write.
        len: u64,
        /// Content-addressed staging filename holding the payload bytes.
        staging_blob: String,
    },
    /// Logical truncation to `new_size`.
    Truncate {
        /// Absolute logical path.
        path: String,
        /// New file size in bytes after the truncation.
        new_size: u64,
    },
    /// File removal.
    Unlink {
        /// Absolute logical path to remove.
        path: String,
    },
    /// Rename / move, possibly across directories.
    Rename {
        /// Source absolute path.
        from: String,
        /// Destination absolute path.
        to: String,
    },
    /// Flush boundary — marks a point at which the daemon promised durability
    /// to the caller (from an `fsync`/`flush` FUSE op).
    FlushBarrier {
        /// Path whose pending writes must be durable at this point.
        path: String,
    },
    /// Chunked-upload progress checkpoint. Recorded after each
    /// `upload_write` ack from the backend so a crash mid-stream can
    /// resume from the last journalled offset rather than the last
    /// fsynced sidecar (the sidecar remains the resume source-of-truth;
    /// this record gives an auditable per-chunk replay log alongside it).
    ///
    /// Added in audit-06 stream E (bd-1du.4.6) to make chunked
    /// pipelining replay-safe: each chunk transmission is a discrete
    /// journal record carrying `(path, upload_id, offset, len)` so a
    /// post-crash inspector can reconstruct upload progress without
    /// trusting the sidecar alone.
    ChunkAck {
        /// Absolute logical path that was being uploaded.
        path: String,
        /// Backend-assigned upload session id.
        upload_id: u64,
        /// Byte offset of the chunk that was acknowledged by the server.
        offset: u64,
        /// Length of the acknowledged chunk in bytes.
        len: u64,
    },
}

/// Record persisted on disk. `seq` gives total ordering; `op` carries the
/// operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRecord {
    /// Monotonic sequence number assigned at commit; gives total ordering
    /// across all operations in the journal.
    pub seq: u64,
    /// The operation to replay on recovery.
    pub op: JournalOp,
}

/// Errors produced by the write journal.
#[derive(Debug, thiserror::Error)]
pub enum WriteJournalError {
    /// Underlying filesystem I/O failure (short write, EIO, permission).
    #[error("journal I/O failure: {0}")]
    Io(#[from] io::Error),
    /// Serialisation / deserialisation failure of a journal record.
    #[error("journal encode/decode failed: {0}")]
    Codec(String),
    /// A single record exceeded the internal maximum (16 MiB); rejected
    /// to keep replay memory bounded.
    #[error("journal record exceeds maximum size: {0} bytes")]
    RecordTooLarge(u32),
    /// The envelope magic or CRC did not match; replay stops at this
    /// offset and earlier records remain valid.
    #[error("journal envelope corrupt at offset {offset}: {reason}")]
    Corrupt {
        /// File offset where the corruption was detected.
        offset: u64,
        /// Human-readable reason (bad magic, truncated payload, crc mismatch).
        reason: &'static str,
    },
}

impl From<serde_json::Error> for WriteJournalError {
    fn from(err: serde_json::Error) -> Self {
        Self::Codec(err.to_string())
    }
}

/// Append-only, fsync-on-commit write journal.
///
/// Records are framed as:
/// ```text
///   u32 magic (LE)
///   u32 payload_len (LE)
///   u32 crc32 (LE, over payload)
///   bytes[payload_len]  // serde_json-encoded JournalRecord
/// ```
///
/// The envelope lets replay recover from torn tails: if any of the fixed
/// header bytes or the payload is short/mismatched, replay stops at that
/// offset and earlier records remain valid.
pub struct WriteJournal {
    path: PathBuf,
    file: File,
    next_seq: u64,
    fsync_on_commit: bool,
}

impl std::fmt::Debug for WriteJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteJournal")
            .field("path", &self.path)
            .field("next_seq", &self.next_seq)
            .field("fsync_on_commit", &self.fsync_on_commit)
            .finish()
    }
}

impl WriteJournal {
    /// Open (or create) a journal file at `path`. The file is created with
    /// mode `0o600`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WriteJournalError> {
        let path = path.as_ref().to_path_buf();
        let file = open_journal_file(&path)?;
        let mut journal = Self {
            path,
            file,
            next_seq: 1,
            fsync_on_commit: true,
        };
        journal.seek_end()?;
        if let Ok(records) = replay_path(&journal.path) {
            if let Some(max_seq) = records.iter().map(|record| record.seq).max() {
                journal.next_seq = max_seq.saturating_add(1);
            }
        }
        Ok(journal)
    }

    /// Disable fsync-on-commit. **Only** intended for tests. Production
    /// callers must leave fsync enabled; see `bd-1du.4.d` durability rules.
    // Scaffolding for bd-1du.4 durability tests — currently unused but
    // exposed so fast-path unit tests can skip the fsync cost without
    // reaching into private fields. Do not call from production code.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn set_fsync_on_commit(&mut self, enabled: bool) {
        self.fsync_on_commit = enabled;
    }

    /// Append `op` with the next monotonic sequence number and commit.
    pub fn append(&mut self, op: JournalOp) -> Result<u64, WriteJournalError> {
        let seq = self.next_seq;
        let record = JournalRecord { seq, op };
        let payload = serde_json::to_vec(&record)?;
        if payload.len() as u64 > u64::from(MAX_RECORD_BYTES) {
            return Err(WriteJournalError::RecordTooLarge(
                u32::try_from(payload.len()).unwrap_or(u32::MAX),
            ));
        }
        let payload_len = u32::try_from(payload.len())
            .map_err(|_| WriteJournalError::RecordTooLarge(u32::MAX))?;
        let crc = crc32_ieee(&payload);

        let mut header = [0u8; 12];
        header[..4].copy_from_slice(&MAGIC.to_le_bytes());
        header[4..8].copy_from_slice(&payload_len.to_le_bytes());
        header[8..12].copy_from_slice(&crc.to_le_bytes());
        self.file.write_all(&header)?;
        self.file.write_all(&payload)?;
        self.commit()?;
        self.next_seq += 1;
        Ok(seq)
    }

    /// Explicit commit point. Flushes and (by default) fsyncs the journal
    /// file, then syncs the parent directory so the rename (or creation) of
    /// the journal file itself is durable on the containing directory entry.
    pub fn commit(&mut self) -> Result<(), WriteJournalError> {
        self.file.flush()?;
        if self.fsync_on_commit {
            self.file.sync_data()?;
            // Sync parent directory so the rename is durable (C-1 audit fix).
            let parent = self.path.parent().unwrap_or(std::path::Path::new("."));
            if let Ok(dir) = std::fs::File::open(parent) {
                if let Err(e) = dir.sync_all() {
                    log::warn!("journal: parent-dir fsync failed (durability gap): {e}");
                }
            }
        }
        Ok(())
    }

    /// Current on-disk size, in bytes.
    pub fn byte_len(&self) -> io::Result<u64> {
        self.file.metadata().map(|m| m.len())
    }

    /// Whether the journal file is currently empty on disk.
    pub fn is_empty(&self) -> io::Result<bool> {
        self.byte_len().map(|n| n == 0)
    }

    /// Truncate the journal after successful replay / checkpoint.
    ///
    /// After calling this, the sequence counter is *not* reset. This
    /// preserves causal ordering across checkpoints.
    pub fn reset(&mut self) -> Result<(), WriteJournalError> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.commit()
    }

    /// Remove records that affect `path`, preserving all unrelated dirty
    /// records in the shared journal.
    ///
    /// This is the path-scoped checkpoint used after a successful flush.
    /// A whole-journal reset is only safe when no other dirty inode has
    /// outstanding records.
    pub fn checkpoint_path(&mut self, path: &str) -> Result<(), WriteJournalError> {
        let records = self.replay()?;
        let retained: Vec<JournalRecord> = records
            .into_iter()
            .filter(|record| !record_targets_path(record, path))
            .collect();
        self.rewrite_records(&retained)
    }

    /// Re-open for read-only replay and return all well-formed records up
    /// to the first torn/garbage tail. The file handle held by `self` is
    /// unaffected.
    pub fn replay(&self) -> Result<Vec<JournalRecord>, WriteJournalError> {
        replay_path(&self.path)
    }

    fn seek_end(&mut self) -> Result<(), WriteJournalError> {
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    fn rewrite_records(&mut self, records: &[JournalRecord]) -> Result<(), WriteJournalError> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        for record in records {
            write_record_frame(&mut self.file, record)?;
        }
        if let Some(max_seq) = records.iter().map(|record| record.seq).max() {
            self.next_seq = self.next_seq.max(max_seq.saturating_add(1));
        }
        self.commit()
    }
}

fn write_record_frame(file: &mut File, record: &JournalRecord) -> Result<(), WriteJournalError> {
    let payload = serde_json::to_vec(record)?;
    if payload.len() as u64 > u64::from(MAX_RECORD_BYTES) {
        return Err(WriteJournalError::RecordTooLarge(
            u32::try_from(payload.len()).unwrap_or(u32::MAX),
        ));
    }
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| WriteJournalError::RecordTooLarge(u32::MAX))?;
    let crc = crc32_ieee(&payload);

    let mut header = [0u8; 12];
    header[..4].copy_from_slice(&MAGIC.to_le_bytes());
    header[4..8].copy_from_slice(&payload_len.to_le_bytes());
    header[8..12].copy_from_slice(&crc.to_le_bytes());
    file.write_all(&header)?;
    file.write_all(&payload)?;
    Ok(())
}

fn record_targets_path(record: &JournalRecord, path: &str) -> bool {
    match &record.op {
        JournalOp::Create { parent_path, name } => join_path(parent_path, name) == path,
        JournalOp::Write { path: p, .. }
        | JournalOp::Truncate { path: p, .. }
        | JournalOp::Unlink { path: p }
        | JournalOp::FlushBarrier { path: p }
        | JournalOp::ChunkAck { path: p, .. } => p == path,
        JournalOp::Rename { from, to } => from == path || to == path,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Read all well-formed records from `path` and stop on the first torn /
/// malformed envelope. Returns the recovered prefix.
pub fn replay_path(path: impl AsRef<Path>) -> Result<Vec<JournalRecord>, WriteJournalError> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = OpenOptions::new().read(true).open(path)?;
    let total = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let mut out = Vec::new();
    let mut cursor: u64 = 0;
    loop {
        if cursor == total {
            break;
        }
        let mut header = [0u8; 12];
        if let Err(err) = reader.read_exact(&mut header) {
            if err.kind() == io::ErrorKind::UnexpectedEof {
                break; // torn header tail — stop cleanly
            }
            return Err(err.into());
        }
        // SAFETY: `header` is `[u8; 12]`; the three slices `[..4]`, `[4..8]`,
        // `[8..12]` are each 4 bytes by construction, so `try_into::<[u8;4]>`
        // is infallible. A panic here would mean a logic error in this
        // decode loop, not runtime data corruption.
        let magic = u32::from_le_bytes(
            header[..4]
                .try_into()
                .expect("invariant: header[..4] is 4 bytes by const construction"),
        );
        if magic != MAGIC {
            // Stop at the first foreign/garbage frame.
            break;
        }
        let payload_len = u32::from_le_bytes(
            header[4..8]
                .try_into()
                .expect("invariant: header[4..8] is 4 bytes by const construction"),
        );
        let crc_expected = u32::from_le_bytes(
            header[8..12]
                .try_into()
                .expect("invariant: header[8..12] is 4 bytes by const construction"),
        );
        if payload_len > MAX_RECORD_BYTES {
            return Err(WriteJournalError::RecordTooLarge(payload_len));
        }
        let remaining = total.saturating_sub(cursor + 12);
        if u64::from(payload_len) > remaining {
            break; // torn payload — stop
        }
        let mut payload = vec![0u8; payload_len as usize];
        if let Err(err) = reader.read_exact(&mut payload) {
            if err.kind() == io::ErrorKind::UnexpectedEof {
                break;
            }
            return Err(err.into());
        }
        let crc_actual = crc32_ieee(&payload);
        if crc_actual != crc_expected {
            break; // torn / corrupt payload — stop at last good record
        }
        match serde_json::from_slice::<JournalRecord>(&payload) {
            Ok(rec) => out.push(rec),
            Err(_) => break,
        }
        cursor += 12 + u64::from(payload_len);
    }
    Ok(out)
}

#[cfg(unix)]
fn open_journal_file(path: &Path) -> Result<File, WriteJournalError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // Tighten permissions even if the file pre-existed with looser mode.
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_journal_file(path: &Path) -> Result<File, WriteJournalError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    Ok(file)
}

// -----------------------------------------------------------------------------
// CRC32/IEEE — tiny self-contained implementation so we do not pull in a
// dependency just for journal envelope checksums. Polynomial 0xEDB88320.
// -----------------------------------------------------------------------------

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        let mut c = (crc ^ u32::from(b)) & 0xFF;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
        crc = (crc >> 8) ^ c;
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn append_then_replay_returns_ordered_records() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.log");
        let mut j = WriteJournal::open(&path).unwrap();
        let s1 = j
            .append(JournalOp::Create {
                parent_path: "/".to_owned(),
                name: "a.txt".to_owned(),
            })
            .unwrap();
        let s2 = j
            .append(JournalOp::Write {
                path: "/a.txt".to_owned(),
                offset: 0,
                len: 5,
                staging_blob: "blob-1".to_owned(),
            })
            .unwrap();
        assert_eq!((s1, s2), (1, 2));

        let records = replay_path(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[1].seq, 2);
        match &records[1].op {
            JournalOp::Write { path, len, .. } => {
                assert_eq!(path, "/a.txt");
                assert_eq!(*len, 5);
            }
            _ => panic!("expected Write"),
        }
    }

    #[test]
    fn replay_stops_on_torn_tail() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.log");
        {
            let mut j = WriteJournal::open(&path).unwrap();
            j.append(JournalOp::Unlink {
                path: "/x".to_owned(),
            })
            .unwrap();
        }
        // Simulate torn tail: append garbage partial bytes.
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[0xDE, 0xAD]).unwrap();
        }
        let records = replay_path(&path).unwrap();
        assert_eq!(records.len(), 1);
        match &records[0].op {
            JournalOp::Unlink { path } => assert_eq!(path, "/x"),
            _ => panic!("expected Unlink"),
        }
    }

    #[test]
    fn replay_stops_on_crc_corruption() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.log");
        let mut j = WriteJournal::open(&path).unwrap();
        j.append(JournalOp::Unlink {
            path: "/x".to_owned(),
        })
        .unwrap();
        j.append(JournalOp::Unlink {
            path: "/y".to_owned(),
        })
        .unwrap();
        drop(j);

        // Flip a byte inside the second record's payload. We find the second
        // magic and corrupt payload byte at magic+12.
        let mut contents = std::fs::read(&path).unwrap();
        let first_hdr = 12usize;
        let first_payload = u32::from_le_bytes(contents[4..8].try_into().unwrap()) as usize;
        let corrupt_at = first_hdr + first_payload + 12 + 1;
        assert!(corrupt_at < contents.len());
        contents[corrupt_at] ^= 0xFF;
        std::fs::write(&path, &contents).unwrap();

        let records = replay_path(&path).unwrap();
        assert_eq!(records.len(), 1, "CRC mismatch must stop replay");
    }

    #[test]
    fn reset_truncates_but_preserves_seq_monotonicity() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.log");
        let mut j = WriteJournal::open(&path).unwrap();
        let s1 = j
            .append(JournalOp::Unlink {
                path: "/a".to_owned(),
            })
            .unwrap();
        j.reset().unwrap();
        let s2 = j
            .append(JournalOp::Unlink {
                path: "/b".to_owned(),
            })
            .unwrap();
        assert_eq!(s1, 1);
        assert_eq!(s2, 2, "seq must be monotonic across reset");
        assert_eq!(replay_path(&path).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn journal_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.log");
        let _j = WriteJournal::open(&path).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "journal file must be 0600, got {mode:o}");
    }

    #[test]
    fn crc32_known_vector() {
        // IEEE CRC32 of "123456789" == 0xCBF43926
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn append_survives_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.log");
        {
            let mut j = WriteJournal::open(&path).unwrap();
            j.append(JournalOp::FlushBarrier {
                path: "/doc".to_owned(),
            })
            .unwrap();
        }
        {
            let mut j = WriteJournal::open(&path).unwrap();
            let seq = j
                .append(JournalOp::Unlink {
                    path: "/gone".to_owned(),
                })
                .unwrap();
            assert_eq!(seq, 2, "reopened journal continues the seq counter");
        }
        let records = replay_path(&path).unwrap();
        assert_eq!(records.len(), 2);
    }
}

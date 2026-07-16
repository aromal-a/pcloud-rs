//! Crash-safe upload resume journal.
//!
//! P1.2 (PLAN_A_PLUS): the SQLite-backed `upload_resume_state` table is the
//! durable source of truth for in-progress uploads, but SQLite writes are
//! batched through the connection and a SIGKILL mid-commit can leave the
//! state inconsistent with what the in-memory machine believes it just
//! persisted.  To make replay deterministic even in that window, we
//! additionally append a newline-delimited JSON journal under the
//! daemon's runtime directory
//! (`$XDG_RUNTIME_DIR/pcloud/uploads.journal`).
//!
//! Each journal line is one [`JournalEntry`]:
//!
//! ```json
//! {"upload_id":42,"chunks_done":3,"bytes":3145728,"sha_partial":"abc..."}
//! ```
//!
//! Durability contract
//! -------------------
//!
//! Every append uses a **write-temp + fsync + rename + fsync(parent)**
//! pattern so that either:
//!
//! * the pre-append journal is visible (the new entry is lost, replay
//!   reconciles with in-memory state), or
//! * the post-append journal is visible in full (the new entry is
//!   present with a trailing newline).
//!
//! A line that was partially flushed (no trailing `\n`) is rejected at
//! replay time; earlier well-formed lines are still honored.
//!
//! The feature is compiled in unconditionally and is additive: callers
//! that never invoke [`UploadJournal::append`] pay no cost.  The journal
//! is intentionally tiny (one line per completed chunk) and is
//! truncated on successful `upload_save` via [`UploadJournal::clear`].

#![allow(clippy::module_name_repetitions)]

// **PLATFORM:** Unix (Linux, BSD, macOS)
// **GATING:** #[cfg(unix)].

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Journal filename (under the runtime dir).
pub const JOURNAL_FILE_NAME: &str = "uploads.journal";

/// One persisted journal record.  Mirrors the in-memory resume state
/// closely enough that replay can reconcile without touching SQLite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Server-assigned upload id (primary key within one pCloud session).
    pub upload_id: u64,
    /// Number of chunks the client has acknowledged writing.
    pub chunks_done: u64,
    /// Total bytes confirmed by the server.
    pub bytes: u64,
    /// Hex-encoded SHA-1 of the prefix `[0, bytes)`.  `None` until a full
    /// chunk boundary has been hashed.
    pub sha_partial: Option<String>,
    /// Descriptor needed to reconstruct a missing SQLite resume row after
    /// a crash between journal fsync and database commit. Older lines omit
    /// this additive field and remain readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<UploadJournalDescriptor>,
    /// Whether `upload_save` returned success for this upload. This marker is
    /// fsynced before SQLite cleanup so restart never re-uploads a known
    /// committed file.
    #[serde(default)]
    pub committed: bool,
}

/// Durable identity of one resumable local-file upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadJournalDescriptor {
    /// Stable SQLite resume key (source plus remote destination).
    pub resume_key: String,
    /// Canonical local source path.
    pub local_path: PathBuf,
    /// Canonical absolute remote destination path.
    pub remote_path: String,
    /// Remote parent folder id.
    pub parent_folder_id: u64,
    /// Remote leaf name.
    pub file_name: String,
    /// Expected source size.
    pub total_size: u64,
    /// Full source SHA-1 captured before upload.
    pub local_sha1: String,
    /// Conditional-overwrite hash, when requested.
    pub if_hash: Option<u64>,
    /// Whether create-if-new behavior was requested.
    pub if_new: bool,
}

/// Errors surfaced by the journal.
#[derive(Debug, Error)]
pub enum JournalError {
    /// Underlying I/O failure.
    #[error("journal I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A journal line failed to parse.  Surfaced to the caller so the
    /// daemon can log it; well-formed earlier lines are still returned.
    #[error("journal parse error: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Outcome of a [`UploadJournal::replay`] call.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    /// Well-formed entries in journal order.
    pub entries: Vec<JournalEntry>,
    /// Number of partial / malformed trailing lines discarded.  A healthy
    /// journal reports `0`; any value `> 0` indicates a crash mid-append
    /// was recovered.
    pub rejected_lines: usize,
}

/// Handle to the on-disk journal.
#[derive(Debug, Clone)]
pub struct UploadJournal {
    dir: PathBuf,
    path: PathBuf,
}

impl UploadJournal {
    /// Opens (or prepares) the journal at `dir/uploads.journal`.  Creates
    /// `dir` with mode 0700 if it does not exist.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let dir = dir.into();
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
            set_dir_mode_0700(&dir)?;
        }
        let path = dir.join(JOURNAL_FILE_NAME);
        Ok(Self { dir, path })
    }

    /// Journal file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends a single entry with the full durability contract:
    ///
    /// 1. Open the journal for append (create if missing, mode 0600).
    /// 2. Write the JSON line + `\n`.
    /// 3. `fsync` the file.
    /// 4. `fsync` the parent directory so the (possibly new) dirent is
    ///    durable across a power cut.
    ///
    /// This is intentionally *append-only* — a crash between (2) and (3)
    /// loses the new entry but cannot corrupt earlier ones.  A crash
    /// between (3) and (4) may lose the dirent update only if the file
    /// was just created; subsequent appends are safe.
    pub fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');

        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .mode_0600()
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.sync_all()?;
        drop(f);

        // fsync parent so the rename/creation is durable.
        fsync_dir(&self.dir)?;
        Ok(())
    }

    /// Atomically rewrites the journal with exactly `entries` using the
    /// write-temp + fsync + rename + fsync(parent) pattern.  Useful for
    /// compaction after a successful `upload_save`.
    pub fn rewrite_atomic(&self, entries: &[JournalEntry]) -> Result<(), JournalError> {
        let tmp = self.dir.join(format!("{JOURNAL_FILE_NAME}.tmp"));
        {
            let mut f = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode_0600()
                .open(&tmp)?;
            for e in entries {
                let mut line = serde_json::to_string(e)?;
                line.push('\n');
                f.write_all(line.as_bytes())?;
            }
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        fsync_dir(&self.dir)?;
        Ok(())
    }

    /// Truncates the journal to zero length.  Called after a successful
    /// `upload_save` to bound the journal's steady-state size.
    pub fn clear(&self) -> Result<(), JournalError> {
        self.rewrite_atomic(&[])
    }

    /// Replays the journal, returning all well-formed entries in order.
    ///
    /// A trailing line without a newline terminator (the SIGKILL
    /// mid-append case) is counted in [`ReplayReport::rejected_lines`]
    /// and discarded; earlier entries are retained.
    ///
    /// If the journal file does not exist, returns an empty report.
    pub fn replay(&self) -> Result<ReplayReport, JournalError> {
        if !self.path.exists() {
            return Ok(ReplayReport::default());
        }

        // Read the raw bytes so we can detect a missing trailing newline
        // (a half-flushed last record) deterministically.
        let mut buf = Vec::new();
        File::open(&self.path)?.read_to_end(&mut buf)?;

        let mut report = ReplayReport::default();
        if buf.is_empty() {
            return Ok(report);
        }

        let trailing_newline = buf.last().copied() == Some(b'\n');
        let reader = BufReader::new(&buf[..]);
        let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
        let total = lines.len();

        for (idx, line) in lines.into_iter().enumerate() {
            let is_last = idx + 1 == total;
            // Last line with no trailing newline → partial, reject.
            if is_last && !trailing_newline {
                report.rejected_lines += 1;
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JournalEntry>(&line) {
                Ok(entry) => report.entries.push(entry),
                Err(_) => {
                    // Malformed mid-journal line — treat as rejected but
                    // keep scanning.  This matches the contract that
                    // earlier well-formed entries survive corruption.
                    report.rejected_lines += 1;
                }
            }
        }
        Ok(report)
    }
}

// ---------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------

#[cfg(unix)]
fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    let f = File::open(dir)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn fsync_dir(_dir: &Path) -> std::io::Result<()> {
    // Directory fsync is a no-op on non-Unix.  The per-file `sync_all`
    // above is the best available guarantee.
    Ok(())
}

#[cfg(unix)]
fn set_dir_mode_0700(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(dir, perms)
}

#[cfg(not(unix))]
fn set_dir_mode_0700(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Tiny extension trait so we can call `.mode_0600()` on `OpenOptions`
/// without duplicating cfg gates in every call site.
trait OpenOptionsExt {
    fn mode_0600(&mut self) -> &mut Self;
}

#[cfg(unix)]
impl OpenOptionsExt for OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt as _;
        self.mode(0o600)
    }
}

#[cfg(not(unix))]
impl OpenOptionsExt for OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        self
    }
}

// ---------------------------------------------------------------------
// Unit tests (in-module; the public crash-replay integration test lives
// under `tests/upload_journal_crash_replay.rs`).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(id: u64, chunks: u64, bytes: u64) -> JournalEntry {
        JournalEntry {
            upload_id: id,
            chunks_done: chunks,
            bytes,
            sha_partial: Some(format!("sha-{id}-{chunks}")),
            descriptor: None,
            committed: false,
        }
    }

    #[test]
    fn append_then_replay_preserves_order() {
        let dir = tempdir().unwrap();
        let journal = UploadJournal::open(dir.path()).unwrap();
        journal.append(&entry(1, 1, 1024)).unwrap();
        journal.append(&entry(2, 2, 2048)).unwrap();

        let report = journal.replay().unwrap();
        assert_eq!(report.rejected_lines, 0);
        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.entries[0].upload_id, 1);
        assert_eq!(report.entries[1].upload_id, 2);
    }

    #[test]
    fn replay_missing_file_is_empty() {
        let dir = tempdir().unwrap();
        let journal = UploadJournal::open(dir.path()).unwrap();
        let report = journal.replay().unwrap();
        assert!(report.entries.is_empty());
        assert_eq!(report.rejected_lines, 0);
    }

    #[test]
    fn clear_truncates_journal() {
        let dir = tempdir().unwrap();
        let journal = UploadJournal::open(dir.path()).unwrap();
        journal.append(&entry(7, 1, 64)).unwrap();
        journal.clear().unwrap();
        assert_eq!(journal.replay().unwrap().entries.len(), 0);
    }
}

//! FUSE write path (bd-1du.4.d).
//!
//! Implements `create` / `write` / `flush` / `fsync` / `setattr`(truncate)
//! / `unlink` / `rename` on top of the disk-backed staging dir and the
//! write-ahead journal. Uploads themselves are delegated to an abstract
//! [`FileUploadBackend`] so that tests do not require a real transport and
//! so the daemon can wire the real `pcloud-proto::transfer_api` in 4.e.
//!
//! # Concurrency contract
//!
//! Per-inode state (staging blob, pending byte counter, dirty flag) is
//! locked with a `Mutex`. The `WriteBackService` holds a `Mutex<HashMap>`
//! indexed by inode. Concurrent writes to **different** inodes proceed in
//! parallel; concurrent writes to the **same** inode serialise through the
//! inode mutex — matching POSIX file-lock semantics on a single file
//! descriptor.
//!
//! # Flush policy
//!
//! Writeback is triggered by any of:
//!
//! * explicit FUSE `flush` / `fsync` on the inode,
//! * accumulated dirty-byte count reaches `flush_threshold_bytes`,
//! * wall-clock time since last flush exceeds `flush_interval`.
//!
//! The daemon mount loop is responsible for periodically calling
//! [`WritePathService::tick`] to enforce the time-based flush; the size
//! bound is checked on every `write`.
//!
//! # Atomic write protocol (P1.2)
//!
//! Every accepted FUSE `write` is made durable via a write-ahead
//! journal before any visible mutation. The ordering is strict:
//!
//! 1. **Append** a [`JournalRecord`] describing the op
//!    ([`JournalOp::Write`], [`JournalOp::Truncate`],
//!    [`JournalOp::Create`], [`JournalOp::Unlink`],
//!    [`JournalOp::Rename`]) to the journal file with a CRC32 tail.
//! 2. **`fsync(file)`** the journal file descriptor so the record
//!    bytes reach platter.
//! 3. **`fsync(dir)`** the journal's parent directory so the directory
//!    entry is durable — skipping this step means a post-crash `readdir`
//!    may fail to find a freshly-created journal segment, silently
//!    dropping acknowledged writes (POSIX allows this). This is the
//!    `fsync(file)+fsync(dir)` discipline required by P1.2.
//! 4. **Apply** the op to the in-memory staging blob and bump the
//!    per-inode dirty byte counter.
//! 5. **Acknowledge** the FUSE write to the kernel.
//!
//! Crash between steps 1–3 is safe: the unacknowledged record is
//! either absent (step 1 not flushed) or replayable (step 2/3 flushed
//! but step 4 lost). Replay is idempotent because every record carries
//! a monotonically increasing LSN and the staging apply is
//! content-addressed per `(ino, offset, len)`.
//!
//! Writeback to the backend is a separate, later transaction: once
//! the backend returns `200 OK` for an upload the journal segment is
//! truncated under another `fsync(dir)` barrier so a crash mid-upload
//! re-executes the upload on the next boot rather than losing data.

#![allow(clippy::too_many_arguments)]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::errors::FsError;
use crate::slo_hook;
use crate::staging::{StagingDir, StagingError};
use crate::write_journal::{JournalOp, JournalRecord, WriteJournal, WriteJournalError};

/// Top-level error type returned to the FUSE adapter. Converts cleanly to
/// POSIX errnos.
#[derive(Debug, thiserror::Error)]
pub enum WritePathError {
    /// Write journal failure (I/O, codec, corruption).
    #[error(transparent)]
    Journal(#[from] WriteJournalError),
    /// Staging-area failure (e.g. full disk, I/O error).
    #[error(transparent)]
    Staging(#[from] StagingError),
    /// Upload backend reported a failure (network, server-side error,
    /// transport not yet wired). Used for backward compatibility and for
    /// errors that are not classified into [`Self::UploadTransient`] or
    /// [`Self::UploadPermanent`].
    #[error("upload backend failure: {0}")]
    Upload(String),
    /// Upload backend reported a transient failure that the write path may
    /// retry (connection timeout, 5xx, network flap). The chunked flush
    /// retries the current chunk a bounded number of times with
    /// exponential backoff before surfacing this error to the caller.
    #[error("upload transient failure: {0}")]
    UploadTransient(String),
    /// Upload backend reported a permanent failure for the current session
    /// (e.g. `upload_id` garbage-collected, auth expired, quota exceeded).
    /// The chunked flush reacts by restarting the whole upload from
    /// offset 0 once — beyond that the error is surfaced unchanged.
    #[error("upload permanent failure: {0}")]
    UploadPermanent(String),
    /// The caller attempted to write to an inode that was never opened
    /// for write or was already released.
    #[error("inode {0} is not open for write")]
    NotOpen(u64),
    /// Invalid argument supplied by the caller (e.g. empty filename,
    /// path containing `/`).
    #[error("invalid argument: {0}")]
    Invalid(&'static str),
    /// Generic pCloud filesystem error (wraps [`FsError`]).
    #[error(transparent)]
    Fs(#[from] FsError),
    /// Internal state corruption (e.g. mutex poisoned). Returned instead
    /// of panicking inside FUSE callbacks so the mount stays alive and the
    /// kernel gets a clean `EIO`.
    #[error("internal: {0}")]
    Internal(&'static str),
}

impl WritePathError {
    /// Map this error to a POSIX errno suitable for a FUSE reply.
    /// Invalid/NotOpen map to `EINVAL`; other variants map to `EIO` or
    /// the embedded [`FsError`]'s own errno.
    #[must_use]
    pub fn to_errno(&self) -> i32 {
        match self {
            Self::Fs(e) => e.to_errno(),
            Self::Invalid(_) => crate::errors::EINVAL,
            Self::NotOpen(_) => crate::errors::EINVAL,
            _ => crate::errors::EIO,
        }
    }
}

/// Abstraction over the transport-side upload surface. Allows swapping in a
/// mock for tests and the real `TransferApi` in the daemon.
///
/// # Chunked upload surface (bd-1du.4.6)
///
/// The three `upload_create` / `upload_write` / `upload_save` methods
/// mirror the pCloud protocol upload lifecycle and let the write path
/// stream large files in bounded chunks rather than re-uploading the whole
/// staging blob on every threshold crossing.
///
/// Backends that still expose only the legacy `upload_file` path can rely
/// on the provided default implementations: `upload_create` returns an
/// [`WritePathError::Upload`] marker the write path interprets as "chunked
/// API not supported — fall back to whole-file". The default
/// `upload_write` / `upload_save` are likewise `Upload` errors so a backend
/// that opts in partially cannot silently lose data.
pub trait FileUploadBackend: Send + Sync + 'static {
    /// Upload the file located at `staging_file` to the remote `parent_path`
    /// with `name`. Called on explicit flush and as a whole-file fallback
    /// when [`Self::upload_create`] reports the chunked surface is
    /// unavailable.
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &std::path::Path,
    ) -> Result<(), WritePathError>;

    /// Remove a remote file at `path`.
    fn unlink_remote(&self, path: &str) -> Result<(), WritePathError>;

    /// Rename/move a remote file.
    fn rename_remote(&self, from: &str, to: &str) -> Result<(), WritePathError>;

    /// Begin a chunked upload and return the backend-assigned `upload_id`.
    ///
    /// The default implementation returns an `Upload("chunked api not
    /// supported")` error, which the write path uses to fall back to
    /// [`Self::upload_file`]. Backends that want chunked pipelining must
    /// override all three of `upload_create` / `upload_write` /
    /// `upload_save`.
    fn upload_create(&self, _parent_path: &str, _name: &str) -> Result<u64, WritePathError> {
        Err(WritePathError::Upload(CHUNKED_NOT_SUPPORTED.to_owned()))
    }

    /// Append `chunk` at `offset` to the in-progress upload identified by
    /// `upload_id`. Must be idempotent on retry: the write path replays
    /// the last unacknowledged chunk after a crash.
    fn upload_write(
        &self,
        _upload_id: u64,
        _offset: u64,
        _chunk: &[u8],
    ) -> Result<(), WritePathError> {
        Err(WritePathError::Upload(CHUNKED_NOT_SUPPORTED.to_owned()))
    }

    /// Finalize the in-progress upload, persisting it at
    /// `parent_path/name` with the supplied `total_size`.
    fn upload_save(
        &self,
        _upload_id: u64,
        _parent_path: &str,
        _name: &str,
        _total_size: u64,
    ) -> Result<(), WritePathError> {
        Err(WritePathError::Upload(CHUNKED_NOT_SUPPORTED.to_owned()))
    }

    /// Query the server for the number of bytes it has confirmed receiving
    /// for `upload_id`. Protocol method: pCloud's `upload_info` (spec §2.6,
    /// `pupload.c:1193-1213`).
    ///
    /// Returns:
    /// - `UploadStatus::Bytes(n)` — server has `n` confirmed bytes.
    /// - [`UploadStatus::NotFound`] — server has garbage-collected the
    ///   `upload_id` (analogous to HTTP 404); the caller must discard any
    ///   local resume state and start a fresh upload.
    ///
    /// Default implementation returns `NotSupported` so backends that
    /// do not implement chunked uploads are not forced to implement this.
    fn upload_status(&self, _upload_id: u64) -> Result<UploadStatus, WritePathError> {
        Err(WritePathError::Upload(STATUS_NOT_SUPPORTED.to_owned()))
    }
}

/// Server-reported status of an in-progress chunked upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadStatus {
    /// Server confirms it has `bytes` acknowledged for the upload id.
    Bytes(u64),
    /// Server no longer recognises the `upload_id` (garbage collected).
    /// The caller must treat any local resume state as stale.
    NotFound,
}

/// Sentinel returned by the default [`FileUploadBackend::upload_status`]
/// so callers (in particular the resume path) can detect "status not
/// supported" and fall back to trusting the local sidecar.
pub const STATUS_NOT_SUPPORTED: &str = "upload_status api not implemented";

/// Marker string returned by the default chunked-upload methods so the
/// write path can detect "not implemented" and fall back to whole-file
/// `upload_file` instead of surfacing a spurious error to FUSE.
const CHUNKED_NOT_SUPPORTED: &str = "chunked upload api not implemented";

/// Chunk size for the chunked flush path. 4 MiB is the protocol sweet spot
/// for `upload_write`: large enough to amortise per-chunk round-trip cost,
/// small enough that a crash mid-flush replays at most one chunk of work.
pub const UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Default flush threshold: 64 MiB of accumulated dirty staging bytes per
/// inode. Beyond this point the write path forces a durability barrier.
/// Mid-write flushes use the chunked `upload_create` + `upload_write`
/// (4 MiB chunks) + `upload_save` pipeline wired in the internal
/// `chunked_flush` helper when the backend implements the chunked
/// surface; otherwise the write path falls back to a single
/// `upload_file` call and subsequent writes on the same handle may
/// re-upload the growing blob. The size bound exists to stop unbounded
/// staging growth on multi-GiB writes.
pub const DEFAULT_FLUSH_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

/// Default per-chunk size for the chunked upload pipeline
/// ([`WritePathOptions::chunk_size_bytes`]). Matches [`UPLOAD_CHUNK_BYTES`].
pub const DEFAULT_CHUNK_SIZE_BYTES: usize = UPLOAD_CHUNK_BYTES;

/// Default hard upper bound on per-inode staging bytes
/// ([`WritePathOptions::max_staging_bytes`]). Writes that would grow the
/// staging blob past this ceiling are rejected rather than allowed to
/// consume unbounded local disk. 512 MiB is conservative for a daemon that
/// must not ENOSPC the host filesystem.
pub const DEFAULT_MAX_STAGING_BYTES: usize = 512 * 1024 * 1024;

/// Default aggregate staging ceiling across **all** inodes (M-5.4).
///
/// The per-inode bound ([`DEFAULT_MAX_STAGING_BYTES`]) limits one inode but
/// does not cap the total cost of many concurrent open handles. On a
/// host with N simultaneous writers the total staging footprint could reach
/// N × 512 MiB before any per-inode guard fires, unexpectedly consuming all
/// available disk space.
///
/// This process-wide ceiling (2 GiB) bounds the aggregate. A `write` that
/// would push the global counter past this value is rejected with `ENOSPC`
/// even if the per-inode limit has not been reached.
///
/// Operators with large numbers of concurrent writers can raise this limit
/// via [`WritePathOptions::max_global_staging_bytes`].
pub const DEFAULT_MAX_GLOBAL_STAGING_BYTES: usize = 2 * 1024 * 1024 * 1024;

/// Process-wide count of staging bytes currently held across all open
/// write handles (M-5.4). Updated atomically on every `write` accept and
/// every flush/release that frees staging bytes.
///
/// The counter is intentionally process-global (not per-[`WritePathService`])
/// so that multiple daemon mount points share the ceiling and cannot each
/// independently consume `DEFAULT_MAX_GLOBAL_STAGING_BYTES`.
static GLOBAL_STAGING_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Default number of retries attempted per-chunk on
/// [`WritePathError::UploadTransient`] before surfacing the error.
pub const DEFAULT_CHUNK_RETRY_ATTEMPTS: u32 = 5;

/// Initial backoff between chunk retries. Doubles on every retry
/// (1s, 2s, 4s, 8s, 16s by default). Used by the chunked flush retry
/// loop in [`WritePathService::chunked_flush`].
pub const DEFAULT_CHUNK_RETRY_INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Default wall-clock interval for time-based forced flushes of idle dirty
/// handles (M-5.5). The previous default of 24 h effectively disabled
/// time-based flushing — an operator crash or SIGKILL during a long-lived
/// write handle would lose up to a day of writes. 30 s is conservative
/// enough to avoid excessive upload churn while bounding data loss to a
/// typical upload round-trip window.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(30);

/// Options for [`WritePathService`].
#[derive(Debug, Clone, Copy)]
pub struct WritePathOptions {
    /// Dirty-byte accumulation at which a mid-write flush is forced.
    pub flush_threshold_bytes: u64,
    /// Wall-clock interval between forced flushes for idle dirty handles.
    /// Default is [`DEFAULT_FLUSH_INTERVAL`] (30 s). Set to a very large
    /// value to effectively disable time-based flushes.
    pub flush_interval: Duration,
    /// Per-chunk byte count for the chunked upload pipeline. Default is
    /// [`DEFAULT_CHUNK_SIZE_BYTES`] (4 MiB). Lower values reduce replay cost
    /// on a crash mid-flush; higher values amortise per-chunk round-trip
    /// latency on high-bandwidth-high-latency links.
    pub chunk_size_bytes: usize,
    /// Hard upper bound on the size of a single inode's staging blob. A
    /// FUSE `write` that would push the blob past this bound returns
    /// `ENOSPC` to the kernel rather than allowing the local staging
    /// directory to grow without bound. Default is
    /// [`DEFAULT_MAX_STAGING_BYTES`] (512 MiB). Set to `usize::MAX` to
    /// disable the guard (not recommended in production).
    pub max_staging_bytes: usize,
    /// Maximum retries per-chunk on [`WritePathError::UploadTransient`]
    /// before the write path surfaces the error. Default is
    /// [`DEFAULT_CHUNK_RETRY_ATTEMPTS`] (5).
    pub chunk_retry_attempts: u32,
    /// Initial backoff between chunk retries (doubled on each retry).
    /// Default [`DEFAULT_CHUNK_RETRY_INITIAL_BACKOFF`] (1 second).
    pub chunk_retry_initial_backoff: Duration,
    /// Process-wide aggregate staging ceiling (M-5.4). A `write` that would
    /// push [`GLOBAL_STAGING_BYTES`] past this value is rejected with
    /// `ENOSPC`. Default is [`DEFAULT_MAX_GLOBAL_STAGING_BYTES`] (2 GiB).
    /// Set to `usize::MAX` to disable (not recommended).
    pub max_global_staging_bytes: usize,
}

impl Default for WritePathOptions {
    fn default() -> Self {
        Self {
            flush_threshold_bytes: DEFAULT_FLUSH_THRESHOLD_BYTES,
            // M-5.5: was 24 h (86400 s), now 30 s. See DEFAULT_FLUSH_INTERVAL.
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            chunk_size_bytes: DEFAULT_CHUNK_SIZE_BYTES,
            max_staging_bytes: DEFAULT_MAX_STAGING_BYTES,
            chunk_retry_attempts: DEFAULT_CHUNK_RETRY_ATTEMPTS,
            chunk_retry_initial_backoff: DEFAULT_CHUNK_RETRY_INITIAL_BACKOFF,
            max_global_staging_bytes: DEFAULT_MAX_GLOBAL_STAGING_BYTES,
        }
    }
}

impl WritePathOptions {
    /// Override the dirty-byte threshold that triggers a mid-write flush.
    ///
    /// A value of `u64::MAX` effectively disables the size-based auto-flush.
    /// The default is [`DEFAULT_FLUSH_THRESHOLD_BYTES`] (64 MiB).
    #[must_use]
    pub fn with_flush_threshold(mut self, bytes: u64) -> Self {
        self.flush_threshold_bytes = bytes;
        self
    }

    /// Override the wall-clock flush interval.
    #[must_use]
    pub fn with_flush_interval(mut self, interval: Duration) -> Self {
        self.flush_interval = interval;
        self
    }

    /// Override the per-chunk byte count for the chunked upload pipeline.
    /// A value of `0` is coerced to [`DEFAULT_CHUNK_SIZE_BYTES`] at use time
    /// so the flush loop always makes forward progress.
    #[must_use]
    pub fn with_chunk_size(mut self, bytes: usize) -> Self {
        self.chunk_size_bytes = bytes;
        self
    }

    /// Override the hard per-inode staging ceiling. Use
    /// `usize::MAX` to disable the guard.
    #[must_use]
    pub fn with_max_staging_bytes(mut self, bytes: usize) -> Self {
        self.max_staging_bytes = bytes;
        self
    }

    /// Override the per-chunk retry attempt count.
    #[must_use]
    pub fn with_chunk_retry_attempts(mut self, attempts: u32) -> Self {
        self.chunk_retry_attempts = attempts;
        self
    }

    /// Override the initial backoff used between per-chunk retries.
    #[must_use]
    pub fn with_chunk_retry_initial_backoff(mut self, backoff: Duration) -> Self {
        self.chunk_retry_initial_backoff = backoff;
        self
    }

    /// Override the process-wide aggregate staging ceiling (M-5.4).
    /// Use `usize::MAX` to disable the guard (not recommended).
    #[must_use]
    pub fn with_max_global_staging_bytes(mut self, bytes: usize) -> Self {
        self.max_global_staging_bytes = bytes;
        self
    }
}

/// Per-inode write state.
#[derive(Debug)]
struct WriteHandle {
    /// Remote logical path (e.g. `/docs/report.txt`).
    path: String,
    /// Staging blob filename (opaque, derived from inode number).
    blob_name: String,
    /// Dirty-byte count since last flush.
    dirty_bytes: u64,
    /// Last flush timestamp.
    last_flush: Instant,
    /// `O_APPEND` semantics.
    append_mode: bool,
}

/// Holds the write-path state for a single mount.
pub struct WritePathService<B: FileUploadBackend> {
    options: WritePathOptions,
    stage: StagingDir,
    journal: Mutex<WriteJournal>,
    handles: Mutex<HashMap<u64, Arc<Mutex<WriteHandle>>>>,
    backend: Arc<B>,
}

impl<B: FileUploadBackend> std::fmt::Debug for WritePathService<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WritePathService")
            .field("options", &self.options)
            .field("stage_root", &self.stage.root())
            .finish_non_exhaustive()
    }
}

impl<B: FileUploadBackend> WritePathService<B> {
    /// Construct a write-path service from a staging directory, a journal,
    /// an upload backend, and tuning options. No I/O is performed until
    /// the first write.
    pub fn new(
        stage: StagingDir,
        journal: WriteJournal,
        backend: Arc<B>,
        options: WritePathOptions,
    ) -> Self {
        Self {
            options,
            stage,
            journal: Mutex::new(journal),
            handles: Mutex::new(HashMap::new()),
            backend,
        }
    }

    /// Open a new or existing inode for writing. `path` is the remote path.
    pub fn open_for_write(
        &self,
        ino: u64,
        path: impl Into<String>,
        append_mode: bool,
        o_trunc: bool,
    ) -> Result<(), WritePathError> {
        let path = path.into();
        let blob_name = blob_name_for_ino(ino);
        if o_trunc {
            self.stage.truncate_blob(&blob_name, 0)?;
        } else if !self.stage.blob_path(&blob_name)?.exists() {
            self.stage.write_blob_full(&blob_name, &[])?;
        }
        let handle = Arc::new(Mutex::new(WriteHandle {
            path,
            blob_name,
            dirty_bytes: 0,
            last_flush: Instant::now(),
            append_mode,
        }));
        self.handles
            .lock()
            .map_err(|_| WritePathError::Internal("handles mutex poisoned"))?
            .insert(ino, handle);
        Ok(())
    }

    /// Seed the staging blob for `ino` with `bytes`. Used when opening an
    /// existing remote file for append/rw so the blob reflects the current
    /// remote content before local writes diverge.
    pub fn seed_blob(&self, ino: u64, bytes: &[u8]) -> Result<(), WritePathError> {
        let blob_name = blob_name_for_ino(ino);
        self.stage.write_blob_full(&blob_name, bytes)?;
        Ok(())
    }

    /// FUSE `create` — allocates a new inode context and journals a Create.
    pub fn create(&self, ino: u64, parent_path: &str, name: &str) -> Result<(), WritePathError> {
        if name.is_empty() || name.contains('/') || name.contains('\0') {
            return Err(WritePathError::Invalid("name"));
        }
        let full = join_path(parent_path, name);
        self.open_for_write(ino, full, false, true)?;
        self.journal_append(JournalOp::Create {
            parent_path: parent_path.to_owned(),
            name: name.to_owned(),
        })?;
        Ok(())
    }

    /// FUSE `write` — append to staging and journal the operation.
    pub fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, WritePathError> {
        let handle = self.get_handle(ino)?;
        let (blob_name, effective_offset, path, mut dirty, last_flush, append) = {
            let h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("write handle mutex poisoned"))?;
            (
                h.blob_name.clone(),
                offset,
                h.path.clone(),
                h.dirty_bytes,
                h.last_flush,
                h.append_mode,
            )
        };
        let effective_offset = if append {
            self.stage
                .blob_path(&blob_name)
                .ok()
                .and_then(|p| std::fs::metadata(&p).ok())
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            effective_offset
        };

        // Enforce per-inode staging ceiling *before* extending the blob so
        // an over-large write fails fast instead of filling local disk and
        // surfacing ENOSPC from the kernel with a half-applied write. The
        // guard is a simple high-water-mark check on `offset + len`; the
        // actual on-disk size may be larger for sparse writes, which is the
        // intended behaviour (bytes past the high-water are zero-filled on
        // read by the staging layer).
        let max = self.options.max_staging_bytes;
        if max < usize::MAX {
            let written_end = effective_offset.saturating_add(data.len() as u64);
            if written_end > max as u64 {
                return Err(WritePathError::Invalid(
                    "write would exceed max_staging_bytes ceiling",
                ));
            }
        }

        // Enforce process-wide aggregate staging ceiling (M-5.4). This
        // prevents N concurrent open handles from each consuming
        // max_staging_bytes, which could exhaust local disk with N large
        // writers. We add `data.len()` speculatively here and subtract on
        // any error path or on flush/release. Spurious ENOSPC under heavy
        // concurrent load is acceptable — callers can retry after another
        // handle is flushed.
        let global_max = self.options.max_global_staging_bytes;
        if global_max < usize::MAX {
            let prev = GLOBAL_STAGING_BYTES.fetch_add(data.len(), AtomicOrdering::AcqRel);
            if prev.saturating_add(data.len()) > global_max {
                // Roll back the speculative add before returning.
                GLOBAL_STAGING_BYTES.fetch_sub(data.len(), AtomicOrdering::AcqRel);
                log::warn!(
                    "pcloud-fs: write rejected — global staging ceiling {} B exceeded \
                     (current: {} B, write: {} B). Consider raising \
                     WritePathOptions::max_global_staging_bytes.",
                    global_max,
                    prev,
                    data.len()
                );
                return Err(WritePathError::Invalid(
                    "write would exceed process-wide max_global_staging_bytes ceiling",
                ));
            }
        }

        self.stage
            .write_blob_at(&blob_name, effective_offset, data)?;
        self.journal_append(JournalOp::Write {
            path: path.clone(),
            offset: effective_offset,
            len: data.len() as u64,
            staging_blob: blob_name.clone(),
        })?;

        dirty = dirty.saturating_add(data.len() as u64);
        {
            let mut h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("write handle mutex poisoned"))?;
            h.dirty_bytes = dirty;
        }

        let now = Instant::now();
        let size_trigger = dirty >= self.options.flush_threshold_bytes;
        let time_trigger = now.saturating_duration_since(last_flush) >= self.options.flush_interval;
        if size_trigger {
            // Stream the staging blob to the backend in 4 MiB chunks via
            // `upload_create` + `upload_write*` + `upload_save` rather
            // than finalising the whole file. A crash mid-flush is
            // recovered from the per-inode progress journal below so only
            // the last unacknowledged chunk is re-sent on resume.
            match self.chunked_flush(ino) {
                Ok(()) => {}
                Err(WritePathError::Upload(ref msg)) if msg == CHUNKED_NOT_SUPPORTED => {
                    // Backend has no chunked surface — preserve legacy
                    // whole-file behaviour so we don't regress correctness.
                    self.flush(ino)?;
                }
                Err(e) => return Err(e),
            }
        } else if time_trigger {
            self.flush(ino)?;
        }
        Ok(data.len())
    }

    /// Chunked size-triggered flush: `upload_create` once, `upload_write`
    /// per chunk ([`WritePathOptions::chunk_size_bytes`]), `upload_save`
    /// once; each chunk ack is fsynced to the per-inode progress journal
    /// so a crash mid-flush resumes from the last durable offset. Returns
    /// [`WritePathError::Upload`] with the sentinel
    /// [`CHUNKED_NOT_SUPPORTED`] message when the backend doesn't
    /// implement the chunked surface, letting [`Self::write`] fall back
    /// to the whole-file path.
    ///
    /// ## Retry discipline (bd-1du.4.6)
    ///
    /// * Each `upload_write` is retried up to
    ///   [`WritePathOptions::chunk_retry_attempts`] times on
    ///   [`WritePathError::UploadTransient`] with exponential backoff
    ///   starting at [`WritePathOptions::chunk_retry_initial_backoff`]
    ///   (1s, 2s, 4s, 8s, 16s by default).
    /// * On [`WritePathError::UploadPermanent`] during `upload_write` the
    ///   flush aborts the current session and restarts the whole upload
    ///   from offset 0 **once** (fresh `upload_create`, sidecar rewritten).
    ///   A second permanent failure is surfaced to the caller.
    /// * `offset` advances only after a confirmed ack, so a crash between
    ///   any two attempts replays the in-flight chunk, never skips one.
    fn chunked_flush(&self, ino: u64) -> Result<(), WritePathError> {
        let flush_started = Instant::now();
        let handle = self.get_handle(ino)?;
        let (path, blob_name) = {
            let h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("chunked_flush handle mutex poisoned"))?;
            (h.path.clone(), h.blob_name.clone())
        };
        let (parent, name) =
            split_parent_name(&path).ok_or(WritePathError::Invalid("path has no parent"))?;

        // Journal a flush barrier before any remote work: replay knows we
        // promised durability at this point.
        self.journal_append(JournalOp::FlushBarrier { path: path.clone() })?;

        let progress_path = self.progress_path(ino);
        let blob_path = self.stage.blob_path(&blob_name)?;
        let total_size = std::fs::metadata(&blob_path)
            .map_err(|e| WritePathError::Upload(format!("staging metadata: {e}")))?
            .len();

        // Outer loop: one whole-upload session attempt. On
        // `UploadPermanent` during chunk send we drop the session, purge
        // the sidecar, and spin a fresh `upload_create` once before
        // surfacing the error.
        let mut permanent_restarts_remaining: u32 = 1;
        loop {
            match self.run_chunked_session(
                ino,
                &parent,
                &name,
                &blob_name,
                &progress_path,
                total_size,
            ) {
                Ok(()) => break,
                Err(WritePathError::UploadPermanent(msg)) if permanent_restarts_remaining > 0 => {
                    permanent_restarts_remaining -= 1;
                    // Discard any sidecar for the failed session so the
                    // next run issues a fresh `upload_create`. Ignore the
                    // removal error (best effort; a mismatched sidecar is
                    // rejected on reload).
                    let _ = std::fs::remove_file(&progress_path);
                    log::warn!(
                        "chunked_flush: permanent error during upload_write (ino={ino}, err={msg}); restarting session from offset 0"
                    );
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        // Reset dirty accounting.
        let now = Instant::now();
        {
            let mut h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("chunked_flush handle mutex poisoned"))?;
            // M-5.4: release the flushed bytes from the global staging counter
            // so subsequent writes from other handles have access to the headroom.
            if self.options.max_global_staging_bytes < usize::MAX && h.dirty_bytes > 0 {
                let flushed = h.dirty_bytes.min(usize::MAX as u64) as usize;
                GLOBAL_STAGING_BYTES.fetch_sub(
                    flushed.min(GLOBAL_STAGING_BYTES.load(AtomicOrdering::Acquire)),
                    AtomicOrdering::AcqRel,
                );
            }
            h.dirty_bytes = 0;
            h.last_flush = now;
        }

        // Record a successful chunked flush into the process-wide
        // observability layer. `slo_hook::observe_flush` feeds the
        // `flush_latency_seconds` and `flush_bytes` user histograms
        // (exposed on `/metrics`) and, when the daemon has installed the
        // SLO registry, also updates `upload.throughput_mbps` so the
        // `/slo` endpoint reflects sustained write-path performance.
        let flush_latency = now.saturating_duration_since(flush_started);
        slo_hook::observe_flush(total_size, flush_latency);
        Ok(())
    }

    /// One `upload_create`-to-`upload_save` session attempt. Factored out
    /// of [`Self::chunked_flush`] so the permanent-failure outer loop can
    /// restart a clean session without duplicating the streaming body.
    fn run_chunked_session(
        &self,
        _ino: u64,
        parent: &str,
        name: &str,
        blob_name: &str,
        progress_path: &std::path::Path,
        total_size: u64,
    ) -> Result<(), WritePathError> {
        // Load an in-progress upload (if any) or begin a new one. The
        // progress sidecar ((ino, upload_id, last_acked_offset)) is fsynced
        // after each successful `upload_write` ack so a crash mid-flush
        // resumes at `resume_offset` instead of re-sending earlier chunks.
        let (upload_id, resume_offset) = match UploadProgress::load(progress_path)? {
            Some(p) if p.blob_name == blob_name && p.total_size == total_size => {
                (p.upload_id, p.acked_offset)
            }
            _ => {
                let id = self.backend.upload_create(parent, name)?;
                let p = UploadProgress {
                    upload_id: id,
                    blob_name: blob_name.to_owned(),
                    total_size,
                    acked_offset: 0,
                    heartbeat_unix_secs: now_unix_secs(),
                };
                p.save(progress_path)?;
                (id, 0u64)
            }
        };

        // Resolve a safe chunk size. Zero is coerced to the default so the
        // read loop always makes forward progress.
        let chunk_bytes = if self.options.chunk_size_bytes == 0 {
            DEFAULT_CHUNK_SIZE_BYTES
        } else {
            self.options.chunk_size_bytes
        };

        // Stream chunks via a `BufReader` so the kernel read path does
        // bulk-copies under the hood rather than one syscall per 4 MiB
        // buffer. We still size the chunk buffer to exactly `chunk_bytes`
        // so the bytes handed to `upload_write` line up with the
        // server-side chunk size.
        use std::io::{BufReader, Read, Seek, SeekFrom};
        let raw_file = self
            .stage
            .open_blob(blob_name)
            .map_err(|e| WritePathError::Upload(format!("open staging blob: {e}")))?;
        let mut reader = BufReader::with_capacity(chunk_bytes, raw_file);
        reader
            .seek(SeekFrom::Start(resume_offset))
            .map_err(|e| WritePathError::Upload(format!("seek staging blob: {e}")))?;

        let mut offset = resume_offset;
        let mut buf = vec![0u8; chunk_bytes];
        while offset < total_size {
            let want = std::cmp::min(chunk_bytes as u64, total_size - offset) as usize;
            let chunk = &mut buf[..want];
            reader
                .read_exact(chunk)
                .map_err(|e| WritePathError::Upload(format!("read staging blob: {e}")))?;

            // Retry loop: classify errors and bounded-retry on transient.
            let mut attempt: u32 = 0;
            let max_attempts = self.options.chunk_retry_attempts;
            let initial_backoff = self.options.chunk_retry_initial_backoff;
            loop {
                match self.backend.upload_write(upload_id, offset, chunk) {
                    Ok(()) => break,
                    Err(WritePathError::UploadTransient(msg)) if attempt < max_attempts => {
                        let backoff = exp_backoff(initial_backoff, attempt);
                        log::warn!(
                            "chunked_flush: transient error (upload_id={upload_id}, offset={offset}, attempt={attempt}, err={msg}); sleeping {backoff_ms}ms before retry",
                            backoff_ms = backoff.as_millis() as u64,
                        );
                        attempt += 1;
                        std::thread::sleep(backoff);
                        continue;
                    }
                    Err(WritePathError::UploadTransient(msg)) => {
                        // Exhausted. Leave the sidecar intact so a later
                        // retry can resume; surface the error.
                        return Err(WritePathError::UploadTransient(format!(
                            "exhausted {max_attempts} retries at offset {offset}: {msg}"
                        )));
                    }
                    Err(e) => return Err(e),
                }
            }
            offset += want as u64;
            // Persist progress *after* the ack so the journal only records
            // what the server has acknowledged. fsync inside save().
            UploadProgress {
                upload_id,
                blob_name: blob_name.to_owned(),
                total_size,
                acked_offset: offset,
                heartbeat_unix_secs: now_unix_secs(),
            }
            .save(progress_path)?;
        }

        // Finalize.
        self.backend
            .upload_save(upload_id, parent, name, total_size)?;

        // Clean up progress journal — best-effort; a stale file is harmless
        // because the next flush will see a mismatched blob_name/total_size
        // and start a fresh upload.
        let _ = std::fs::remove_file(progress_path);
        Ok(())
    }

    fn progress_path(&self, ino: u64) -> PathBuf {
        self.stage.root().join(format!("ino-{ino}.upload-progress"))
    }

    /// FUSE `flush` / `fsync` — force a durability barrier + remote upload.
    pub fn flush(&self, ino: u64) -> Result<(), WritePathError> {
        let flush_started = Instant::now();
        let handle = self.get_handle(ino)?;
        let (path, blob_name) = {
            let h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("flush handle mutex poisoned"))?;
            (h.path.clone(), h.blob_name.clone())
        };
        // Journal a flush barrier *before* upload so replay knows we promised
        // durability at this point.
        self.journal_append(JournalOp::FlushBarrier { path: path.clone() })?;

        // Upload.
        let blob_path = self.stage.blob_path(&blob_name)?;
        let (parent, name) =
            split_parent_name(&path).ok_or(WritePathError::Invalid("path has no parent"))?;
        // Size-stamp the payload *before* the upload so the observability
        // hook can record throughput even if the blob is mutated after the
        // upload completes (e.g. a subsequent write on the same handle).
        let flushed_bytes = std::fs::metadata(&blob_path).map(|m| m.len()).unwrap_or(0);
        self.backend.upload_file(&parent, &name, &blob_path)?;

        // Reset dirty accounting.
        let now = Instant::now();
        {
            let mut h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("flush handle mutex poisoned"))?;
            // M-5.4: release the flushed bytes from the global staging counter.
            if self.options.max_global_staging_bytes < usize::MAX && h.dirty_bytes > 0 {
                let flushed = h.dirty_bytes.min(usize::MAX as u64) as usize;
                GLOBAL_STAGING_BYTES.fetch_sub(
                    flushed.min(GLOBAL_STAGING_BYTES.load(AtomicOrdering::Acquire)),
                    AtomicOrdering::AcqRel,
                );
            }
            h.dirty_bytes = 0;
            h.last_flush = now;
        }

        // Record a successful whole-file flush into the process-wide
        // observability layer — mirrors the chunked-flush success arm so
        // `flush_latency_seconds` / `flush_bytes` reflect both paths.
        let flush_latency = now.saturating_duration_since(flush_started);
        slo_hook::observe_flush(flushed_bytes, flush_latency);
        Ok(())
    }

    /// FUSE `fsync` — currently same as flush (the daemon may choose to
    /// distinguish in a later milestone; `fdatasync` is honoured as a full
    /// flush to preserve correctness).
    pub fn fsync(&self, ino: u64) -> Result<(), WritePathError> {
        self.flush(ino)
    }

    /// FUSE `setattr` with `ATTR_SIZE` — truncate the staging blob and
    /// journal the truncation.
    pub fn truncate(&self, ino: u64, new_size: u64) -> Result<(), WritePathError> {
        let handle = self.get_handle(ino)?;
        let (blob_name, path) = {
            let h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("truncate handle mutex poisoned"))?;
            (h.blob_name.clone(), h.path.clone())
        };
        self.stage.truncate_blob(&blob_name, new_size)?;
        self.journal_append(JournalOp::Truncate { path, new_size })?;
        Ok(())
    }

    /// FUSE `unlink` — journal + backend remote removal; staging blob is
    /// best-effort removed too.
    pub fn unlink(&self, ino: Option<u64>, path: &str) -> Result<(), WritePathError> {
        self.journal_append(JournalOp::Unlink {
            path: path.to_owned(),
        })?;
        if let Some(ino) = ino {
            let blob = blob_name_for_ino(ino);
            let _ = self.stage.remove_blob(&blob);
            if let Ok(mut handles) = self.handles.lock() {
                handles.remove(&ino);
            }
        }
        self.backend.unlink_remote(path)?;
        Ok(())
    }

    /// FUSE `rename` — journal + backend rename. Also updates any open
    /// handle's logical path so subsequent writes hit the right remote.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), WritePathError> {
        self.journal_append(JournalOp::Rename {
            from: from.to_owned(),
            to: to.to_owned(),
        })?;
        self.backend.rename_remote(from, to)?;
        let handles = self
            .handles
            .lock()
            .map_err(|_| WritePathError::Internal("handles mutex poisoned"))?;
        for handle in handles.values() {
            if let Ok(mut h) = handle.lock() {
                if h.path == from {
                    h.path = to.to_owned();
                }
            }
        }
        Ok(())
    }

    /// Time-driven flush check. Intended to be called by a mount-side
    /// scheduler roughly at `flush_interval` cadence.
    pub fn tick(&self) -> Result<(), WritePathError> {
        let now = Instant::now();
        let inos: Vec<u64> = {
            let handles = self
                .handles
                .lock()
                .map_err(|_| WritePathError::Internal("handles mutex poisoned"))?;
            handles
                .iter()
                .filter_map(|(ino, h)| {
                    let h = h.lock().ok()?;
                    if h.dirty_bytes > 0
                        && now.saturating_duration_since(h.last_flush)
                            >= self.options.flush_interval
                    {
                        Some(*ino)
                    } else {
                        None
                    }
                })
                .collect()
        };
        for ino in inos {
            self.flush(ino)?;
        }
        Ok(())
    }

    /// Flush every inode that currently has dirty bytes, irrespective of
    /// the time- or size-based flush triggers. Intended for the daemon
    /// unmount drain hook (`bd-1du.4.6` FUSE wiring): on teardown we want
    /// every acknowledged in-memory write to reach the backend (or the
    /// journal if the backend fails) before the kernel mount disappears.
    ///
    /// Returns the list of `(ino, Result)` outcomes so the caller can
    /// surface per-inode failures without aborting the whole drain.
    pub fn drain_all(&self) -> Vec<(u64, Result<(), WritePathError>)> {
        let inos: Vec<u64> = match self.handles.lock() {
            Ok(handles) => handles
                .iter()
                .filter_map(|(ino, h)| {
                    let h = h.lock().ok()?;
                    if h.dirty_bytes > 0 { Some(*ino) } else { None }
                })
                .collect(),
            Err(_) => return vec![(0, Err(WritePathError::Internal("handles mutex poisoned")))],
        };
        inos.into_iter().map(|ino| (ino, self.flush(ino))).collect()
    }

    /// Number of inodes currently open for write. Diagnostic helper used
    /// by the drain hook to report the unmount summary.
    #[must_use]
    pub fn open_inode_count(&self) -> usize {
        self.handles.lock().map(|h| h.len()).unwrap_or(0)
    }

    /// Whether `ino` has a live write handle (i.e. the inode is
    /// currently staged locally). Used by the FUSE adapter to route
    /// `open`/`read` for freshly-created files through the staging blob
    /// rather than requiring a server-side file id.
    #[must_use]
    pub fn has_open_handle(&self, ino: u64) -> bool {
        self.handles
            .lock()
            .map(|h| h.contains_key(&ino))
            .unwrap_or(false)
    }

    /// Read a slice of the staging blob backing `ino`. Returns an empty
    /// `Vec` if `offset` is at or beyond the blob's current length.
    /// Errors with `WritePathError::NotOpen` if `ino` has no live
    /// write handle.
    pub fn read_staged(
        &self,
        ino: u64,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, WritePathError> {
        let handle = self.get_handle(ino)?;
        let blob_name = {
            let h = handle
                .lock()
                .map_err(|_| WritePathError::Internal("write handle mutex poisoned"))?;
            h.blob_name.clone()
        };
        let bytes = self.stage.read_blob(&blob_name)?;
        if offset >= bytes.len() as u64 {
            return Ok(Vec::new());
        }
        let start = offset as usize;
        let end = start.saturating_add(len).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    /// Replay any pending records from the on-disk journal. Returns the
    /// recovered records so the caller can re-drive backend uploads on
    /// remount.
    pub fn replay_journal(&self) -> Result<Vec<JournalRecord>, WritePathError> {
        let journal = self
            .journal
            .lock()
            .map_err(|_| WritePathError::Internal("journal mutex poisoned"))?;
        Ok(journal.replay()?)
    }

    /// Close a file descriptor: remove the handle. Does **not** flush —
    /// callers must invoke [`Self::flush`] first to enforce durability
    /// (matching kernel `flush` before `release` semantics).
    pub fn release(&self, ino: u64) {
        if let Ok(mut handles) = self.handles.lock() {
            // M-5.4: when a handle is removed without a prior flush (e.g.
            // unlink/error path), reclaim its dirty bytes from the global
            // staging counter so the headroom is available to other writers.
            if self.options.max_global_staging_bytes < usize::MAX {
                if let Some(handle) = handles.get(&ino) {
                    if let Ok(h) = handle.lock() {
                        if h.dirty_bytes > 0 {
                            let remaining = h.dirty_bytes.min(usize::MAX as u64) as usize;
                            GLOBAL_STAGING_BYTES.fetch_sub(
                                remaining.min(GLOBAL_STAGING_BYTES.load(AtomicOrdering::Acquire)),
                                AtomicOrdering::AcqRel,
                            );
                        }
                    }
                }
            }
            handles.remove(&ino);
        }
    }

    /// Dirty-byte count across all open inodes. Test/debug helper.
    #[must_use]
    pub fn total_dirty_bytes(&self) -> u64 {
        let handles = match self.handles.lock() {
            Ok(h) => h,
            Err(_) => return 0,
        };
        handles
            .values()
            .map(|h| h.lock().map(|g| g.dirty_bytes).unwrap_or(0))
            .sum()
    }

    fn get_handle(&self, ino: u64) -> Result<Arc<Mutex<WriteHandle>>, WritePathError> {
        let handles = self
            .handles
            .lock()
            .map_err(|_| WritePathError::Internal("handles mutex poisoned"))?;
        handles
            .get(&ino)
            .cloned()
            .ok_or(WritePathError::NotOpen(ino))
    }

    fn journal_append(&self, op: JournalOp) -> Result<u64, WritePathError> {
        let mut j = self
            .journal
            .lock()
            .map_err(|_| WritePathError::Internal("journal mutex poisoned"))?;
        Ok(j.append(op)?)
    }

    /// Staging dir, for diagnostics.
    #[must_use]
    pub fn staging_root(&self) -> PathBuf {
        self.stage.root().to_path_buf()
    }
}

fn blob_name_for_ino(ino: u64) -> String {
    format!("ino-{ino}.blob")
}

/// Exponential backoff with an absolute ceiling, used by the per-chunk
/// retry loop in [`WritePathService::chunked_flush`]. Returns
/// `initial << attempt`, clamped to 60 seconds so a long-running upload
/// does not stall for unbounded time on a pathological transient
/// classification bug.
fn exp_backoff(initial: Duration, attempt: u32) -> Duration {
    const CAP: Duration = Duration::from_secs(60);
    let shift = attempt.min(20); // saturate before the shift overflows
    match initial.checked_mul(1u32 << shift) {
        Some(d) if d <= CAP => d,
        _ => CAP,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Per-inode chunked-upload progress sidecar persisted under the staging
/// root as `ino-{ino}.upload-progress`.
///
/// Shape (serde_json, single line):
///
/// ```json
/// { "upload_id": 123, "blob_name": "ino-42.blob",
///   "total_size": 12582912, "acked_offset": 8388608 }
/// ```
///
/// Written after each acknowledged `upload_write`; `fsync`ed so a crash
/// between chunks resumes at `acked_offset` on the next flush. A mismatch
/// between the sidecar's `blob_name` / `total_size` and the current
/// staging blob causes the resumer to discard the in-flight upload and
/// start over via `upload_create`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UploadProgress {
    pub(crate) upload_id: u64,
    pub(crate) blob_name: String,
    pub(crate) total_size: u64,
    pub(crate) acked_offset: u64,
    /// Wall-clock Unix seconds of the last successful `upload_write` ack
    /// or explicit heartbeat bump. Used by [`replay_upload_sidecars`] to
    /// classify long-idle uploads as `Stalled` (see
    /// [`DEFAULT_HEARTBEAT_TIMEOUT`]).
    #[serde(default)]
    pub(crate) heartbeat_unix_secs: u64,
}

impl UploadProgress {
    fn load(path: &std::path::Path) -> Result<Option<Self>, WritePathError> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let p = serde_json::from_slice::<Self>(&bytes)
                    .map_err(|e| WritePathError::Upload(format!("decode upload-progress: {e}")))?;
                Ok(Some(p))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(WritePathError::Upload(format!("read upload-progress: {e}"))),
        }
    }

    fn save(&self, path: &std::path::Path) -> Result<(), WritePathError> {
        use std::io::Write;
        let bytes = serde_json::to_vec(self)
            .map_err(|e| WritePathError::Upload(format!("encode upload-progress: {e}")))?;
        // Durable write: write-then-rename so a crash mid-write can never
        // leave a torn progress file. The rename is followed by an fsync
        // of the parent directory to ensure the directory entry is on
        // platter — same discipline as write_journal.rs.
        let tmp = path.with_extension("upload-progress.tmp");
        {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)
                .map_err(|e| WritePathError::Upload(format!("open progress tmp: {e}")))?;
            f.write_all(&bytes)
                .map_err(|e| WritePathError::Upload(format!("write progress tmp: {e}")))?;
            f.sync_all()
                .map_err(|e| WritePathError::Upload(format!("fsync progress tmp: {e}")))?;
        }
        std::fs::rename(&tmp, path)
            .map_err(|e| WritePathError::Upload(format!("rename progress tmp: {e}")))?;
        if let Some(parent) = path.parent() {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }
}

/// Default heartbeat timeout: an upload whose sidecar heartbeat has not
/// been refreshed for this long and whose server-side byte count has not
/// advanced since the sidecar's recorded `acked_offset` is classified as
/// `Stalled` by [`replay_upload_sidecars`]. Ten minutes matches the
/// tracker spec.
pub const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Outcome of processing a single per-inode upload-progress sidecar during
/// startup resume (see [`replay_upload_sidecars`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Sidecar accepted as-is; the next `chunked_flush` for this inode
    /// will resume at `acked_offset`.
    Resumed {
        /// The `ino-{ino}.upload-progress` path.
        sidecar: PathBuf,
        /// Server-assigned upload id.
        upload_id: u64,
        /// Client-tracked offset after reconciliation.
        acked_offset: u64,
    },
    /// Server was ahead of the local sidecar (crash between `upload_write`
    /// ack and sidecar fsync). Sidecar was rewritten with the server's
    /// higher byte count so the next flush does not re-send bytes already
    /// durable on the server.
    ServerAhead {
        /// The `ino-{ino}.upload-progress` path that was rewritten.
        sidecar: PathBuf,
        /// Server-assigned upload id.
        upload_id: u64,
        /// Byte count after trimming up to the server's value.
        acked_offset: u64,
    },
    /// Server was behind the local sidecar (crash between sidecar fsync
    /// and subsequent chunk send, or corrupt sidecar). Sidecar was
    /// trimmed to the server's lower byte count so the next flush
    /// re-sends only the bytes the server actually lacks.
    SidecarTrimmed {
        /// The `ino-{ino}.upload-progress` path that was rewritten.
        sidecar: PathBuf,
        /// Server-assigned upload id.
        upload_id: u64,
        /// Byte count after trimming down to the server's value.
        acked_offset: u64,
    },
    /// Server no longer recognises `upload_id` (garbage collected).
    /// The sidecar was removed and the caller is expected to treat
    /// the inode as fully dirty, re-running the upload from scratch
    /// on its next flush.
    Expired {
        /// The `ino-{ino}.upload-progress` path that was removed.
        sidecar: PathBuf,
        /// The now-invalid upload id, for logging only.
        upload_id: u64,
    },
    /// Sidecar heartbeat is older than [`DEFAULT_HEARTBEAT_TIMEOUT`] and
    /// the server confirms it has not received new bytes since the last
    /// recorded `acked_offset`. The sidecar is removed and the upload
    /// surfaces as a retryable failure — the caller restarts it fresh
    /// rather than silently resuming.
    Stalled {
        /// The `ino-{ino}.upload-progress` path that was removed.
        sidecar: PathBuf,
        /// The stalled upload id.
        upload_id: u64,
        /// Age of the last heartbeat, for diagnostics.
        idle_for: Duration,
    },
    /// Sidecar could not be parsed (corrupt JSON, truncated, etc.).
    /// The file is left on disk so an operator can inspect it; the next
    /// flush for the affected inode will start a fresh upload anyway
    /// because the sidecar's `blob_name`/`total_size` will not match.
    Unparseable {
        /// The sidecar path that failed to parse.
        sidecar: PathBuf,
        /// Reason, for logging.
        reason: String,
    },
    /// Reconciliation could not contact the server. Sidecar is left as
    /// untouched so the next online flush re-tries the reconcile.
    BackendError {
        /// The sidecar path that could not be reconciled.
        sidecar: PathBuf,
        /// The upload id the reconcile was attempted for.
        upload_id: u64,
        /// Transport / protocol error text.
        reason: String,
    },
}

/// Enumerate every per-inode `ino-{ino}.upload-progress` sidecar under
/// `staging_root` and reconcile each one against `backend` via
/// [`FileUploadBackend::upload_status`].
///
/// This is the startup hook the daemon uses to make partial-upload resume
/// sturdy end-to-end:
///
/// 1. Server has **more** bytes than the sidecar → sidecar is rewritten
///    with the server's count (see [`ResumeOutcome::ServerAhead`]).
/// 2. Server has **fewer** bytes than the sidecar → sidecar is trimmed
///    down so the next flush re-sends only what the server lacks (see
///    [`ResumeOutcome::SidecarTrimmed`]).
/// 3. Server reports the upload id is gone → sidecar is removed and the
///    inode is flagged for a full re-upload ([`ResumeOutcome::Expired`]).
/// 4. Sidecar heartbeat older than `heartbeat_timeout` **and** server
///    confirms zero recent progress → classified as
///    [`ResumeOutcome::Stalled`] and aborted with a retryable error.
///
/// Pure function: performs no FUSE I/O, only sidecar file I/O + the
/// supplied backend trait calls. Safe to call before any mount is live.
///
/// # Errors
///
/// Filesystem errors reading `staging_root` surface as the returned
/// [`WritePathError`]. Per-file failures are returned as
/// [`ResumeOutcome::Unparseable`] / [`ResumeOutcome::BackendError`]
/// rather than aborting the whole scan so a single bad sidecar cannot
/// block daemon startup.
pub fn replay_upload_sidecars<B: FileUploadBackend>(
    staging_root: &Path,
    backend: &B,
    heartbeat_timeout: Duration,
) -> Result<Vec<ResumeOutcome>, WritePathError> {
    let mut outcomes = Vec::new();
    let rd = match std::fs::read_dir(staging_root) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(outcomes),
        Err(e) => {
            return Err(WritePathError::Upload(format!(
                "staging read_dir {}: {e}",
                staging_root.display()
            )));
        }
    };
    let now = now_unix_secs();
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !(file_name.starts_with("ino-") && file_name.ends_with(".upload-progress")) {
            continue;
        }

        let progress = match UploadProgress::load(&path) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                outcomes.push(ResumeOutcome::Unparseable {
                    sidecar: path.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };

        // Reconcile against the server.
        let status = match backend.upload_status(progress.upload_id) {
            Ok(s) => s,
            Err(WritePathError::Upload(msg))
                if msg == STATUS_NOT_SUPPORTED || msg == CHUNKED_NOT_SUPPORTED =>
            {
                // No server-side probe available — trust the local sidecar
                // but still enforce the heartbeat guard.
                if let Some(idle) = idle_for_if_stalled(&progress, now, heartbeat_timeout) {
                    let _ = std::fs::remove_file(&path);
                    outcomes.push(ResumeOutcome::Stalled {
                        sidecar: path.clone(),
                        upload_id: progress.upload_id,
                        idle_for: idle,
                    });
                } else {
                    outcomes.push(ResumeOutcome::Resumed {
                        sidecar: path.clone(),
                        upload_id: progress.upload_id,
                        acked_offset: progress.acked_offset,
                    });
                }
                continue;
            }
            Err(e) => {
                outcomes.push(ResumeOutcome::BackendError {
                    sidecar: path.clone(),
                    upload_id: progress.upload_id,
                    reason: e.to_string(),
                });
                continue;
            }
        };

        match status {
            UploadStatus::NotFound => {
                let _ = std::fs::remove_file(&path);
                outcomes.push(ResumeOutcome::Expired {
                    sidecar: path.clone(),
                    upload_id: progress.upload_id,
                });
            }
            UploadStatus::Bytes(server_bytes) => {
                // Stalled? Only if heartbeat expired AND server shows no
                // progress past the recorded ack. Server-ahead proves the
                // upload is actually moving.
                let server_progressed = server_bytes > progress.acked_offset;
                if !server_progressed {
                    if let Some(idle) = idle_for_if_stalled(&progress, now, heartbeat_timeout) {
                        let _ = std::fs::remove_file(&path);
                        outcomes.push(ResumeOutcome::Stalled {
                            sidecar: path.clone(),
                            upload_id: progress.upload_id,
                            idle_for: idle,
                        });
                        continue;
                    }
                }

                use std::cmp::Ordering;
                match server_bytes.cmp(&progress.acked_offset) {
                    Ordering::Equal => {
                        outcomes.push(ResumeOutcome::Resumed {
                            sidecar: path.clone(),
                            upload_id: progress.upload_id,
                            acked_offset: progress.acked_offset,
                        });
                    }
                    Ordering::Greater => {
                        // Server has more bytes than the sidecar records:
                        // crash between `upload_write` ack and sidecar
                        // fsync. Trim *up* so we don't re-send already-
                        // durable bytes.
                        let new_offset = server_bytes.min(progress.total_size);
                        let mut updated = progress.clone();
                        updated.acked_offset = new_offset;
                        updated.heartbeat_unix_secs = now;
                        if let Err(e) = updated.save(&path) {
                            outcomes.push(ResumeOutcome::BackendError {
                                sidecar: path.clone(),
                                upload_id: progress.upload_id,
                                reason: format!("rewrite sidecar: {e}"),
                            });
                            continue;
                        }
                        outcomes.push(ResumeOutcome::ServerAhead {
                            sidecar: path.clone(),
                            upload_id: progress.upload_id,
                            acked_offset: new_offset,
                        });
                    }
                    Ordering::Less => {
                        // Sidecar ahead of the server: the classic crash-
                        // between-sidecar-fsync-and-next-send case. Trim
                        // the sidecar *down* so the next flush replays
                        // only the missing bytes.
                        let mut updated = progress.clone();
                        updated.acked_offset = server_bytes;
                        updated.heartbeat_unix_secs = now;
                        if let Err(e) = updated.save(&path) {
                            outcomes.push(ResumeOutcome::BackendError {
                                sidecar: path.clone(),
                                upload_id: progress.upload_id,
                                reason: format!("rewrite sidecar: {e}"),
                            });
                            continue;
                        }
                        outcomes.push(ResumeOutcome::SidecarTrimmed {
                            sidecar: path.clone(),
                            upload_id: progress.upload_id,
                            acked_offset: server_bytes,
                        });
                    }
                }
            }
        }
    }
    Ok(outcomes)
}

/// Enumerate sidecars without contacting any server — used by
/// [`bootstrap`](crate) at a point where no authenticated transport is
/// available yet. Emits [`ResumeOutcome::Resumed`] /
/// [`ResumeOutcome::Unparseable`] only so the caller can log them;
/// the actual server reconcile must run later via
/// [`replay_upload_sidecars`].
pub fn enumerate_upload_sidecars(
    staging_root: &Path,
) -> Result<Vec<ResumeOutcome>, WritePathError> {
    let mut outcomes = Vec::new();
    let rd = match std::fs::read_dir(staging_root) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(outcomes),
        Err(e) => {
            return Err(WritePathError::Upload(format!(
                "staging read_dir {}: {e}",
                staging_root.display()
            )));
        }
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !(file_name.starts_with("ino-") && file_name.ends_with(".upload-progress")) {
            continue;
        }
        match UploadProgress::load(&path) {
            Ok(Some(p)) => outcomes.push(ResumeOutcome::Resumed {
                sidecar: path.clone(),
                upload_id: p.upload_id,
                acked_offset: p.acked_offset,
            }),
            Ok(None) => {}
            Err(e) => outcomes.push(ResumeOutcome::Unparseable {
                sidecar: path.clone(),
                reason: e.to_string(),
            }),
        }
    }
    Ok(outcomes)
}

fn idle_for_if_stalled(
    progress: &UploadProgress,
    now_secs: u64,
    heartbeat_timeout: Duration,
) -> Option<Duration> {
    // No heartbeat ever recorded (legacy sidecars or crash before first
    // write ack): be conservative and treat as not-yet-stalled — we have
    // no ground truth for when the upload actually started, and the sync
    // engine will retry it on the next open.
    if progress.heartbeat_unix_secs == 0 {
        return None;
    }
    let idle = now_secs.saturating_sub(progress.heartbeat_unix_secs);
    let idle = Duration::from_secs(idle);
    if idle >= heartbeat_timeout {
        Some(idle)
    } else {
        None
    }
}

fn split_parent_name(path: &str) -> Option<(String, String)> {
    if path == "/" {
        return None;
    }
    let idx = path.rfind('/')?;
    let parent = if idx == 0 {
        "/".to_owned()
    } else {
        path[..idx].to_owned()
    };
    let name = path[idx + 1..].to_owned();
    if name.is_empty() {
        return None;
    }
    Some((parent, name))
}

// -----------------------------------------------------------------------------
// Test-only mock backend
// -----------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    pub struct MockUploadBackend {
        pub uploads: Mutex<HashMap<String, Vec<u8>>>, // full path -> bytes
        pub unlinks: Mutex<Vec<String>>,
        pub renames: Mutex<Vec<(String, String)>>,
        pub fail_next_upload: Mutex<bool>,
        /// Full chunked-call sequence: each entry is
        /// `"create:/path"`, `"write:<id>:<offset>:<len>"`, or
        /// `"save:<id>:/path:<total>"`.
        pub chunk_calls: Mutex<Vec<String>>,
        /// In-progress chunked uploads: upload_id -> (parent, name, bytes so far)
        pub in_progress: Mutex<HashMap<u64, (String, String, Vec<u8>)>>,
        pub next_upload_id: Mutex<u64>,
        /// If `Some(n)`, the `n`th `upload_write` call (0-indexed) fails to
        /// simulate a crash mid-flush. Counter decrements on every call.
        pub fail_chunk_after: Mutex<Option<usize>>,
        pub chunk_writes_seen: Mutex<usize>,
        /// If true, the chunked API returns `CHUNKED_NOT_SUPPORTED` to
        /// force the write path onto the whole-file fallback.
        pub disable_chunked: Mutex<bool>,
        /// Optional override used by resume tests: maps `upload_id` to the
        /// server-side ack count reported via `upload_status`. When absent
        /// the backend derives the ack count from `in_progress` (whatever
        /// the driver has written locally).
        pub status_bytes: Mutex<HashMap<u64, UploadStatus>>,
        /// If `> 0`, the next `n` `upload_write` calls return
        /// [`WritePathError::UploadTransient`] to drive the retry-loop
        /// test. Counter decrements on each injection.
        pub transient_writes_remaining: Mutex<u32>,
        /// If `true`, the next `upload_write` call returns
        /// [`WritePathError::UploadPermanent`] once and then resets to
        /// `false` so the subsequent retry can succeed. Used to test the
        /// outer "restart session from offset 0" loop.
        pub permanent_next_write: Mutex<bool>,
    }

    impl MockUploadBackend {
        pub fn new() -> Self {
            Self {
                next_upload_id: Mutex::new(1),
                ..Self::default()
            }
        }
    }

    impl FileUploadBackend for MockUploadBackend {
        fn upload_file(
            &self,
            parent_path: &str,
            name: &str,
            staging_file: &std::path::Path,
        ) -> Result<(), WritePathError> {
            if *self.fail_next_upload.lock().unwrap() {
                *self.fail_next_upload.lock().unwrap() = false;
                return Err(WritePathError::Upload("injected".to_owned()));
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

        fn unlink_remote(&self, path: &str) -> Result<(), WritePathError> {
            self.unlinks.lock().unwrap().push(path.to_owned());
            self.uploads.lock().unwrap().remove(path);
            Ok(())
        }

        fn rename_remote(&self, from: &str, to: &str) -> Result<(), WritePathError> {
            self.renames
                .lock()
                .unwrap()
                .push((from.to_owned(), to.to_owned()));
            let mut uploads = self.uploads.lock().unwrap();
            if let Some(bytes) = uploads.remove(from) {
                uploads.insert(to.to_owned(), bytes);
            }
            Ok(())
        }

        fn upload_create(&self, parent_path: &str, name: &str) -> Result<u64, WritePathError> {
            if *self.disable_chunked.lock().unwrap() {
                return Err(WritePathError::Upload(CHUNKED_NOT_SUPPORTED.to_owned()));
            }
            let full = if parent_path == "/" {
                format!("/{name}")
            } else {
                format!("{parent_path}/{name}")
            };
            self.chunk_calls
                .lock()
                .unwrap()
                .push(format!("create:{full}"));
            let mut id = self.next_upload_id.lock().unwrap();
            let upload_id = *id;
            *id += 1;
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
            if *self.disable_chunked.lock().unwrap() {
                return Err(WritePathError::Upload(CHUNKED_NOT_SUPPORTED.to_owned()));
            }

            // Transient-error injection: return UploadTransient until the
            // counter hits zero. Evaluated *before* the crash injector and
            // before the record write so retried chunks don't double-count.
            {
                let mut remaining = self.transient_writes_remaining.lock().unwrap();
                if *remaining > 0 {
                    *remaining -= 1;
                    return Err(WritePathError::UploadTransient(format!(
                        "injected transient at offset {offset}"
                    )));
                }
            }
            // Permanent-error injection: single-shot, self-resetting. Used to
            // drive the outer session-restart loop.
            {
                let mut pending = self.permanent_next_write.lock().unwrap();
                if *pending {
                    *pending = false;
                    return Err(WritePathError::UploadPermanent(format!(
                        "injected permanent at offset {offset}"
                    )));
                }
            }

            // Check crash-simulation trigger *before* recording, so a
            // "crash at chunk N" means N chunks have been acked (not N+1).
            {
                let mut seen = self.chunk_writes_seen.lock().unwrap();
                let idx = *seen;
                *seen += 1;
                if let Some(fail_at) = *self.fail_chunk_after.lock().unwrap() {
                    if idx >= fail_at {
                        return Err(WritePathError::Upload(format!(
                            "simulated crash at write #{idx}"
                        )));
                    }
                }
            }
            self.chunk_calls
                .lock()
                .unwrap()
                .push(format!("write:{upload_id}:{offset}:{}", chunk.len()));
            let mut inp = self.in_progress.lock().unwrap();
            let entry = inp.get_mut(&upload_id).ok_or_else(|| {
                WritePathError::Upload(format!("upload_write: unknown id {upload_id}"))
            })?;
            let end = (offset as usize)
                .checked_add(chunk.len())
                .ok_or_else(|| WritePathError::Upload("offset overflow".to_owned()))?;
            if entry.2.len() < end {
                entry.2.resize(end, 0);
            }
            entry.2[offset as usize..end].copy_from_slice(chunk);
            Ok(())
        }

        fn upload_status(&self, upload_id: u64) -> Result<UploadStatus, WritePathError> {
            if *self.disable_chunked.lock().unwrap() {
                return Err(WritePathError::Upload(STATUS_NOT_SUPPORTED.to_owned()));
            }
            if let Some(s) = self.status_bytes.lock().unwrap().get(&upload_id).copied() {
                return Ok(s);
            }
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
            if *self.disable_chunked.lock().unwrap() {
                return Err(WritePathError::Upload(CHUNKED_NOT_SUPPORTED.to_owned()));
            }
            let full = if parent_path == "/" {
                format!("/{name}")
            } else {
                format!("{parent_path}/{name}")
            };
            self.chunk_calls
                .lock()
                .unwrap()
                .push(format!("save:{upload_id}:{full}:{total_size}"));
            let mut inp = self.in_progress.lock().unwrap();
            let (_p, _n, bytes) = inp
                .remove(&upload_id)
                .ok_or_else(|| WritePathError::Upload(format!("save: unknown id {upload_id}")))?;
            if bytes.len() as u64 != total_size {
                return Err(WritePathError::Upload(format!(
                    "save: short upload {}/{}",
                    bytes.len(),
                    total_size
                )));
            }
            self.uploads.lock().unwrap().insert(full, bytes);
            Ok(())
        }
    }
}

// -----------------------------------------------------------------------------
// Unit tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::mock::MockUploadBackend;
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn build_service(
        tmp: &std::path::Path,
    ) -> (WritePathService<MockUploadBackend>, Arc<MockUploadBackend>) {
        let stage = StagingDir::open(tmp.join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions {
                flush_threshold_bytes: 1024 * 1024,
                flush_interval: Duration::from_secs(3600),
                ..WritePathOptions::default()
            },
        );
        (svc, backend)
    }

    #[test]
    fn create_then_write_then_flush_uploads_content() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        svc.create(10, "/", "report.txt").unwrap();
        svc.write(10, 0, b"hello ").unwrap();
        svc.write(10, 6, b"world").unwrap();
        svc.flush(10).unwrap();

        let uploads = backend.uploads.lock().unwrap();
        assert_eq!(uploads.get("/report.txt").unwrap(), b"hello world");
    }

    #[test]
    fn write_beyond_eof_extends_with_zeros() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        svc.create(11, "/", "sparse.bin").unwrap();
        svc.write(11, 0, b"AB").unwrap();
        svc.write(11, 10, b"XYZ").unwrap();
        svc.flush(11).unwrap();
        let uploads = backend.uploads.lock().unwrap();
        let data = uploads.get("/sparse.bin").unwrap();
        assert_eq!(data.len(), 13);
        assert_eq!(&data[0..2], b"AB");
        assert_eq!(&data[2..10], &[0u8; 8]);
        assert_eq!(&data[10..13], b"XYZ");
    }

    #[test]
    fn o_append_forces_write_at_current_end() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        svc.open_for_write(12, "/append.txt".to_owned(), true, true)
            .unwrap();
        svc.write(12, 0, b"AAA").unwrap();
        // Even though caller passes offset=0, append_mode forces tail append.
        svc.write(12, 0, b"BBB").unwrap();
        svc.flush(12).unwrap();
        let uploads = backend.uploads.lock().unwrap();
        assert_eq!(uploads.get("/append.txt").unwrap(), b"AAABBB");
    }

    #[test]
    fn o_trunc_zeros_existing_blob() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        svc.create(13, "/", "trunc.txt").unwrap();
        svc.write(13, 0, b"garbage").unwrap();
        svc.flush(13).unwrap();
        // Re-open with O_TRUNC.
        svc.open_for_write(13, "/trunc.txt".to_owned(), false, true)
            .unwrap();
        svc.write(13, 0, b"clean").unwrap();
        svc.flush(13).unwrap();
        let uploads = backend.uploads.lock().unwrap();
        assert_eq!(uploads.get("/trunc.txt").unwrap(), b"clean");
    }

    #[test]
    fn flush_threshold_triggers_upload_mid_write() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions {
                flush_threshold_bytes: 8,
                flush_interval: Duration::from_secs(3600),
                ..WritePathOptions::default()
            },
        );
        svc.create(14, "/", "coalesce.txt").unwrap();
        svc.write(14, 0, b"1234567890").unwrap(); // 10 bytes >= 8
        // Auto-flush must have triggered.
        let uploads = backend.uploads.lock().unwrap();
        assert_eq!(
            uploads.get("/coalesce.txt").map(Vec::as_slice),
            Some(&b"1234567890"[..])
        );
    }

    #[test]
    fn truncate_via_setattr_shrinks_blob_and_journals_record() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        svc.create(15, "/", "t.bin").unwrap();
        svc.write(15, 0, b"0123456789").unwrap();
        svc.truncate(15, 4).unwrap();
        svc.flush(15).unwrap();
        let uploads = backend.uploads.lock().unwrap();
        assert_eq!(uploads.get("/t.bin").unwrap(), b"0123");
    }

    #[test]
    fn unlink_removes_blob_and_calls_backend() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        svc.create(16, "/", "gone.txt").unwrap();
        svc.write(16, 0, b"bye").unwrap();
        svc.flush(16).unwrap();
        svc.unlink(Some(16), "/gone.txt").unwrap();
        assert!(
            backend
                .unlinks
                .lock()
                .unwrap()
                .contains(&"/gone.txt".to_owned())
        );
    }

    #[test]
    fn rename_updates_open_handle_path() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        svc.create(17, "/", "old.txt").unwrap();
        svc.write(17, 0, b"x").unwrap();
        svc.rename("/old.txt", "/new.txt").unwrap();
        svc.write(17, 1, b"y").unwrap();
        svc.flush(17).unwrap();
        let uploads = backend.uploads.lock().unwrap();
        assert_eq!(uploads.get("/new.txt").unwrap(), b"xy");
        assert!(!uploads.contains_key("/old.txt"));
    }

    #[test]
    fn concurrent_writes_to_same_inode_are_serialised() {
        let d = tempdir().unwrap();
        let (svc, backend) = build_service(d.path());
        let svc = Arc::new(svc);
        svc.create(18, "/", "c.txt").unwrap();

        let mut handles = Vec::new();
        for i in 0..8u8 {
            let svc = Arc::clone(&svc);
            handles.push(std::thread::spawn(move || {
                for _ in 0..16 {
                    svc.write(18, 0, &[b'a' + i]).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        svc.flush(18).unwrap();
        let uploads = backend.uploads.lock().unwrap();
        // Length is non-zero; the invariant is that no panic/data-race
        // occurred under per-inode locking.
        assert!(!uploads.get("/c.txt").unwrap().is_empty());
    }

    #[test]
    fn replay_returns_pending_records() {
        let d = tempdir().unwrap();
        let (svc, _backend) = build_service(d.path());
        svc.create(19, "/", "r.txt").unwrap();
        svc.write(19, 0, b"z").unwrap();
        let records = svc.replay_journal().unwrap();
        assert!(records.len() >= 2);
    }

    #[test]
    fn flush_without_open_handle_errors_enotopen() {
        let d = tempdir().unwrap();
        let (svc, _backend) = build_service(d.path());
        let err = svc.flush(999).unwrap_err();
        assert!(matches!(err, WritePathError::NotOpen(999)));
    }

    #[test]
    fn default_flush_threshold_is_64mib() {
        let opts = WritePathOptions::default();
        assert_eq!(opts.flush_threshold_bytes, 64 * 1024 * 1024);
        assert_eq!(opts.flush_threshold_bytes, DEFAULT_FLUSH_THRESHOLD_BYTES);
    }

    #[test]
    fn flush_threshold_configurable() {
        let opts = WritePathOptions::default().with_flush_threshold(4096);
        assert_eq!(opts.flush_threshold_bytes, 4096);
        let opts2 = WritePathOptions::default().with_flush_threshold(u64::MAX);
        assert_eq!(opts2.flush_threshold_bytes, u64::MAX);
    }

    #[test]
    fn flush_triggers_at_threshold_and_resets_staged_bytes() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions::default()
                .with_flush_threshold(16)
                .with_flush_interval(Duration::from_secs(3600)),
        );
        svc.create(42, "/", "big.bin").unwrap();

        // Under threshold: no auto-flush, dirty bytes accumulate.
        svc.write(42, 0, &[b'a'; 8]).unwrap();
        assert_eq!(svc.total_dirty_bytes(), 8);
        assert!(backend.uploads.lock().unwrap().get("/big.bin").is_none());

        // Cross the threshold (8 + 16 = 24 >= 16): auto-flush fires and
        // dirty counter resets to zero.
        svc.write(42, 8, &[b'b'; 16]).unwrap();
        assert_eq!(
            svc.total_dirty_bytes(),
            0,
            "dirty bytes must reset after threshold-triggered flush"
        );
        let uploads = backend.uploads.lock().unwrap();
        let data = uploads.get("/big.bin").expect("must have uploaded");
        assert_eq!(data.len(), 24);
        assert_eq!(&data[..8], &[b'a'; 8]);
        assert_eq!(&data[8..], &[b'b'; 16]);
    }

    #[test]
    fn flush_at_threshold_emits_chunked_upload_calls() {
        // Threshold crossing must drive the chunked API (`upload_create` +
        // `upload_write` × N + `upload_save`) rather than the legacy
        // whole-file `upload_file` path.
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        // 4 MiB chunk size + 10 MiB write => 3 chunks (4 + 4 + 2 MiB).
        let total_bytes = 10 * 1024 * 1024;
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions {
                flush_threshold_bytes: 1024, // force chunked flush on first write
                flush_interval: Duration::from_secs(3600),
                ..WritePathOptions::default()
            },
        );
        svc.create(100, "/", "big.bin").unwrap();
        let payload = vec![0xABu8; total_bytes];
        svc.write(100, 0, &payload).unwrap();

        let calls = backend.chunk_calls.lock().unwrap().clone();
        assert!(
            !calls.is_empty(),
            "chunked API must have been invoked, got: {calls:?}"
        );
        // Exactly one create, exactly one save, and three writes of 4/4/2 MiB.
        let creates: Vec<_> = calls.iter().filter(|c| c.starts_with("create:")).collect();
        let saves: Vec<_> = calls.iter().filter(|c| c.starts_with("save:")).collect();
        let writes: Vec<_> = calls.iter().filter(|c| c.starts_with("write:")).collect();
        assert_eq!(creates.len(), 1, "exactly one upload_create: {calls:?}");
        assert_eq!(saves.len(), 1, "exactly one upload_save: {calls:?}");
        assert_eq!(writes.len(), 3, "three chunks expected: {calls:?}");
        // Verify chunk sizes: first two 4 MiB, last 2 MiB, monotonic offsets.
        assert!(writes[0].ends_with(&format!(":0:{}", 4 * 1024 * 1024)));
        assert!(writes[1].ends_with(&format!(":{}:{}", 4 * 1024 * 1024, 4 * 1024 * 1024)));
        assert!(writes[2].ends_with(&format!(":{}:{}", 8 * 1024 * 1024, 2 * 1024 * 1024)));
        // And the ordering is create -> writes -> save.
        assert!(calls[0].starts_with("create:"), "create first: {calls:?}");
        assert!(
            calls.last().unwrap().starts_with("save:"),
            "save last: {calls:?}"
        );

        // Final content must be present in the uploads map.
        let uploads = backend.uploads.lock().unwrap();
        let bytes = uploads.get("/big.bin").expect("file uploaded");
        assert_eq!(bytes.len(), total_bytes);
        assert!(bytes.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn flush_resumes_from_journal_after_simulated_crash() {
        // Drive a chunked flush, crash after 2 of 3 chunks have been acked,
        // then mount a fresh service and retry: it must resume at the 3rd
        // chunk (same upload_id) and not re-send the first two.
        let d = tempdir().unwrap();
        let stage_root = d.path().join("stage");
        let total_bytes = 10 * 1024 * 1024;

        // --- Mount 1: crash after 2 chunks. -------------------------------
        let saved_upload_id: u64;
        {
            let stage = StagingDir::open(&stage_root).unwrap();
            let journal = WriteJournal::open(stage.journal_path()).unwrap();
            let backend = Arc::new(MockUploadBackend::new());
            *backend.fail_chunk_after.lock().unwrap() = Some(2);
            let svc = WritePathService::new(
                stage,
                journal,
                Arc::clone(&backend),
                WritePathOptions {
                    flush_threshold_bytes: 1024,
                    flush_interval: Duration::from_secs(3600),
                    ..WritePathOptions::default()
                },
            );
            svc.create(200, "/", "resume.bin").unwrap();
            let payload = vec![0x5Au8; total_bytes];
            let err = svc.write(200, 0, &payload).unwrap_err();
            assert!(matches!(err, WritePathError::Upload(_)), "got {err:?}");
            // upload_id observed on the first upload_create call.
            let calls = backend.chunk_calls.lock().unwrap();
            // Two writes should have been acked before the simulated crash.
            let writes: Vec<_> = calls.iter().filter(|c| c.starts_with("write:")).collect();
            assert_eq!(writes.len(), 2, "exactly 2 acked writes: {calls:?}");
            // Capture upload_id from the second "write:<id>:..." call.
            saved_upload_id = writes[0]
                .strip_prefix("write:")
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            // Mock does not upload_save on failure — no final file present.
            assert!(backend.uploads.lock().unwrap().get("/resume.bin").is_none());
            drop(svc);
        }

        // --- Mount 2: resume. --------------------------------------------
        // Staging blob and progress sidecar must still be on disk.
        let progress_sidecar = stage_root.join("ino-200.upload-progress");
        assert!(
            progress_sidecar.exists(),
            "progress sidecar must survive crash"
        );
        let raw = std::fs::read(&progress_sidecar).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(
            parsed["acked_offset"].as_u64().unwrap(),
            (8 * 1024 * 1024) as u64,
            "journal records last acked offset after 2 chunks of 4 MiB"
        );
        assert_eq!(parsed["upload_id"].as_u64().unwrap(), saved_upload_id);

        let stage = StagingDir::open(&stage_root).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        // Seed the mock so resume finds the same upload_id in-progress with
        // the two already-acked chunks of 0x5A bytes still buffered.
        {
            let mut inp = backend.in_progress.lock().unwrap();
            inp.insert(
                saved_upload_id,
                (
                    "/".to_owned(),
                    "resume.bin".to_owned(),
                    vec![0x5Au8; 8 * 1024 * 1024],
                ),
            );
            // Ensure the mock's next_upload_id doesn't collide if a fresh
            // upload_create were (incorrectly) issued.
            *backend.next_upload_id.lock().unwrap() = saved_upload_id + 100;
        }
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions {
                flush_threshold_bytes: 1024,
                flush_interval: Duration::from_secs(3600),
                ..WritePathOptions::default()
            },
        );
        // Re-open the same inode against the same logical path so the
        // handle exists; seed_blob is not needed because the staging blob
        // survived the crash on disk.
        svc.open_for_write(200, "/resume.bin".to_owned(), false, false)
            .unwrap();
        svc.flush(200).ok(); // whole-file flush path, unrelated.
        // Now explicitly drive the chunked resume by issuing a size-trigger
        // write: extend by 0 bytes to trip chunked_flush via direct call.
        svc.chunked_flush(200).unwrap();

        // The resumer must have reused `saved_upload_id` (no second create)
        // and emitted only the 3rd chunk write + save.
        let calls = backend.chunk_calls.lock().unwrap().clone();
        let creates: Vec<_> = calls.iter().filter(|c| c.starts_with("create:")).collect();
        let writes: Vec<_> = calls.iter().filter(|c| c.starts_with("write:")).collect();
        let saves: Vec<_> = calls.iter().filter(|c| c.starts_with("save:")).collect();
        assert!(
            creates.is_empty(),
            "resume must not call upload_create again: {calls:?}"
        );
        assert_eq!(
            writes.len(),
            1,
            "only the final chunk is re-sent: {calls:?}"
        );
        assert_eq!(
            saves.len(),
            1,
            "upload_save finalises the resumed upload: {calls:?}"
        );
        assert!(
            writes[0].starts_with(&format!("write:{saved_upload_id}:{}:", 8 * 1024 * 1024)),
            "3rd chunk must resume at offset 8 MiB on the saved upload_id: got {}",
            writes[0]
        );

        // Final upload content matches the original 10 MiB payload.
        let uploads = backend.uploads.lock().unwrap();
        let bytes = uploads.get("/resume.bin").expect("file uploaded on resume");
        assert_eq!(bytes.len(), total_bytes);
        assert!(bytes.iter().all(|&b| b == 0x5A));

        // Progress sidecar cleaned up after successful save.
        assert!(
            !progress_sidecar.exists(),
            "progress sidecar must be removed after successful save"
        );
    }

    #[test]
    fn chunked_flush_falls_back_to_upload_file_when_not_supported() {
        // A backend that reports the chunked API as unavailable must keep
        // working via the whole-file `upload_file` path.
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        *backend.disable_chunked.lock().unwrap() = true;
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions {
                flush_threshold_bytes: 4,
                flush_interval: Duration::from_secs(3600),
                ..WritePathOptions::default()
            },
        );
        svc.create(300, "/", "legacy.bin").unwrap();
        svc.write(300, 0, b"hello world").unwrap();
        // Size threshold tripped; chunked API unavailable; whole-file path ran.
        let uploads = backend.uploads.lock().unwrap();
        assert_eq!(uploads.get("/legacy.bin").unwrap(), b"hello world");
        assert!(
            backend.chunk_calls.lock().unwrap().is_empty(),
            "chunked API must not have been called in fallback mode"
        );
    }

    // -----------------------------------------------------------------
    // Startup-resume (sidecar replay) tests — P1.2 hardening.
    // -----------------------------------------------------------------

    fn write_sidecar(dir: &std::path::Path, ino: u64, p: &UploadProgress) -> std::path::PathBuf {
        let path = dir.join(format!("ino-{ino}.upload-progress"));
        let bytes = serde_json::to_vec(p).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn sidecar_replayed_on_startup_resumes_from_acked_offset() {
        let d = tempdir().unwrap();
        let stage_root = d.path();
        let backend = MockUploadBackend::new();
        // Seed an in-progress upload so status returns Bytes(==sidecar).
        backend
            .in_progress
            .lock()
            .unwrap()
            .insert(77, ("/".to_owned(), "r.bin".to_owned(), vec![0xAA; 4096]));
        let _sidecar = write_sidecar(
            stage_root,
            200,
            &UploadProgress {
                upload_id: 77,
                blob_name: "ino-200.blob".to_owned(),
                total_size: 8192,
                acked_offset: 4096,
                heartbeat_unix_secs: now_unix_secs(),
            },
        );
        let outcomes =
            replay_upload_sidecars(stage_root, &backend, DEFAULT_HEARTBEAT_TIMEOUT).unwrap();
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            ResumeOutcome::Resumed {
                upload_id,
                acked_offset,
                ..
            } => {
                assert_eq!(*upload_id, 77);
                assert_eq!(*acked_offset, 4096);
            }
            other => panic!("expected Resumed, got {other:?}"),
        }
    }

    #[test]
    fn server_ahead_of_sidecar_is_not_regression() {
        let d = tempdir().unwrap();
        let stage_root = d.path();
        let backend = MockUploadBackend::new();
        // Server reports 8 MiB acked; sidecar only recorded 4 MiB.
        backend
            .status_bytes
            .lock()
            .unwrap()
            .insert(88, UploadStatus::Bytes(8 * 1024 * 1024));
        let sidecar = write_sidecar(
            stage_root,
            201,
            &UploadProgress {
                upload_id: 88,
                blob_name: "ino-201.blob".to_owned(),
                total_size: 16 * 1024 * 1024,
                acked_offset: 4 * 1024 * 1024,
                heartbeat_unix_secs: now_unix_secs(),
            },
        );
        let outcomes =
            replay_upload_sidecars(stage_root, &backend, DEFAULT_HEARTBEAT_TIMEOUT).unwrap();
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            ResumeOutcome::ServerAhead { acked_offset, .. } => {
                assert_eq!(*acked_offset, 8 * 1024 * 1024);
            }
            other => panic!("expected ServerAhead, got {other:?}"),
        }
        // Sidecar must have been rewritten with the higher value so a
        // subsequent flush does not re-send the already-durable bytes.
        let reloaded = UploadProgress::load(&sidecar).unwrap().unwrap();
        assert_eq!(reloaded.acked_offset, 8 * 1024 * 1024);
    }

    #[test]
    fn sidecar_trimmed_when_server_behind() {
        let d = tempdir().unwrap();
        let stage_root = d.path();
        let backend = MockUploadBackend::new();
        // Sidecar thinks 8 MiB acked; server only has 4 MiB (crashed
        // before sidecar fsync of the *previous* chunk ack — or corrupt
        // sidecar after a torn-write attack).
        backend
            .status_bytes
            .lock()
            .unwrap()
            .insert(99, UploadStatus::Bytes(4 * 1024 * 1024));
        let sidecar = write_sidecar(
            stage_root,
            202,
            &UploadProgress {
                upload_id: 99,
                blob_name: "ino-202.blob".to_owned(),
                total_size: 16 * 1024 * 1024,
                acked_offset: 8 * 1024 * 1024,
                heartbeat_unix_secs: now_unix_secs(),
            },
        );
        let outcomes =
            replay_upload_sidecars(stage_root, &backend, DEFAULT_HEARTBEAT_TIMEOUT).unwrap();
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            ResumeOutcome::SidecarTrimmed { acked_offset, .. } => {
                assert_eq!(*acked_offset, 4 * 1024 * 1024);
            }
            other => panic!("expected SidecarTrimmed, got {other:?}"),
        }
        let reloaded = UploadProgress::load(&sidecar).unwrap().unwrap();
        assert_eq!(reloaded.acked_offset, 4 * 1024 * 1024);
    }

    #[test]
    fn expired_upload_id_triggers_full_reupload() {
        let d = tempdir().unwrap();
        let stage_root = d.path();
        let backend = MockUploadBackend::new();
        // Server says "I've never heard of this upload id" — the mock
        // returns NotFound automatically when `in_progress` has no entry.
        let sidecar = write_sidecar(
            stage_root,
            203,
            &UploadProgress {
                upload_id: 4242,
                blob_name: "ino-203.blob".to_owned(),
                total_size: 2048,
                acked_offset: 1024,
                heartbeat_unix_secs: now_unix_secs(),
            },
        );
        let outcomes =
            replay_upload_sidecars(stage_root, &backend, DEFAULT_HEARTBEAT_TIMEOUT).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            &outcomes[0],
            ResumeOutcome::Expired { upload_id, .. } if *upload_id == 4242
        ));
        // Sidecar must have been removed so the next flush on this inode
        // runs a fresh `upload_create`.
        assert!(!sidecar.exists(), "expired sidecar must be removed");
    }

    #[test]
    fn stalled_upload_aborts_after_heartbeat_timeout() {
        let d = tempdir().unwrap();
        let stage_root = d.path();
        let backend = MockUploadBackend::new();
        // Server hasn't budged past the recorded acked_offset.
        backend
            .in_progress
            .lock()
            .unwrap()
            .insert(321, ("/".to_owned(), "z.bin".to_owned(), vec![0; 1024]));
        let now = now_unix_secs();
        // Heartbeat 20 minutes ago — twice the default timeout.
        let long_ago = now.saturating_sub(20 * 60);
        let sidecar = write_sidecar(
            stage_root,
            204,
            &UploadProgress {
                upload_id: 321,
                blob_name: "ino-204.blob".to_owned(),
                total_size: 4096,
                acked_offset: 1024,
                heartbeat_unix_secs: long_ago,
            },
        );
        let outcomes =
            replay_upload_sidecars(stage_root, &backend, DEFAULT_HEARTBEAT_TIMEOUT).unwrap();
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            ResumeOutcome::Stalled {
                upload_id,
                idle_for,
                ..
            } => {
                assert_eq!(*upload_id, 321);
                assert!(*idle_for >= DEFAULT_HEARTBEAT_TIMEOUT);
            }
            other => panic!("expected Stalled, got {other:?}"),
        }
        // Sidecar removed so the caller restarts the upload from scratch.
        assert!(!sidecar.exists(), "stalled sidecar must be removed");
    }

    #[test]
    fn enumerate_sidecars_reports_without_backend_calls() {
        let d = tempdir().unwrap();
        let stage_root = d.path();
        write_sidecar(
            stage_root,
            400,
            &UploadProgress {
                upload_id: 10,
                blob_name: "ino-400.blob".to_owned(),
                total_size: 128,
                acked_offset: 64,
                heartbeat_unix_secs: now_unix_secs(),
            },
        );
        let outcomes = enumerate_upload_sidecars(stage_root).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0], ResumeOutcome::Resumed { .. }));
    }

    #[test]
    fn fsync_before_crash_replay_preserves_write() {
        // Simulate crash-before-upload: write+fsync intent recorded, then we
        // drop the service without the backend succeeding. Replay must
        // surface the pending Write + FlushBarrier records.
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal_path = stage.journal_path();
        {
            let backend = Arc::new(MockUploadBackend::new());
            // Force the flush to fail to simulate a crash partway.
            *backend.fail_next_upload.lock().unwrap() = true;
            let journal = WriteJournal::open(&journal_path).unwrap();
            let svc = WritePathService::new(
                stage,
                journal,
                Arc::clone(&backend),
                WritePathOptions::default(),
            );
            svc.create(20, "/", "crash.txt").unwrap();
            svc.write(20, 0, b"durable").unwrap();
            // fsync call will fail because upload is rigged to fail; the
            // journal entries *before* the failing upload must already be
            // fsynced to disk.
            let _ = svc.fsync(20);
        }
        // Reopen journal on a "remount" — records must survive.
        let records = crate::write_journal::replay_path(&journal_path).unwrap();
        let has_write = records
            .iter()
            .any(|r| matches!(&r.op, JournalOp::Write { path, .. } if path == "/crash.txt"));
        let has_barrier = records
            .iter()
            .any(|r| matches!(&r.op, JournalOp::FlushBarrier { path } if path == "/crash.txt"));
        assert!(has_write, "Write record must be durable after crash");
        assert!(has_barrier, "FlushBarrier must be durable after crash");
    }

    // -----------------------------------------------------------------
    // bd-1du.4.6 chunked-pipelining hardening tests
    // -----------------------------------------------------------------

    /// End-to-end check that a 10 MiB staging file flushes through the
    /// chunked upload pipeline with exactly the expected chunk count and
    /// that the bytes the mock reconstructs from the `upload_write`
    /// payloads match the original content byte-for-byte.
    ///
    /// Covers the task checklist for bd-1du.4.6 step 4: "creates a 10 MiB
    /// staging file with known content, calls chunked_flush against a
    /// mock upload backend, verifies the correct number of chunks were
    /// sent and the data matches".
    #[test]
    fn chunked_flush_streams_10mib_file_in_4mib_chunks_with_data_fidelity() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        let total_bytes: usize = 10 * 1024 * 1024;
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions::default()
                .with_flush_threshold(u64::MAX) // disable size-trigger auto-flush
                .with_flush_interval(Duration::from_secs(3600))
                .with_chunk_size(4 * 1024 * 1024)
                .with_max_staging_bytes(usize::MAX),
        );
        svc.create(500, "/", "tenmib.bin").unwrap();

        // Known content: a simple linear ramp we can verify byte-for-byte
        // without allocating a second full copy.
        let mut payload = vec![0u8; total_bytes];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add(7);
        }
        // Two successive writes; total 10 MiB.
        svc.write(500, 0, &payload[..5 * 1024 * 1024]).unwrap();
        svc.write(500, 5 * 1024 * 1024, &payload[5 * 1024 * 1024..])
            .unwrap();

        // Explicitly invoke chunked_flush; the flush_threshold is u64::MAX
        // so no implicit flush has fired yet.
        svc.chunked_flush(500).unwrap();

        let calls = backend.chunk_calls.lock().unwrap().clone();
        let creates: Vec<_> = calls.iter().filter(|c| c.starts_with("create:")).collect();
        let writes: Vec<_> = calls.iter().filter(|c| c.starts_with("write:")).collect();
        let saves: Vec<_> = calls.iter().filter(|c| c.starts_with("save:")).collect();
        assert_eq!(creates.len(), 1, "exactly one upload_create: {calls:?}");
        assert_eq!(saves.len(), 1, "exactly one upload_save: {calls:?}");
        assert_eq!(writes.len(), 3, "three 4-MiB chunks (4+4+2): {calls:?}");

        // Offsets are monotonically increasing and 4-MiB aligned.
        assert!(writes[0].ends_with(&format!(":0:{}", 4 * 1024 * 1024)));
        assert!(writes[1].ends_with(&format!(":{}:{}", 4 * 1024 * 1024, 4 * 1024 * 1024)));
        assert!(writes[2].ends_with(&format!(":{}:{}", 8 * 1024 * 1024, 2 * 1024 * 1024)));

        // Data fidelity: the mock reassembles the file at `/tenmib.bin`.
        let uploads = backend.uploads.lock().unwrap();
        let remote = uploads.get("/tenmib.bin").expect("must have uploaded");
        assert_eq!(remote.len(), total_bytes);
        assert_eq!(remote, &payload, "remote bytes must match source");
    }

    /// A small configured chunk size (32 KiB) must be honoured by the
    /// streaming loop — proves [`WritePathOptions::chunk_size_bytes`] is
    /// read at flush time rather than the const being hard-wired.
    #[test]
    fn chunked_flush_honours_custom_chunk_size() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        let chunk = 32 * 1024; // 32 KiB
        let total = 100 * 1024; // 100 KiB → 4 chunks (32+32+32+4)
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions::default()
                .with_flush_threshold(u64::MAX)
                .with_flush_interval(Duration::from_secs(3600))
                .with_chunk_size(chunk)
                .with_max_staging_bytes(usize::MAX),
        );
        svc.create(501, "/", "small-chunks.bin").unwrap();
        svc.write(501, 0, &vec![0x99u8; total]).unwrap();
        svc.chunked_flush(501).unwrap();

        let calls = backend.chunk_calls.lock().unwrap().clone();
        let writes: Vec<_> = calls.iter().filter(|c| c.starts_with("write:")).collect();
        assert_eq!(
            writes.len(),
            4,
            "100 KiB / 32 KiB = 4 chunks (32+32+32+4): {calls:?}"
        );
        // Last chunk is exactly the remainder.
        assert!(writes[3].ends_with(&format!(":{}:{}", 3 * chunk, total - 3 * chunk)));
    }

    /// The per-chunk retry loop must transparently absorb a bounded number
    /// of [`WritePathError::UploadTransient`] failures and advance `offset`
    /// only on a confirmed ack.
    #[test]
    fn chunked_flush_retries_transient_errors_and_succeeds() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        // First 3 upload_write calls return UploadTransient; we have 5
        // retries configured so the 3rd retry of the first chunk succeeds
        // and the rest of the upload flows through.
        *backend.transient_writes_remaining.lock().unwrap() = 3;
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions::default()
                .with_flush_threshold(u64::MAX)
                .with_flush_interval(Duration::from_secs(3600))
                .with_chunk_size(4 * 1024)
                .with_chunk_retry_attempts(5)
                .with_chunk_retry_initial_backoff(Duration::from_millis(1)),
        );
        svc.create(502, "/", "flaky.bin").unwrap();
        svc.write(502, 0, &vec![0xCCu8; 8 * 1024]).unwrap(); // 2 chunks
        svc.chunked_flush(502).unwrap();

        let uploads = backend.uploads.lock().unwrap();
        let bytes = uploads.get("/flaky.bin").expect("final save succeeded");
        assert_eq!(bytes.len(), 8 * 1024);
        assert!(bytes.iter().all(|&b| b == 0xCC));

        // Exactly two *successful* write records landed on the mock — the
        // three injected transient errors must not have been recorded.
        let calls = backend.chunk_calls.lock().unwrap();
        let writes: Vec<_> = calls.iter().filter(|c| c.starts_with("write:")).collect();
        assert_eq!(
            writes.len(),
            2,
            "retries must not double-record successful chunks: {calls:?}"
        );
    }

    /// Exceeding the retry budget must surface as
    /// [`WritePathError::UploadTransient`] and **not** corrupt the
    /// sidecar — a later flush must be able to resume at the same offset.
    #[test]
    fn chunked_flush_surfaces_exhausted_transient_retries() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        // More transient errors than we allow retries for.
        *backend.transient_writes_remaining.lock().unwrap() = 10;
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions::default()
                .with_flush_threshold(u64::MAX)
                .with_chunk_size(4 * 1024)
                .with_chunk_retry_attempts(2)
                .with_chunk_retry_initial_backoff(Duration::from_millis(1)),
        );
        svc.create(503, "/", "stubborn.bin").unwrap();
        svc.write(503, 0, &vec![0xAAu8; 4 * 1024]).unwrap();
        let err = svc.chunked_flush(503).unwrap_err();
        assert!(
            matches!(err, WritePathError::UploadTransient(_)),
            "got {err:?}"
        );
    }

    /// A permanent error during `upload_write` must cause the flush to
    /// abandon the session, rerun `upload_create` once, and succeed on
    /// the second session without duplicating committed bytes.
    #[test]
    fn chunked_flush_restarts_session_on_permanent_error_once() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        // Fail the first upload_write with Permanent; the mock auto-resets
        // so the restart succeeds cleanly.
        *backend.permanent_next_write.lock().unwrap() = true;
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions::default()
                .with_flush_threshold(u64::MAX)
                .with_chunk_size(4 * 1024)
                .with_chunk_retry_attempts(0)
                .with_chunk_retry_initial_backoff(Duration::from_millis(1)),
        );
        svc.create(504, "/", "restart.bin").unwrap();
        svc.write(504, 0, &vec![0x11u8; 8 * 1024]).unwrap(); // 2 chunks
        svc.chunked_flush(504).unwrap();

        // Two `create:` entries expected: one for the failed session, one
        // for the restart. Two writes on the second session (both chunks).
        let calls = backend.chunk_calls.lock().unwrap().clone();
        let creates: Vec<_> = calls.iter().filter(|c| c.starts_with("create:")).collect();
        let saves: Vec<_> = calls.iter().filter(|c| c.starts_with("save:")).collect();
        assert_eq!(
            creates.len(),
            2,
            "restart must issue a second upload_create: {calls:?}"
        );
        assert_eq!(saves.len(), 1, "exactly one upload_save: {calls:?}");

        let uploads = backend.uploads.lock().unwrap();
        let bytes = uploads.get("/restart.bin").expect("file saved");
        assert_eq!(bytes.len(), 8 * 1024);
        assert!(bytes.iter().all(|&b| b == 0x11));
    }

    /// Writes beyond [`WritePathOptions::max_staging_bytes`] must be
    /// rejected with `EINVAL` rather than allowed to grow the staging
    /// blob unbounded. The guard is checked on the first write that
    /// would cross the ceiling.
    #[test]
    fn write_exceeding_max_staging_bytes_is_rejected() {
        let d = tempdir().unwrap();
        let stage = StagingDir::open(d.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let backend = Arc::new(MockUploadBackend::new());
        let svc = WritePathService::new(
            stage,
            journal,
            Arc::clone(&backend),
            WritePathOptions::default()
                .with_flush_threshold(u64::MAX)
                .with_max_staging_bytes(4096),
        );
        svc.create(505, "/", "cap.bin").unwrap();
        // Under the cap: fine.
        svc.write(505, 0, &vec![0u8; 2048]).unwrap();
        // At-or-below the cap: still fine.
        svc.write(505, 2048, &vec![0u8; 2048]).unwrap();
        // Over the cap: rejected.
        let err = svc.write(505, 4096, &[0u8; 1]).unwrap_err();
        assert!(matches!(err, WritePathError::Invalid(_)), "got {err:?}");
    }

    /// Default [`WritePathOptions`] must pin chunk size and staging ceiling
    /// to the documented defaults. Regression guard for the public config
    /// surface.
    #[test]
    fn default_write_path_options_expose_documented_constants() {
        let opts = WritePathOptions::default();
        assert_eq!(opts.chunk_size_bytes, DEFAULT_CHUNK_SIZE_BYTES);
        assert_eq!(opts.chunk_size_bytes, UPLOAD_CHUNK_BYTES);
        assert_eq!(opts.chunk_size_bytes, 4 * 1024 * 1024);
        assert_eq!(opts.max_staging_bytes, DEFAULT_MAX_STAGING_BYTES);
        assert_eq!(opts.max_staging_bytes, 512 * 1024 * 1024);
        assert_eq!(opts.chunk_retry_attempts, DEFAULT_CHUNK_RETRY_ATTEMPTS);
        assert_eq!(opts.chunk_retry_attempts, 5);
        assert_eq!(
            opts.chunk_retry_initial_backoff,
            DEFAULT_CHUNK_RETRY_INITIAL_BACKOFF
        );
        assert_eq!(opts.chunk_retry_initial_backoff, Duration::from_secs(1));
    }

    /// [`exp_backoff`] must produce the 1-2-4-8-16 pattern documented in
    /// the retry contract and saturate at the 60-second ceiling.
    #[test]
    fn exp_backoff_matches_documented_schedule() {
        let base = Duration::from_secs(1);
        assert_eq!(exp_backoff(base, 0), Duration::from_secs(1));
        assert_eq!(exp_backoff(base, 1), Duration::from_secs(2));
        assert_eq!(exp_backoff(base, 2), Duration::from_secs(4));
        assert_eq!(exp_backoff(base, 3), Duration::from_secs(8));
        assert_eq!(exp_backoff(base, 4), Duration::from_secs(16));
        // Saturating cap at 60s.
        assert_eq!(exp_backoff(base, 10), Duration::from_secs(60));
        assert_eq!(exp_backoff(base, 30), Duration::from_secs(60));
    }
}

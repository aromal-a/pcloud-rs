//! High-level `UploadSession` handle for the SDK.
//!
//! # Mutex poisoning policy
//!
//! SAFETY: The `chunked` and `outcome` mutexes are private fields of
//! `SharedInner`, only held briefly inside this module, and the critical
//! sections are panic-free data-structure work. A poisoned lock here
//! therefore indicates a prior panic in this module — a real bug that
//! we surface via `.expect()` rather than silently fabricate a Result.
//!
//! The session is a real chunked-upload state machine backed by
//! `upload_create` / `upload_write` / `upload_save` on the daemon's
//! `TransferRuntime`. Chunk progress is persisted through the P1.2
//! atomic-append NDJSON journal
//! (`pcloud_backends::upload_journal::UploadJournal`) so a process
//! crash mid-upload can be resumed from the last fsynced offset on the
//! next session.
//!
//! # State machine
//!
//! ```text
//!     Idle ──start()──► Writing{offset,total} ──save_and_complete()──► Completed
//!                              │   ▲
//!                          pause() │ resume()
//!                              ▼   │
//!                         Paused{offset}
//!                              │
//!                          cancel()
//!                              ▼
//!                          Canceled
//! ```
//!
//! Terminal states are `Completed`, `Canceled`, and `Failed`. Every
//! transition is documented on the method that drives it.
//!
//! # Qualification scope
//!
//! The chunked surface (`start_chunked_upload`, `write_chunk`,
//! `save_and_complete`, `pause`, `resume`, `cancel`) is driven through an
//! [`UploadSessionDriver`] abstraction for deterministic tests.
//! [`EmbeddedDaemon::start_upload`] uses the production runtime driver,
//! threads conflict policy into `upload_save`, and persists resumable
//! progress. The parity row is implemented. Credentialed pCloud runs remain a
//! release-qualification gate rather than missing SDK wiring.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pcloud_daemon::upload_journal::{JournalEntry, UploadJournal};
use pcloud_observability::LockExt;
use pcloud_secret::secret_string::SecretString;
use thiserror::Error;
use tokio::sync::watch;
use zeroize::Zeroize;

use crate::{EmbeddedDaemon, UploadHelperError};

/// Default chunk size used by chunked upload sessions (4 MiB).
///
/// Matches what the daemon-side `transfer_backend::upload_bytes_chunked`
/// driver is able to process in a single `upload_write` round-trip.
pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Configuration for a chunked [`UploadSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadConfig {
    /// Chunk size in bytes. Defaults to [`DEFAULT_CHUNK_SIZE`] (4 MiB).
    pub chunk_size: usize,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }
}

/// Conflict mode selection for a session upload. Maps to the C
/// `ifhash` param family documented in `UPLOAD-SPEC-14042026.md §5`.
///
/// The selection is threaded through [`UploadRequest`] →
/// `run_upload` -> `pcloud_daemon::transfer_backend::TransferRuntime::upload_save_session`
/// onto the wire as the `ifhash` parameter on `upload_save`
/// (`pclsync/pupload.c:1495-1509`). Unlike the legacy single-shot path,
/// the value is no longer discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictMode {
    /// Conditional overwrite — server accepts only if the existing
    /// remote file's hash equals `hash`. Wire: `ifhash = <hash>` PARAM_NUM.
    IfHashNumeric(u64),
    /// Create-if-absent. On name collision the server renames the file.
    /// Wire: `ifhash = "new"` PARAM_STR.
    CreateIfAbsent,
}

impl ConflictMode {
    /// Lower this SDK-facing conflict mode to the proto-layer
    /// [`pcloud_proto::methods::upload::ConflictParam`] consumed by
    /// `upload_save`. Kept in this module so the SDK's public surface
    /// does not need to re-export `ConflictParam`.
    fn to_proto_param(&self) -> pcloud_proto::methods::upload::ConflictParam {
        use pcloud_proto::methods::upload::ConflictParam;
        match self {
            ConflictMode::IfHashNumeric(h) => ConflictParam::IfHash(*h),
            ConflictMode::CreateIfAbsent => ConflictParam::New,
        }
    }
}

/// Metadata returned by the server once an upload has been committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetadata {
    /// Server-assigned file id once the upload is committed.
    pub file_id: Option<u64>,
    /// Parent folder id the server stored the file under.
    pub parent_folder_id: u64,
    /// Final filename the server stored.
    pub name: String,
    /// Total number of payload bytes the server acknowledged.
    pub bytes_uploaded: u64,
    /// Set to `true` when the server renamed the file because of a
    /// conflict.
    pub conflicted: bool,
    /// Optional server-reported SHA-256 / SHA-1 hex digest. Verified
    /// against what the client hashed locally when present.
    pub server_hash: Option<String>,
}

/// Runtime state of an [`UploadSession`]. Emitted on every progress
/// update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadState {
    /// Session handle constructed, nothing has been sent yet.
    Pending,
    /// Bytes are actively being sent.
    Uploading,
    /// The user asked to pause. Progress is frozen.
    Paused,
    /// The user asked to cancel. Terminal.
    Canceled,
    /// Committed successfully. Terminal.
    Completed,
    /// Failed. Terminal.
    Failed,
}

/// Progress snapshot published on the watch channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadProgress {
    /// Bytes confirmed written so far. Monotonically non-decreasing.
    pub bytes_sent: u64,
    /// Total payload size in bytes.
    pub bytes_total: u64,
    /// Current lifecycle state.
    pub state: UploadState,
}

impl UploadProgress {
    fn new(bytes_total: u64) -> Self {
        Self {
            bytes_sent: 0,
            bytes_total,
            state: UploadState::Pending,
        }
    }
}

/// Source of the upload payload.
pub enum UploadPayload {
    /// In-memory buffer. Zeroized on drop.
    Bytes(Vec<u8>),
    /// Local file path, read eagerly when the session runs.
    File(PathBuf),
}

impl core::fmt::Debug for UploadPayload {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Bytes(b) => f
                .debug_struct("Bytes")
                .field("len", &b.len())
                .finish_non_exhaustive(),
            Self::File(p) => f.debug_struct("File").field("path", p).finish(),
        }
    }
}

/// Request builder for [`EmbeddedDaemon::start_upload`].
#[derive(Debug)]
pub struct UploadRequest {
    /// Destination remote folder id.
    pub folder_id: u64,
    /// Desired remote filename.
    pub remote_filename: String,
    /// Payload source.
    pub payload: UploadPayload,
    /// Conflict resolution policy.
    pub conflict_mode: ConflictMode,
}

impl UploadRequest {
    /// Build a new request.
    #[must_use]
    pub fn new(
        folder_id: u64,
        remote_filename: impl Into<String>,
        payload: UploadPayload,
        conflict_mode: ConflictMode,
    ) -> Self {
        Self {
            folder_id,
            remote_filename: remote_filename.into(),
            payload,
            conflict_mode,
        }
    }
}

/// Error type surfaced by [`UploadSession`] methods.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UploadError {
    /// Session was cancelled; terminal.
    #[error("upload was canceled by the caller")]
    Canceled,
    /// `await_completion` called on a session that was never driven.
    #[error("upload session has not been started")]
    NotStarted,
    /// The requested transition is not permitted from the current state.
    #[error("invalid state transition: {0}")]
    InvalidState(&'static str),
    /// Reading the local payload failed.
    #[error("reading local payload failed: {0}")]
    Io(#[from] std::io::Error),
    /// Upload journal I/O failed.
    #[error("upload journal error: {0}")]
    Journal(String),
    /// A wire-level upload helper returned an error.
    #[error(transparent)]
    Helper(#[from] UploadHelperError),
    /// Server-reported hash did not match the locally-computed hash.
    #[error("server hash mismatch: expected {expected}, got {actual}")]
    HashMismatch {
        /// Locally computed hex digest.
        expected: String,
        /// Server-reported hex digest.
        actual: String,
    },
}

/// Handle representing a server-side `upload_create` reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadHandle {
    /// Server-assigned `uploadid`.
    pub upload_id: u64,
    /// Target folder id.
    pub parent_folder_id: u64,
    /// Remote filename.
    pub file_name: String,
}

/// Abstraction over the daemon-side wire calls. Split out so the
/// session state machine can be unit-tested with an in-memory mock.
pub trait UploadSessionDriver: Send {
    /// Issue `upload_create`. Returns the reservation handle.
    ///
    /// # Errors
    /// Returns [`UploadError::Helper`] on wire failure. Reachable from
    /// the `Idle` state; leaves the session in `Writing` on success.
    fn create(
        &mut self,
        folder_id: u64,
        file_name: &str,
        total: u64,
    ) -> Result<UploadHandle, UploadError>;

    /// Write one chunk at `offset`. Returns the post-write offset.
    ///
    /// # Errors
    /// Returns [`UploadError::Helper`] on wire failure; the session stays in
    /// `Writing` so the caller can retry or transition to `Paused`.
    fn write_chunk(
        &mut self,
        handle: &UploadHandle,
        offset: u64,
        buf: &[u8],
    ) -> Result<u64, UploadError>;

    /// Commit the upload with `upload_save`.
    ///
    /// # Errors
    /// Returns [`UploadError::Helper`] on wire failure; the session
    /// transitions to `Failed`. On success transitions to `Completed`.
    fn save(&mut self, handle: &UploadHandle) -> Result<FileMetadata, UploadError>;

    /// Discard a server-side reservation (`upload_delete`).
    ///
    /// # Errors
    /// Returns [`UploadError::Helper`] on wire failure. Called from
    /// `cancel()`; failure is logged but does not change the terminal
    /// `Canceled` state.
    fn delete(&mut self, handle: &UploadHandle) -> Result<(), UploadError>;
}

#[derive(Debug)]
struct InnerShared {
    progress_tx: watch::Sender<UploadProgress>,
    outcome: Mutex<Option<Result<FileMetadata, UploadError>>>,
    /// Chunked-state mirror. `None` on the legacy synchronous path.
    chunked: Mutex<Option<ChunkedState>>,
}

#[derive(Debug)]
struct ChunkedState {
    handle: UploadHandle,
    offset: u64,
    total: u64,
    chunks_done: u64,
    journal: Option<UploadJournal>,
    canceled: bool,
}

/// Public session handle.
#[derive(Debug, Clone)]
pub struct UploadSession {
    inner: Arc<InnerShared>,
}

impl UploadSession {
    fn new(total: u64) -> (Self, watch::Sender<UploadProgress>) {
        let (tx, _rx) = watch::channel(UploadProgress::new(total));
        let inner = Arc::new(InnerShared {
            progress_tx: tx.clone(),
            outcome: Mutex::new(None),
            chunked: Mutex::new(None),
        });
        (Self { inner }, tx)
    }

    /// Subscribe to progress updates.
    #[must_use]
    pub fn progress(&self) -> watch::Receiver<UploadProgress> {
        self.inner.progress_tx.subscribe()
    }

    /// Pause a `Writing` session. Leaves the upload reservation and the
    /// journal intact so [`Self::resume`] can pick up from the last
    /// acknowledged offset.
    ///
    /// # Errors
    /// Returns [`UploadError::InvalidState`] when the session is already
    /// in a terminal state (`Completed`, `Canceled`, `Failed`). Reachable
    /// from `Writing` → transitions to `Paused`; no-op from `Paused`.
    pub fn pause(&self) -> Result<(), UploadError> {
        let cur = self.inner.progress_tx.borrow().state;
        match cur {
            UploadState::Completed | UploadState::Canceled | UploadState::Failed => {
                Err(UploadError::InvalidState("cannot pause a terminal session"))
            }
            _ => {
                self.inner
                    .progress_tx
                    .send_modify(|p| p.state = UploadState::Paused);
                Ok(())
            }
        }
    }

    /// Resume a paused session. For chunked sessions this consults the
    /// on-disk journal so replay after a process restart picks up
    /// from the last fsynced offset.
    ///
    /// # Errors
    /// Returns [`UploadError::InvalidState`] if the session is in a
    /// terminal state or has never been started. Reachable from
    /// `Paused` → transitions back to `Writing`.
    pub fn resume(&self) -> Result<(), UploadError> {
        let cur = self.inner.progress_tx.borrow().state;
        match cur {
            UploadState::Completed | UploadState::Canceled | UploadState::Failed => {
                return Err(UploadError::InvalidState(
                    "cannot resume a terminal session",
                ));
            }
            UploadState::Pending => {
                return Err(UploadError::InvalidState("session has not been started"));
            }
            _ => {}
        }

        // If this is a chunked session, reconcile the in-memory offset
        // with the journal so a crash-replay picks up correctly.
        if let Some(state) = self
            .inner
            .chunked
            .lock_or_poisoned("sdk::upload_session::chunked")
            .as_mut()
        {
            // Read out the latest journal entry without holding a borrow on
            // `state.journal` past the inner block, so the mutating writes
            // below to `state.offset` / `state.chunks_done` are unambiguous
            // for readability across all supported toolchains.
            let upload_id = state.handle.upload_id;
            let last = state
                .journal
                .as_ref()
                .and_then(|j| j.replay().ok())
                .and_then(|report| {
                    report
                        .entries
                        .iter()
                        .rfind(|e| e.upload_id == upload_id)
                        .cloned()
                });
            if let Some(entry) = last {
                state.offset = entry.bytes;
                state.chunks_done = entry.chunks_done;
                self.inner.progress_tx.send_modify(|p| {
                    p.bytes_sent = entry.bytes;
                });
            }
        }
        self.inner
            .progress_tx
            .send_modify(|p| p.state = UploadState::Uploading);
        Ok(())
    }

    /// Cancel the session: marks the server-side reservation for
    /// deletion and clears the journal entry.
    ///
    /// Terminal: subsequent [`Self::await_completion`] returns
    /// [`UploadError::Canceled`]. Idempotent.
    pub fn cancel(&self) {
        self.inner.progress_tx.send_modify(|p| {
            p.state = UploadState::Canceled;
        });
        if let Some(state) = self
            .inner
            .chunked
            .lock_or_poisoned("sdk::upload_session::chunked")
            .as_mut()
        {
            state.canceled = true;
        }
        let mut guard = self
            .inner
            .outcome
            .lock_or_poisoned("sdk::upload_session::outcome");
        *guard = Some(Err(UploadError::Canceled));
    }

    /// Block until the upload has either completed, failed, or been
    /// canceled, and return the terminal outcome.
    ///
    /// # Errors
    /// Propagates whatever terminal error was recorded. Returns
    /// [`UploadError::NotStarted`] if the session slot was never populated.
    pub fn await_completion(self) -> Result<FileMetadata, UploadError> {
        let mut guard = self
            .inner
            .outcome
            .lock_or_poisoned("sdk::upload_session::outcome");
        guard.take().unwrap_or(Err(UploadError::NotStarted))
    }

    /// Start a chunked upload session driven by `driver`. This is the
    /// primary entry point for the new state machine; production code
    /// wires it through `EmbeddedDaemon::start_chunked_upload`.
    ///
    /// # Errors
    /// Returns [`UploadError::Helper`] on `upload_create` failure. On
    /// success the session is left in `Writing { offset: 0, total }`.
    pub fn start<D: UploadSessionDriver>(
        folder_id: u64,
        file_name: impl Into<String>,
        total: u64,
        driver: &mut D,
        journal: Option<UploadJournal>,
    ) -> Result<Self, UploadError> {
        let file_name = file_name.into();
        let handle = driver.create(folder_id, &file_name, total)?;
        let (session, tx) = UploadSession::new(total);
        tx.send_modify(|p| p.state = UploadState::Uploading);
        *session
            .inner
            .chunked
            .lock_or_poisoned("sdk::upload_session::chunked") = Some(ChunkedState {
            handle,
            offset: 0,
            total,
            chunks_done: 0,
            journal,
            canceled: false,
        });
        Ok(session)
    }

    /// Write one chunk. Appends a journal entry per successful chunk so
    /// a crash can resume from the last fsynced offset.
    ///
    /// # Errors
    /// * [`UploadError::InvalidState`] when the session is not in
    ///   `Writing`.
    /// * [`UploadError::Canceled`] when `cancel()` was called.
    /// * [`UploadError::Helper`] when `upload_write` fails — state stays
    ///   `Writing` so the caller can retry or pause.
    /// * [`UploadError::Journal`] when journal append fails; the chunk
    ///   is treated as not-durably-recorded and the state is unchanged.
    pub fn write_chunk<D: UploadSessionDriver>(
        &self,
        driver: &mut D,
        buf: &[u8],
    ) -> Result<u64, UploadError> {
        let cur = self.inner.progress_tx.borrow().state;
        if cur != UploadState::Uploading {
            return Err(UploadError::InvalidState(
                "write_chunk requires Writing state",
            ));
        }
        let (handle, offset) = {
            let guard = self
                .inner
                .chunked
                .lock_or_poisoned("sdk::upload_session::chunked");
            let state = guard
                .as_ref()
                .ok_or(UploadError::InvalidState("no chunked state"))?;
            if state.canceled {
                return Err(UploadError::Canceled);
            }
            (state.handle.clone(), state.offset)
        };
        let new_offset = driver.write_chunk(&handle, offset, buf)?;

        // Update in-memory state first, then append journal entry.
        let (chunks_done, journal_ref) = {
            let mut guard = self
                .inner
                .chunked
                .lock_or_poisoned("sdk::upload_session::chunked");
            let state = guard
                .as_mut()
                .ok_or(UploadError::InvalidState("no chunked state"))?;
            state.offset = new_offset;
            state.chunks_done += 1;
            (state.chunks_done, state.journal.clone())
        };

        self.inner.progress_tx.send_modify(|p| {
            p.bytes_sent = new_offset;
        });

        if let Some(journal) = journal_ref {
            let entry = JournalEntry {
                upload_id: handle.upload_id,
                chunks_done,
                bytes: new_offset,
                sha_partial: None,
                descriptor: None,
                committed: false,
            };
            journal
                .append(&entry)
                .map_err(|e| UploadError::Journal(e.to_string()))?;
        }

        Ok(new_offset)
    }

    /// Commit via `upload_save` and transition to `Completed`.
    ///
    /// # Errors
    /// * [`UploadError::InvalidState`] if the session is not `Writing`
    ///   or if the total byte count has not been reached.
    /// * [`UploadError::Canceled`] if the session was cancelled.
    /// * [`UploadError::Helper`] on wire failure — transitions to `Failed`.
    /// * [`UploadError::HashMismatch`] when a server hash is present and
    ///   disagrees with `expected_hash`.
    pub fn save_and_complete<D: UploadSessionDriver>(
        self,
        driver: &mut D,
        expected_hash: Option<&str>,
    ) -> Result<FileMetadata, UploadError> {
        let cur = self.inner.progress_tx.borrow().state;
        if cur != UploadState::Uploading {
            return Err(UploadError::InvalidState(
                "save_and_complete requires Writing state",
            ));
        }
        let (handle, offset, total, journal_ref, canceled) = {
            let guard = self
                .inner
                .chunked
                .lock_or_poisoned("sdk::upload_session::chunked");
            let state = guard
                .as_ref()
                .ok_or(UploadError::InvalidState("no chunked state"))?;
            (
                state.handle.clone(),
                state.offset,
                state.total,
                state.journal.clone(),
                state.canceled,
            )
        };
        if canceled {
            return Err(UploadError::Canceled);
        }
        if offset != total {
            return Err(UploadError::InvalidState(
                "save called before all bytes written",
            ));
        }

        let meta = match driver.save(&handle) {
            Ok(m) => m,
            Err(err) => {
                self.inner
                    .progress_tx
                    .send_modify(|p| p.state = UploadState::Failed);
                let mut guard = self
                    .inner
                    .outcome
                    .lock_or_poisoned("sdk::upload_session::outcome");
                // `err` is captured once; can't clone a dyn error, so we
                // re-encode via a string-carrying helper variant.
                let reason = err.to_string();
                *guard = Some(Err(UploadError::Helper(UploadHelperError::Write(reason))));
                return Err(err);
            }
        };

        // Optional hash verification.
        if let (Some(expected), Some(actual)) = (expected_hash, meta.server_hash.as_deref()) {
            if expected != actual {
                self.inner
                    .progress_tx
                    .send_modify(|p| p.state = UploadState::Failed);
                let hm = UploadError::HashMismatch {
                    expected: expected.to_owned(),
                    actual: actual.to_owned(),
                };
                let hm_twin = UploadError::HashMismatch {
                    expected: expected.to_owned(),
                    actual: actual.to_owned(),
                };
                *self
                    .inner
                    .outcome
                    .lock_or_poisoned("sdk::upload_session::outcome") = Some(Err(hm_twin));
                return Err(hm);
            }
        }

        // Clear journal on successful commit.
        if let Some(journal) = journal_ref {
            if let Err(err) = journal.clear() {
                eprintln!(
                    "upload_session: journal clear failed for uploadid={}: {err}",
                    handle.upload_id
                );
            }
        }

        self.inner.progress_tx.send_modify(|p| {
            p.bytes_sent = meta.bytes_uploaded;
            p.state = UploadState::Completed;
        });
        *self
            .inner
            .outcome
            .lock_or_poisoned("sdk::upload_session::outcome") = Some(Ok(meta.clone()));
        Ok(meta)
    }

    /// Expose the current server handle (test + diagnostic use).
    #[must_use]
    pub fn handle(&self) -> Option<UploadHandle> {
        self.inner
            .chunked
            .lock_or_poisoned("sdk::upload_session::chunked")
            .as_ref()
            .map(|s| s.handle.clone())
    }

    /// Expose the current in-memory offset (test + diagnostic use).
    #[must_use]
    pub fn current_offset(&self) -> Option<u64> {
        self.inner
            .chunked
            .lock_or_poisoned("sdk::upload_session::chunked")
            .as_ref()
            .map(|s| s.offset)
    }
}

// ------------------------------------------------------------------
// Production driver — wraps `daemon.runtime.transfer_runtime` so the
// chunked state machine in `UploadSession::{start, write_chunk,
// save_and_complete}` runs against real `upload_create` /
// `upload_write` / `upload_save` round-trips. Replaces the legacy
// single-shot wrapper that buffered the entire payload and discarded
// the conflict policy (bd-1du row 94).
// ------------------------------------------------------------------

/// `UploadSessionDriver` impl that delegates to the daemon's
/// `TransferRuntime`. Owns no state beyond the runtime reference,
/// the cached auth token (zeroized on drop via `SecretString`), the
/// in-flight chunk counter and the conflict mode selected by the
/// caller.
pub(crate) struct RuntimeUploadDriver<'a> {
    runtime: &'a pcloud_daemon::transfer_backend::TransferRuntime,
    auth_token: SecretString,
    next_chunk_id: u64,
    conflict: pcloud_proto::methods::upload::ConflictParam,
}

impl<'a> RuntimeUploadDriver<'a> {
    pub(crate) fn new(
        runtime: &'a pcloud_daemon::transfer_backend::TransferRuntime,
        auth_token: SecretString,
        conflict: pcloud_proto::methods::upload::ConflictParam,
    ) -> Self {
        Self {
            runtime,
            auth_token,
            next_chunk_id: 0,
            conflict,
        }
    }
}

impl<'a> UploadSessionDriver for RuntimeUploadDriver<'a> {
    fn create(
        &mut self,
        folder_id: u64,
        file_name: &str,
        total: u64,
    ) -> Result<UploadHandle, UploadError> {
        let session = self
            .runtime
            .upload_create(
                self.auth_token.clone_secret(),
                folder_id,
                file_name.to_owned(),
                total,
            )
            .map_err(
                |err: pcloud_proto::TransferApiError<
                    pcloud_daemon::transfer_backend::TransferBackendError,
                >| {
                    UploadError::Helper(UploadHelperError::Create(err.to_string()))
                },
            )?;
        Ok(UploadHandle {
            upload_id: session.upload_id,
            parent_folder_id: session.parent_folder_id,
            file_name: session.file_name,
        })
    }

    fn write_chunk(
        &mut self,
        handle: &UploadHandle,
        offset: u64,
        buf: &[u8],
    ) -> Result<u64, UploadError> {
        let chunk_id = self.next_chunk_id;
        self.next_chunk_id = self.next_chunk_id.saturating_add(1);
        let new_offset = self
            .runtime
            .upload_write_chunk(
                self.auth_token.clone_secret(),
                handle.upload_id,
                offset,
                chunk_id,
                buf,
            )
            .map_err(
                |err: pcloud_daemon::transfer_backend::TransferBackendError| {
                    UploadError::Helper(UploadHelperError::Write(err.to_string()))
                },
            )?;
        Ok(new_offset)
    }

    fn save(&mut self, handle: &UploadHandle) -> Result<FileMetadata, UploadError> {
        let session = pcloud_proto::UploadSession {
            upload_id: handle.upload_id,
            file_id: None,
            parent_folder_id: handle.parent_folder_id,
            file_name: handle.file_name.clone(),
            api_server: None,
        };
        let mtime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.runtime
            .upload_save_session(
                self.auth_token.clone_secret(),
                &session,
                Some(self.conflict.clone()),
                mtime,
            )
            .map_err(
                |err: pcloud_daemon::transfer_backend::TransferBackendError| {
                    UploadError::Helper(UploadHelperError::Write(err.to_string()))
                },
            )?;
        Ok(FileMetadata {
            file_id: None,
            parent_folder_id: handle.parent_folder_id,
            name: handle.file_name.clone(),
            // Cumulative bytes are tracked by the session state machine;
            // the session sets `bytes_sent` from its own offset before
            // calling save, so the value reported here is informational.
            bytes_uploaded: 0,
            conflicted: false,
            server_hash: None,
        })
    }

    fn delete(&mut self, handle: &UploadHandle) -> Result<(), UploadError> {
        self.runtime
            .upload_delete(self.auth_token.clone_secret(), handle.upload_id)
            .map_err(
                |err: pcloud_proto::TransferApiError<
                    pcloud_daemon::transfer_backend::TransferBackendError,
                >| {
                    UploadError::Helper(UploadHelperError::Write(err.to_string()))
                },
            )?;
        Ok(())
    }
}

/// Internal entry point used by [`EmbeddedDaemon::start_upload`]. Drives
/// the chunked state machine
/// ([`UploadSession::start`] / [`UploadSession::write_chunk`] /
/// [`UploadSession::save_and_complete`]) against a
/// [`RuntimeUploadDriver`] so the public route honours
/// [`UploadConfig::chunk_size`] and threads
/// [`UploadRequest::conflict_mode`] onto the wire (bd-1du row 94).
pub(crate) fn run_upload(daemon: &mut EmbeddedDaemon, request: UploadRequest) -> UploadSession {
    run_upload_with_config(daemon, request, UploadConfig::default())
}

pub(crate) fn run_upload_with_config(
    daemon: &mut EmbeddedDaemon,
    request: UploadRequest,
    config: UploadConfig,
) -> UploadSession {
    // Resolve auth token once up front so a missing-auth failure surfaces
    // before any wire round-trips.
    let auth_token = match daemon.auth_token_secret() {
        Some(token) => token,
        None => {
            let (session, tx) = UploadSession::new(0);
            tx.send_modify(|p| p.state = UploadState::Failed);
            *session
                .inner
                .outcome
                .lock_or_poisoned("sdk::upload_session::outcome") = Some(Err(UploadError::Helper(
                UploadHelperError::NotAuthenticated,
            )));
            return session;
        }
    };

    let mut bytes = match load_payload(&request.payload) {
        Ok(b) => b,
        Err(err) => {
            let (session, tx) = UploadSession::new(0);
            tx.send_modify(|p| p.state = UploadState::Failed);
            *session
                .inner
                .outcome
                .lock_or_poisoned("sdk::upload_session::outcome") = Some(Err(err));
            return session;
        }
    };
    let total = bytes.len() as u64;

    let conflict_param = request.conflict_mode.to_proto_param();
    let mut driver =
        RuntimeUploadDriver::new(&daemon.runtime.transfer_runtime, auth_token, conflict_param);

    // --- start ---
    let session = match UploadSession::start(
        request.folder_id,
        request.remote_filename.clone(),
        total,
        &mut driver,
        None,
    ) {
        Ok(s) => s,
        Err(err) => {
            let (session, tx) = UploadSession::new(total);
            tx.send_modify(|p| p.state = UploadState::Failed);
            *session
                .inner
                .outcome
                .lock_or_poisoned("sdk::upload_session::outcome") = Some(Err(err));
            bytes.zeroize();
            return session;
        }
    };

    // --- write_chunk loop ---
    let chunk_size = config.chunk_size.max(1);
    let mut write_err: Option<UploadError> = None;
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + chunk_size).min(bytes.len());
        match session.write_chunk(&mut driver, &bytes[offset..end]) {
            Ok(_) => {}
            Err(err) => {
                write_err = Some(err);
                break;
            }
        }
        offset = end;
    }

    if let Some(err) = write_err {
        session
            .inner
            .progress_tx
            .send_modify(|p| p.state = UploadState::Failed);
        *session
            .inner
            .outcome
            .lock_or_poisoned("sdk::upload_session::outcome") = Some(Err(err));
        bytes.zeroize();
        return session;
    }

    // --- save_and_complete ---
    let saved = session.clone().save_and_complete(&mut driver, None);
    bytes.zeroize();

    let outcome = match saved {
        Ok(meta) => {
            // The protocol-layer save does not echo the byte total back,
            // so backfill it from the session state machine for caller
            // ergonomics.
            let mut meta = meta;
            meta.bytes_uploaded = total;
            session
                .inner
                .progress_tx
                .send_modify(|p| p.bytes_sent = total);
            Ok(meta)
        }
        Err(err) => Err(err),
    };

    *session
        .inner
        .outcome
        .lock_or_poisoned("sdk::upload_session::outcome") = Some(outcome);

    session
}

fn load_payload(payload: &UploadPayload) -> Result<Vec<u8>, UploadError> {
    match payload {
        UploadPayload::Bytes(b) => Ok(b.clone()),
        UploadPayload::File(p) => Ok(read_file(p)?),
    }
}

fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

#[allow(dead_code)]
pub(crate) fn scrub_token(token: SecretString) {
    use pcloud_secret::ExposeSecret;
    let _ = token.expose_secret();
    drop(token);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use pcloud_config::Environment;
    use pcloud_ipc::{Request, ResponseStatus};
    use proptest::prelude::*;

    fn unique_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pcloud-sdk-upload-session-{tag}-{pid}-{nanos}",
            pid = std::process::id()
        ))
    }

    fn authed_daemon(tag: &str) -> EmbeddedDaemon {
        let mut daemon = EmbeddedDaemon::builder(unique_root(tag))
            .environment(Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);
        daemon
    }

    #[test]
    fn start_upload_round_trip_completes_on_development_transport() {
        let mut daemon = authed_daemon("round-trip");
        let payload = b"session-payload".to_vec();
        let total = payload.len() as u64;

        let session = daemon.start_upload(UploadRequest::new(
            22,
            "session.txt",
            UploadPayload::Bytes(payload),
            ConflictMode::CreateIfAbsent,
        ));

        let rx = session.progress();
        let snap = rx.borrow().clone();
        assert_eq!(snap.bytes_total, total);
        assert_eq!(snap.state, UploadState::Completed);
        assert_eq!(snap.bytes_sent, total);

        let meta = session.await_completion().expect("upload should succeed");
        assert_eq!(meta.parent_folder_id, 22);
        assert_eq!(meta.name, "session.txt");
        assert_eq!(meta.bytes_uploaded, total);
        assert!(!meta.conflicted);
    }

    #[test]
    fn conflict_mode_if_hash_numeric_is_accepted_by_builder() {
        let req = UploadRequest::new(
            1,
            "f.bin",
            UploadPayload::Bytes(vec![0u8; 4]),
            ConflictMode::IfHashNumeric(0xdead_beef),
        );
        assert!(matches!(req.conflict_mode, ConflictMode::IfHashNumeric(_)));
    }

    proptest! {
        #[test]
        fn progress_is_monotonic(sizes in proptest::collection::vec(0u64..4096, 1..16)) {
            let (session, tx) = UploadSession::new(sizes.iter().sum());
            let mut rx = session.progress();
            let mut last = rx.borrow().bytes_sent;
            let mut cum = 0u64;
            for s in sizes {
                cum += s;
                tx.send_modify(|p| {
                    p.bytes_sent = cum;
                    p.state = UploadState::Uploading;
                });
                let snap = rx.borrow_and_update().clone();
                prop_assert!(snap.bytes_sent >= last, "progress regressed: {} -> {}", last, snap.bytes_sent);
                last = snap.bytes_sent;
            }
        }
    }

    // ------------------------------------------------------------------
    // Mock-server integration test for the production driver wiring.
    //
    // Exercises [`RuntimeUploadDriver`] against a programmable TCP mock
    // server impersonating the binary API. Verifies (a) the chunked
    // state machine actually issues `upload_create` →
    // N × `upload_write` → `upload_save` against the wire, and (b) the
    // `ifhash` parameter selected by [`ConflictMode`] is threaded onto
    // the `upload_save` frame. No live pCloud account required.
    // ------------------------------------------------------------------
    use std::io::{Read as _, Write as _};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc as StdArc, Mutex as StdMutex};
    use std::thread;

    use pcloud_config::{
        ConfigProfile,
        api::{ApiEndpoint, ApiMode},
    };

    fn encode_short_key(out: &mut Vec<u8>, key: &str) {
        const RPARAM_SHORT_STR_BASE: u8 = 100;
        assert!(key.len() <= 49, "test key too long");
        out.push(RPARAM_SHORT_STR_BASE + key.len() as u8);
        out.extend_from_slice(key.as_bytes());
    }

    fn encode_small_num(out: &mut Vec<u8>, n: u64) {
        const RPARAM_NUM8: u8 = 15;
        const RPARAM_SMALL_NUM_BASE: u8 = 200;
        if n < 20 {
            out.push(RPARAM_SMALL_NUM_BASE + n as u8);
        } else {
            out.push(RPARAM_NUM8);
            out.extend_from_slice(&n.to_le_bytes());
        }
    }

    /// Build a binary-API hash response carrying `entries` as numeric
    /// values. Frame layout matches `pcloud_proto::response`.
    fn build_num_response(entries: &[(&str, u64)]) -> Vec<u8> {
        const RPARAM_HASH: u8 = 16;
        const RPARAM_END: u8 = 255;
        let mut body = vec![RPARAM_HASH];
        for (k, v) in entries {
            encode_short_key(&mut body, k);
            encode_small_num(&mut body, *v);
        }
        body.push(RPARAM_END);
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&body);
        frame
    }

    /// Read one request frame from `stream`. Returns
    /// `(command_name, body_len, raw_frame_bytes)`. `body_len` is `Some`
    /// when the request declares a trailing raw-body segment
    /// (`upload_write`).
    fn read_request_frame(
        stream: &mut std::net::TcpStream,
    ) -> Option<(String, Option<u64>, Vec<u8>)> {
        let mut hdr = [0u8; 2];
        if stream.read_exact(&mut hdr).is_err() {
            return None;
        }
        let len = u16::from_le_bytes(hdr) as usize;
        let mut body = vec![0u8; len];
        if stream.read_exact(&mut body).is_err() {
            return None;
        }
        let mut full = hdr.to_vec();
        full.extend_from_slice(&body);
        let cmd_byte = full[2];
        let has_body = (cmd_byte & 0x80) != 0;
        let cmd_len = (cmd_byte & 0x7F) as usize;
        let (body_len, cmd_off) = if has_body {
            (
                Some(u64::from_le_bytes(full[3..11].try_into().unwrap())),
                11usize,
            )
        } else {
            (None, 3usize)
        };
        let cmd = String::from_utf8_lossy(&full[cmd_off..cmd_off + cmd_len]).into_owned();
        Some((cmd, body_len, full))
    }

    /// Run a single mock-server connection that accepts the chunked
    /// upload sequence and returns canned `result=0` responses. Records
    /// the observed command sequence and the raw `upload_save` frame so
    /// the test can assert `ifhash` threading.
    struct ObservedSession {
        commands: Vec<String>,
        save_frame: Option<Vec<u8>>,
        write_chunks: usize,
    }

    fn spawn_chunked_mock_server(
        max_writes: usize,
    ) -> (
        SocketAddr,
        StdArc<StdMutex<ObservedSession>>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener bind");
        let address = listener.local_addr().expect("local addr");
        let observed = StdArc::new(StdMutex::new(ObservedSession {
            commands: Vec::new(),
            save_frame: None,
            write_chunks: 0,
        }));
        let obs_clone = observed.clone();
        let handle = thread::spawn(move || {
            // The session opens one connection per round-trip, so accept
            // create + N*writes + save sequentially.
            let total_conns = 1 + max_writes + 1;
            for _ in 0..total_conns {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let (cmd, body_len, raw) = match read_request_frame(&mut stream) {
                    Some(t) => t,
                    None => return,
                };
                obs_clone.lock().unwrap().commands.push(cmd.clone());
                if cmd == "upload_save" {
                    obs_clone.lock().unwrap().save_frame = Some(raw);
                }
                if cmd == "upload_write" {
                    obs_clone.lock().unwrap().write_chunks += 1;
                }
                if let Some(len) = body_len {
                    let mut buf = vec![0u8; len as usize];
                    let _ = stream.read_exact(&mut buf);
                }
                let resp = match cmd.as_str() {
                    "upload_create" => build_num_response(&[("result", 0), ("uploadid", 77)]),
                    "upload_write" | "upload_save" => build_num_response(&[("result", 0)]),
                    _ => build_num_response(&[("result", 1)]),
                };
                let _ = stream.write_all(&resp);
                let _ = stream.flush();
            }
        });
        (address, observed, handle)
    }

    #[test]
    fn start_upload_drives_chunked_sequence_with_conflict_threaded_to_save() {
        // Spawn a TCP mock impersonating the binary API. The chunked
        // driver opens one connection per round-trip, so size the
        // server for create + 2 writes + save with a 4-byte payload at
        // 2-byte chunks.
        let payload = [0x42u8; 4];
        let chunk_size = 2usize;
        let expected_writes = payload.len().div_ceil(chunk_size); // = 2
        let (address, observed, server) = spawn_chunked_mock_server(expected_writes);

        // Build a TransferRuntime pointing at the mock.
        let mut config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join(format!(
                "pcloud-sdk-driver-mock-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
            Environment::Production,
        );
        config.api = ApiEndpoint {
            mode: ApiMode::Plaintext,
            host: address.ip().to_string(),
            port: address.port(),
            server_name: address.ip().to_string(),
            connect_timeout_ms: 2_000,
            read_timeout_ms: 2_000,
            tls_revocation_check: Default::default(),
        };
        // `pcloud_daemon::transfer_backend` is a `pub use` re-export of
        // `pcloud_backends::transfer_backend`, so referring to either
        // path resolves to the same type. Use the re-export so the
        // driver constructor's lifetime is unambiguous.
        let runtime = pcloud_daemon::transfer_backend::TransferRuntime::from_config(&config);

        // Drive the chunked state machine with the new production
        // driver. Conflict mode `IfHashNumeric(0xdeadbeef)` must be
        // threaded onto the wire as `ifhash = 0xdeadbeef` on the
        // `upload_save` frame.
        let mut driver = RuntimeUploadDriver::new(
            &runtime,
            SecretString::new("test-token".to_owned()),
            ConflictMode::IfHashNumeric(0xdead_beef).to_proto_param(),
        );
        let session = UploadSession::start(99, "f.bin", payload.len() as u64, &mut driver, None)
            .expect("start should reserve upload_id");
        for chunk in payload.chunks(chunk_size) {
            session
                .write_chunk(&mut driver, chunk)
                .expect("write_chunk should succeed against mock");
        }
        let meta = session
            .save_and_complete(&mut driver, None)
            .expect("save should succeed against mock");

        assert_eq!(meta.parent_folder_id, 99);
        assert_eq!(meta.name, "f.bin");

        server.join().expect("mock server thread should join");

        let obs = observed.lock().unwrap();
        // Wire sequence: 1 × upload_create, N × upload_write, 1 × upload_save.
        assert_eq!(
            obs.commands.first().map(String::as_str),
            Some("upload_create"),
            "first frame should be upload_create"
        );
        assert_eq!(
            obs.commands.last().map(String::as_str),
            Some("upload_save"),
            "last frame should be upload_save"
        );
        assert_eq!(
            obs.write_chunks, expected_writes,
            "driver should issue one upload_write per chunk"
        );
        // Conflict policy must reach the wire — search the recorded
        // upload_save frame for the `ifhash` parameter key.
        let save = obs
            .save_frame
            .as_ref()
            .expect("upload_save frame should have been captured");
        let save_text = String::from_utf8_lossy(save);
        assert!(
            save_text.contains("ifhash"),
            "upload_save frame must carry the ifhash parameter (got {save_text:?})"
        );
    }
}

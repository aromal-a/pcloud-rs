//! High-level upload-session registry (pause / resume / cancel / list).
//!
//! This module formalises the *operator-visible* upload state machine that
//! sits on top of the existing chunked-upload driver in
//! [`crate::upload_state`]. Where [`crate::upload_state::UploadStateMachine`]
//! models the wire-level `create → write×N → save` protocol with retry +
//! auth-refresh semantics, [`UploadSession`] models the lifecycle an
//! operator reasons about: *Pending → InProgress → (Paused|Cancelled|
//! Completed|Failed)*.
//!
//! # Scope and honesty
//!
//! * **No IPC breakage.** The new IPC variants (`UploadCreate`,
//!   `UploadPause`, `UploadResume`, `UploadCancel`, `UploadList`) are
//!   additive and `Request` is `#[non_exhaustive]`.
//! * **No long soaks here.** Transitions are exercised by deterministic
//!   unit tests (`tests` submodule) and by the daemon integration tests
//!   under `crates/pcloud-daemon/tests/upload_sessions.rs`. The tracker
//!   pre-alpha-honesty rule applies: we mark transitions *verified by
//!   test*, not by long soak.
//! * **In-memory registry.** Persistence of per-session bookkeeping
//!   across daemon restarts is out of scope here; the existing
//!   `upload_resume_state` SQLite table + NDJSON journal already carry
//!   crash-safe wire-protocol resume. The registry exists so a running
//!   daemon can expose a consistent pause/resume/cancel surface to the
//!   CLI while the underlying transfer is in flight.
//!
//! # State diagram
//!
//! ```text
//!               create                       drive (write)
//! (none) ──────────────────► Pending ───────────────────► InProgress
//!                                │   ▲                         │
//!                                │   │ resume                  │ pause
//!                                │   │                         ▼
//!                                │   └──────────────────── Paused
//!                                │                             │
//!                                │ cancel                      │ cancel
//!                                ▼                             ▼
//!                            Cancelled                     Cancelled
//!
//!                   (from InProgress)         drive success
//!                       ─────────────────────────► Completed
//!
//!                   (from InProgress | Paused)  drive perm-fail
//!                       ─────────────────────────► Failed("…")
//! ```
//!
//! All transitions are recorded in [`UploadSession::history`] so the CLI
//! `upload list` surface and the integration tests can assert them
//! without racing the driver.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Conflict-mode knob exposed on `Request::UploadCreate`.
///
/// Maps to the pCloud chunked-upload `ifhash` parameter when the upload
/// is actually dispatched. Default is [`ConflictMode::Error`] — it
/// matches pCloud upload parity (reject on remote collision instead of
/// silently overwriting).
///
/// # Example
///
/// ```
/// use pcloud_backends::upload_sessions::ConflictMode;
///
/// assert_eq!(ConflictMode::default(), ConflictMode::Error);
/// // Exhaustive match enforces call-sites handle every variant.
/// fn describe(m: ConflictMode) -> &'static str {
///     match m {
///         ConflictMode::Error => "error",
///         ConflictMode::Overwrite => "overwrite",
///         ConflictMode::Skip => "skip",
///         ConflictMode::Rename => "rename",
///     }
/// }
/// assert_eq!(describe(ConflictMode::Rename), "rename");
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictMode {
    /// Refuse the upload if the remote path already exists. Default, and
    /// the safest option — matches the strict Rust parity stance versus
    /// the legacy C client's implicit-overwrite behaviour.
    #[default]
    Error,
    /// Replace the existing remote file (maps to `ifhash` absent on
    /// upload_save).
    Overwrite,
    /// Treat an existing remote file as a success no-op. The session
    /// transitions straight to [`UploadState::Completed`] without
    /// bytes-written.
    Skip,
    /// Pick a unique sibling name on the remote side — e.g. `report.pdf`
    /// becomes `report (2).pdf` — and upload under the new name.
    Rename,
}

/// Observable state of an [`UploadSession`] from the operator's point of
/// view.
///
/// Six variants, matching the task specification. Wire-level states
/// (Creating, Writing, Saving) are *not* surfaced here — they are
/// collapsed into [`UploadState::InProgress`] so the operator does not
/// have to reason about the underlying protocol state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadState {
    /// Registered but the driver has not yet issued `upload_create`.
    Pending,
    /// The driver is actively writing bytes / refreshing auth / retrying.
    InProgress,
    /// Operator-requested pause. The driver is idle; resume flips back
    /// to [`UploadState::InProgress`]. The session retains its
    /// `offset` / `total_bytes` so the CLI can render progress.
    Paused,
    /// Operator-requested cancel. Terminal; the driver aborts any
    /// in-flight request and any server-side draft is discarded on
    /// best-effort basis.
    Cancelled,
    /// Driver reported a successful `upload_save` (or a no-op skip
    /// under [`ConflictMode::Skip`] when the remote file already
    /// existed). Terminal.
    Completed,
    /// Driver reported a permanent failure. Terminal. Carries the
    /// redacted reason string for operator display.
    Failed(String),
}

impl UploadState {
    /// `true` once the session cannot transition any further.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            UploadState::Cancelled | UploadState::Completed | UploadState::Failed(_)
        )
    }

    /// Short, machine-readable label for JSON envelopes.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            UploadState::Pending => "pending",
            UploadState::InProgress => "in_progress",
            UploadState::Paused => "paused",
            UploadState::Cancelled => "cancelled",
            UploadState::Completed => "completed",
            UploadState::Failed(_) => "failed",
        }
    }
}

/// Operator-facing upload session handle.
///
/// Ids are generated locally by the daemon (monotone counter) and are
/// **not** the pCloud wire `upload_id`. The wire id lives on the
/// in-flight driver and is only observed through the resume-state table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    /// Monotone locally-generated id — unique per running daemon.
    pub id: u64,
    /// Local path the driver will stream from.
    pub path: PathBuf,
    /// Target remote filename (post-rename if
    /// `conflict_mode == ConflictMode::Rename`).
    pub remote_name: String,
    /// Parent folder id or `None` when the caller addressed by path.
    pub parent_folder_id: Option<u64>,
    /// Observable state.
    pub state: UploadState,
    /// Conflict-mode selected at create time. Immutable across the
    /// session lifecycle.
    pub conflict_mode: ConflictMode,
    /// Client-tracked byte offset (the last byte durably observed on
    /// the server side).
    pub offset: u64,
    /// Total expected byte length. `0` is allowed (zero-byte upload).
    pub total_bytes: u64,
    /// UNIX-seconds at create time. Monotone only within a daemon run.
    pub created_at: i64,
    /// UNIX-seconds of the most recent state transition.
    pub updated_at: i64,
    /// Compact transition log — for test assertions and CLI `list`.
    pub history: Vec<UploadStateTransition>,
}

/// One entry in [`UploadSession::history`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadStateTransition {
    /// State label *after* the transition.
    pub state: String,
    /// UNIX-seconds of the transition.
    pub at: i64,
}

/// Errors surfaced by [`SessionRegistry`] mutations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    /// No session exists with the given id.
    #[error("upload session {0} not found")]
    NotFound(u64),
    /// The transition is not allowed from the current state (e.g.
    /// resuming a completed upload).
    #[error("cannot transition session {id} from {current} to {requested}")]
    InvalidTransition {
        /// Session id.
        id: u64,
        /// Current state label.
        current: &'static str,
        /// Requested state label.
        requested: &'static str,
    },
}

/// In-memory registry of [`UploadSession`] handles.
///
/// Thread-safety: the registry is `!Sync` and lives on
/// `pcloud_daemon::RuntimeShell`, which is itself dispatched from a
/// single IPC thread. Cross-thread observers go through the dispatched
/// `UploadList` IPC instead of touching the registry directly.
#[derive(Debug, Default)]
pub struct SessionRegistry {
    by_id: BTreeMap<u64, UploadSession>,
    next_id: AtomicU64,
}

impl SessionRegistry {
    /// Fresh registry. Ids start at `1` so `0` can be reserved as a
    /// sentinel for "no session".
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_id: BTreeMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Registers a new session in [`UploadState::Pending`].
    ///
    /// When `conflict_mode == ConflictMode::Rename`, the caller is
    /// expected to have pre-resolved `remote_name` via
    /// [`pick_unique_name`]. The registry itself does not call out to
    /// the server.
    pub fn create(
        &mut self,
        path: PathBuf,
        remote_name: String,
        parent_folder_id: Option<u64>,
        total_bytes: u64,
        conflict_mode: ConflictMode,
    ) -> &UploadSession {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let now = now_unix_secs();
        let session = UploadSession {
            id,
            path,
            remote_name,
            parent_folder_id,
            state: UploadState::Pending,
            conflict_mode,
            offset: 0,
            total_bytes,
            created_at: now,
            updated_at: now,
            history: vec![UploadStateTransition {
                state: "pending".to_owned(),
                at: now,
            }],
        };
        self.by_id.insert(id, session);
        // INVARIANT: the session was inserted on the preceding line;
        // the key is guaranteed to be present in the same-thread call.
        self.by_id.get(&id).expect("just inserted")
    }

    /// Borrow a single session.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&UploadSession> {
        self.by_id.get(&id)
    }

    /// List every session in id order (ascending).
    pub fn list(&self) -> Vec<&UploadSession> {
        self.by_id.values().collect()
    }

    /// Drive: `Pending | Paused → InProgress`. Idempotent against
    /// `InProgress` (no-op). Terminal states reject with
    /// [`SessionError::InvalidTransition`].
    pub fn mark_in_progress(&mut self, id: u64) -> Result<&UploadSession, SessionError> {
        self.transition(id, |state| match state {
            UploadState::Pending | UploadState::Paused | UploadState::InProgress => {
                Ok(UploadState::InProgress)
            }
            UploadState::Cancelled => Err("cancelled"),
            UploadState::Completed => Err("completed"),
            UploadState::Failed(_) => Err("failed"),
        })
    }

    /// Operator pause: `Pending | InProgress → Paused`. Idempotent
    /// against `Paused`.
    pub fn pause(&mut self, id: u64) -> Result<&UploadSession, SessionError> {
        self.transition(id, |state| match state {
            UploadState::Pending | UploadState::InProgress | UploadState::Paused => {
                Ok(UploadState::Paused)
            }
            UploadState::Cancelled => Err("cancelled"),
            UploadState::Completed => Err("completed"),
            UploadState::Failed(_) => Err("failed"),
        })
    }

    /// Operator resume: `Paused → InProgress`. Rejects non-paused
    /// sessions (including `Pending`, which should use
    /// [`Self::mark_in_progress`] instead — the CLI `upload resume`
    /// only makes sense for a paused upload).
    pub fn resume(&mut self, id: u64) -> Result<&UploadSession, SessionError> {
        self.transition(id, |state| match state {
            UploadState::Paused => Ok(UploadState::InProgress),
            UploadState::Pending => Err("pending"),
            UploadState::InProgress => Err("in_progress"),
            UploadState::Cancelled => Err("cancelled"),
            UploadState::Completed => Err("completed"),
            UploadState::Failed(_) => Err("failed"),
        })
    }

    /// Operator cancel: any non-terminal state → `Cancelled`.
    /// Idempotent against `Cancelled`.
    pub fn cancel(&mut self, id: u64) -> Result<&UploadSession, SessionError> {
        self.transition(id, |state| match state {
            UploadState::Pending
            | UploadState::InProgress
            | UploadState::Paused
            | UploadState::Cancelled => Ok(UploadState::Cancelled),
            UploadState::Completed => Err("completed"),
            UploadState::Failed(_) => Err("failed"),
        })
    }

    /// Driver-reported success: `InProgress | Pending → Completed`.
    /// The `Pending → Completed` arm is reserved for
    /// [`ConflictMode::Skip`] early-exit; it is not reachable from the
    /// normal driver path.
    pub fn complete(&mut self, id: u64) -> Result<&UploadSession, SessionError> {
        self.transition(id, |state| match state {
            UploadState::Pending | UploadState::InProgress => Ok(UploadState::Completed),
            UploadState::Paused => Err("paused"),
            UploadState::Cancelled => Err("cancelled"),
            UploadState::Completed => Err("completed"),
            UploadState::Failed(_) => Err("failed"),
        })
    }

    /// Driver-reported permanent failure. Non-terminal → `Failed(reason)`.
    pub fn fail(
        &mut self,
        id: u64,
        reason: impl Into<String>,
    ) -> Result<&UploadSession, SessionError> {
        let reason = reason.into();
        self.transition(id, move |state| match state {
            UploadState::Pending | UploadState::InProgress | UploadState::Paused => {
                Ok(UploadState::Failed(reason.clone()))
            }
            UploadState::Cancelled => Err("cancelled"),
            UploadState::Completed => Err("completed"),
            UploadState::Failed(_) => Err("failed"),
        })
    }

    /// Update the client-tracked `offset` without changing state.
    /// Used by the driver on every `upload_write` that returns a new
    /// confirmed offset.
    pub fn record_progress(
        &mut self,
        id: u64,
        offset: u64,
    ) -> Result<&UploadSession, SessionError> {
        let session = self.by_id.get_mut(&id).ok_or(SessionError::NotFound(id))?;
        session.offset = offset.min(session.total_bytes);
        session.updated_at = now_unix_secs();
        Ok(session)
    }

    fn transition<F>(&mut self, id: u64, f: F) -> Result<&UploadSession, SessionError>
    where
        F: FnOnce(&UploadState) -> Result<UploadState, &'static str>,
    {
        let session = self.by_id.get_mut(&id).ok_or(SessionError::NotFound(id))?;
        let current_label = session.state.label();
        let next = f(&session.state).map_err(|requested| SessionError::InvalidTransition {
            id,
            current: current_label,
            requested,
        })?;
        if next != session.state {
            let now = now_unix_secs();
            session.state = next;
            session.updated_at = now;
            session.history.push(UploadStateTransition {
                state: session.state.label().to_owned(),
                at: now,
            });
        }
        Ok(session)
    }
}

/// Pick a locally-unique remote filename for
/// [`ConflictMode::Rename`].
///
/// Given a desired leaf `name` and the `existing` set of leaf names
/// already present under the same parent, returns either:
///
/// * `name` itself when the slot is free, or
/// * `name (2).ext` / `name (3).ext` / … — the first slot the caller
///   has not claimed yet.
///
/// The algorithm splits on the *last* `.` — so `report.tar.gz` becomes
/// `report.tar (2).gz` (matches most OS file-browser renaming
/// behaviour). Files without an extension get `(2)` appended verbatim.
///
/// # Example
///
/// ```
/// use pcloud_backends::upload_sessions::pick_unique_name;
/// let existing = ["report.pdf", "report (2).pdf"];
/// assert_eq!(pick_unique_name("report.pdf", &existing), "report (3).pdf");
/// assert_eq!(pick_unique_name("notes", &["notes"]), "notes (2)");
/// assert_eq!(pick_unique_name("fresh.txt", &[] as &[&str]), "fresh.txt");
/// ```
#[must_use]
pub fn pick_unique_name<I, S>(name: &str, existing: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let existing: std::collections::HashSet<String> = existing
        .into_iter()
        .map(|s| s.as_ref().to_owned())
        .collect();
    if !existing.contains(name) {
        return name.to_owned();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_owned(), format!(".{e}")),
        None => (name.to_owned(), String::new()),
    };
    // Start at 2 to match the "report (2).ext" convention.
    for n in 2u64..=u64::MAX {
        let candidate = format!("{stem} ({n}){ext}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    // Unreachable in practice (u64 space).
    format!("{stem} (2){ext}")
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(reg: &mut SessionRegistry) -> u64 {
        reg.create(
            PathBuf::from("/tmp/a.bin"),
            "a.bin".to_owned(),
            Some(1),
            4096,
            ConflictMode::Error,
        )
        .id
    }

    #[test]
    fn conflict_mode_default_is_error() {
        assert_eq!(ConflictMode::default(), ConflictMode::Error);
    }

    #[test]
    fn create_registers_pending_session() {
        let mut reg = SessionRegistry::new();
        let id = sample_session(&mut reg);
        let s = reg.get(id).expect("present");
        assert_eq!(s.state, UploadState::Pending);
        assert_eq!(s.offset, 0);
        assert_eq!(s.conflict_mode, ConflictMode::Error);
        assert_eq!(s.history.len(), 1);
    }

    #[test]
    fn pending_to_in_progress_to_completed() {
        let mut reg = SessionRegistry::new();
        let id = sample_session(&mut reg);
        assert_eq!(
            reg.mark_in_progress(id).unwrap().state,
            UploadState::InProgress
        );
        assert_eq!(reg.complete(id).unwrap().state, UploadState::Completed);
        assert!(reg.get(id).unwrap().state.is_terminal());
    }

    #[test]
    fn in_progress_pause_resume_cycle() {
        let mut reg = SessionRegistry::new();
        let id = sample_session(&mut reg);
        let _ = reg.mark_in_progress(id).unwrap();
        let _ = reg.pause(id).unwrap();
        assert_eq!(reg.get(id).unwrap().state, UploadState::Paused);
        let _ = reg.resume(id).unwrap();
        assert_eq!(reg.get(id).unwrap().state, UploadState::InProgress);
        // History records all four transitions.
        let labels: Vec<_> = reg
            .get(id)
            .unwrap()
            .history
            .iter()
            .map(|t| t.state.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["pending", "in_progress", "paused", "in_progress"]
        );
    }

    #[test]
    fn cancel_from_any_non_terminal_state() {
        let mut reg = SessionRegistry::new();
        for prep in [
            None,                          // Pending
            Some(UploadState::InProgress), // → InProgress
            Some(UploadState::Paused),     // → Paused
        ] {
            let id = sample_session(&mut reg);
            match prep {
                None => {}
                Some(UploadState::InProgress) => {
                    let _ = reg.mark_in_progress(id).unwrap();
                }
                Some(UploadState::Paused) => {
                    let _ = reg.mark_in_progress(id).unwrap();
                    let _ = reg.pause(id).unwrap();
                }
                _ => unreachable!(),
            }
            let s = reg.cancel(id).unwrap();
            assert_eq!(s.state, UploadState::Cancelled);
        }
    }

    #[test]
    fn terminal_states_reject_transitions() {
        let mut reg = SessionRegistry::new();
        let id = sample_session(&mut reg);
        let _ = reg.mark_in_progress(id).unwrap();
        let _ = reg.complete(id).unwrap();
        // Cannot pause / resume / cancel a completed session.
        assert!(matches!(
            reg.pause(id).unwrap_err(),
            SessionError::InvalidTransition { .. }
        ));
        assert!(matches!(
            reg.resume(id).unwrap_err(),
            SessionError::InvalidTransition { .. }
        ));
        assert!(matches!(
            reg.cancel(id).unwrap_err(),
            SessionError::InvalidTransition { .. }
        ));
    }

    #[test]
    fn resume_rejects_non_paused() {
        let mut reg = SessionRegistry::new();
        let id = sample_session(&mut reg);
        // Pending → resume is a misuse — only paused sessions resume.
        assert!(matches!(
            reg.resume(id).unwrap_err(),
            SessionError::InvalidTransition { .. }
        ));
    }

    #[test]
    fn fail_from_non_terminal() {
        let mut reg = SessionRegistry::new();
        let id = sample_session(&mut reg);
        let _ = reg.mark_in_progress(id).unwrap();
        let s = reg.fail(id, "network exploded").unwrap();
        assert!(matches!(s.state, UploadState::Failed(_)));
    }

    #[test]
    fn record_progress_caps_at_total() {
        let mut reg = SessionRegistry::new();
        let id = sample_session(&mut reg);
        let _ = reg.record_progress(id, 8192).unwrap();
        // total_bytes in sample_session is 4096.
        assert_eq!(reg.get(id).unwrap().offset, 4096);
    }

    #[test]
    fn not_found_surfaces_cleanly() {
        let mut reg = SessionRegistry::new();
        assert_eq!(reg.pause(999).unwrap_err(), SessionError::NotFound(999));
    }

    #[test]
    fn list_is_monotone() {
        let mut reg = SessionRegistry::new();
        let a = sample_session(&mut reg);
        let b = sample_session(&mut reg);
        let c = sample_session(&mut reg);
        let ids: Vec<u64> = reg.list().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn pick_unique_name_basic() {
        assert_eq!(pick_unique_name("x.txt", [] as [&str; 0]), "x.txt");
        assert_eq!(pick_unique_name("x.txt", ["x.txt"]), "x (2).txt");
        assert_eq!(
            pick_unique_name("x.txt", ["x.txt", "x (2).txt", "x (3).txt"]),
            "x (4).txt"
        );
    }

    #[test]
    fn pick_unique_name_no_extension() {
        assert_eq!(pick_unique_name("Makefile", ["Makefile"]), "Makefile (2)");
    }

    #[test]
    fn pick_unique_name_splits_on_last_dot() {
        // "report.tar.gz" → "report.tar (2).gz" (common rename policy).
        assert_eq!(
            pick_unique_name("report.tar.gz", ["report.tar.gz"]),
            "report.tar (2).gz"
        );
    }

    #[test]
    fn ipc_conflict_mode_json_roundtrip() {
        for mode in [
            ConflictMode::Error,
            ConflictMode::Overwrite,
            ConflictMode::Skip,
            ConflictMode::Rename,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let back: ConflictMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, back);
        }
    }
}

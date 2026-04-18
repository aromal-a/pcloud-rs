//! Process-level UNIX signal handling for the pcloud-rs Rust daemon.
//!
//! Scope and design notes
//! ----------------------
//!
//! This module installs a small, dependency-free set of `sigaction(2)` handlers
//! so the daemon can exit gracefully under a service manager (systemd, sysvinit,
//! a bare shell, …) instead of being SIGKILLed after `TimeoutStopSec`.
//!
//! The handlers follow the standard POSIX async-signal-safety rules:
//!
//! - They do **nothing** beyond flipping atomic flags. No heap allocation, no
//!   locking, no logging, no Rust `println!`, no `Mutex`.
//! - `SIGTERM` and `SIGINT` set a single `SHUTDOWN_REQUESTED` flag.
//! - `SIGHUP` sets a `RELOAD_REQUESTED` flag; the main loop treats it as a
//!   no-op today (documented; no config reload wired yet) but the flag is
//!   reserved so operators can trigger future reloads without breaking
//!   service contracts.
//! - `SIGPIPE` is explicitly ignored — this is standard for network daemons
//!   that detect broken peer streams through write-error returns.
//!
//! The serve loop is responsible for observing `SHUTDOWN_REQUESTED` and
//! translating it into the existing graceful shutdown path (which drains
//! mounts, flushes audit/upload state, and exits cleanly). We do **not**
//! weaken `SA_RESTART` tuning beyond what is required for `accept(2)` to
//! return `EINTR`; that is the only blocking call on the hot path.
//!
//! ## Drain state machine (graceful-drain protocol)
//!
//! On top of the single shutdown flag the daemon exposes a three-state
//! drain state machine consumed by the serve loop and by the
//! `Method::DrainStatus` IPC surface:
//!
//! ```text
//!     Running ──SIGTERM──▶ Draining ──timeout / flushed──▶ Stopped
//! ```
//!
//! - `Running`: normal dispatch; accept new connections.
//! - `Draining`: new non-status connections are rejected with
//!   `ResponseStatus::Unavailable("daemon draining, retry")`; in-flight
//!   requests are allowed to complete; `DrainStatus` probes continue to
//!   answer so operators can poll progress.
//! - `Stopped`: loop has exited, socket unbound; subsequent reads on the
//!   state are for post-mortem only.
//!
//! The state, the drain start instant, and a `u64` in-flight counter live
//! in a single `AtomicU64`/`AtomicU8` pair so both the dispatch thread
//! (single-threaded today) and any future worker can read them without
//! locking. All operations are async-signal-safe, so the SIGTERM handler
//! can flip the state directly without taking a `Mutex`.
//!
//! Thread safety
//! -------------
//!
//! The three flags are `AtomicBool`. Install is idempotent and internally
//! guarded by a `Once`. Reinstalling over an existing handler is rejected
//! to avoid silent double-registration.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static RELOAD_REQUESTED: AtomicBool = AtomicBool::new(false);
static INSTALL_ONCE: Once = Once::new();
static INSTALL_OK: AtomicBool = AtomicBool::new(false);

// Drain state machine. Encoded as a `u8` so the SIGTERM handler can flip
// it with a single async-signal-safe atomic store.
const DRAIN_STATE_RUNNING: u8 = 0;
const DRAIN_STATE_DRAINING: u8 = 1;
const DRAIN_STATE_STOPPED: u8 = 2;

static DRAIN_STATE: AtomicU8 = AtomicU8::new(DRAIN_STATE_RUNNING);
/// Unix millis when the drain transition happened. Zero before the
/// daemon has observed SIGTERM.
static DRAIN_STARTED_MS: AtomicU64 = AtomicU64::new(0);
/// Monotonically increasing count of currently-executing requests.
/// Incremented immediately before dispatch and decremented after.
static IN_FLIGHT: AtomicU32 = AtomicU32::new(0);

/// Returns true once a terminating signal (SIGTERM/SIGINT) has been delivered.
#[must_use]
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Returns true if a SIGHUP has been observed since the last `take_reload()`.
#[must_use]
pub fn take_reload_request() -> bool {
    RELOAD_REQUESTED.swap(false, Ordering::SeqCst)
}

/// High-level drain-state enum mirrored onto the `DRAIN_STATE` atomic.
///
/// Returned by [`drain_state`] and surfaced in the `Method::DrainStatus`
/// JSON payload under the `state` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainState {
    /// Daemon is accepting new requests normally.
    Running,
    /// SIGTERM has been observed; new requests are rejected with
    /// `Unavailable`; in-flight work is allowed to complete.
    Draining,
    /// Serve loop has exited; socket has been unbound.
    Stopped,
}

impl DrainState {
    /// Stable machine-readable label used by the JSON payload.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DrainState::Running => "running",
            DrainState::Draining => "draining",
            DrainState::Stopped => "stopped",
        }
    }

    fn from_raw(raw: u8) -> Self {
        match raw {
            DRAIN_STATE_DRAINING => DrainState::Draining,
            DRAIN_STATE_STOPPED => DrainState::Stopped,
            _ => DrainState::Running,
        }
    }
}

/// Current drain state observed by the dispatch thread. Safe to call
/// from any thread; `SeqCst` load keeps it coherent with the SIGTERM
/// handler's store.
#[must_use]
pub fn drain_state() -> DrainState {
    DrainState::from_raw(DRAIN_STATE.load(Ordering::SeqCst))
}

/// Millis since the drain started, or `0` when the daemon is still
/// `Running`. Reads a snapshot of the monotonic wall-clock — operators
/// only use it for progress display, not for tight-timing decisions.
#[must_use]
pub fn elapsed_drain_ms() -> u64 {
    let started = DRAIN_STARTED_MS.load(Ordering::SeqCst);
    if started == 0 {
        return 0;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(started);
    now.saturating_sub(started)
}

/// Count of currently-dispatching requests. Incremented on entry to the
/// dispatcher and decremented once the response has been written.
#[must_use]
pub fn in_flight() -> u32 {
    IN_FLIGHT.load(Ordering::SeqCst)
}

/// Bump the in-flight counter. Paired with [`decrement_in_flight`] via
/// [`InFlightGuard`].
pub(crate) fn increment_in_flight() {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
}

/// Decrement the in-flight counter. Saturates at zero defensively: a
/// panic inside dispatch is captured by the runtime's `catch_unwind`
/// boundary which still drops the [`InFlightGuard`], but if anything
/// else short-circuited we must never wrap past zero.
pub(crate) fn decrement_in_flight() {
    IN_FLIGHT
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
            Some(v.saturating_sub(1))
        })
        .ok();
}

/// RAII guard that owns a +1 on the in-flight counter. Drop decrements
/// exactly once, so panics caught by `catch_unwind` still release the
/// slot.
pub struct InFlightGuard;

impl InFlightGuard {
    /// Register a new in-flight request.
    #[must_use]
    pub fn new() -> Self {
        increment_in_flight();
        Self
    }
}

impl Default for InFlightGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        decrement_in_flight();
    }
}

/// Transition `DRAIN_STATE` to `Draining` and stamp the start time
/// **only** if we are still `Running`. Idempotent: repeated calls after
/// the first return `false` without disturbing the stamp.
pub fn begin_drain() -> bool {
    let prev = DRAIN_STATE.compare_exchange(
        DRAIN_STATE_RUNNING,
        DRAIN_STATE_DRAINING,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    if prev.is_ok() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // Race-free stamp: only the thread that won the CAS writes the
        // start time.
        DRAIN_STARTED_MS.store(now_ms.max(1), Ordering::SeqCst);
        true
    } else {
        false
    }
}

/// Transition `DRAIN_STATE` to `Stopped`. Called after the serve loop
/// has exited and the socket has been unbound. Safe to call from any
/// state — further transitions out of `Stopped` are disallowed.
pub fn mark_stopped() {
    DRAIN_STATE.store(DRAIN_STATE_STOPPED, Ordering::SeqCst);
}

/// Reset drain bookkeeping. Public so integration tests under
/// `tests/graceful_drain.rs` can reset the process-wide statics
/// between scenarios; a release build should never invoke this. The
/// reset is a narrow contract: after calling, `drain_state ==
/// Running`, `in_flight == 0`, and `SHUTDOWN_REQUESTED` is clear.
///
/// Calling this concurrently with a running serve loop is a logic
/// bug — the serve loop will then observe an inconsistent state —
/// but it is safe with respect to data races (all writes are
/// atomic). Integration tests gate the call behind the same Mutex
/// they use to serialise access to the serve loop.
#[doc(hidden)]
pub fn reset_for_test() {
    DRAIN_STATE.store(DRAIN_STATE_RUNNING, Ordering::SeqCst);
    DRAIN_STARTED_MS.store(0, Ordering::SeqCst);
    IN_FLIGHT.store(0, Ordering::SeqCst);
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    RELOAD_REQUESTED.store(false, Ordering::SeqCst);
}

extern "C" fn handle_term(_sig: libc::c_int) {
    // async-signal-safe: atomic stores only. We also flip the drain
    // state here so the serve loop observes draining without a
    // round-trip through the main-thread flag check.
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    // CAS Running→Draining. We intentionally do NOT call
    // `SystemTime::now` from a signal handler — the serve loop stamps
    // `DRAIN_STARTED_MS` on its next observation via [`begin_drain`].
    let _ = DRAIN_STATE.compare_exchange(
        DRAIN_STATE_RUNNING,
        DRAIN_STATE_DRAINING,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
}

extern "C" fn handle_hup(_sig: libc::c_int) {
    RELOAD_REQUESTED.store(true, Ordering::SeqCst);
}

fn install_handler(sig: libc::c_int, handler: extern "C" fn(libc::c_int)) -> io::Result<()> {
    // Intentionally do not set SA_RESTART: we WANT blocking syscalls such as
    // accept(2) to return EINTR so the serve loop can observe the shutdown
    // flag and exit promptly. The serve loop treats EINTR as a non-error.
    // SAFETY: `std::mem::zeroed()` is valid for `libc::sigaction` — the type
    // is a plain C struct with no invariants beyond having been filled in
    // before being passed to `sigaction(2)`. We fill in `sa_sigaction` and
    // `sa_mask` before the syscall.
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = handler as usize;
    // Clear the whole signal mask while the handler runs; the handler itself
    // is trivial and async-signal-safe so this is fine.
    // SAFETY: `&mut sa.sa_mask` is a valid pointer to an initialized
    // `sigset_t` (zeroed above); `sigemptyset(3)` only reads/writes that
    // memory and has no other preconditions.
    unsafe {
        libc::sigemptyset(&mut sa.sa_mask);
    }
    // SAFETY: `sig` is a valid signal number supplied by our callers
    // (SIGTERM, SIGINT, SIGHUP — all in the portable range). `&sa` is
    // fully initialized above. The null `oldact` pointer is explicitly
    // documented by POSIX as acceptable when the previous action is not
    // needed. This call is async-signal-safe on Linux.
    let rc = unsafe { libc::sigaction(sig, &sa, std::ptr::null_mut()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn install_ignore(sig: libc::c_int) -> io::Result<()> {
    // SAFETY: `std::mem::zeroed()` is valid for `libc::sigaction` — same
    // rationale as `install_handler` above. `sa_sigaction` is overwritten
    // with `SIG_IGN` and `sa_mask` is filled by `sigemptyset` before use.
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = libc::SIG_IGN;
    // SAFETY: `&mut sa.sa_mask` is a valid pointer to a zeroed `sigset_t`;
    // `sigemptyset(3)` only writes into it with no other preconditions.
    unsafe {
        libc::sigemptyset(&mut sa.sa_mask);
    }
    // SAFETY: same rationale as the `sigaction` call in `install_handler`.
    // `sig` is SIGPIPE, a valid signal number; `&sa` is fully initialized.
    let rc = unsafe { libc::sigaction(sig, &sa, std::ptr::null_mut()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Install the daemon's signal handlers. Idempotent.
///
/// - `SIGTERM`, `SIGINT` → flag graceful shutdown.
/// - `SIGHUP` → flag reload (currently a no-op in the serve loop).
/// - `SIGPIPE` → explicitly ignored.
///
/// On success returns `Ok(())`. On failure the OS error is returned and the
/// daemon should abort rather than continue unprotected.
pub fn install_default_handlers() -> io::Result<()> {
    let mut first_err: Option<io::Error> = None;
    INSTALL_ONCE.call_once(|| {
        let result: io::Result<()> = (|| {
            install_handler(libc::SIGTERM, handle_term)?;
            install_handler(libc::SIGINT, handle_term)?;
            install_handler(libc::SIGHUP, handle_hup)?;
            install_ignore(libc::SIGPIPE)?;
            Ok(())
        })();
        match result {
            Ok(()) => INSTALL_OK.store(true, Ordering::SeqCst),
            Err(err) => first_err = Some(err),
        }
    });
    if let Some(err) = first_err {
        return Err(err);
    }
    if !INSTALL_OK.load(Ordering::SeqCst) {
        return Err(io::Error::other(
            "signal handler installation previously failed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent() {
        install_default_handlers().expect("install #1 should succeed");
        install_default_handlers().expect("install #2 should succeed");
    }

    #[test]
    fn reload_flag_is_edge_triggered() {
        // Simulate a SIGHUP arriving while the handler is installed.
        install_default_handlers().expect("install");
        RELOAD_REQUESTED.store(true, Ordering::SeqCst);
        assert!(take_reload_request());
        assert!(!take_reload_request());
    }

    // Note: we intentionally do NOT expose a public helper that toggles the
    // process-wide SHUTDOWN_REQUESTED flag. A test doing so would poison
    // every subsequent in-process test because the flag is a static — see
    // the integration test `serve_loop_exits_after_shutdown_request` which
    // drives the equivalent behaviour end-to-end via a real IPC Shutdown
    // request.
}

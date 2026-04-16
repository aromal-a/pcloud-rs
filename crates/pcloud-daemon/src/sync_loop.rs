//! Autonomous background sync loop.
//!
//! Drives incremental synchronization of all active (non-paused) sync roots
//! on a configurable interval. Runs as a `std::thread` (matching codebase
//! convention — no tokio) and communicates with the IPC dispatch thread
//! through shared atomics and a condvar-based wake signal.
//!
//! ## Lifecycle
//!
//! 1. Spawned by [`spawn_sync_loop`] after bootstrap completes.
//! 2. Each iteration: for every active sync root, poll remote diff, ingest
//!    local scan entries, reconcile, resolve conflicts, and advance
//!    transfers.
//! 3. Sleeps until the next poll interval, or wakes early on condvar
//!    signal (new root added, root paused/resumed, shutdown).
//! 4. On shutdown signal: finishes the current cycle, then exits.
//!
//! ## Thread safety
//!
//! The loop takes an `Arc<Mutex<SyncLoopState>>` for shared mutable state
//! that the IPC thread can read/wake. The engine and runtime are
//! exclusively owned by the loop thread and communicated back through the
//! shared state snapshot.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pcloud_config::sync_loop::SyncLoopConfig;
use pcloud_model::sync::SyncType;
use pcloud_secret::secret_string::SecretString;
use pcloud_store::repositories::sync_graph::SyncRootRecord;
use serde::{Deserialize, Serialize};

/// Snapshot of the sync loop's status, readable from the IPC thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncLoopStatus {
    /// Current loop state.
    pub state: SyncLoopState,
    /// Number of active (non-paused) sync roots.
    pub active_roots: usize,
    /// Unix timestamp (seconds) of the last completed cycle, or 0 if
    /// no cycle has run yet.
    pub last_cycle_at: u64,
    /// Duration in milliseconds of the last completed cycle.
    pub last_cycle_duration_ms: u64,
    /// Number of uploads pending in the engine scheduler.
    pub pending_uploads: usize,
    /// Number of downloads pending in the engine scheduler.
    pub pending_downloads: usize,
    /// Total cycles completed since the loop started.
    pub cycles_completed: u64,
    /// Total errors encountered across all cycles.
    pub total_errors: u64,
}

impl Default for SyncLoopStatus {
    fn default() -> Self {
        Self {
            state: SyncLoopState::Idle,
            active_roots: 0,
            last_cycle_at: 0,
            last_cycle_duration_ms: 0,
            pending_uploads: 0,
            pending_downloads: 0,
            cycles_completed: 0,
            total_errors: 0,
        }
    }
}

/// State of the background sync loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncLoopState {
    /// The loop is between cycles, waiting for the next poll interval.
    Idle,
    /// The loop is actively processing a sync cycle.
    Running,
    /// The loop is globally paused (via `PauseSync` IPC command).
    Paused,
    /// The loop was never started because config has `enabled = false`.
    Disabled,
    /// The loop has exited (shutdown completed).
    Stopped,
}

impl std::fmt::Display for SyncLoopState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Disabled => write!(f, "disabled"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// Shared state between the sync loop thread and the IPC dispatch thread.
#[derive(Debug)]
pub struct SyncLoopShared {
    /// Current status snapshot, updated by the loop thread after each
    /// cycle and read by the IPC thread for `GetSyncStatus`.
    pub status: Mutex<SyncLoopStatus>,
    /// Condvar used to wake the loop early (on root add/remove/pause/
    /// resume/shutdown). The mutex bool is a spurious-wakeup guard.
    pub wake: (Mutex<bool>, Condvar),
    /// Shutdown signal. Once set to `true`, the loop finishes its
    /// current cycle and exits.
    pub shutdown: AtomicBool,
    /// Global pause flag. When `true`, the loop skips all roots and
    /// sleeps until unpaused or shutdown.
    pub paused: AtomicBool,
}

impl SyncLoopShared {
    /// Create a new shared state with the given initial state.
    #[must_use]
    pub fn new(initial_state: SyncLoopState) -> Self {
        Self {
            status: Mutex::new(SyncLoopStatus {
                state: initial_state,
                ..SyncLoopStatus::default()
            }),
            wake: (Mutex::new(false), Condvar::new()),
            shutdown: AtomicBool::new(false),
            paused: AtomicBool::new(false),
        }
    }

    /// Signal the sync loop to wake up immediately (e.g. after
    /// sync-add, sync-remove, sync-resume).
    pub fn wake(&self) {
        let (lock, cvar) = &self.wake;
        if let Ok(mut woken) = lock.lock() {
            *woken = true;
            cvar.notify_one();
        }
    }

    /// Request shutdown and wake the loop.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake();
    }

    /// Pause the sync loop globally.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
        if let Ok(mut status) = self.status.lock() {
            status.state = SyncLoopState::Paused;
        }
    }

    /// Resume the sync loop globally.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
        self.wake();
    }

    /// Read the current status snapshot.
    #[must_use]
    pub fn current_status(&self) -> SyncLoopStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Result of a single sync cycle iteration for one root.
#[derive(Debug, Clone, Default)]
pub struct RootCycleResult {
    /// Number of remote diff entries ingested.
    pub remote_changes: usize,
    /// Number of local scan entries ingested.
    pub local_changes: usize,
    /// Number of uploads advanced.
    pub uploads: usize,
    /// Number of downloads advanced.
    pub downloads: usize,
    /// Number of conflicts detected.
    pub conflicts: usize,
    /// Number of errors encountered.
    pub errors: usize,
}

/// Result of a full sync cycle across all roots.
#[derive(Debug, Clone, Default)]
pub struct CycleResult {
    /// Per-root results.
    pub roots_processed: usize,
    /// Aggregate uploads.
    pub total_uploads: usize,
    /// Aggregate downloads.
    pub total_downloads: usize,
    /// Aggregate conflicts.
    pub total_conflicts: usize,
    /// Aggregate errors.
    pub total_errors: usize,
    /// Duration of the cycle.
    pub duration: Duration,
}

/// Trait abstracting the sync runtime dependencies so the loop can be
/// tested with mocks. The real implementation delegates to
/// `RuntimeShell`'s one-shot methods.
pub trait SyncLoopRuntime: Send + 'static {
    /// Get the current auth token, if authenticated.
    fn auth_token(&self) -> Option<SecretString>;

    /// List all persisted sync root records.
    fn list_sync_roots(&self) -> Vec<SyncRootRecord>;

    /// Check if a sync root is paused in the engine.
    fn is_sync_root_paused(&self, root: &SyncRootRecord) -> bool;

    /// Run one remote diff poll for a sync root. Returns the count of
    /// planned operations generated.
    fn poll_remote_diff(
        &mut self,
        root: &SyncRootRecord,
        auth_token: &SecretString,
    ) -> Result<usize, String>;

    /// Run one local scan pass for a sync root. Returns the count of
    /// planned operations generated.
    fn run_local_scan(&mut self, root: &SyncRootRecord) -> Result<usize, String>;

    /// Advance the transfer cycle (move queued ops to in-flight).
    fn advance_transfers(&mut self) -> usize;

    /// Execute active downloads. Returns count of completed downloads.
    fn execute_downloads(&mut self, auth_token: &SecretString) -> Result<usize, String>;

    /// Execute active uploads. Returns count of completed uploads.
    fn execute_uploads(&mut self, auth_token: &SecretString) -> Result<usize, String>;

    /// Get current conflict count.
    fn conflict_count(&self) -> usize;

    /// Get pending upload count.
    fn pending_upload_count(&self) -> usize;

    /// Get pending download count.
    fn pending_download_count(&self) -> usize;

    /// Emit an audit event for a completed cycle.
    fn emit_cycle_audit(&mut self, root_id: u64, result: &CycleResult);
}

/// Run one sync cycle for a single root, respecting its `SyncType`.
fn sync_one_root(
    runtime: &mut dyn SyncLoopRuntime,
    root: &SyncRootRecord,
    auth_token: &SecretString,
) -> RootCycleResult {
    let mut result = RootCycleResult::default();

    // 1. Remote diff poll (skip for UploadOnly)
    if root.sync_type != SyncType::UploadOnly {
        match runtime.poll_remote_diff(root, auth_token) {
            Ok(count) => result.remote_changes = count,
            Err(_err) => result.errors += 1,
        }
    }

    // 2. Local scan (skip for DownloadOnly)
    if root.sync_type != SyncType::DownloadOnly {
        match runtime.run_local_scan(root) {
            Ok(count) => result.local_changes = count,
            Err(_err) => result.errors += 1,
        }
    }

    // 3. Advance transfers into in-flight slots
    let _advanced = runtime.advance_transfers();

    // 4. Execute downloads (skip for UploadOnly)
    if root.sync_type != SyncType::UploadOnly {
        match runtime.execute_downloads(auth_token) {
            Ok(count) => result.downloads = count,
            Err(_err) => result.errors += 1,
        }
    }

    // 5. Execute uploads (skip for DownloadOnly)
    if root.sync_type != SyncType::DownloadOnly {
        match runtime.execute_uploads(auth_token) {
            Ok(count) => result.uploads = count,
            Err(_err) => result.errors += 1,
        }
    }

    // 6. Conflict count
    result.conflicts = runtime.conflict_count();

    result
}

/// Run one full sync cycle across all active roots.
pub fn run_cycle(runtime: &mut dyn SyncLoopRuntime, config: &SyncLoopConfig) -> CycleResult {
    let started = Instant::now();
    let mut cycle = CycleResult::default();

    let auth_token = match runtime.auth_token() {
        Some(token) => token,
        None => {
            // Not authenticated — skip the cycle entirely.
            cycle.duration = started.elapsed();
            return cycle;
        }
    };

    let roots = runtime.list_sync_roots();
    let _ = config.batch_size; // reserved for future batching refinement

    for root in &roots {
        if root.paused || runtime.is_sync_root_paused(root) {
            continue;
        }

        let root_result = sync_one_root(runtime, root, &auth_token);

        cycle.roots_processed += 1;
        cycle.total_uploads += root_result.uploads;
        cycle.total_downloads += root_result.downloads;
        cycle.total_conflicts += root_result.conflicts;
        cycle.total_errors += root_result.errors;
    }

    cycle.duration = started.elapsed();

    // Emit audit event for the full cycle
    runtime.emit_cycle_audit(0, &cycle);

    cycle
}

/// Wait on the shared condvar for the given duration. Clears the wake
/// flag after returning so spurious wake-ups are absorbed.
fn wait_on_condvar(shared: &SyncLoopShared, timeout: Duration) {
    let (lock, cvar) = &shared.wake;
    if let Ok(guard) = lock.lock() {
        let shutdown = &shared.shutdown;
        // `wait_timeout_while` returns the guard and whether it timed out.
        // We only care about clearing the flag afterwards.
        if let Ok((mut g, _)) = cvar.wait_timeout_while(guard, timeout, |woken| {
            !*woken && !shutdown.load(Ordering::Acquire)
        }) {
            *g = false;
        }
    }
}

/// The main sync loop function, intended to run on a dedicated thread.
///
/// Loops until `shared.shutdown` is set. Each iteration:
/// 1. Check shutdown/pause.
/// 2. Run one full cycle via [`run_cycle`].
/// 3. Update the shared status snapshot.
/// 4. Sleep for `poll_interval` or wake early on condvar signal.
pub fn sync_loop_main(
    runtime: &mut dyn SyncLoopRuntime,
    config: SyncLoopConfig,
    shared: Arc<SyncLoopShared>,
) {
    let poll_interval = Duration::from_secs(config.poll_interval_secs);

    loop {
        // Check shutdown
        if shared.shutdown.load(Ordering::Acquire) {
            if let Ok(mut status) = shared.status.lock() {
                status.state = SyncLoopState::Stopped;
            }
            return;
        }

        // Check global pause
        if shared.paused.load(Ordering::Acquire) {
            if let Ok(mut status) = shared.status.lock() {
                status.state = SyncLoopState::Paused;
            }
            // Wait on condvar until woken (resume or shutdown).
            wait_on_condvar(&shared, Duration::from_secs(1));
            continue;
        }

        // Mark as running
        if let Ok(mut status) = shared.status.lock() {
            status.state = SyncLoopState::Running;
        }

        // Run one full cycle
        let cycle = run_cycle(runtime, &config);

        // Update shared status
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if let Ok(mut status) = shared.status.lock() {
            status.state = if shared.paused.load(Ordering::Acquire) {
                SyncLoopState::Paused
            } else {
                SyncLoopState::Idle
            };
            status.active_roots = cycle.roots_processed;
            status.last_cycle_at = now_unix;
            status.last_cycle_duration_ms = cycle.duration.as_millis() as u64;
            status.pending_uploads = runtime.pending_upload_count();
            status.pending_downloads = runtime.pending_download_count();
            status.cycles_completed += 1;
            status.total_errors += cycle.total_errors as u64;
        }

        // Sleep for poll_interval, waking early on condvar signal
        if shared.shutdown.load(Ordering::Acquire) {
            if let Ok(mut status) = shared.status.lock() {
                status.state = SyncLoopState::Stopped;
            }
            return;
        }

        wait_on_condvar(&shared, poll_interval);
    }
}

/// Handle returned from [`spawn_sync_loop`] that allows the caller to
/// join the background thread and access shared state.
pub struct SyncLoopHandle {
    /// Shared state for status queries and wake signals.
    pub shared: Arc<SyncLoopShared>,
    /// Join handle for the background thread.
    thread: Option<JoinHandle<()>>,
}

impl SyncLoopHandle {
    /// Request shutdown and wait for the loop thread to exit.
    ///
    /// Returns `Ok(())` if the thread exits cleanly, or `Err(())` if
    /// the thread panicked.
    #[allow(clippy::result_unit_err)]
    pub fn shutdown_and_join(mut self) -> Result<(), ()> {
        self.shared.request_shutdown();
        if let Some(handle) = self.thread.take() {
            handle.join().map_err(|_| ())
        } else {
            Ok(())
        }
    }

    /// Check if the loop thread is still running.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for SyncLoopHandle {
    fn drop(&mut self) {
        self.shared.request_shutdown();
        // Best-effort join on drop; do not block indefinitely.
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Spawn the background sync loop on a dedicated `std::thread`.
///
/// Returns a [`SyncLoopHandle`] the caller uses to query status, wake
/// the loop, and join on shutdown.
///
/// If `config.enabled` is `false`, returns a handle with a `Disabled`
/// state and no background thread.
pub fn spawn_sync_loop<R: SyncLoopRuntime>(
    mut runtime: R,
    config: SyncLoopConfig,
    shared: Arc<SyncLoopShared>,
) -> SyncLoopHandle {
    if !config.enabled {
        if let Ok(mut status) = shared.status.lock() {
            status.state = SyncLoopState::Disabled;
        }
        return SyncLoopHandle {
            shared,
            thread: None,
        };
    }

    let shared_clone = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name("pcloud-sync-loop".to_owned())
        .spawn(move || {
            sync_loop_main(&mut runtime, config, shared_clone);
        })
        .expect("failed to spawn sync loop thread");

    SyncLoopHandle {
        shared,
        thread: Some(thread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_model::ids::SyncId;
    use std::sync::atomic::AtomicUsize;

    /// Mock runtime that tracks method call counts.
    struct MockRuntime {
        roots: Vec<SyncRootRecord>,
        poll_count: Arc<AtomicUsize>,
        scan_count: Arc<AtomicUsize>,
        download_count: Arc<AtomicUsize>,
        upload_count: Arc<AtomicUsize>,
        advance_count: Arc<AtomicUsize>,
        audit_count: Arc<AtomicUsize>,
        authenticated: bool,
    }

    impl MockRuntime {
        fn new(roots: Vec<SyncRootRecord>) -> Self {
            Self {
                roots,
                poll_count: Arc::new(AtomicUsize::new(0)),
                scan_count: Arc::new(AtomicUsize::new(0)),
                download_count: Arc::new(AtomicUsize::new(0)),
                upload_count: Arc::new(AtomicUsize::new(0)),
                advance_count: Arc::new(AtomicUsize::new(0)),
                audit_count: Arc::new(AtomicUsize::new(0)),
                authenticated: true,
            }
        }
    }

    impl SyncLoopRuntime for MockRuntime {
        fn auth_token(&self) -> Option<SecretString> {
            if self.authenticated {
                Some(SecretString::new("mock-token".to_owned()))
            } else {
                None
            }
        }

        fn list_sync_roots(&self) -> Vec<SyncRootRecord> {
            self.roots.clone()
        }

        fn is_sync_root_paused(&self, _root: &SyncRootRecord) -> bool {
            false
        }

        fn poll_remote_diff(
            &mut self,
            _root: &SyncRootRecord,
            _auth: &SecretString,
        ) -> Result<usize, String> {
            self.poll_count.fetch_add(1, Ordering::Relaxed);
            Ok(0)
        }

        fn run_local_scan(&mut self, _root: &SyncRootRecord) -> Result<usize, String> {
            self.scan_count.fetch_add(1, Ordering::Relaxed);
            Ok(0)
        }

        fn advance_transfers(&mut self) -> usize {
            self.advance_count.fetch_add(1, Ordering::Relaxed);
            0
        }

        fn execute_downloads(&mut self, _auth: &SecretString) -> Result<usize, String> {
            self.download_count.fetch_add(1, Ordering::Relaxed);
            Ok(0)
        }

        fn execute_uploads(&mut self, _auth: &SecretString) -> Result<usize, String> {
            self.upload_count.fetch_add(1, Ordering::Relaxed);
            Ok(0)
        }

        fn conflict_count(&self) -> usize {
            0
        }

        fn pending_upload_count(&self) -> usize {
            0
        }

        fn pending_download_count(&self) -> usize {
            0
        }

        fn emit_cycle_audit(&mut self, _root_id: u64, _result: &CycleResult) {
            self.audit_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn make_root(id: u64, paused: bool, sync_type: SyncType) -> SyncRootRecord {
        SyncRootRecord {
            sync_id: SyncId::new(id),
            local_path: format!("/tmp/sync-test-{id}"),
            remote_path: format!("/Remote/{id}"),
            paused,
            sync_type,
        }
    }

    #[test]
    fn cycle_processes_active_roots_skips_paused() {
        let roots = vec![
            make_root(1, false, SyncType::Full),
            make_root(2, true, SyncType::Full), // paused
            make_root(3, false, SyncType::Full),
        ];
        let mut runtime = MockRuntime::new(roots);
        let config = SyncLoopConfig::default();

        let result = run_cycle(&mut runtime, &config);

        // 2 active roots processed (root 2 is paused)
        assert_eq!(result.roots_processed, 2);
        // Each active root gets: 1 poll + 1 scan = 2 polls, 2 scans
        assert_eq!(runtime.poll_count.load(Ordering::Relaxed), 2);
        assert_eq!(runtime.scan_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn cycle_respects_download_only_sync_type() {
        let roots = vec![make_root(1, false, SyncType::DownloadOnly)];
        let mut runtime = MockRuntime::new(roots);
        let config = SyncLoopConfig::default();

        let _result = run_cycle(&mut runtime, &config);

        // DownloadOnly: poll yes, scan no, downloads yes, uploads no
        assert_eq!(runtime.poll_count.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.scan_count.load(Ordering::Relaxed), 0);
        assert_eq!(runtime.download_count.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.upload_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cycle_respects_upload_only_sync_type() {
        let roots = vec![make_root(1, false, SyncType::UploadOnly)];
        let mut runtime = MockRuntime::new(roots);
        let config = SyncLoopConfig::default();

        let _result = run_cycle(&mut runtime, &config);

        // UploadOnly: poll no, scan yes, downloads no, uploads yes
        assert_eq!(runtime.poll_count.load(Ordering::Relaxed), 0);
        assert_eq!(runtime.scan_count.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.download_count.load(Ordering::Relaxed), 0);
        assert_eq!(runtime.upload_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cycle_skips_when_not_authenticated() {
        let roots = vec![make_root(1, false, SyncType::Full)];
        let mut runtime = MockRuntime::new(roots);
        runtime.authenticated = false;
        let config = SyncLoopConfig::default();

        let result = run_cycle(&mut runtime, &config);

        assert_eq!(result.roots_processed, 0);
        assert_eq!(runtime.poll_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn shutdown_exits_loop() {
        let roots = vec![make_root(1, false, SyncType::Full)];
        let poll_count = Arc::new(AtomicUsize::new(0));
        let poll_clone = Arc::clone(&poll_count);

        let mut runtime = MockRuntime::new(roots);
        runtime.poll_count = poll_clone;

        let config = SyncLoopConfig {
            poll_interval_secs: 5,
            ..SyncLoopConfig::default()
        };
        let shared = Arc::new(SyncLoopShared::new(SyncLoopState::Idle));

        // Signal shutdown before starting so the loop exits after one
        // iteration.
        let shared_clone = Arc::clone(&shared);
        let handle = thread::spawn(move || {
            // Give the loop a moment to start, then shut it down.
            thread::sleep(Duration::from_millis(100));
            shared_clone.request_shutdown();
        });

        let sync_shared = Arc::clone(&shared);
        sync_loop_main(&mut runtime, config, sync_shared);

        handle.join().unwrap();

        let status = shared.current_status();
        assert_eq!(status.state, SyncLoopState::Stopped);
    }

    #[test]
    fn spawn_disabled_returns_disabled_handle() {
        let roots = vec![make_root(1, false, SyncType::Full)];
        let runtime = MockRuntime::new(roots);
        let config = SyncLoopConfig {
            enabled: false,
            ..SyncLoopConfig::default()
        };
        let shared = Arc::new(SyncLoopShared::new(SyncLoopState::Idle));

        let handle = spawn_sync_loop(runtime, config, shared);

        assert_eq!(
            handle.shared.current_status().state,
            SyncLoopState::Disabled
        );
        assert!(!handle.is_alive());
    }

    #[test]
    fn spawn_and_shutdown_within_timeout() {
        let roots = vec![make_root(1, false, SyncType::Full)];
        let runtime = MockRuntime::new(roots);
        let config = SyncLoopConfig {
            poll_interval_secs: 5,
            ..SyncLoopConfig::default()
        };
        let shared = Arc::new(SyncLoopShared::new(SyncLoopState::Idle));

        let handle = spawn_sync_loop(runtime, config, shared);

        // Let it run briefly
        thread::sleep(Duration::from_millis(50));
        assert!(handle.is_alive());

        // Shutdown
        let result = handle.shutdown_and_join();
        assert!(result.is_ok());
    }

    #[test]
    fn integration_two_roots_three_cycles() {
        let roots = vec![
            make_root(1, false, SyncType::Full),
            make_root(2, false, SyncType::DownloadOnly),
        ];
        let poll_count = Arc::new(AtomicUsize::new(0));
        let scan_count = Arc::new(AtomicUsize::new(0));

        let mut runtime = MockRuntime::new(roots);
        runtime.poll_count = Arc::clone(&poll_count);
        runtime.scan_count = Arc::clone(&scan_count);

        let config = SyncLoopConfig::default();

        // Run 3 cycles manually
        for _ in 0..3 {
            let _result = run_cycle(&mut runtime, &config);
        }

        // Root 1 (Full): polled + scanned each cycle = 3 polls, 3 scans
        // Root 2 (DownloadOnly): polled each cycle, no scan = 3 polls, 0 scans
        // Total: 6 polls, 3 scans
        assert_eq!(poll_count.load(Ordering::Relaxed), 6);
        assert_eq!(scan_count.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn sync_loop_status_serde_roundtrip() {
        let status = SyncLoopStatus {
            state: SyncLoopState::Running,
            active_roots: 3,
            last_cycle_at: 1700000000,
            last_cycle_duration_ms: 150,
            pending_uploads: 5,
            pending_downloads: 2,
            cycles_completed: 42,
            total_errors: 1,
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: SyncLoopStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, back);
    }

    #[test]
    fn wake_signal_interrupts_sleep() {
        let roots = vec![make_root(1, false, SyncType::Full)];
        let runtime = MockRuntime::new(roots);
        let config = SyncLoopConfig {
            poll_interval_secs: 3600, // very long interval
            ..SyncLoopConfig::default()
        };
        let shared = Arc::new(SyncLoopShared::new(SyncLoopState::Idle));

        let handle = spawn_sync_loop(runtime, config, shared);

        // Wait for first cycle to start
        thread::sleep(Duration::from_millis(50));

        // Wake to trigger immediate re-evaluation, then shutdown
        handle.shared.wake();
        thread::sleep(Duration::from_millis(50));
        handle.shared.request_shutdown();

        // Should exit quickly despite the 3600s interval
        let start = Instant::now();
        let result = handle.shutdown_and_join();
        assert!(result.is_ok());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}

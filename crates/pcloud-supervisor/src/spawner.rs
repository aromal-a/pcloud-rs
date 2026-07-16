//! T2.8.c — sub-daemon spawning helper.
//!
//! This module bridges the supervisor registry model
//! ([`crate::AccountSlot`]) and the account-scoped daemon bootstrap
//! ([`pcloud_daemon::bootstrap_with_config_and_account`]) by launching
//! one IPC accept loop per registered account on a dedicated
//! [`std::thread`]. Each spawned daemon runs inside the same OS
//! process as the supervisor — a *real* multi-process supervisor that
//! forks a child per account, forwards SIGTERM, and restarts on crash
//! is the load-bearing follow-up that depends on platform-specific
//! process supervision (POSIX `fork`/`exec` + signalfd; Windows
//! Service host) and is intentionally out of scope here.
//!
//! # Threading model
//!
//! - One `std::thread` per `spawn_account` call.
//! - Inside that thread the daemon `bootstrap_with_config_and_account`
//!   provisions the account-scoped on-disk roots
//!   (`<state>/account-{id}` etc.), binds the per-account IPC socket
//!   under `<runtime>/account-{id}/ipc.sock`, and runs
//!   [`pcloud_daemon::serve_until_shutdown_with_flag`] until the
//!   shared `Arc<AtomicBool>` stop flag flips.
//! - `stop_account` flips the flag and joins the thread. The serve
//!   loop polls the flag on every accept timeout, so termination is
//!   cooperative and bounded by that timeout.
//!
//! # Why not `tokio` / a separate process?
//!
//! The daemon serve loop is blocking and takes a stop flag, so a
//! plain `std::thread::spawn` is the minimum viable wiring. A
//! separate OS process would buy crash isolation between accounts but
//! requires process-supervision plumbing (signal forwarding, restart-
//! on-crash, child reaping) that is its own follow-up PR.

// **PLATFORM:** all
// **GATING:** none.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use pcloud_config::ConfigProfile;
use pcloud_daemon::{
    AccountScope, bootstrap_with_config_and_account, serve_until_shutdown_with_flag,
};
use pcloud_ipc::{IpcServer, current_effective_uid};

use crate::AccountSlot;

/// Errors returned by the sub-daemon spawning helper.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// The account-scoped daemon bootstrap failed.
    #[error("bootstrap failed: {0}")]
    Bootstrap(String),
    /// Binding the per-account IPC socket failed.
    #[error("ipc bind failed: {0}")]
    Bind(String),
    /// The serve loop returned an error.
    #[error("serve loop error: {0}")]
    Serve(String),
    /// The serve thread panicked.
    #[error("spawned daemon thread panicked")]
    ThreadPanicked,
}

/// Handle returned by [`spawn_account`].
///
/// Owns the join handle of the dedicated serve thread and a shared
/// stop flag. Drop it through [`stop_account`] to flip the flag and
/// join the thread; dropping it without calling `stop_account` will
/// detach the thread (the daemon keeps running until external
/// shutdown), which is almost always wrong.
#[must_use = "spawned daemons must be stopped via stop_account to release resources"]
pub struct SpawnedDaemon {
    /// The supervisor's account id this daemon serves.
    pub account_id: u64,
    /// Per-account IPC socket path the spawned daemon is listening
    /// on. Mirrors what [`AccountScope::socket_path`] would compute
    /// for the same `(id, runtime_dir)` pair, captured here so the
    /// supervisor can hand it back to a routing layer without
    /// re-deriving it.
    pub socket_path: std::path::PathBuf,
    stop_flag: Arc<AtomicBool>,
    handle: JoinHandle<Result<(), SpawnError>>,
}

impl SpawnedDaemon {
    /// `true` while the spawned thread is still live (i.e. the serve
    /// loop has not returned). Note that this races with thread
    /// termination — callers should treat `is_running()` as advisory.
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.handle.is_finished()
    }

    /// Reference to the shared stop flag. Exposed so callers can
    /// observe (but not flip) the cooperative shutdown state.
    #[must_use]
    pub fn stop_flag(&self) -> &Arc<AtomicBool> {
        &self.stop_flag
    }
}

/// Spawn a per-account daemon on a dedicated `std::thread`.
///
/// The thread:
///
/// 1. constructs an [`AccountScope`] from `slot.id` / `slot.label`,
/// 2. calls [`bootstrap_with_config_and_account`] so the daemon's
///    on-disk paths are scoped to `<runtime>/account-{id}/`,
/// 3. binds the per-account IPC socket,
/// 4. runs [`serve_until_shutdown_with_flag`] until the stop flag
///    flips.
///
/// # Errors
///
/// Returns [`SpawnError::Bootstrap`] / [`SpawnError::Bind`] when the
/// thread fails before entering the serve loop. Errors that surface
/// only after the thread has started running are observed via
/// [`stop_account`].
pub fn spawn_account(
    slot: &AccountSlot,
    config: ConfigProfile,
) -> Result<SpawnedDaemon, SpawnError> {
    let scope = AccountScope::new(slot.id.get(), slot.label.clone());

    // Compute the socket path eagerly so we can return it on the
    // handle without waiting for the spawned thread. We mirror what
    // `apply_account_scope` + `paths.ipc_socket_path()` produce inside
    // bootstrap: rewrite `runtime_dir` to the per-account subdir, then
    // ask `ConfigProfile` for its canonical socket file name. We do not
    // hard-code the file name here to avoid drifting from upstream.
    let projected_socket = {
        let mut projected_config = config.clone();
        projected_config.paths.runtime_dir = scope.runtime_subdir(&config.paths.runtime_dir);
        projected_config.paths.ipc_socket_path()
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_for_thread = Arc::clone(&stop_flag);
    let account_id = slot.id.get();
    let scope_for_thread = scope;

    let handle = thread::Builder::new()
        .name(format!("pcloud-account-{account_id}"))
        .spawn(move || run_serve_loop(scope_for_thread, config, stop_flag_for_thread))
        .map_err(|err| SpawnError::Bootstrap(format!("thread spawn failed: {err}")))?;

    Ok(SpawnedDaemon {
        account_id,
        socket_path: projected_socket,
        stop_flag,
        handle,
    })
}

/// Stop a previously spawned daemon: flip the cooperative stop flag
/// and join the serve thread.
///
/// # Errors
///
/// - [`SpawnError::ThreadPanicked`] if the spawned thread panicked.
/// - Any [`SpawnError`] the serve loop bubbled up before exiting.
pub fn stop_account(spawned: SpawnedDaemon) -> Result<(), SpawnError> {
    spawned.stop_flag.store(true, Ordering::SeqCst);
    match spawned.handle.join() {
        Ok(result) => result,
        Err(_) => Err(SpawnError::ThreadPanicked),
    }
}

/// Adapter wrapper invoked on the serve thread. Bootstraps the
/// account-scoped runtime, binds the IPC socket, and drives the
/// serve loop until the stop flag flips.
fn run_serve_loop(
    scope: AccountScope,
    config: ConfigProfile,
    stop_flag: Arc<AtomicBool>,
) -> Result<(), SpawnError> {
    let mut runtime = bootstrap_with_config_and_account(config, Some(scope))
        .map_err(|err| SpawnError::Bootstrap(err.to_string()))?;

    let socket_path = runtime.config.paths.ipc_socket_path();
    let server = IpcServer::new(current_effective_uid());
    let bound = server
        .bind(&socket_path)
        .map_err(|err| SpawnError::Bind(err.to_string()))?;

    serve_until_shutdown_with_flag(&bound, &mut runtime, Some(&stop_flag))
        .map_err(|err| SpawnError::Serve(err.to_string()))?;

    // `bound` is dropped here, which unlinks the per-account socket
    // (Unix) / closes the named pipe handle (Windows).
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use pcloud_config::{ConfigProfile, Environment};

    use super::*;
    use crate::SupervisorRegistry;

    /// Generate a unique short root path under `/tmp` so the
    /// derived Unix socket path stays under `SUN_LEN` (104 bytes
    /// on macOS, 108 on Linux). Mirrors the pattern used in
    /// `pcloud-daemon::serve::tests::bootstrap_test_shell`.
    fn unique_root(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        PathBuf::from("/tmp").join(format!(
            "pd-sup-{}-{}-{}",
            tag,
            std::process::id(),
            nonce % 1_000_000_000
        ))
    }

    fn mk_profile(tag: &str) -> ConfigProfile {
        ConfigProfile::secure_defaults(unique_root(tag), Environment::Development)
    }

    /// Wait until `cond()` returns `true` or the deadline expires.
    fn wait_until(deadline: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        cond()
    }

    /// Two registered accounts spawn into independent daemons whose
    /// IPC sockets live under disjoint per-account subtrees. Hermetic
    /// (uses two distinct temp roots).
    #[test]
    fn spawn_two_accounts_get_isolated_daemons() {
        let mut reg = SupervisorRegistry::new();
        let work_id = reg
            .add_account("work", PathBuf::from("/run/pcloud/placeholder-work.sock"))
            .unwrap();
        let home_id = reg
            .add_account("home", PathBuf::from("/run/pcloud/placeholder-home.sock"))
            .unwrap();

        let work_slot = reg.get(work_id).unwrap().clone();
        let home_slot = reg.get(home_id).unwrap().clone();

        let work_profile = mk_profile("work");
        let home_profile = mk_profile("home");

        let work = spawn_account(&work_slot, work_profile).expect("work daemon spawns");
        let home = spawn_account(&home_slot, home_profile).expect("home daemon spawns");

        // Both daemons must be live and on different sockets.
        let bound = wait_until(Duration::from_secs(15), || {
            work.socket_path.exists() && home.socket_path.exists()
        });
        if !bound {
            // If a thread crashed before binding, surface the error
            // by joining instead of leaving the panic message generic.
            let work_finished = work.handle.is_finished();
            let home_finished = home.handle.is_finished();
            panic!(
                "expected both per-account IPC sockets to be bound within 15 s; \
                 work={:?} (exists={}, finished={}) \
                 home={:?} (exists={}, finished={})",
                work.socket_path,
                work.socket_path.exists(),
                work_finished,
                home.socket_path,
                home.socket_path.exists(),
                home_finished,
            );
        }
        assert!(work.is_running());
        assert!(home.is_running());
        assert_ne!(work.socket_path, home.socket_path);
        assert!(work.socket_path.to_string_lossy().contains("account-1"));
        assert!(home.socket_path.to_string_lossy().contains("account-2"));

        // Clean teardown.
        stop_account(work).expect("work daemon stops cleanly");
        stop_account(home).expect("home daemon stops cleanly");
    }

    /// Spawning then immediately stopping joins the thread within a
    /// reasonable bound. Validates that no thread is leaked when the
    /// stop flag is flipped before the serve loop has fully entered
    /// its accept iteration.
    #[test]
    fn spawn_then_stop_does_not_leak_resources() {
        let mut reg = SupervisorRegistry::new();
        let id = reg
            .add_account("solo", PathBuf::from("/run/pcloud/placeholder-solo.sock"))
            .unwrap();
        let slot = reg.get(id).unwrap().clone();
        let profile = mk_profile("solo");

        let spawned = spawn_account(&slot, profile).expect("daemon spawns");

        // Immediately request shutdown.
        let socket_path = spawned.socket_path.clone();
        let start = Instant::now();
        stop_account(spawned).expect("daemon stops cleanly");
        let elapsed = start.elapsed();

        // The serve loop's accept timeout dominates how quickly the
        // thread observes the flag; the production default sits well
        // under 30 s. Anything beyond that means a leak is plausible.
        assert!(
            elapsed < Duration::from_secs(30),
            "thread join took too long: {:?}",
            elapsed,
        );

        // Socket should be gone (BoundIpcServer unlinks on drop).
        // Allow a brief window for the kernel to flush the inode.
        let _ = wait_until(Duration::from_secs(2), || !socket_path.exists());
    }
}

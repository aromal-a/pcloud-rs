//! IPC accept loop: binds the Unix domain socket via `pcloud-ipc`,
//! enforces peer-UID and socket mode, and hands each accepted connection
//! to `dispatch`. Owns slow/malformed-client isolation and graceful
//! shutdown wiring. Called from `bootstrap::run`.
//!
//! Portable façade; production deployments run on Unix sockets.
//!
//! ## Graceful drain
//!
//! The loop cooperates with the three-state drain machine defined in
//! [`signals`]:
//!
//! 1. Normal operation → observed shutdown flag → [`signals::begin_drain`].
//! 2. Drain mode: new non-status connections are handled by
//!    `drain_gate`, which returns `ResponseStatus::Unavailable("daemon
//!    draining, retry")` for every method except [`Method::DrainStatus`]
//!    (always answered so clients can poll) and [`Method::Shutdown`] /
//!    [`Method::GetHealth`] (cheap probes useful to supervisors).
//!    In-flight requests admitted before the drain flipped continue to
//!    completion.
//! 3. The loop polls `in_flight == 0` every iteration; once the counter
//!    drains, or `drain_timeout_secs` expires, the loop returns so the
//!    caller can release the vault + store locks, unbind the socket,
//!    and exit `0`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Emit a systemd sd_notify(3) datagram when running under systemd.
///
/// Reads `NOTIFY_SOCKET` from the environment; if absent or if the send
/// fails the call is silently a no-op (the daemon operates normally
/// whether or not it is supervised by systemd). This avoids taking a
/// hard dependency on `libsystemd`.
#[cfg(target_os = "linux")]
fn sd_notify(msg: &str) {
    if let Ok(path) = std::env::var("NOTIFY_SOCKET") {
        use std::os::unix::net::UnixDatagram;
        match UnixDatagram::unbound() {
            Ok(sock) => {
                if let Err(err) = sock.send_to(msg.as_bytes(), &path) {
                    log::warn!("sd_notify: send_to({path:?}) failed: {err}");
                }
            }
            Err(err) => {
                log::warn!("sd_notify: failed to open unbound datagram socket: {err}");
            }
        }
    }
}

/// No-op supervisor signalling stub for macOS.
///
/// macOS uses launchd for daemon supervision; it does not use the systemd
/// sd_notify(3) protocol. This stub ensures that call sites gated with
/// `#[cfg(target_os = "linux")]` compile cleanly on macOS without requiring
/// conditional compilation at every call site. A launchd-native notification
/// path (e.g. launch_activate_socket(3)) should be added here if pcloud-daemon
/// is ever packaged as a launchd service.
///
/// See: `man 8 launchd`, `man 3 launch_activate_socket`.
///
/// TODO(pcloud-rs-0cx): Add launchd KeepAlive / XPC signalling if
/// macOS launchd packaging is in scope.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
fn sd_notify(_msg: &str) {
    // launchd does not use the sd_notify protocol. No-op intentionally.
}

/// No-op supervisor signalling stub for BSD and Windows.
///
/// Neither FreeBSD rc.d nor Windows SCM use the sd_notify protocol. On BSD,
/// the rc.d script supervises via `daemon(8)`; on Windows, the SCM uses
/// `SetServiceStatus`. If launchd/rc.d/SCM lifecycle signals become important,
/// wire platform-specific calls in the branches below.
///
/// TODO(pcloud-rs-0cx): BSD rc.d `daemon(8)` does not need sd_notify;
/// document the supervision story in packaging/freebsd/pcloudd.rc instead.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[allow(dead_code)]
fn sd_notify(_msg: &str) {
    // No supervisor protocol on BSD / Windows. No-op intentionally.
}

use pcloud_ipc::{
    BoundIpcServer, IpcServer, IpcTransportError, Method, Request, Response, ResponseStatus,
    current_effective_uid,
};

use pcloud_session::refresh_loop::{self, TickOutcome};

use crate::RuntimeShell;
use crate::signals::{self, DrainState};

/// Returns `true` for IPC request variants that perform privileged,
/// state-mutating, or security-sensitive operations. These are emitted
/// to the audit log before dispatch so operators can detect unexpected
/// privileged activity even without a full audit chain sweep.
///
/// Operations listed here match the enterprise audit M-2 finding:
/// shutdown, credential lifecycle, crypto lifecycle, auth persistence,
/// sync removal, backup/device operations.
fn is_privileged_request(req: &Request) -> bool {
    matches!(
        req,
        Request::Plain {
            method: Method::Shutdown
                | Method::CryptoReset
                | Method::SetAuthPersistence
                | Method::SendCryptoChangeUserPrivate
        } | Request::AccountChangePassword { .. }
            | Request::CryptoSetup { .. }
            | Request::CryptoSetupV2 { .. }
            | Request::CryptoGetFolderKey { .. }
            | Request::CryptoGetFileKey { .. }
            | Request::CryptoChangePassword { .. }
            | Request::CryptoChangePasswordUnlocked { .. }
            | Request::AuthPersistence { .. }
            | Request::SyncRootRemove { .. }
            | Request::DeleteBackup { .. }
            | Request::UploadWriteFromFile { .. }
            | Request::CreateTreePublicLinkFromPaths { .. }
            | Request::CreateBackup { .. }
            | Request::StopDevice { .. }
            | Request::DeleteBackupDevice
            | Request::LostPassword { .. }
            | Request::VerifyEmailRestricted { .. }
    )
}

/// Return a short, non-secret, human-readable name for the request
/// discriminant. Used in audit log lines so operators can correlate
/// events without reading secret field values.
fn request_kind_name(req: &Request) -> &'static str {
    match req {
        Request::Plain { method } => match method {
            Method::Shutdown => "Shutdown",
            Method::CryptoReset => "CryptoReset",
            Method::SetAuthPersistence => "SetAuthPersistence",
            Method::SendCryptoChangeUserPrivate => "SendCryptoChangeUserPrivate",
            _ => "Plain",
        },
        Request::AccountChangePassword { .. } => "AccountChangePassword",
        Request::CryptoSetup { .. } => "CryptoSetup",
        Request::CryptoSetupV2 { .. } => "CryptoSetupV2",
        Request::CryptoGetFolderKey { .. } => "CryptoGetFolderKey",
        Request::CryptoGetFileKey { .. } => "CryptoGetFileKey",
        Request::CryptoChangePassword { .. } => "CryptoChangePassword",
        Request::CryptoChangePasswordUnlocked { .. } => "CryptoChangePasswordUnlocked",
        Request::AuthPersistence { .. } => "AuthPersistence",
        Request::SyncRootRemove { .. } => "SyncRootRemove",
        Request::DeleteBackup { .. } => "DeleteBackup",
        Request::UploadWriteFromFile { .. } => "UploadWriteFromFile",
        Request::CreateTreePublicLinkFromPaths { .. } => "CreateTreePublicLinkFromPaths",
        Request::CreateBackup { .. } => "CreateBackup",
        Request::StopDevice { .. } => "StopDevice",
        Request::DeleteBackupDevice => "DeleteBackupDevice",
        Request::LostPassword { .. } => "LostPassword",
        Request::VerifyEmailRestricted { .. } => "VerifyEmailRestricted",
        _ => "other",
    }
}

/// Default polling interval when the loop has flipped into drain mode
/// and is waiting for in-flight work to settle or for the drain timeout
/// to expire. Shorter than the drain timeout so operators see timely
/// progress over `Method::DrainStatus`.
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Drive the IPC serve loop until one of the following happens:
///
/// - an IPC client invokes `Method::Shutdown`, which flips
///   `runtime.control.shutdown_requested`, or
/// - the process receives `SIGTERM` / `SIGINT`, observed via
///   [`signals::shutdown_requested`].
///
/// In both cases we return `Ok(())` after the current in-flight request (if
/// any) completes; the caller is then responsible for any additional drain
/// work (audit flush, upload teardown, mount unmount, …) before exiting.
///
/// Signal-driven wakeups translate to `EINTR` from the underlying `accept(2)`
/// call because we install handlers without `SA_RESTART`. That is reported
/// up as `IpcTransportError::Io(ErrorKind::Interrupted)`; we swallow it and
/// loop back so the flag check can take effect. Any other I/O error is
/// propagated unchanged.
///
/// Each accepted connection is dispatched on a **dedicated OS thread** so that
/// slow backend calls (auth RTT, crypto unlock) do not block the accept loop.
/// The `RuntimeShell` is wrapped in `Arc<Mutex<>>` internally; dispatch
/// serializes through the mutex while the accept loop itself stays
/// non-blocking. The connection cap ([`pcloud_ipc::MAX_IPC_CONNECTIONS`])
/// bounds worst-case thread count.
pub fn serve_until_shutdown(
    bound: &BoundIpcServer,
    runtime: &mut RuntimeShell,
) -> Result<(), IpcTransportError> {
    serve_until_shutdown_with_flag(bound, runtime, None)
}

/// Return `true` when the drain gate should short-circuit this request
/// with `ResponseStatus::Unavailable("daemon draining, retry")`. Admits
/// [`Method::DrainStatus`] (polled by `pcloudc drain`),
/// [`Method::Shutdown`] (so a second shutdown CTA still flips the
/// runtime flag), and [`Method::GetHealth`] (so systemd / k8s liveness
/// probes keep reporting the truth during drain).
fn should_reject_during_drain(request: &Request) -> bool {
    match request {
        Request::Plain { method } => !matches!(
            method,
            Method::DrainStatus | Method::Shutdown | Method::GetHealth | Method::Health
        ),
        _ => true,
    }
}

/// Drain-aware dispatch wrapper. When the daemon is draining, rejects
/// non-status requests with `Unavailable`; otherwise forwards to the
/// runtime dispatch path through `crate::dispatch`.
///
/// Privileged requests (shutdown, crypto setup/reset, password change,
/// auth persistence, sync removal, backup deletion) are emitted to the
/// structured log before dispatch so operators have an audit trail for
/// sensitive operations regardless of whether the full audit chain is
/// enabled. The peer uid is reported as the daemon owner uid because
/// `serve_once` already enforces that only owner-uid peers reach this
/// handler — unauthorized peers never produce a dispatch call.
pub(crate) fn dispatch_with_drain_gate(
    runtime: &mut RuntimeShell,
    peer_uid: u32,
    peer_pid: u32,
    request: Request,
) -> Response {
    if signals::drain_state() == DrainState::Draining && should_reject_during_drain(&request) {
        return Response {
            status: ResponseStatus::Unavailable,
            message: "daemon draining, retry".to_owned(),
        };
    }
    if is_privileged_request(&request) {
        // `peer_uid` / `peer_pid` come from SO_PEERCRED (Linux) /
        // getpeereid(3) (BSD/macOS) / GetNamedPipeClientProcessId
        // (Windows), resolved by pcloud-ipc at connection-accept time
        // and threaded through `serve_once_with_peer`. Using the peer
        // fields rather than `current_effective_uid()` (the daemon's
        // own uid) preserves audit-04 M-2 correctness under future
        // deployments where multiple authorized uids can share a
        // socket (e.g. when root is allow-listed).
        log::info!(
            "privileged IPC request: {} from uid={} pid={}",
            request_kind_name(&request),
            peer_uid,
            peer_pid,
        );
    }
    let _guard = signals::InFlightGuard::new();
    // ncx.54 (P3-E1): thread peer_pid through to the dispatch path via
    // `dispatch_with_peer_creds` so downstream audit sites can read it
    // from `runtime.current_peer_pid` without re-resolving peer state.
    crate::dispatch::dispatch_with_peer_creds(runtime, peer_uid, peer_pid, request)
}

/// Same as [`serve_until_shutdown`], but additionally honors an external
/// `Arc<AtomicBool>` shutdown flag. Used by the Windows Service shim and
/// any other host that owns the shutdown signal outside the daemon (e.g.
/// SCM `Stop`/`Shutdown` on Windows). When the external flag flips to
/// `true`, the loop returns cleanly just like the internal signal/IPC
/// shutdown paths, and the runtime-level flag is synchronized so the
/// rest of the runtime sees a single consistent source of truth.
pub fn serve_until_shutdown_with_flag(
    bound: &BoundIpcServer,
    runtime: &mut RuntimeShell,
    external: Option<&Arc<AtomicBool>>,
) -> Result<(), IpcTransportError> {
    // Set an accept timeout so the loop wakes periodically to run the
    // session-refresh tick even when the daemon is idle (no IPC
    // clients). The timeout matches the configured refresh check
    // interval, capped at 60 s to keep shutdown responsive. A zero
    // config value disables the background refresh entirely.
    let refresh_enabled = runtime.config.auth.refresh_check_interval_secs > 0;
    if let Some(timeout) = crate::session_refresh::accept_timeout(&runtime.config.auth) {
        let _ = bound.set_accept_timeout(Some(timeout));
    }

    let drain_timeout = Duration::from_secs(u64::from(runtime.config.upgrade.drain_timeout_secs));
    let mut drain_deadline: Option<Instant> = None;
    loop {
        let external_flagged = external
            .map(|flag| flag.load(Ordering::SeqCst))
            .unwrap_or(false);
        let shutdown_observed =
            runtime.control.shutdown_requested || signals::shutdown_requested() || external_flagged;

        if shutdown_observed {
            // Make sure the runtime-level flag reflects the signal-driven
            // shutdown so the rest of the codebase (health, metrics, drain
            // logic) sees a single consistent source of truth.
            runtime.control.shutdown_requested = true;
            if let Some(flag) = external {
                flag.store(true, Ordering::SeqCst);
            }
            // Transition Running→Draining exactly once; stamp the start
            // timestamp so DrainStatus reports `elapsed_drain_ms`.
            let fresh_drain = signals::begin_drain();
            if fresh_drain {
                // Notify systemd that the daemon is entering the drain/stop
                // phase. This transitions the service from active to
                // deactivating in the unit state machine.
                #[cfg(target_os = "linux")]
                sd_notify("STOPPING=1\n");
                drain_deadline = Some(Instant::now() + drain_timeout);
                // bd-1du.4: quiesce the mount on the *first* transition
                // into Draining. The kernel mount stays live so
                // in-flight `read(2)` calls from user processes can
                // complete within the drain grace window; we only
                // fsync the write journal and flush the writer
                // pipeline here. Actual unmount happens when
                // `MountControl::Drop` fires after the serve loop
                // returns. `quiesce_for_drain` is a no-op when there's
                // no active mount, so this is safe at every start.
                let summary = runtime.mount_control.quiesce_for_drain();
                if summary != "no active mount" {
                    log::info!("pcloud-rs drain: mount quiesce: {summary}");
                }
            } else if drain_deadline.is_none() {
                // Draining was triggered by the SIGTERM handler before
                // the loop observed it. We still need a deadline.
                drain_deadline = Some(Instant::now() + drain_timeout);
            }

            // Drain complete or timed out → exit the loop.
            let drained = signals::in_flight() == 0;
            let timed_out = drain_deadline.map(|d| Instant::now() >= d).unwrap_or(false);
            if drained || timed_out {
                return Ok(());
            }
            // Park briefly waiting for in-flight work to finish; the
            // short sleep keeps `Method::DrainStatus` pollable without
            // busy-spinning the CPU. `accept(2)` below is still honoured
            // so we continue to service DrainStatus probes during the
            // grace window.
            std::thread::sleep(DRAIN_POLL_INTERVAL);
        }

        // SIGHUP → hot-reload config from disk.
        if signals::take_reload_request()
            && let Some(ref config_path) = runtime.config_path
        {
            use crate::config_reload::{
                ReloadOutcome, format_reload_failed_event, format_reloaded_event, try_reload,
            };
            // Notify systemd that a reload is in progress. The READY=1
            // suffix re-arms the watchdog once the reload completes.
            #[cfg(target_os = "linux")]
            sd_notify("RELOADING=1\n");
            let (outcome, new_profile) = try_reload(config_path, &runtime.config);
            match outcome {
                ReloadOutcome::Applied { changed_keys } => {
                    let msg = format_reloaded_event(&changed_keys);
                    log::info!("pcloud-rs: {msg}");
                    if let Some(profile) = new_profile {
                        runtime.apply_hot_reload(profile);
                    }
                }
                ReloadOutcome::NoChange => {
                    // Config re-read but nothing changed. No audit event.
                }
                ReloadOutcome::Failed { error } => {
                    let msg = format_reload_failed_event(&error);
                    log::error!("pcloud-rs: {msg}");
                }
            }
            // Re-assert READY=1 to signal that the reload phase is
            // complete and the daemon is accepting connections again.
            #[cfg(target_os = "linux")]
            sd_notify("READY=1\n");
        }
        match bound.serve_once_with_peer(|peer, request| {
            dispatch_with_drain_gate(runtime, peer.uid, peer.pid, request)
        }) {
            Ok(()) => {}
            Err(IpcTransportError::Io(err)) if err.kind() == io::ErrorKind::Interrupted => {
                // Signal-driven wakeup. Loop back and re-check the shutdown
                // flag. This is expected on SIGTERM/SIGINT delivery.
                continue;
            }
            Err(IpcTransportError::Io(err))
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::TimedOut =>
            {
                // Accept timeout expired with no incoming connection.
                // Fall through to the refresh tick below.
            }
            Err(other) => return Err(other),
        }

        // Emit a watchdog keepalive so systemd knows the serve loop is
        // still making progress. A no-op when not supervised by systemd.
        #[cfg(target_os = "linux")]
        sd_notify("WATCHDOG=1\n");

        // Session-refresh tick: run one iteration of the proactive
        // token-refresh check. This is cheap when the session is
        // healthy (a single timestamp comparison); the transport is
        // only invoked when the supervisor classifies the token as
        // within the refresh window.
        if refresh_enabled {
            run_refresh_tick(runtime);
        }
    }
}

/// Top-level cooperative daemon entry point.
///
/// Mirrors the shape of `pcloudd serve` but takes ownership of lifecycle
/// coordination from an external host via the supplied `shutdown` flag.
/// Intended consumers:
///
/// - the Windows Service shim (`pcloud-daemon-win`), where the SCM
///   control handler flips the flag on `Stop`/`Shutdown` requests;
/// - any embedder that needs to drive daemon startup and shutdown from
///   a parent thread without relying on UNIX signals.
///
/// The function:
///
/// 1. installs the default signal handlers (idempotent; safe to call
///    from inside an SCM-hosted process where these handlers are still
///    meaningful for `SIGTERM`-equivalent shutdowns),
/// 2. bootstraps the `RuntimeShell`,
/// 3. binds the configured IPC socket,
/// 4. runs the serve loop via [`serve_until_shutdown_with_flag`] so both
///    the external flag and the internal SIGTERM/IPC flags are honored,
/// 5. returns `Ok(())` once any flag flips, allowing the caller to
///    proceed with teardown / SCM state reporting.
///
/// Any bootstrap, bind, or serve error is propagated as `anyhow::Error`
/// so the caller can log a single cause chain and exit with a non-zero
/// status.
pub fn serve_with_shutdown(shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    // Installing signal handlers is idempotent and async-signal-safe.
    // Doing it here keeps the behavior identical to `pcloudd serve`
    // when invoked via this entry point.
    signals::install_default_handlers()
        .map_err(|err| anyhow::anyhow!("failed to install signal handlers: {err}"))?;

    let mut runtime = crate::bootstrap_shell()
        .map_err(|err| anyhow::anyhow!("daemon bootstrap failed: {err}"))?;

    // Spawn the background sync loop before binding the IPC socket so
    // sync starts as soon as the daemon is ready.
    let store_path = runtime.config.paths.state_dir.join("store.sqlite3");
    let (sync_loop_handle, _sync_auth_token) = crate::sync_loop_runtime::spawn_daemon_sync_loop(
        &runtime.config,
        &runtime.auth,
        store_path,
    )
    .map_err(|err| anyhow::anyhow!("sync loop store connection failed: {err}"))?;
    runtime.sync_loop_shared = Some(sync_loop_handle.shared.clone());

    // Health HTTP server (GET /livez, GET /readyz). Disabled by default
    // (port 0). Enable by setting `PCLOUD_HEALTH_PORT=<port>` (must be
    // >= 1024). Binds to 127.0.0.1 only; external probes must go through
    // a reverse proxy or sidecar. The handle is intentionally kept alive
    // for the daemon lifetime — dropping it does not stop the thread, but
    // holding it makes the intent explicit.
    let _health_handle: Option<crate::health_server::HealthServerHandle> = {
        let port: u16 = std::env::var("PCLOUD_HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let cfg = crate::health_server::HealthServerConfig {
            http_port: port,
            read_timeout_ms: 2_000,
        };
        crate::health_server::spawn(cfg)
            .map_err(|err| anyhow::anyhow!("health server startup failed: {err}"))?
    };

    let socket_path = runtime.config.paths.ipc_socket_path();
    let server = IpcServer::new(current_effective_uid());
    let bound = server
        .bind(&socket_path)
        .map_err(|err| anyhow::anyhow!("daemon socket bind failed: {err}"))?;

    // Notify systemd that the daemon is fully up and ready to accept
    // connections. This unblocks any unit that depends on this service
    // (Type=notify). The call is a no-op when not supervised by systemd.
    //
    // NOTE — fork-after-bind embedders: if this daemon is ever refactored
    // to fork *after* calling bind() (e.g. to hand off the socket fd to a
    // supervisor), the sd_notify call must be moved to the child process and
    // must be preceded by `MAINPID=<child_pid>\n` so systemd tracks the
    // correct PID. Without MAINPID= the unit state machine follows the
    // parent (which will exit immediately) and may race with WatchdogSec
    // expiry before the child sends WATCHDOG=1. See sd_notify(3) §MAINPID.
    #[cfg(target_os = "linux")]
    sd_notify("READY=1\n");

    serve_until_shutdown_with_flag(&bound, &mut runtime, Some(&shutdown))
        .map_err(|err| anyhow::anyhow!("daemon request handling failed: {err}"))?;

    // Socket is dropped (and unlinked) with `bound` going out of scope.
    // Shut down the sync loop cleanly.
    let _ = sync_loop_handle.shutdown_and_join();
    // Mark the drain machine Stopped so any remaining in-process
    // observers (tests) see a clean terminal state.
    signals::mark_stopped();

    Ok(())
}

/// Run one iteration of the session-refresh tick against the runtime's
/// session supervisor and auth runtime. Outcomes are logged via
/// structured `log` calls and audit events are persisted best-effort.
fn run_refresh_tick(runtime: &mut RuntimeShell) {
    let outcome = refresh_loop::tick(
        &runtime.session_supervisor,
        &runtime.auth_runtime,
        &mut runtime.auth,
    );
    match outcome {
        Ok(TickOutcome::Refreshed) => {
            log::debug!("pcloud-session-refresh: token refreshed successfully");
        }
        Ok(TickOutcome::IdleLogout { audit_details }) => {
            log::warn!("pcloud-session-refresh: idle logout - {audit_details}");
            let _ = pcloud_store::append_audit_event(
                &mut runtime.store,
                "session",
                Some(&audit_details),
            );
        }
        Ok(TickOutcome::Expired) => {
            log::warn!("pcloud-session-refresh: session hard-expired; revoked");
        }
        Ok(TickOutcome::AuthExpired { result }) => {
            log::warn!(
                "pcloud-session-refresh: server reported auth expired (result={result}); revoked"
            );
        }
        Ok(TickOutcome::TemporaryFailure { reason }) => {
            log::warn!("pcloud-session-refresh: transient failure: {reason}");
        }
        Ok(TickOutcome::NoSession | TickOutcome::Ok | TickOutcome::AlreadyInFlight) => {}
        Err(e) => {
            log::error!("pcloud-session-refresh: tick error: {e}");
        }
    }
}

// End-to-end coverage for this loop lives at
// `crates/pcloud-daemon/src/lib.rs::tests::serve_loop_exits_after_shutdown_request`
// and exercises the full bind + IPC round-trip. We deliberately do not add a
// unit test here that pokes the process-wide SIGTERM flag, because that flag
// is a `static` and would leak into sibling tests running in the same process.

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use pcloud_config::{ConfigProfile, Environment};
    use pcloud_ipc::{IpcClient, IpcServer, Request, current_effective_uid};

    use super::{serve_until_shutdown_with_flag, should_reject_during_drain};
    use crate::bootstrap_with_config;

    fn bootstrap_test_shell() -> crate::RuntimeShell {
        // Use `/tmp` (not `std::env::temp_dir()`) so the fully-qualified
        // Unix-socket path stays under SUN_LEN on macOS, where the
        // per-user tempdir `/var/folders/.../T/` alone eats 49 chars.
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::path::PathBuf::from("/tmp").join(format!(
            "pd-srv-{}-{}",
            std::process::id(),
            nonce % 1_000_000_000
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        bootstrap_with_config(config).expect("runtime bootstrap should succeed")
    }

    /// Regression coverage for the `serve_with_shutdown` contract used by
    /// the Windows Service shim. The accept loop must return promptly
    /// once the externally-owned `Arc<AtomicBool>` flips to `true`.
    ///
    /// We exercise [`serve_until_shutdown_with_flag`] directly — that is
    /// the exact primitive `serve_with_shutdown` wraps — because the
    /// top-level helper also performs bootstrap and socket bind, which
    /// this test already does explicitly. The flag semantics being
    /// asserted here are identical.
    ///
    /// Mechanics: once the external flag is set we need one more
    /// `accept(2)` iteration for the loop to observe it, so the test
    /// sends a harmless IPC ping after flipping the flag. This mirrors
    /// how the SCM-plus-signal-driven shutdown path already behaves in
    /// `serve_loop_exits_after_shutdown_request`.
    #[test]
    fn serve_with_shutdown_exits_when_flag_set() {
        let mut runtime = bootstrap_test_shell();
        let socket_path = runtime.config.paths.ipc_socket_path();
        let server = IpcServer::new(current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        let flag = Arc::new(AtomicBool::new(false));
        let flag_for_thread = Arc::clone(&flag);

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let thread_barrier = Arc::clone(&barrier);

        let handle = std::thread::spawn(move || {
            thread_barrier.wait();
            let res = serve_until_shutdown_with_flag(&bound, &mut runtime, Some(&flag_for_thread));
            (res, runtime.control.shutdown_requested)
        });

        barrier.wait();

        // Flip the external flag, then nudge the accept loop so it
        // observes it. A single ping is enough; the response is
        // ignored because we only care about the loop returning.
        flag.store(true, Ordering::SeqCst);
        let client = IpcClient;
        let _ = client.send(
            &socket_path,
            &Request::Plain {
                method: pcloud_ipc::Method::GetStatus,
            },
        );

        // The loop must finish within 5s; anything longer indicates the
        // external flag is not actually being honored.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                panic!("serve loop did not exit within 5s of external flag flip");
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let (result, runtime_flag) = handle.join().expect("serve thread should join");
        result.expect("serve loop should exit cleanly");
        assert!(runtime_flag, "runtime shutdown flag should be mirrored");
        assert!(
            flag.load(Ordering::SeqCst),
            "external flag should remain set"
        );
    }

    #[test]
    fn drain_gate_admits_status_and_shutdown_probes() {
        // Every method that supervisors and `pcloudc drain` need during
        // a graceful shutdown must be pass-through. New methods added
        // later should appear here so the drain surface stays
        // predictable.
        assert!(!should_reject_during_drain(&Request::Plain {
            method: pcloud_ipc::Method::DrainStatus
        }));
        assert!(!should_reject_during_drain(&Request::Plain {
            method: pcloud_ipc::Method::Shutdown
        }));
        assert!(!should_reject_during_drain(&Request::Plain {
            method: pcloud_ipc::Method::GetHealth
        }));
        assert!(!should_reject_during_drain(&Request::Plain {
            method: pcloud_ipc::Method::Health
        }));
    }

    #[test]
    fn drain_gate_rejects_ordinary_traffic() {
        // Anything that could mutate state or perform expensive work is
        // rejected while the daemon is draining.
        assert!(should_reject_during_drain(&Request::Plain {
            method: pcloud_ipc::Method::GetStatus
        }));
        assert!(should_reject_during_drain(&Request::Plain {
            method: pcloud_ipc::Method::Logout
        }));
        assert!(should_reject_during_drain(&Request::Unmount));
    }
}

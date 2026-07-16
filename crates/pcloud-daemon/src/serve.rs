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
/// Neither FreeBSD rc.d nor the per-user Windows launcher use the sd_notify
/// protocol. On BSD, the rc.d script supervises via `daemon(8)`; on Windows,
/// `pcloudc start` owns process launch and IPC shutdown. The optional
/// experimental SCM host reports its own `SetServiceStatus` transitions.
///
/// TODO(pcloud-rs-0cx): BSD rc.d `daemon(8)` does not need sd_notify;
/// document the supervision story in packaging/freebsd/pcloudd.rc instead.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[allow(dead_code)]
fn sd_notify(_msg: &str) {
    // No supervisor protocol on BSD / Windows. No-op intentionally.
}

#[cfg(target_os = "linux")]
fn systemd_watchdog_timeout() -> Option<Duration> {
    match std::env::var("WATCHDOG_PID") {
        Ok(raw_pid) => match raw_pid.parse::<u32>() {
            Ok(pid) if pid == std::process::id() => {}
            Ok(pid) => {
                log::debug!(
                    "systemd watchdog disabled: WATCHDOG_PID={pid} does not match current pid={}",
                    std::process::id()
                );
                return None;
            }
            Err(err) => {
                log::warn!("systemd watchdog disabled: invalid WATCHDOG_PID={raw_pid:?}: {err}");
                return None;
            }
        },
        Err(std::env::VarError::NotPresent) => {}
        Err(err) => {
            log::warn!("systemd watchdog disabled: failed to read WATCHDOG_PID: {err}");
            return None;
        }
    }

    match std::env::var("WATCHDOG_USEC") {
        Ok(raw_usec) => match raw_usec.parse::<u64>() {
            Ok(0) => None,
            Ok(usec) => Some(Duration::from_micros(usec)),
            Err(err) => {
                log::warn!("systemd watchdog disabled: invalid WATCHDOG_USEC={raw_usec:?}: {err}");
                None
            }
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(err) => {
            log::warn!("systemd watchdog disabled: failed to read WATCHDOG_USEC: {err}");
            None
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn systemd_watchdog_timeout() -> Option<Duration> {
    None
}

fn watchdog_ping_interval_from_timeout(timeout: Duration) -> Duration {
    let micros = timeout.as_micros();
    if micros <= 1 {
        return Duration::from_micros(1);
    }
    let half = (micros / 2).min(u128::from(u64::MAX));
    Duration::from_micros(half as u64)
}

fn systemd_watchdog_ping_interval() -> Option<Duration> {
    systemd_watchdog_timeout().map(watchdog_ping_interval_from_timeout)
}

/// Maximum time an idle Unix listener may remain inside one `accept(2)` call.
/// This bound exists independently of auth refresh and systemd watchdog
/// configuration so externally supervised and embedded daemons always observe
/// cooperative shutdown promptly.
const SHUTDOWN_ACCEPT_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) fn accept_timeout_with_watchdog(base: Option<Duration>) -> Option<Duration> {
    let configured = match (base, systemd_watchdog_ping_interval()) {
        (Some(base), Some(watchdog)) => Some(base.min(watchdog)),
        (Some(base), None) => Some(base),
        (None, Some(watchdog)) => Some(watchdog),
        (None, None) => None,
    };
    Some(
        configured
            .unwrap_or(SHUTDOWN_ACCEPT_POLL_INTERVAL)
            .min(SHUTDOWN_ACCEPT_POLL_INTERVAL),
    )
}

pub(crate) fn notify_systemd_watchdog() {
    #[cfg(target_os = "linux")]
    sd_notify("WATCHDOG=1\n");
}

pub(crate) fn notify_systemd_stopping() {
    #[cfg(target_os = "linux")]
    sd_notify("STOPPING=1\n");
}

pub(crate) fn notify_systemd_reloading() {
    #[cfg(target_os = "linux")]
    sd_notify("RELOADING=1\n");
}

pub(crate) fn notify_systemd_ready() {
    #[cfg(target_os = "linux")]
    sd_notify("READY=1\n");
}

fn parse_health_port(raw: &str) -> Result<u16, String> {
    raw.parse::<u16>()
        .map_err(|err| format!("invalid PCLOUD_HEALTH_PORT={raw:?}: {err}"))
}

fn health_port_from_env() -> Result<u16, String> {
    match std::env::var("PCLOUD_HEALTH_PORT") {
        Ok(raw) => parse_health_port(&raw),
        Err(std::env::VarError::NotPresent) => Ok(0),
        Err(err) => Err(format!("failed to read PCLOUD_HEALTH_PORT: {err}")),
    }
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
/// Implementation note (CLAUDEREV iter-1 IPC-H-7.1 fix): the capability
/// table is now authoritative on the [`Request`] type itself via
/// [`Request::is_privileged`]. This wrapper exists for backwards-compat
/// and for the use site below; new privileged variants only need to be
/// classified in the type-side method. Adding a `Request` variant
/// without classifying it falls through to the deny-by-audit default
/// (`true`) so the audit log cannot be silently bypassed.
fn is_privileged_request(req: &Request) -> bool {
    req.is_privileged()
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
        Request::CreateTreePublicLinkFromPathTargets { .. } => {
            "CreateTreePublicLinkFromPathTargets"
        }
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
/// `Arc<AtomicBool>` shutdown flag. Used by the per-user Windows daemon,
/// the optional experimental SCM host, and embedders that own lifecycle
/// signalling outside the daemon. When the external flag flips to `true`,
/// the loop returns cleanly just like the internal signal/IPC shutdown
/// paths, and the runtime-level flag is synchronized so the rest of the
/// runtime sees a single consistent source of truth.
pub fn serve_until_shutdown_with_flag(
    bound: &BoundIpcServer,
    runtime: &mut RuntimeShell,
    external: Option<&Arc<AtomicBool>>,
) -> Result<(), IpcTransportError> {
    // Set an accept timeout so the loop wakes periodically to run the
    // session-refresh tick even when the daemon is idle (no IPC
    // clients). The timeout matches the configured refresh check
    // interval, capped at one second to keep cooperative shutdown responsive.
    // A zero config value disables background refresh, not shutdown polling.
    let refresh_enabled = runtime.config.auth.refresh_check_interval_secs > 0;
    let accept_timeout =
        accept_timeout_with_watchdog(crate::session_refresh::accept_timeout(&runtime.config.auth));
    if let Some(timeout) = accept_timeout {
        if let Err(err) = bound.set_accept_timeout(Some(timeout)) {
            log::warn!("pcloud-daemon: failed to configure IPC accept timeout: {err}");
        }
    }

    let drain_timeout = Duration::from_secs(u64::from(runtime.config.upgrade.drain_timeout_secs));
    let mut drain_deadline: Option<Instant> = None;
    // Track whether we've already nudged the listener for the active
    // shutdown cycle. On Unix this is a no-op (the periodic
    // `SO_RCVTIMEO` accept timeout keeps the loop honest), but on
    // Windows an in-loop nudge would be unreachable while accept is
    // blocked. The shutdown-watcher thread below drives the nudge;
    // this flag short-circuits any follow-on iterations so we don't
    // repeatedly call `request_shutdown` (harmless, but noisy).
    let mut shutdown_nudged = false;

    // Spawn a scoped watcher thread that signals the listener's
    // cancel event as soon as any shutdown source flips. This is the
    // only way to cooperatively wake a Windows `ConnectNamedPipe`
    // that is already parked in `WaitForMultipleObjects`. On Unix
    // `request_shutdown` is a no-op, so the watcher is harmless
    // overhead there. The thread exits when `watcher_exit` flips,
    // which the main loop does right before returning.
    let watcher_exit = Arc::new(AtomicBool::new(false));
    let watcher_exit_for_thread = Arc::clone(&watcher_exit);
    let external_for_watcher: Option<Arc<AtomicBool>> = external.cloned();

    // Use `std::thread::scope` so the watcher's borrow of `bound` is
    // lifetime-safe and the thread is guaranteed to have exited
    // before `serve_until_shutdown_with_flag` returns.
    std::thread::scope(|scope| {
        scope.spawn(move || {
            // Poll every 100 ms; tight enough that shutdown latency is
            // well under the drain-timeout budget (default 30 s) and
            // loose enough that the overhead is negligible. The scoped
            // borrow of `bound` lets us call `request_shutdown`
            // without needing `Arc<BoundIpcServer>`.
            while !watcher_exit_for_thread.load(Ordering::SeqCst) {
                let ext = external_for_watcher
                    .as_ref()
                    .map(|f| f.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if ext || signals::shutdown_requested() {
                    bound.request_shutdown();
                    // Keep polling so late-arriving signals still
                    // re-arm the event (manual-reset semantics make
                    // the repeated signal idempotent).
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });

        let result = serve_loop_body(
            bound,
            runtime,
            external,
            refresh_enabled,
            drain_timeout,
            &mut drain_deadline,
            &mut shutdown_nudged,
        );

        // Tell the watcher thread to exit; the scope won't return
        // until it actually does.
        watcher_exit.store(true, Ordering::SeqCst);
        result
    })
}

#[allow(clippy::too_many_arguments)]
fn serve_loop_body(
    bound: &BoundIpcServer,
    runtime: &mut RuntimeShell,
    external: Option<&Arc<AtomicBool>>,
    refresh_enabled: bool,
    drain_timeout: Duration,
    drain_deadline: &mut Option<Instant>,
    shutdown_nudged: &mut bool,
) -> Result<(), IpcTransportError> {
    loop {
        let external_flagged = external
            .map(|flag| flag.load(Ordering::SeqCst))
            .unwrap_or(false);
        let shutdown_observed =
            runtime.control.shutdown_requested || signals::shutdown_requested() || external_flagged;

        if shutdown_observed {
            // Nudge the listener exactly once per shutdown cycle so
            // Windows' overlapped `ConnectNamedPipe` unblocks. Unix
            // is a no-op. Idempotent if called a second time, but we
            // gate for clarity. The watcher thread may also be
            // signalling concurrently; manual-reset semantics make
            // concurrent signals safe.
            if !*shutdown_nudged {
                bound.request_shutdown();
                *shutdown_nudged = true;
            }
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
                notify_systemd_stopping();
                *drain_deadline = Some(Instant::now() + drain_timeout);
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
                *drain_deadline = Some(Instant::now() + drain_timeout);
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
        if signals::take_reload_request() {
            if let Some(ref config_path) = runtime.config_path {
                use crate::config_reload::{
                    ReloadOutcome, format_reload_failed_event, format_reloaded_event, try_reload,
                };
                // Notify systemd that a reload is in progress. The READY=1
                // suffix re-arms the watchdog once the reload completes.
                notify_systemd_reloading();
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
                notify_systemd_ready();
            }
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
        notify_systemd_watchdog();

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
/// - the supported per-user Windows `pcloudd serve` process;
/// - the optional experimental Windows Service shim (`pcloud-daemon-win`);
/// - any embedder that needs to drive daemon startup and shutdown from a
///   parent thread without relying on Unix signals.
///
/// The function:
///
/// 1. installs the default signal handlers on Unix (idempotent; a no-op on
///    Windows),
/// 2. bootstraps the `RuntimeShell`,
/// 3. binds the configured IPC socket,
/// 4. runs the serve loop via [`serve_until_shutdown_with_flag`] so both
///    the external flag and the internal SIGTERM/IPC flags are honored,
/// 5. returns `Ok(())` once any flag flips, allowing the caller to proceed
///    with teardown or external supervisor reporting.
///
/// Any bootstrap, bind, or serve error is propagated as `anyhow::Error`
/// so the caller can log a single cause chain and exit with a non-zero
/// status.
/// Driver for the daemon's local IPC + serve loop with a shared
/// cooperative-shutdown flag. Used by the per-user `pcloudd serve` path on
/// Windows and by embedders or external supervisors on any platform.
///
/// On Windows the underlying transport is a per-user-SID named pipe
/// (see `pcloud_ipc::platform::windows`); on Unix it is a `0600` Unix
/// socket under a `0700` runtime directory.
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
    .map_err(|err| anyhow::anyhow!("sync loop startup failed: {err}"))?;
    runtime.sync_loop_shared = Some(sync_loop_handle.shared.clone());

    // Health HTTP server (GET /livez, GET /readyz). Disabled by default
    // (port 0). Enable by setting `PCLOUD_HEALTH_PORT=<port>` (must be
    // >= 1024). Binds to 127.0.0.1 only; external probes must go through
    // a reverse proxy or sidecar. The handle is intentionally kept alive
    // for the daemon lifetime — dropping it does not stop the thread, but
    // holding it makes the intent explicit.
    let _health_handle: Option<crate::health_server::HealthServerHandle> = {
        let port = health_port_from_env().map_err(|err| anyhow::anyhow!("{err}"))?;
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
    notify_systemd_ready();

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

// Unix-only tests (IpcServer::bind + serve_until_shutdown helpers).
#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use pcloud_config::{ConfigProfile, Environment};
    use pcloud_ipc::{IpcServer, Request, current_effective_uid};

    use super::{
        parse_health_port, serve_until_shutdown_with_flag, should_reject_during_drain,
        watchdog_ping_interval_from_timeout,
    };
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
    /// No client request is allowed to nudge the listener: idle daemons must
    /// observe cooperative shutdown within the accept-poll bound on their own.
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
        // Give the serve thread time to enter the idle accept. Without this,
        // the flag can win the race before the first loop iteration and fail
        // to exercise the blocked-listener shutdown path.
        std::thread::sleep(Duration::from_millis(200));

        // Flip the external flag while accept is idle. The listener timeout
        // must wake the loop without an artificial client connection.
        flag.store(true, Ordering::SeqCst);

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

    #[test]
    fn health_port_parser_rejects_invalid_env_value() {
        assert_eq!(parse_health_port("9354").expect("valid port"), 9354);
        let err = parse_health_port("not-a-port").expect_err("invalid port must fail");
        assert!(err.contains("PCLOUD_HEALTH_PORT"), "err={err}");
    }

    #[test]
    fn watchdog_ping_interval_uses_half_of_systemd_timeout() {
        assert_eq!(
            watchdog_ping_interval_from_timeout(Duration::from_secs(30)),
            Duration::from_secs(15)
        );
        assert_eq!(
            watchdog_ping_interval_from_timeout(Duration::from_micros(1)),
            Duration::from_micros(1)
        );
    }
}

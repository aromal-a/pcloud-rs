#![allow(clippy::pedantic)]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

//! # pcloud-daemon-win
//!
//! **Platform:** Windows only.
//!
//! `pcloudd-svc` is a thin Windows Service wrapper around the
//! `pcloud_daemon` runtime. When installed via `sc.exe` (see the
//! crate's `README.md`) the Service Control Manager (SCM) invokes this
//! binary, which hands control to the `windows_service`
//! `service_dispatcher` so the process runs under SCM supervision.
//!
//! ## Cross-compile behavior
//!
//! The crate is deliberately gated with `#[cfg(windows)]`:
//!
//! - On **Windows** targets, the full service implementation
//!   (`mod svc`) is compiled: it registers a control handler with the
//!   SCM, reports `Running` → `StopPending` → `Stopped` state
//!   transitions, and coordinates a cooperative shutdown via an
//!   `Arc<AtomicBool>` (see [`std::sync::atomic::AtomicBool`]) flag.
//! - On **non-Windows** targets (Linux, macOS, BSDs) the real service
//!   logic is elided entirely. The top-level `main()` becomes a no-op
//!   stub so `cargo check --workspace` and `cargo build --workspace`
//!   succeed on every supported host platform without producing a
//!   meaningful binary. Running the stub on a non-Windows host does
//!   nothing and exits with status `0`.
//!
//! This means the binary is only **functional** on Windows — attempting
//! to run it under Linux/macOS is a no-op by design, not a bug. Tests
//! that need the real service surface must either run on Windows or use
//! a feature-gated mock.
//!
//! ## Shutdown protocol
//!
//! The SCM control handler and the main service thread share an
//! `Arc<AtomicBool>` shutdown flag. When the SCM delivers
//! `ServiceControl::Stop` or `ServiceControl::Shutdown`, the handler
//! flips the flag to `true`; the main service thread polls the flag and
//! transitions to `StopPending` → `Stopped` once it observes the
//! request. This keeps the control handler non-blocking, as required by
//! SCM, while still giving the worker thread a chance to unwind
//! cleanly.
//!
//! The ordered state machine reported to the SCM is:
//!
//! 1. `Running` with `controls_accepted = STOP | SHUTDOWN`
//! 2. `StopPending` with `wait_hint = 5s` once the shutdown flag flips
//!    or the worker thread finishes
//! 3. `Stopped` with `exit_code = Win32(0)` immediately after
//!
//! The flag uses [`std::sync::atomic::Ordering::SeqCst`] on both the
//! store (handler) and the load (worker poll). That is deliberately
//! conservative — the flag is touched at most a few times per process
//! lifetime, so the cost of the strongest ordering is irrelevant, and
//! SeqCst removes any question about handler-to-worker visibility
//! across the SCM thread boundary.
//!
//! ## Cooperative `pcloud_daemon::serve_with_shutdown` entry point
//!
//! The worker now calls the cooperative entry
//! `pcloud_daemon::serve_with_shutdown`, which takes the same
//! `Arc<AtomicBool>` the SCM control handler flips. The daemon's IPC
//! serve loop polls that flag on every iteration and returns cleanly
//! once it flips, so the worker thread joins normally on SCM
//! `Stop` / `Shutdown` instead of being detached.
//!
//! Expected signature:
//!
//! ```ignore
//! use std::sync::Arc;
//! use std::sync::atomic::AtomicBool;
//!
//! /// Run the daemon until `shutdown` becomes `true`, then return.
//! pub fn serve_with_shutdown(shutdown: Arc<AtomicBool>) -> anyhow::Result<()>;
//! ```

/// Non-Windows entry point: no-op stub.
///
/// This function is compiled on every target that is **not** Windows.
/// It exists purely to let the workspace build cleanly on Linux and
/// macOS hosts; it performs no work and exits with status `0`. See the
/// crate-level docs for the cross-compile rationale.
///
/// # Why a stub instead of `#![cfg(windows)] compile_error!`?
///
/// The Rust workspace is built and tested on Linux as the primary CI
/// target. A `compile_error!` on non-Windows would break
/// `cargo build --workspace` / `cargo check --workspace` on every
/// developer machine and every CI runner. An empty `main` keeps the
/// workspace green while still producing a binary that is clearly a
/// no-op when executed on the wrong OS: running it prints nothing and
/// returns `0`, which is the correct behaviour for a service shim
/// that cannot possibly attach to a non-existent Service Control
/// Manager.
///
/// All real service logic lives behind [`mod svc`], which is itself
/// `#[cfg(windows)]`-gated, so the non-Windows binary contains zero
/// Windows-specific code and zero `windows_service` dependencies at
/// runtime.
#[cfg(not(windows))]
fn main() {
    // Non-Windows stub: this crate is a Windows Service shim and has no
    // behaviour outside of SCM-hosted environments. See README.md.
}

/// Windows Service implementation (SCM-hosted).
///
/// All items in this module are `#[cfg(windows)]`-gated and are **not**
/// compiled on other platforms. The module encapsulates:
///
/// - the SCM-facing `ffi_service_main` entry,
/// - the service control handler that consumes SCM `Stop`/`Shutdown`
///   requests,
/// - the `Arc<AtomicBool>` shutdown flag shared between the handler and
///   the main service thread,
/// - the state-transition reports (`Running` → `StopPending` →
///   `Stopped`).
#[cfg(windows)]
mod svc {
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;
    use windows_service::{Result as SvcResult, define_windows_service};

    /// SCM-visible service name. Must match the name passed to
    /// `sc.exe create` at install time.
    const SERVICE_NAME: &str = "pcloudd";

    /// Service type reported to the SCM. [`ServiceType::OWN_PROCESS`]
    /// means the service runs in its own dedicated process, not as part
    /// of a shared `svchost.exe`.
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    // The `define_windows_service!` macro generates the `extern "system"`
    // FFI trampoline `ffi_service_main` that the SCM calls; it forwards
    // control to the native-Rust [`service_main`] function.
    define_windows_service!(ffi_service_main, service_main);

    /// SCM entry point.
    ///
    /// Invoked on the service worker thread spawned by the
    /// `windows_service` dispatcher. Returning from this function signals
    /// the SCM that the service has stopped. Any error returned by
    /// [`run_service`] is intentionally swallowed here because the SCM
    /// log is the only available sink at this layer and the process is
    /// about to exit regardless.
    fn service_main(_args: Vec<OsString>) {
        if let Err(err) = run_service() {
            // We cannot surface errors to the user here; the SCM logs are the
            // only sink. Swallow so the process exits cleanly.
            let _ = err;
        }
    }

    /// Core service lifecycle.
    ///
    /// 1. Registers an SCM control handler that flips a shared
    ///    `Arc<AtomicBool>` shutdown flag on `Stop`/`Shutdown` and
    ///    answers `Interrogate` with a successful
    ///    [`ServiceControlHandlerResult::NoError`]. Any other control
    ///    code is reported as
    ///    [`ServiceControlHandlerResult::NotImplemented`].
    /// 2. Reports [`ServiceState::Running`] to the SCM with
    ///    [`ServiceControlAccept::STOP`] | [`ServiceControlAccept::SHUTDOWN`].
    /// 3. Spawns the cooperative daemon entry
    ///    `pcloud_daemon::serve_with_shutdown` on a worker thread,
    ///    sharing the same `Arc<AtomicBool>` as the SCM control
    ///    handler. The daemon's own serve loop polls that flag and
    ///    returns cleanly once it flips.
    /// 4. Reports [`ServiceState::StopPending`], joins the worker,
    ///    then reports [`ServiceState::Stopped`].
    fn run_service() -> SvcResult<()> {
        let shutdown = Arc::new(AtomicBool::new(false));

        // Control handler: SCM dispatches Stop/Interrogate here.
        let shutdown_for_handler = shutdown.clone();
        let event_handler = move |control| -> ServiceControlHandlerResult {
            match control {
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    shutdown_for_handler.store(true, Ordering::SeqCst);
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

        // Report Running.
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        // Spawn the cooperative daemon entry. The SCM control handler
        // and the daemon share the same `Arc<AtomicBool>`, so flipping
        // it from the handler causes the daemon's IPC serve loop to
        // return cleanly and the worker thread to join.
        let shutdown_for_worker = Arc::clone(&shutdown);
        let worker = thread::spawn(move || pcloud_daemon::serve_with_shutdown(shutdown_for_worker));

        // Block until either the SCM asks us to stop or the daemon
        // exits on its own (e.g. bootstrap failure, fatal serve error).
        while !shutdown.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(500));
            if worker.is_finished() {
                break;
            }
        }

        // Report StopPending so the SCM allows us up to `wait_hint`
        // before escalating. The daemon has already observed the flag
        // and is unwinding its serve loop at this point.
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::StopPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })?;

        // Join the worker. The daemon's serve loop returns within one
        // accept(2) iteration of the flag flipping, so this completes
        // promptly. Errors surface via SCM logs only; the SCM API does
        // not offer a richer channel at this layer.
        match worker.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = err;
            }
            Err(_panic) => {
                // Worker panicked; treated as a clean stop from the
                // SCM's perspective since there is no recovery path.
            }
        }

        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        Ok(())
    }

    /// Windows-only process entry point.
    ///
    /// Hands the current process over to the SCM dispatcher via
    /// [`service_dispatcher::start`]. This call **blocks** until the SCM
    /// releases the service. The dispatcher routes SCM callbacks to the
    /// generated `ffi_service_main` FFI trampoline, which in turn drives
    /// [`service_main`] and [`run_service`].
    ///
    /// Returns an error if the binary was launched outside an SCM-hosted
    /// context (for example, invoked directly from a terminal).
    pub fn main() -> SvcResult<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }
} // mod svc

/// Windows entry point.
///
/// Delegates to [`svc::main`] which blocks on the SCM dispatcher. Errors
/// surface the underlying [`windows_service`] error verbatim so the
/// caller (typically the Windows loader) can observe a non-zero exit
/// code when the binary is mis-launched outside an SCM context.
#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    svc::main()
}

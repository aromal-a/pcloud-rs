#![warn(unsafe_op_in_unsafe_fn)]
//! `pcloudd` — pcloud-rs daemon binary entry point.
//!
//! Boots the daemon runtime, binds the IPC server, and dispatches
//! shutdown signals. Thin wrapper over `pcloud_daemon::bootstrap`.
#![deny(missing_docs)]

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::{IpcServer, current_effective_uid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Summary,
    Serve,
    Help,
    Version,
}

fn help_text() -> &'static str {
    concat!(
        "pcloudd - pCloud background service\n",
        "\n",
        "USAGE:\n",
        "  pcloudd\n",
        "  pcloudd serve\n",
        "  pcloudd --help\n",
        "  pcloudd --version\n",
        "\n",
        "MODES:\n",
        "  (no subcommand)                Print a one-shot runtime summary and exit\n",
        "  serve                          Bind the configured IPC socket and serve requests\n",
        "\n",
        "OPTIONS:\n",
        "  -h, --help                     Show this help text\n",
        "  -V, --version                  Show daemon version\n",
        "\n",
        "RUNTIME BEHAVIOR:\n",
        "  The daemon bootstraps its secure-default runtime, then either prints a\n",
        "  summary or enters the local IPC serve loop. The socket path comes from\n",
        "  the active config/profile bootstrap; in development defaults it resolves\n",
        "  under the selected pcloud root.\n",
        "\n",
        "ENVIRONMENT:\n",
        "  PCLOUD_ROOT                    Override the daemon state root\n",
        "  PCLOUD_ENV                     Select bootstrap environment semantics\n",
        "  PCLOUD_API_MODE                Select development/plaintext/tls API mode\n",
        "\n",
        "NOTES:\n",
        "  `serve` is the long-running mode used by `pcloudc start`.\n",
        "  Unknown arguments are rejected.\n",
    )
}

fn parse_mode(args: &[String]) -> Result<Mode, String> {
    match args.get(1).map(String::as_str) {
        None => Ok(Mode::Summary),
        Some("serve") => Ok(Mode::Serve),
        Some("--help" | "-h" | "help") => Ok(Mode::Help),
        Some("--version" | "-V" | "version") => Ok(Mode::Version),
        Some(other) => Err(format!("unknown argument '{other}'")),
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match parse_mode(args)? {
        Mode::Help => {
            println!("{}", help_text());
            Ok(())
        }
        Mode::Version => {
            println!("pcloudd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Mode::Summary => {
            let runtime = pcloud_daemon::bootstrap_shell()
                .map_err(|err| format!("daemon bootstrap failed: {err}"))?;
            println!("{}", runtime.summary());
            Ok(())
        }
        Mode::Serve => {
            // Install signal handlers BEFORE bootstrapping subsystems so
            // even a slow bootstrap can be interrupted by SIGTERM/SIGINT
            // without the daemon being SIGKILLed by systemd after
            // TimeoutStopSec. SIGHUP is currently a no-op; SIGPIPE is
            // ignored (standard for network daemons).
            pcloud_daemon::signals::install_default_handlers()
                .map_err(|err| format!("failed to install signal handlers: {err}"))?;
            let mut runtime = pcloud_daemon::bootstrap_shell()
                .map_err(|err| format!("daemon bootstrap failed: {err}"))?;
            let socket_path = runtime.config.paths.ipc_socket_path();
            let server = IpcServer::new(current_effective_uid());
            let bound = server
                .bind(&socket_path)
                .map_err(|err| format!("daemon socket bind failed: {err}"))?;
            // Best-effort pidfile used by operator tooling (`pcloudc
            // drain`, supervision scripts). Failure to write the
            // pidfile is non-fatal — operators can fall back to
            // `pgrep pcloudd` — but we surface a warning so a misconfigured
            // state_dir is still visible.
            let pid_path = pcloud_daemon::daemon_pid_path(&runtime.config.paths.state_dir);
            if let Err(err) = write_pid_file(&pid_path) {
                log::warn!("failed to write pid file {}: {err}", pid_path.display());
            }
            println!("daemon listening on {}", bound.socket_path().display());

            // Spawn the background sync loop. This gives the daemon
            // autonomous sync capability — the loop polls remote diff,
            // scans local changes, and advances transfers on a
            // configurable interval. The shared auth token handle is
            // stored so the IPC dispatch path can update it on
            // login/logout.
            let store_path = runtime.config.paths.state_dir.join("store.sqlite3");
            let (sync_loop_handle, _sync_auth_token) =
                pcloud_daemon::sync_loop_runtime::spawn_daemon_sync_loop(
                    &runtime.config,
                    &runtime.auth,
                    store_path,
                );
            runtime.sync_loop_shared = Some(sync_loop_handle.shared.clone());
            if runtime.config.sync_loop.enabled {
                println!(
                    "sync loop started (poll_interval={}s)",
                    runtime.config.sync_loop.poll_interval_secs
                );
            } else {
                println!("sync loop disabled by config");
            }

            // Feature-gated Prometheus scrape listener. Default OFF. When
            // enabled, binds loopback by default; wildcard bind requires
            // both PCLOUD_METRICS_BIND_ALL=1 and Environment::Development.
            #[cfg(feature = "metrics")]
            {
                use std::sync::Arc;
                use std::sync::atomic::{AtomicBool, Ordering};
                // Wire the runtime's canonical SLO registry into the
                // bridge so `/slo` and `Method::GetSlo` render from the
                // same instance.
                let bridge = pcloud_daemon::metrics_server::MetricsBridge::new()
                    .with_slo(Arc::clone(&runtime.observability.slo));
                bridge.refresh(&runtime);
                let shutdown_flag = Arc::new(AtomicBool::new(false));
                let exporter = match pcloud_daemon::metrics_server::spawn_from_env(
                    runtime.config.environment,
                    Arc::clone(&shutdown_flag),
                    bridge.clone(),
                ) {
                    Ok(handle) => {
                        println!(
                            "metrics exporter listening on http://{}/metrics",
                            handle.local_addr()
                        );
                        Some(handle)
                    }
                    Err(err) => {
                        log::error!("metrics exporter failed to bind: {err}");
                        None
                    }
                };
                pcloud_daemon::metrics_server::serve_with_metrics(&bound, &mut runtime, &bridge)
                    .map_err(|err| format!("daemon request handling failed: {err}"))?;
                shutdown_flag.store(true, Ordering::SeqCst);
                if let Some(h) = exporter {
                    h.shutdown();
                }
            }
            #[cfg(not(feature = "metrics"))]
            pcloud_daemon::serve_until_shutdown(&bound, &mut runtime)
                .map_err(|err| format!("daemon request handling failed: {err}"))?;
            // The serve loop has returned; request_shutdown handling inside
            // the dispatch path performed any in-request drain. Explicit
            // mount/upload drain remains the responsibility of runtime
            // Drop impls and the backends that register teardown hooks.
            drop(bound);
            // Shut down the background sync loop cleanly before
            // proceeding with the rest of daemon teardown. This ensures
            // any in-flight sync cycle completes and the loop thread is
            // joined.
            if let Err(()) = sync_loop_handle.shutdown_and_join() {
                log::warn!("sync loop thread panicked during shutdown");
            }
            // Mark drain machine Stopped after the socket has been
            // unbound. Operators polling DrainStatus before pidfile
            // removal now observe `state = "stopped"`.
            pcloud_daemon::signals::mark_stopped();
            // Best-effort cleanup: remove the pidfile so stale
            // `pcloudc drain` invocations cannot target a dead pid.
            let _ = std::fs::remove_file(pcloud_daemon::daemon_pid_path(
                &runtime.config.paths.state_dir,
            ));
            println!("daemon shutdown complete");
            Ok(())
        }
    }
}

/// Write the current process id into `path` atomically with `0600`
/// ownership. The write goes through a temporary sibling followed by
/// `rename(2)` so an operator never observes a half-written pid.
fn write_pid_file(path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pid.tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        writeln!(f, "{}", std::process::id())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(err) = run(&args) {
        eprintln!("{err}");
        eprintln!("\n{}", help_text());
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, help_text, parse_mode};

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn help_mentions_serve_and_summary() {
        let text = help_text();
        assert!(text.contains("pcloudd"));
        assert!(text.contains("serve"));
        assert!(text.contains("PCLOUD_ROOT"));
    }

    #[test]
    fn parse_mode_defaults_to_summary() {
        assert_eq!(parse_mode(&argv(&["pcloudd"])).unwrap(), Mode::Summary);
    }

    #[test]
    fn parse_mode_accepts_help_and_version() {
        assert_eq!(
            parse_mode(&argv(&["pcloudd", "--help"])).unwrap(),
            Mode::Help
        );
        assert_eq!(
            parse_mode(&argv(&["pcloudd", "-V"])).unwrap(),
            Mode::Version
        );
    }

    #[test]
    fn parse_mode_rejects_unknown_argument() {
        let err = parse_mode(&argv(&["pcloudd", "--bogus"])).unwrap_err();
        assert!(err.contains("unknown argument"));
    }
}

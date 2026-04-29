#![allow(clippy::pedantic)]
#![warn(unsafe_op_in_unsafe_fn)]
// CLI binary requires targeted unsafe for pre_exec/setsid daemon
// detach and signal handler registration.
//! `pcloudc` — the pcloud-rs command-line client.
//!
//! Thin front-end that parses subcommands, connects to the local daemon
//! over IPC, and renders responses. All business logic lives in
//! `pcloud-daemon` and the protocol/SDK crates.
// dead_code: several private helpers are only reachable on unix (cfg-gated
// by #[cfg(unix)] / #[cfg(not(unix))]) and some LoginOptions fields are used
// only through cfg-gated paths. Suppressed at file level because cfg conditionals
// make per-item annotation unwieldy in a binary crate.
#![allow(dead_code)]
#![deny(missing_docs)]

// **PLATFORM:** cross-platform
// **GATING:** platform-specific sections (`migrate-from-c`, IPC stack)
// are behind `#[cfg(unix)]` / `#[cfg(windows)]`. The `doctor` dispatch
// in this file is portable.

#[allow(unsafe_code)]
mod app;
#[allow(unsafe_code)]
mod commands;
mod completion;
mod config;
mod crypto_setup_picker;
#[allow(unsafe_code)]
mod doctor;
mod exit_code;
mod field_selector;
#[allow(unsafe_code)]
mod globals;
mod json_output;
#[cfg(unix)]
mod migrate;
mod output;
mod progress;
#[allow(unsafe_code)]
mod prompt;
mod verify;

use pcloud_config::{ConfigProfile, Environment};
use pcloud_ipc::IpcClient;

use crate::exit_code::{EXIT_CODE_HELP, ExitCode};
use crate::globals::{GlobalFlags, OutputFormat};
use crate::json_output::JsonEnvelope;

fn main() {
    // Warn in release builds when build provenance metadata is missing.
    // Debug builds (cargo run / cargo test) routinely omit GIT_HASH, so
    // the check is skipped there to avoid noise in development workflows.
    #[cfg(not(debug_assertions))]
    {
        if option_env!("GIT_HASH").is_none() {
            eprintln!(
                "warning: pcloudc built without GIT_HASH; \
                 build provenance is unknown. \
                 Ensure the build.rs git-hash injection ran correctly."
            );
        }
        if option_env!("BUILD_PROFILE").is_none() {
            eprintln!(
                "warning: pcloudc built without BUILD_PROFILE; \
                 build profile is unknown."
            );
        }
    }

    let argv: Vec<String> = std::env::args().collect();
    let code = run(&argv);
    std::process::exit(code.as_i32());
}

/// `pcloudc <version> (<git-hash>, <profile>)` banner printed by
/// `--version`. Git hash and profile are injected at build time by
/// `build.rs`; both fall back to `"unknown"` gracefully when the source
/// tree isn't a git checkout or cargo didn't expose the profile.
fn version_banner() -> String {
    let git = option_env!("GIT_HASH").unwrap_or("unknown");
    let profile = option_env!("BUILD_PROFILE").unwrap_or("unknown");
    format!(
        "{} {} ({}, {})",
        completion::BIN_NAME,
        env!("CARGO_PKG_VERSION"),
        git,
        profile,
    )
}

/// Short, friendly message printed when the user runs `pcloudc` with no
/// arguments. Replaces the legacy behavior of silently defaulting to
/// `status`, which on a cold daemon produced a long engine-counter blob.
fn zero_arg_hint() -> &'static str {
    "pcloud is idle. Try:\n  \
     pcloudc status              Show current state\n  \
     pcloudc --help              List all commands"
}

/// Classify a `PromptError` as "non-TTY stdin had no data to read".
///
/// `rpassword` opens `/dev/tty` when stdin is not a TTY; on non-interactive
/// contexts (CI, systemd units, piped stdin with no bytes) this surfaces as
/// either [`std::io::ErrorKind::NotFound`] or raw `ENXIO` (os error 6). We use
/// this to hand the operator an actionable remediation for the 2FA prompt
/// instead of a bare "login cancelled".
fn is_non_tty_stdin_unavailable(err: &crate::prompt::PromptError) -> bool {
    use crate::prompt::PromptError;
    use std::io::ErrorKind;
    match err {
        PromptError::Io(io_err) => {
            if io_err.kind() == ErrorKind::NotFound || io_err.kind() == ErrorKind::UnexpectedEof {
                return true;
            }
            // Some platforms surface ENXIO (no such device/address) when
            // the controlling terminal is absent — the raw OS code is
            // more reliable than the mapped `ErrorKind` here.
            matches!(io_err.raw_os_error(), Some(6))
        }
        // Interactive Ctrl-D on a TTY — treat as plain cancellation.
        PromptError::Eof => false,
    }
}

/// Drive the CLI end-to-end. Factored out of `main` so every path is
/// exercised by unit tests (and so exit codes are deterministic).
fn run(argv: &[String]) -> ExitCode {
    // 1. Extract global flags (-q/-v/--json/--output) before any legacy parsing.
    let (mut flags, reduced) = match GlobalFlags::extract(argv) {
        Ok(pair) => pair,
        Err(err) => {
            return report_error(
                None,
                OutputFormat::Text,
                false,
                ExitCode::Usage,
                &err.to_string(),
            );
        }
    };

    // 2. Handle `--help` and `--version` at the global level.
    if flags.help && reduced.len() <= 1 {
        if !flags.quiet {
            println!("{}\n\n{}", app::help_text(), EXIT_CODE_HELP);
        }
        return ExitCode::Ok;
    }
    if flags.version {
        if !flags.quiet {
            println!("{}", version_banner());
        }
        return ExitCode::Ok;
    }

    // 3. `completion <shell>` is CLI-local, no daemon contact.
    if let Some(shell_arg) = completion_request(&reduced) {
        return handle_completion(&flags, shell_arg);
    }

    // 3b. Zero-arg invocation: print a short, friendly hint instead of
    // silently defaulting to `status` (which on a cold daemon produces a
    // long engine-counter blob nobody asked for). Preserves `--quiet`
    // and `--json` semantics; JSON mode emits a success envelope so
    // scripted pipelines still parse.
    if reduced.len() <= 1 {
        if flags.output == OutputFormat::Json {
            if !flags.quiet {
                let env = JsonEnvelope::Success {
                    command: "hint".into(),
                    status: json_output::JsonStatus::Ok,
                    message: zero_arg_hint().to_owned(),
                    exit_code: 0,
                };
                print!("{}", env.render());
            }
        } else if !flags.quiet {
            println!("{}", zero_arg_hint());
        }
        return ExitCode::Ok;
    }

    // 4. Legacy parser continues to own everything else.
    let command = match app::parse_command(&reduced) {
        Ok(c) => c,
        Err(err) => {
            return report_error(
                None,
                flags.output,
                flags.quiet,
                ExitCode::Usage,
                &format!("cli command parse failed: {err}"),
            );
        }
    };

    // 4b. For field-safe commands (those whose positional args are
    // read-only identifiers or where there are none at all), absorb any
    // trailing bare tokens as implicit `--field` selectors. This is
    // what powers `pcloudc userinfo quota usedquota` without a flag.
    let (reduced, implicit_fields) = extract_bare_field_positionals(&command, reduced);
    for f in implicit_fields {
        flags.fields.push(f);
    }

    // 5. `help` prints help; in JSON mode we emit a success envelope too.
    if matches!(command, commands::Command::Help) {
        if flags.output == OutputFormat::Json {
            if !flags.quiet {
                let env = JsonEnvelope::Success {
                    command: "help".into(),
                    status: json_output::JsonStatus::Ok,
                    message: app::help_text().trim_end().to_owned(),
                    exit_code: 0,
                };
                print!("{}", env.render());
            }
        } else if !flags.quiet {
            println!("{}\n\n{}", app::help_text(), EXIT_CODE_HELP);
        }
        return ExitCode::Ok;
    }

    // 6. `start` is a CLI-side command that spawns the daemon binary in
    // the background. Idempotent: if the socket is already reachable we
    // return Ok without a second spawn.
    if matches!(command, commands::Command::Start) {
        return run_daemon_start(&flags);
    }

    // 6b. `drain` is a CLI-side command that looks up the daemon pidfile,
    // dispatches SIGTERM, and polls `DrainStatus` until the daemon
    // reports `stopped` or the configured handoff timeout expires. See
    // `docs/book/src/operations/upgrade.md` §Graceful drain.
    if matches!(command, commands::Command::Drain) {
        return run_daemon_drain(&flags);
    }

    // 6c. `reload` is a CLI-side command that looks up the daemon pidfile
    // and sends SIGHUP to trigger a config hot-reload.
    if matches!(command, commands::Command::Reload) {
        return run_daemon_reload(&flags);
    }

    // `doctor` is CLI-side and never contacts the daemon beyond one
    // best-effort `GetStatus` probe performed inside `doctor::run`.
    if matches!(command, commands::Command::Doctor) {
        return run_doctor(&flags, &reduced);
    }

    // `migrate-from-c` is CLI-side only: it reads the legacy C client's
    // `~/.pcloud/.pclouddb` file and seeds the Rust daemon's XDG state
    // dir. Never contacts the daemon. Unix-only — the C client never
    // ran on Windows (the module and command are gated at compile time).
    #[cfg(unix)]
    if matches!(command, commands::Command::MigrateFromC { .. }) {
        return run_migrate_from_c(&flags, &reduced);
    }

    // `verify` is CLI-side: it walks a local path and cross-checks
    // SHA256 against the server-reported digest. R9 enhancement #12.
    // The matching `Request::VerifyPath` IPC variant is wired for a
    // future daemon-walks-tree implementation, but today the walk
    // happens CLI-side to keep the first landing minimal.
    if matches!(command, commands::Command::Verify { .. }) {
        return verify::run(&flags, &reduced);
    }

    // `diff` / `restore` are CLI-side stubs for the R9 #9 revision
    // follow-up. Parsing still runs so usage errors surface with
    // ExitCode::Usage. The command itself emits the same structured
    // "not_configured" JSON envelope as `pcloudc log` so tooling keys
    // on a single `status` field across all three revision operations.
    // Exit code stays `6 Unavailable`.
    if matches!(
        command,
        commands::Command::FileDiff | commands::Command::FileRestore
    ) {
        // Touch the inputs to surface positional-arg errors first.
        if let Err(err) = app::parse_inputs_for_command(&command, &reduced) {
            return report_error(
                Some(label_for(&command)),
                flags.output,
                flags.quiet,
                ExitCode::Usage,
                &format!("cli input resolution failed: {err}"),
            );
        }
        // Keep this string in sync with
        // `pcloud_proto::revision_provider::NULL_PROVIDER_MESSAGE`; the
        // `null_provider_payload_matches_cli_stub` test in the proto
        // crate asserts both surfaces emit the same message verbatim.
        let detail = concat!(
            "{\"status\":\"not_configured\",",
            "\"message\":\"pCloud listrevisions API not yet public; ",
            "configure [file_history].revision_url to point at a custom endpoint\",",
            "\"next\":\"configure [file_history].revision_url ",
            "or wait for pCloud public API\"}",
        )
        .to_owned();
        return report_error(
            Some(label_for(&command)),
            flags.output,
            flags.quiet,
            ExitCode::Unavailable,
            &detail,
        );
    }

    // `login` is a CLI-side REPL that chains username → password →
    // (on TwoFactorChallengeIssued) TFA code, all via interactive
    // prompts. This matches the legacy C `pcloud-rs` readline experience
    // while keeping secrets off argv.
    //
    // mysql-style flag shorthands to skip individual prompts:
    //   -u | --user | --username <name>     preset username
    //   -c | --crypto                       unlock crypto folder after login
    //   -y | --passascrypto                 reuse account pw as crypto pw
    //   -r | --trust-device                 ask pCloud to trust this device
    //   -s | --save-password                enable auth-token vault
    //   -m | --mountpoint [<path>]          mount after login
    //   -O | --fuse-opts <opts>             FUSE mount options
    //   -T | --tfa-channel       <sms|push> pre-select 2FA channel
    //         --password-stdin              read password from stdin
    //         --password-env <VAR>          read password from env var
    //         --log-path / --fs-event-log / --log-level / --config
    if matches!(command, commands::Command::LoginBegin) {
        let opts = LoginOptions::from_argv(&reduced);
        let config_path =
            config::CliConfig::default_path(opts.config_path.as_deref().map(std::path::Path::new));
        let mut cfg = config::CliConfig::load_or_init(&config_path).unwrap_or_default();
        // Persist any config-only flags that the user just supplied so
        // `pcloudc start` next time picks them up. CLI flag wins for
        // the in-memory `cfg` we hand to `run_interactive_login`.
        let mut dirty = false;
        if let Some(v) = opts.log_path.as_deref() {
            cfg.log_path = Some(std::path::PathBuf::from(v));
            dirty = true;
        }
        if let Some(v) = opts.fs_event_log.as_deref() {
            cfg.fs_event_log = Some(std::path::PathBuf::from(v));
            dirty = true;
        }
        if let Some(v) = opts.log_level.as_deref() {
            cfg.log_level = Some(v.to_owned());
            dirty = true;
        }
        if let Some(v) = opts.fuse_opts.as_deref() {
            cfg.fuse_opts = Some(v.to_owned());
            dirty = true;
        }
        if let Some(v) = opts.cache_size_gb {
            cfg.cache_size_gb = Some(v);
            dirty = true;
        }
        if dirty {
            let _ = cfg.write_with_comments(&config_path);
            // If the daemon is currently running, queue it for restart
            // so the new env-bound settings (cache size, mount-default,
            // log paths/level, fuse opts) actually take effect. We
            // drain pending writes by unmounting first; the auth-token
            // vault (if `authsave` was on) and the new mountpoint will
            // be restored automatically by the post-login flow below.
            queue_daemon_restart_for_config_change(&flags);
        }
        return run_interactive_login(&flags, opts, cfg);
    }

    // 7. Resolve inputs for the chosen command.
    let inputs = match app::parse_inputs_for_command(&command, &reduced) {
        Ok(inputs) => inputs,
        Err(err) => {
            return report_error(
                Some(label_for(&command)),
                flags.output,
                flags.quiet,
                ExitCode::Usage,
                &format!("cli input resolution failed: {err}"),
            );
        }
    };

    // 8. Dispatch to the daemon.
    //
    // Legacy snapshot alias deprecation: the `backup snapshot-*` tokens
    // are accepted for one release cycle but emit a one-line stderr
    // warning redirecting operators to the new `snapshot *` surface.
    // Suppressed under `--quiet` so scripts that opt into silent mode
    // do not get surprise stderr output.
    if !flags.quiet
        && matches!(
            command,
            crate::commands::Command::BackupSnapshotCreate
                | crate::commands::Command::BackupSnapshotRestore
                | crate::commands::Command::BackupSnapshotVerify
                | crate::commands::Command::BackupSnapshotPrune
        )
    {
        let (old, new_): (&str, &str) = match command {
            crate::commands::Command::BackupSnapshotCreate => {
                ("backup snapshot-create", "snapshot create")
            }
            crate::commands::Command::BackupSnapshotRestore => {
                ("backup snapshot-restore", "snapshot restore")
            }
            crate::commands::Command::BackupSnapshotVerify => {
                ("backup snapshot-verify", "snapshot verify")
            }
            crate::commands::Command::BackupSnapshotPrune => {
                ("backup snapshot-prune", "snapshot prune")
            }
            // INVARIANT: the outer `matches!` guard above restricts the
            // block to exactly the four BackupSnapshot* variants; no other
            // Command variant can reach this arm.
            _ => unreachable!(),
        };
        eprintln!(
            "warning: `{old}` is deprecated and will be removed in the next release; use `{new_}` instead"
        );
    }

    let request = command.clone().into_request(&inputs);
    let client = IpcClient;
    // **PLATFORM:** all. Matches daemon default: XDG-canonical via
    // `PcloudDirs::discover()` on Linux/BSD/macOS/Windows; overridable
    // via `PCLOUD_ROOT` for multi-instance and tests.
    let socket_path = match socket_path_for_defaults() {
        Ok(p) => p,
        Err(err) => {
            return report_error(
                Some(label_for(&command)),
                flags.output,
                flags.quiet,
                ExitCode::Internal,
                &format!("resolve socket path: {err}"),
            );
        }
    };

    // When a W3C `traceparent` is active for this invocation, echo
    // it to stderr exactly once (before the command result) so the
    // operator can paste it into a support ticket. `--quiet` keeps
    // stderr silent too.
    if let Some(tp) = flags.traceparent.as_deref() {
        if !flags.quiet {
            eprintln!("[trace: {tp}]");
        }
    }

    // Attach the resolved traceparent (if any) to the outgoing
    // envelope. When no trace context is present we stay on the bare
    // `send` path, which wraps the request in an envelope with
    // `traceparent = None` — byte-identical to the pre-trace-id
    // contract.
    let do_send = || match flags.traceparent.as_deref() {
        Some(tp) => {
            let envelope =
                pcloud_ipc::RequestEnvelope::new(request.clone()).with_traceparent(tp.to_owned());
            client.send_envelope(&socket_path, &envelope)
        }
        None => client.send(&socket_path, &request),
    };
    let mut send_result = do_send();
    // If the daemon socket is missing (no daemon running), offer to
    // start `pcloudd` on the user's behalf and retry once. Skipped in
    // non-interactive contexts (`--quiet` or stdin not a TTY) so
    // scripts get the original NotFound error.
    if let Err(pcloud_ipc::IpcTransportError::Io(ref io_err)) = send_result {
        if io_err.kind() == std::io::ErrorKind::NotFound {
            if let Ok(()) = try_autostart_daemon(&socket_path, &flags) {
                send_result = do_send();
            }
        }
    }

    match send_result {
        Ok(response) => {
            let code = ExitCode::from_response_status(&response.status);
            // `log` command: when the daemon delivers an Ok payload, the
            // `message` carries a JSON array of revisions. Render it as
            // git-log-style text, or pass through as JSON. Unavailable/
            // error statuses fall through to the standard envelope so
            // the honest-scope "not yet supported" message surfaces.
            if matches!(command, commands::Command::FileHistory)
                && response.status == pcloud_ipc::ResponseStatus::Ok
                && !flags.quiet
            {
                return render_file_history(&command, &response, &flags, code);
            }
            // Field-selector projection: when the user supplied one or
            // more `--field`/`-f`/`--select` paths (or bare trailing
            // positionals on a whitelisted command), route through the
            // projection renderer. Failed selectors map to exit 2.
            if !flags.fields.is_empty() {
                return render_with_field_selection(&command, &response, &flags, code);
            }
            if !flags.quiet {
                match flags.output {
                    OutputFormat::Json => {
                        let env = JsonEnvelope::from_response(label_for(&command), &response);
                        print!("{}", env.render());
                    }
                    OutputFormat::Text => {
                        // Verbosity ladder (matches `--quiet`/`-v`/`-vv`/`-vvv`):
                        //   quiet (already filtered above): nothing
                        //   v=0 (default): just the response message
                        //   v>=1: prefix with command + status
                        //   v>=2: also include the daemon banner
                        //
                        // Stage 4b.4: for crypto commands we translate
                        // well-known server result codes to human-friendly
                        // messages before rendering; the original daemon
                        // string is preserved at v>=1 via the `[command
                        // status] message` prefix.
                        let rendered_msg =
                            if matches!(
                                command,
                                commands::Command::CryptoSetupV2
                                    | commands::Command::SubmitCryptoPassword
                                    | commands::Command::CryptoGetFolderKey
                                    | commands::Command::CryptoGetFileKey
                                    | commands::Command::CryptoStatus
                            ) && !matches!(response.status, pcloud_ipc::ResponseStatus::Ok)
                            {
                                translate_server_result_code(&response.message)
                            } else {
                                response.message.clone()
                            };
                        let line = match flags.verbosity {
                            0 => rendered_msg.clone(),
                            1 => format!(
                                "[{:?} {:?}] {}",
                                command, response.status, response.message
                            ),
                            _ => format!(
                                "{} | command={:?} | status={:?} | message={}",
                                app::banner(),
                                command,
                                response.status,
                                response.message
                            ),
                        };
                        let rendered = output::RenderedOutput::from_message(line);
                        if !rendered.title.is_empty() {
                            // Stage 4b.4 UX: prepend a `Backend:` line for
                            // `crypto status` and append a `(backend: ...)`
                            // suffix to `crypto start` / `unlock-crypto`.
                            // The daemon response does not currently carry
                            // a structured backend field — see
                            // `crypto_status` in
                            // crates/pcloud-daemon/src/runtime.rs. Until
                            // the IPC response is widened (tracked under
                            // bd-1du.10 Stage 6), this renderer extracts
                            // what it can from the response message and
                            // otherwise emits an honest 'unknown' marker
                            // rather than silently skipping the line.
                            if let Some(prefix) = render_backend_prefix(&command, &response) {
                                println!("{prefix}");
                            }
                            println!("{}", rendered.title);
                            if let Some(suffix) = render_backend_suffix(&command, &response) {
                                println!("{suffix}");
                            }
                        }
                    }
                }
            }
            code
        }
        Err(err) => {
            let detail = format!("cli request failed: {err}");
            let code = ExitCode::classify_transport_error(&detail);
            report_error(
                Some(label_for(&command)),
                flags.output,
                flags.quiet,
                code,
                &detail,
            )
        }
    }
}

/// Render the `pcloudc log` output for an Ok daemon response.
///
/// The daemon carries the revision list as a JSON array in
/// [`pcloud_ipc::Response::message`]. In JSON mode we preserve the
/// raw array inside the standard success envelope so downstream
/// tooling sees a structured payload; in text mode we format each
/// revision in a git-log-style block:
///
/// ```text
/// rev <hex>
/// Author: <user>
/// Date:   <rfc3339-ish>
///
///     <optional comment>
/// ```
///
/// Malformed payloads fall back to the standard success envelope so
/// the operator still sees *something* rather than a silent exit.
fn render_file_history(
    command: &commands::Command,
    response: &pcloud_ipc::Response,
    flags: &GlobalFlags,
    code: ExitCode,
) -> ExitCode {
    #[derive(serde::Deserialize)]
    struct Revision {
        #[serde(default)]
        rev_id: String,
        #[serde(default)]
        mtime: u64,
        #[serde(default)]
        size: u64,
        #[serde(default)]
        user: String,
        #[serde(default)]
        comment: String,
    }

    match flags.output {
        OutputFormat::Json => {
            // Preserve the array payload verbatim inside the success
            // envelope so callers parse one stable schema.
            let env = JsonEnvelope::from_response(label_for(command), response);
            print!("{}", env.render());
        }
        OutputFormat::Text => {
            let revisions: Vec<Revision> =
                serde_json::from_str(&response.message).unwrap_or_default();
            if revisions.is_empty() {
                // Either an empty history or a malformed payload; a
                // single-line note is less surprising than silence.
                println!("(no revisions)");
                return code;
            }
            for (idx, rev) in revisions.iter().enumerate() {
                if idx > 0 {
                    println!();
                }
                println!("rev {}", rev.rev_id);
                if !rev.user.is_empty() {
                    println!("Author: {}", rev.user);
                }
                // RFC3339-ish rendering without pulling in chrono: emit
                // the raw UNIX seconds with the token a human reader can
                // post-process. Keeps the dependency surface minimal.
                println!("Date:   {} (size={})", rev.mtime, rev.size);
                if !rev.comment.is_empty() {
                    println!();
                    for line in rev.comment.lines() {
                        println!("    {line}");
                    }
                }
            }
        }
    }
    code
}

/// Emit an error on the appropriate stream and return the chosen exit code.
///
/// In text mode: error text goes to stderr (preserving legacy behavior).
/// In JSON mode: a JSON error envelope goes to stdout so pipelines can
/// consume one stream, and stderr is silent. `quiet` suppresses stderr in
/// text mode and stdout in JSON mode, but never changes the exit code.
fn report_error(
    command: Option<String>,
    format: OutputFormat,
    quiet: bool,
    code: ExitCode,
    detail: &str,
) -> ExitCode {
    match format {
        OutputFormat::Json => {
            if !quiet {
                let env = JsonEnvelope::from_error(command, code, detail);
                print!("{}", env.render());
            }
        }
        OutputFormat::Text => {
            if !quiet {
                eprintln!("{detail}");
            }
        }
    }
    code
}

/// Detect a top-level `completion <shell>` invocation in the reduced argv
/// (i.e. after global flags have been stripped). Returns the shell token if
/// the command is `completion`, otherwise `None`.
fn completion_request(reduced: &[String]) -> Option<&str> {
    if reduced.get(1).map(String::as_str) == Some("completion") {
        Some(reduced.get(2).map(String::as_str).unwrap_or(""))
    } else {
        None
    }
}

fn handle_completion(flags: &GlobalFlags, shell_arg: &str) -> ExitCode {
    let Some(shell) = completion::parse_shell(shell_arg) else {
        return report_error(
            Some("completion".into()),
            flags.output,
            flags.quiet,
            ExitCode::Usage,
            &format!(
                "completion requires a shell name (bash|zsh|fish|elvish|powershell), got: '{shell_arg}'"
            ),
        );
    };
    if flags.quiet {
        return ExitCode::Ok;
    }
    let mut stdout = std::io::stdout().lock();
    completion::generate_completion(shell, &mut stdout);
    ExitCode::Ok
}

/// Human-readable label for a command, used in JSON envelopes.
fn label_for(command: &commands::Command) -> String {
    format!("{command:?}")
}

/// `true` when the command's positional arguments are read-only
/// identifiers (or the command takes none), so that any extra trailing
/// positional tokens can safely be re-interpreted as implicit
/// `--field` values for the jq-lite projection layer.
///
/// The list is intentionally conservative: every entry was audited
/// against `app::parse_inputs_for_command` to confirm it does not
/// consume variable-length positional arguments that a user might
/// have meant literally. Additions require the same audit.
/// Translate well-known pCloud server result codes to human-friendly
/// messages for the Stage 4b.4 crypto commands. If `message` does not
/// contain a recognised code, it is returned verbatim so the caller
/// still surfaces whatever the server said. Tokens are matched
/// conservatively: we search for the literal `"result=<code>"` /
/// `"code=<code>"` / `" <code>"` substring and require a digit
/// boundary so `"12110"` does not match `"2110"`.
///
/// Recognised codes (subset — only those documented in the Stage 4b.4
/// spec are translated; everything else is surfaced as
/// `result=<code>: <server-provided message>`):
///
/// | Code | Human text                                                                     |
/// |------|-------------------------------------------------------------------------------|
/// | 1000 | not logged in                                                                 |
/// | 2000 | can't connect to pCloud server                                                |
/// | 2110 | crypto already set up — use 'crypto change-password' to rotate                 |
fn translate_server_result_code(message: &str) -> String {
    const TABLE: &[(u32, &str)] = &[
        (1000, "not logged in"),
        (2000, "can't connect to pCloud server"),
        (
            2110,
            "crypto already set up — use 'crypto change-password' to rotate",
        ),
    ];
    for (code, human) in TABLE {
        let patterns = [
            format!("result={code}"),
            format!("code={code}"),
            format!(" {code}"),
        ];
        for pat in &patterns {
            if let Some(idx) = message.find(pat.as_str()) {
                // Digit-boundary guard: the char immediately after the
                // match must not be another digit (rejects 21100).
                let after = idx + pat.len();
                let next_is_digit = message[after..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit());
                if !next_is_digit {
                    return format!("{human} (server result={code})");
                }
            }
        }
    }
    // Unknown code — surface the raw message so operators can still
    // see the server-provided detail. This is the "Other codes: surface
    // the raw code + server-provided message" branch from the spec.
    message.to_owned()
}

/// Extract a `backend=<value>` token from `response.message` if the
/// daemon emitted one. Returns `None` if the field is not present so
/// the caller can fall back to the `"unknown"` renderer.
///
/// The daemon's current `crypto_status` handler in
/// `crates/pcloud-daemon/src/runtime.rs` does **not** emit this token.
/// The parsing hook is here so widening the IPC response under
/// bd-1du.10 Stage 6 becomes a pure daemon change with zero CLI
/// churn — add `backend=<name>` to the status/unlock response and
/// this function starts returning `Some(...)` without further edits.
///
/// Daemon emits `backend=<name>` as one token in the comma-separated
/// status / start messages; this extracts it without allocating. If/when
/// the daemon grows a typed `CryptoStatusPayload` (tracked as a separate
/// follow-up), this helper can be retired.
fn scrape_backend_token(message: &str) -> Option<&str> {
    for part in message.split(|c: char| c == ',' || c.is_whitespace()) {
        if let Some(rest) = part.strip_prefix("backend=") {
            let rest = rest.trim_matches('"');
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

/// For `crypto status`, render a leading line of the form:
/// `Backend: pclsync-compat` or
/// `Backend: enhanced  (⚠ not interoperable with pCloud apps)`.
///
/// Returns `None` for every other command (the regular title renders
/// unchanged).
fn render_backend_prefix(
    command: &commands::Command,
    response: &pcloud_ipc::Response,
) -> Option<String> {
    if !matches!(command, commands::Command::CryptoStatus) {
        return None;
    }
    match scrape_backend_token(&response.message) {
        Some("enhanced") => {
            Some("Backend: enhanced  (⚠ not interoperable with pCloud apps)".to_owned())
        }
        Some(other) => Some(format!("Backend: {other}")),
        // Daemon always emits `backend=<name>` now; if we reach this arm
        // we are talking to a pre-Wave-2 daemon. Render nothing rather
        // than surfacing "unknown" to the user.
        None => None,
    }
}

/// For `crypto start` / `unlock-crypto`, append a `(backend: ...)`
/// tail to the success line. See [`render_backend_prefix`] for the
/// pre-Wave-2 daemon compatibility notes.
fn render_backend_suffix(
    command: &commands::Command,
    response: &pcloud_ipc::Response,
) -> Option<String> {
    if !matches!(command, commands::Command::SubmitCryptoPassword) {
        return None;
    }
    if !matches!(response.status, pcloud_ipc::ResponseStatus::Ok) {
        return None;
    }
    scrape_backend_token(&response.message).map(|name| format!("Unlocked (backend: {name})"))
}

fn command_accepts_bare_fields(command: &commands::Command) -> bool {
    use commands::Command as C;
    matches!(
        command,
        C::UserInfo
            | C::Status
            | C::Health
            | C::Doctor
            | C::ListLinks
            | C::ListUploadLinks
            | C::ListNotifications
            | C::SessionStatus
            | C::SyncList
            | C::IntegrityStatus
            | C::HaStatus
            | C::FilesystemStatus
            | C::CryptoStatus
            | C::Slo
    )
}

/// Collect any trailing bare positional tokens left in `argv` *after*
/// the command token as implicit `--field` values for whitelisted
/// commands, and return the argv with those tokens stripped.
///
/// Rules:
/// - Only applies when the command is [`command_accepts_bare_fields`].
/// - Tokens starting with `-` are left in place (flags).
/// - The very next token after the program/command is preserved for
///   commands that already have exactly one required positional
///   (none of the whitelisted ones do today, but the guard is kept).
/// - Collection starts at offset 2 (argv\[0\]=program, argv\[1\]=command).
fn extract_bare_field_positionals(
    command: &commands::Command,
    argv: Vec<String>,
) -> (Vec<String>, Vec<String>) {
    if !command_accepts_bare_fields(command) {
        return (argv, Vec::new());
    }
    let mut kept = Vec::with_capacity(argv.len());
    let mut fields = Vec::new();
    for (idx, tok) in argv.iter().enumerate() {
        if idx < 2 || tok.starts_with('-') || tok == "-" {
            kept.push(tok.clone());
            continue;
        }
        fields.push(tok.clone());
    }
    (kept, fields)
}

/// Apply the collected field selectors to `response` and render
/// accordingly. Returns the effective exit code.
///
/// Behaviour:
/// - Parses `response.message` via [`field_selector::parse_message_to_json`].
/// - For each selector string, builds a
///   [`field_selector::FieldSelector`] and applies it.
/// - On any NotFound/TypeMismatch, exits `2 Usage` via [`report_error`]
///   with a clear message (no daemon call is retried).
/// - In text mode: prints one value per line in selector order,
///   using [`field_selector::render_value_plain`].
/// - In JSON mode: emits a [`JsonEnvelope::Filtered`] envelope.
fn render_with_field_selection(
    command: &commands::Command,
    response: &pcloud_ipc::Response,
    flags: &GlobalFlags,
    code: ExitCode,
) -> ExitCode {
    use crate::field_selector::{FieldSelector, parse_message_to_json, render_value_plain};

    let parsed = parse_message_to_json(&response.message);

    let mut rendered_plain: Vec<String> = Vec::with_capacity(flags.fields.len());
    let mut rendered_json = serde_json::Map::with_capacity(flags.fields.len());
    for raw in &flags.fields {
        let sel = FieldSelector::parse(raw);
        match sel.apply(&parsed) {
            Ok(v) => {
                rendered_plain.push(render_value_plain(&v));
                rendered_json.insert(raw.clone(), v);
            }
            Err(err) => {
                let detail = format!("{err}");
                return report_error(
                    Some(label_for(command)),
                    flags.output,
                    flags.quiet,
                    ExitCode::Usage,
                    &detail,
                );
            }
        }
    }

    if flags.quiet {
        return code;
    }

    match flags.output {
        OutputFormat::Json => {
            let env =
                json_output::JsonEnvelope::from_fields(label_for(command), response, rendered_json);
            print!("{}", env.render());
        }
        OutputFormat::Text => {
            for line in &rendered_plain {
                println!("{line}");
            }
        }
    }
    code
}

/// Spawn `pcloudd` in the background. Idempotent: if the socket
/// already responds (daemon already running) we report and exit
/// success. Otherwise we locate the daemon binary (same directory as
/// this CLI, then `$PATH`), redirect its stdio to
/// `${PCLOUD_ROOT:-~/.pcloud}/state/daemon.log`, detach it via
/// `setsid`, and poll the socket for up to ~5 seconds waiting for
/// `daemon listening` readiness.
///
/// Security notes:
/// - The child inherits the parent's environment as-is so
///   `PCLOUD_DURABLE_AUTH_TOKENS`, `PCLOUD_ENV`, etc. carry through.
///   Secret-bearing envs (e.g. from `--password-env`) are already
///   scrubbed before we reach here.
/// - The daemon log file is created 0600.
/// - No privileges are acquired or dropped; we run as the caller.
fn run_daemon_start(flags: &GlobalFlags) -> ExitCode {
    #[cfg(not(unix))]
    {
        let _ = flags;
        return report_error(
            Some("Start".into()),
            flags.output,
            flags.quiet,
            ExitCode::GenericError,
            "`pcloudc start` daemon-detach path is Unix-only; Windows service \
             start is tracked under bd-xplat-windows.",
        );
    }
    #[cfg(unix)]
    {
        use pcloud_ipc::{IpcClient, Method, Request};
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::os::unix::process::CommandExt as _;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let socket_path = match socket_path_for_defaults() {
            Ok(p) => p,
            Err(err) => {
                return report_error(
                    Some("Start".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::GenericError,
                    &format!("socket resolution failed: {err}"),
                );
            }
        };

        // Fast path: daemon is already running?
        let client = IpcClient;
        let probe = Request::Plain {
            method: Method::GetHealth,
        };
        if client.send(&socket_path, &probe).is_ok() {
            if !flags.quiet {
                println!(
                    "pcloudd already running (socket {} is live)",
                    socket_path.display()
                );
            }
            return ExitCode::Ok;
        }

        // Resolve daemon binary path. Prefer sibling of the running CLI so
        // a local build works without $PATH manipulation; then PATH.
        let daemon_path: std::path::PathBuf = match std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("pcloudd")))
            .filter(|p| p.is_file())
        {
            Some(p) => p,
            None => std::path::PathBuf::from("pcloudd"),
        };

        // Prepare the log directory and file (0600). Under the same
        // ~/.pcloud/ root the daemon itself uses so operators have a single
        // location to tail.
        let root = socket_path
            .parent() // runtime/
            .and_then(|p| p.parent()) // root
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let log_dir = root.join("state");
        if let Err(err) = std::fs::create_dir_all(&log_dir) {
            return report_error(
                Some("Start".into()),
                flags.output,
                flags.quiet,
                ExitCode::GenericError,
                &format!("log dir create failed: {err}"),
            );
        }
        let log_path = log_dir.join("daemon.log");
        let log_file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)
        {
            Ok(f) => f,
            Err(err) => {
                return report_error(
                    Some("Start".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::GenericError,
                    &format!("log open failed ({}): {err}", log_path.display()),
                );
            }
        };
        let log_stderr = match log_file.try_clone() {
            Ok(f) => f,
            Err(err) => {
                return report_error(
                    Some("Start".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::GenericError,
                    &format!("log clone failed: {err}"),
                );
            }
        };

        // Read the user's config and project the relevant non-secret
        // settings into env vars the daemon honours at bootstrap. CLI is
        // the orchestrator of the daemon's startup environment per the
        // user's requirement (the daemon never reads the TOML directly).
        let cfg = config::CliConfig::load_or_init(&config::CliConfig::default_path(None))
            .unwrap_or_default();

        // Spawn with setsid so the child becomes a session leader and is
        // not killed when this CLI exits.
        let mut cmd = Command::new(&daemon_path);
        cmd.arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_stderr));
        if let Some(gb) = cfg.cache_size_gb {
            cmd.env("PCLOUD_CACHE_SIZE_GB", gb.to_string());
        }
        if let Some(p) = cfg.mountpoint.as_ref() {
            cmd.env("PCLOUD_DEFAULT_MOUNTPOINT", p);
        }
        if let Some(p) = cfg.log_path.as_ref() {
            cmd.env("PCLOUD_LOG_PATH", p);
        }
        if let Some(p) = cfg.fs_event_log.as_ref() {
            cmd.env("PCLOUD_FS_EVENT_LOG", p);
        }
        if let Some(lvl) = cfg.log_level.as_deref() {
            cmd.env("PCLOUD_LOG_LEVEL", lvl);
        }
        if let Some(opts) = cfg.fuse_opts.as_deref() {
            cmd.env("PCLOUD_FUSE_OPTS", opts);
        }
        // SAFETY: `pre_exec` closure runs in the child process after `fork()`
        // and before `exec()`, where only async-signal-safe functions may be
        // called. `setsid(2)` is explicitly async-signal-safe per POSIX and
        // is the standard way to detach a daemon from its controlling terminal.
        unsafe {
            cmd.pre_exec(|| {
                // SAFETY: setsid(2) is async-signal-safe per POSIX.
                // Errors here (already session leader, EPERM) are
                // surfaced as spawn failures via errno.
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                return report_error(
                    Some("Start".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::GenericError,
                    &format!(
                        "failed to spawn pcloudd ({}): {err}. Ensure pcloudd is on PATH or \
                     in the same directory as pcloudc.",
                        daemon_path.display()
                    ),
                );
            }
        };
        let pid = child.id();
        // We intentionally drop `child` without waiting: the daemon is
        // now a detached session leader. Leaking `Child` is correct here.
        std::mem::forget(child);

        // Poll the socket until `GetHealth` responds or we time out.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last_err = String::from("no response within timeout");
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            match client.send(&socket_path, &probe) {
                Ok(_) => {
                    if !flags.quiet {
                        // Also print the log path for convenience.
                        let _ = writeln!(
                            std::io::stdout(),
                            "pcloudd started (pid={pid}, socket={}, log={})",
                            socket_path.display(),
                            log_path.display()
                        );
                    }
                    return ExitCode::Ok;
                }
                Err(e) => last_err = e.to_string(),
            }
        }

        report_error(
            Some("Start".into()),
            flags.output,
            flags.quiet,
            ExitCode::Unavailable,
            &format!(
                "pcloudd was spawned (pid={pid}) but its socket did not come up within 5s: \
             {last_err}. Check {}",
                log_path.display()
            ),
        )
    }
}

/// Drive the CLI-side `pcloudc drain` command.
///
/// 1. Resolves `<state_dir>/daemon.pid` from the active config profile.
/// 2. Parses the pid, sends SIGTERM via `kill(2)`.
/// 3. Polls `Method::DrainStatus` every 500 ms.
/// 4. Exits `Ok` once the daemon reports `state == "stopped"`; exits
///    `Unavailable` on timeout (`upgrade.handoff_timeout_secs`) or when
///    the pidfile is missing/unreadable.
///
/// Honors `--quiet` and `--json`. The JSON envelope on success is
/// `{kind: "success", command: "drain", status: "Ok", message: "<final
/// drain payload>", exit_code: 0}`.
#[cfg(unix)]
fn run_daemon_drain(flags: &GlobalFlags) -> ExitCode {
    use pcloud_ipc::{Method, Request, ResponseStatus};
    use std::time::{Duration, Instant};

    // Resolve state_dir via the same config discovery path `start`
    // uses, so the two commands always target the same daemon
    // instance.
    let (state_dir, socket_path, handoff_timeout_secs) = match resolve_drain_paths() {
        Ok(triple) => triple,
        Err(err) => {
            return report_error(
                Some("drain".into()),
                flags.output,
                flags.quiet,
                ExitCode::GenericError,
                &format!("drain path resolution failed: {err}"),
            );
        }
    };

    let pid_path = state_dir.join("daemon.pid");
    let pid = match read_pid_file(&pid_path) {
        Ok(p) => p,
        Err(err) => {
            return report_error(
                Some("drain".into()),
                flags.output,
                flags.quiet,
                ExitCode::Unavailable,
                &format!("pidfile {} unreadable: {err}", pid_path.display()),
            );
        }
    };

    // Send SIGTERM. A missing process (ESRCH) is treated as "already
    // stopped" — idempotent.
    let send_rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if send_rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            if !flags.quiet {
                println!("pcloudd (pid={pid}) already stopped");
            }
            return ExitCode::Ok;
        }
        return report_error(
            Some("drain".into()),
            flags.output,
            flags.quiet,
            ExitCode::GenericError,
            &format!("SIGTERM to pid={pid} failed: {err}"),
        );
    }

    if !flags.quiet {
        println!(
            "SIGTERM sent to pcloudd (pid={pid}); polling drain status (timeout={handoff_timeout_secs}s)"
        );
    }

    // Poll DrainStatus every 500 ms.
    let client = IpcClient;
    let deadline = Instant::now() + Duration::from_secs(u64::from(handoff_timeout_secs));
    let poll_interval = Duration::from_millis(500);
    let probe = Request::Plain {
        method: Method::DrainStatus,
    };
    let mut last_message = String::new();
    while Instant::now() < deadline {
        match client.send(&socket_path, &probe) {
            Ok(resp) => {
                last_message = resp.message.clone();
                if matches!(resp.status, ResponseStatus::Ok)
                    && resp.message.contains("\"state\":\"stopped\"")
                {
                    if !flags.quiet {
                        println!("drain complete: {}", resp.message);
                    }
                    return ExitCode::Ok;
                }
            }
            Err(_) => {
                // Socket gone → daemon has fully exited. Treat as
                // success: the contract says `Ok` on `state=stopped`
                // OR socket-gone (which is the terminal state after
                // `serve_until_shutdown` returns and `BoundIpcServer`
                // is dropped).
                if !flags.quiet {
                    println!("drain complete: socket closed by daemon");
                }
                return ExitCode::Ok;
            }
        }
        std::thread::sleep(poll_interval);
    }

    report_error(
        Some("drain".into()),
        flags.output,
        flags.quiet,
        ExitCode::Unavailable,
        &format!(
            "drain timed out after {handoff_timeout_secs}s (pid={pid}); last status: {last_message}"
        ),
    )
}

/// Resolve the (state_dir, socket_path, handoff_timeout_secs) triple
/// used by `run_daemon_drain`. Mirrors the discovery logic in
/// `socket_path_for_defaults` so both commands target the same daemon.
fn resolve_drain_paths() -> Result<(std::path::PathBuf, std::path::PathBuf, u32), String> {
    let config = match std::env::var_os("PCLOUD_ROOT") {
        Some(r) => {
            ConfigProfile::secure_defaults(std::path::PathBuf::from(r), Environment::Development)
        }
        None => {
            let dirs = pcloud_config::paths::PcloudDirs::discover().map_err(|e| format!("{e}"))?;
            let mut p = ConfigProfile::secure_defaults(
                std::path::PathBuf::from("/"),
                Environment::Development,
            );
            p.paths = dirs.to_managed_paths();
            p
        }
    };
    let socket = config.paths.ipc_socket_path();
    let state = config.paths.state_dir.clone();
    let timeout = config.upgrade.handoff_timeout_secs;
    Ok((state, socket, timeout))
}

/// Read a pidfile written by `pcloudd serve`. Accepts a single
/// whitespace-trimmed decimal integer on the first non-empty line.
fn read_pid_file(path: &std::path::Path) -> Result<u32, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("pidfile is empty".to_owned());
    }
    trimmed
        .parse::<u32>()
        .map_err(|e| format!("pidfile contained non-numeric '{trimmed}': {e}"))
}

/// Send SIGHUP to the running daemon to trigger a config hot-reload.
///
/// Resolves the pidfile via the same config discovery path as `drain`,
/// sends `SIGHUP` via `kill(2)`, and exits. The daemon's serve loop
/// observes the `RELOAD_REQUESTED` flag and re-reads the config file.
#[cfg(unix)]
fn run_daemon_reload(flags: &GlobalFlags) -> ExitCode {
    let (state_dir, _socket_path, _timeout) = match resolve_drain_paths() {
        Ok(t) => t,
        Err(err) => {
            return report_error(
                Some("reload".into()),
                flags.output,
                flags.quiet,
                ExitCode::Unavailable,
                &format!("cannot resolve daemon paths: {err}"),
            );
        }
    };

    let pid_path = state_dir.join("daemon.pid");
    let pid = match read_pid_file(&pid_path) {
        Ok(p) => p,
        Err(err) => {
            return report_error(
                Some("reload".into()),
                flags.output,
                flags.quiet,
                ExitCode::Unavailable,
                &format!("pidfile {} unreadable: {err}", pid_path.display()),
            );
        }
    };

    // Send SIGHUP.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGHUP) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return report_error(
            Some("reload".into()),
            flags.output,
            flags.quiet,
            ExitCode::Unavailable,
            &format!("kill({pid}, SIGHUP) failed: {err}"),
        );
    }

    if !flags.quiet {
        eprintln!("SIGHUP sent to daemon (pid={pid}); config hot-reload requested.");
    }
    ExitCode::Ok
}

#[cfg(not(unix))]
fn run_daemon_reload(flags: &GlobalFlags) -> ExitCode {
    report_error(
        Some("reload".into()),
        flags.output,
        flags.quiet,
        ExitCode::Unavailable,
        "config reload via SIGHUP is only supported on Unix",
    )
}

#[cfg(not(unix))]
fn run_daemon_drain(flags: &GlobalFlags) -> ExitCode {
    report_error(
        Some("drain".into()),
        flags.output,
        flags.quiet,
        ExitCode::Unavailable,
        "`pcloudc drain` (SIGTERM + pidfile poll) is Unix-only; \
         Windows service-controlled stop is tracked under bd-xplat-windows.",
    )
}

/// Drive the `pcloudc doctor` self-diagnostic flow. Respects global
/// `--json`, `--quiet`, and the documented exit-code mapping defined on
/// [`doctor::DoctorReport::exit_code`]. Never reads any secret-bearing
/// input; only prints filesystem metadata, TCP probe results, and the
/// outcome of a single plain `GetStatus` IPC round-trip.
fn run_doctor(flags: &GlobalFlags, reduced: &[String]) -> ExitCode {
    let mount_root = config::CliConfig::load_or_init(&config::CliConfig::default_path(None))
        .ok()
        .and_then(|c| c.mountpoint);
    // `pcloudc doctor --strict` promotes every WARN to FAIL so CI
    // pipelines can gate on advisory warnings. The flag has no value
    // and is accepted only for the `doctor` subcommand.
    let strict = reduced.iter().any(|a| a == "--strict");
    let opts = doctor::DoctorOptions {
        mount_root,
        strict,
        ..Default::default()
    };
    let report = match doctor::run(&opts) {
        Ok(r) => r,
        Err(e) => {
            return report_error(
                Some("doctor".into()),
                flags.output,
                flags.quiet,
                ExitCode::GenericError,
                &format!("doctor failed: {e}"),
            );
        }
    };
    if !flags.quiet {
        match flags.output {
            OutputFormat::Json => match report.render_json() {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    return report_error(
                        Some("doctor".into()),
                        flags.output,
                        flags.quiet,
                        ExitCode::GenericError,
                        &format!("doctor json render failed: {e}"),
                    );
                }
            },
            OutputFormat::Text => {
                print!("{}", report.render_text());
            }
        }
    }
    report.exit_code()
}

/// Drive `pcloudc migrate-from-c`. Parses the subcommand's own flags
/// (`--dry-run`, `--force-overwrite`, `--from <path>`) out of the
/// normalized argv, then invokes the detection / execution pipeline in
/// [`crate::migrate`]. Never contacts the daemon.
///
/// Secret handling: the auth token lifted from the legacy DB never
/// leaves the `SecretString` wrapper inside `migrate.rs`; this driver
/// only sees a boolean "was a token found" signal, so there is nothing
/// secret to log here.
#[cfg(unix)]
fn run_migrate_from_c(flags: &GlobalFlags, reduced: &[String]) -> ExitCode {
    // Flag parsing — hand-rolled to stay aligned with the rest of the
    // CLI which avoids `clap` at this layer for argv determinism.
    let mut dry_run = false;
    let mut force_overwrite = false;
    let mut from: Option<std::path::PathBuf> = None;
    let mut i = 2; // reduced[0] = argv[0], reduced[1] = canonical token
    while i < reduced.len() {
        let tok = reduced[i].as_str();
        match tok {
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            "--force-overwrite" => {
                force_overwrite = true;
                i += 1;
            }
            "--from" => {
                if i + 1 >= reduced.len() {
                    return report_error(
                        Some("migrate-from-c".into()),
                        flags.output,
                        flags.quiet,
                        ExitCode::Usage,
                        "migrate-from-c --from: missing <path>",
                    );
                }
                from = Some(std::path::PathBuf::from(reduced[i + 1].as_str()));
                i += 2;
            }
            other if other.starts_with("--from=") => {
                from = Some(std::path::PathBuf::from(&other[7..]));
                i += 1;
            }
            _ => i += 1,
        }
    }

    let plan = match migrate::MigrationPlan::detect_from(from, dry_run, force_overwrite) {
        Ok(Some(p)) => p,
        Ok(None) => {
            let msg = "no legacy ~/.pcloud/.pclouddb found — nothing to migrate";
            if !flags.quiet {
                match flags.output {
                    OutputFormat::Json => {
                        let env = JsonEnvelope::Success {
                            command: "migrate-from-c".into(),
                            status: json_output::JsonStatus::Ok,
                            message: msg.to_owned(),
                            exit_code: 0,
                        };
                        print!("{}", env.render());
                    }
                    OutputFormat::Text => println!("{msg}"),
                }
            }
            return ExitCode::Ok;
        }
        Err(e) => {
            return report_error(
                Some("migrate-from-c".into()),
                flags.output,
                flags.quiet,
                ExitCode::GenericError,
                &format!("migrate-from-c detect failed: {e}"),
            );
        }
    };

    if dry_run {
        let preview = plan.render_preview();
        if !flags.quiet {
            match flags.output {
                OutputFormat::Json => {
                    let env = JsonEnvelope::Success {
                        command: "migrate-from-c".into(),
                        status: json_output::JsonStatus::Ok,
                        message: preview,
                        exit_code: 0,
                    };
                    print!("{}", env.render());
                }
                OutputFormat::Text => {
                    print!("{preview}");
                    println!(
                        "\nRerun without --dry-run to execute. Add --force-overwrite to \
                         replace an existing Rust store (destructive)."
                    );
                }
            }
        }
        return ExitCode::Ok;
    }

    match plan.execute() {
        Ok(report) => {
            if !flags.quiet {
                match flags.output {
                    OutputFormat::Json => {
                        let env = JsonEnvelope::Success {
                            command: "migrate-from-c".into(),
                            status: json_output::JsonStatus::Ok,
                            message: migrate::render_report(&report),
                            exit_code: 0,
                        };
                        print!("{}", env.render());
                    }
                    OutputFormat::Text => {
                        print!("{}", migrate::render_report(&report));
                    }
                }
            }
            ExitCode::Ok
        }
        Err(migrate::MigrateError::RustStateAlreadyPresent { path }) => report_error(
            Some("migrate-from-c".into()),
            flags.output,
            flags.quiet,
            ExitCode::Conflict,
            &format!(
                "Rust daemon state already present at {}. Remove it first OR use \
                 `--force-overwrite` (destructive).",
                path.display()
            ),
        ),
        Err(e) => report_error(
            Some("migrate-from-c".into()),
            flags.output,
            flags.quiet,
            ExitCode::GenericError,
            &format!("migrate-from-c failed: {e}"),
        ),
    }
}

/// Queue the daemon for a clean restart so that env-bound config
/// changes (cache size, default mountpoint, log paths/level, FUSE
/// opts) take effect. Called by `pcloudc login` whenever it writes
/// such a value to the TOML.
///
/// Order of operations:
///   1. If daemon is not running → nothing to do.
///   2. If a filesystem is mounted → request unmount. The daemon's
///      own drain hook flushes the write-path journal + staging
///      blobs through `upload_save` before releasing the mountpoint
///      (see `mount_runtime::pcloud_shim_adapter_factory`).
///   3. Send a Shutdown request and wait briefly for the socket to
///      close.
///   4. Re-spawn `pcloudd` via the same `run_daemon_start` code path
///      so the new env vars (read from the freshly-saved TOML) reach
///      `pcloudd serve`.
///
/// Note: post-restart, the daemon resumes from the auth-token vault
/// when `authsave` is on; otherwise the user's subsequent
/// `run_interactive_login` flow re-authenticates. Mountpoint is
/// re-established by the post-login `Mount` call. Crypto-unlocked
/// state cannot be restored automatically (no persisted password) —
/// `-c` / `-y` options on the same `login` invocation will re-unlock.
fn queue_daemon_restart_for_config_change(flags: &GlobalFlags) {
    use pcloud_ipc::{IpcClient, Method, Request};
    let Ok(socket_path) = socket_path_for_defaults() else {
        return;
    };
    let client = IpcClient;
    // 1. Daemon running?
    let probe = Request::Plain {
        method: Method::GetHealth,
    };
    if client.send(&socket_path, &probe).is_err() {
        return;
    }
    if !flags.quiet {
        eprintln!("config changed; restarting daemon (draining pending writes…)");
    }
    // 2. Drain via unmount (no-op if not mounted).
    let _ = client.send(&socket_path, &Request::Unmount);
    // 3. Shutdown.
    let _ = client.send(
        &socket_path,
        &Request::Plain {
            method: Method::Shutdown,
        },
    );
    // Wait for socket to actually go away (up to ~3s).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if client.send(&socket_path, &probe).is_err() {
            break;
        }
    }
    // 4. Re-spawn the daemon. We reuse the same code path as
    //    `pcloudc start` so env-projection and log redirection are
    //    identical.
    let _ = run_daemon_start(flags);
}

/// **PLATFORM:** all. Resolve the IPC socket path from XDG-canonical
/// directories (`PcloudDirs::discover()`) unless `PCLOUD_ROOT` is set,
/// in which case the legacy single-root layout is honoured.
/// Extracted so the interactive-login REPL can share it.
fn socket_path_for_defaults() -> Result<std::path::PathBuf, String> {
    let config = match std::env::var_os("PCLOUD_ROOT") {
        Some(r) => {
            ConfigProfile::secure_defaults(std::path::PathBuf::from(r), Environment::Development)
        }
        None => {
            let dirs = pcloud_config::paths::PcloudDirs::discover().map_err(|e| format!("{e}"))?;
            let mut p = ConfigProfile::secure_defaults(
                std::path::PathBuf::from("/"),
                Environment::Development,
            );
            p.paths = dirs.to_managed_paths();
            p
        }
    };
    Ok(config.paths.ipc_socket_path())
}

/// **PLATFORM:** all (Unix and Windows). When a request fails because
/// the daemon socket does not exist, prompt the user once for consent
/// and spawn `pcloudd serve` detached, then wait up to 10s for the
/// socket to bind. Skips the prompt entirely (returns `Err` quietly)
/// when stdin is not a TTY or `--quiet` is set, so scripts and CI
/// preserve the historical NotFound error.
///
/// Locates the daemon binary by:
/// 1. `$PCLOUDD` env var (full path), if set;
/// 2. sibling of `current_exe()` (covers `cargo install`, in-tree
///    `target/release/`, and packaged installs that ship pcloudd next
///    to pcloudc);
/// 3. `PATH` lookup as a last resort.
///
/// The spawned child detaches from the controlling terminal so the
/// CLI can exit cleanly while the daemon keeps running.
fn try_autostart_daemon(
    socket_path: &std::path::Path,
    flags: &GlobalFlags,
) -> Result<(), String> {
    use std::io::{IsTerminal, Write};

    if flags.quiet {
        return Err("--quiet: skipping interactive autostart prompt".into());
    }
    if !std::io::stdin().is_terminal() {
        return Err("stdin is not a TTY: skipping interactive autostart prompt".into());
    }

    eprint!("pcloudd is not running. Start it now? [Y/n] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return Err("could not read confirmation".into());
    }
    if !is_affirmative(answer.trim()) {
        return Err("declined by user".into());
    }

    let daemon = locate_daemon_binary()?;
    eprintln!("Starting daemon: {} serve …", daemon.display());
    let mut cmd = std::process::Command::new(&daemon);
    cmd.arg("serve");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        // Detach from the controlling terminal so a Ctrl-C in the CLI
        // does not also kill the freshly spawned daemon.
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Best-effort setsid; ignore EPERM (already a session
                // leader) — the only failure that matters is
                // "completely cannot daemonize", which would be
                // surfaced by the spawn() call below anyway.
                let _ = libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn()
        .map_err(|e| format!("spawn pcloudd: {e}"))?;

    // Wait for the socket to appear and the daemon to start accepting
    // connections. Connect-and-disconnect probes are the only honest
    // signal: existence of the socket file alone does not mean
    // `accept()` is armed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if socket_path.exists() {
            #[cfg(unix)]
            {
                if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
                    return Ok(());
                }
            }
            #[cfg(not(unix))]
            {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Err("daemon did not become reachable within 10s".into())
}

/// Match a Y/N answer in a few common forms (English + French) plus
/// the bare-Enter default. Empty input means "yes" since the prompt
/// is rendered with `[Y/n]`.
fn is_affirmative(answer: &str) -> bool {
    let a = answer.trim().to_ascii_lowercase();
    matches!(
        a.as_str(),
        "" | "y" | "yes" | "yeah" | "yep" | "o" | "oui" | "ok" | "1"
    )
}

/// Locate the `pcloudd` binary for autostart. Order: `$PCLOUDD`,
/// sibling of `current_exe()`, then `$PATH`.
fn locate_daemon_binary() -> Result<std::path::PathBuf, String> {
    let bin_name = if cfg!(windows) { "pcloudd.exe" } else { "pcloudd" };
    if let Some(env_path) = std::env::var_os("PCLOUDD") {
        let p = std::path::PathBuf::from(env_path);
        if p.exists() {
            return Ok(p);
        }
        return Err(format!(
            "$PCLOUDD={} does not exist",
            p.display()
        ));
    }
    if let Ok(self_path) = std::env::current_exe() {
        if let Some(dir) = self_path.parent() {
            let candidate = dir.join(bin_name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(bin_name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    Err(format!(
        "could not find `{bin_name}` (set $PCLOUDD or place it next to pcloudc / on $PATH)"
    ))
}

/// Pre-supplied inputs for `pcloudc login` that skip the corresponding
/// REPL prompts, mirroring mysql's `-u <user>` / `-p` flag style.
///
/// Three layers, highest priority first:
///   1. Command-line flag (this struct, set by `from_argv`)
///   2. `~/.pcloud/config.toml` (or `--config <path>` / `$PCLOUD_CONFIG`)
///   3. Built-in default (typically `None` / `false`)
///
/// Secrets never come from `argv`; the password can be supplied only
/// via stdin (`--password-stdin`) or a scrubbed env var
/// (`--password-env VAR`).
#[derive(Debug, Default, Clone)]
struct LoginOptions {
    username: Option<String>,
    tfa_channel: Option<TfaChannel>,
    password_source: PasswordSource,
    crypto: Option<bool>,
    passascrypto: Option<bool>,
    trust_device: Option<bool>,
    save_password: Option<bool>,
    /// Empty string `Some("")` means "mount, use config or default
    /// path". `Some("/path")` means "mount at this explicit path".
    /// `None` means "do not mount".
    mountpoint: Option<String>,
    fuse_opts: Option<String>,
    log_path: Option<String>,
    fs_event_log: Option<String>,
    log_level: Option<String>,
    /// Maximum local cache size in **gigabytes**. Mirrors C
    /// `pcloud-rs --cache-size`. Persisted to config; effective on
    /// next daemon start.
    cache_size_gb: Option<u64>,
    /// Override the config file path. Read in `main` for ALL commands.
    config_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TfaChannel {
    Sms,
    Push,
}

#[derive(Debug, Default, Clone)]
enum PasswordSource {
    #[default]
    Prompt,
    Stdin,
    Env(String),
}

impl LoginOptions {
    /// Scan the reduced argv for login-specific flags.
    ///
    /// Supported forms:
    /// - `-u <user>` | `--user <user>` | `--username <user>`
    /// - `-T <sms|push>` | `--tfa-channel <sms|push>` | `--channel <sms|push>`
    ///   (`-c` is reserved for `--crypto`, matching C `pcloud-rs`)
    /// - `--password-stdin`
    /// - `--password-env <VAR>`
    fn from_argv(argv: &[String]) -> Self {
        let mut out = Self::default();
        let mut i = 0;
        // Helper for `<flag> <value>` pairs.
        let take = |argv: &[String], i: usize| argv.get(i + 1).cloned();
        while i < argv.len() {
            let a = argv[i].as_str();
            let mut consumed = 1;
            match a {
                "-u" | "--user" | "--username" => {
                    if let Some(v) = take(argv, i) {
                        out.username = Some(v);
                        consumed = 2;
                    }
                }
                s if s.starts_with("--user=") => {
                    out.username = Some(s["--user=".len()..].to_owned())
                }
                s if s.starts_with("--username=") => {
                    out.username = Some(s["--username=".len()..].to_owned())
                }
                "-T" | "--tfa-channel" | "--channel" => {
                    if let Some(v) = take(argv, i) {
                        out.tfa_channel = parse_channel(&v);
                        consumed = 2;
                    }
                }
                "--password-stdin" => out.password_source = PasswordSource::Stdin,
                "--password-env" => {
                    if let Some(v) = take(argv, i) {
                        out.password_source = PasswordSource::Env(v);
                        consumed = 2;
                    }
                }
                "-c" | "--crypto" => out.crypto = Some(true),
                "-y" | "--passascrypto" | "--pass-as-crypto" => {
                    out.passascrypto = Some(true);
                    out.crypto = Some(true); // implies crypto
                }
                "-r" | "--trust-device" | "--trusted-device" => out.trust_device = Some(true),
                "-s" | "--save-password" => out.save_password = Some(true),
                "-m" | "--mountpoint" => {
                    // Two-shape: `-m` alone (use config / default), or
                    // `-m /some/path`. We lookahead one token: if it
                    // exists and doesn't start with `-`, it's the value.
                    match argv.get(i + 1) {
                        Some(next) if !next.starts_with('-') => {
                            out.mountpoint = Some(next.clone());
                            consumed = 2;
                        }
                        _ => out.mountpoint = Some(String::new()),
                    }
                }
                s if s.starts_with("--mountpoint=") => {
                    out.mountpoint = Some(s["--mountpoint=".len()..].to_owned());
                }
                "-O" | "--fuse-opts" => {
                    if let Some(v) = take(argv, i) {
                        out.fuse_opts = Some(v);
                        consumed = 2;
                    }
                }
                "--log-path" => {
                    if let Some(v) = take(argv, i) {
                        out.log_path = Some(v);
                        consumed = 2;
                    }
                }
                "--fs-event-log" => {
                    if let Some(v) = take(argv, i) {
                        out.fs_event_log = Some(v);
                        consumed = 2;
                    }
                }
                "--log-level" => {
                    if let Some(v) = take(argv, i) {
                        out.log_level = Some(v);
                        consumed = 2;
                    }
                }
                "--cache-size" => {
                    if let Some(v) = take(argv, i) {
                        out.cache_size_gb = v.parse().ok();
                        consumed = 2;
                    }
                }
                "--config" => {
                    if let Some(v) = take(argv, i) {
                        out.config_path = Some(v);
                        consumed = 2;
                    }
                }
                _ => {}
            }
            i += consumed;
        }
        out
    }
}

fn parse_channel(s: &str) -> Option<TfaChannel> {
    match s.to_ascii_lowercase().as_str() {
        "sms" => Some(TfaChannel::Sms),
        "push" | "notification" | "notif" => Some(TfaChannel::Push),
        _ => None,
    }
}

/// Interactive `login` REPL.
///
/// Chains the legacy C `pcloud-rs` prompts — username, password, (optional)
/// 2FA code — into a single client-side flow that drives multiple IPC
/// round-trips based on the daemon's reply. No secret ever reaches
/// `argv`. Passwords are read via `rpassword` (no echo, no history).
fn run_interactive_login(
    flags: &GlobalFlags,
    opts: LoginOptions,
    cfg: config::CliConfig,
) -> ExitCode {
    use crate::prompt::SecretPrompt;
    use pcloud_ipc::{Method, Request};
    use pcloud_secret::{ExposeSecret, secret_string::SecretString};

    // Merge config defaults with explicit flags. Flags always win.
    let crypto_enabled = opts.crypto.unwrap_or(cfg.crypto);
    let passascrypto = opts.passascrypto.unwrap_or(cfg.passascrypto);
    let trust_device = opts.trust_device.unwrap_or(cfg.trust_device);
    let save_password = opts.save_password.unwrap_or(cfg.save_password);
    let mountpoint_request = opts.mountpoint.clone();
    let _fuse_opts = opts.fuse_opts.clone().or_else(|| cfg.fuse_opts.clone());
    let _log_path = opts
        .log_path
        .clone()
        .or_else(|| cfg.log_path.as_ref().map(|p| p.display().to_string()));
    let _fs_event_log = opts
        .fs_event_log
        .clone()
        .or_else(|| cfg.fs_event_log.as_ref().map(|p| p.display().to_string()));
    let _log_level = opts.log_level.clone().or_else(|| cfg.log_level.clone());
    let username_pre = opts.username.clone().or_else(|| cfg.username.clone());

    // Warn when `pcloudc login` was given log/fuse knobs that the
    // running daemon can't honor without restart. They've been written
    // to (or already exist in) the config file and will take effect on
    // next `pcloudc start`.
    if !flags.quiet
        && (opts.log_path.is_some()
            || opts.fs_event_log.is_some()
            || opts.log_level.is_some()
            || opts.fuse_opts.is_some()
            || opts.cache_size_gb.is_some())
    {
        eprintln!(
            "note: --log-path/--fs-event-log/--log-level/--fuse-opts/--cache-size \
             have been written to the config file and apply on next daemon \
             start (run `pcloudc stop && pcloudc start`)."
        );
    }

    let socket_path = match socket_path_for_defaults() {
        Ok(p) => p,
        Err(err) => {
            return report_error(
                Some("Login".into()),
                flags.output,
                flags.quiet,
                ExitCode::GenericError,
                &format!("socket resolution failed: {err}"),
            );
        }
    };
    let client = IpcClient;

    // Fast-path: if the daemon already holds a valid authenticated session
    // (e.g. it loaded a vault token at startup), skip all credential
    // prompts and jump straight to the post-login side-effects.
    //
    // This only fires when no explicit username or non-interactive password
    // source was supplied — if the caller passed -u/--username or
    // --password-env they intend a deliberate re-login.
    if opts.username.is_none() && matches!(opts.password_source, PasswordSource::Prompt) {
        let already_authenticated = match client.send(
            &socket_path,
            &Request::Plain {
                method: Method::GetStatus,
            },
        ) {
            Ok(r) => r.message.contains("auth=Authenticated"),
            Err(_) => false,
        };
        if already_authenticated {
            if !flags.quiet {
                println!("Already authenticated — skipping login prompts.");
            }
            // Post-login side-effects: vault opt-in, crypto unlock, mount,
            // userinfo. passascrypto is not applicable here (no account
            // password available), so crypto always prompts separately.
            if save_password {
                if !flags.quiet {
                    eprintln!(
                        "WARNING: --save-password enables the auth-token vault at \
                         ~/.pcloud/config/auth_token (mode 0600).\n\
                         The vault stores the LONG-LIVED PCLOUD TOKEN, not the \
                         password. Anyone with read access to that file (root, \
                         backups, leaked dumps) can use the token to access your \
                         account until you `pcloudc logout`. Hit Ctrl-C in the \
                         next 2s to cancel."
                    );
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
                let req = Request::AuthPersistence { enabled: true };
                match client.send(&socket_path, &req) {
                    Ok(r) if flags.verbosity > 0 => eprintln!("authsave: {}", r.message),
                    Err(e) => eprintln!("authsave failed: {e}"),
                    _ => {}
                }
            }
            if crypto_enabled {
                let crypto_pw = match SecretPrompt::new("Crypto password").read_secret() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("crypto prompt failed: {e}");
                        return ExitCode::Auth;
                    }
                };
                let req = Request::CryptoUnlock {
                    password: crypto_pw.into(),
                };
                match client.send(&socket_path, &req) {
                    Ok(r) if flags.verbosity > 0 => eprintln!("crypto: {}", r.message),
                    Err(e) => eprintln!("crypto failed: {e}"),
                    _ => {}
                }
            }
            if let Some(req_path) = mountpoint_request.as_deref() {
                let target = if req_path.is_empty() {
                    cfg.mountpoint.clone().unwrap_or_else(|| {
                        std::env::var_os("HOME")
                            .map(std::path::PathBuf::from)
                            .unwrap_or_default()
                            .join("pCloudDrive")
                    })
                } else {
                    std::path::PathBuf::from(req_path)
                };
                if let Err(e) = std::fs::create_dir_all(&target) {
                    eprintln!("mountpoint create failed ({}): {e}", target.display());
                    return ExitCode::GenericError;
                }
                #[cfg(unix)]
                let _ = std::fs::set_permissions(
                    &target,
                    std::os::unix::fs::PermissionsExt::from_mode(0o700),
                );
                let req = Request::Mount {
                    path: target.clone(),
                };
                match client.send(&socket_path, &req) {
                    Ok(r) if !flags.quiet => println!("mount: {}", r.message),
                    Err(e) => {
                        eprintln!("mount failed ({}): {e}", target.display());
                        return ExitCode::GenericError;
                    }
                    _ => {}
                }
            }
            let info_req = Request::Plain {
                method: Method::GetUserInfo,
            };
            if let Ok(info) = client.send(&socket_path, &info_req) {
                if !flags.quiet {
                    println!("{}", info.message);
                }
            }
            return ExitCode::Ok;
        }
    }

    // Username: use flag/config if supplied, otherwise prompt.
    let username = match username_pre {
        Some(u) => {
            if !flags.quiet {
                println!("Username: {u}");
            }
            u
        }
        None => match SecretPrompt::new("Username").read_line() {
            Ok(u) => u,
            Err(crate::prompt::PromptError::Eof) => {
                // Ctrl-D at the username prompt: clean exit with
                // auth-cancelled status and a stderr message that
                // scripts can grep for.
                if !flags.quiet {
                    eprintln!("login cancelled (EOF)");
                }
                return ExitCode::Auth;
            }
            Err(e) => {
                return report_error(
                    Some("Login".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::Usage,
                    &format!("username prompt failed: {e}"),
                );
            }
        },
    };
    // Password: prompt (default), stdin, or env — same security surface
    // as `pcloudc submit-password`.
    let password = match &opts.password_source {
        PasswordSource::Prompt => match SecretPrompt::new("Password").read_secret() {
            Ok(p) => SecretString::new(p),
            Err(e) => {
                return report_error(
                    Some("Login".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::Usage,
                    &format!("password prompt failed: {e}"),
                );
            }
        },
        PasswordSource::Stdin => {
            use std::io::BufRead;
            let mut line = String::new();
            if let Err(e) = std::io::stdin().lock().read_line(&mut line) {
                return report_error(
                    Some("Login".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::Usage,
                    &format!("password stdin read failed: {e}"),
                );
            }
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            SecretString::new(line)
        }
        PasswordSource::Env(var) => match std::env::var(var) {
            Ok(v) => {
                // Scrub the env var immediately so /proc/self/environ
                // stops exposing the password as soon as possible.
                //
                // SAFETY (M-8.3): `std::env::remove_var` is inherently
                // not thread-safe (mutating the process environment under
                // concurrent `getenv`/`setenv` is UB per POSIX). The call
                // is safe here because:
                //   1. This code path executes before the Tokio runtime is
                //      started — no async task pool has been created.
                //   2. No rayon or `std::thread::spawn` threads have been
                //      spawned by the CLI before this point.
                //   3. The `prompt.rs` password-read path is also
                //      single-threaded (reads from a tty, no concurrency).
                //
                // If this crate ever adopts an async-first entry point that
                // spawns worker threads before credential resolution, this
                // call must be moved earlier (before any thread is spawned)
                // or replaced with a `OnceLock`-guarded pre-read approach
                // that avoids the mutation entirely.
                #[allow(unsafe_code)]
                // SAFETY: see comment above — single-threaded pre-runtime.
                unsafe {
                    std::env::remove_var(var)
                };
                SecretString::new(v)
            }
            Err(_) => {
                return report_error(
                    Some("Login".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::Usage,
                    &format!("--password-env: variable '{var}' is not set"),
                );
            }
        },
    };

    // Post-success actions: save_password (token vault) → unlock crypto
    // → mount filesystem → print userinfo. Returns the appropriate
    // ExitCode; called from BOTH the 2FA success branch and the
    // no-2FA-needed branch so the same UX runs in both cases.
    let post_login_actions = |password: &SecretString| -> ExitCode {
        // 1. Token vault opt-in.
        if save_password {
            if !flags.quiet {
                eprintln!(
                    "WARNING: --save-password enables the auth-token vault at \
                     ~/.pcloud/config/auth_token (mode 0600).\n\
                     The vault stores the LONG-LIVED PCLOUD TOKEN, not the \
                     password. Anyone with read access to that file (root, \
                     backups, leaked dumps) can use the token to access your \
                     account until you `pcloudc logout`. Hit Ctrl-C in the \
                     next 2s to cancel."
                );
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            // Toggle the durable auth-token vault on. Direct IPC build
            // keeps the wire shape consistent with the `authsave on`
            // command without needing a `SecretInputs` constructor.
            let req = Request::AuthPersistence { enabled: true };
            match client.send(&socket_path, &req) {
                Ok(r) if flags.verbosity > 0 => eprintln!("authsave: {}", r.message),
                Err(e) => eprintln!("authsave failed: {e}"),
                _ => {}
            }
        }

        // 2. Crypto unlock.
        if crypto_enabled {
            // Reuse the account password when --passascrypto was set;
            // otherwise prompt for a separate crypto passphrase via
            // rpassword (no echo).
            let crypto_pw = if passascrypto {
                password.expose_secret().to_owned()
            } else {
                match SecretPrompt::new("Crypto password").read_secret() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("crypto prompt failed: {e}");
                        return ExitCode::Auth;
                    }
                }
            };
            let req = Request::CryptoUnlock {
                password: crypto_pw.into(),
            };
            match client.send(&socket_path, &req) {
                Ok(r) if flags.verbosity > 0 => eprintln!("crypto: {}", r.message),
                Err(e) => eprintln!("crypto failed: {e}"),
                _ => {}
            }
        }

        // 3. Mount.
        if let Some(req_path) = mountpoint_request.as_deref() {
            let target = if req_path.is_empty() {
                cfg.mountpoint.clone().unwrap_or_else(|| {
                    std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_default()
                        .join("pCloudDrive")
                })
            } else {
                std::path::PathBuf::from(req_path)
            };
            // Ensure the directory exists. Mode 0700 because the daemon
            // refuses world-writable mountpoints.
            if let Err(e) = std::fs::create_dir_all(&target) {
                eprintln!("mountpoint create failed ({}): {e}", target.display());
                return ExitCode::GenericError;
            }
            #[cfg(unix)]
            let _ = std::fs::set_permissions(
                &target,
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            );
            let req = Request::Mount {
                path: target.clone(),
            };
            match client.send(&socket_path, &req) {
                Ok(r) if !flags.quiet => println!("mount: {}", r.message),
                Err(e) => {
                    eprintln!("mount failed ({}): {e}", target.display());
                    return ExitCode::GenericError;
                }
                _ => {}
            }
        }

        // 4. userinfo summary.
        let info_req = Request::Plain {
            method: Method::GetUserInfo,
        };
        if let Ok(info) = client.send(&socket_path, &info_req) {
            if !flags.quiet {
                println!("{}", info.message);
            }
        }
        ExitCode::Ok
    };

    // Helper that (re-)submits the password and fires the requested 2FA
    // channel(s). Called once at the start of the flow and again every
    // time the challenge token is invalidated (server-side single-shot
    // policy). `-c sms` fires only SMS, `-c push` fires only the
    // device notification, otherwise we fire both.
    let channel = opts.tfa_channel;
    let submit_password_and_challenges = |password: &SecretString| -> Result<(), ExitCode> {
        let req = Request::PasswordSubmission {
            username: username.clone(),
            value: password.expose_secret().to_owned().into(),
        };
        let response = match client.send(&socket_path, &req) {
            Ok(r) => r,
            Err(e) => {
                let detail = format!("submit-password dispatch failed: {e}");
                let code = ExitCode::classify_transport_error(&detail);
                return Err(report_error(
                    Some("Login".into()),
                    flags.output,
                    flags.quiet,
                    code,
                    &detail,
                ));
            }
        };
        if flags.verbosity > 0 {
            eprintln!("auth: {}", response.message);
        }
        // Only fire SMS / push when a 2FA challenge was actually issued.
        let issued_2fa = response.message.contains("TwoFactorChallengeIssued")
            || response.message.to_ascii_lowercase().contains("2fa")
            || response.message.to_ascii_lowercase().contains("two-factor");
        if issued_2fa {
            let want_sms = matches!(channel, None | Some(TfaChannel::Sms));
            let want_push = matches!(channel, None | Some(TfaChannel::Push));
            if want_sms {
                let sms_req = Request::Plain {
                    method: Method::SendTwoFactorSms,
                };
                if let Ok(resp) = client.send(&socket_path, &sms_req) {
                    if flags.verbosity > 0 {
                        eprintln!("{}", resp.message);
                    }
                }
            }
            if want_push {
                let push_req = Request::Plain {
                    method: Method::SendTwoFactorNotification,
                };
                if let Ok(resp) = client.send(&socket_path, &push_req) {
                    if flags.verbosity > 0 {
                        eprintln!("{}", resp.message);
                    }
                }
            }
        }
        Ok(())
    };

    // Initial password submission + challenge fan-out. We keep `password`
    // alive in a `SecretString` for the duration of the 2FA retry loop so
    // we can re-issue a fresh challenge on wrong-code / expired-token
    // errors without requiring the user to retype it. SecretString
    // zeroises on drop when the function returns.
    if let Err(code) = submit_password_and_challenges(&password) {
        return code;
    }
    // Fetch the current daemon status so the 2FA branch below sees the
    // up-to-date state (the initial send-and-forget response message is
    // already echoed).
    let response = match client.send(
        &socket_path,
        &Request::Plain {
            method: Method::GetStatus,
        },
    ) {
        Ok(r) => r,
        Err(e) => {
            let detail = format!("status after password dispatch failed: {e}");
            let code = ExitCode::classify_transport_error(&detail);
            return report_error(
                Some("Login".into()),
                flags.output,
                flags.quiet,
                code,
                &detail,
            );
        }
    };

    // If the daemon is now in the TwoFactorRequired state, enter the
    // retry-capable 2FA loop. The initial challenge + SMS/push were
    // already dispatched by `submit_password_and_challenges` above.
    if response.message.contains("auth=TwoFactorRequired")
        || response.message.to_ascii_lowercase().contains("2fa")
        || response.message.to_ascii_lowercase().contains("two-factor")
    {
        if !flags.quiet {
            eprintln!(
                "Enter the 2FA code from the SMS or device push (or press Ctrl-D to cancel).\n\
                 Recovery-code alternative: `pcloudc submit-recovery <code>`."
            );
        }
        loop {
            let raw =
                match SecretPrompt::new("2FA code (or 'sms' / 'push' / 'resend' to re-notify)")
                    .read_masked()
                {
                    Ok(s) => s,
                    Err(err) => {
                        // Non-TTY stdin with no data (pipe closed / /dev/tty
                        // unavailable) surfaces as ErrorKind::NotFound or
                        // ENXIO (os error 6). Give the operator an
                        // actionable remediation instead of a bare
                        // "login cancelled".
                        let detail = if is_non_tty_stdin_unavailable(&err) {
                            "2FA code: no value available on stdin.\n\
                             Provide one with --code VALUE, or run interactively on a TTY."
                                .to_owned()
                        } else {
                            "login cancelled".to_owned()
                        };
                        return report_error(
                            Some("Login".into()),
                            flags.output,
                            flags.quiet,
                            ExitCode::Auth,
                            &detail,
                        );
                    }
                };
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                // Empty = Ctrl-D / blank line → cancel.
                return report_error(
                    Some("Login".into()),
                    flags.output,
                    flags.quiet,
                    ExitCode::Auth,
                    "login cancelled",
                );
            }
            // Keyword shortcuts let the user resend without re-running the
            // whole login.
            match trimmed {
                "sms" | "SMS" => {
                    let req = Request::Plain {
                        method: Method::SendTwoFactorSms,
                    };
                    if let Ok(resp) = client.send(&socket_path, &req) {
                        if !flags.quiet {
                            eprintln!("{}", resp.message);
                        }
                    }
                    continue;
                }
                "push" | "PUSH" | "notification" | "notif" => {
                    let req = Request::Plain {
                        method: Method::SendTwoFactorNotification,
                    };
                    if let Ok(resp) = client.send(&socket_path, &req) {
                        if !flags.quiet {
                            eprintln!("{}", resp.message);
                        }
                    }
                    continue;
                }
                "resend" => {
                    // Re-issue the whole challenge (new token, new SMS+push).
                    // Useful when pCloud has invalidated the current token
                    // server-side and neither `sms` nor `push` against the
                    // dead token would help.
                    if let Err(code) = submit_password_and_challenges(&password) {
                        return code;
                    }
                    continue;
                }
                _ => {}
            }
            let code = trimmed.to_owned();
            // Sanity: pCloud 2FA codes are always exactly 6 digits. Anything
            // else is a typo or an accidental keystroke — skip before the
            // round-trip so we don't burn the single-shot challenge token.
            // Recovery codes (different format) go through
            // `pcloudc submit-recovery <code>` instead.
            if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
                if !flags.quiet {
                    eprintln!(
                        "(that doesn't look like a 2FA code; expected exactly 6 digits. \
                         For a recovery code, cancel and use `pcloudc submit-recovery <code>`.)"
                    );
                }
                continue;
            }
            let tfa_req = Request::TwoFactorCodeSubmission {
                value: code,
                trust_device,
                recovery_code: false,
            };
            let resp = match client.send(&socket_path, &tfa_req) {
                Ok(r) => r,
                Err(e) => {
                    let detail = format!("submit-tfa dispatch failed: {e}");
                    let code = ExitCode::classify_transport_error(&detail);
                    return report_error(
                        Some("Login".into()),
                        flags.output,
                        flags.quiet,
                        code,
                        &detail,
                    );
                }
            };
            if flags.verbosity > 0 {
                eprintln!("auth: {}", resp.message);
            }
            if resp.message.contains("LoginSucceeded") {
                return post_login_actions(&password);
            }
            let lower = resp.message.to_ascii_lowercase();
            // pCloud's 2FA challenge token is single-shot: a wrong
            // submit invalidates it server-side, so the user needs a
            // fresh token (and a fresh code) to retry. We preserve the
            // local `pending_challenge` (via `MarkTwoFactorCodeInvalid`)
            // only so the daemon state stays coherent; the SERVER side
            // definitely requires a re-issuance. We simply prompt the
            // user for what to do next rather than auto-bursting the
            // SMS throttle.
            if lower.contains("invalid") || lower.contains("no challenge is pending") {
                if !flags.quiet {
                    eprintln!(
                        "code was rejected — the pCloud challenge token is likely burned.\n\
                         Type 'resend' to request a fresh code, or Ctrl-D to cancel."
                    );
                }
                continue;
            }
            if lower.contains("expired") {
                if !flags.quiet {
                    eprintln!("challenge expired, re-issuing with a fresh SMS + push…");
                }
                if let Err(code) = submit_password_and_challenges(&password) {
                    return code;
                }
                continue;
            }
            // Any other terminal status: surface and stop.
            let code = ExitCode::from_response_status(&resp.status);
            return code;
        }
    }

    // No 2FA needed: if we succeeded, run the same post-login actions.
    if response.message.contains("LoginSucceeded") {
        return post_login_actions(&password);
    }

    ExitCode::from_response_status(&response.status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn completion_unknown_shell_is_usage_error() {
        let code = run(&argv(&["pcloud-rs", "completion", "nu"]));
        assert_eq!(code, ExitCode::Usage);
    }

    #[test]
    fn completion_missing_shell_is_usage_error() {
        let code = run(&argv(&["pcloud-rs", "completion"]));
        assert_eq!(code, ExitCode::Usage);
    }

    #[test]
    fn completion_known_shell_ok() {
        // --quiet keeps test output tidy; the generator still validates the shell.
        let code = run(&argv(&["pcloud-rs", "--quiet", "completion", "bash"]));
        assert_eq!(code, ExitCode::Ok);
    }

    #[test]
    fn help_is_ok() {
        let code = run(&argv(&["pcloud-rs", "--quiet", "--help"]));
        assert_eq!(code, ExitCode::Ok);
    }

    #[test]
    fn quit_command_is_unknown() {
        let code = run(&argv(&["pcloud-rs", "--quiet", "quit"]));
        assert_eq!(code, ExitCode::Usage);
    }

    #[test]
    fn unknown_command_is_usage_error() {
        let code = run(&argv(&["pcloud-rs", "--quiet", "definitely-not-a-command"]));
        assert_eq!(code, ExitCode::Usage);
    }

    #[test]
    fn unknown_output_value_is_usage_error() {
        let code = run(&argv(&["pcloud-rs", "--output", "yaml", "status"]));
        assert_eq!(code, ExitCode::Usage);
    }

    #[test]
    fn version_is_ok() {
        let code = run(&argv(&["pcloud-rs", "--quiet", "--version"]));
        assert_eq!(code, ExitCode::Ok);
    }

    #[test]
    fn unknown_global_flag_is_usage_error() {
        // P0 regression: `pcloudc --badflag` used to silently run
        // `status` with exit 0. It must now reject the unknown flag
        // with ExitCode::Usage before even parsing the subcommand.
        let code = run(&argv(&["pcloud-rs", "--badflag"]));
        assert_eq!(code, ExitCode::Usage);
    }

    #[test]
    fn zero_args_prints_hint_and_exits_zero() {
        // Bare `pcloud-rs` (no subcommand) must print a friendly hint
        // rather than silently defaulting to `status`. We assert the
        // exit code here; `--quiet` suppresses stdout so the test
        // harness stays tidy.
        let code = run(&argv(&["pcloud-rs", "--quiet"]));
        assert_eq!(code, ExitCode::Ok);
        // Sanity on the hint text itself so a refactor doesn't silently
        // regress the friendly message.
        let hint = zero_arg_hint();
        assert!(hint.contains("pcloud is idle"), "hint: {hint}");
        assert!(hint.contains("pcloudc status"), "hint: {hint}");
        assert!(hint.contains("pcloudc --help"), "hint: {hint}");
    }

    #[test]
    fn two_factor_non_tty_error_is_actionable() {
        // The 2FA loop sits behind a live daemon dispatch, so we test
        // the classifier that decides whether to print the actionable
        // remediation text. This mirrors what `read_masked()` surfaces
        // when stdin is a closed pipe (no /dev/tty available): either
        // ErrorKind::NotFound, UnexpectedEof, or raw ENXIO (os error 6).
        use crate::prompt::PromptError;
        use std::io::{self, ErrorKind};

        let not_found = PromptError::Io(io::Error::from(ErrorKind::NotFound));
        assert!(
            is_non_tty_stdin_unavailable(&not_found),
            "NotFound must map to actionable error"
        );

        let enxio = PromptError::Io(io::Error::from_raw_os_error(6));
        assert!(
            is_non_tty_stdin_unavailable(&enxio),
            "ENXIO (os error 6) must map to actionable error"
        );

        let unexpected_eof = PromptError::Io(io::Error::from(ErrorKind::UnexpectedEof));
        assert!(
            is_non_tty_stdin_unavailable(&unexpected_eof),
            "UnexpectedEof must map to actionable error"
        );

        // Interactive Ctrl-D on a TTY should NOT be reclassified as
        // non-TTY stdin unavailability — it's a clean cancel.
        assert!(!is_non_tty_stdin_unavailable(&PromptError::Eof));

        // An unrelated IO error (e.g. permission denied) should also
        // not be reclassified.
        let perm = PromptError::Io(io::Error::from(ErrorKind::PermissionDenied));
        assert!(!is_non_tty_stdin_unavailable(&perm));
    }

    #[test]
    fn version_banner_includes_name_and_version() {
        let b = version_banner();
        assert!(b.starts_with(completion::BIN_NAME), "banner: {b}");
        assert!(b.contains(env!("CARGO_PKG_VERSION")), "banner: {b}");
        // Git hash / profile are either real values or "unknown"; we
        // only assert that both slots are present (two comma-separated
        // tokens inside the parenthesised suffix).
        assert!(b.contains('(') && b.contains(')'), "banner: {b}");
        assert!(b.contains(','), "banner: {b}");
    }
}

//! Global CLI flags that apply to every subcommand.
//!
//! This module is intentionally decoupled from the legacy token parser in
//! [`crate::app`] so that we can introduce `--json`, `--quiet`, `-v/-vv/-vvv`
//! without disturbing the existing positional-argument layout or any of the
//! legacy alias behaviors.
//!
//! The flow is:
//!
//! 1. [`GlobalFlags::extract`] scans `argv`, pulls out recognized global
//!    flags, and returns the *reduced* argv plus the parsed flag state.
//! 2. The reduced argv is handed to the existing `app::parse_command`
//!    pipeline, which has never seen (and does not need to see) these flags.
//!
//! Security notes:
//! - `--json` output goes through [`crate::json_output`], which hard-codes the
//!   set of whitelisted response fields (`status`, `message`). Secret-bearing
//!   request fields are NEVER serialized.
//! - `--quiet` only suppresses stdout. Failure exit codes are preserved.
//! - Verbosity maps to a u8 tracing level hint the caller can wire into a
//!   subscriber if desired. We do not initialize a global logger here to
//!   avoid touching unrelated crates.

// **PLATFORM:** all
// **GATING:** none (portable).

/// Canonical output format selected by `--output` / `--json`.
///
/// The set is intentionally tiny — `text` for interactive humans, `json`
/// for machines — so scripts never have to sniff which renderer was used.
/// Adding a third variant (e.g. `yaml`, `tsv`) would be a **breaking
/// change** because `--json` currently promises a documented envelope in
/// [`crate::json_output`]; new formats must come with their own stability
/// note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Human-readable, free-form text output (default). Stable for
    /// readability but **not** parse-stable; scripts should use `json`.
    #[default]
    Text,
    /// Machine-readable JSON envelope. The schema is documented in
    /// [`crate::json_output`] and is part of the crate's semver surface.
    Json,
}

/// Parsed global flags extracted from `argv` before the legacy positional
/// parser runs.
///
/// # Stable-ABI guarantee
///
/// Field names and semantics are part of the `pcloudc` UX contract and
/// evolve under semver:
///
/// - the **field set** may grow (new knobs such as `--no-color`) in
///   patch/minor releases; existing fields never change meaning,
/// - the **parsed flag names** (see [`known_flag_names`]) are likewise
///   additive,
/// - removing or renaming a documented flag requires a major release
///   with a migration note in the changelog.
///
/// Consumers that embed `pcloudc` as a subprocess can therefore pin on
/// the current flag names and expect future releases to accept them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlobalFlags {
    /// Selected output format (`--output text|json` / `--json`).
    pub output: OutputFormat,
    /// `--quiet` / `-q`: suppress stdout. Exit codes are preserved so
    /// scripts can still detect failure.
    pub quiet: bool,
    /// 0 = default, 1 = -v (info), 2 = -vv (debug), 3+ = -vvv (trace).
    pub verbosity: u8,
    /// User asked for `--help` / `-h` at the global level with no subcommand,
    /// or together with a subcommand. The legacy `help` subcommand still
    /// works and continues to go through the normal dispatch path.
    pub help: bool,
    /// User asked for `--version` / `-V`.
    pub version: bool,
    /// Resolved W3C `traceparent` string for this invocation, if any.
    ///
    /// Precedence (first match wins):
    ///
    /// 1. `--trace-id <HEX>` — a 32-hex W3C trace id on the command
    ///    line. The CLI synthesises a fresh 16-byte span id and the
    ///    `01` (sampled) flags byte to form a full
    ///    `00-<trace>-<span>-01` traceparent.
    /// 2. `TRACEPARENT` envvar — when set and well-formed
    ///    (`00-<32hex>-<16hex>-<2hex>`), it is adopted verbatim.
    ///    Malformed values are silently dropped.
    ///
    /// When neither is present this stays `None` and the CLI sends the
    /// bare request over IPC. No auto-generation happens for untraced
    /// invocations — that decision is owned by the daemon.
    pub traceparent: Option<String>,
    /// Built-in dotted-path field selectors requested via `--field` /
    /// `-f` / `--select`, in the order the user supplied them.
    ///
    /// Empty (default) means "render the full response" — the
    /// historical behaviour. When non-empty, the CLI's `run` entry
    /// point parses the response `message` into JSON and projects
    /// each listed path, failing with exit `2 Usage` if any path is
    /// not present in the response.
    ///
    /// See [`crate::field_selector`] for the accepted syntax.
    pub fields: Vec<String>,
}

/// Errors produced by [`GlobalFlags::extract`].
///
/// All variants are `Display`-safe and never quote back a user-supplied
/// value of an unknown flag; see [`GlobalFlagError::UnknownFlag`] for the
/// secret-redaction discipline. Exit-code mapping: [`GlobalFlagError`]
/// always maps to [`crate::exit_code::ExitCode::Usage`].
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GlobalFlagError {
    /// Value supplied to `--output` was not one of the accepted tokens
    /// (`text`, `human`, `json`).
    #[error("unknown value for --output: '{0}' (expected 'text' or 'json')")]
    UnknownOutputFormat(String),
    /// `--output` was the last token on the command line with no value
    /// following it.
    #[error("missing value for --output")]
    MissingOutputValue,
    /// A `--something` / `-x` token was not recognized by any layer of
    /// the parser (global flag, login flag, subcommand flag).
    ///
    /// P0.9 unknown-flag rejection: we reject rather than silently drop
    /// so a typo like `pcloudc --qiet status` surfaces as a usage
    /// error instead of running `status` with default options. The
    /// reported `String` is the **flag name only** (everything before
    /// `=`) so a typo like `--badtoken=sekret` never leaks the value
    /// into error output / logs.
    #[error("unknown flag '{0}'. Run `pcloudc --help`.")]
    UnknownFlag(String),
    /// `--trace-id` was given with a malformed value (not a 32-character
    /// lowercase hex string per the W3C Trace Context
    /// [trace-id](https://www.w3.org/TR/trace-context/#trace-id) grammar).
    /// The reported value is intentionally omitted so nothing sensitive
    /// leaks into error output.
    #[error("invalid --trace-id value (expected 32 lowercase hex characters)")]
    InvalidTraceId,
    /// `--trace-id` was the last token on the command line with no value
    /// following it.
    #[error("missing value for --trace-id")]
    MissingTraceIdValue,
    /// `--field` / `-f` / `--select` was the last token on the command
    /// line with no value following it.
    #[error("missing value for --field")]
    MissingFieldValue,
}

/// Single source of truth for every `--flag` / `-x` token the CLI accepts.
///
/// Extended beyond [`GlobalFlags::extract`]'s own set with the login,
/// password-source, per-subcommand (`publink send`), and config/log
/// knobs handled in `main.rs` / `app.rs`. Keeping the list in one place
/// means adding a new flag is a one-line change and the
/// `unknown flag '--foo'` rejector in [`GlobalFlags::extract`] stays
/// accurate.
#[must_use]
pub fn known_flag_names() -> &'static [&'static str] {
    &[
        // --- Global (see GlobalFlags::extract) ---
        "--json",
        "--output",
        "--quiet",
        "-q",
        "--verbose",
        "--dbg",
        "--debug",
        "--help",
        "-h",
        "--version",
        "-V",
        "--trace-id",
        "--field",
        "-f",
        "--select",
        // --- Login / auth flags (main.rs::LoginOptions::from_argv) ---
        "--user",
        "-u",
        "--username",
        "--tfa-channel",
        "-T",
        "--channel",
        "--password-stdin",
        "--password-env",
        "--crypto",
        "-c",
        "--passascrypto",
        "-y",
        "--pass-as-crypto",
        "--trust-device",
        "-r",
        "--trusted-device",
        "--save-password",
        "-s",
        "--mountpoint",
        "-m",
        "--fuse-opts",
        "-O",
        "--log-path",
        "--fs-event-log",
        "--log-level",
        "--cache-size",
        "--config",
        // --- Per-subcommand flags parsed in app.rs ---
        "--to",
        "--message",
        "--from",
        // `verify <path> [--recursive] [--fix] [--yes]` — R9 #12.
        "--recursive",
        "--fix",
        "--yes",
        // `backup snapshot-{create,restore,verify,prune}` — H12 PR1.
        "--gpg-recipient",
        "--retention-days",
        // `account register <EMAIL> [--accept-terms]`.
        "--accept-terms",
        // `submit-password` / `submit-auth <TOKEN>` — argv-secret gate.
        "--allow-argv-password",
        // `file history --limit <N>`.
        "--limit",
        // `sync suggest --max <N>`.
        "--max",
        // `sync add --type <FLAVOR>`.
        "--type",
        // `crypto setup --backend <name> [--acknowledge-not-interop]
        // [--hint <TEXT>]` (CryptoSetupV2).
        "--backend",
        "--acknowledge-not-interop",
        "--hint",
    ]
}

/// `true` when `token` (without any trailing `=value`) is a recognized
/// flag. `-vvv` style stacks are handled separately by the caller.
fn is_known_flag(token: &str) -> bool {
    // Split off `--foo=bar` → `--foo`.
    let name = token.split_once('=').map_or(token, |(k, _)| k);
    known_flag_names().contains(&name)
}

/// `true` for `-v`, `-vv`, `-vvv`, ... (any all-`v` short stack).
fn is_verbosity_stack(token: &str) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() >= 2
        && token.chars().skip(1).all(|c| c == 'v')
}

impl GlobalFlags {
    /// Extract recognized global flags from `argv`, returning the reduced
    /// argv (suitable for the legacy parser) and the parsed flag state.
    ///
    /// Recognized flags:
    /// - `--json`                       shorthand for `--output json`
    /// - `--output <text|json>`         set output format
    /// - `--output=<text|json>`         set output format (equals form)
    /// - `-q`, `--quiet`                suppress stdout
    /// - `-v`, `-vv`, `-vvv`            stacking verbosity (up to 3)
    /// - `--verbose`                    same as `-v`, stacks with `-v` flags
    /// - `--help`, `-h`                 set help flag (legacy `help` still works)
    /// - `--version`, `-V`              set version flag
    ///
    /// Unknown `--` flags are left in place for the legacy parser to reject,
    /// preserving backward-compatible error messages.
    pub fn extract(argv: &[String]) -> Result<(Self, Vec<String>), GlobalFlagError> {
        let mut flags = Self::default();
        let mut out: Vec<String> = Vec::with_capacity(argv.len());

        if argv.is_empty() {
            return Ok((flags, out));
        }
        out.push(argv[0].clone());

        let mut i = 1;
        while i < argv.len() {
            let a = argv[i].as_str();
            match a {
                "--json" => {
                    flags.output = OutputFormat::Json;
                }
                "--output" => {
                    let Some(val) = argv.get(i + 1) else {
                        return Err(GlobalFlagError::MissingOutputValue);
                    };
                    flags.output = parse_output_value(val)?;
                    i += 1;
                }
                s if s.starts_with("--output=") => {
                    let val = &s["--output=".len()..];
                    flags.output = parse_output_value(val)?;
                }
                "--quiet" | "-q" => {
                    flags.quiet = true;
                }
                "--verbose" => {
                    flags.verbosity = flags.verbosity.saturating_add(1);
                }
                "--dbg" | "--debug" => {
                    // Convenience alias: jump to maximum verbosity in
                    // one flag rather than `-vvv`. Mirrors the
                    // common dev-tool convention (cargo --verbose vs
                    // RUST_LOG=debug).
                    flags.verbosity = 3;
                }
                "--help" | "-h" => {
                    flags.help = true;
                }
                "--version" | "-V" => {
                    flags.version = true;
                }
                "--trace-id" => {
                    let Some(val) = argv.get(i + 1) else {
                        return Err(GlobalFlagError::MissingTraceIdValue);
                    };
                    flags.traceparent = Some(traceparent_from_trace_id(val)?);
                    i += 1;
                }
                s if s.starts_with("--trace-id=") => {
                    let val = &s["--trace-id=".len()..];
                    flags.traceparent = Some(traceparent_from_trace_id(val)?);
                }
                "--field" | "-f" | "--select" => {
                    let Some(val) = argv.get(i + 1) else {
                        return Err(GlobalFlagError::MissingFieldValue);
                    };
                    flags.fields.push(val.clone());
                    i += 1;
                }
                s if s.starts_with("--field=") => {
                    flags.fields.push(s["--field=".len()..].to_owned());
                }
                s if s.starts_with("--select=") => {
                    flags.fields.push(s["--select=".len()..].to_owned());
                }
                s if s.starts_with("-f=") => {
                    flags.fields.push(s["-f=".len()..].to_owned());
                }
                s if s.starts_with('-') && s.len() >= 2 && s.chars().skip(1).all(|c| c == 'v') => {
                    // -v, -vv, -vvv, ...  (cap at 3 for tracing level mapping)
                    let count = (s.len() - 1) as u8;
                    flags.verbosity = flags.verbosity.saturating_add(count);
                }
                _ => {
                    // Reject any unknown `--flag` / `-flag` token before
                    // the legacy parser silently drops it. Positionals
                    // (e.g. `publink send alpha123`, paths, ids) never
                    // start with `-` and pass through untouched.
                    if a.starts_with('-') && a != "-" {
                        // `-vv`/`-vvv` stacks aren't in the allow-list
                        // but are handled by a dedicated arm above;
                        // this is a second line of defence if a future
                        // refactor reorders match arms.
                        if is_verbosity_stack(a) {
                            let count = (a.len() - 1) as u8;
                            flags.verbosity = flags.verbosity.saturating_add(count);
                        } else if is_known_flag(a) {
                            // Recognised by the wider CLI (login /
                            // subcommand flag). Forward to the legacy
                            // parser untouched.
                            out.push(a.to_owned());
                        } else {
                            // Report the flag *name* (without any
                            // `=value`) so passwords / tokens / paths
                            // never leak into error output.
                            let reported = a.split_once('=').map_or(a, |(k, _)| k).to_owned();
                            return Err(GlobalFlagError::UnknownFlag(reported));
                        }
                    } else {
                        out.push(a.to_owned());
                    }
                }
            }
            i += 1;
        }

        if flags.verbosity > 3 {
            flags.verbosity = 3;
        }

        // If `--trace-id` was not provided, fall back to the
        // `TRACEPARENT` environment variable. Malformed values are
        // dropped silently per the documented contract (see
        // [`GlobalFlags::traceparent`]).
        if flags.traceparent.is_none() {
            if let Ok(raw) = std::env::var("TRACEPARENT") {
                if is_well_formed_traceparent(&raw) {
                    flags.traceparent = Some(raw);
                }
            }
        }

        Ok((flags, out))
    }

    /// Human label for the tracing level implied by `verbosity`.
    #[must_use]
    pub fn tracing_level(&self) -> &'static str {
        match self.verbosity {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }
    }
}

fn parse_output_value(v: &str) -> Result<OutputFormat, GlobalFlagError> {
    match v {
        "text" | "human" => Ok(OutputFormat::Text),
        "json" => Ok(OutputFormat::Json),
        other => Err(GlobalFlagError::UnknownOutputFormat(other.to_owned())),
    }
}

/// Accept a 32-character lowercase hexadecimal W3C trace id and return
/// a canonical `traceparent` line of the form
/// `00-<trace-id>-<span-id>-01`. A fresh random 16-byte span id is
/// synthesised and the invocation is marked as explicitly sampled
/// (`01`) — the explicit `--trace-id` flag implicitly force-samples per
/// the manpage OBSERVABILITY / DISTRIBUTED TRACING section.
fn traceparent_from_trace_id(raw: &str) -> Result<String, GlobalFlagError> {
    if !is_valid_trace_id(raw) {
        return Err(GlobalFlagError::InvalidTraceId);
    }
    let mut span_bytes = [0u8; 8];
    // Best-effort non-cryptographic span id. Pull from two fast sources
    // (nanosecond clock + ASLR-ish pointer) and mix them. Span ids are
    // opaque correlation tokens, not secrets; a predictable value is
    // harmless in a trace backend, but a zero value is not legal in
    // W3C Trace Context. We therefore guarantee the result is non-zero.
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let stack_marker = &span_bytes as *const _ as usize as u64;
    let mixed = now_ns ^ stack_marker.rotate_left(17) ^ 0x9E37_79B9_7F4A_7C15_u64;
    let mixed = if mixed == 0 { 1 } else { mixed };
    span_bytes.copy_from_slice(&mixed.to_be_bytes());
    let span_hex: String = span_bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("00-{raw}-{span_hex}-01"))
}

/// `true` iff `s` is a 32-character lowercase hexadecimal string (the
/// shape of a W3C Trace Context `trace-id` field).
fn is_valid_trace_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// `true` iff `s` matches the canonical W3C Trace Context `traceparent`
/// header shape `00-<trace:32hex>-<span:16hex>-<flags:2hex>`. The flags
/// byte may carry any value; the trace id and span id must be non-zero.
fn is_well_formed_traceparent(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 4 {
        return false;
    }
    let [ver, trace, span, flags] = [parts[0], parts[1], parts[2], parts[3]];
    if ver != "00" {
        return false;
    }
    if !(trace.len() == 32 && trace.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))) {
        return false;
    }
    if !(span.len() == 16 && span.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))) {
        return false;
    }
    if !(flags.len() == 2 && flags.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))) {
        return false;
    }
    // Trace and span must be non-zero per W3C spec.
    if trace.chars().all(|c| c == '0') || span.chars().all(|c| c == '0') {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn empty_argv_roundtrip() {
        let (f, out) = GlobalFlags::extract(&[]).unwrap();
        assert_eq!(f, GlobalFlags::default());
        assert!(out.is_empty());
    }

    #[test]
    fn no_flags_preserves_argv() {
        let argv = s(&["pcloud-rs", "status"]);
        let (f, out) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(f, GlobalFlags::default());
        assert_eq!(out, argv);
    }

    #[test]
    fn json_short_flag() {
        let argv = s(&["pcloud-rs", "--json", "status"]);
        let (f, out) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(f.output, OutputFormat::Json);
        assert_eq!(out, s(&["pcloud-rs", "status"]));
    }

    #[test]
    fn output_spaced_and_equals() {
        let a1 = s(&["pcloud-rs", "--output", "json", "status"]);
        let a2 = s(&["pcloud-rs", "--output=json", "status"]);
        assert_eq!(
            GlobalFlags::extract(&a1).unwrap().0.output,
            OutputFormat::Json
        );
        assert_eq!(
            GlobalFlags::extract(&a2).unwrap().0.output,
            OutputFormat::Json
        );
    }

    #[test]
    fn output_text_explicit() {
        let argv = s(&["pcloud-rs", "--output", "text", "status"]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(f.output, OutputFormat::Text);
    }

    #[test]
    fn output_unknown_value() {
        let argv = s(&["pcloud-rs", "--output", "yaml"]);
        let err = GlobalFlags::extract(&argv).unwrap_err();
        assert!(matches!(err, GlobalFlagError::UnknownOutputFormat(_)));
    }

    #[test]
    fn output_missing_value() {
        let argv = s(&["pcloud-rs", "--output"]);
        let err = GlobalFlags::extract(&argv).unwrap_err();
        assert_eq!(err, GlobalFlagError::MissingOutputValue);
    }

    #[test]
    fn quiet_flag() {
        let argv = s(&["pcloud-rs", "-q", "status"]);
        let (f, out) = GlobalFlags::extract(&argv).unwrap();
        assert!(f.quiet);
        assert_eq!(out, s(&["pcloud-rs", "status"]));
    }

    #[test]
    fn verbose_stacking() {
        let cases: [(&[&str], u8, &str); 5] = [
            (&["pcloud-rs", "status"], 0, "warn"),
            (&["pcloud-rs", "-v", "status"], 1, "info"),
            (&["pcloud-rs", "-vv", "status"], 2, "debug"),
            (&["pcloud-rs", "-vvv", "status"], 3, "trace"),
            (&["pcloud-rs", "-v", "-v", "status"], 2, "debug"),
        ];
        for (argv, expected, level) in cases {
            let v: Vec<String> = argv.iter().map(|x| (*x).to_owned()).collect();
            let (f, _) = GlobalFlags::extract(&v).unwrap();
            assert_eq!(f.verbosity, expected, "argv={argv:?}");
            assert_eq!(f.tracing_level(), level, "argv={argv:?}");
        }
    }

    #[test]
    fn verbose_saturates_at_three() {
        let argv = s(&["pcloud-rs", "-vvvvvvvv", "status"]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(f.verbosity, 3);
        assert_eq!(f.tracing_level(), "trace");
    }

    #[test]
    fn help_and_version_flags() {
        let (f, _) = GlobalFlags::extract(&s(&["pcloud-rs", "--help"])).unwrap();
        assert!(f.help);
        let (f, _) = GlobalFlags::extract(&s(&["pcloud-rs", "-V"])).unwrap();
        assert!(f.version);
    }

    #[test]
    fn unknown_dashdash_flag_is_usage_error() {
        // P0: `pcloudc --badflag` must not silently run the default
        // `status` with exit 0 — it has to fail loudly with a usage
        // error so scripts/CI see the problem.
        let argv = s(&["pcloud-rs", "--nope", "status"]);
        let err = GlobalFlags::extract(&argv).unwrap_err();
        match err {
            GlobalFlagError::UnknownFlag(name) => assert_eq!(name, "--nope"),
            other => panic!("expected UnknownFlag, got {other:?}"),
        }
    }

    #[test]
    fn unknown_flag_with_value_redacts_value() {
        // Protect against leaking secrets via `--badtoken=sekret` typos.
        let argv = s(&["pcloud-rs", "--badtoken=sekret"]);
        let err = GlobalFlags::extract(&argv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--badtoken"), "msg={msg}");
        assert!(!msg.contains("sekret"), "secret leaked into error: {msg}");
    }

    #[test]
    fn known_login_flags_pass_through() {
        let argv = s(&["pcloud-rs", "login", "--user", "alice"]);
        let (_, out) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(out, argv);
    }

    #[test]
    fn positional_dash_only_token_passes_through() {
        // A bare `-` means stdin in many tools; leave it alone.
        let argv = s(&["pcloud-rs", "status", "-"]);
        let (_, out) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(out, argv);
    }

    #[test]
    fn publink_send_positional_preserved() {
        // Regression guard: the short-code positional arg for
        // `publink send` must not be mistaken for a flag.
        let argv = s(&["pcloud-rs", "publink", "send", "alpha123", "--to", "a@b.c"]);
        let (_, out) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(out, argv);
    }

    #[test]
    fn flags_do_not_reorder_positional() {
        let argv = s(&["pcloud-rs", "--json", "sync", "add", "/a", "/b"]);
        let (f, out) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(f.output, OutputFormat::Json);
        assert_eq!(out, s(&["pcloud-rs", "sync", "add", "/a", "/b"]));
    }

    /// Shared test guard to serialize tests that mutate the
    /// `TRACEPARENT` environment variable. `std::env::{set_var,
    /// remove_var}` are not thread-safe, and `cargo test` runs tests
    /// in parallel by default.
    fn trace_env_guard() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn trace_id_flag_produces_canonical_traceparent() {
        let _g = trace_env_guard();
        // SAFETY: tests that mutate TRACEPARENT serialize via
        // `trace_env_guard`; no concurrent readers inside the test
        // process.
        unsafe { std::env::remove_var("TRACEPARENT") };
        let argv = s(&[
            "pcloud-rs",
            "--trace-id",
            "4bf92f3577b34da6a3ce929d0e0e4736",
            "status",
        ]);
        let (f, out) = GlobalFlags::extract(&argv).unwrap();
        let tp = f.traceparent.expect("traceparent should be present");
        assert!(tp.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
        assert!(tp.ends_with("-01"));
        assert_eq!(tp.len(), 55); // 2+1+32+1+16+1+2
        assert_eq!(out, s(&["pcloud-rs", "status"]));
    }

    #[test]
    fn trace_id_flag_equals_form() {
        let _g = trace_env_guard();
        // SAFETY: serialized via `trace_env_guard`; no concurrent env readers.
        unsafe { std::env::remove_var("TRACEPARENT") };
        let argv = s(&[
            "pcloud-rs",
            "--trace-id=4bf92f3577b34da6a3ce929d0e0e4736",
            "status",
        ]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        assert!(f.traceparent.is_some());
    }

    #[test]
    fn trace_id_rejects_bad_hex() {
        let _g = trace_env_guard();
        // SAFETY: serialized via `trace_env_guard`; no concurrent env readers.
        unsafe { std::env::remove_var("TRACEPARENT") };
        let argv = s(&["pcloud-rs", "--trace-id", "not-hex", "status"]);
        let err = GlobalFlags::extract(&argv).unwrap_err();
        assert!(matches!(err, GlobalFlagError::InvalidTraceId));
    }

    #[test]
    fn trace_id_missing_value_errors() {
        let _g = trace_env_guard();
        // SAFETY: serialized via `trace_env_guard`; no concurrent env readers.
        unsafe { std::env::remove_var("TRACEPARENT") };
        let argv = s(&["pcloud-rs", "--trace-id"]);
        let err = GlobalFlags::extract(&argv).unwrap_err();
        assert_eq!(err, GlobalFlagError::MissingTraceIdValue);
    }

    #[test]
    fn traceparent_envvar_adopted_when_flag_absent() {
        let _g = trace_env_guard();
        // SAFETY: serialized via `trace_env_guard`; no concurrent env readers.
        unsafe {
            std::env::set_var(
                "TRACEPARENT",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            );
        }
        let argv = s(&["pcloud-rs", "status"]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(
            f.traceparent.as_deref(),
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        );
        // SAFETY: cleanup; still under the guard.
        unsafe { std::env::remove_var("TRACEPARENT") };
    }

    #[test]
    fn trace_id_flag_beats_envvar() {
        let _g = trace_env_guard();
        // SAFETY: serialized via `trace_env_guard`; no concurrent env readers.
        unsafe {
            std::env::set_var(
                "TRACEPARENT",
                "00-11111111111111111111111111111111-2222222222222222-01",
            );
        }
        let argv = s(&[
            "pcloud-rs",
            "--trace-id",
            "4bf92f3577b34da6a3ce929d0e0e4736",
            "status",
        ]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        let tp = f.traceparent.unwrap();
        assert!(
            tp.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"),
            "flag should win, got {tp}"
        );
        // SAFETY: cleanup; still under the guard.
        unsafe { std::env::remove_var("TRACEPARENT") };
    }

    #[test]
    fn traceparent_envvar_malformed_dropped() {
        let _g = trace_env_guard();
        // SAFETY: serialized via `trace_env_guard`; no concurrent env readers.
        unsafe { std::env::set_var("TRACEPARENT", "not-a-traceparent") };
        let argv = s(&["pcloud-rs", "status"]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        assert!(f.traceparent.is_none());
        // SAFETY: cleanup; still under the guard.
        unsafe { std::env::remove_var("TRACEPARENT") };
    }

    #[test]
    fn no_trace_context_means_none() {
        let _g = trace_env_guard();
        // SAFETY: serialized via `trace_env_guard`; no concurrent env readers.
        unsafe { std::env::remove_var("TRACEPARENT") };
        let argv = s(&["pcloud-rs", "status"]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        assert!(f.traceparent.is_none());
    }

    #[test]
    fn field_flag_collects_paths_in_order() {
        let argv = s(&[
            "pcloud-rs",
            "--field",
            "quota",
            "-f",
            "usedquota",
            "--select",
            "premium",
            "userinfo",
        ]);
        let (f, out) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(f.fields, vec!["quota", "usedquota", "premium"]);
        assert_eq!(out, s(&["pcloud-rs", "userinfo"]));
    }

    #[test]
    fn field_flag_equals_form() {
        let argv = s(&["pcloud-rs", "--field=quota", "--select=premium", "userinfo"]);
        let (f, _) = GlobalFlags::extract(&argv).unwrap();
        assert_eq!(f.fields, vec!["quota", "premium"]);
    }

    #[test]
    fn field_flag_missing_value_errors() {
        let argv = s(&["pcloud-rs", "--field"]);
        let err = GlobalFlags::extract(&argv).unwrap_err();
        assert_eq!(err, GlobalFlagError::MissingFieldValue);
    }

    #[test]
    fn traceparent_well_formed_helper() {
        assert!(is_well_formed_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        ));
        // Non-zero-span but non-zero-trace -> ok.
        assert!(is_well_formed_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000001-00"
        ));
        // Zero trace id is invalid.
        assert!(!is_well_formed_traceparent(
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01"
        ));
        // Wrong version.
        assert!(!is_well_formed_traceparent(
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        ));
        // Too many parts.
        assert!(!is_well_formed_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra"
        ));
        // Uppercase hex rejected (spec requires lowercase).
        assert!(!is_well_formed_traceparent(
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00F067AA0BA902B7-01"
        ));
    }
}

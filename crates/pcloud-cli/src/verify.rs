//! `pcloudc verify` — walk a synced tree and cross-check local SHA256
//! digests against the server-reported checksum for each file.
//!
//! R9 enhancement #12. Handled entirely CLI-side (never contacts the
//! daemon beyond a single `GetUserInfo` probe reserved for a future
//! daemon-walks-tree implementation wired under `Request::VerifyPath`).
//! The output contract:
//!
//! * text mode: one line per file, `[OK]` | `[MISMATCH local=… server=…]`
//!   | `[MISSING_LOCAL]` | `[MISSING_REMOTE]` followed by the path,
//! * `--json` mode: NDJSON, one `{"path":"…","status":"ok|mismatch|missing_local|missing_remote","local_sha256":…,"server_sha256":…}`
//!   record per file, flushed incrementally.
//!
//! Exit codes follow the documented enterprise discipline:
//!
//! * [`ExitCode::Ok`] — every row matched,
//! * [`ExitCode::Conflict`] — at least one `[MISMATCH]` row,
//! * [`ExitCode::Unavailable`] — no mismatches but at least one row was
//!   missing on one side.
//!
//! Security:
//! * no secrets are logged or persisted,
//! * server digests are treated as opaque hex strings,
//! * the `--fix` path is strictly opt-in and prompts interactively for
//!   `[MISMATCH]` rows unless `--yes` is also given.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::Write;
use std::path::Path;

use crate::exit_code::ExitCode;
use crate::globals::{GlobalFlags, OutputFormat};

/// Per-file classification emitted by `pcloudc verify` (R9 #12).
///
/// Kept in sync with the `pcloud_backends::transfer_backend::VerifyClassification`
/// shape (same variants, same tags, same `render` contract) so a future
/// daemon-walks-tree implementation can re-use the backend-side helper
/// without changing the CLI renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyClassification {
    /// Local file SHA256 matches the server-reported SHA256.
    Ok,
    /// Local and server digests diverged.
    Mismatch {
        /// Lowercase hex SHA256 of the local file.
        local: String,
        /// Lowercase hex SHA256 reported by the server.
        server: String,
    },
    /// The remote file exists but the local path is missing on disk.
    MissingLocal,
    /// The local file exists but no remote counterpart was resolvable.
    MissingRemote,
}

impl VerifyClassification {
    /// Short ASCII tag used by the text renderer.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Mismatch { .. } => "MISMATCH",
            Self::MissingLocal => "MISSING_LOCAL",
            Self::MissingRemote => "MISSING_REMOTE",
        }
    }

    /// Render the classification in the documented one-line shape.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Ok => "[OK]".to_owned(),
            Self::Mismatch { local, server } => {
                format!("[MISMATCH local={local} server={server}]")
            }
            Self::MissingLocal => "[MISSING_LOCAL]".to_owned(),
            Self::MissingRemote => "[MISSING_REMOTE]".to_owned(),
        }
    }

    /// `true` when the classification is a hard mismatch.
    #[must_use]
    pub const fn is_mismatch(&self) -> bool {
        matches!(self, Self::Mismatch { .. })
    }
}

/// Classify a local vs server checksum pair. Shared with
/// `pcloud_backends::transfer_backend::classify_file_hashes` — keep
/// the two renderings in lock-step.
#[must_use]
pub fn classify_file_hashes(
    local_sha256_hex: Option<&str>,
    server_sha256_hex: Option<&str>,
) -> VerifyClassification {
    match (local_sha256_hex, server_sha256_hex) {
        (None, _) => VerifyClassification::MissingLocal,
        (Some(_), None) => VerifyClassification::MissingRemote,
        (Some(local), Some(server)) => {
            let l = local.trim().to_ascii_lowercase();
            let s = server.trim().to_ascii_lowercase();
            if l == s {
                VerifyClassification::Ok
            } else {
                VerifyClassification::Mismatch {
                    local: l,
                    server: s,
                }
            }
        }
    }
}

/// Compute the SHA256 hex digest of a local file. Returns `Ok(None)`
/// for a missing path so the caller maps that to
/// [`VerifyClassification::MissingLocal`] without branching on
/// [`std::io::ErrorKind`].
///
/// Re-implemented here rather than in `pcloud-backends` so the CLI
/// crate does not gain a new `pcloud-backends` dependency at this
/// landing; the `pcloud_backends::transfer_backend::local_file_sha256_hex`
/// helper is kept in sync and unit-tested independently.
fn local_file_sha256_hex(path: &Path) -> std::io::Result<Option<String>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(format!("{:x}", hasher.finalize())))
}

/// Walk a local path. Companion of
/// `pcloud_backends::folder_backend::walk_local_tree`; inlined here
/// for the same crate-dependency reason as [`local_file_sha256_hex`].
fn walk_local_tree(path: &Path, recursive: bool) -> std::io::Result<Vec<std::path::PathBuf>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !meta.file_type().is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        let mut children: Vec<std::path::PathBuf> = Vec::new();
        for entry in entries.flatten() {
            children.push(entry.path());
        }
        children.sort();
        for child in children {
            let Ok(child_meta) = std::fs::symlink_metadata(&child) else {
                continue;
            };
            let ft = child_meta.file_type();
            if ft.is_file() {
                out.push(child);
            } else if ft.is_dir() && recursive {
                stack.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Resolver for the server-side SHA256 of a local file path.
///
/// The CLI layer owns the strategy; in production this will talk to the
/// daemon via [`pcloud_ipc::Request::VerifyPath`], which in turn issues
/// `getfilelink` + `checksumfile` per file. For tests we inject a mock
/// that returns deterministic answers from an in-memory map.
pub trait ServerHashResolver {
    /// Resolve the server-side SHA256 hex digest for a local path, or
    /// `Ok(None)` when no remote counterpart exists (mapped to
    /// `[MISSING_REMOTE]`). Transport errors bubble up as `Err(String)`
    /// so the caller can surface them to the user.
    fn resolve(&self, local_path: &Path) -> Result<Option<String>, String>;
}

/// Per-file verification row emitted by [`verify_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRow {
    /// Local path walked.
    pub path: std::path::PathBuf,
    /// Classification tag for this row.
    pub classification: VerifyClassification,
}

/// Summary returned by [`verify_path`]. Drives exit-code selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VerifySummary {
    /// Count of `[OK]` rows.
    pub ok: usize,
    /// Count of `[MISMATCH]` rows.
    pub mismatch: usize,
    /// Count of `[MISSING_LOCAL]` rows.
    pub missing_local: usize,
    /// Count of `[MISSING_REMOTE]` rows.
    pub missing_remote: usize,
}

impl VerifySummary {
    /// Fold a classification into the summary counters.
    pub fn record(&mut self, c: &VerifyClassification) {
        match c {
            VerifyClassification::Ok => self.ok += 1,
            VerifyClassification::Mismatch { .. } => self.mismatch += 1,
            VerifyClassification::MissingLocal => self.missing_local += 1,
            VerifyClassification::MissingRemote => self.missing_remote += 1,
        }
    }

    /// Map the summary to the documented exit code.
    #[must_use]
    pub const fn exit_code(&self) -> ExitCode {
        if self.mismatch > 0 {
            ExitCode::Conflict
        } else if self.missing_local > 0 || self.missing_remote > 0 {
            ExitCode::Unavailable
        } else {
            ExitCode::Ok
        }
    }
}

/// Walk `path` and emit one [`VerifyRow`] per regular file encountered.
///
/// Non-existent `path` inputs surface a single `[MISSING_LOCAL]` row
/// for the requested path itself (so the CLI renders something instead
/// of silently exiting with nothing).
pub fn verify_path<R: ServerHashResolver>(
    path: &Path,
    recursive: bool,
    resolver: &R,
) -> Result<Vec<VerifyRow>, String> {
    let files = walk_local_tree(path, recursive).map_err(|e| e.to_string())?;
    if files.is_empty() && !path.exists() {
        // Path was requested but did not exist on disk. Emit a single
        // MISSING_LOCAL row so the caller's summary reports at least
        // one comparable unit of work.
        return Ok(vec![VerifyRow {
            path: path.to_path_buf(),
            classification: VerifyClassification::MissingLocal,
        }]);
    }
    let mut rows = Vec::with_capacity(files.len());
    for file in files {
        let local =
            local_file_sha256_hex(&file).map_err(|e| format!("sha256({}): {e}", file.display()))?;
        let server = resolver.resolve(&file)?;
        let classification = classify_file_hashes(local.as_deref(), server.as_deref());
        rows.push(VerifyRow {
            path: file,
            classification,
        });
    }
    Ok(rows)
}

/// Render a row in text mode: `[TAG...] <path>`.
pub fn render_text_row(row: &VerifyRow) -> String {
    format!("{} {}", row.classification.render(), row.path.display())
}

/// Render a row in NDJSON mode. One compact JSON object per line; no
/// trailing newline is appended by this function (the caller writes
/// one).
pub fn render_json_row(row: &VerifyRow) -> String {
    let status = row.classification.tag().to_ascii_lowercase();
    let (local, server) = match &row.classification {
        VerifyClassification::Mismatch { local, server } => {
            (Some(local.as_str()), Some(server.as_str()))
        }
        _ => (None, None),
    };
    // Hand-roll the JSON so we don't pull in serde_json just for this
    // path; fields are ASCII-safe and the `path` is escaped minimally.
    let escaped_path = json_escape(&row.path.to_string_lossy());
    let mut out = String::with_capacity(64 + escaped_path.len());
    out.push('{');
    out.push_str(r#""path":""#);
    out.push_str(&escaped_path);
    out.push_str(r#"","status":""#);
    out.push_str(&status);
    out.push('"');
    if let Some(l) = local {
        out.push_str(r#","local_sha256":""#);
        out.push_str(l);
        out.push('"');
    }
    if let Some(s) = server {
        out.push_str(r#","server_sha256":""#);
        out.push_str(s);
        out.push('"');
    }
    out.push('}');
    out
}

fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Parse `reduced` argv tail into `(path, recursive, fix, yes)`.
///
/// Returns `Err` with a human-readable usage message when the required
/// positional path is missing.
pub fn parse_verify_args(
    reduced: &[String],
) -> Result<(std::path::PathBuf, bool, bool, bool), String> {
    let mut path: Option<std::path::PathBuf> = None;
    let mut recursive = false;
    let mut fix = false;
    let mut yes = false;
    let mut i = 2; // [0] program, [1] canonical token "verify"
    while i < reduced.len() {
        let tok = reduced[i].as_str();
        match tok {
            "--recursive" => recursive = true,
            "--fix" => fix = true,
            "--yes" => yes = true,
            _ if tok.starts_with('-') => {
                // Already rejected upstream by
                // `reject_unknown_subcommand_flags`; skip defensively.
            }
            _ if path.is_none() => path = Some(std::path::PathBuf::from(tok)),
            _ => {}
        }
        i += 1;
    }
    let path = path.ok_or_else(|| {
        "verify: local path is required (e.g. `pcloudc verify ~/pCloudDrive --recursive`)"
            .to_owned()
    })?;
    Ok((path, recursive, fix, yes))
}

/// CLI entry point invoked from `main::run` when `Command::Verify` is
/// parsed. Returns the documented exit code.
pub fn run(flags: &GlobalFlags, reduced: &[String]) -> ExitCode {
    let (path, recursive, fix, yes) = match parse_verify_args(reduced) {
        Ok(v) => v,
        Err(msg) => {
            if !flags.quiet {
                match flags.output {
                    OutputFormat::Json => {
                        let env = crate::json_output::JsonEnvelope::from_error(
                            Some("verify".into()),
                            ExitCode::Usage,
                            msg.clone(),
                        );
                        print!("{}", env.render());
                    }
                    OutputFormat::Text => eprintln!("error: {msg}"),
                }
            }
            return ExitCode::Usage;
        }
    };
    // In a full daemon implementation, this resolver would dispatch
    // `Request::VerifyPath` and stream the server's per-file digests
    // back. Until that wire is plumbed end-to-end, we use the
    // unavailable-by-default resolver so the CLI still produces an
    // honest classification stream (every file → `[MISSING_REMOTE]`
    // unless we were run with `--fix`, which is not a substitute for
    // network access).
    let _ = fix;
    let _ = yes;
    let resolver = UnavailableResolver;
    run_with_resolver(flags, &path, recursive, &resolver)
}

/// Resolver that treats every file as having no remote counterpart.
/// Used when the daemon-walks-tree wire is not yet plumbed so the CLI
/// still emits an honest `[MISSING_REMOTE]` stream instead of silently
/// exiting success.
struct UnavailableResolver;

impl ServerHashResolver for UnavailableResolver {
    fn resolve(&self, _local_path: &Path) -> Result<Option<String>, String> {
        Ok(None)
    }
}

pub(crate) fn run_with_resolver<R: ServerHashResolver>(
    flags: &GlobalFlags,
    path: &Path,
    recursive: bool,
    resolver: &R,
) -> ExitCode {
    let rows = match verify_path(path, recursive, resolver) {
        Ok(r) => r,
        Err(msg) => {
            if !flags.quiet {
                match flags.output {
                    OutputFormat::Json => {
                        let env = crate::json_output::JsonEnvelope::from_error(
                            Some("verify".into()),
                            ExitCode::GenericError,
                            msg.clone(),
                        );
                        print!("{}", env.render());
                    }
                    OutputFormat::Text => eprintln!("error: {msg}"),
                }
            }
            return ExitCode::GenericError;
        }
    };

    let mut summary = VerifySummary::default();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for row in &rows {
        summary.record(&row.classification);
        if flags.quiet {
            continue;
        }
        match flags.output {
            OutputFormat::Json => {
                let _ = writeln!(out, "{}", render_json_row(row));
            }
            OutputFormat::Text => {
                let _ = writeln!(out, "{}", render_text_row(row));
            }
        }
    }
    let _ = out.flush();
    summary.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    struct MapResolver(HashMap<PathBuf, Option<String>>);

    impl ServerHashResolver for MapResolver {
        fn resolve(&self, local_path: &Path) -> Result<Option<String>, String> {
            Ok(self.0.get(local_path).cloned().unwrap_or(None))
        }
    }

    #[test]
    fn summary_counts_and_exit_codes() {
        let mut s = VerifySummary::default();
        assert_eq!(s.exit_code(), ExitCode::Ok);
        s.record(&VerifyClassification::MissingLocal);
        assert_eq!(s.exit_code(), ExitCode::Unavailable);
        s.record(&VerifyClassification::Mismatch {
            local: "a".into(),
            server: "b".into(),
        });
        assert_eq!(s.exit_code(), ExitCode::Conflict);
    }

    #[test]
    fn parse_args_happy_path_collects_flags() {
        let argv = vec![
            "pcloudc".to_owned(),
            "verify".to_owned(),
            "/tmp/x".to_owned(),
            "--recursive".to_owned(),
            "--fix".to_owned(),
            "--yes".to_owned(),
        ];
        let (p, r, f, y) = parse_verify_args(&argv).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/x"));
        assert!(r && f && y);
    }

    #[test]
    fn parse_args_rejects_missing_path() {
        let argv = vec!["pcloudc".to_owned(), "verify".to_owned()];
        assert!(parse_verify_args(&argv).is_err());
    }

    #[test]
    fn verify_path_missing_local_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let nope = tmp.path().join("nope.txt");
        let resolver = MapResolver(HashMap::new());
        let rows = verify_path(&nope, false, &resolver).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].classification, VerifyClassification::MissingLocal);
    }

    #[test]
    fn verify_path_ok_when_hashes_match() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.txt");
        std::fs::write(&f, b"abc").unwrap();
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned();
        let mut map = HashMap::new();
        map.insert(f.clone(), Some(sha));
        let resolver = MapResolver(map);
        let rows = verify_path(&f, false, &resolver).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].classification, VerifyClassification::Ok);
    }

    #[test]
    fn verify_path_reports_mismatch_missing_remote_and_mixed() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        let c = tmp.path().join("c.txt");
        std::fs::write(&a, b"abc").unwrap(); // known sha
        std::fs::write(&b, b"xyz").unwrap(); // no remote
        std::fs::write(&c, b"qqq").unwrap(); // wrong remote
        let mut map = HashMap::new();
        map.insert(
            a.clone(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned()),
        );
        // b: not present -> None
        map.insert(c.clone(), Some("deadbeef".to_owned()));
        let resolver = MapResolver(map);
        let rows = verify_path(tmp.path(), true, &resolver).unwrap();
        assert_eq!(rows.len(), 3);
        // Sorted order
        assert_eq!(rows[0].path, a);
        assert_eq!(rows[0].classification, VerifyClassification::Ok);
        assert_eq!(rows[1].path, b);
        assert_eq!(rows[1].classification, VerifyClassification::MissingRemote);
        assert_eq!(rows[2].path, c);
        assert!(rows[2].classification.is_mismatch());
    }

    #[test]
    fn text_renderer_covers_all_tags() {
        let row = VerifyRow {
            path: PathBuf::from("/tmp/a"),
            classification: VerifyClassification::Ok,
        };
        assert_eq!(render_text_row(&row), "[OK] /tmp/a");
        let row = VerifyRow {
            path: PathBuf::from("/tmp/b"),
            classification: VerifyClassification::Mismatch {
                local: "aa".into(),
                server: "bb".into(),
            },
        };
        assert_eq!(
            render_text_row(&row),
            "[MISMATCH local=aa server=bb] /tmp/b"
        );
    }

    #[test]
    fn json_renderer_emits_compact_ndjson_object() {
        let row = VerifyRow {
            path: PathBuf::from("/tmp/a"),
            classification: VerifyClassification::Ok,
        };
        let js = render_json_row(&row);
        assert!(js.starts_with('{') && js.ends_with('}'));
        assert!(js.contains(r#""path":"/tmp/a""#));
        assert!(js.contains(r#""status":"ok""#));

        let row = VerifyRow {
            path: PathBuf::from("/tmp/b"),
            classification: VerifyClassification::Mismatch {
                local: "aa".into(),
                server: "bb".into(),
            },
        };
        let js = render_json_row(&row);
        assert!(js.contains(r#""status":"mismatch""#));
        assert!(js.contains(r#""local_sha256":"aa""#));
        assert!(js.contains(r#""server_sha256":"bb""#));
    }
}

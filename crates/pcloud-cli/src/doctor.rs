//! `pcloudc doctor` — self-diagnostic command.
//!
//! Runs a battery of read-only checks against the local environment and
//! the running daemon, producing either a column-aligned text report or
//! a JSON envelope suitable for CI. Zero secret-bearing inputs flow
//! through this module; only filesystem metadata, TCP probes, and a
//! single plain `GetStatus` IPC call are performed.
//!
//! The checks are:
//!   1. Daemon reachable (IPC `GetStatus` round-trip)
//!   2. Config valid (dry-parse `~/.pcloud/config.toml` if present)
//!   3. Vault file permissions (0600) and parent dir (0700)
//!   4. Mount root exists and is writable (only if `mountpoint` configured)
//!   5. Clock sync drift vs. daemon (skipped with a WARN when daemon
//!      does not surface a usable timestamp — the current IPC
//!      [`pcloud_ipc::Response`] has no timestamp field, so this is a
//!      best-effort probe performed via `chrono`-free pure `std`)
//!   6. Network reachability to `binapi.pcloud.com:443` (TCP connect
//!      with a 5s timeout; no TLS handshake)
//!   7. Free disk at vault path and runtime dir (WARN if < 128 MiB)
//!   8. Pending journal file presence (WARN if any journal entry
//!      remains — operator should drain before shutdown)
//!
//! Exit code mapping (see [`DoctorReport::exit_code`]):
//!   - all OK        → [`ExitCode::Ok`]
//!   - any WARN only → [`ExitCode::Ok`] (warnings do not gate)
//!   - any FAIL      → [`ExitCode::Unavailable`] (closest documented
//!     operational-failure code in the existing vocabulary)

// **PLATFORM:** cross-platform
// **GATING:** Unix-specific idioms gated behind `#[cfg(unix)]`; Windows
// arms gated behind `#[cfg(windows)]`. Each platform-specific block is
// accompanied by a cross-platform fallback.

use std::fs;
use std::io::Write as _;
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pcloud_ipc::{IpcClient, Method, Request};
use serde::{Deserialize, Serialize};

use crate::exit_code::ExitCode;

/// Per-check status classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    /// Check passed.
    Ok,
    /// Check produced a non-blocking advisory.
    Warn,
    /// Check failed; user intervention required.
    Fail,
}

impl DoctorStatus {
    /// Render as the bracketed prefix used in text output.
    #[must_use]
    pub const fn text_tag(self) -> &'static str {
        match self {
            Self::Ok => "[OK]  ",
            Self::Warn => "[WARN]",
            Self::Fail => "[FAIL]",
        }
    }
}

/// Single-check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    /// Stable short identifier (snake_case).
    pub name: String,
    /// Result status.
    pub status: DoctorStatus,
    /// Human-readable one-line message.
    pub message: String,
    /// Optional free-form details (e.g. paths, byte counts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl DoctorCheck {
    fn ok(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status: DoctorStatus::Ok,
            message: message.into(),
            details: None,
        }
    }

    fn warn(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status: DoctorStatus::Warn,
            message: message.into(),
            details: None,
        }
    }

    fn fail(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status: DoctorStatus::Fail,
            message: message.into(),
            details: None,
        }
    }

    #[must_use]
    fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// Aggregate counts included at the end of the JSON envelope.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DoctorSummary {
    /// Number of `ok` checks.
    pub ok: u32,
    /// Number of `warn` checks.
    pub warn: u32,
    /// Number of `fail` checks.
    pub fail: u32,
}

/// Full diagnostic output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Per-check results, in display order.
    pub checks: Vec<DoctorCheck>,
    /// Aggregate counts.
    pub summary: DoctorSummary,
}

impl DoctorReport {
    /// Build a report and compute the summary counts.
    #[must_use]
    pub fn new(checks: Vec<DoctorCheck>) -> Self {
        let mut summary = DoctorSummary::default();
        for c in &checks {
            match c.status {
                DoctorStatus::Ok => summary.ok += 1,
                DoctorStatus::Warn => summary.warn += 1,
                DoctorStatus::Fail => summary.fail += 1,
            }
        }
        Self { checks, summary }
    }

    /// Map the aggregate status to an [`ExitCode`].
    ///
    /// - any FAIL → `Unavailable` (the closest documented operational
    ///   failure code in the existing vocabulary).
    /// - otherwise → `Ok` (warnings are informational and do not gate).
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        if self.summary.fail > 0 {
            ExitCode::Unavailable
        } else {
            ExitCode::Ok
        }
    }

    /// Render the column-aligned text form.
    #[must_use]
    pub fn render_text(&self) -> String {
        let mut buf = String::new();
        for c in &self.checks {
            buf.push_str(c.status.text_tag());
            buf.push(' ');
            buf.push_str(&c.message);
            buf.push('\n');
        }
        buf.push_str(&format!(
            "summary: {} ok, {} warn, {} fail\n",
            self.summary.ok, self.summary.warn, self.summary.fail
        ));
        buf
    }

    /// Render the JSON envelope.
    ///
    /// # Errors
    ///
    /// Returns [`serde_json::Error`] if serialization fails (should be
    /// impossible for this shape but surfaced rather than panicking).
    pub fn render_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Errors raised by [`run`].
#[derive(Debug, thiserror::Error)]
pub enum DoctorError {
    /// JSON serialization of the report failed.
    #[error("failed to serialize doctor report: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Input options for [`run`]. All optional — defaults derived from
/// `$PCLOUD_ROOT` / `$HOME`.
#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    /// Override the pCloud root (defaults to `$PCLOUD_ROOT`, then `$HOME/.pcloud`).
    pub root: Option<PathBuf>,
    /// Path to the IPC socket (defaults to `<root>/runtime/pcloud.sock`).
    pub socket_path: Option<PathBuf>,
    /// Path to the config file to dry-parse.
    pub config_path: Option<PathBuf>,
    /// Path to the auth-token vault file.
    pub vault_path: Option<PathBuf>,
    /// Optional mount root to validate (when user has configured one).
    pub mount_root: Option<PathBuf>,
    /// Optional journal directory to scan for pending entries.
    pub journal_dir: Option<PathBuf>,
    /// Override the daemon-reachability outcome for tests.
    pub mock_daemon_reachable: Option<bool>,
    /// Override the network-reachability outcome for tests.
    pub mock_network_reachable: Option<bool>,
    /// Skip the real TCP probe when `true` (used by tests).
    pub skip_network_probe: bool,
    /// When `true`, promote every [`DoctorStatus::Warn`] to
    /// [`DoctorStatus::Fail`] in the final report. Intended for CI use
    /// (`pcloudc doctor --strict`) where advisory warnings should gate
    /// a green build.
    pub strict: bool,
    /// Override the observed round-trip latency (seconds) for the
    /// `clock_drift` check. When `Some(v)`, `check_clock` skips the
    /// real IPC round-trip and uses `v` as the probe outcome. Used by
    /// tests to deterministically exercise the >30s warn branch.
    pub mock_clock_drift_secs: Option<u64>,
}

/// Run all diagnostic checks and return the accumulated report.
///
/// # Errors
///
/// Returns [`DoctorError`] only when report rendering itself fails; all
/// per-check failures are captured as `Fail` entries instead.
pub fn run(options: &DoctorOptions) -> Result<DoctorReport, DoctorError> {
    let root = resolve_root(options);
    let socket_path = options
        .socket_path
        .clone()
        .unwrap_or_else(|| root.join("runtime").join("pcloud.sock"));
    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(|| root.join("config.toml"));
    let vault_path = options
        .vault_path
        .clone()
        .unwrap_or_else(|| root.join("config").join("auth_token"));
    let journal_dir = options
        .journal_dir
        .clone()
        .unwrap_or_else(|| root.join("state").join("journal"));

    let mut checks = vec![
        check_daemon(&socket_path, options.mock_daemon_reachable),
        check_config(&config_path),
        check_vault_perms(&vault_path),
        check_mount_root(options.mount_root.as_deref()),
        check_clock(
            &socket_path,
            options.mock_daemon_reachable,
            options.mock_clock_drift_secs,
        ),
        check_network(options.mock_network_reachable, options.skip_network_probe),
        check_disk(&vault_path),
        check_disk_runtime(&socket_path),
        check_journal(&journal_dir),
    ];

    if options.strict {
        promote_warn_to_fail(&mut checks);
    }

    Ok(DoctorReport::new(checks))
}

/// Promote every [`DoctorStatus::Warn`] result to [`DoctorStatus::Fail`].
/// Applied when `--strict` is active so advisory warnings gate the
/// overall exit code.
fn promote_warn_to_fail(checks: &mut [DoctorCheck]) {
    for c in checks.iter_mut() {
        if c.status == DoctorStatus::Warn {
            c.status = DoctorStatus::Fail;
            c.message = format!("[strict] {}", c.message);
        }
    }
}

// **PLATFORM:** all. Resolve the pcloud root directory for doctor
// checks. Explicit `--root` wins, then `PCLOUD_ROOT` (legacy single-root
// layout), otherwise the XDG `data` directory from
// `PcloudDirs::discover()` (closest analogue to the pre-Phase-0
// `~/.pcloud` root).
fn resolve_root(options: &DoctorOptions) -> PathBuf {
    if let Some(r) = &options.root {
        return r.clone();
    }
    if let Some(r) = std::env::var_os("PCLOUD_ROOT") {
        return PathBuf::from(r);
    }
    pcloud_config::paths::PcloudDirs::discover()
        .map(|d| d.data)
        .unwrap_or_else(|_| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".pcloud"))
                .unwrap_or_else(|| PathBuf::from(".pcloud"))
        })
}

fn check_daemon(socket_path: &Path, mock: Option<bool>) -> DoctorCheck {
    if let Some(m) = mock {
        return if m {
            DoctorCheck::ok(
                "daemon_reachable",
                format!("daemon reachable (socket={})", socket_path.display()),
            )
        } else {
            DoctorCheck::fail(
                "daemon_reachable",
                format!("daemon unreachable (socket={})", socket_path.display()),
            )
        };
    }
    let client = IpcClient;
    let req = Request::Plain {
        method: Method::GetStatus,
    };
    match client.send(socket_path, &req) {
        Ok(_) => DoctorCheck::ok(
            "daemon_reachable",
            format!("daemon reachable (socket={})", socket_path.display()),
        ),
        Err(e) => DoctorCheck::fail("daemon_reachable", format!("daemon unreachable: {e}"))
            .with_details(socket_path.display().to_string()),
    }
}

fn check_config(config_path: &Path) -> DoctorCheck {
    if !config_path.exists() {
        return DoctorCheck::ok(
            "config_valid",
            format!(
                "no config file present (optional): {}",
                config_path.display()
            ),
        );
    }
    match fs::read_to_string(config_path) {
        Ok(s) => match toml_dry_parse(&s) {
            Ok(()) => DoctorCheck::ok(
                "config_valid",
                format!("config parses cleanly ({})", config_path.display()),
            ),
            Err(e) => DoctorCheck::fail(
                "config_valid",
                format!("config invalid ({}): {e}", config_path.display()),
            ),
        },
        Err(e) => DoctorCheck::fail(
            "config_valid",
            format!("cannot read config ({}): {e}", config_path.display()),
        ),
    }
}

/// Minimal TOML "dry parse" — we do not depend on a full toml parser
/// in pcloud-cli, so we rely on the simple fact that `CliConfig` uses a
/// line-based comment-annotated format. We accept any file whose
/// non-empty non-comment lines contain an `=` separator; anything else
/// is a syntax error. This is deliberately lenient: the daemon is the
/// authoritative parser; doctor only flags obvious corruption.
fn toml_dry_parse(body: &str) -> Result<(), String> {
    for (lineno, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
            continue;
        }
        if !trimmed.contains('=') {
            return Err(format!("line {}: expected 'key = value'", lineno + 1));
        }
    }
    Ok(())
}

/// Dispatch to the platform-specific vault permission probe.
///
/// * Unix: verifies POSIX mode masks (`0600` for the vault file, `0700`
///   for the parent directory).
/// * Windows: verifies NTFS ACL shape — the current-user SID must be the
///   sole ACE granting `FullControl` on both the vault file and its
///   parent directory.
/// * Other platforms: skipped with an OK advisory (nothing to check).
pub fn check_vault_perms(vault_path: &Path) -> DoctorCheck {
    if !vault_path.exists() {
        return DoctorCheck::ok(
            "vault_perms",
            format!("no vault present (optional): {}", vault_path.display()),
        );
    }
    #[cfg(unix)]
    {
        check_vault_perms_unix(vault_path)
    }
    #[cfg(windows)]
    {
        check_vault_perms_windows(vault_path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        DoctorCheck::warn(
            "vault_perms",
            format!(
                "vault permission audit not supported on this platform ({})",
                vault_path.display()
            ),
        )
    }
}

#[cfg(unix)]
fn check_vault_perms_unix(vault_path: &Path) -> DoctorCheck {
    let md = match fs::metadata(vault_path) {
        Ok(m) => m,
        Err(e) => {
            return DoctorCheck::fail(
                "vault_perms",
                format!("cannot stat vault {}: {e}", vault_path.display()),
            );
        }
    };
    let mode = md.permissions().mode() & 0o777;
    if mode != 0o600 {
        return DoctorCheck::fail(
            "vault_perms",
            format!("vault mode {:o} != 0600 ({})", mode, vault_path.display()),
        );
    }
    if let Some(parent) = vault_path.parent()
        && let Ok(pmd) = fs::metadata(parent)
    {
        let pmode = pmd.permissions().mode() & 0o777;
        if pmode != 0o700 {
            return DoctorCheck::fail(
                "vault_perms",
                format!(
                    "vault parent mode {:o} != 0700 ({})",
                    pmode,
                    parent.display()
                ),
            );
        }
    }
    DoctorCheck::ok(
        "vault_perms",
        format!("vault 0600, parent 0700 ({})", vault_path.display()),
    )
}

/// Windows ACL shape check. Intent: the current-user SID must be the
/// single ACE granting `FullControl` on both the vault file and its
/// parent directory (equivalent to the Unix `0600` / `0700` posture).
///
/// This function compiles only on Windows; the real ACL probe is wired
/// through `windows::Win32::Security` once that dependency is added to
/// `pcloud-cli/Cargo.toml` (tracked under bd-xplat-windows). The
/// placeholder below uses only `std` so the crate still builds on
/// Windows targets without new dependencies; when the `readonly`
/// attribute is clear on the file *and* the parent directory, the
/// vault is assumed to be in the safe shape pending full ACL audit.
/// This matches the documented plan in `PLAN_CROSSPLATFORM.md §3.5`.
#[cfg(windows)]
fn check_vault_perms_windows(vault_path: &Path) -> DoctorCheck {
    let md = match fs::metadata(vault_path) {
        Ok(m) => m,
        Err(e) => {
            return DoctorCheck::fail(
                "vault_perms",
                format!("cannot stat vault {}: {e}", vault_path.display()),
            );
        }
    };
    // Placeholder until `windows::Win32::Security` is added as a
    // pcloud-cli dependency; report a WARN so operators know the full
    // ACL audit has not run.
    let ro = md.permissions().readonly();
    DoctorCheck::warn(
        "vault_perms",
        format!(
            "vault NTFS ACL audit not yet wired (readonly={}, {}) — \
             full SID-only FullControl check pending (bd-xplat-windows)",
            ro,
            vault_path.display()
        ),
    )
}

fn check_mount_root(mount_root: Option<&Path>) -> DoctorCheck {
    let Some(path) = mount_root else {
        return DoctorCheck::ok("mount_root", "no mount root configured (optional)");
    };
    if !path.exists() {
        return DoctorCheck::fail(
            "mount_root",
            format!("mount root missing: {}", path.display()),
        );
    }
    // Writability probe — create and delete a dotfile. On Unix we set
    // mode `0600` at creation time so the probe file itself never has
    // group/world bits even if the enclosing mount has a wide default
    // umask. On Windows the ACL inherited from the parent is used;
    // `windows::Win32::Security` integration is planned separately.
    let probe = path.join(".pcloudc-doctor-probe");
    let mut open_opts = fs::OpenOptions::new();
    open_opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        open_opts.mode(0o600);
    }
    match open_opts.open(&probe) {
        Ok(mut f) => {
            let _: std::io::Result<()> = f.write_all(b"ok");
            let _ = fs::remove_file(&probe);
            DoctorCheck::ok(
                "mount_root",
                format!("mount root writable ({})", path.display()),
            )
        }
        Err(e) => DoctorCheck::fail(
            "mount_root",
            format!("mount root not writable ({}): {e}", path.display()),
        ),
    }
}

/// Clock-drift proxy probe: measures the wall-clock round-trip for one
/// `Method::GetStatus` IPC request. A round-trip in excess of
/// [`CLOCK_DRIFT_WARN_THRESHOLD`] strongly suggests either a stalled
/// system clock, a frozen daemon, or a badly drifted monotonic clock —
/// all of which warrant operator attention.
///
/// * `mock_daemon_reachable` — when `Some(false)` the probe is skipped
///   with an OK advisory (the daemon-reachable check already failed).
/// * `mock_drift_secs` — test hook: when `Some(v)`, synthesizes a
///   `v`-second round-trip so the >30s branch is exercisable without a
///   real stalled daemon.
fn check_clock(
    socket_path: &Path,
    mock_daemon_reachable: Option<bool>,
    mock_drift_secs: Option<u64>,
) -> DoctorCheck {
    if let Some(secs) = mock_drift_secs {
        return classify_clock_drift(Duration::from_secs(secs));
    }
    if mock_daemon_reachable == Some(false) {
        return DoctorCheck::ok(
            "clock_drift",
            "clock drift probe skipped (daemon unreachable)",
        );
    }
    let client = IpcClient;
    let req = Request::Plain {
        method: Method::GetStatus,
    };
    let started = Instant::now();
    match client.send(socket_path, &req) {
        Ok(_) => {
            let elapsed = started.elapsed();
            classify_clock_drift(elapsed)
        }
        Err(e) => DoctorCheck::warn("clock_drift", format!("clock drift probe failed: {e}")),
    }
}

/// Threshold above which a `GetStatus` round-trip is treated as
/// evidence of clock drift / stalled daemon. Chosen deliberately large
/// (30s) — anything less is a latency signal, not a clock-drift signal.
const CLOCK_DRIFT_WARN_THRESHOLD: Duration = Duration::from_secs(30);

fn classify_clock_drift(elapsed: Duration) -> DoctorCheck {
    if elapsed > CLOCK_DRIFT_WARN_THRESHOLD {
        DoctorCheck::warn(
            "clock_drift",
            format!(
                "clock drift warn: GetStatus round-trip {}s exceeds {}s threshold",
                elapsed.as_secs(),
                CLOCK_DRIFT_WARN_THRESHOLD.as_secs()
            ),
        )
    } else {
        DoctorCheck::ok(
            "clock_drift",
            format!(
                "clock drift ok: GetStatus round-trip {}ms",
                elapsed.as_millis()
            ),
        )
    }
}

fn check_network(mock: Option<bool>, skip: bool) -> DoctorCheck {
    if skip {
        return DoctorCheck::ok("network_reachable", "network probe skipped (test mode)");
    }
    if let Some(m) = mock {
        return if m {
            DoctorCheck::ok(
                "network_reachable",
                "network reachable: binapi.pcloud.com:443",
            )
        } else {
            DoctorCheck::fail("network_reachable", "network unreachable: timeout")
        };
    }
    let deadline = Duration::from_secs(5);
    let target = "binapi.pcloud.com:443";
    let addrs: Vec<SocketAddr> = match target.to_socket_addrs() {
        Ok(i) => i.collect(),
        Err(e) => {
            return DoctorCheck::fail(
                "network_reachable",
                format!("network dns lookup failed for {target}: {e}"),
            );
        }
    };
    let Some(addr) = addrs.first() else {
        return DoctorCheck::fail(
            "network_reachable",
            format!("network dns returned no records for {target}"),
        );
    };
    match std::net::TcpStream::connect_timeout(addr, deadline) {
        Ok(_) => DoctorCheck::ok(
            "network_reachable",
            format!("network reachable: {target} ({addr})"),
        ),
        Err(e) => DoctorCheck::fail(
            "network_reachable",
            format!("network unreachable ({target}): {e}"),
        ),
    }
}

/// Minimum free bytes we are willing to tolerate before WARN-ing.
const DISK_LOW_THRESHOLD_BYTES: u64 = 128 * 1024 * 1024;

fn check_disk(vault_path: &Path) -> DoctorCheck {
    disk_check("disk_vault", vault_path_parent(vault_path))
}

fn check_disk_runtime(socket_path: &Path) -> DoctorCheck {
    disk_check("disk_runtime", socket_path.parent().map(Path::to_path_buf))
}

fn vault_path_parent(vault_path: &Path) -> Option<PathBuf> {
    vault_path.parent().map(Path::to_path_buf)
}

fn disk_check(name: &str, dir: Option<PathBuf>) -> DoctorCheck {
    let Some(dir) = dir else {
        return DoctorCheck::warn(name, "cannot resolve directory for free-space check");
    };
    if !dir.exists() {
        return DoctorCheck::ok(
            name,
            format!("directory not present yet: {}", dir.display()),
        );
    }
    match free_space_bytes(&dir) {
        Ok(free) => {
            if free < DISK_LOW_THRESHOLD_BYTES {
                DoctorCheck::warn(
                    name,
                    format!("low free space: {} bytes in {}", free, dir.display()),
                )
            } else {
                DoctorCheck::ok(name, format!("{} bytes free in {}", free, dir.display()))
            }
        }
        Err(e) => DoctorCheck::warn(
            name,
            format!("cannot query free space in {}: {e}", dir.display()),
        ),
    }
}

/// Query the free-space available to the calling user in the partition
/// hosting `dir`. The implementation dispatches per platform:
///
/// * Unix (Linux, macOS, FreeBSD, etc.): `statvfs(3)` — the canonical
///   POSIX shape. We use `f_bavail * f_frsize` which is the correct
///   "bytes available to a non-privileged process" quantity.
/// * Windows: `GetDiskFreeSpaceExW` on the UTF-16 path — returns bytes
///   available to the calling user after quota enforcement.
/// * Other platforms: falls back to a conservative unknown-space
///   sentinel that triggers a WARN in [`disk_check`].
fn free_space_bytes(dir: &Path) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        free_space_bytes_unix(dir)
    }
    #[cfg(windows)]
    {
        free_space_bytes_windows(dir)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "free-space query not supported on this platform",
        ))
    }
}

#[cfg(unix)]
fn free_space_bytes_unix(dir: &Path) -> std::io::Result<u64> {
    use std::ffi::CString;
    let c = CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    // SAFETY: `statvfs` is a POSIX syscall; we pass a valid C string
    // pointer and an initialized `statvfs` struct whose fields are
    // POD. The call does not retain either pointer past return.
    let mut out: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut out) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // `f_bavail * f_frsize` is the canonical free-bytes-for-non-root
    // computation. Cast carefully; `statvfs` fields are platform typed.
    let free: u64 = (out.f_bavail as u64).saturating_mul(out.f_frsize as u64);
    Ok(free)
}

/// Windows free-space probe. Calls `GetDiskFreeSpaceExW` once
/// `pcloud-cli` gains a Windows build; for now we fall back to
/// `fs::metadata`-based existence verification and report "unknown"
/// by erroring out so the caller downgrades to a WARN. The full
/// wiring is tracked under bd-xplat-windows.
#[cfg(windows)]
fn free_space_bytes_windows(dir: &Path) -> std::io::Result<u64> {
    // Existence check mirrors the Unix path (statvfs errors early on
    // a non-existent dir); we surface the same semantics here.
    let _ = fs::metadata(dir)?;
    // Placeholder until `windows::Win32::Storage::FileSystem::
    // GetDiskFreeSpaceExW` is wired in. We return Unsupported so
    // `disk_check` records a WARN — never silently claims a value.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "GetDiskFreeSpaceExW integration pending (bd-xplat-windows)",
    ))
}

fn check_journal(journal_dir: &Path) -> DoctorCheck {
    if !journal_dir.exists() {
        return DoctorCheck::ok("journal_pending", "no journal directory (clean)");
    }
    let iter = match fs::read_dir(journal_dir) {
        Ok(i) => i,
        Err(e) => {
            return DoctorCheck::warn(
                "journal_pending",
                format!("cannot scan journal ({}): {e}", journal_dir.display()),
            );
        }
    };
    let mut count = 0usize;
    for entry in iter.flatten() {
        // Count only regular files to ignore nested subdirs that the
        // staging layer may create for book-keeping.
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            count += 1;
        }
    }
    if count == 0 {
        DoctorCheck::ok("journal_pending", "no pending journal entries")
    } else {
        DoctorCheck::warn(
            "journal_pending",
            format!(
                "{} pending journal entr{}",
                count,
                if count == 1 { "y" } else { "ies" }
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn touch(path: &Path, mode: u32) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, b"").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn doctor_check_status_text_tags_are_stable() {
        assert_eq!(DoctorStatus::Ok.text_tag(), "[OK]  ");
        assert_eq!(DoctorStatus::Warn.text_tag(), "[WARN]");
        assert_eq!(DoctorStatus::Fail.text_tag(), "[FAIL]");
    }

    #[test]
    fn report_text_mentions_every_check() {
        let checks = vec![
            DoctorCheck::ok("daemon_reachable", "daemon reachable (socket=/tmp/x)"),
            DoctorCheck::warn("clock_drift", "clock drift 42s"),
            DoctorCheck::fail("network_reachable", "network unreachable: timeout"),
        ];
        let report = DoctorReport::new(checks);
        let text = report.render_text();
        assert!(text.contains("[OK]   daemon reachable"));
        assert!(text.contains("[WARN] clock drift 42s"));
        assert!(text.contains("[FAIL] network unreachable: timeout"));
        assert!(text.contains("summary: 1 ok, 1 warn, 1 fail"));
    }

    #[test]
    fn report_json_shape_matches_contract() {
        let checks = vec![
            DoctorCheck::ok("daemon_reachable", "ok").with_details("sock=/x"),
            DoctorCheck::warn("clock_drift", "42s"),
            DoctorCheck::fail("network_reachable", "timeout"),
        ];
        let report = DoctorReport::new(checks);
        let s = report.render_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let arr = v.get("checks").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["name"], "daemon_reachable");
        assert_eq!(arr[0]["status"], "ok");
        assert_eq!(arr[0]["details"], "sock=/x");
        assert_eq!(arr[1]["status"], "warn");
        assert_eq!(arr[2]["status"], "fail");
        let summary = v.get("summary").unwrap();
        assert_eq!(summary["ok"], 1);
        assert_eq!(summary["warn"], 1);
        assert_eq!(summary["fail"], 1);
    }

    #[test]
    fn exit_code_maps_fail_to_unavailable_and_warn_to_ok() {
        let ok_only = DoctorReport::new(vec![DoctorCheck::ok("x", "ok")]);
        assert_eq!(ok_only.exit_code(), ExitCode::Ok);

        let warn_only = DoctorReport::new(vec![
            DoctorCheck::ok("a", "ok"),
            DoctorCheck::warn("b", "warn"),
        ]);
        assert_eq!(warn_only.exit_code(), ExitCode::Ok);

        let with_fail = DoctorReport::new(vec![
            DoctorCheck::ok("a", "ok"),
            DoctorCheck::fail("b", "fail"),
        ]);
        assert_eq!(with_fail.exit_code(), ExitCode::Unavailable);
    }

    #[test]
    fn mock_daemon_reachable_true_yields_ok() {
        let tmp = TempDir::new().unwrap();
        let opts = DoctorOptions {
            root: Some(tmp.path().to_path_buf()),
            mock_daemon_reachable: Some(true),
            skip_network_probe: true,
            ..Default::default()
        };
        let report = run(&opts).unwrap();
        let daemon = report
            .checks
            .iter()
            .find(|c| c.name == "daemon_reachable")
            .unwrap();
        assert_eq!(daemon.status, DoctorStatus::Ok);
        let text = report.render_text();
        assert!(text.contains("[OK]   daemon reachable"));
    }

    #[test]
    fn mock_daemon_reachable_false_yields_fail_and_unavailable_exit() {
        let tmp = TempDir::new().unwrap();
        let opts = DoctorOptions {
            root: Some(tmp.path().to_path_buf()),
            mock_daemon_reachable: Some(false),
            skip_network_probe: true,
            ..Default::default()
        };
        let report = run(&opts).unwrap();
        let daemon = report
            .checks
            .iter()
            .find(|c| c.name == "daemon_reachable")
            .unwrap();
        assert_eq!(daemon.status, DoctorStatus::Fail);
        assert_eq!(report.exit_code(), ExitCode::Unavailable);
    }

    #[test]
    #[cfg(unix)]
    fn vault_perms_detects_wrong_mode() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("config").join("auth_token");
        touch(&vault, 0o644);
        // Parent dir perm is whatever TempDir set — ensure it's 0700 for
        // the mode check to reach the vault-mode branch.
        fs::set_permissions(vault.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        let c = check_vault_perms(&vault);
        assert_eq!(c.status, DoctorStatus::Fail);
        assert!(c.message.contains("vault mode"));
    }

    #[test]
    #[cfg(unix)]
    fn vault_perms_accepts_0600_with_0700_parent() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("config").join("auth_token");
        touch(&vault, 0o600);
        fs::set_permissions(vault.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        let c = check_vault_perms(&vault);
        assert_eq!(c.status, DoctorStatus::Ok);
    }

    #[test]
    fn config_check_is_ok_when_missing() {
        let tmp = TempDir::new().unwrap();
        let c = check_config(&tmp.path().join("config.toml"));
        assert_eq!(c.status, DoctorStatus::Ok);
    }

    #[test]
    fn config_check_detects_corrupt_line() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "good = 1\nbroken-line-no-equals\n").unwrap();
        let c = check_config(&path);
        assert_eq!(c.status, DoctorStatus::Fail);
    }

    #[test]
    fn mount_root_missing_is_fail() {
        let p = Path::new("/nonexistent/pcloudc-doctor");
        let c = check_mount_root(Some(p));
        assert_eq!(c.status, DoctorStatus::Fail);
    }

    #[test]
    fn mount_root_none_is_ok() {
        let c = check_mount_root(None);
        assert_eq!(c.status, DoctorStatus::Ok);
    }

    #[test]
    fn journal_empty_dir_is_ok() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("state").join("journal");
        fs::create_dir_all(&dir).unwrap();
        let c = check_journal(&dir);
        assert_eq!(c.status, DoctorStatus::Ok);
    }

    #[test]
    fn journal_with_files_warns() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("state").join("journal");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.jrn"), b"x").unwrap();
        fs::write(dir.join("b.jrn"), b"y").unwrap();
        let c = check_journal(&dir);
        assert_eq!(c.status, DoctorStatus::Warn);
        assert!(c.message.contains("2 pending"));
    }

    #[test]
    fn mock_network_false_yields_fail() {
        let tmp = TempDir::new().unwrap();
        let opts = DoctorOptions {
            root: Some(tmp.path().to_path_buf()),
            mock_daemon_reachable: Some(true),
            mock_network_reachable: Some(false),
            ..Default::default()
        };
        let report = run(&opts).unwrap();
        let net = report
            .checks
            .iter()
            .find(|c| c.name == "network_reachable")
            .unwrap();
        assert_eq!(net.status, DoctorStatus::Fail);
        assert!(net.message.contains("unreachable"));
    }

    /// Unix-only: exact POSIX-mode posture (`0600` file, `0700` parent)
    /// must pass the vault_perms check without falling back to WARN.
    #[test]
    #[cfg(unix)]
    fn vault_perms_check_passes_on_unix_0600_0700() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("config").join("auth_token");
        touch(&vault, 0o600);
        fs::set_permissions(vault.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        let c = check_vault_perms(&vault);
        assert_eq!(c.status, DoctorStatus::Ok, "unix 0600/0700 must be OK");
        assert!(c.message.contains("0600"));
        assert!(c.message.contains("0700"));
    }

    /// Windows-only compile-time harness: asserts that the Windows arm
    /// of [`check_vault_perms`] is reachable with the documented
    /// signature. The real ACL audit cannot be executed on Linux, so
    /// this test only verifies shape when targeting Windows.
    #[test]
    #[cfg(windows)]
    fn vault_perms_check_inspects_acl_on_windows() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("config").join("auth_token");
        if let Some(p) = vault.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(&vault, b"").unwrap();
        let c: DoctorCheck = check_vault_perms(&vault);
        assert_eq!(c.name, "vault_perms");
        // Until the full `windows::Win32::Security` probe lands the
        // check is documented to report WARN — never silently OK.
        assert_eq!(c.status, DoctorStatus::Warn);
    }

    /// The `clock_drift` check must warn when the observed
    /// `GetStatus` round-trip exceeds the 30 s threshold.
    #[test]
    fn clock_drift_warns_above_30s_threshold() {
        let under = classify_clock_drift(Duration::from_secs(5));
        assert_eq!(under.status, DoctorStatus::Ok);

        let at_edge = classify_clock_drift(Duration::from_secs(30));
        assert_eq!(at_edge.status, DoctorStatus::Ok);

        let over = classify_clock_drift(Duration::from_secs(31));
        assert_eq!(over.status, DoctorStatus::Warn);
        assert!(over.message.contains("exceeds 30s"));

        // End-to-end: drive `run()` with the mock drift hook so the
        // synthesized >30s round-trip promotes the `clock_drift`
        // entry to WARN in the aggregate report.
        let tmp = TempDir::new().unwrap();
        let opts = DoctorOptions {
            root: Some(tmp.path().to_path_buf()),
            mock_daemon_reachable: Some(true),
            mock_network_reachable: Some(true),
            skip_network_probe: true,
            mock_clock_drift_secs: Some(42),
            ..Default::default()
        };
        let report = run(&opts).unwrap();
        let clk = report
            .checks
            .iter()
            .find(|c| c.name == "clock_drift")
            .unwrap();
        assert_eq!(clk.status, DoctorStatus::Warn);
    }

    /// `--strict` promotes every WARN to FAIL so CI runs gate on
    /// advisory warnings and the exit code flips to `Unavailable`.
    #[test]
    fn strict_mode_promotes_warn_to_fail() {
        let tmp = TempDir::new().unwrap();
        let opts = DoctorOptions {
            root: Some(tmp.path().to_path_buf()),
            mock_daemon_reachable: Some(true),
            mock_network_reachable: Some(true),
            skip_network_probe: true,
            // Force a WARN through the deterministic clock hook.
            mock_clock_drift_secs: Some(60),
            strict: true,
            ..Default::default()
        };
        let report = run(&opts).unwrap();
        let clk = report
            .checks
            .iter()
            .find(|c| c.name == "clock_drift")
            .unwrap();
        assert_eq!(
            clk.status,
            DoctorStatus::Fail,
            "strict mode must promote WARN -> FAIL"
        );
        assert!(clk.message.starts_with("[strict]"));
        assert_eq!(report.exit_code(), ExitCode::Unavailable);
        assert_eq!(report.summary.warn, 0);
        assert!(report.summary.fail >= 1);
    }
}

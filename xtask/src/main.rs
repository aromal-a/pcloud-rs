#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Repository-owned local CI/CD orchestration.
//!
//! GitHub Actions is intentionally disabled for this repository. Run
//! `cargo xtask ci` from the workspace root to execute the authoritative
//! Linux, coverage, Docker, packaging, and remote-Windows gates.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const TOOLCHAIN: &str = "1.96.1";
const COVERAGE_FLOOR: &str = "90";
const DEFAULT_WINDOWS_HOST: &str = "winovh.docbetry.fr";
const DEFAULT_WINDOWS_USER: &str = "Administrator";
const DEFAULT_WINDOWS_ROOT: &str = r"C:\pcloud-rs-local-ci-runs";

type TaskResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[xtask] FAILED: {error}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> TaskResult {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_owned());
    if args.next().is_some() {
        return Err("xtask commands do not accept positional arguments".into());
    }

    ensure_workspace_root()?;
    match command.as_str() {
        "ci" => run_ci(),
        "host" => run_host(),
        "compat" => run_compatibility(),
        "coverage" => run_coverage(),
        "docker" => run_docker(),
        "windows" => run_windows(),
        "package" => run_packaging(),
        "release" => run_release(),
        "preflight" => run_preflight(),
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        other => Err(format!("unknown xtask command: {other}").into()),
    }
}

fn print_help() {
    println!(
        "\
pcloud-rs local CI/CD

USAGE:
    cargo xtask <COMMAND>

COMMANDS:
    preflight  Verify local tools, the Rust pin, and disabled GitHub workflows
    compat     Run the Rust 1.89/1.91 MSRV and optional-feature checks
    host       Run formatting, compile, lint, test, docs, and security gates
    coverage   Generate LCOV and require workspace line coverage above 90%
    docker     Build and smoke-test the OCI image and Linux portability matrix
    windows    Sync the dirty working tree and run native Windows gates over SSH
    package    Validate NAS/Unix/package metadata and reproducibility
    ci         Run preflight + host + coverage + package + Docker + Windows
    release    Run the full CI pipeline plus reproducible release builds

ENVIRONMENT:
    PCLOUD_CI_SKIP_DOCKER=1       Skip Docker only for an explicitly partial run
    PCLOUD_CI_SKIP_WINDOWS=1      Skip Windows only for an explicitly partial run
    PCLOUD_CI_WINDOWS_HOST        Windows SSH host (default: {DEFAULT_WINDOWS_HOST})
    PCLOUD_CI_WINDOWS_USER        Windows SSH user (default: {DEFAULT_WINDOWS_USER})
    PCLOUD_CI_WINDOWS_KEY         SSH private key (default: ~/.ssh/hetzner_id)
    PCLOUD_CI_WINDOWS_PASSWORD    Password used only for the DPAPI-capable test session
    PCLOUD_CI_WINDOWS_ROOT        Remote source directory
"
    );
}

fn run_ci() -> TaskResult {
    let started = Instant::now();
    run_preflight()?;
    run_compatibility()?;
    run_host()?;
    run_coverage()?;
    run_packaging()?;
    if !env_flag("PCLOUD_CI_SKIP_DOCKER") {
        run_docker()?;
    } else {
        println!("[xtask] Docker explicitly skipped; result is partial");
    }
    if !env_flag("PCLOUD_CI_SKIP_WINDOWS") {
        run_windows()?;
    } else {
        println!("[xtask] Windows explicitly skipped; result is partial");
    }
    println!("[xtask] CI passed in {:.1?}", started.elapsed());
    Ok(())
}

fn run_compatibility() -> TaskResult {
    println!("[xtask] compatibility gates");
    step(
        "portable-core MSRV (Rust 1.89)",
        warnings_as_errors(command(
            "cargo",
            [
                "+1.89.0",
                "check",
                "--workspace",
                "--all-targets",
                "--locked",
                "--exclude",
                "pcloud-plugin-wasmtime",
            ],
        )),
    )?;
    step(
        "Wasmtime plugin MSRV (Rust 1.91)",
        warnings_as_errors(command(
            "cargo",
            [
                "+1.91.0",
                "check",
                "-p",
                "pcloud-plugin-wasmtime",
                "--all-targets",
                "--locked",
            ],
        )),
    )?;
    for (package, feature) in [
        ("pcloud-config", "kms-factory"),
        ("pcloud-config", "aws-kms"),
        ("pcloud-daemon", "metrics"),
        ("pcloud-daemon", "json-logs"),
        ("pcloud-daemon", "tracing-otlp"),
        ("pcloud-observability", "prometheus-exporter"),
        ("pcloud-observability", "tracing-otlp"),
    ] {
        step(
            &format!("optional feature {package}/{feature}"),
            warnings_as_errors(command(
                "cargo",
                ["check", "-p", package, "--features", feature, "--locked"],
            )),
        )?;
    }
    Ok(())
}

fn run_release() -> TaskResult {
    run_ci()?;
    step(
        "reproducible release binaries",
        command0("packaging/scripts/verify-reproducibility.sh"),
    )?;
    println!("[xtask] release candidate gates passed; signing remains operator-controlled");
    Ok(())
}

fn run_preflight() -> TaskResult {
    println!("[xtask] preflight");
    let active_yaml: Vec<PathBuf> = fs::read_dir(".github/workflows")?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(OsStr::to_str),
                Some("yml" | "yaml")
            )
        })
        .collect();
    if !active_yaml.is_empty() {
        return Err(format!(
            "GitHub Actions is not disabled; active workflow YAML remains: {active_yaml:?}"
        )
        .into());
    }
    if !Path::new(".github/workflows-disabled").is_dir() {
        return Err("disabled workflow archive is missing".into());
    }

    let rustc = output(command("rustc", ["--version"]))?;
    if !rustc.contains(&format!("rustc {TOOLCHAIN} ")) {
        return Err(format!("expected Rust {TOOLCHAIN}, got {rustc:?}").into());
    }
    for tool in ["cargo", "git", "ssh", "tar"] {
        require_tool(tool)?;
    }
    require_tool("cargo-llvm-cov")?;
    println!("[xtask] GitHub workflows disabled; Rust {TOOLCHAIN} active");
    Ok(())
}

fn run_host() -> TaskResult {
    println!("[xtask] host gates");
    step("rustfmt", cargo(["fmt", "--all", "--", "--check"]))?;
    step(
        "workspace check",
        cargo(["check", "--workspace", "--all-targets", "--locked"]),
    )?;
    step(
        "workspace clippy",
        cargo([
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ]),
    )?;
    step(
        "workspace tests",
        cargo(["test", "--workspace", "--locked"]),
    )?;

    let mut rustdoc = cargo(["doc", "--workspace", "--locked", "--no-deps"]);
    rustdoc.env("RUSTDOCFLAGS", "-D warnings");
    step("rustdoc", rustdoc)?;
    step(
        "mdBook link validation",
        command("ruby", ["scripts/check-mdbook-links.rb"]),
    )?;
    step("handbook build", command("mdbook", ["build", "docs/book"]))?;
    step(
        "architecture atlas generation",
        command("python3", ["docs/architecture-atlas/tools/generate.py"]),
    )?;
    step(
        "architecture atlas links",
        command("python3", ["docs/architecture-atlas/tools/check_links.py"]),
    )?;
    step(
        "architecture atlas build",
        command("mdbook", ["build", "docs/architecture-atlas"]),
    )?;
    step("version contract", command0("scripts/check-versions.sh"))?;
    run_shell_syntax_checks()?;
    if tool_available("cargo-deny") {
        step(
            "cargo deny",
            command("cargo", ["deny", "--locked", "check"]),
        )?;
    } else {
        return Err("cargo-deny is required for the local CI pipeline".into());
    }
    if tool_available("cargo-audit") {
        step(
            "cargo audit",
            command(
                "cargo",
                [
                    "audit",
                    "--deny",
                    "warnings",
                    "--ignore",
                    "RUSTSEC-2023-0071",
                ],
            ),
        )?;
    } else {
        return Err("cargo-audit is required for the local CI pipeline".into());
    }
    Ok(())
}

fn run_coverage() -> TaskResult {
    println!("[xtask] coverage gate (required: > {COVERAGE_FLOOR}%)");
    fs::create_dir_all("target/xtask")?;
    let report = Path::new("target/xtask/lcov.info");
    step(
        "clean prior coverage profiles",
        command("cargo", ["llvm-cov", "clean", "--workspace", "--locked"]),
    )?;
    let mut coverage = command(
        "cargo",
        [
            "llvm-cov",
            "--workspace",
            "--exclude",
            "xtask",
            "--locked",
            "--no-report",
        ],
    );
    coverage.env("PCLOUD_FUSE_TEST", "1");
    step("instrumented workspace tests", coverage)?;

    #[cfg(target_os = "linux")]
    {
        let mut live_mount_unit = command(
            "cargo",
            [
                "llvm-cov",
                "-p",
                "pcloud-fs",
                "--lib",
                "--locked",
                "--no-report",
                "--",
                "--ignored",
                "--test-threads=1",
            ],
        );
        live_mount_unit.env("PCLOUD_FUSE_TEST", "1");
        step("coverage live FUSE: mount-service unit", live_mount_unit)?;
    }

    #[cfg(target_os = "linux")]
    for test_name in [
        "fuse_mount_integration",
        "fuse_lifecycle_hardening",
        "mount_transport_wiring",
        "fuse_read_path_live",
        "fuse_write_path_live",
        "fuse_small_write_wiring",
        "fuse_kernel_e2e",
        "fuse_dyn_shim_write",
    ] {
        let mut live_mount = command(
            "cargo",
            [
                "llvm-cov",
                "-p",
                "pcloud-fs",
                "--test",
                test_name,
                "--locked",
                "--no-report",
                "--",
                "--ignored",
                "--test-threads=1",
            ],
        );
        live_mount.env("PCLOUD_FUSE_TEST", "1");
        step(&format!("coverage live FUSE: {test_name}"), live_mount)?;
    }

    #[cfg(unix)]
    {
        let mut chaos = command(
            "cargo",
            [
                "llvm-cov",
                "-p",
                "pcloud-chaos",
                "--locked",
                "--no-report",
                "--",
                "--ignored",
                "--test-threads=1",
            ],
        );
        chaos.env("PCLOUD_CHAOS", "1");
        step("coverage deterministic chaos suites", chaos)?;
    }

    step(
        "generate LCOV report",
        command(
            "cargo",
            [
                "llvm-cov",
                "report",
                "--lcov",
                "--output-path",
                report.to_str().ok_or("non-UTF8 coverage path")?,
                "--ignore-filename-regex",
                r"(^|/)(tests|benches|examples|fuzz)/|/build\.rs$",
            ],
        ),
    )?;
    step(
        "coverage policy",
        command(
            "scripts/coverage-check.sh",
            [
                report.to_str().ok_or("non-UTF8 coverage path")?,
                COVERAGE_FLOOR,
            ],
        ),
    )?;
    Ok(())
}

fn run_packaging() -> TaskResult {
    println!("[xtask] packaging gates");
    step(
        "NAS package validation",
        command0("packaging/nas/validate.sh"),
    )?;
    step(
        "portable Unix package validation",
        command0("packaging/unix/validate.sh"),
    )?;
    step(
        "SDK model package verification",
        cargo(["package", "-p", "pcloud-model", "--allow-dirty", "--locked"]),
    )?;
    step(
        "SDK IPC package manifest",
        cargo([
            "package",
            "-p",
            "pcloud-ipc",
            "--allow-dirty",
            "--locked",
            "--list",
        ]),
    )?;
    step(
        "public SDK package manifest",
        cargo([
            "package",
            "-p",
            "pcloud-sdk",
            "--allow-dirty",
            "--locked",
            "--list",
        ]),
    )?;
    Ok(())
}

fn run_docker() -> TaskResult {
    println!("[xtask] Docker/Linux matrix");
    let docker = Docker::discover()?;
    let git_sha = output(command("git", ["rev-parse", "--short=12", "HEAD"]))?;
    docker.run([
        "build",
        "--network",
        "host",
        "--build-arg",
        &format!("RUST_VERSION={TOOLCHAIN}"),
        "--build-arg",
        &format!("GIT_SHA={git_sha}"),
        "--tag",
        "pcloud-rs:local-ci",
        "--file",
        "packaging/docker/Dockerfile",
        ".",
    ])?;
    docker.run([
        "run",
        "--rm",
        "--network",
        "none",
        "--entrypoint",
        "/usr/local/bin/pcloudc",
        "pcloud-rs:local-ci",
        "--version",
    ])?;

    let root = env::current_dir()?;
    let root_mount = format!("{}:/workspace:ro", root.display());
    docker.run([
        "run",
        "--rm",
        "--network",
        "host",
        "--volume",
        &root_mount,
        "--workdir",
        "/workspace",
        "rust:1.96.1-bookworm",
        "bash",
        "-c",
        "apt-get update >/dev/null && \
         apt-get install -y --no-install-recommends libfuse3-dev mandoc pkg-config >/dev/null && \
         CARGO_TARGET_DIR=/tmp/target /usr/local/cargo/bin/cargo \
         check --workspace --all-targets --locked && \
         mandoc -T lint packaging/man/*.1 packaging/man/*.5",
    ])?;
    Ok(())
}

fn run_windows() -> TaskResult {
    println!("[xtask] native Windows matrix");
    let remote = WindowsRemote::from_env()?;
    if let Err(sync_error) = remote.sync_workspace() {
        if let Err(cleanup_error) = remote.cleanup_workspace() {
            eprintln!(
                "[xtask] warning: Windows workspace cleanup after sync failure also failed: {cleanup_error}"
            );
        }
        return Err(sync_error);
    }
    let pipeline_result = remote.run_pipeline();
    let cleanup_result = remote.cleanup_workspace();
    match (pipeline_result, cleanup_result) {
        (Err(pipeline_error), Err(cleanup_error)) => {
            eprintln!("[xtask] warning: Windows workspace cleanup also failed: {cleanup_error}");
            Err(pipeline_error)
        }
        (Err(pipeline_error), Ok(())) => Err(pipeline_error),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn run_shell_syntax_checks() -> TaskResult {
    let paths = output(command(
        "find",
        [
            ".",
            "-path",
            "./target",
            "-prune",
            "-o",
            "-path",
            "./docs/book/book",
            "-prune",
            "-o",
            "-path",
            "./docs/architecture-atlas/book",
            "-prune",
            "-o",
            "-type",
            "f",
            "-name",
            "*.sh",
            "-print",
        ],
    ))?;
    for path in paths.lines().filter(|line| !line.is_empty()) {
        step(
            &format!("shell syntax {path}"),
            command("bash", ["-n", path]),
        )?;
    }
    Ok(())
}

fn ensure_workspace_root() -> TaskResult {
    if !Path::new("Cargo.toml").is_file() || !Path::new("crates/pcloud-daemon").is_dir() {
        return Err("run cargo xtask from the pcloud-rs workspace root".into());
    }
    Ok(())
}

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn cargo<const N: usize>(args: [&str; N]) -> Command {
    let mut command = Command::new("cargo");
    command.args(args);
    command
}

fn warnings_as_errors(mut command: Command) -> Command {
    command.env("RUSTFLAGS", "-D warnings");
    command
}

fn command<I, S>(program: impl AsRef<OsStr>, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    command
}

fn command0(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

fn step(label: &str, mut command: Command) -> TaskResult {
    println!("[xtask] >>> {label}");
    let started = Instant::now();
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{label} failed with {status}").into());
    }
    println!("[xtask] <<< {label} ({:.1?})", started.elapsed());
    Ok(())
}

fn output(mut command: Command) -> TaskResult<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "command failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn require_tool(tool: &str) -> TaskResult {
    if tool_available(tool) {
        Ok(())
    } else {
        Err(format!("required tool is missing: {tool}").into())
    }
}

fn tool_available(tool: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|directory| {
        let candidate = directory.join(tool);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            ["exe", "cmd", "bat"]
                .iter()
                .any(|extension| directory.join(format!("{tool}.{extension}")).is_file())
        }
        #[cfg(not(windows))]
        {
            false
        }
    })
}

struct Docker {
    sudo: bool,
}

impl Docker {
    fn discover() -> TaskResult<Self> {
        if Command::new("docker")
            .arg("info")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Ok(Self { sudo: false });
        }
        if Command::new("sudo")
            .args(["-n", "docker", "info"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Ok(Self { sudo: true });
        }
        Err("Docker daemon is unavailable (directly and via sudo -n)".into())
    }

    fn run<const N: usize>(&self, args: [&str; N]) -> TaskResult {
        let mut command = if self.sudo {
            let mut command = Command::new("sudo");
            command.args(["-n", "docker"]);
            command
        } else {
            Command::new("docker")
        };
        command.args(args);
        step("docker", command)
    }
}

struct WindowsRemote {
    host: String,
    user: String,
    key: PathBuf,
    password: OsString,
    root: String,
}

impl WindowsRemote {
    fn from_env() -> TaskResult<Self> {
        let host =
            env::var("PCLOUD_CI_WINDOWS_HOST").unwrap_or_else(|_| DEFAULT_WINDOWS_HOST.to_owned());
        let user =
            env::var("PCLOUD_CI_WINDOWS_USER").unwrap_or_else(|_| DEFAULT_WINDOWS_USER.to_owned());
        let root_base =
            env::var("PCLOUD_CI_WINDOWS_ROOT").unwrap_or_else(|_| DEFAULT_WINDOWS_ROOT.to_owned());
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let root = format!(
            r"{}\run-{nonce}-{}",
            root_base.trim_end_matches(['\\', '/']),
            std::process::id()
        );
        let key = env::var_os("PCLOUD_CI_WINDOWS_KEY")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".ssh/hetzner_id"))
            })
            .ok_or("PCLOUD_CI_WINDOWS_KEY is unset and HOME is unavailable")?;
        if !key.is_file() {
            return Err(format!("Windows SSH key does not exist: {}", key.display()).into());
        }
        let password = env::var_os("PCLOUD_CI_WINDOWS_PASSWORD")
            .filter(|value| !value.is_empty())
            .ok_or(
                "PCLOUD_CI_WINDOWS_PASSWORD is required: Windows CurrentUser DPAPI is unavailable in public-key OpenSSH logons",
            )?;
        require_tool("sshpass")?;
        Ok(Self {
            host,
            user,
            key,
            password,
            root,
        })
    }

    fn destination(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    fn ssh_base(&self) -> Command {
        let mut command = Command::new("ssh");
        command
            .arg("-i")
            .arg(&self.key)
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=20",
                "-o",
                "StrictHostKeyChecking=accept-new",
            ])
            .arg(self.destination());
        command
    }

    fn credentialed_ssh_base(&self) -> Command {
        let mut command = Command::new("sshpass");
        command
            .arg("-e")
            .arg("ssh")
            .args([
                "-o",
                "PreferredAuthentications=password",
                "-o",
                "PubkeyAuthentication=no",
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "BatchMode=no",
                "-o",
                "ConnectTimeout=20",
                "-o",
                "StrictHostKeyChecking=accept-new",
            ])
            .arg(self.destination())
            .env("SSHPASS", &self.password);
        command
    }

    fn sync_workspace(&self) -> TaskResult {
        println!(
            "[xtask] syncing working tree to {}:{}",
            self.host, self.root
        );
        let prepare_command = format!("cmd.exe /d /c \"mkdir {root}\"", root = self.root);
        let mut prepare = self.ssh_base();
        prepare.arg(prepare_command);
        step("prepare Windows workspace", prepare)?;

        let mut tar = Command::new("tar");
        tar.args([
            "--exclude=.git",
            "--exclude=target",
            "--exclude=docs/book/book",
            "--exclude=docs/architecture-atlas/book",
            "--exclude=lcov.info",
            "--exclude=lcov-*.info",
            "-czf",
            "-",
            ".",
        ])
        .stdout(Stdio::piped());
        let mut tar_child = tar.spawn()?;
        let tar_stdout = tar_child
            .stdout
            .take()
            .ok_or("failed to capture tar output")?;

        let mut ssh = self.ssh_base();
        ssh.arg(format!("tar -xzf - -C {}", self.root))
            .stdin(Stdio::from(tar_stdout));
        let ssh_status = ssh.status()?;
        drop(ssh);
        let tar_status = wait_child(tar_child)?;
        if !tar_status.success() {
            return Err(format!("workspace tar failed with {tar_status}").into());
        }
        if !ssh_status.success() {
            return Err(format!("workspace SSH extraction failed with {ssh_status}").into());
        }
        Ok(())
    }

    fn run_pipeline(&self) -> TaskResult {
        let script = format!(r"{}\scripts\local-ci\windows.ps1", self.root);
        let remote_command = format!(
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass \
             -File \"{script}\" -Workspace \"{}\" -Toolchain \"{TOOLCHAIN}\"",
            self.root
        );
        // CurrentUser DPAPI requires a credential-bearing Windows logon.
        // Public-key OpenSSH sessions intentionally lack those credential
        // materials and return ERROR_ACCESS_DENIED from CryptProtectData.
        let mut ssh = self.credentialed_ssh_base();
        ssh.arg(remote_command);
        step("Windows native pipeline", ssh)
    }

    fn cleanup_workspace(&self) -> TaskResult {
        let mut ssh = self.ssh_base();
        ssh.arg(format!("cmd.exe /d /c \"rmdir /s /q {}\"", self.root));
        step("clean Windows workspace", ssh)
    }
}

fn wait_child(mut child: Child) -> io::Result<std::process::ExitStatus> {
    child.wait()
}

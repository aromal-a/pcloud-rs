#![cfg(unix)]

use std::{
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

use pcloud_ipc::{IpcClient, Method, Request, ResponseStatus};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_pcloudd")
}

fn run(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env("PCLOUD_ROOT", root)
        .env("PCLOUD_ENV", "development")
        .output()
        .expect("run pcloudd")
}

#[test]
fn help_version_unknown_and_summary_modes_are_installable() {
    let root = tempfile::tempdir().expect("temporary daemon root");
    for args in [
        vec!["--help"],
        vec!["help"],
        vec!["--version"],
        vec!["version"],
    ] {
        let output = run(root.path(), &args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
    }
    let unknown = run(root.path(), &["unknown"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown argument"));

    let summary = run(root.path(), &[]);
    assert!(
        summary.status.success(),
        "{}",
        String::from_utf8_lossy(&summary.stderr)
    );
    assert!(String::from_utf8_lossy(&summary.stdout).contains("daemon runtime ready"));
}

#[test]
fn serve_mode_writes_pid_accepts_ipc_and_shuts_down_cleanly() {
    let root = tempfile::tempdir().expect("temporary daemon root");
    let socket = root.path().join("runtime/pcloud.sock");
    let pidfile = root.path().join("state/daemon.pid");
    let mut child = Command::new(binary())
        .arg("serve")
        .env("PCLOUD_ROOT", root.path())
        .env("PCLOUD_ENV", "development")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() || !pidfile.exists() {
        if let Some(status) = child.try_wait().expect("poll daemon") {
            let output = child.wait_with_output().expect("daemon output");
            panic!(
                "daemon exited early with {status}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            Instant::now() < deadline,
            "daemon socket and pidfile did not both appear"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(pidfile.exists());
    let pid = std::fs::read_to_string(&pidfile).expect("read pidfile");
    assert_eq!(pid.trim().parse::<u32>().expect("numeric pid"), child.id());

    let client = IpcClient;
    let health = client
        .send(
            &socket,
            &Request::Plain {
                method: Method::GetHealth,
            },
        )
        .expect("health IPC");
    assert_eq!(health.status, ResponseStatus::Ok);

    let shutdown = client
        .send(
            &socket,
            &Request::Plain {
                method: Method::Shutdown,
            },
        )
        .expect("shutdown IPC");
    assert_eq!(shutdown.status, ResponseStatus::Ok);

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll daemon shutdown") {
            break status;
        }
        assert!(Instant::now() < deadline, "daemon did not shut down");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "daemon exit status: {status}");
    assert!(!pidfile.exists(), "pidfile must be removed on shutdown");
}

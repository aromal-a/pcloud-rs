#![cfg(unix)]

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_pcloud-web")
}

fn run(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(binary());
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run pcloud-web")
}

fn unused_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .expect("reserve local address")
        .local_addr()
        .expect("local address")
}

#[test]
fn help_version_and_invalid_cli_inputs_exit_deterministically() {
    for args in [vec!["--help"], vec!["-h"], vec!["--version"], vec!["-V"]] {
        let output = run(&args, &[]);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
    }

    let dir = tempfile::tempdir().expect("temporary token root");
    let empty = dir.path().join("empty-token");
    std::fs::write(&empty, "\n").expect("empty token fixture");
    let missing = dir.path().join("missing-token");
    let cases = [
        vec!["--unknown"],
        vec!["--bind"],
        vec!["--bind=not-an-address"],
        vec!["--socket"],
        vec!["--token"],
        vec!["--token=", "--socket=/tmp/daemon.sock"],
        vec!["--token=one", "--web-token=two"],
        vec!["--allow-host="],
        vec![
            "--token-file",
            missing.to_str().expect("UTF-8 missing path"),
            "--socket=/tmp/daemon.sock",
        ],
        vec![
            "--token-file",
            empty.to_str().expect("UTF-8 empty path"),
            "--socket=/tmp/daemon.sock",
        ],
    ];
    for args in cases {
        let output = run(&args, &[]);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("pcloud-web:"),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let no_paths = run(
        &["--bind=127.0.0.1:0", "--token=fixture"],
        &[
            ("HOME", ""),
            ("XDG_RUNTIME_DIR", ""),
            ("XDG_CACHE_HOME", ""),
        ],
    );
    assert!(!no_paths.status.success());
    assert!(String::from_utf8_lossy(&no_paths.stderr).contains("default socket path"));
}

#[test]
fn serve_mode_binds_and_exposes_health_and_readiness() {
    let root = tempfile::tempdir().expect("temporary web root");
    let token_file = root.path().join("web-token");
    std::fs::write(&token_file, "fixture-web-token\r\n").expect("token fixture");
    let addr = unused_addr();
    let mut child = Command::new(binary())
        .args([
            format!("--bind={addr}"),
            format!("--socket={}", root.path().join("missing.sock").display()),
            format!("--token-file={}", token_file.display()),
            "--allow-host=coverage.local".to_owned(),
            "--not-ready".to_owned(),
        ])
        .env("PCLOUD_ROOT", root.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn web server");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(addr) {
            Ok(stream) => break stream,
            Err(error) if Instant::now() < deadline => {
                let _ = child.try_wait().expect("poll child").map(|status| {
                    panic!("web server exited early with {status}: {error}");
                });
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("web server did not bind: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: coverage.local\r\nConnection: close\r\n\r\n")
        .expect("write HTTP request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read HTTP response");
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");

    child.kill().expect("stop web server");
    let status = child.wait().expect("reap web server");
    assert!(!status.success());
}

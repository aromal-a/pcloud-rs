#![cfg(unix)]

use std::{
    io::Write as _,
    process::{Command, Output, Stdio},
};

use pcloud_ipc::{
    IpcServer, ListFolderEntry, RemoteDownloadPayload, RemoteUploadPayload, Response,
    ResponseStatus,
};

fn run_with_responses(args: &[&str], responses: Vec<Response>) -> Output {
    let root = tempfile::tempdir().expect("temporary CLI root");
    run_at_root(root.path(), args, responses, &[])
}

fn run_at_root(
    root: &std::path::Path,
    args: &[&str],
    responses: Vec<Response>,
    extra_env: &[(&str, &str)],
) -> Output {
    let runtime = root.join("runtime");
    std::fs::create_dir_all(&runtime).expect("runtime directory");
    let socket = runtime.join("pcloud.sock");
    let server = IpcServer::new(pcloud_ipc::current_effective_uid());
    let bound = server.bind(&socket).expect("bind fake daemon");
    bound
        .set_accept_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("set fake daemon timeout");
    let expected_responses = responses.len();
    let server_thread = std::thread::spawn(move || {
        let mut served = 0;
        for response in responses {
            match bound.serve_once(move |_| response) {
                Ok(()) => served += 1,
                Err(pcloud_ipc::IpcTransportError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("serve fake daemon response: {error}"),
            }
        }
        served
    });

    let mut command = Command::new(env!("CARGO_BIN_EXE_pcloudc"));
    command
        .args(args)
        .env("PCLOUD_ROOT", root)
        .env_remove("PCLOUD_TRACEPARENT");
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let output = command.output().expect("run pcloudc");
    let served = server_thread.join().expect("fake daemon thread");
    assert_eq!(
        served,
        expected_responses,
        "CLI did not issue the expected request(s); args={args:?}, stderr={}",
        stderr(&output)
    );
    output
}

fn run_at_root_with_input(
    root: &std::path::Path,
    args: &[&str],
    responses: Vec<Response>,
    extra_env: &[(&str, &str)],
    input: &str,
) -> Output {
    let runtime = root.join("runtime");
    std::fs::create_dir_all(&runtime).expect("runtime directory");
    let socket = runtime.join("pcloud.sock");
    let bound = IpcServer::new(pcloud_ipc::current_effective_uid())
        .bind(&socket)
        .expect("bind fake daemon");
    bound
        .set_accept_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("set fake daemon timeout");
    let expected_responses = responses.len();
    let server_thread = std::thread::spawn(move || {
        let mut served = 0;
        for response in responses {
            match bound.serve_once(move |_| response) {
                Ok(()) => served += 1,
                Err(pcloud_ipc::IpcTransportError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("serve fake daemon response: {error}"),
            }
        }
        served
    });

    let mut command = Command::new(env!("CARGO_BIN_EXE_pcloudc"));
    command
        .args(args)
        .env("PCLOUD_ROOT", root)
        .env_remove("PCLOUD_TRACEPARENT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("spawn pcloudc");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("write pcloudc stdin");
    let output = child.wait_with_output().expect("wait for pcloudc");
    let served = server_thread.join().expect("fake daemon thread");
    assert_eq!(
        served,
        expected_responses,
        "CLI did not issue the expected request(s); args={args:?}, stderr={}",
        stderr(&output)
    );
    output
}

fn run_without_server(root: &std::path::Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pcloudc"));
    command
        .args(args)
        .env("PCLOUD_ROOT", root)
        .env_remove("PCLOUD_TRACEPARENT");
    for (name, value) in extra_env {
        command.env(name, value);
    }
    command.output().expect("run pcloudc")
}

fn one(args: &[&str], status: ResponseStatus, message: impl Into<String>) -> Output {
    run_with_responses(
        args,
        vec![Response {
            status,
            message: message.into(),
        }],
    )
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("UTF-8 stderr")
}

#[test]
fn global_help_version_completion_and_hint_render_without_daemon() {
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["completion", "bash"],
        vec!["help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pcloudc"))
            .args(&args)
            .output()
            .expect("run local CLI command");
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
        assert!(!stdout(&output).is_empty(), "{args:?}");
    }

    let hint = Command::new(env!("CARGO_BIN_EXE_pcloudc"))
        .output()
        .expect("run zero-argument CLI");
    assert!(hint.status.success());
    assert!(stdout(&hint).contains("pcloud is idle"));

    let json_hint = Command::new(env!("CARGO_BIN_EXE_pcloudc"))
        .args(["--json"])
        .output()
        .expect("run JSON hint");
    assert!(json_hint.status.success());
    let body: serde_json::Value = serde_json::from_slice(&json_hint.stdout).unwrap();
    assert_eq!(body["command"], "hint");

    let bad_completion = Command::new(env!("CARGO_BIN_EXE_pcloudc"))
        .args(["--json", "completion", "nushell"])
        .output()
        .expect("run invalid completion");
    assert!(!bad_completion.status.success());
    assert!(stdout(&bad_completion).contains("completion requires a shell"));
}

#[test]
fn doctor_text_json_quiet_and_strict_modes_execute_without_a_daemon() {
    let root = tempfile::tempdir().expect("temporary CLI root");
    let text = run_without_server(root.path(), &["doctor"], &[]);
    assert!(!text.status.success());
    assert!(stdout(&text).contains("daemon unreachable"));
    assert!(stdout(&text).contains("summary:"));

    let json = run_without_server(root.path(), &["--json", "doctor", "--strict"], &[]);
    assert!(!json.status.success());
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert!(report["checks"].is_array());
    assert!(report["summary"]["fail"].as_u64().unwrap() >= 1);

    let quiet = run_without_server(root.path(), &["--quiet", "doctor"], &[]);
    assert!(!quiet.status.success());
    assert!(quiet.stdout.is_empty());
}

#[test]
fn verify_cli_walks_files_renders_text_json_quiet_and_usage_failures() {
    let root = tempfile::tempdir().expect("verify root");
    let file = root.path().join("one.txt");
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(&file, b"one").unwrap();
    std::fs::write(nested.join("two.txt"), b"two").unwrap();

    let missing_path = run_without_server(root.path(), &["verify"], &[]);
    assert!(!missing_path.status.success());
    assert!(stderr(&missing_path).contains("local path is required"));

    let text = run_without_server(root.path(), &["verify", file.to_str().unwrap()], &[]);
    assert!(!text.status.success());
    assert!(stdout(&text).contains("[MISSING_REMOTE]"));
    assert!(stdout(&text).contains("one.txt"));

    let json = run_without_server(
        root.path(),
        &[
            "--json",
            "verify",
            root.path().to_str().unwrap(),
            "--recursive",
            "--fix",
            "--yes",
        ],
        &[],
    );
    assert!(!json.status.success());
    assert_eq!(stdout(&json).lines().count(), 2);
    assert!(stdout(&json).contains("\"status\":\"missing_remote\""));

    let quiet = run_without_server(
        root.path(),
        &["--quiet", "verify", file.to_str().unwrap()],
        &[],
    );
    assert!(!quiet.status.success());
    assert!(quiet.stdout.is_empty());

    let directory_without_recursive =
        run_without_server(root.path(), &["verify", root.path().to_str().unwrap()], &[]);
    assert!(!directory_without_recursive.status.success());
    assert!(stdout(&directory_without_recursive).contains("one.txt"));
    assert!(!stdout(&directory_without_recursive).contains("two.txt"));
}

#[test]
fn text_json_and_verbosity_render_success_responses() {
    let plain = one(&["status"], ResponseStatus::Ok, "daemon ready");
    assert!(plain.status.success());
    assert_eq!(stdout(&plain).trim(), "daemon ready");

    let verbose = one(&["-v", "status"], ResponseStatus::Ok, "daemon ready");
    assert!(verbose.status.success());
    assert!(stdout(&verbose).contains("Ok"));
    assert!(stdout(&verbose).contains("daemon ready"));

    let very_verbose = one(&["-vv", "status"], ResponseStatus::Ok, "daemon ready");
    assert!(very_verbose.status.success());
    assert!(stdout(&very_verbose).contains("command="));
    assert!(stdout(&very_verbose).contains("message=daemon ready"));

    let json = one(
        &["--json", "status"],
        ResponseStatus::Ok,
        r#"{"ready":true}"#,
    );
    assert!(json.status.success());
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["message"], r#"{"ready":true}"#);

    let quiet = one(&["--quiet", "status"], ResponseStatus::Ok, "hidden");
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty());
    assert!(quiet.stderr.is_empty());
}

#[test]
fn response_statuses_and_transport_failures_map_to_stable_process_errors() {
    for status in [
        ResponseStatus::InvalidRequest,
        ResponseStatus::Unauthorized,
        ResponseStatus::Conflict,
        ResponseStatus::Unavailable,
        ResponseStatus::InternalError,
        ResponseStatus::PolicyViolation {
            kind: "coverage".to_owned(),
        },
    ] {
        let output = one(&["status"], status.clone(), format!("fixture {status:?}"));
        assert!(!output.status.success(), "{status:?}");
        assert!(stdout(&output).contains("fixture"), "{status:?}");
    }

    let root = tempfile::tempdir().unwrap();
    let missing = Command::new(env!("CARGO_BIN_EXE_pcloudc"))
        .args(["--quiet", "status"])
        .env("PCLOUD_ROOT", root.path())
        .output()
        .expect("run without daemon");
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(missing.stderr.is_empty());

    let json_error = one(
        &["--json", "status"],
        ResponseStatus::Unauthorized,
        "authentication required",
    );
    assert!(!json_error.status.success());
    assert!(json_error.stderr.is_empty());
    let body: serde_json::Value = serde_json::from_slice(&json_error.stdout).unwrap();
    assert_eq!(body["status"], "unauthorized");
    assert_eq!(body["message"], "authentication required");
}

#[test]
fn remote_listing_and_transfer_receipts_have_human_renderers() {
    let listing = serde_json::to_string(&vec![
        ListFolderEntry {
            file_id: 7,
            name: "hello.txt".to_owned(),
            size: 5,
            hash: "abc".to_owned(),
            modified: 1,
            created: 1,
            is_folder: false,
            is_mine: true,
            is_shared: false,
            encrypted: false,
            permissions: Some(7),
        },
        ListFolderEntry {
            file_id: 8,
            name: "Documents".to_owned(),
            size: 0,
            hash: String::new(),
            modified: 1,
            created: 1,
            is_folder: true,
            is_mine: true,
            is_shared: true,
            encrypted: false,
            permissions: Some(7),
        },
    ])
    .unwrap();
    let ls = one(&["ls", "/"], ResponseStatus::Ok, listing);
    assert!(ls.status.success());
    assert!(stdout(&ls).contains("-\t5\thello.txt"));
    assert!(stdout(&ls).contains("d\t0\tDocuments"));

    let malformed_ls = one(&["ls", "/"], ResponseStatus::Ok, "{bad json");
    assert!(!malformed_ls.status.success());
    assert!(stderr(&malformed_ls).contains("malformed listing"));

    let download = RemoteDownloadPayload {
        path: "/tmp/hello.txt".into(),
        bytes: 5,
        sha256_hex: "abc123".to_owned(),
        resumed_from: 2,
    };
    let get = one(
        &["get", "/hello.txt", "/tmp/hello.txt"],
        ResponseStatus::Ok,
        serde_json::to_string(&download).unwrap(),
    );
    assert!(get.status.success());
    assert!(stdout(&get).contains("downloaded 5 bytes"));
    assert!(stdout(&get).contains("resumed_from=2"));

    let upload = RemoteUploadPayload {
        upload_id: 9,
        file_id: None,
        bytes: 5,
        sha1_hex: "def456".to_owned(),
        resumed_from: 0,
    };
    let source = tempfile::NamedTempFile::new().unwrap();
    let put = one(
        &["put", source.path().to_str().unwrap(), "/remote/hello.txt"],
        ResponseStatus::Ok,
        serde_json::to_string(&upload).unwrap(),
    );
    assert!(put.status.success());
    assert!(stdout(&put).contains("file_id=unknown"));
    assert!(!stdout(&put).contains("resumed_from"));

    let malformed = one(
        &["get", "/hello.txt", "/tmp/hello.txt"],
        ResponseStatus::Ok,
        "not-json",
    );
    assert!(!malformed.status.success());
    assert!(stderr(&malformed).contains("malformed download receipt"));
}

#[test]
fn field_selection_covers_plain_json_missing_and_type_error_paths() {
    let plain = one(
        &["status", "--field", "quota", "--field", "usedquota"],
        ResponseStatus::Ok,
        "status: quota=42, usedquota=7",
    );
    assert!(plain.status.success());
    assert_eq!(stdout(&plain), "42\n7\n");

    let json = one(
        &["--json", "status", "--field", "nested.value"],
        ResponseStatus::Ok,
        r#"{"nested":{"value":9}}"#,
    );
    assert!(json.status.success());
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["fields"]["nested.value"], 9);

    let missing = one(
        &["status", "--field", "missing"],
        ResponseStatus::Ok,
        "status: quota=42, usedquota=7",
    );
    assert!(!missing.status.success());
    assert!(!stderr(&missing).is_empty());

    let type_error = one(
        &["--json", "status", "--field", "quota.child"],
        ResponseStatus::Ok,
        r#"{"quota":42}"#,
    );
    assert!(!type_error.status.success());
    let body: serde_json::Value = serde_json::from_slice(&type_error.stdout).unwrap();
    assert!(body.is_object());
    assert!(body.to_string().contains("quota"));
}

#[test]
fn crypto_error_translation_backend_labels_and_trace_context_render() {
    let status = one(
        &["crypto-status"],
        ResponseStatus::Ok,
        "crypto unlocked backend=pclsync-compat",
    );
    assert!(status.status.success());
    assert!(stdout(&status).contains("Backend: pclsync-compat"));

    let unlock = one(
        &[
            "unlock-crypto",
            "secret",
            "--allow-argv-password",
            "--trace-id",
            "4bf92f3577b34da6a3ce929d0e0e4736",
        ],
        ResponseStatus::InternalError,
        "result=2110",
    );
    assert!(!unlock.status.success());
    assert!(stdout(&unlock).contains("crypto already set up"));
    assert!(stderr(&unlock).contains("[trace: 00-4bf92f"));

    let unknown = one(
        &["crypto-status"],
        ResponseStatus::InternalError,
        "result=9999: backend unavailable",
    );
    assert!(!unknown.status.success());
    assert!(stdout(&unknown).contains("result=9999"));
}

#[test]
fn remote_cat_streams_multiple_ranges_and_rejects_json_mode() {
    use base64::Engine as _;

    let first = pcloud_ipc::ReadRangePayload {
        data_b64: base64::engine::general_purpose::STANDARD.encode(b"hello "),
        bytes_returned: 6,
        total_size: 11,
        eof: false,
    };
    let second = pcloud_ipc::ReadRangePayload {
        data_b64: base64::engine::general_purpose::STANDARD.encode(b"world"),
        bytes_returned: 5,
        total_size: 11,
        eof: true,
    };
    let output = run_with_responses(
        &["cat", "/hello.txt"],
        vec![
            Response {
                status: ResponseStatus::Ok,
                message: serde_json::to_string(&first).unwrap(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: serde_json::to_string(&second).unwrap(),
            },
        ],
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello world");

    let json = Command::new(env!("CARGO_BIN_EXE_pcloudc"))
        .args(["--json", "cat", "/hello.txt"])
        .output()
        .expect("run JSON cat");
    assert!(!json.status.success());
    assert!(stdout(&json).contains("cannot be combined with --json"));
}

#[test]
fn remote_cat_rejects_malformed_nonprogressing_and_failed_ranges() {
    use base64::Engine as _;

    for (response, expected) in [
        (
            Response {
                status: ResponseStatus::Unavailable,
                message: "remote unavailable".to_owned(),
            },
            "remote unavailable",
        ),
        (
            Response {
                status: ResponseStatus::Ok,
                message: "{not-json".to_owned(),
            },
            "malformed range metadata",
        ),
        (
            Response {
                status: ResponseStatus::Ok,
                message: serde_json::to_string(&pcloud_ipc::ReadRangePayload {
                    data_b64: "%%%".to_owned(),
                    bytes_returned: 1,
                    total_size: 1,
                    eof: true,
                })
                .unwrap(),
            },
            "malformed base64",
        ),
        (
            Response {
                status: ResponseStatus::Ok,
                message: serde_json::to_string(&pcloud_ipc::ReadRangePayload {
                    data_b64: base64::engine::general_purpose::STANDARD.encode(b"x"),
                    bytes_returned: 2,
                    total_size: 2,
                    eof: true,
                })
                .unwrap(),
            },
            "range length does not match",
        ),
        (
            Response {
                status: ResponseStatus::Ok,
                message: serde_json::to_string(&pcloud_ipc::ReadRangePayload {
                    data_b64: String::new(),
                    bytes_returned: 0,
                    total_size: 1,
                    eof: false,
                })
                .unwrap(),
            },
            "made no progress",
        ),
    ] {
        let output = run_with_responses(&["cat", "/fixture"], vec![response]);
        assert!(!output.status.success());
        assert!(stderr(&output).contains(expected), "{}", stderr(&output));
    }

    let quiet = run_with_responses(
        &["--quiet", "cat", "/fixture"],
        vec![Response {
            status: ResponseStatus::Ok,
            message: serde_json::to_string(&pcloud_ipc::ReadRangePayload {
                data_b64: base64::engine::general_purpose::STANDARD.encode(b"hidden"),
                bytes_returned: 6,
                total_size: 6,
                eof: true,
            })
            .unwrap(),
        }],
    );
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty());
}

#[test]
fn start_drain_and_reload_cover_idempotent_and_pidfile_paths() {
    let root = tempfile::tempdir().unwrap();
    let start = run_at_root(
        root.path(),
        &["start"],
        vec![Response {
            status: ResponseStatus::Ok,
            message: "health: ready".to_owned(),
        }],
        &[],
    );
    assert!(start.status.success());
    assert!(stdout(&start).contains("already running"));

    let missing_drain = run_without_server(root.path(), &["--json", "drain"], &[]);
    assert!(!missing_drain.status.success());
    assert!(stdout(&missing_drain).contains("pidfile"));

    std::fs::create_dir_all(root.path().join("state")).unwrap();
    std::fs::write(root.path().join("state/daemon.pid"), "not-a-pid\n").unwrap();
    let malformed_drain = run_without_server(root.path(), &["drain"], &[]);
    assert!(!malformed_drain.status.success());
    assert!(stderr(&malformed_drain).contains("non-numeric"));

    std::fs::write(root.path().join("state/daemon.pid"), "4294967294\n").unwrap();
    let stopped = run_without_server(root.path(), &["drain"], &[]);
    assert!(stopped.status.success());
    assert!(stdout(&stopped).contains("already stopped"));

    let reload_missing_process = run_without_server(root.path(), &["reload"], &[]);
    assert!(!reload_missing_process.status.success());
    assert!(stderr(&reload_missing_process).contains("SIGHUP"));

    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    std::fs::write(
        root.path().join("state/daemon.pid"),
        format!("{}\n", child.id()),
    )
    .unwrap();
    let reload = run_without_server(root.path(), &["reload"], &[]);
    assert!(reload.status.success());
    assert!(stderr(&reload).contains("hot-reload requested"));
    let _ = child.wait();

    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    std::fs::write(
        root.path().join("state/daemon.pid"),
        format!("{}\n", child.id()),
    )
    .unwrap();
    let drain = run_at_root(
        root.path(),
        &["drain"],
        vec![Response {
            status: ResponseStatus::Ok,
            message: r#"{"state":"stopped","in_flight":0}"#.to_owned(),
        }],
        &[],
    );
    assert!(drain.status.success());
    assert!(stdout(&drain).contains("drain complete"));
    let _ = child.wait();
}

#[test]
fn start_honors_pcloudd_override_and_waits_for_readiness() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let fake_daemon = root.path().join("fixture-pcloudd");
    std::fs::write(&fake_daemon, "#!/bin/sh\nsleep 1\n").unwrap();
    std::fs::set_permissions(&fake_daemon, std::fs::Permissions::from_mode(0o700)).unwrap();

    let runtime = root.path().join("runtime");
    let socket = runtime.join("pcloud.sock");
    let readiness = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        std::fs::create_dir_all(&runtime).unwrap();
        let bound = IpcServer::new(pcloud_ipc::current_effective_uid())
            .bind(&socket)
            .expect("bind readiness fixture");
        bound
            .serve_once(|_| Response {
                status: ResponseStatus::Ok,
                message: "health: ready".to_owned(),
            })
            .expect("serve readiness probe");
    });

    let config_path = root.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "mountpoint = {:?}\n\
             fuse_opts = \"ro,noatime\"\n\
             log_path = {:?}\n\
             fs_event_log = {:?}\n\
             log_level = \"debug\"\n\
             cache_size_gb = 12\n",
            root.path().join("mount").display().to_string(),
            root.path().join("configured.log").display().to_string(),
            root.path().join("fs-events.log").display().to_string(),
        ),
    )
    .unwrap();
    let output = run_without_server(
        root.path(),
        &["start"],
        &[
            ("PCLOUDD", fake_daemon.to_str().unwrap()),
            ("PCLOUD_CONFIG", config_path.to_str().unwrap()),
        ],
    );
    readiness.join().unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("pcloudd started"));
    let log = root.path().join("state/daemon.log");
    assert!(log.is_file());
    assert_eq!(
        std::fs::metadata(log).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let missing = root.path().join("missing-pcloudd");
    let failed = run_without_server(
        root.path(),
        &["--json", "start"],
        &[("PCLOUDD", missing.to_str().unwrap())],
    );
    assert!(!failed.status.success());
    assert!(stdout(&failed).contains("does not exist"));

    let non_executable = root.path().join("non-executable-pcloudd");
    std::fs::write(&non_executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o600)).unwrap();
    let spawn_failed = run_without_server(
        &root.path().join("spawn-failure"),
        &["start"],
        &[("PCLOUDD", non_executable.to_str().unwrap())],
    );
    assert!(!spawn_failed.status.success());
    assert!(stderr(&spawn_failed).contains("failed to spawn pcloudd"));

    let bad_state_root = root.path().join("bad-state-root");
    std::fs::create_dir_all(&bad_state_root).unwrap();
    std::fs::write(bad_state_root.join("state"), b"not a directory").unwrap();
    let log_dir_failed = run_without_server(
        &bad_state_root,
        &["start"],
        &[("PCLOUDD", fake_daemon.to_str().unwrap())],
    );
    assert!(!log_dir_failed.status.success());
    assert!(stderr(&log_dir_failed).contains("log dir create failed"));

    let bad_log_root = root.path().join("bad-log-root");
    std::fs::create_dir_all(bad_log_root.join("state/daemon.log")).unwrap();
    let log_open_failed = run_without_server(
        &bad_log_root,
        &["start"],
        &[("PCLOUDD", fake_daemon.to_str().unwrap())],
    );
    assert!(!log_open_failed.status.success());
    assert!(stderr(&log_open_failed).contains("log open failed"));
}

#[test]
fn login_covers_noninteractive_success_and_existing_session_paths() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let xdg_config = root.path().join("xdg-config");
    let xdg_data = root.path().join("xdg-data");
    let xdg_cache = root.path().join("xdg-cache");
    let xdg_runtime = root.path().join("xdg-runtime");
    for directory in [&home, &xdg_config, &xdg_data, &xdg_cache, &xdg_runtime] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let isolated_env = [
        ("HOME", home.to_str().unwrap()),
        ("XDG_CONFIG_HOME", xdg_config.to_str().unwrap()),
        ("XDG_DATA_HOME", xdg_data.to_str().unwrap()),
        ("XDG_CACHE_HOME", xdg_cache.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", xdg_runtime.to_str().unwrap()),
        ("PCLOUD_COVERAGE_PASSWORD", "fixture-password"),
    ];
    let login = run_at_root(
        root.path(),
        &[
            "-v",
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_PASSWORD",
        ],
        vec![
            Response {
                status: ResponseStatus::Ok,
                message: "LoginSucceeded".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "auth=Authenticated LoginSucceeded".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "userinfo: email=alice@example.com".to_owned(),
            },
        ],
        &isolated_env,
    );
    assert!(login.status.success(), "{}", stderr(&login));

    let existing = run_at_root(
        root.path(),
        &["login"],
        vec![
            Response {
                status: ResponseStatus::Ok,
                message: "auth=Authenticated".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "userinfo: email=alice@example.com".to_owned(),
            },
        ],
        &isolated_env,
    );
    assert!(existing.status.success());
    assert!(stdout(&existing).contains("Already authenticated"));

    let missing_env = run_without_server(
        root.path(),
        &[
            "--json",
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_MISSING",
        ],
        &isolated_env,
    );
    assert!(!missing_env.status.success());
    assert!(stdout(&missing_env).contains("is not set"));

    let log_path = root.path().join("configured.log");
    let fs_event_log = root.path().join("fs-events.log");
    let configured = run_without_server(
        root.path(),
        &[
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_MISSING",
            "--log-path",
            log_path.to_str().unwrap(),
            "--fs-event-log",
            fs_event_log.to_str().unwrap(),
            "--log-level",
            "debug",
            "--fuse-opts",
            "ro,noatime",
            "--cache-size",
            "12",
        ],
        &isolated_env,
    );
    assert!(!configured.status.success());
    assert!(stderr(&configured).contains("apply on next daemon start"));
    let config_text =
        std::fs::read_to_string(xdg_config.join("pcloud-rs").join("config.toml")).unwrap();
    assert!(config_text.contains("fs-events.log"));
    assert!(config_text.contains("cache_size_gb = 12"));
}

#[test]
fn login_existing_session_runs_vault_crypto_mount_and_userinfo_actions() {
    let root = tempfile::tempdir().unwrap();
    let mountpoint = root.path().join("existing-session-mount");
    let output = run_at_root_with_input(
        root.path(),
        &[
            "-v",
            "login",
            "--save-password",
            "--crypto",
            "--mountpoint",
            mountpoint.to_str().unwrap(),
        ],
        vec![
            Response {
                status: ResponseStatus::Ok,
                message: "auth=Authenticated".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "authsave enabled".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "crypto unlocked".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "mounted".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "userinfo: alice@example.com".to_owned(),
            },
        ],
        &[],
        "crypto-password\n",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(mountpoint.is_dir());
    assert!(stdout(&output).contains("Already authenticated"));
    assert!(stdout(&output).contains("userinfo"));
    assert!(stderr(&output).contains("authsave enabled"));
    assert!(stderr(&output).contains("crypto unlocked"));
}

#[test]
fn login_covers_two_factor_retries_channels_and_post_login_actions() {
    let root = tempfile::tempdir().unwrap();
    let mountpoint = root.path().join("mount");
    let mountpoint_arg = mountpoint.to_str().unwrap();
    let responses = vec![
        Response {
            status: ResponseStatus::Ok,
            message: "TwoFactorChallengeIssued".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "SMS sent".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "auth=TwoFactorRequired".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "SMS resent".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "push sent".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "TwoFactorChallengeIssued".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "SMS sent".to_owned(),
        },
        Response {
            status: ResponseStatus::Unauthorized,
            message: "challenge expired".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "TwoFactorChallengeIssued".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "SMS sent".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "LoginSucceeded".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "auth persistence enabled".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "crypto unlocked".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "mounted".to_owned(),
        },
        Response {
            status: ResponseStatus::Ok,
            message: "userinfo fixture".to_owned(),
        },
    ];
    let output = run_at_root_with_input(
        root.path(),
        &[
            "-v",
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_PASSWORD",
            "--tfa-channel",
            "sms",
            "--passascrypto",
            "--trust-device",
            "--save-password",
            "--mountpoint",
            mountpoint_arg,
        ],
        responses,
        &[("PCLOUD_COVERAGE_PASSWORD", "fixture-password")],
        "not-a-code\nsms\npush\nresend\n654321\n123456\n",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(mountpoint.is_dir());
    assert!(!stderr(&output).contains("fixture-password"));
}

#[test]
fn login_transport_and_post_action_failures_map_to_stable_exit_paths() {
    let root = tempfile::tempdir().unwrap();
    let password_env = [("PCLOUD_COVERAGE_PASSWORD", "fixture-password")];

    let submit_failure = run_without_server(
        root.path(),
        &[
            "--json",
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_PASSWORD",
        ],
        &password_env,
    );
    assert!(!submit_failure.status.success());
    assert!(stdout(&submit_failure).contains("submit-password dispatch failed"));

    let status_failure = run_at_root(
        root.path(),
        &[
            "--json",
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_PASSWORD",
        ],
        vec![Response {
            status: ResponseStatus::Ok,
            message: "LoginSucceeded".to_owned(),
        }],
        &password_env,
    );
    assert!(!status_failure.status.success());
    assert!(stdout(&status_failure).contains("status after password dispatch failed"));

    let tfa_failure = run_at_root_with_input(
        root.path(),
        &[
            "--json",
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_PASSWORD",
            "--tfa-channel",
            "push",
        ],
        vec![
            Response {
                status: ResponseStatus::Ok,
                message: "TwoFactorChallengeIssued".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "push sent".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "auth=TwoFactorRequired".to_owned(),
            },
        ],
        &password_env,
        "123456\n",
    );
    assert!(!tfa_failure.status.success());
    assert!(stdout(&tfa_failure).contains("submit-tfa dispatch failed"));

    let mountpoint = root.path().join("mount-after-login");
    let mount_failure = run_at_root(
        root.path(),
        &[
            "login",
            "--username",
            "alice@example.com",
            "--password-env",
            "PCLOUD_COVERAGE_PASSWORD",
            "--mountpoint",
            mountpoint.to_str().unwrap(),
        ],
        vec![
            Response {
                status: ResponseStatus::Ok,
                message: "LoginSucceeded".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "auth=Authenticated LoginSucceeded".to_owned(),
            },
        ],
        &password_env,
    );
    assert!(!mount_failure.status.success());
    assert!(stderr(&mount_failure).contains("mount failed"));

    let crypto_prompt_failure = run_at_root_with_input(
        root.path(),
        &["login", "--crypto"],
        vec![Response {
            status: ResponseStatus::Ok,
            message: "auth=Authenticated".to_owned(),
        }],
        &[],
        "",
    );
    assert!(!crypto_prompt_failure.status.success());
    assert!(stderr(&crypto_prompt_failure).contains("crypto prompt failed"));
}

#[test]
fn login_covers_piped_username_password_and_eof_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let piped = run_at_root_with_input(
        root.path(),
        &["--quiet", "login", "--password-stdin"],
        vec![
            Response {
                status: ResponseStatus::Ok,
                message: "LoginSucceeded".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "auth=Authenticated LoginSucceeded".to_owned(),
            },
            Response {
                status: ResponseStatus::Ok,
                message: "userinfo fixture".to_owned(),
            },
        ],
        &[],
        "alice@example.com\nfixture-password\n",
    );
    assert!(piped.status.success(), "{}", stderr(&piped));
    assert!(!stdout(&piped).contains("fixture-password"));

    let cancelled = Command::new(env!("CARGO_BIN_EXE_pcloudc"))
        .args(["login"])
        .env("PCLOUD_ROOT", root.path().join("cancelled"))
        .stdin(Stdio::null())
        .output()
        .expect("run EOF login");
    assert!(!cancelled.status.success());
    assert!(stderr(&cancelled).contains("login cancelled"));
}

#[test]
fn migrate_command_covers_preview_execute_conflict_and_force() {
    use rusqlite::{Connection, params};

    let root = tempfile::tempdir().unwrap();
    let legacy = root.path().join("legacy");
    std::fs::create_dir_all(&legacy).unwrap();
    let connection = Connection::open(legacy.join(".pclouddb")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE setting (id TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE syncfolder (
                 id INTEGER PRIMARY KEY,
                 folderid INTEGER,
                 localpath TEXT,
                 synctype INTEGER,
                 flags INTEGER,
                 inode INTEGER,
                 deviceid INTEGER
             );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO setting (id, value) VALUES ('auth', 'fixture-token')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO syncfolder
             (id, folderid, localpath, synctype, flags, inode, deviceid)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 0)",
            params![1_i64, 42_i64, "/tmp/coverage-sync", 3_i64],
        )
        .unwrap();
    drop(connection);

    let legacy_arg = legacy.to_str().unwrap();
    let home = root.path().join("home");
    let xdg_config = root.path().join("xdg-config");
    let xdg_data = root.path().join("xdg-data");
    let xdg_cache = root.path().join("xdg-cache");
    let xdg_runtime = root.path().join("xdg-runtime");
    for directory in [&home, &xdg_config, &xdg_data, &xdg_cache, &xdg_runtime] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let isolated_env = [
        ("HOME", home.to_str().unwrap()),
        ("XDG_CONFIG_HOME", xdg_config.to_str().unwrap()),
        ("XDG_DATA_HOME", xdg_data.to_str().unwrap()),
        ("XDG_CACHE_HOME", xdg_cache.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", xdg_runtime.to_str().unwrap()),
    ];
    let preview = run_without_server(
        root.path(),
        &["migrate-from-c", "--from", legacy_arg, "--dry-run"],
        &isolated_env,
    );
    assert!(preview.status.success(), "{}", stderr(&preview));
    assert!(stdout(&preview).contains("Rerun without --dry-run"));
    assert!(!stdout(&preview).contains("fixture-token"));

    let execute = run_without_server(
        root.path(),
        &["migrate-from-c", "--from", legacy_arg],
        &isolated_env,
    );
    assert!(execute.status.success(), "{}", stderr(&execute));
    assert!(stdout(&execute).contains("sync roots"));

    let conflict = run_without_server(
        root.path(),
        &["migrate-from-c", "--from", legacy_arg],
        &isolated_env,
    );
    assert!(!conflict.status.success());
    assert!(stderr(&conflict).contains("already present"));

    let forced = run_without_server(
        root.path(),
        &[
            "--json",
            "migrate-from-c",
            "--from",
            legacy_arg,
            "--force-overwrite",
        ],
        &isolated_env,
    );
    assert!(forced.status.success(), "{}", stderr(&forced));
    let body: serde_json::Value = serde_json::from_slice(&forced.stdout).unwrap();
    assert_eq!(body["status"], "ok");
}

#[test]
fn command_families_lower_to_ipc_and_render_success_end_to_end() {
    let commands: &[&[&str]] = &[
        &["health"],
        &["pending"],
        &["slo"],
        &["stop"],
        &["userinfo"],
        &["logout"],
        &["authsave", "on"],
        &["authsave", "off"],
        &["submit-auth", "fixture-token", "--allow-argv-password"],
        &[
            "submit-tfa",
            "123456",
            "--trust-device",
            "--allow-argv-password",
        ],
        &["submit-recovery", "recovery-code", "--allow-argv-password"],
        &["send-tfa-sms"],
        &["send-tfa-notification"],
        &["session", "status"],
        &["mount", "/tmp/pcloud-coverage-mount"],
        &["unmount"],
        &["fs", "status", "/tmp"],
        &["sync", "list"],
        &["sync", "status"],
        &["sync", "add", "/tmp", "/remote-sync", "--type", "full"],
        &["sync", "remove", "1"],
        &["sync", "change-type", "1", "mirror"],
        &["sync", "exclude", "add", "1", "*.tmp"],
        &["sync", "exclude", "remove", "1", "*.tmp"],
        &["sync", "exclude", "list", "1"],
        &["sync", "localscan"],
        &["sync", "pause"],
        &["sync", "resume"],
        &["sync", "suggest", "/tmp", "--max", "3"],
        &["sync", "is-syncable", "/tmp"],
        &["conflict", "list"],
        &["conflict", "resolve", "docs/report.txt", "--keep-local"],
        &["folder", "create", "/Coverage"],
        &["folder", "id", "/"],
        &["folder", "flags", "/"],
        &["folder", "owner", "/"],
        &["stat", "/"],
        &["cp", "/from", "/to"],
        &["mv", "/from", "/to"],
        &["rm", "/target", "--recursive"],
        &["mkdir", "/created"],
        &["list-links"],
        &["list-upload-links"],
        &["show-link", "fixture-code"],
        &["delete-link", "1"],
        &["delete-link", "fixture-code"],
        &["create-file-link", "/file.txt"],
        &["create-folder-link", "/folder"],
        &["change-link-expire", "1", "1970-01-02"],
        &[
            "change-link-password",
            "1",
            "fixture-password",
            "--allow-argv-password",
        ],
        &["change-link-upload", "1", "everyone"],
        &["create-upload-link", "/incoming"],
        &["delete-upload-link", "1"],
        &[
            "create-tree-link",
            "Selection",
            "1",
            "1,2",
            "3,4",
            "123",
            "7",
            "2048",
        ],
        &["list-link-access", "1"],
        &["add-link-access", "1", "alice@example.com"],
        &["remove-link-access", "1", "2"],
        &["list-bookmarks"],
        &["remove-bookmark", "fixture-code", "1"],
        &[
            "change-bookmark",
            "fixture-code",
            "1",
            "Coverage",
            "Description",
        ],
        &[
            "publink",
            "send",
            "fixture-code",
            "--to",
            "alice@example.com",
            "--message",
            "hello",
        ],
        &["list-incoming-shares"],
        &["list-outgoing-shares"],
        &["list-incoming-share-requests"],
        &["list-outgoing-share-requests"],
        &["list-contacts"],
        &["list-myteams"],
        &["share-folder", "1", "Docs", "alice@example.com", "7"],
        &["cancel-share-request", "1"],
        &["decline-share-request", "1"],
        &["accept-share-request", "1", "0", "Accepted"],
        &["remove-share", "1"],
        &["modify-share", "1", "7"],
        &["notifications", "list"],
        &["notifications", "mark-read", "9"],
        &["audit", "verify", "--from", "1", "--to", "9"],
        &["audit-verifier", "status"],
        &["integrity", "status"],
        &["integrity", "run-once"],
        &["integrity", "skip", "**/*.tmp"],
        &["ha", "status"],
        &["upload", "create", "/tmp/source", "remote.bin", "10"],
        &["upload", "pause", "1"],
        &["upload", "resume", "1"],
        &["upload", "cancel", "1"],
        &["upload", "list"],
        &["upload", "write-from-file", "1", "2", "3", "0", "0", "4"],
        &["crypto", "status"],
        &["crypto", "stop"],
        &["crypto", "reset"],
        &["crypto", "priv-key-flags"],
        &["crypto", "send-change-private"],
        &["crypto", "hint"],
        &["crypto", "get-folder-key", "1"],
        &["crypto", "get-file-key", "1"],
        &["account", "verify-email"],
        &["account", "verify-email-restricted", "fixture-token"],
        &["account", "lost-password", "alice@example.com"],
        &["account", "api-servers"],
        &["account", "set-api-server", "1", "binapi-eu.pcloud.com"],
        &["account", "set-language", "de"],
        &["account", "promo"],
        &["download", "link", "1"],
        &["download", "file", "1", "/tmp/pcloud-coverage-download"],
        &["backup", "delete", "1"],
        &["backup", "create", "Coverage", "1", "/tmp"],
        &["backup", "stop-device", "1"],
        &["backup", "delete-device"],
        &[
            "create-tree-link-from-paths",
            "Coverage",
            "--root",
            "/",
            "--folder",
            "/Documents",
            "--file",
            "/Documents/notes.txt",
        ],
        &["snapshot", "create", "/tmp/coverage.tar.zst"],
        &["snapshot", "restore", "/tmp/coverage.tar.zst", "--yes"],
        &["snapshot", "verify", "/tmp/coverage.tar.zst"],
        &[
            "snapshot",
            "prune",
            "/tmp",
            "--retention-days",
            "30",
            "--yes",
        ],
    ];

    for args in commands {
        let output = one(args, ResponseStatus::Ok, "fixture ok");
        assert!(
            output.status.success(),
            "command {args:?} failed: stdout={} stderr={}",
            stdout(&output),
            stderr(&output)
        );
    }
}

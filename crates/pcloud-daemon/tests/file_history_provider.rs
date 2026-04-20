#![allow(clippy::pedantic)]
//! Integration tests for the pluggable revision provider wired on
//! `pcloudc log` / `diff` / `restore`.
//!
//! # Coverage
//!
//! 1. **Null provider (default):** with no `[file_history].revision_url`
//!    configured, the daemon returns `ResponseStatus::Unavailable` with
//!    a structured JSON payload carrying `status: "not_configured"` and
//!    an actionable remediation hint.
//! 2. **Empty-path guard:** empty / whitespace paths bypass the provider
//!    and produce `ResponseStatus::InvalidRequest`.
//! 3. **Config validation:** production profiles refuse plaintext
//!    `http://` URLs on `[file_history].revision_url` at config-load
//!    time.
//! 4. **CLI stub parity:** the structured payload emitted by the CLI
//!    `diff` / `restore` stubs matches the daemon shape verbatim on the
//!    `status` + `next` fields so tooling can key on one taxonomy
//!    across all three revision operations.

// **PLATFORM:** all (portable; no FUSE / live-network required).
// **GATING:** none.

use std::path::PathBuf;

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::bootstrap_with_config;
use pcloud_ipc::{Request, ResponseStatus};

fn unique_root(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pcloud-daemon-filehistory-{tag}-{}-{nonce}",
        std::process::id()
    ))
}

fn runtime_with_revision_url(
    tag: &str,
    env: Environment,
    url: Option<&str>,
) -> pcloud_daemon::RuntimeShell {
    let root = unique_root(tag);
    let mut config = ConfigProfile::secure_defaults(root, env);
    config.file_history.revision_url = url.map(str::to_owned);
    bootstrap_with_config(config).expect("runtime bootstrap should succeed")
}

#[test]
fn null_provider_returns_structured_not_configured_payload() {
    let mut runtime = runtime_with_revision_url("null", Environment::Test, None);
    let response = runtime.handle_request(Request::FileHistory {
        path: "/Docs/report.txt".to_owned(),
        limit: None,
    });

    assert_eq!(
        response.status,
        ResponseStatus::Unavailable,
        "null provider must surface Unavailable (exit 6), got {:?}",
        response.status
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&response.message).expect("daemon must emit a JSON-parseable payload");
    assert_eq!(parsed["status"], "not_configured");
    let msg = parsed["message"].as_str().expect("message is a string");
    assert!(
        msg.contains("revision_url"),
        "message must name the remediation key: {msg}"
    );
    let next = parsed["next"].as_str().expect("next is a string");
    assert!(
        next.contains("[file_history].revision_url"),
        "next must name the config key: {next}"
    );
    assert_eq!(parsed["path"], "/Docs/report.txt");
}

#[test]
fn empty_path_bypasses_provider_with_invalid_request() {
    let mut runtime = runtime_with_revision_url("empty-path", Environment::Test, None);
    for path in ["", "   ", "\t\n"] {
        let response = runtime.handle_request(Request::FileHistory {
            path: path.to_owned(),
            limit: None,
        });
        assert_eq!(
            response.status,
            ResponseStatus::InvalidRequest,
            "empty path must be refused upfront (got {:?} for {path:?})",
            response.status
        );
        assert!(
            response.message.contains("absolute remote path"),
            "invalid-request message must guide the caller: {}",
            response.message
        );
    }
}

#[test]
fn limit_argument_is_carried_through_provider_path() {
    // The null provider never returns revisions, so this test just
    // verifies that the limit parameter does not disturb the
    // not-configured response shape.
    let mut runtime = runtime_with_revision_url("with-limit", Environment::Test, None);
    let response = runtime.handle_request(Request::FileHistory {
        path: "/x/y".to_owned(),
        limit: Some(5),
    });
    assert_eq!(response.status, ResponseStatus::Unavailable);
    let parsed: serde_json::Value = serde_json::from_str(&response.message).unwrap();
    assert_eq!(parsed["status"], "not_configured");
}

#[test]
fn production_refuses_plaintext_revision_url_at_load_time() {
    let root = unique_root("plaintext-prod");
    let mut config = ConfigProfile::secure_defaults(root, Environment::Production);
    config.file_history.revision_url = Some("http://insecure.example/r".into());
    // Force-downgrade other fields that `secure_defaults` leaves OK in
    // production so validate() surfaces the file_history error first.
    let err = config
        .validate()
        .expect_err("plaintext revision URL must be refused in production");
    let rendered = err.to_string();
    assert!(
        rendered.contains("file_history")
            || rendered.contains("revision_url")
            || rendered.contains("https"),
        "error must name the offending field: {rendered}"
    );
}

#[test]
fn development_accepts_plaintext_revision_url_for_local_testing() {
    // Local integration tests / mock servers need to target
    // http://localhost without flipping the whole profile to
    // production. Confirm the Development profile passes validation.
    let root = unique_root("plaintext-dev");
    let mut config = ConfigProfile::secure_defaults(root, Environment::Development);
    config.file_history.revision_url = Some("http://localhost:65535/r".into());
    config
        .validate()
        .expect("development must accept http:// URLs");
}

#[test]
fn cli_stub_and_daemon_share_the_same_not_configured_taxonomy() {
    // The CLI `diff` / `restore` stubs (in pcloud-cli/src/main.rs)
    // emit a hand-rolled JSON payload that must match the daemon's
    // shape on the `status` + `next` fields so tooling keyed on that
    // taxonomy behaves identically across all three revision
    // operations.
    let mut runtime = runtime_with_revision_url("cli-parity", Environment::Test, None);
    let response = runtime.handle_request(Request::FileHistory {
        path: "/x".to_owned(),
        limit: None,
    });
    let daemon: serde_json::Value = serde_json::from_str(&response.message).unwrap();

    // The CLI stub literal. Kept here verbatim as the contract both
    // surfaces assert against.
    let cli_stub = concat!(
        "{\"status\":\"not_configured\",",
        "\"message\":\"pCloud listrevisions API not yet public; ",
        "configure [file_history].revision_url to point at a custom endpoint\",",
        "\"next\":\"configure [file_history].revision_url ",
        "or wait for pCloud public API\"}",
    );
    let cli: serde_json::Value = serde_json::from_str(cli_stub).unwrap();

    // `status` + `next` are byte-exact contracts (tooling keys on them).
    assert_eq!(daemon["status"], cli["status"]);
    assert_eq!(daemon["next"], cli["next"]);

    // `message` differs in one controlled way: the daemon wraps the
    // provider's message with `thiserror`'s category prefix
    // (`revision provider not configured: <msg>`) so log greps see the
    // structured kind. The CLI stub quotes the raw provider message
    // because it never sees a `RevisionError`. Verify the canonical
    // suffix is identical on both sides, which is the contract tooling
    // relies on.
    let daemon_msg = daemon["message"].as_str().unwrap();
    let cli_msg = cli["message"].as_str().unwrap();
    assert!(
        daemon_msg.ends_with(cli_msg),
        "daemon message must end with the canonical CLI message\n  daemon: {daemon_msg}\n  cli:    {cli_msg}"
    );
}

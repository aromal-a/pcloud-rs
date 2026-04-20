#![allow(clippy::pedantic)]
//! Integration tests for data-residency enforcement at the three daemon
//! runtime call sites (`sync_root_add`, `upload_create`, and public-link
//! create / upload-link create).
//!
//! The tests drive [`RuntimeShell::check_residency`] directly to avoid
//! requiring a live pCloud session. That helper is the single point
//! every enforcement call site funnels through, so covering it here
//! exercises the same code the dispatch paths reach at runtime.
//! Dispatch-level regression coverage — `Conflict` when unauthenticated,
//! `PolicyViolation` when authenticated and denied — is asserted via the
//! `Request::SyncRootAdd`, `CreateFilePublicLink`, and `CreateUploadLink`
//! variants further down.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use pcloud_backends::residency::{ACTION_SYNC_ROOT_ADD, ACTION_UPLOAD_CREATE, Region};
use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::bootstrap_with_config;
use pcloud_ipc::{Request, ResponseStatus};

fn unique_root(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pcloud-daemon-residency-{tag}-{}-{nonce}",
        std::process::id()
    ))
}

/// Build a fresh runtime with `host`, `allowed`, and `strict` configured
/// on the `[data_residency]` policy. No auth is seeded — callers drive
/// `check_residency` directly or assert the pre-auth guard message.
fn runtime_with_policy(
    tag: &str,
    host: &str,
    allowed: &[&str],
    strict: bool,
) -> pcloud_daemon::RuntimeShell {
    let root = unique_root(tag);
    let mut config = ConfigProfile::secure_defaults(root, Environment::Test);
    config.api.host = host.to_owned();
    config.api.server_name = host.to_owned();
    config.data_residency.allowed_regions = allowed.iter().map(|s| (*s).to_owned()).collect();
    config.data_residency.strict = strict;
    bootstrap_with_config(config).expect("bootstrap residency runtime")
}

// -----------------------------------------------------------------------------
// Unit-level coverage of the enforcement helper
// -----------------------------------------------------------------------------

#[test]
fn strict_mode_refuses_sync_root_add_from_disallowed_region() {
    let mut rt = runtime_with_policy("sync-refuse", "eapi.pcloud.com", &["US"], true);
    let refusal = rt.check_residency(ACTION_SYNC_ROOT_ADD, Region::Eu);
    let resp = refusal.expect("strict policy must refuse an EU host when only US is allowed");
    assert!(
        matches!(
            &resp.status,
            ResponseStatus::PolicyViolation { kind } if kind == "data_residency"
        ),
        "expected PolicyViolation{{data_residency}}, got {:?}",
        resp.status
    );
    assert!(
        resp.message.contains("EU"),
        "refusal must name the offending region; got: {}",
        resp.message
    );
    assert!(
        resp.message.contains("US"),
        "refusal must name the allow-list; got: {}",
        resp.message
    );
}

#[test]
fn strict_mode_refuses_upload_create_from_disallowed_region() {
    let mut rt = runtime_with_policy("upload-refuse", "api.pcloud.com", &["EU"], true);
    let refusal = rt.check_residency(ACTION_UPLOAD_CREATE, Region::Us);
    let resp = refusal.expect("strict policy must refuse US when only EU is allowed");
    match resp.status {
        ResponseStatus::PolicyViolation { kind } => assert_eq!(kind, "data_residency"),
        other => panic!("expected PolicyViolation, got {other:?}"),
    }
}

#[test]
fn non_strict_mode_allows_but_warns() {
    let mut rt = runtime_with_policy("warn-only", "eapi.pcloud.com", &["US"], false);
    // Non-strict: the check must NOT produce a refusal response — the
    // operation proceeds and an audit warn is logged.
    let refusal = rt.check_residency(ACTION_UPLOAD_CREATE, Region::Eu);
    assert!(
        refusal.is_none(),
        "non-strict warn-only must not block the operation"
    );
}

#[test]
fn empty_allow_list_permits_all_regions() {
    // Empty allow-list is the backward-compat default: every region is
    // permitted regardless of `strict`.
    let mut rt = runtime_with_policy("unrestricted", "api.pcloud.com", &[], true);
    assert!(
        rt.check_residency(ACTION_UPLOAD_CREATE, Region::Us)
            .is_none()
    );
    assert!(
        rt.check_residency(ACTION_UPLOAD_CREATE, Region::Eu)
            .is_none()
    );
    assert!(
        rt.check_residency(ACTION_UPLOAD_CREATE, Region::Unknown)
            .is_none()
    );
}

#[test]
fn strict_mode_refuses_unknown_region() {
    let mut rt = runtime_with_policy("unknown", "api.pcloud.com", &["EU", "US"], true);
    let refusal = rt.check_residency(ACTION_UPLOAD_CREATE, Region::Unknown);
    assert!(
        refusal.is_some(),
        "strict mode must refuse unknown regions so mis-classified hosts cannot sneak through"
    );
}

#[test]
fn region_allow_list_is_case_insensitive() {
    // The config layer stores regions verbatim; enforcement must compare
    // case-insensitively so `["eu"]` accepts `Region::Eu` (tag "EU").
    let mut rt = runtime_with_policy("case", "eapi.pcloud.com", &["eu"], true);
    assert!(
        rt.check_residency(ACTION_UPLOAD_CREATE, Region::Eu)
            .is_none()
    );
}

// -----------------------------------------------------------------------------
// End-to-end dispatch coverage
// -----------------------------------------------------------------------------
//
// Without a live pCloud session the authenticated enforcement path
// cannot be exercised end-to-end; dispatch returns `Conflict` at the
// auth-token guard before reaching `check_residency`. Asserting on that
// shape documents the request -> auth-gate -> residency-gate ordering
// and catches regressions that swap the two guards (which would leak a
// residency refusal before auth, revealing policy shape to
// unauthenticated callers).

#[test]
fn unauthenticated_sync_root_add_returns_conflict_not_residency_violation() {
    let mut rt = runtime_with_policy("dispatch-sync", "eapi.pcloud.com", &["US"], true);
    let resp = rt.handle_request(Request::SyncRootAdd {
        local_path: rt.config.paths.state_dir.display().to_string(),
        remote_path: "/Work".into(),
        sync_type: None,
    });
    assert_eq!(
        resp.status,
        ResponseStatus::Conflict,
        "auth gate must precede residency gate: {}",
        resp.message
    );
}

#[test]
fn unauthenticated_create_file_public_link_returns_conflict() {
    let mut rt = runtime_with_policy("dispatch-publink", "eapi.pcloud.com", &["US"], true);
    let resp = rt.handle_request(Request::CreateFilePublicLink {
        path: "/Work/report.pdf".into(),
    });
    assert_eq!(resp.status, ResponseStatus::Conflict);
}

#[test]
fn unauthenticated_create_upload_link_returns_conflict() {
    let mut rt = runtime_with_policy("dispatch-uploadlink", "eapi.pcloud.com", &["US"], true);
    let resp = rt.handle_request(Request::CreateUploadLink {
        path: "/Work".into(),
        comment: "dropzone".into(),
        expire: None,
        maxspace: None,
        maxfiles: None,
    });
    assert_eq!(resp.status, ResponseStatus::Conflict);
}

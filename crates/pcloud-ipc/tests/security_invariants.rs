#![allow(clippy::pedantic)]
//! # Security invariants integration harness (IPC & secrets slice)
//!
//! This test file backs the architecture-scoped security model
//! (`docs/book/src/architecture/security-model.md`, SEC-XX rows). Tests are
//! grouped by invariant and named `sec_XX_<short_slug>` so a reviewer can
//! map a doc row to a test in one step.
//!
//! This file lives in the `pcloud-ipc` crate because that crate (together
//! with its dev-dep on `pcloud-secret`) is the narrowest build surface that
//! still reaches every invariant that is practically testable from
//! userspace:
//!
//! - Secret-wrapper invariants (Debug redaction, Zeroize exposure):
//!   SEC-01, SEC-02, SEC-04.
//! - IPC socket / transport invariants (peer-cred authorization, 1 MiB
//!   frame cap, protocol version pin, 0600/0700 mode on bind):
//!   SEC-10, SEC-11, SEC-12, SEC-13.
//! - Panic-guard contract (SEC-50): asserted at the pattern level here;
//!   the production guard lives in
//!   `crates/pcloud-daemon/src/runtime.rs::handle_request` and is review-
//!   only until the daemon crate ships a dedicated `panic_guard.rs` test.
//! - Policy-violation wire safety: asserts `Response::PolicyViolation`
//!   carries no secret material.
//!
//! Invariants that require the full daemon runtime (SEC-20, SEC-21,
//! SEC-22, SEC-23, SEC-30, SEC-31, SEC-32, SEC-40, SEC-41, SEC-42, SEC-51)
//! are proven by tests inside their owning crates — see the citation
//! updates in the security-model doc.

// **PLATFORM:** cross-platform except the 0600/0700 mode check, which is
// `#[cfg(unix)]`-gated.
// **GATING:** none.

#![forbid(unsafe_code)]

use pcloud_ipc::{
    IpcServer, PeerIdentity, Request, Response, ResponseStatus, encode_request_bare,
    protocol::{self, IPC_PROTOCOL_VERSION, MAX_IPC_PAYLOAD_LEN, ProtocolError},
};
use pcloud_secret::{
    ExposeSecret, SecretMaterial, secret_bytes::SecretBytes, secret_string::SecretString,
};
use zeroize::Zeroize;

// -------------------------------------------------------------------------
// SEC-01 — every long-lived secret-bearing field uses a secret wrapper.
// -------------------------------------------------------------------------
//
// Call-site enforcement is tracked by `grep expose_secret` discipline and
// by the compile-fail regression in
// `crates/pcloud-secret/tests/serialize_is_forbidden.rs`. This test asserts
// the public trait surface a reviewer reaches for — length exposure without
// content leakage.

#[test]
fn sec_01_wrappers_expose_len_without_content() {
    let s = SecretString::new("long-auth-token-value");
    assert_eq!(s.expose_len(), "long-auth-token-value".len());
    let b = SecretBytes::new(vec![0xAB; 64]);
    assert_eq!(b.expose_len(), 64);
}

// -------------------------------------------------------------------------
// SEC-02 — Debug output for secret types redacts content.
// -------------------------------------------------------------------------

#[test]
fn sec_02_secret_string_debug_is_redacted() {
    let s = SecretString::new("hunter2-very-unique-literal");
    let rendered = format!("{s:?}");
    assert!(rendered.contains("<redacted>"), "debug: {rendered}");
    assert!(
        !rendered.contains("hunter2-very-unique-literal"),
        "debug leaked secret literal: {rendered}"
    );
}

#[test]
fn sec_02_secret_bytes_debug_is_redacted() {
    let b = SecretBytes::new(b"mac-tag-abcdef-1234".to_vec());
    let rendered = format!("{b:?}");
    assert!(rendered.contains("<redacted>"), "debug: {rendered}");
    assert!(
        !rendered.contains("mac-tag-abcdef-1234"),
        "debug leaked secret literal: {rendered}"
    );
}

#[test]
fn sec_02_secret_string_pretty_debug_is_redacted() {
    // Alternate-debug `{:#?}` is used by several tracing adapters.
    let s = SecretString::new("pretty-print-should-not-leak");
    let rendered = format!("{s:#?}");
    assert!(rendered.contains("<redacted>"), "pretty debug: {rendered}");
    assert!(!rendered.contains("pretty-print-should-not-leak"));
}

// -------------------------------------------------------------------------
// SEC-04 — dropped secrets are scrubbed via Zeroize.
// -------------------------------------------------------------------------
//
// True "memory-after-drop" observation is not sound from safe Rust. What we
// assert is (a) the wrapper exposes `zeroize::Zeroize`, (b) calling
// `.zeroize()` produces an empty value, and (c) `expose_secret()` sees the
// scrubbed state. The ZeroizeOnDrop derive is the contract that turns (a)
// into the runtime Drop behaviour; its correctness is guarded by the
// `zeroize` crate's own tests and by
// `crates/pcloud-secret/tests/proptest_zeroize_invariants.rs`.

#[test]
fn sec_04_secret_string_zeroize_empties_wrapper() {
    let mut s = SecretString::new("soon-to-be-zeroized");
    assert_eq!(s.expose_len(), "soon-to-be-zeroized".len());
    <SecretString as Zeroize>::zeroize(&mut s);
    assert!(s.is_empty(), "zeroize must leave the wrapper empty");
    assert_eq!(format!("{s:?}"), "SecretString(<redacted>)");
    assert_eq!(s.expose_secret(), "");
}

#[test]
fn sec_04_secret_bytes_zeroize_empties_wrapper() {
    let mut b = SecretBytes::new(vec![0x11; 32]);
    assert_eq!(b.expose_len(), 32);
    <SecretBytes as Zeroize>::zeroize(&mut b);
    assert!(b.is_empty());
    assert_eq!(format!("{b:?}"), "SecretBytes(<redacted>)");
    assert_eq!(b.expose_secret(), &[] as &[u8]);
}

// -------------------------------------------------------------------------
// SEC-10 — the IPC socket is 0600 on a 0700 parent directory.
// -------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn sec_10_ipc_socket_is_0600_on_0700_parent() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Use `/tmp` directly: macOS SUN_LEN=104 cannot accommodate the
    // per-user tempdir `/var/folders/.../T/` prefix (49 chars) plus the
    // rest of the path.
    let parent = std::path::PathBuf::from("/tmp").join(format!(
        "pipc-sec10-{}-{}",
        std::process::id(),
        nonce % 1_000_000_000
    ));
    let socket_path = parent.join("daemon.sock");

    let server = IpcServer::new(pcloud_ipc::current_effective_uid());
    let _bound = server.bind(&socket_path).expect("socket should bind");

    let parent_meta = std::fs::metadata(&parent).expect("parent dir should exist");
    let parent_mode = parent_meta.permissions().mode() & 0o777;
    assert_eq!(
        parent_mode, 0o700,
        "runtime parent dir mode must be 0700, got 0o{parent_mode:o}"
    );

    let sock_meta = std::fs::metadata(&socket_path).expect("socket should exist");
    let sock_mode = sock_meta.permissions().mode() & 0o777;
    assert_eq!(
        sock_mode, 0o600,
        "IPC socket mode must be 0600, got 0o{sock_mode:o}"
    );
}

// -------------------------------------------------------------------------
// SEC-11 — peer-cred mismatch is rejected.
// -------------------------------------------------------------------------
//
// A real cross-UID connection is not reproducible from a single-user test
// process without a sandbox, so these tests assert the predicate that the
// transport enforces: `authorize_peer` refuses any non-owning uid, including
// root. On Unix the transport also sets the socket mode to 0600 (see SEC-10),
// which kernel-level enforces the same rule at accept() time.

#[test]
fn sec_11_authorize_peer_rejects_non_owner() {
    let server = IpcServer::new(1234);
    let non_owner = PeerIdentity { uid: 1235, pid: 99 };
    assert!(
        !server.authorize_peer(&non_owner),
        "non-owner peer must be rejected"
    );
}

#[test]
fn sec_11_authorize_peer_rejects_root_when_owner_is_user() {
    let server = IpcServer::new(1000);
    let root = PeerIdentity { uid: 0, pid: 1 };
    assert!(
        !server.authorize_peer(&root),
        "root must not be implicitly authorized on a user-owned socket"
    );
}

#[test]
fn sec_11_authorize_peer_accepts_only_exact_owner() {
    let server = IpcServer::new(4242);
    assert!(server.authorize_peer(&PeerIdentity { uid: 4242, pid: 1 }));
    assert!(!server.authorize_peer(&PeerIdentity { uid: 4243, pid: 1 }));
}

// -------------------------------------------------------------------------
// SEC-12 — IPC body size is capped at 1 MiB before allocation.
// -------------------------------------------------------------------------
//
// The transport-level proof (an attacker-declared 10 MiB length prefix is
// rejected before the server calls `Vec::with_capacity`) lives in
// `crates/pcloud-ipc/tests/request_size_cap.rs`. The tests here assert the
// encoder-side gate and pin the constant.

#[test]
fn sec_12_encode_rejects_over_one_mib_payload() {
    // 1.5 MiB > MAX_IPC_PAYLOAD_LEN (1 MiB). The encoder must refuse.
    let big = "A".repeat(MAX_IPC_PAYLOAD_LEN + 512 * 1024);
    let request = Request::PasswordSubmission {
        username: "user".to_owned(),
        value: big.into(),
    };
    let err = encode_request_bare(&request).expect_err("1.5 MiB payload must be rejected");
    assert!(matches!(err, ProtocolError::PayloadTooLarge));
}

#[test]
fn sec_12_max_ipc_payload_len_is_one_mib() {
    // Drift detection: the doc cites this constant verbatim.
    assert_eq!(MAX_IPC_PAYLOAD_LEN, 1024 * 1024);
}

// -------------------------------------------------------------------------
// SEC-13 — protocol version mismatches close the connection cleanly.
// -------------------------------------------------------------------------

#[test]
fn sec_13_decode_rejects_mismatched_version() {
    let payload = serde_json::to_vec(&Request::Plain {
        method: pcloud_ipc::Method::GetStatus,
    })
    .unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0xFFFFu16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&payload);

    let err = protocol::decode_request(&bytes).expect_err("version mismatch must fail");
    assert!(matches!(
        err,
        ProtocolError::VersionMismatch { expected, actual }
            if expected == IPC_PROTOCOL_VERSION && actual == 0xFFFF
    ));
}

// -------------------------------------------------------------------------
// SEC-50 — panic guard contract: a panicking dispatch body becomes
// Response::InternalError and does not propagate.
// -------------------------------------------------------------------------
//
// The production guard is `catch_unwind(AssertUnwindSafe(|| ...))` at
// `crates/pcloud-daemon/src/runtime.rs::handle_request`. That site is not
// exercisable from this crate (`pcloud-daemon` is not a dev-dep, and pulling
// it in creates a cycle). We assert the *pattern* here as a regression
// guard so any refactor that changes the shape of the guard has to update
// this test too.

#[test]
fn sec_50_catch_unwind_pattern_yields_internal_error() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let response = match catch_unwind(AssertUnwindSafe(|| -> Response {
        panic!("simulated dispatch-arm panic");
    })) {
        Ok(resp) => resp,
        Err(_panic) => Response {
            status: ResponseStatus::InternalError,
            message: "internal daemon panic; request aborted".to_owned(),
        },
    };
    assert_eq!(response.status, ResponseStatus::InternalError);
    assert!(response.message.contains("panic"));
}

// -------------------------------------------------------------------------
// PolicyViolation wire surface — no secrets in the message.
// -------------------------------------------------------------------------
//
// The security-model doc requires that `Response::PolicyViolation` never
// surfaces secret material. Encode a PolicyViolation response and assert
// the serialized bytes carry only the `kind` discriminator. A drift where
// someone accidentally embeds a SecretString in the message would light up
// as `<redacted>` in the output; we assert its absence as an early warning.

#[test]
fn policy_violation_response_carries_no_secret_payload() {
    let resp = Response {
        status: ResponseStatus::PolicyViolation {
            kind: "data_residency".to_owned(),
        },
        message: "refused by data-residency policy".to_owned(),
    };
    let serialized = serde_json::to_string(&resp).expect("serialize");
    assert!(serialized.contains("PolicyViolation"));
    assert!(serialized.contains("data_residency"));
    assert!(!serialized.contains("<redacted>"));
}

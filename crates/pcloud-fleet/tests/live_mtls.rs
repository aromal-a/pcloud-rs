#![allow(clippy::pedantic)]
//! End-to-end integration test: fleet agent -> in-process reference server.
//!
//! This closes the audit gap where `MtlsFleetAgent` was only exercised with
//! mocked I/O. A real HTTPS listener is stood up on `127.0.0.1:<auto_port>`
//! using the shipped self-signed CA + leaf cert fixtures; the agent is
//! pointed at it with the CA installed as a pinned trust root. The
//! reference server validates the ed25519 body-signature header against a
//! configured trust set — mirroring what an enterprise fleet controller
//! would enforce.
//!
//! Coverage:
//!
//! 1. `heartbeat_is_accepted_end_to_end` — happy path: TLS handshake
//!    succeeds against the pinned CA, agent signs the body, server verifies
//!    the signature, 200 OK is returned to the caller.
//! 2. `tampered_body_signature_is_rejected` — tamper scenario: a stub
//!    client that signs a different payload than it sends causes the
//!    server to respond 401.
//! 3. `untrusted_device_sid_is_rejected` — identity allow-list: an agent
//!    whose device key is not in the server's trust set gets 401 even
//!    though its own signature is mathematically valid.
//!
//! Any failure here means `MtlsFleetAgent` is no longer wire-compatible
//! with its own specified server side.
//!
//! ## Runtime discipline
//!
//! `MtlsFleetAgent` wraps a `reqwest::blocking::Client`, which internally
//! owns a current-thread tokio runtime. That runtime cannot be dropped
//! from within another tokio runtime (panics with "Cannot drop a runtime
//! in a context where blocking is not allowed"). Every test below
//! therefore creates AND drops the agent inside a `spawn_blocking` closure
//! so the nested runtime only ever sees a plain OS thread.

#![forbid(unsafe_code)]

#[path = "reference_server.rs"]
mod reference_server;

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer, SigningKey};
use pcloud_fleet::{FleetError, MtlsFleetAgent, MtlsFleetConfig};
use tempfile::TempDir;

fn mk_config(tmp: &std::path::Path, base_url: String) -> MtlsFleetConfig {
    MtlsFleetConfig {
        server_url: base_url,
        device_group: "integration-test".into(),
        identity_path: tmp.join("identity.json"),
        ca_bundle_path: reference_server::write_ca_bundle(tmp),
        trusted_server_keys: Vec::new(),
        request_timeout: Some(Duration::from_secs(5)),
    }
}

/// Mint the agent once so the ed25519 identity file exists on disk, then
/// return the base64-encoded device SID. The agent itself is dropped on the
/// blocking thread so its internal blocking-reqwest runtime cleans up
/// without tripping tokio's nested-runtime guard.
async fn mint_identity(tmp_path: std::path::PathBuf) -> String {
    tokio::task::spawn_blocking(move || {
        let ca_path = reference_server::write_ca_bundle(&tmp_path);
        let cfg = MtlsFleetConfig {
            server_url: "https://127.0.0.1:1".into(),
            device_group: "integration-test".into(),
            identity_path: tmp_path.join("identity.json"),
            ca_bundle_path: ca_path,
            trusted_server_keys: Vec::new(),
            request_timeout: Some(Duration::from_secs(1)),
        };
        let a = MtlsFleetAgent::new(cfg).expect("mint agent identity");
        a.identity().public_key_b64()
    })
    .await
    .expect("mint_identity join")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_is_accepted_end_to_end() {
    let tmp = TempDir::new().expect("tempdir");

    // Stage 1: mint identity.
    let device_sid_b64 = mint_identity(tmp.path().to_path_buf()).await;

    // Stage 2: start the reference server trusting that device key.
    let srv = reference_server::spawn(vec![device_sid_b64.clone()])
        .await
        .expect("spawn reference server");

    // Stage 3: rebuild the agent against the real URL and send the HB —
    // all on a blocking worker thread for the runtime-nesting reason
    // documented in the file header.
    let tmp_path = tmp.path().to_path_buf();
    let base_url = srv.base_url();
    let result = tokio::task::spawn_blocking(move || {
        let cfg = mk_config(&tmp_path, base_url);
        let agent = MtlsFleetAgent::new(cfg).expect("rebuild agent");
        let hb = agent.default_heartbeat();
        agent.send_heartbeat(&hb)
    })
    .await
    .expect("spawn_blocking join");

    assert!(
        result.is_ok(),
        "expected heartbeat to succeed, got {:?}",
        result
    );
    assert!(
        result.unwrap().is_none(),
        "reference server returns empty body — no pending command expected"
    );
    assert_eq!(
        srv.requests_total.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        srv.requests_verified
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        srv.requests_rejected
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampered_body_signature_is_rejected() {
    // Hand-craft a request whose signature header covers a DIFFERENT
    // payload than the one actually posted. The server must refuse.

    let tmp = TempDir::new().expect("tempdir");
    let ca_path = reference_server::write_ca_bundle(tmp.path());

    use rand_core::OsRng;
    let sk = SigningKey::generate(&mut OsRng);
    let vk = sk.verifying_key();
    let sid_b64 = B64.encode(vk.to_bytes());

    let srv = reference_server::spawn(vec![sid_b64.clone()])
        .await
        .expect("spawn reference server");

    let base_url = srv.base_url();

    let status = tokio::task::spawn_blocking(move || -> u16 {
        let ca_pem = std::fs::read(&ca_path).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        let mut r = std::io::Cursor::new(&ca_pem);
        for c in rustls_pemfile::certs(&mut r) {
            roots.add(c.unwrap()).unwrap();
        }
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let client = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(tls)
            .tls_built_in_root_certs(false)
            .https_only(true)
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let real_body = br#"{"tampered":true}"#.to_vec();
        // Sign a DIFFERENT body than we POST.
        let decoy = b"this-is-not-what-gets-sent";
        let sig = sk.sign(decoy).to_bytes();

        let url = format!("{}/v1/heartbeat", base_url.trim_end_matches('/'));
        let resp = client
            .post(url)
            .header("X-PCloud-Device-SID", &sid_b64)
            .header("X-PCloud-Body-Signature", B64.encode(sig))
            .header("Content-Type", "application/json")
            .body(real_body)
            .send()
            .expect("send tampered");
        resp.status().as_u16()
    })
    .await
    .expect("spawn_blocking join");

    assert_eq!(status, 401, "tampered signature must be rejected");
    assert_eq!(
        srv.requests_verified
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no request should have verified"
    );
    assert_eq!(
        srv.requests_rejected
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_device_sid_is_rejected() {
    // Start a server with a trust set that does NOT include the agent's
    // actual SID; the agent's signature is mathematically valid but layer-7
    // identity pinning must still refuse it.
    let tmp = TempDir::new().expect("tempdir");
    let _mint_sid = mint_identity(tmp.path().to_path_buf()).await;

    use rand_core::OsRng;
    let other = SigningKey::generate(&mut OsRng);
    let other_sid = B64.encode(other.verifying_key().to_bytes());
    let srv = reference_server::spawn(vec![other_sid])
        .await
        .expect("spawn reference server");

    let tmp_path = tmp.path().to_path_buf();
    let base_url = srv.base_url();
    let result = tokio::task::spawn_blocking(move || {
        let cfg = mk_config(&tmp_path, base_url);
        let agent = MtlsFleetAgent::new(cfg).expect("rebuild agent");
        let hb = agent.default_heartbeat();
        agent.send_heartbeat(&hb)
    })
    .await
    .expect("spawn_blocking join");

    match result {
        Err(FleetError::Transport(msg)) => {
            assert!(msg.contains("401"), "expected 401 Unauthorized, got: {msg}");
        }
        other => panic!("expected Transport(401), got {:?}", other),
    }
    assert_eq!(
        srv.requests_verified
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        srv.requests_rejected
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    srv.shutdown().await;
}

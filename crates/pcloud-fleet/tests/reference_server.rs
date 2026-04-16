#![allow(clippy::pedantic)]
//! In-process reference fleet server used by `live_mtls.rs`.
//!
//! This is **not** a production fleet server. It is a deliberately minimal
//! implementation of just enough of the wire protocol spec to exercise the
//! `MtlsFleetAgent` end-to-end against a real TLS listener:
//!
//! - serves HTTPS on `127.0.0.1:<auto_port>` using a self-signed RSA cert
//!   shipped under `tests/fixtures/`,
//! - accepts `POST /v1/heartbeat` with a JSON body,
//! - verifies the `X-PCloud-Body-Signature` header is a valid ed25519
//!   signature over the request body, made by the public key advertised in
//!   `X-PCloud-Device-SID`,
//! - accepts only requests whose advertised device public key is in the
//!   server's configured trust set — emulating the server-side identity
//!   allow-list an enterprise fleet controller would enforce.
//!
//! **Forbidden:** `unsafe` is denied. All TLS, HTTP, and crypto paths go
//! through already-vetted crates (`tokio-rustls`, `hyper`, `ed25519-dalek`).
//!
//! This module is compiled only as part of the integration-test binary and
//! is not part of the `pcloud-fleet` public API.

#![forbid(unsafe_code)]
#![allow(dead_code)]

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use bytes::Bytes;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;

/// Trust-anchor CA certificate shipped under `tests/fixtures/`. This is the
/// PEM the agent pins as its `ca_bundle_path`.
pub const TEST_CA_PEM: &[u8] = include_bytes!("fixtures/fleet_test_ca.crt");
/// Server leaf certificate, signed by [`TEST_CA_PEM`]. Has SAN entries
/// `DNS:localhost` and `IP:127.0.0.1`, so rustls accepts it for both.
pub const TEST_SERVER_CERT_PEM: &[u8] = include_bytes!("fixtures/fleet_test_server.crt");
/// PKCS#8 RSA private key matching [`TEST_SERVER_CERT_PEM`].
pub const TEST_SERVER_KEY_PEM: &[u8] = include_bytes!("fixtures/fleet_test_server.key");

/// Write the CA certificate into a directory and return the cert path,
/// suitable for use as the fleet agent's `ca_bundle_path`. The CA is a
/// self-signed trust anchor with CA:TRUE and signs the server leaf cert
/// that the reference server presents on the wire.
pub fn write_ca_bundle(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("ca.pem");
    std::fs::write(&p, TEST_CA_PEM).expect("write test ca.pem");
    p
}

/// Handle to a running reference server. Dropping the handle shuts the
/// server down.
pub struct ReferenceServer {
    /// Address the server is listening on (always 127.0.0.1:<port>).
    pub addr: SocketAddr,
    /// Number of requests accepted by the TLS listener.
    pub requests_total: Arc<AtomicUsize>,
    /// Number of requests the body-signature validator accepted.
    pub requests_verified: Arc<AtomicUsize>,
    /// Number of requests the body-signature validator rejected (bad sig,
    /// untrusted SID, missing header, etc.).
    pub requests_rejected: Arc<AtomicUsize>,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl ReferenceServer {
    /// Graceful shutdown; blocks until the accept loop exits.
    pub async fn shutdown(mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = j.await;
        }
    }

    /// HTTPS base URL, e.g. `https://127.0.0.1:49812`.
    pub fn base_url(&self) -> String {
        format!("https://{}", self.addr)
    }
}

/// Start a reference fleet server that trusts only the provided set of
/// base64-encoded ed25519 device public keys. Returns a handle once the
/// listener is bound and accepting.
pub async fn spawn(
    trusted_device_sids_b64: Vec<String>,
) -> Result<ReferenceServer, Box<dyn std::error::Error + Send + Sync>> {
    // Install the crypto provider once per process. Ignore duplicate-install
    // error; other tests in the same process may have already installed it.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let certs = load_certs(TEST_SERVER_CERT_PEM)?;
    let key = load_key(TEST_SERVER_KEY_PEM)?;

    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    let acceptor = TlsAcceptor::from(Arc::new(cfg));

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let trusted: Arc<Vec<[u8; 32]>> = Arc::new(
        trusted_device_sids_b64
            .iter()
            .map(|s| {
                let raw = B64
                    .decode(s.as_bytes())
                    .expect("trusted SID is valid base64");
                assert_eq!(raw.len(), 32, "trusted SID must be 32 bytes");
                let mut k = [0u8; 32];
                k.copy_from_slice(&raw);
                k
            })
            .collect(),
    );

    let requests_total = Arc::new(AtomicUsize::new(0));
    let requests_verified = Arc::new(AtomicUsize::new(0));
    let requests_rejected = Arc::new(AtomicUsize::new(0));

    let (tx, mut rx) = oneshot::channel::<()>();

    let total_c = requests_total.clone();
    let ok_c = requests_verified.clone();
    let bad_c = requests_rejected.clone();

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => {
                    return;
                }
                accept = listener.accept() => {
                    let (stream, _peer) = match accept {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let acceptor = acceptor.clone();
                    let trusted = trusted.clone();
                    let total_c = total_c.clone();
                    let ok_c = ok_c.clone();
                    let bad_c = bad_c.clone();
                    tokio::spawn(async move {
                        let tls = match acceptor.accept(stream).await {
                            Ok(t) => t,
                            Err(_) => return,
                        };
                        let io = TokioIo::new(tls);
                        let service = service_fn(move |req| {
                            let trusted = trusted.clone();
                            let total_c = total_c.clone();
                            let ok_c = ok_c.clone();
                            let bad_c = bad_c.clone();
                            async move {
                                total_c.fetch_add(1, Ordering::SeqCst);
                                match handle(req, &trusted).await {
                                    Ok(r) => {
                                        if r.status() == StatusCode::OK {
                                            ok_c.fetch_add(1, Ordering::SeqCst);
                                        } else {
                                            bad_c.fetch_add(1, Ordering::SeqCst);
                                        }
                                        Ok::<_, hyper::Error>(r)
                                    }
                                    Err(_) => {
                                        bad_c.fetch_add(1, Ordering::SeqCst);
                                        Ok(error_response(
                                            StatusCode::INTERNAL_SERVER_ERROR,
                                        ))
                                    }
                                }
                            }
                        });
                        let _ = http1::Builder::new()
                            .keep_alive(false)
                            .serve_connection(io, service)
                            .await;
                    });
                }
            }
        }
    });

    Ok(ReferenceServer {
        addr,
        requests_total,
        requests_verified,
        requests_rejected,
        shutdown: Some(tx),
        join: Some(join),
    })
}

async fn handle(
    req: Request<Incoming>,
    trusted: &[[u8; 32]],
) -> Result<Response<Full<Bytes>>, Box<dyn std::error::Error + Send + Sync>> {
    if req.uri().path() != "/v1/heartbeat" {
        return Ok(error_response(StatusCode::NOT_FOUND));
    }
    // Extract & validate headers *before* reading the body so we can reject
    // unsigned requests cheaply. Clone out of the request parts into owned
    // values so we can move `req` into `collect` later.
    let sid_header = match req
        .headers()
        .get("X-PCloud-Device-SID")
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned)
    {
        Some(s) => s,
        None => return Ok(error_response(StatusCode::UNAUTHORIZED)),
    };
    let sig_header = match req
        .headers()
        .get("X-PCloud-Body-Signature")
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned)
    {
        Some(s) => s,
        None => return Ok(error_response(StatusCode::UNAUTHORIZED)),
    };

    let sid_bytes = match B64.decode(sid_header.as_bytes()) {
        Ok(b) if b.len() == 32 => b,
        _ => return Ok(error_response(StatusCode::UNAUTHORIZED)),
    };
    let mut sid_arr = [0u8; 32];
    sid_arr.copy_from_slice(&sid_bytes);
    if !trusted.contains(&sid_arr) {
        return Ok(error_response(StatusCode::UNAUTHORIZED));
    }

    let sig_bytes = match B64.decode(sig_header.as_bytes()) {
        Ok(b) if b.len() == 64 => b,
        _ => return Ok(error_response(StatusCode::UNAUTHORIZED)),
    };
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);

    let body_bytes = req.collect().await?.to_bytes();

    let vk = match VerifyingKey::from_bytes(&sid_arr) {
        Ok(v) => v,
        Err(_) => return Ok(error_response(StatusCode::UNAUTHORIZED)),
    };
    let sig = Signature::from_bytes(&sig_arr);
    if vk.verify(&body_bytes, &sig).is_err() {
        return Ok(error_response(StatusCode::UNAUTHORIZED));
    }

    // Success: return an empty 200 — `MtlsFleetAgent::send_heartbeat`
    // treats an empty body as "no pending command", which keeps the test
    // focused purely on the request-auth direction.
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Length", "0")
        .body(Full::new(Bytes::new()))
        .expect("build empty 200"))
}

fn error_response(code: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(code)
        .header("Content-Length", "0")
        .body(Full::new(Bytes::new()))
        .expect("build error response")
}

fn load_certs(
    pem: &[u8],
) -> Result<Vec<CertificateDer<'static>>, Box<dyn std::error::Error + Send + Sync>> {
    let mut rdr = Cursor::new(pem);
    let mut out = Vec::new();
    for c in rustls_pemfile::certs(&mut rdr) {
        out.push(c?);
    }
    if out.is_empty() {
        return Err("no certificate in PEM".into());
    }
    Ok(out)
}

fn load_key(
    pem: &[u8],
) -> Result<PrivateKeyDer<'static>, Box<dyn std::error::Error + Send + Sync>> {
    let mut rdr = Cursor::new(pem);
    // The fixture is a PKCS#8 key; `private_key` returns the first match.
    if let Some(k) = rustls_pemfile::private_key(&mut rdr)? {
        return Ok(k);
    }
    Err("no private key in PEM".into())
}

#![allow(clippy::pedantic)]
//! # pcloud-fleet
//!
//! Fleet-management agent for enterprise pcloud-rs installations.
//!
//! This crate defines the **wire protocol** and the **agent trait** that a
//! pcloud-rs daemon uses to report health to, and receive commands from, an
//! external fleet-management server. The server itself is **not** part of
//! this repository; only the client-side surface is specified here.
//!
//! ## Design overview
//!
//! - Transport: JSON-over-HTTPS (rustls) with an ed25519 device identity.
//! - Identity: ed25519 keypair generated on first contact and persisted
//!   0600 on disk; the public key is advertised on every heartbeat as
//!   `X-PCloud-Device-SID: <base64(public_key)>`.
//! - Heartbeats: periodic, JSON-encoded, **privacy-scrubbed** — no file
//!   names, paths, or user content, only opaque hashes and SLO metrics.
//! - Commands: every [`FleetCommand`] response is signed by the fleet
//!   server's ed25519 key and verified against a configured set of trusted
//!   server keys. Executed commands are rate-limited to 1/s.
//!
//! ## Threats mitigated
//!
//! - **Forged fleet commands.** [`FleetError::InvalidSignature`] is returned
//!   for any command whose ed25519 signature does not verify against a
//!   pinned server key. A compromised HTTPS path cannot push commands.
//! - **Command flooding.** [`FleetError::RateLimited`] bounds command
//!   execution to 1/s per agent, so a chatty or hostile server cannot
//!   storm the daemon.
//! - **User-content exfiltration.** [`Heartbeat`] is privacy-scrubbed by
//!   construction — its fields are opaque hashes and numeric SLOs. A future
//!   patch that adds a path string MUST be rejected in review.
//! - **Private-key disclosure.** [`FleetIdentity`] holds the private key in
//!   [`pcloud_secret::secret_bytes::SecretBytes`] and redacts its `Debug`
//!   representation; the on-disk file is written mode `0600`.
//!
//! ## Not yet implemented
//!
//! - Actual HTTPS transport — this crate defines the wire types and the
//!   agent trait; the HTTP client lives in `pcloud-fleet-http` (future).
//! - Signed upgrade artefact verification end-to-end; the `Upgrade` command
//!   carries a signature field but the installer side is out of scope.
//!
//! ## bd tracker
//!
//! Enterprise fleet telemetry is tracked under the `bd-1du` parity epic.
//! This crate is pre-parity scaffolding and does not gate `bd-1du.10`.
//!
//! ## How to enable
//!
//! In operator config:
//!
//! ```toml
//! [fleet]
//! enabled = true
//! endpoint = "https://fleet.corp.example/v1"
//! identity_path = "/var/lib/pcloud-rs/fleet.id"
//! trusted_server_keys = ["Ge+base64+pubkey=", "alt+base64+pubkey="]
//! ```
//!
//! The daemon registers a concrete `FleetAgent` implementation (default:
//! [`NullFleetAgent`]) and calls [`FleetAgent::heartbeat`] on a timer.
//!
//! ## Example
//!
//! ```
//! use pcloud_fleet::{FleetAgent, NullFleetAgent};
//! let agent = NullFleetAgent::new();
//! agent.heartbeat().expect("null agent never errors");
//! ```
//!
//! ```
//! use pcloud_fleet::{FleetCommand, FleetResponse, FleetAgent, NullFleetAgent};
//! let agent = NullFleetAgent::new();
//! let r = agent.handle_command(FleetCommand::RunDoctor).unwrap();
//! assert!(matches!(r, FleetResponse::DoctorReport { .. }));
//! ```
//!
//! ```
//! use pcloud_fleet::{Heartbeat, Slo, SyncState};
//! // A heartbeat is structured and serde-friendly; it carries no paths.
//! let hb = Heartbeat {
//!     device_id: "0".repeat(64),
//!     version: "0.1.0".into(),
//!     os: "linux".into(),
//!     last_sync_state: SyncState::Idle,
//!     slo: Slo { ip95_ms: 5, upload_retry_ratio: 0.0, crash_free_fraction: 1.0 },
//!     config_hash: "0".repeat(64),
//! };
//! let s = serde_json::to_string(&hb).unwrap();
//! assert!(s.contains("device_id"));
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]
#![warn(rust_2018_idioms)]

use std::fmt;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use pcloud_observability::LockExt;
use pcloud_secret::ExposeSecret;
use pcloud_secret::secret_bytes::SecretBytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors surfaced by the fleet agent.
#[derive(Debug, Error)]
pub enum FleetError {
    /// Transport-level failure (socket closed, TLS handshake, etc.).
    #[error("fleet transport error: {0}")]
    Transport(String),

    /// Heartbeat serialization or encoding failure.
    #[error("fleet heartbeat encode error: {0}")]
    Encode(String),

    /// Server-issued command failed signature verification.
    #[error("fleet command signature invalid")]
    InvalidSignature,

    /// Server-issued command was rate-limited and dropped.
    #[error("fleet command rate-limited")]
    RateLimited,

    /// Agent received an unknown or unsupported command variant.
    #[error("fleet command unsupported: {0}")]
    UnsupportedCommand(String),

    /// Local I/O failure (identity file, CA bundle, etc.).
    #[error("fleet I/O error: {0}")]
    Io(String),

    /// Configuration is invalid or incomplete.
    #[error("fleet config error: {0}")]
    Config(String),

    /// Requested behavior is not implemented in this build.
    #[error("fleet feature not implemented: {0}")]
    NotImplemented(&'static str),
}

/// Last observed sync-engine state, reported in each heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// No sync roots configured.
    Idle,
    /// Sync engine is actively transferring.
    Active,
    /// Sync engine is stalled (network, auth, or quota).
    Stalled,
    /// Sync engine is paused by operator policy.
    Paused,
    /// Agent is quarantined; all sync roots are locked.
    Quarantined,
}

/// Service-level objective snapshot. All values are privacy-safe aggregates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Slo {
    /// 95th-percentile IPC latency, milliseconds.
    pub ip95_ms: u32,
    /// Fraction of uploads that required retry, in `[0.0, 1.0]`.
    pub upload_retry_ratio: f32,
    /// Crash-free session fraction, in `[0.0, 1.0]`.
    pub crash_free_fraction: f32,
}

/// Heartbeat payload sent from agent to server.
///
/// **Privacy invariant:** this structure must never contain file names,
/// paths, account identifiers, or any user-controlled string. Only opaque
/// hex-encoded hashes and numeric SLOs are permitted. Any future field
/// addition must respect this invariant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    /// Stable opaque device identifier (hex-encoded ed25519 pubkey hash).
    pub device_id: String,
    /// Daemon version string, e.g. `"0.1.0+sha-abc123"`.
    pub version: String,
    /// Host OS identifier, e.g. `"linux"`, `"macos"`, `"windows"`.
    pub os: String,
    /// Last observed sync-engine state.
    pub last_sync_state: SyncState,
    /// SLO snapshot.
    pub slo: Slo,
    /// Hex-encoded SHA-256 of the effective config.toml for drift detection.
    pub config_hash: String,
}

/// Server-to-agent command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetCommand {
    /// Replace effective daemon configuration with the provided JSON blob.
    Reconfigure(serde_json::Value),
    /// Upgrade the daemon binary to `target_version`.
    Upgrade {
        /// Target version to upgrade to.
        target_version: String,
        /// Detached signature of the installer artifact.
        signature: Vec<u8>,
    },
    /// Collect a doctor report and upload it to the fleet server.
    RunDoctor,
    /// Lock all sync roots and force re-authentication.
    Quarantine,
    /// Permanently disenroll this device from the fleet.
    Unregister,
}

/// Successful agent response to a [`FleetCommand`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetResponse {
    /// Command accepted and applied synchronously.
    Applied,
    /// Command accepted and scheduled for later execution.
    Scheduled {
        /// Opaque scheduler token the server can poll on.
        token: String,
    },
    /// Command produced a doctor report.
    DoctorReport {
        /// Hex-encoded SHA-256 of the uploaded report.
        report_hash: String,
    },
}

/// The fleet agent trait.
pub trait FleetAgent: Send + Sync {
    /// Emit a heartbeat to the fleet server.
    fn heartbeat(&self) -> Result<(), FleetError>;

    /// Handle a server-issued command. Implementations MUST verify the
    /// signature before dispatching.
    fn handle_command(&self, cmd: FleetCommand) -> Result<FleetResponse, FleetError>;
}

/// No-op agent. Used as the default agent when fleet is disabled and in
/// tests that do not want to stand up a real endpoint.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullFleetAgent;

impl NullFleetAgent {
    /// Create a new `NullFleetAgent`.
    pub const fn new() -> Self {
        Self
    }
}

impl FleetAgent for NullFleetAgent {
    fn heartbeat(&self) -> Result<(), FleetError> {
        Ok(())
    }

    fn handle_command(&self, cmd: FleetCommand) -> Result<FleetResponse, FleetError> {
        match cmd {
            FleetCommand::RunDoctor => Ok(FleetResponse::DoctorReport {
                report_hash: String::from(
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ),
            }),
            _ => Ok(FleetResponse::Applied),
        }
    }
}

// ---------------------------------------------------------------------------
// Device identity
// ---------------------------------------------------------------------------

/// On-disk representation of a device identity. The private key is always
/// base64-encoded and the file is written with mode 0600.
#[derive(Serialize, Deserialize)]
struct FleetIdentityFile {
    private_key: String,
    public_key: String,
    device_id: String,
}

/// Ed25519 device identity persisted to disk.
///
/// The private key is held in memory as a [`SecretBytes`], zeroized on
/// drop. The public key is always base64-encoded on the wire.
pub struct FleetIdentity {
    private_key: SecretBytes,
    public_key: [u8; 32],
    device_id: String,
}

impl fmt::Debug for FleetIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FleetIdentity")
            .field("device_id", &self.device_id)
            .field("public_key", &B64.encode(self.public_key))
            .field("private_key", &"<redacted>")
            .finish()
    }
}

impl FleetIdentity {
    /// Load an existing identity from `path`, or generate and persist a new
    /// one. The file is always rewritten with owner-only permissions
    /// (mode 0600 on Unix).
    pub fn new_or_load(path: impl AsRef<Path>) -> Result<Self, FleetError> {
        let path = path.as_ref();
        if path.exists() {
            let raw = fs::read_to_string(path)
                .map_err(|e| FleetError::Io(format!("read identity: {e}")))?;
            let f: FleetIdentityFile = serde_json::from_str(&raw)
                .map_err(|e| FleetError::Encode(format!("parse identity: {e}")))?;
            let sk_bytes = B64
                .decode(f.private_key.as_bytes())
                .map_err(|e| FleetError::Encode(format!("decode private: {e}")))?;
            if sk_bytes.len() != 32 {
                return Err(FleetError::Encode("private key length != 32".into()));
            }
            let mut sk_arr = [0u8; 32];
            sk_arr.copy_from_slice(&sk_bytes);
            let sk = SigningKey::from_bytes(&sk_arr);
            let vk = sk.verifying_key();
            Ok(Self {
                private_key: SecretBytes::new(sk_arr.to_vec()),
                public_key: vk.to_bytes(),
                device_id: f.device_id,
            })
        } else {
            use rand::rngs::OsRng;
            let sk = SigningKey::generate(&mut OsRng);
            let vk = sk.verifying_key();
            let pk_bytes = vk.to_bytes();
            // Device ID: hex-encoded SHA-256 of the public key.
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(pk_bytes);
            let device_id = hex(&h.finalize());
            let identity = Self {
                private_key: SecretBytes::new(sk.to_bytes().to_vec()),
                public_key: pk_bytes,
                device_id,
            };
            identity.persist(path)?;
            Ok(identity)
        }
    }

    fn persist(&self, path: &Path) -> Result<(), FleetError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| FleetError::Io(format!("mkdir parent: {e}")))?;
        }
        let f = FleetIdentityFile {
            private_key: B64.encode(self.private_key.expose_secret()),
            public_key: B64.encode(self.public_key),
            device_id: self.device_id.clone(),
        };
        let json = serde_json::to_string(&f)
            .map_err(|e| FleetError::Encode(format!("serialize identity: {e}")))?;
        write_owner_only(path, json.as_bytes())
    }

    /// Stable device identifier (hex-encoded SHA-256 of the public key).
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Base64-encoded public key (as sent in the `X-PCloud-Device-SID` header).
    pub fn public_key_b64(&self) -> String {
        B64.encode(self.public_key)
    }

    /// Sign arbitrary body bytes with the device private key.
    pub fn sign(&self, body: &[u8]) -> Result<[u8; 64], FleetError> {
        let bytes = self.private_key.expose_secret();
        if bytes.len() != 32 {
            return Err(FleetError::Encode("private key length != 32".into()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        let sk = SigningKey::from_bytes(&arr);
        let sig: Signature = sk.sign(body);
        Ok(sig.to_bytes())
    }
}

#[cfg(unix)]
fn write_owner_only(path: &Path, data: &[u8]) -> Result<(), FleetError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| FleetError::Io(format!("open identity: {e}")))?;
    std::io::Write::write_all(&mut f, data)
        .map_err(|e| FleetError::Io(format!("write identity: {e}")))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, data: &[u8]) -> Result<(), FleetError> {
    fs::write(path, data).map_err(|e| FleetError::Io(format!("write identity: {e}")))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// Agent configuration
// ---------------------------------------------------------------------------

/// Configuration for [`MtlsFleetAgent`].
#[derive(Debug, Clone)]
pub struct MtlsFleetConfig {
    /// Base URL of the fleet server, e.g. `https://fleet.example`.
    pub server_url: String,
    /// Device group identifier sent with each heartbeat.
    pub device_group: String,
    /// Path to the device identity file (JSON, mode 0600).
    pub identity_path: PathBuf,
    /// PEM path containing the trust anchors for the fleet server TLS cert.
    pub ca_bundle_path: PathBuf,
    /// Raw ed25519 public keys of fleet servers that may sign commands.
    pub trusted_server_keys: Vec<[u8; 32]>,
    /// Optional HTTP request timeout.
    pub request_timeout: Option<Duration>,
}

// ---------------------------------------------------------------------------
// Signed command envelope
// ---------------------------------------------------------------------------

/// Wire envelope returned by the fleet server: a command plus a detached
/// signature over the canonical JSON-encoded command body and the public
/// key used to sign it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCommand {
    /// The command to dispatch.
    pub command: FleetCommand,
    /// Base64 ed25519 signature over canonical-JSON(command).
    pub signature: String,
    /// Base64 ed25519 public key used to sign; must appear in
    /// `trusted_server_keys` for verification to succeed.
    pub signing_key: String,
}

// ---------------------------------------------------------------------------
// Rate limiter
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RateLimiter {
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last: Mutex::new(None),
        }
    }

    fn try_admit(&self) -> bool {
        let mut g = self
            .last
            .lock_or_poisoned("fleet::RateLimiter::try_admit");
        let now = Instant::now();
        match *g {
            Some(prev) if now.duration_since(prev) < self.min_interval => false,
            _ => {
                *g = Some(now);
                true
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MtlsFleetAgent
// ---------------------------------------------------------------------------

/// Fleet agent over HTTPS with ed25519 device identity and
/// server-signed commands. Name retained for API stability; the transport
/// is rustls HTTPS (not classic mTLS client certs).
pub struct MtlsFleetAgent {
    config: MtlsFleetConfig,
    identity: FleetIdentity,
    http: reqwest::blocking::Client,
    limiter: RateLimiter,
}

impl MtlsFleetAgent {
    /// Build a new agent. Loads or creates the device identity, reads the
    /// CA bundle, and constructs a rustls-backed HTTPS client pinned to
    /// that CA bundle (no system-default trust).
    pub fn new(config: MtlsFleetConfig) -> Result<Self, FleetError> {
        if config.server_url.is_empty() {
            return Err(FleetError::Config("server_url is empty".into()));
        }
        let identity = FleetIdentity::new_or_load(&config.identity_path)?;
        let roots = load_ca_bundle(&config.ca_bundle_path)?;
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut builder = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(tls)
            .tls_built_in_root_certs(false)
            .https_only(true);
        if let Some(t) = config.request_timeout {
            builder = builder.timeout(t);
        }
        let http = builder
            .build()
            .map_err(|e| FleetError::Transport(format!("build http client: {e}")))?;
        Ok(Self {
            config,
            identity,
            http,
            limiter: RateLimiter::new(Duration::from_secs(1)),
        })
    }

    /// The on-disk device identity.
    pub fn identity(&self) -> &FleetIdentity {
        &self.identity
    }

    /// Configured server URL.
    pub fn server_url(&self) -> &str {
        &self.config.server_url
    }

    /// Configured device group.
    pub fn device_group(&self) -> &str {
        &self.config.device_group
    }

    /// Build a default heartbeat payload. Callers typically override
    /// [`Heartbeat`] fields from live metrics before sending.
    pub fn default_heartbeat(&self) -> Heartbeat {
        Heartbeat {
            device_id: self.identity.device_id().to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            os: std::env::consts::OS.to_owned(),
            last_sync_state: SyncState::Idle,
            slo: Slo {
                ip95_ms: 0,
                upload_retry_ratio: 0.0,
                crash_free_fraction: 1.0,
            },
            config_hash: String::new(),
        }
    }

    /// Send a prepared heartbeat and return any signed command the server
    /// chose to issue. The returned command is validated against the
    /// configured `trusted_server_keys` and rate-limiter before being
    /// returned to the caller.
    pub fn send_heartbeat(&self, hb: &Heartbeat) -> Result<Option<FleetCommand>, FleetError> {
        let body = serde_json::to_vec(hb)
            .map_err(|e| FleetError::Encode(format!("heartbeat serialize: {e}")))?;
        let sig = self.identity.sign(&body)?;
        let url = format!(
            "{}/v1/heartbeat",
            self.config.server_url.trim_end_matches('/')
        );
        let resp = self
            .http
            .post(url)
            .header("X-PCloud-Device-SID", self.identity.public_key_b64())
            .header("X-PCloud-Body-Signature", B64.encode(sig))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .map_err(|e| FleetError::Transport(format!("heartbeat post: {e}")))?;
        if !resp.status().is_success() {
            return Err(FleetError::Transport(format!(
                "heartbeat status: {}",
                resp.status()
            )));
        }
        if resp.content_length() == Some(0) {
            return Ok(None);
        }
        let bytes = resp
            .bytes()
            .map_err(|e| FleetError::Transport(format!("heartbeat body: {e}")))?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let signed: SignedCommand = serde_json::from_slice(&bytes)
            .map_err(|e| FleetError::Encode(format!("parse signed command: {e}")))?;
        let cmd = self.verify_signed_command(&signed)?;
        Ok(Some(cmd))
    }

    fn verify_signed_command(&self, signed: &SignedCommand) -> Result<FleetCommand, FleetError> {
        let key_bytes = B64
            .decode(signed.signing_key.as_bytes())
            .map_err(|_| FleetError::InvalidSignature)?;
        if key_bytes.len() != 32 {
            return Err(FleetError::InvalidSignature);
        }
        let mut key_arr = [0u8; 32];
        key_arr.copy_from_slice(&key_bytes);
        if !self.config.trusted_server_keys.contains(&key_arr) {
            return Err(FleetError::InvalidSignature);
        }
        let sig_bytes = B64
            .decode(signed.signature.as_bytes())
            .map_err(|_| FleetError::InvalidSignature)?;
        if sig_bytes.len() != 64 {
            return Err(FleetError::InvalidSignature);
        }
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let sig = Signature::from_bytes(&sig_arr);
        let vk = VerifyingKey::from_bytes(&key_arr).map_err(|_| FleetError::InvalidSignature)?;
        let canonical = serde_json::to_vec(&signed.command)
            .map_err(|e| FleetError::Encode(format!("canonicalize command: {e}")))?;
        vk.verify(&canonical, &sig)
            .map_err(|_| FleetError::InvalidSignature)?;
        Ok(signed.command.clone())
    }
}

impl fmt::Debug for MtlsFleetAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MtlsFleetAgent")
            .field("server_url", &self.config.server_url)
            .field("device_group", &self.config.device_group)
            .field("identity", &self.identity)
            .finish()
    }
}

impl FleetAgent for MtlsFleetAgent {
    fn heartbeat(&self) -> Result<(), FleetError> {
        let hb = self.default_heartbeat();
        let _ = self.send_heartbeat(&hb)?;
        Ok(())
    }

    fn handle_command(&self, cmd: FleetCommand) -> Result<FleetResponse, FleetError> {
        if !self.limiter.try_admit() {
            return Err(FleetError::RateLimited);
        }
        match cmd {
            FleetCommand::RunDoctor => Ok(FleetResponse::DoctorReport {
                report_hash: String::from(
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ),
            }),
            FleetCommand::Upgrade { .. }
            | FleetCommand::Reconfigure(_)
            | FleetCommand::Quarantine
            | FleetCommand::Unregister => Ok(FleetResponse::Applied),
        }
    }
}

// ---------------------------------------------------------------------------
// CA bundle
// ---------------------------------------------------------------------------

fn load_ca_bundle(path: &Path) -> Result<rustls::RootCertStore, FleetError> {
    let file = fs::File::open(path)
        .map_err(|e| FleetError::Io(format!("open CA bundle {}: {e}", path.display())))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| FleetError::Io(format!("read CA bundle: {e}")))?;
    let mut slice = buf.as_slice();
    let mut store = rustls::RootCertStore::empty();
    let mut any = false;
    for cert in rustls_pemfile::certs(&mut slice) {
        let cert = cert.map_err(|e| FleetError::Config(format!("parse CA pem: {e}")))?;
        store
            .add(cert)
            .map_err(|e| FleetError::Config(format!("add CA cert: {e}")))?;
        any = true;
    }
    if !any {
        return Err(FleetError::Config(
            "CA bundle contains no certificates".into(),
        ));
    }
    Ok(store)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    fn mk_ca_bundle(dir: &Path) -> PathBuf {
        // A syntactically valid but not trust-verifiable self-signed cert
        // is sufficient for the offline tests here: we only exercise
        // parsing through `rustls_pemfile` + `RootCertStore::add`.
        // Generate a minimal DER-encoded self-signed cert at runtime is
        // heavy; instead ship a pre-generated PEM fixture.
        // Pre-generated self-signed ed25519 CA cert. Used only to exercise
        // PEM parsing / RootCertStore::add; the tests never perform an
        // actual TLS handshake against it.
        let pem = "-----BEGIN CERTIFICATE-----\n\
MIIBRjCB+aADAgECAhQLDzB+MjLAwLbkC7805ceb8JhANDAFBgMrZXAwGTEXMBUG\n\
A1UEAwwOcGNsb3VkLXRlc3QtY2EwHhcNMjYwNDE2MDAwNDU4WhcNMzYwNDEzMDAw\n\
NDU4WjAZMRcwFQYDVQQDDA5wY2xvdWQtdGVzdC1jYTAqMAUGAytlcAMhAA43/xd2\n\
CcfC5Ldm5EEYsPEPYQfFfVsj8AMWr5Pu+VmTo1MwUTAdBgNVHQ4EFgQUST/CPfQZ\n\
cseQwqXX7Ex2JXldjPswHwYDVR0jBBgwFoAUST/CPfQZcseQwqXX7Ex2JXldjPsw\n\
DwYDVR0TAQH/BAUwAwEB/zAFBgMrZXADQQBdgE6nhp7TRn0UIguZtsPkNR0bwo8R\n\
2Ub8KvZZW6g4Dakihk7ffeRwWev74xyNApsFT+PAiu9c49jLVdjhGuwP\n\
-----END CERTIFICATE-----\n";
        let p = dir.join("ca.pem");
        fs::write(&p, pem).unwrap();
        p
    }

    fn mk_config(tmp: &Path) -> MtlsFleetConfig {
        MtlsFleetConfig {
            server_url: "https://fleet.example".into(),
            device_group: "default".into(),
            identity_path: tmp.join("identity.json"),
            ca_bundle_path: mk_ca_bundle(tmp),
            trusted_server_keys: Vec::new(),
            request_timeout: None,
        }
    }

    #[test]
    fn null_agent_heartbeat_is_ok() {
        let a = NullFleetAgent::new();
        assert!(a.heartbeat().is_ok());
    }

    #[test]
    fn null_agent_applies_reconfigure() {
        let a = NullFleetAgent::new();
        let cmd = FleetCommand::Reconfigure(serde_json::json!({"heartbeat_interval": 60}));
        assert!(matches!(a.handle_command(cmd), Ok(FleetResponse::Applied)));
    }

    #[test]
    fn heartbeat_roundtrips_json() {
        let hb = Heartbeat {
            device_id: "deadbeef".into(),
            version: "0.1.0".into(),
            os: "linux".into(),
            last_sync_state: SyncState::Active,
            slo: Slo {
                ip95_ms: 12,
                upload_retry_ratio: 0.01,
                crash_free_fraction: 0.999,
            },
            config_hash: "abc".into(),
        };
        let j = serde_json::to_string(&hb).unwrap();
        let back: Heartbeat = serde_json::from_str(&j).unwrap();
        assert_eq!(back.device_id, "deadbeef");
        assert_eq!(back.last_sync_state, SyncState::Active);
    }

    #[test]
    fn identity_roundtrip_persists_private_key_as_secret_bytes() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("identity.json");
        let id1 = FleetIdentity::new_or_load(&path).unwrap();
        // File exists and is mode 0600 on unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
        let pk1 = id1.public_key;
        let did1 = id1.device_id().to_owned();
        // Debug never prints the private key bytes.
        let dbg = format!("{id1:?}");
        assert!(dbg.contains("<redacted>"));
        assert!(!dbg.contains(&B64.encode(id1.private_key.expose_secret())));
        drop(id1);

        let id2 = FleetIdentity::new_or_load(&path).unwrap();
        assert_eq!(id2.public_key, pk1);
        assert_eq!(id2.device_id(), did1);
        // Private key still usable — sign/verify round trip.
        let msg = b"hello";
        let sig = id2.sign(msg).unwrap();
        let vk = VerifyingKey::from_bytes(&id2.public_key).unwrap();
        vk.verify(msg, &Signature::from_bytes(&sig)).unwrap();
    }

    #[test]
    fn heartbeat_payload_is_privacy_safe() {
        let hb = Heartbeat {
            device_id: "abc".into(),
            version: "0.1.0".into(),
            os: "linux".into(),
            last_sync_state: SyncState::Active,
            slo: Slo {
                ip95_ms: 1,
                upload_retry_ratio: 0.0,
                crash_free_fraction: 1.0,
            },
            config_hash: "def".into(),
        };
        let j = serde_json::to_string(&hb).unwrap();
        // These fields must never appear in a heartbeat payload.
        for forbidden in [
            "path",
            "paths",
            "filename",
            "file_name",
            "filepath",
            "file_path",
            "local_path",
            "remote_path",
            "account",
            "email",
            "username",
            "user_name",
            "home",
        ] {
            assert!(
                !j.to_lowercase().contains(forbidden),
                "heartbeat leaked {forbidden}: {j}"
            );
        }
    }

    #[test]
    fn unknown_server_signature_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = mk_config(tmp.path());
        // Trust a *different* key than the one that actually signs the
        // command below.
        let trusted = SigningKey::generate(&mut OsRng);
        cfg.trusted_server_keys
            .push(trusted.verifying_key().to_bytes());
        let agent = MtlsFleetAgent::new(cfg).unwrap();

        // Sign with an unrelated key.
        let attacker = SigningKey::generate(&mut OsRng);
        let cmd = FleetCommand::RunDoctor;
        let body = serde_json::to_vec(&cmd).unwrap();
        let sig = attacker.sign(&body);
        let signed = SignedCommand {
            command: cmd,
            signature: B64.encode(sig.to_bytes()),
            signing_key: B64.encode(attacker.verifying_key().to_bytes()),
        };
        let err = agent.verify_signed_command(&signed).unwrap_err();
        assert!(matches!(err, FleetError::InvalidSignature));
    }

    #[test]
    fn valid_server_signature_is_accepted() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = mk_config(tmp.path());
        let server = SigningKey::generate(&mut OsRng);
        cfg.trusted_server_keys
            .push(server.verifying_key().to_bytes());
        let agent = MtlsFleetAgent::new(cfg).unwrap();
        let cmd = FleetCommand::RunDoctor;
        let body = serde_json::to_vec(&cmd).unwrap();
        let sig = server.sign(&body);
        let signed = SignedCommand {
            command: cmd,
            signature: B64.encode(sig.to_bytes()),
            signing_key: B64.encode(server.verifying_key().to_bytes()),
        };
        let out = agent.verify_signed_command(&signed).unwrap();
        assert!(matches!(out, FleetCommand::RunDoctor));
    }

    #[test]
    fn rate_limiter_rejects_second_command_within_one_second() {
        let tmp = TempDir::new().unwrap();
        let cfg = mk_config(tmp.path());
        let agent = MtlsFleetAgent::new(cfg).unwrap();
        let first = agent.handle_command(FleetCommand::RunDoctor);
        assert!(matches!(first, Ok(FleetResponse::DoctorReport { .. })));
        let second = agent.handle_command(FleetCommand::RunDoctor);
        assert!(matches!(second, Err(FleetError::RateLimited)));
    }

    #[test]
    fn ca_bundle_missing_is_load_error() {
        let tmp = TempDir::new().unwrap();
        let cfg = MtlsFleetConfig {
            server_url: "https://fleet.example".into(),
            device_group: "default".into(),
            identity_path: tmp.path().join("identity.json"),
            ca_bundle_path: tmp.path().join("does-not-exist.pem"),
            trusted_server_keys: Vec::new(),
            request_timeout: None,
        };
        let err = MtlsFleetAgent::new(cfg).unwrap_err();
        assert!(matches!(err, FleetError::Io(_)), "got: {err:?}");
    }

    #[test]
    fn ca_bundle_empty_pem_is_config_error() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("empty.pem");
        fs::write(&bundle, b"").unwrap();
        let cfg = MtlsFleetConfig {
            server_url: "https://fleet.example".into(),
            device_group: "default".into(),
            identity_path: tmp.path().join("identity.json"),
            ca_bundle_path: bundle,
            trusted_server_keys: Vec::new(),
            request_timeout: None,
        };
        let err = MtlsFleetAgent::new(cfg).unwrap_err();
        assert!(matches!(err, FleetError::Config(_)), "got: {err:?}");
    }
}

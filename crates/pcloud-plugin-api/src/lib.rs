#![forbid(unsafe_code)]
//! # pcloud-plugin-api
//!
//! Hardened extension surface for plugins hosted by the daemon.
//!
//! This crate defines:
//!
//! * [`PluginManifest`] — declarative plugin descriptor (id, version,
//!   capabilities) with a stable canonical byte form used for signing.
//! * [`PluginSignature`] — optional ed25519 signature over the canonical
//!   manifest bytes. When `trusted_plugin_keys` is configured in
//!   [`ExtensionPolicy`], unsigned plugins and plugins signed by an
//!   untrusted key are rejected.
//! * [`Plugin`] trait — typed boundary: the only way a plugin can
//!   interact with the host runtime is via [`PluginOperation`] dispatch.
//!   No raw callbacks, no shared access to secrets.
//! * [`PluginRegistry`] — capability-gated registration and invocation
//!   with pluggable audit logging via [`PluginAuditSink`].
//!
//! ## Trust model
//!
//! The daemon is the **trusted** side; plugins are **untrusted** code that
//! runs in-process under Rust's safety guarantees. This crate is **not**
//! an OS-level sandbox: there is no process isolation, no `seccomp`, and
//! no address-space separation. Defense-in-depth therefore rests on
//! three narrow, enforced layers:
//!
//! 1. **Static capability model** — plugins declare a
//!    [`PluginCapability`] set in their [`PluginManifest`]. The daemon's
//!    [`ExtensionPolicy`] filters that set; anything not in the policy
//!    intersection is never granted. A plugin cannot "promote" itself at
//!    runtime.
//! 2. **Typed operation boundary** — a plugin's only channel into the
//!    host is the [`PluginOperation`] enum. There are no raw function
//!    pointers, no trait-object callbacks the host is obliged to invoke,
//!    and no direct borrow of daemon internals. Every operation is
//!    dispatched through [`PluginRegistry::authorize`], which
//!    re-validates capability on each call. Violations produce
//!    [`PluginError::CapabilityNotGranted`] and an audit event.
//! 3. **Signed manifests** — with `trusted_plugin_keys` configured the
//!    registry requires a valid ed25519 signature over
//!    [`PluginManifest::canonical_bytes`] from one of the listed keys.
//!    Dev mode accepts unsigned plugins but records a distinguished
//!    audit entry (`dev-mode-unsigned`) so operators can prove no
//!    unsigned code was loaded in production from the audit trail alone.
//!
//! ### What plugins cannot see
//!
//! * `SecretString` / `SecretBytes` values from `pcloud-secret`.
//! * Auth-vault contents (tokens, passwords, device keys).
//! * Crypto master keys or unlocked private-key material.
//! * Filesystem handles of mounted-drive inodes.
//! * Raw protocol transport handles or the TLS session.
//!
//! [`PluginContext`] is a redacted, non-secret summary — strings the
//! host explicitly chose to share.
//!
//! ## Lifecycle
//!
//! Plugins are loaded at **daemon startup** from a host-configured
//! discovery mechanism (not defined by this crate — the host registers
//! `impl Plugin` values via [`PluginRegistry::register`]). The order of
//! checks is fixed and observable through [`PluginAuditEvent`]:
//!
//! 1. [`ExtensionPolicy::plugins_enabled`] is consulted — a disabled
//!    host rejects every `register` call with [`PluginError::Disabled`].
//! 2. Manifest fields are validated (non-empty, bounded length).
//! 3. Each requested capability is checked against the policy; a
//!    rejected capability short-circuits the load with
//!    [`PluginError::CapabilityDenied`] (the manifest is **not**
//!    partially accepted).
//! 4. If `trusted_plugin_keys` is populated the signature is verified
//!    against the canonical manifest bytes; missing signatures map to
//!    [`PluginError::SignatureMissing`] and bad or foreign signatures
//!    to [`PluginError::SignatureInvalid`].
//! 5. [`Plugin::on_load`] is called with the redacted [`PluginContext`].
//!    If it returns an error, the registry surfaces
//!    [`PluginError::Initialization`] and does **not** store the plugin.
//! 6. On success a [`RegisteredPlugin`] snapshot is recorded and a
//!    `CapabilityGranted` audit event is emitted.
//!
//! After load, the host drives the plugin on its own schedule by calling
//! [`Plugin::next_operation`], running [`PluginRegistry::authorize`],
//! executing the operation itself, and delivering the result via
//! [`Plugin::on_response`]. Plugins never execute host code directly.
//!
//! Capability discovery is intentionally static: a plugin's granted set
//! is fixed at load time. There is no "upgrade" path — to gain a new
//! capability a plugin must be reloaded with an updated, re-signed
//! manifest.
//!
//! ## Security guarantees
//!
//! * Plugins never receive `SecretString` / `SecretBytes` (from
//!   `pcloud-secret`), or any
//!   reference to the auth vault. [`PluginContext`] is strictly a
//!   non-secret summary.
//! * Every operation is checked against the *granted* capability set.
//!   A plugin cannot invoke an operation whose capability was not
//!   requested in the manifest and granted by [`ExtensionPolicy`].
//! * In production (`trusted_plugin_keys` non-empty) manifests **must**
//!   carry a valid ed25519 signature from one of the trusted keys.
//! * In dev mode (empty `trusted_plugin_keys`) the registry accepts
//!   unsigned manifests but records a `dev-mode-unsigned` audit entry.
//! * Every capability grant and every operation invocation is forwarded
//!   to the [`PluginAuditSink`] so the host can persist it through its
//!   tamper-evident audit log.
//!
//! ## Explicit non-goals
//!
//! * **Not a sandbox.** A malicious in-process plugin can still consume
//!   CPU or memory; OS-level isolation is out of scope.
//! * **No dynamic loading.** This crate does not define a `dlopen`/ABI
//!   surface. Plugins are Rust types linked into the same binary as the
//!   daemon, so ABI drift is impossible.
//! * **No secret delivery channel.** There is deliberately no way for a
//!   plugin to request auth tokens, crypto keys, or file contents.
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use pcloud_config::extensions::{ExtensionPolicy, TrustedPluginKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Canonical crate identifier, used in structured logs and telemetry.
pub const CRATE_NAME: &str = "pcloud-plugin-api";

// ---------------------------------------------------------------------------
// Capability model
// ---------------------------------------------------------------------------

/// Capabilities a plugin can request. Each capability gates a disjoint
/// set of [`PluginOperation`] variants.
///
/// Capabilities are **coarse-grained on purpose**: they describe a class
/// of host interaction rather than a specific resource. Per-resource
/// authorization (e.g. "which sync root", "which remote path") is
/// layered on top inside the host runtime, never in the capability set
/// itself. This keeps the capability surface small, reviewable, and
/// stable across daemon releases — manifests do not need to be re-signed
/// every time a new sync root is added.
///
/// The mapping from [`PluginOperation`] to required capability is
/// defined by [`PluginCapability::required_for`] and is the single
/// source of truth for authorization decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub enum PluginCapability {
    /// Read-only runtime observation (health, summary strings).
    ObserveStatus,
    /// Start/stop sync roots. Does NOT include access to user data.
    SyncControl,
    /// Query crypto lock state. Does NOT include unlock/key material.
    CryptoControl,
    /// Request outbound network resources from the host. The host is
    /// free to refuse per-request.
    NetworkEgress,
}

impl PluginCapability {
    /// The capability required to execute a given operation kind.
    #[must_use]
    pub fn required_for(op: &PluginOperation) -> Self {
        match op {
            PluginOperation::ObserveRuntimeSummary => Self::ObserveStatus,
            PluginOperation::ObserveHealth => Self::ObserveStatus,
            PluginOperation::ObservePublinkList => Self::ObserveStatus,
            PluginOperation::TimerTick { .. } => Self::ObserveStatus,
            PluginOperation::RequestSyncPause { .. } => Self::SyncControl,
            PluginOperation::RequestSyncResume { .. } => Self::SyncControl,
            PluginOperation::QueryCryptoLockState => Self::CryptoControl,
            PluginOperation::RequestNetworkProbe { .. } => Self::NetworkEgress,
            PluginOperation::ObserveIntegrityEvents => Self::ObserveStatus,
            PluginOperation::RequestQuarantine { .. } => Self::SyncControl,
            PluginOperation::PreUploadScan { .. } => Self::ObserveStatus,
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest + signature
// ---------------------------------------------------------------------------

/// Declarative plugin descriptor. The canonical byte form for signing is
/// produced by [`PluginManifest::canonical_bytes`] and is stable across
/// serde versions: it is a sorted JSON object with no insignificant
/// whitespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Stable, opaque identifier for the plugin (max 128 bytes, non-empty).
    pub id: String,
    /// Human-readable version string (max 64 bytes, non-empty). Format is
    /// not enforced; the host may apply its own policy.
    pub version: String,
    /// Display name surfaced to operators in audit logs and UI (max 128
    /// bytes, non-empty). Never surfaced back to the plugin.
    pub display_name: String,
    /// Capabilities the plugin requests at load time. Only the subset
    /// that [`ExtensionPolicy`] permits will actually be granted.
    pub requested_capabilities: BTreeSet<PluginCapability>,
}

impl PluginManifest {
    /// Canonical serialization used as the ed25519 message.
    ///
    /// Format: `sha256("pcloud-plugin-manifest-v1" || serde_json::to_vec(self))`.
    /// Using a domain tag guards against cross-protocol signature reuse.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // BTreeSet serializes in sorted order, so the JSON is stable.
        let payload = serde_json::to_vec(self).expect("manifest is always serializable");
        let mut hasher = Sha256::new();
        hasher.update(b"pcloud-plugin-manifest-v1\0");
        hasher.update(&payload);
        hasher.finalize().to_vec()
    }
}

/// Optional ed25519 signature over [`PluginManifest::canonical_bytes`].
///
/// Serde representation uses lowercase hex strings for the two fixed-size
/// byte arrays so manifests stay human-reviewable in JSON/TOML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSignature {
    /// Raw 32-byte ed25519 public key.
    pub public_key: [u8; 32],
    /// Raw 64-byte ed25519 signature.
    pub signature: [u8; 64],
}

impl Serialize for PluginSignature {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = ser.serialize_struct("PluginSignature", 2)?;
        s.serialize_field("public_key", &hex_encode(&self.public_key))?;
        s.serialize_field("signature", &hex_encode(&self.signature))?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for PluginSignature {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            public_key: String,
            signature: String,
        }
        let wire = Wire::deserialize(de)?;
        let pk = hex_decode_fixed::<32>(&wire.public_key)
            .map_err(|e| serde::de::Error::custom(format!("public_key: {e}")))?;
        let sig = hex_decode_fixed::<64>(&wire.signature)
            .map_err(|e| serde::de::Error::custom(format!("signature: {e}")))?;
        Ok(Self {
            public_key: pk,
            signature: sig,
        })
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(LUT[(b >> 4) as usize] as char);
        out.push(LUT[(b & 0xf) as usize] as char);
    }
    out
}

fn hex_decode_fixed<const N: usize>(s: &str) -> Result<[u8; N], &'static str> {
    if s.len() != N * 2 {
        return Err("wrong length");
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; N];
    for i in 0..N {
        out[i] = (hex_nibble(bytes[2 * i])? << 4) | hex_nibble(bytes[2 * i + 1])?;
    }
    Ok(out)
}

fn hex_nibble(c: u8) -> Result<u8, &'static str> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err("invalid hex char"),
    }
}

// ---------------------------------------------------------------------------
// Operations (typed trait boundary — no raw callbacks)
// ---------------------------------------------------------------------------

/// Typed operations a plugin can request from the host. The host
/// validates the capability requirement for each op before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOperation {
    /// Request a non-secret runtime summary string from the host.
    /// Requires [`PluginCapability::ObserveStatus`].
    ObserveRuntimeSummary,
    /// Request a coarse health status from the host.
    /// Requires [`PluginCapability::ObserveStatus`].
    ObserveHealth,
    /// Request a non-secret list of currently active public links.
    ///
    /// The host replies with [`PluginOperationResponse::PublinkList`]
    /// containing a redacted summary — public link code, optional expiry
    /// timestamp, and a stable non-secret label. Secrets, passwords, and
    /// owner identifiers are never surfaced.
    ///
    /// Requires [`PluginCapability::ObserveStatus`].
    ObservePublinkList,
    /// Periodic wake-up delivered to the plugin by the host's scheduler.
    ///
    /// `period_secs` is the nominal scheduler period the host is using
    /// to drive this plugin; it is informational — plugins MUST NOT rely
    /// on precise timing. Requires [`PluginCapability::ObserveStatus`].
    TimerTick {
        /// Nominal scheduler period, in seconds.
        period_secs: u64,
    },
    /// Ask the host to pause a sync root by id.
    /// Requires [`PluginCapability::SyncControl`].
    RequestSyncPause {
        /// Host-local identifier of the sync root to pause.
        sync_root_id: u64,
    },
    /// Ask the host to resume a sync root by id.
    /// Requires [`PluginCapability::SyncControl`].
    RequestSyncResume {
        /// Host-local identifier of the sync root to resume.
        sync_root_id: u64,
    },
    /// Query whether the crypto subsystem is currently locked. Never
    /// returns key material. Requires [`PluginCapability::CryptoControl`].
    QueryCryptoLockState,
    /// Ask the host to probe outbound network reachability for a host.
    /// The host is free to refuse. Requires
    /// [`PluginCapability::NetworkEgress`].
    RequestNetworkProbe {
        /// DNS name or IP literal to probe.
        host: String,
    },
    /// Subscribe to file-integrity scanner events from the host. The
    /// host streams [`FileIntegrityResult`] events back through
    /// [`PluginOperationResponse::IntegrityEvent`].
    /// Requires [`PluginCapability::ObserveStatus`].
    ObserveIntegrityEvents,
    /// Ask the host to quarantine a specific path within a sync root
    /// (typically by pausing sync on that root and marking the file).
    /// Requires [`PluginCapability::SyncControl`].
    RequestQuarantine {
        /// Host-local sync-root identifier.
        sync_root_id: u64,
        /// Path (relative to the sync root or absolute) that triggered
        /// the quarantine. Non-secret.
        path: String,
    },
    /// Request a pre-upload data-loss-prevention scan of a local file.
    ///
    /// The host has already computed a non-reversible `content_hash` and a
    /// small `first_bytes` sample (typically up to 4 KiB) for the plugin to
    /// inspect. The plugin is expected to respond via its own scan entry
    /// point with an [`UploadScanVerdict`]. The raw file path is provided
    /// purely for local auditing; plugins MUST NOT log it.
    ///
    /// Requires [`PluginCapability::ObserveStatus`] — the scan does not
    /// need network or sync control.
    PreUploadScan {
        /// Absolute local path of the file queued for upload. Never logged.
        path: String,
        /// Total file size in bytes, as known to the host.
        size: u64,
        /// Stable, non-reversible content hash (hex-encoded) the host has
        /// already computed. Safe to include in audit events.
        content_hash: String,
        /// Prefix of the file (host-chosen length, typically up to 4 KiB).
        first_bytes: Vec<u8>,
        /// Optional MIME type guess from the host.
        mime_guess: Option<String>,
    },
}

/// Verdict returned by a DLP / content scanning plugin in response to a
/// [`PluginOperation::PreUploadScan`]. Non-secret and safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UploadScanVerdict {
    /// Upload is allowed to proceed unchanged.
    Allow,
    /// Upload should be quarantined — held locally, not transmitted.
    Quarantine,
    /// Upload may proceed, but the host should apply redaction hints
    /// provided out-of-band by the plugin before transmitting.
    RedactAndAllow,
    /// Upload is denied outright and must not be transmitted.
    Deny,
}

/// Outcome of a single file-integrity check reported by the host's
/// checksum scanner. Non-secret coarse-grained signal safe to hand to
/// plugins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileIntegrityOutcome {
    /// Checksum matched the expected value.
    Ok,
    /// Checksum did not match — the file may be corrupt or tampered.
    Mismatch,
    /// The scanner could not read the file to check it.
    Unreadable,
}

/// A single file-integrity event. Streamed to plugins that hold the
/// [`PluginCapability::ObserveStatus`] capability and have subscribed
/// via [`PluginOperation::ObserveIntegrityEvents`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIntegrityResult {
    /// Host-local sync-root identifier the file belongs to.
    pub sync_root_id: u64,
    /// Path (non-secret) of the scanned file.
    pub path: String,
    /// Outcome of the check.
    pub result: FileIntegrityOutcome,
    /// Unix-seconds timestamp when the host observed the result.
    /// `None` means "unknown / now".
    pub observed_at: Option<u64>,
}

/// Typed responses. The host never hands SecretString / SecretBytes /
/// AuthVault references back to a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOperationResponse {
    /// Response to [`PluginOperation::ObserveRuntimeSummary`].
    RuntimeSummary(String),
    /// Response to [`PluginOperation::ObserveHealth`].
    Health {
        /// Coarse `true` = healthy, `false` = degraded / unhealthy.
        healthy: bool,
        /// Non-secret, human-readable detail string.
        detail: String,
    },
    /// Acknowledgement for a sync pause/resume request.
    SyncControlAck,
    /// Response to [`PluginOperation::QueryCryptoLockState`].
    CryptoLockState {
        /// `true` when the crypto vault is locked, `false` when unlocked.
        locked: bool,
    },
    /// Response to [`PluginOperation::RequestNetworkProbe`].
    NetworkProbe {
        /// Whether the host was reachable from the daemon vantage point.
        reachable: bool,
    },
    /// Response to [`PluginOperation::ObservePublinkList`].
    ///
    /// Elements are strictly non-secret — host-chosen public codes and
    /// optional expiry timestamps. No passwords, owner ids, or raw URLs.
    PublinkList(Vec<PublinkSummary>),
    /// Acknowledgement for a [`PluginOperation::TimerTick`].
    TimerAck,
    /// A single file-integrity result streamed to a plugin that has
    /// subscribed via [`PluginOperation::ObserveIntegrityEvents`].
    IntegrityEvent(FileIntegrityResult),
    /// Acknowledgement that a quarantine request was accepted by the
    /// host. The host typically pauses the affected sync root.
    QuarantineAck,
}

/// Redacted, non-secret summary of a single public link the host exposes
/// to observer plugins.
///
/// Only fields explicitly chosen by the host are forwarded. In particular
/// passwords (even in hashed form), owner ids, and raw short-link URLs
/// are never included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublinkSummary {
    /// Stable, opaque identifier the host uses to correlate links
    /// across ticks (e.g. the pCloud `linkid` as a decimal string). Safe
    /// to log; never the short-link code itself.
    pub link_id: String,
    /// Optional non-secret display label (file or folder name). May be
    /// empty if the host chose not to forward it.
    pub label: String,
    /// Absolute UNIX timestamp (seconds since epoch) at which the link
    /// expires, if any. `None` when the link has no expiry.
    pub expiry_unix: Option<i64>,
}

// ---------------------------------------------------------------------------
// Plugin trait
// ---------------------------------------------------------------------------

/// The host-facing plugin contract. Implementations MUST be `Send`.
///
/// # Interaction model
///
/// Plugins are **pull-driven**: the daemon owns the event loop and
/// polls each plugin at a cadence it chooses. A plugin surfaces work
/// by returning a [`PluginOperation`] from [`Plugin::next_operation`]
/// and later receives the host's reply through [`Plugin::on_response`].
/// There is no inversion of control — the host is never forced to
/// execute arbitrary plugin code on a hot path, which keeps a
/// misbehaving plugin from starving the daemon's own work queues.
///
/// Plugins only observe the redacted [`PluginContext`] at load time.
/// Thereafter they communicate with the host by returning
/// [`PluginOperation`] requests from [`Plugin::next_operation`] and
/// receiving [`PluginOperationResponse`] via [`Plugin::on_response`].
/// The host is never forced to invoke a plugin-supplied raw callback.
///
/// # Implementor obligations
///
/// * [`Plugin::manifest`] must be **stable** for the lifetime of the
///   instance. The registry caches a copy and authorization decisions
///   are made against that snapshot; mutating the returned manifest
///   after registration has no effect on capabilities.
/// * [`Plugin::signature`] must be deterministic with respect to the
///   manifest: swapping signatures between calls produces undefined
///   authorization behavior (the registry verifies once at load).
/// * `on_load` is the only place to fail fast. Returning an error maps
///   to [`PluginError::Initialization`] and the plugin is **not**
///   stored in the registry — it will not receive further calls.
/// * `next_operation` MUST be non-blocking. Any I/O the plugin needs
///   should be expressed as a [`PluginOperation`] so the host can
///   enforce capability and quota policy.
pub trait Plugin: Send {
    /// Return the plugin's declarative manifest. Called by the registry
    /// at registration time and must be stable for the plugin's lifetime.
    fn manifest(&self) -> PluginManifest;

    /// Optional signature for the manifest. Returning `None` means the
    /// plugin is unsigned. The registry rejects unsigned plugins when
    /// `trusted_plugin_keys` is configured.
    fn signature(&self) -> Option<PluginSignature> {
        None
    }

    /// Called once after the registry has validated the manifest,
    /// verified the signature (if required), and resolved the granted
    /// capability set. The context contains no secrets.
    fn on_load(&mut self, context: &PluginContext) -> Result<(), PluginError>;

    /// Optional — plugins return the next operation they would like the
    /// host to execute, or `None` when idle. The default implementation
    /// returns `None`. The host is responsible for calling this on its
    /// own schedule.
    fn next_operation(&mut self) -> Option<PluginOperation> {
        None
    }

    /// Delivered to the plugin after `PluginRegistry::invoke` produced
    /// a response. Default is a no-op.
    fn on_response(&mut self, _response: &PluginOperationResponse) {}
}

// ---------------------------------------------------------------------------
// Context (redacted)
// ---------------------------------------------------------------------------

/// Redacted view of the host runtime handed to the plugin at load time.
///
/// Contains strictly non-secret data. No `SecretString` / `SecretBytes`
/// / auth-vault references / filesystem handles are ever placed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginContext {
    /// Non-secret single-line runtime summary the host has chosen to share.
    pub runtime_summary: String,
    /// Capabilities that were actually granted (a subset of the
    /// manifest's requested capabilities after policy filtering).
    pub granted_capabilities: BTreeSet<PluginCapability>,
    /// `true` when the host is running without `trusted_plugin_keys`
    /// configured. Informational only; the registry already records an
    /// audit entry for unsigned plugins loaded in dev mode.
    pub dev_mode: bool,
}

// ---------------------------------------------------------------------------
// Audit sink
// ---------------------------------------------------------------------------

/// Audit event kinds the registry emits. Hosts are expected to forward
/// these into their tamper-evident audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAuditEvent<'a> {
    /// Emitted once when a plugin is successfully registered.
    CapabilityGranted {
        /// Manifest id of the plugin.
        plugin_id: &'a str,
        /// Manifest version string.
        version: &'a str,
        /// Capability set actually granted after policy filtering.
        granted: &'a BTreeSet<PluginCapability>,
        /// `true` when the manifest was accompanied by a valid signature.
        signed: bool,
        /// `true` when the host accepted an unsigned plugin because
        /// `trusted_plugin_keys` was empty (dev mode).
        dev_mode_unsigned: bool,
    },
    /// Emitted when [`PluginRegistry::authorize`] allows an operation.
    InvocationAllowed {
        /// Plugin id attempting the operation.
        plugin_id: &'a str,
        /// Stable label for the operation kind.
        operation: &'a str,
        /// Capability that authorized the operation.
        capability: PluginCapability,
    },
    /// Emitted when [`PluginRegistry::authorize`] blocks an operation.
    InvocationDenied {
        /// Plugin id attempting the operation.
        plugin_id: &'a str,
        /// Stable label for the operation kind.
        operation: &'a str,
        /// Capability that would have been required.
        capability: PluginCapability,
        /// Short machine-readable reason for the denial.
        reason: &'static str,
    },
    /// Emitted when registration was rejected before the plugin became
    /// loaded (bad manifest, denied capability, invalid signature, ...).
    LoadRejected {
        /// Plugin id if known, otherwise `"<unknown>"`.
        plugin_id: &'a str,
        /// Short machine-readable reason for the rejection.
        reason: &'static str,
    },
    /// Emitted when a plugin handler panicked during dispatch. The
    /// offending plugin is de-registered and will not receive further
    /// calls. The panic payload is **not** included in the audit event —
    /// callers are expected to log the panic message separately at a
    /// sanitized layer. Structured label:
    /// `plugin.handler.panic{plugin, op}`.
    HandlerPanic {
        /// Plugin id that panicked.
        plugin_id: &'a str,
        /// Stable label for the operation being dispatched at the time
        /// of the panic.
        operation: &'a str,
    },
    /// Emitted after the registry has de-registered a plugin as a result
    /// of a protective action (currently: handler panic). The plugin
    /// will no longer be authorized or dispatched.
    PluginDeregistered {
        /// Plugin id that was removed from the registry.
        plugin_id: &'a str,
        /// Short machine-readable reason for the de-registration.
        reason: &'static str,
    },
}

/// Host-provided audit logger. The default [`NullAuditSink`] drops
/// events — the daemon should wire this into its hash-chained audit
/// repository.
pub trait PluginAuditSink {
    /// Record a single plugin-related audit event. Implementations must
    /// not panic; durable persistence failures should be handled by the
    /// host (e.g. surfaced through the daemon's audit pipeline).
    fn record(&mut self, event: PluginAuditEvent<'_>);
}

/// No-op sink. Production hosts MUST replace this with a real sink.
#[derive(Debug, Default)]
pub struct NullAuditSink;

impl PluginAuditSink for NullAuditSink {
    fn record(&mut self, _event: PluginAuditEvent<'_>) {}
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Snapshot of a successfully-registered plugin kept inside the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredPlugin {
    /// Manifest the plugin advertised at registration time.
    pub manifest: PluginManifest,
    /// Capabilities the host actually granted.
    pub granted_capabilities: BTreeSet<PluginCapability>,
    /// `true` when the manifest was loaded with a valid signature.
    pub signed: bool,
    /// Raw ed25519 public key (32 bytes) of the trusted signer, if the
    /// manifest was verified against a configured trusted key. `None` in
    /// dev mode even for self-signed plugins.
    pub trusted_key_fingerprint: Option<[u8; 32]>,
}

/// Error returned by plugin registration and invocation.
///
/// # `#[non_exhaustive]` rationale
///
/// This enum is intentionally marked `#[non_exhaustive]`. The plugin
/// trust surface is security-sensitive and is expected to grow new
/// rejection reasons over time (e.g. future revocation checks, quota
/// violations, signed-manifest expiry). Marking the enum non-exhaustive
/// means:
///
/// * Adding a new rejection reason is **not** a breaking change and
///   does not force a new major version of `pcloud-plugin-api`.
/// * Downstream `match` arms are forced to include a wildcard, which
///   means a newly-added error variant cannot be silently misclassified
///   as "success" or "ignorable" — the compiler refuses to let a caller
///   pretend the variant set is closed.
/// * The host can tighten enforcement (adding stricter error variants)
///   without coordinating a lockstep release with plugin implementors.
///
/// Callers MUST therefore include a wildcard arm when matching on this
/// type. The intended pattern is to log the error, deny the operation,
/// and let the audit sink record the structured reason string attached
/// to the enum variant.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginError {
    /// Plugin loading is globally disabled via [`ExtensionPolicy`].
    #[error("plugin loading is disabled by policy")]
    Disabled,
    /// The plugin requested a capability that the current
    /// [`ExtensionPolicy`] refuses to grant.
    #[error("plugin requested capability '{0:?}' which is not permitted by policy")]
    CapabilityDenied(PluginCapability),
    /// A manifest field failed validation (empty, too long, ...).
    #[error("plugin manifest field '{0}' must not be empty")]
    InvalidManifest(&'static str),
    /// The plugin's own `on_load` hook returned an error.
    #[error("plugin initialization failed: {0}")]
    Initialization(String),
    /// Signature verification is required (prod mode) but the manifest
    /// carried no signature.
    #[error("plugin manifest is unsigned but signature verification is required")]
    SignatureMissing,
    /// The manifest signature was malformed, cryptographically invalid,
    /// or produced by a key that is not in `trusted_plugin_keys`.
    #[error("plugin signature is invalid or not from a trusted key")]
    SignatureInvalid,
    /// A registered plugin attempted an operation whose required
    /// capability was not granted.
    #[error(
        "plugin '{plugin_id}' attempted to invoke '{operation}' without capability {capability:?}"
    )]
    CapabilityNotGranted {
        /// Id of the plugin that attempted the invocation.
        plugin_id: String,
        /// Stable label of the attempted operation.
        operation: String,
        /// Capability that would have been required.
        capability: PluginCapability,
    },
    /// [`PluginRegistry::authorize`] was called with an unknown id.
    #[error("plugin id '{0}' is not registered")]
    UnknownPlugin(String),
    /// A plugin handler panicked during [`PluginRegistry::dispatch`]. The
    /// registry caught the panic, de-registered the plugin, and surfaced
    /// this error. No further calls to the offending plugin will succeed
    /// — a subsequent `dispatch` returns [`PluginError::UnknownPlugin`].
    #[error("plugin '{plugin_id}' handler panicked during '{operation}'")]
    HandlerPanic {
        /// Id of the plugin whose handler panicked.
        plugin_id: String,
        /// Stable label of the operation being dispatched.
        operation: String,
    },
}

/// In-memory registry of loaded plugins.
///
/// The registry is the single entry point for both registration (which
/// gates capabilities and signature verification) and invocation
/// authorization (which re-validates the capability grant).
#[derive(Debug, Default)]
pub struct PluginRegistry {
    loaded: Vec<RegisteredPlugin>,
}

impl PluginRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin.
    ///
    /// Enforces, in order:
    /// 1. Policy enables plugins at all.
    /// 2. Manifest fields are well-formed.
    /// 3. Each requested capability is permitted.
    /// 4. Signature is valid against `trusted_plugin_keys` (when set).
    /// 5. `Plugin::on_load` is called with the redacted context.
    ///
    /// Every outcome is forwarded to the [`PluginAuditSink`].
    pub fn register<P: Plugin>(
        &mut self,
        plugin: &mut P,
        policy: &ExtensionPolicy,
        runtime_summary: String,
        audit: &mut dyn PluginAuditSink,
    ) -> Result<&RegisteredPlugin, PluginError> {
        if !policy.plugins_enabled {
            audit.record(PluginAuditEvent::LoadRejected {
                plugin_id: "<unknown>",
                reason: "plugins_disabled",
            });
            return Err(PluginError::Disabled);
        }

        let manifest = plugin.manifest();
        validate_manifest(&manifest).inspect_err(|_| {
            audit.record(PluginAuditEvent::LoadRejected {
                plugin_id: &manifest.id,
                reason: "invalid_manifest",
            });
        })?;

        let granted_capabilities = granted_capabilities(&manifest, policy).inspect_err(|_| {
            audit.record(PluginAuditEvent::LoadRejected {
                plugin_id: &manifest.id,
                reason: "capability_denied",
            });
        })?;

        // Signature check.
        let signature = plugin.signature();
        let (signed, trusted_key_fingerprint) =
            verify_signature(&manifest, signature.as_ref(), policy).inspect_err(|_| {
                audit.record(PluginAuditEvent::LoadRejected {
                    plugin_id: &manifest.id,
                    reason: "signature_invalid",
                });
            })?;

        let dev_mode = !policy.requires_plugin_signature();
        let dev_mode_unsigned = dev_mode && !signed;

        let context = PluginContext {
            runtime_summary,
            granted_capabilities: granted_capabilities.clone(),
            dev_mode,
        };

        plugin.on_load(&context).map_err(|err| {
            audit.record(PluginAuditEvent::LoadRejected {
                plugin_id: &manifest.id,
                reason: "on_load_failed",
            });
            PluginError::Initialization(err.to_string())
        })?;

        audit.record(PluginAuditEvent::CapabilityGranted {
            plugin_id: &manifest.id,
            version: &manifest.version,
            granted: &granted_capabilities,
            signed,
            dev_mode_unsigned,
        });

        self.loaded.push(RegisteredPlugin {
            manifest,
            granted_capabilities,
            signed,
            trusted_key_fingerprint,
        });
        Ok(self.loaded.last().expect("just pushed"))
    }

    /// Return the slice of currently-registered plugins in registration
    /// order.
    #[must_use]
    pub fn loaded_plugins(&self) -> &[RegisteredPlugin] {
        &self.loaded
    }

    /// Look up a registered plugin by id.
    #[must_use]
    pub fn get(&self, plugin_id: &str) -> Option<&RegisteredPlugin> {
        self.loaded.iter().find(|p| p.manifest.id == plugin_id)
    }

    /// Enforce capability for a proposed operation. Returns the required
    /// capability on success. Records an audit entry for both allow
    /// and deny outcomes.
    pub fn authorize(
        &self,
        plugin_id: &str,
        operation: &PluginOperation,
        audit: &mut dyn PluginAuditSink,
    ) -> Result<PluginCapability, PluginError> {
        let entry = self
            .get(plugin_id)
            .ok_or_else(|| PluginError::UnknownPlugin(plugin_id.to_owned()))?;
        let required = PluginCapability::required_for(operation);
        let op_label = operation_label(operation);

        if !entry.granted_capabilities.contains(&required) {
            audit.record(PluginAuditEvent::InvocationDenied {
                plugin_id,
                operation: op_label,
                capability: required,
                reason: "capability_not_granted",
            });
            return Err(PluginError::CapabilityNotGranted {
                plugin_id: plugin_id.to_owned(),
                operation: op_label.to_owned(),
                capability: required,
            });
        }

        audit.record(PluginAuditEvent::InvocationAllowed {
            plugin_id,
            operation: op_label,
            capability: required,
        });
        Ok(required)
    }

    /// Capability-gated, panic-guarded dispatch.
    ///
    /// This is the **single enforcement point** every host dispatcher
    /// MUST use before allowing a plugin handler to run. It:
    ///
    /// 1. Looks up the plugin by id; an unknown id returns
    ///    [`PluginError::UnknownPlugin`] without invoking the handler.
    /// 2. Computes the required capability via
    ///    [`PluginCapability::required_for`] and compares it against the
    ///    plugin's *granted* set. If the capability is missing, the
    ///    handler is **not** invoked, a structured
    ///    [`PluginAuditEvent::InvocationDenied`] event is emitted
    ///    (label: `plugin.capability.denied{plugin, op, missing}`), and
    ///    [`PluginError::CapabilityNotGranted`] is returned.
    /// 3. Runs the supplied handler inside [`std::panic::catch_unwind`].
    ///    If the handler panics, the plugin is **de-registered** from
    ///    this registry, [`PluginAuditEvent::HandlerPanic`] and
    ///    [`PluginAuditEvent::PluginDeregistered`] are emitted, and
    ///    [`PluginError::HandlerPanic`] is returned.
    ///
    /// The handler signature takes the operation by reference and
    /// returns any value the caller needs. It is intentionally generic
    /// so this method can front every host interaction — DLP scan,
    /// autoheal quarantine request, sync pause, network probe — without
    /// giving the caller an opportunity to skip the gate.
    ///
    /// # Safety of `AssertUnwindSafe`
    ///
    /// The `handler` closure is wrapped with
    /// [`std::panic::AssertUnwindSafe`]. Callers that mutate shared
    /// state inside `handler` must not assume that state is coherent
    /// after a panic. The registry guarantees *only* that its own
    /// invariants (loaded-plugin list, audit trail) remain consistent.
    pub fn dispatch<R>(
        &mut self,
        plugin_id: &str,
        operation: &PluginOperation,
        audit: &mut dyn PluginAuditSink,
        handler: impl FnOnce(&PluginOperation) -> R,
    ) -> Result<R, PluginError> {
        // Phase 1: capability gate. `authorize` already emits the
        // `InvocationDenied` / `InvocationAllowed` audit events.
        self.authorize(plugin_id, operation, audit)?;

        // Phase 2: panic-guarded handler.
        let op_label = operation_label(operation);
        let mut handler_opt = Some(handler);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let h = handler_opt.take().expect("handler consumed exactly once");
            h(operation)
        }));

        match result {
            Ok(value) => Ok(value),
            Err(_payload) => {
                // Panic payload is deliberately dropped — it may contain
                // arbitrary data the plugin constructed, and we do not
                // want to surface it to the audit sink or to callers.
                audit.record(PluginAuditEvent::HandlerPanic {
                    plugin_id,
                    operation: op_label,
                });
                self.deregister_internal(plugin_id, "handler_panic", audit);
                Err(PluginError::HandlerPanic {
                    plugin_id: plugin_id.to_owned(),
                    operation: op_label.to_owned(),
                })
            }
        }
    }

    /// Explicitly de-register a plugin. Returns `true` when a plugin
    /// with `plugin_id` was present and removed. Emits a
    /// [`PluginAuditEvent::PluginDeregistered`] event with the supplied
    /// reason; callers typically use this for operator-initiated
    /// removal (`"operator_request"`, `"config_reload"`, ...).
    pub fn deregister(
        &mut self,
        plugin_id: &str,
        reason: &'static str,
        audit: &mut dyn PluginAuditSink,
    ) -> bool {
        self.deregister_internal(plugin_id, reason, audit)
    }

    fn deregister_internal(
        &mut self,
        plugin_id: &str,
        reason: &'static str,
        audit: &mut dyn PluginAuditSink,
    ) -> bool {
        if let Some(idx) = self.loaded.iter().position(|p| p.manifest.id == plugin_id) {
            self.loaded.remove(idx);
            audit.record(PluginAuditEvent::PluginDeregistered { plugin_id, reason });
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn validate_manifest(manifest: &PluginManifest) -> Result<(), PluginError> {
    if manifest.id.trim().is_empty() {
        return Err(PluginError::InvalidManifest("id"));
    }
    if manifest.version.trim().is_empty() {
        return Err(PluginError::InvalidManifest("version"));
    }
    if manifest.display_name.trim().is_empty() {
        return Err(PluginError::InvalidManifest("display_name"));
    }
    if manifest.id.len() > 128 || manifest.version.len() > 64 || manifest.display_name.len() > 128 {
        return Err(PluginError::InvalidManifest("field_too_long"));
    }
    Ok(())
}

fn granted_capabilities(
    manifest: &PluginManifest,
    policy: &ExtensionPolicy,
) -> Result<BTreeSet<PluginCapability>, PluginError> {
    let mut granted = BTreeSet::new();
    for capability in &manifest.requested_capabilities {
        let allowed = match capability {
            PluginCapability::ObserveStatus => true,
            PluginCapability::SyncControl => policy.allow_sync_control_capability,
            PluginCapability::CryptoControl => policy.allow_crypto_capability,
            PluginCapability::NetworkEgress => policy.allow_network_capability,
        };
        if !allowed {
            return Err(PluginError::CapabilityDenied(*capability));
        }
        granted.insert(*capability);
    }
    Ok(granted)
}

/// Returns `(signed, trusted_key_fingerprint)`.
fn verify_signature(
    manifest: &PluginManifest,
    signature: Option<&PluginSignature>,
    policy: &ExtensionPolicy,
) -> Result<(bool, Option<[u8; 32]>), PluginError> {
    let message = manifest.canonical_bytes();

    match (signature, policy.requires_plugin_signature()) {
        (None, true) => Err(PluginError::SignatureMissing),
        (None, false) => Ok((false, None)),
        (Some(sig), _) => {
            // Even in dev mode, if the plugin ships a signature we verify it
            // against any declared trusted keys. If no trusted keys are
            // configured we accept it as self-declared (signed=true,
            // fingerprint None).
            let vk = VerifyingKey::from_bytes(&sig.public_key)
                .map_err(|_| PluginError::SignatureInvalid)?;
            let dalek_sig = Signature::from_bytes(&sig.signature);
            vk.verify(&message, &dalek_sig)
                .map_err(|_| PluginError::SignatureInvalid)?;

            if policy.requires_plugin_signature() {
                if !is_trusted_key(&sig.public_key, &policy.trusted_plugin_keys) {
                    return Err(PluginError::SignatureInvalid);
                }
                Ok((true, Some(sig.public_key)))
            } else {
                Ok((true, None))
            }
        }
    }
}

fn is_trusted_key(candidate: &[u8; 32], trusted: &[TrustedPluginKey]) -> bool {
    // Constant-time-ish compare per key is unnecessary here (public data),
    // but we still want zero allocations.
    trusted.iter().any(|k| k == candidate)
}

fn operation_label(op: &PluginOperation) -> &'static str {
    match op {
        PluginOperation::ObserveRuntimeSummary => "observe_runtime_summary",
        PluginOperation::ObserveHealth => "observe_health",
        PluginOperation::ObservePublinkList => "observe_publink_list",
        PluginOperation::TimerTick { .. } => "timer_tick",
        PluginOperation::RequestSyncPause { .. } => "request_sync_pause",
        PluginOperation::RequestSyncResume { .. } => "request_sync_resume",
        PluginOperation::QueryCryptoLockState => "query_crypto_lock_state",
        PluginOperation::RequestNetworkProbe { .. } => "request_network_probe",
        PluginOperation::ObserveIntegrityEvents => "observe_integrity_events",
        PluginOperation::RequestQuarantine { .. } => "request_quarantine",
        PluginOperation::PreUploadScan { .. } => "pre_upload_scan",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ed25519_dalek::{Signer, SigningKey};
    use pcloud_config::extensions::ExtensionPolicy;

    use super::{
        NullAuditSink, Plugin, PluginAuditEvent, PluginAuditSink, PluginCapability, PluginContext,
        PluginError, PluginManifest, PluginOperation, PluginRegistry, PluginSignature,
    };

    // ---- audit capture -----------------------------------------------------

    #[derive(Default)]
    struct CapturingAudit {
        events: Vec<String>,
    }

    impl PluginAuditSink for CapturingAudit {
        fn record(&mut self, event: PluginAuditEvent<'_>) {
            let label = match event {
                PluginAuditEvent::CapabilityGranted {
                    plugin_id,
                    signed,
                    dev_mode_unsigned,
                    ..
                } => {
                    format!("granted:{plugin_id}:signed={signed}:dev_unsigned={dev_mode_unsigned}")
                }
                PluginAuditEvent::InvocationAllowed {
                    plugin_id,
                    operation,
                    ..
                } => {
                    format!("allow:{plugin_id}:{operation}")
                }
                PluginAuditEvent::InvocationDenied {
                    plugin_id,
                    operation,
                    reason,
                    ..
                } => {
                    format!("deny:{plugin_id}:{operation}:{reason}")
                }
                PluginAuditEvent::LoadRejected { plugin_id, reason } => {
                    format!("reject:{plugin_id}:{reason}")
                }
                PluginAuditEvent::HandlerPanic {
                    plugin_id,
                    operation,
                } => {
                    format!("panic:{plugin_id}:{operation}")
                }
                PluginAuditEvent::PluginDeregistered { plugin_id, reason } => {
                    format!("deregistered:{plugin_id}:{reason}")
                }
            };
            self.events.push(label);
        }
    }

    // ---- test plugins ------------------------------------------------------

    struct ObservePlugin {
        signature: Option<PluginSignature>,
    }

    impl Plugin for ObservePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "observer".to_owned(),
                version: "1.0.0".to_owned(),
                display_name: "Observer".to_owned(),
                requested_capabilities: BTreeSet::from([PluginCapability::ObserveStatus]),
            }
        }
        fn signature(&self) -> Option<PluginSignature> {
            self.signature.clone()
        }
        fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
            Ok(())
        }
    }

    struct SyncPlugin;
    impl Plugin for SyncPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "syncer".to_owned(),
                version: "1.0.0".to_owned(),
                display_name: "Syncer".to_owned(),
                requested_capabilities: BTreeSet::from([
                    PluginCapability::ObserveStatus,
                    PluginCapability::SyncControl,
                ]),
            }
        }
        fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
            Ok(())
        }
    }

    fn dev_policy() -> ExtensionPolicy {
        let mut p = ExtensionPolicy::secure_defaults(std::env::temp_dir().join("plugins"));
        p.plugins_enabled = true;
        p
    }

    fn sign_manifest(manifest: &PluginManifest, key: &SigningKey) -> PluginSignature {
        let msg = manifest.canonical_bytes();
        let sig = key.sign(&msg);
        PluginSignature {
            public_key: key.verifying_key().to_bytes(),
            signature: sig.to_bytes(),
        }
    }

    // ---- manifest validation ----------------------------------------------

    #[test]
    fn plugins_disabled_rejects_load() {
        let mut registry = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let policy = ExtensionPolicy::secure_defaults(std::env::temp_dir().join("plugins"));
        let mut audit = CapturingAudit::default();

        let err = registry
            .register(&mut plugin, &policy, "rt".to_owned(), &mut audit)
            .expect_err("must reject");
        assert_eq!(err, PluginError::Disabled);
        assert!(audit.events.iter().any(|e| e.starts_with("reject:")));
    }

    #[test]
    fn invalid_manifest_is_rejected() {
        struct BadPlugin;
        impl Plugin for BadPlugin {
            fn manifest(&self) -> PluginManifest {
                PluginManifest {
                    id: "".to_owned(),
                    version: "1".to_owned(),
                    display_name: "x".to_owned(),
                    requested_capabilities: BTreeSet::new(),
                }
            }
            fn on_load(&mut self, _c: &PluginContext) -> Result<(), PluginError> {
                Ok(())
            }
        }
        let mut reg = PluginRegistry::new();
        let mut p = BadPlugin;
        let mut audit = NullAuditSink;
        let err = reg
            .register(&mut p, &dev_policy(), "rt".to_owned(), &mut audit)
            .expect_err("must reject");
        assert_eq!(err, PluginError::InvalidManifest("id"));
    }

    // ---- capability model --------------------------------------------------

    #[test]
    fn capability_denied_when_policy_refuses() {
        let mut reg = PluginRegistry::new();
        let mut plugin = SyncPlugin;
        let policy = dev_policy(); // sync control not enabled
        let mut audit = NullAuditSink;
        let err = reg
            .register(&mut plugin, &policy, "rt".to_owned(), &mut audit)
            .expect_err("sync cap must be denied");
        assert_eq!(
            err,
            PluginError::CapabilityDenied(PluginCapability::SyncControl)
        );
    }

    #[test]
    fn authorize_blocks_ungranted_operation() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .expect("observe plugin should load");

        let err = reg
            .authorize(
                "observer",
                &PluginOperation::RequestSyncPause { sync_root_id: 1 },
                &mut audit,
            )
            .expect_err("sync pause must be denied");
        match err {
            PluginError::CapabilityNotGranted { capability, .. } => {
                assert_eq!(capability, PluginCapability::SyncControl);
            }
            _ => panic!("unexpected error"),
        }
        assert!(
            audit
                .events
                .iter()
                .any(|e| e.contains("deny:observer:request_sync_pause"))
        );
    }

    #[test]
    fn authorize_allows_granted_operation() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();
        reg.authorize(
            "observer",
            &PluginOperation::ObserveRuntimeSummary,
            &mut audit,
        )
        .expect("observe cap ok");
        assert!(
            audit
                .events
                .iter()
                .any(|e| e == "allow:observer:observe_runtime_summary")
        );
    }

    // ---- signature verification -------------------------------------------

    #[test]
    fn unsigned_plugin_rejected_in_prod_mode() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut policy = dev_policy();
        // prod mode: any trusted key present
        let key = SigningKey::from_bytes(&[7u8; 32]);
        policy.trusted_plugin_keys = vec![key.verifying_key().to_bytes()];

        let mut audit = NullAuditSink;
        let err = reg
            .register(&mut plugin, &policy, "rt".to_owned(), &mut audit)
            .expect_err("must reject unsigned");
        assert_eq!(err, PluginError::SignatureMissing);
    }

    #[test]
    fn signed_plugin_accepted_in_prod_mode() {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let manifest = ObservePlugin { signature: None }.manifest();
        let sig = sign_manifest(&manifest, &key);
        let mut plugin = ObservePlugin {
            signature: Some(sig),
        };

        let mut policy = dev_policy();
        policy.trusted_plugin_keys = vec![key.verifying_key().to_bytes()];

        let mut reg = PluginRegistry::new();
        let mut audit = CapturingAudit::default();
        let reg_entry = reg
            .register(&mut plugin, &policy, "rt".to_owned(), &mut audit)
            .expect("signed trusted plugin should load");
        assert!(reg_entry.signed);
        assert_eq!(
            reg_entry.trusted_key_fingerprint,
            Some(key.verifying_key().to_bytes())
        );
        assert!(
            audit
                .events
                .iter()
                .any(|e| e.contains("granted:observer:signed=true"))
        );
    }

    #[test]
    fn signed_plugin_with_untrusted_key_rejected() {
        let trusted = SigningKey::from_bytes(&[1u8; 32]);
        let attacker = SigningKey::from_bytes(&[2u8; 32]);
        let manifest = ObservePlugin { signature: None }.manifest();
        let sig = sign_manifest(&manifest, &attacker);
        let mut plugin = ObservePlugin {
            signature: Some(sig),
        };

        let mut policy = dev_policy();
        policy.trusted_plugin_keys = vec![trusted.verifying_key().to_bytes()];

        let mut reg = PluginRegistry::new();
        let mut audit = NullAuditSink;
        let err = reg
            .register(&mut plugin, &policy, "rt".to_owned(), &mut audit)
            .expect_err("untrusted signer must be rejected");
        assert_eq!(err, PluginError::SignatureInvalid);
    }

    #[test]
    fn tampered_signature_rejected() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let manifest = ObservePlugin { signature: None }.manifest();
        let mut sig = sign_manifest(&manifest, &key);
        sig.signature[0] ^= 0xFF; // corrupt

        let mut plugin = ObservePlugin {
            signature: Some(sig),
        };
        let mut policy = dev_policy();
        policy.trusted_plugin_keys = vec![key.verifying_key().to_bytes()];

        let mut reg = PluginRegistry::new();
        let mut audit = NullAuditSink;
        let err = reg
            .register(&mut plugin, &policy, "rt".to_owned(), &mut audit)
            .expect_err("tampered sig must be rejected");
        assert_eq!(err, PluginError::SignatureInvalid);
    }

    #[test]
    fn dev_mode_unsigned_load_warns_in_audit() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .expect("dev-mode unsigned is accepted");
        assert!(audit.events.iter().any(|e| e.contains("dev_unsigned=true")));
    }

    // ---- isolation boundary -----------------------------------------------

    #[test]
    fn plugin_context_contains_no_secret_types() {
        // Compile-time proof: PluginContext must not expose any field whose
        // type name ends in "Secret*". We enforce this by pattern-matching
        // every field and ensuring the exhaustive set is known & harmless.
        let ctx = PluginContext {
            runtime_summary: "rt".to_owned(),
            granted_capabilities: BTreeSet::new(),
            dev_mode: true,
        };
        let PluginContext {
            runtime_summary: _,
            granted_capabilities: _,
            dev_mode: _,
        } = ctx;
        // If a future refactor adds a SecretString field here, this
        // destructure will fail to compile, which is the intended guard.
    }

    #[test]
    fn observe_publink_list_requires_observe_status() {
        assert_eq!(
            PluginCapability::required_for(&PluginOperation::ObservePublinkList),
            PluginCapability::ObserveStatus
        );
    }

    #[test]
    fn timer_tick_requires_observe_status() {
        assert_eq!(
            PluginCapability::required_for(&PluginOperation::TimerTick { period_secs: 60 }),
            PluginCapability::ObserveStatus
        );
    }

    #[test]
    fn authorize_allows_observe_publink_list_with_observe_status() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = NullAuditSink;
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();
        reg.authorize("observer", &PluginOperation::ObservePublinkList, &mut audit)
            .expect("observe publink ok");
        reg.authorize(
            "observer",
            &PluginOperation::TimerTick { period_secs: 60 },
            &mut audit,
        )
        .expect("timer tick ok");
    }

    #[test]
    fn unknown_plugin_authorize_errors() {
        let reg = PluginRegistry::new();
        let mut audit = NullAuditSink;
        let err = reg
            .authorize("ghost", &PluginOperation::ObserveHealth, &mut audit)
            .expect_err("must error");
        assert!(matches!(err, PluginError::UnknownPlugin(_)));
    }

    // ---- dispatch gate (capability) ---------------------------------------

    /// A DLP-shaped plugin whose manifest requests an *empty* capability
    /// set — i.e. `ObserveStatus` has been revoked by the operator. It
    /// must not be allowed to perform a pre-upload scan through the
    /// registry's `dispatch` gate even though it would otherwise be
    /// happy to run.
    struct DlpShapedPlugin;
    impl Plugin for DlpShapedPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "dlp.scanner".to_owned(),
                version: "1.0.0".to_owned(),
                display_name: "DLP Scanner".to_owned(),
                // Intentionally empty — capability was withheld.
                requested_capabilities: BTreeSet::new(),
            }
        }
        fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
            Ok(())
        }
    }

    #[test]
    fn dispatch_blocks_dlp_scan_without_observe_status() {
        let mut reg = PluginRegistry::new();
        let mut plugin = DlpShapedPlugin;
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .expect("empty-capability manifest still loads");

        let op = PluginOperation::PreUploadScan {
            path: "/tmp/a".to_owned(),
            size: 4,
            content_hash: "00".to_owned(),
            first_bytes: vec![0u8; 4],
            mime_guess: None,
        };

        let err = reg
            .dispatch("dlp.scanner", &op, &mut audit, |_op| {
                panic!("handler must not run when capability is missing");
            })
            .expect_err("scan must be denied");
        match err {
            PluginError::CapabilityNotGranted {
                plugin_id,
                operation,
                capability,
            } => {
                assert_eq!(plugin_id, "dlp.scanner");
                assert_eq!(operation, "pre_upload_scan");
                assert_eq!(capability, PluginCapability::ObserveStatus);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        // Structured deny event with missing-capability info.
        assert!(
            audit
                .events
                .iter()
                .any(|e| e == "deny:dlp.scanner:pre_upload_scan:capability_not_granted")
        );
    }

    /// An autoheal-shaped plugin that only requested `ObserveStatus`:
    /// it can observe integrity events, but MUST NOT be able to issue
    /// a quarantine request (which requires `SyncControl`).
    struct AutohealShapedPlugin;
    impl Plugin for AutohealShapedPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "autoheal".to_owned(),
                version: "1.0.0".to_owned(),
                display_name: "Autoheal".to_owned(),
                requested_capabilities: BTreeSet::from([PluginCapability::ObserveStatus]),
            }
        }
        fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
            Ok(())
        }
    }

    #[test]
    fn dispatch_blocks_autoheal_quarantine_without_sync_control() {
        let mut reg = PluginRegistry::new();
        let mut plugin = AutohealShapedPlugin;
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();

        let q = PluginOperation::RequestQuarantine {
            sync_root_id: 7,
            path: "secret.key".to_owned(),
        };
        let err = reg
            .dispatch("autoheal", &q, &mut audit, |_op| {
                panic!("handler must not run for denied op");
            })
            .expect_err("quarantine without SyncControl must be denied");
        match err {
            PluginError::CapabilityNotGranted { capability, .. } => {
                assert_eq!(capability, PluginCapability::SyncControl);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(
            audit
                .events
                .iter()
                .any(|e| e == "deny:autoheal:request_quarantine:capability_not_granted")
        );
    }

    #[test]
    fn dispatch_runs_handler_when_capability_granted() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();

        let out = reg
            .dispatch(
                "observer",
                &PluginOperation::ObserveRuntimeSummary,
                &mut audit,
                |_op| "ok".to_owned(),
            )
            .expect("dispatch should succeed");
        assert_eq!(out, "ok");
        assert!(
            audit
                .events
                .iter()
                .any(|e| e == "allow:observer:observe_runtime_summary")
        );
    }

    #[test]
    fn dispatch_unknown_plugin_short_circuits() {
        let mut reg = PluginRegistry::new();
        let mut audit = CapturingAudit::default();
        let err = reg
            .dispatch(
                "ghost",
                &PluginOperation::ObserveHealth,
                &mut audit,
                |_op| "unreachable",
            )
            .expect_err("must error for unknown plugin");
        assert!(matches!(err, PluginError::UnknownPlugin(_)));
    }

    // ---- panic guard ------------------------------------------------------

    #[test]
    fn dispatch_catches_handler_panic_and_deregisters() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();
        assert_eq!(reg.loaded_plugins().len(), 1);

        let err = reg
            .dispatch(
                "observer",
                &PluginOperation::ObserveRuntimeSummary,
                &mut audit,
                |_op| -> () { panic!("boom: plugin misbehaved") },
            )
            .expect_err("handler panic must surface as PluginError");
        match err {
            PluginError::HandlerPanic {
                plugin_id,
                operation,
            } => {
                assert_eq!(plugin_id, "observer");
                assert_eq!(operation, "observe_runtime_summary");
            }
            other => panic!("unexpected error: {other:?}"),
        }

        // Offending plugin must be gone.
        assert!(reg.loaded_plugins().is_empty());
        assert!(
            audit
                .events
                .iter()
                .any(|e| e == "panic:observer:observe_runtime_summary")
        );
        assert!(
            audit
                .events
                .iter()
                .any(|e| e == "deregistered:observer:handler_panic")
        );

        // Subsequent dispatch returns UnknownPlugin (boundary case).
        let err2 = reg
            .dispatch(
                "observer",
                &PluginOperation::ObserveRuntimeSummary,
                &mut audit,
                |_op| -> () {},
            )
            .expect_err("deregistered plugin must be unreachable");
        assert!(matches!(err2, PluginError::UnknownPlugin(_)));
    }

    #[test]
    fn explicit_deregister_removes_plugin_and_audits() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = CapturingAudit::default();
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();

        let removed = reg.deregister("observer", "operator_request", &mut audit);
        assert!(removed);
        assert!(reg.loaded_plugins().is_empty());
        assert!(
            audit
                .events
                .iter()
                .any(|e| e == "deregistered:observer:operator_request")
        );

        // Idempotent: second call returns false, no extra audit event.
        let before = audit.events.len();
        let removed2 = reg.deregister("observer", "operator_request", &mut audit);
        assert!(!removed2);
        assert_eq!(audit.events.len(), before);
    }

    // ---- boundary cases ---------------------------------------------------

    #[test]
    fn dispatch_denies_network_probe_without_network_egress() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = NullAuditSink;
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();

        let probe = PluginOperation::RequestNetworkProbe {
            host: "example.com".to_owned(),
        };
        let err = reg
            .dispatch("observer", &probe, &mut audit, |_op| ())
            .expect_err("network probe must be denied");
        match err {
            PluginError::CapabilityNotGranted { capability, .. } => {
                assert_eq!(capability, PluginCapability::NetworkEgress);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn dispatch_denies_crypto_query_without_crypto_capability() {
        let mut reg = PluginRegistry::new();
        let mut plugin = ObservePlugin { signature: None };
        let mut audit = NullAuditSink;
        reg.register(&mut plugin, &dev_policy(), "rt".to_owned(), &mut audit)
            .unwrap();
        let err = reg
            .dispatch(
                "observer",
                &PluginOperation::QueryCryptoLockState,
                &mut audit,
                |_op| (),
            )
            .expect_err("crypto query must be denied");
        match err {
            PluginError::CapabilityNotGranted { capability, .. } => {
                assert_eq!(capability, PluginCapability::CryptoControl);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}

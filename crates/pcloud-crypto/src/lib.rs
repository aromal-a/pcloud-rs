#![forbid(unsafe_code)]
//! # pcloud-crypto
//!
//! Active crypto subsystem for the Rust pcloud-rs path.
//!
//! ## Security posture
//!
//! - Master key material is wrapped in [`pcloud_secret::secret_bytes::SecretBytes`]
//!   and is zeroized on drop.
//! - No password, no derived key, and no content key is ever persisted on
//!   disk by this crate. Only a non-secret setup fingerprint is stored so
//!   that the wrong start-password can be rejected without ever touching
//!   ciphertext.
//! - All encrypted-content / encrypted-name operations are gated by the
//!   active [`state::UnlockState`]. When locked, the crate returns
//!   [`CryptoError::Locked`] rather than returning plaintext.
//! - The policy surface (see [`policy::CryptoPolicy`]) rejects any
//!   configuration that would persist master key material.
//!
//! ## Parity with the C client
//!
//! The functions below correspond to the retained C API in
//! `pclsync/pcryptofolder.c` / `pclsync/psynclib.h`:
//!
//! | C symbol                                | Rust equivalent                                  |
//! |-----------------------------------------|--------------------------------------------------|
//! | `psync_crypto_setup`                    | [`CryptoShell::setup`]                           |
//! | `psync_crypto_start`                    | [`CryptoShell::start`]                           |
//! | `psync_crypto_stop`                     | [`CryptoShell::stop`]                            |
//! | `psync_crypto_isstarted`                | [`CryptoShell::is_started`]                      |
//! | `psync_crypto_issetup`                  | [`CryptoShell::is_setup`]                        |
//! | `psync_crypto_reset`                    | [`CryptoShell::reset`]                           |
//! | `psync_crypto_mkdir`                    | [`CryptoShell::mkdir`]                           |
//! | `pcryptofolder_fileencoder_get` (sector)| [`content::seal_sector`] / [`content::open_sector`] |
//!
//! Retained-but-not-yet-mirrored C behaviour (change password / remote
//! encoded-key exchange / team crypto) is tracked in the parity matrix and
//! intentionally omitted from the active Rust path until the transport and
//! account surfaces are ready.

#![deny(missing_docs)]
// The crate-level pedantic blanket was removed by audit-04 P3/LOW. Narrowly-scoped
// allows are added at the specific call sites where clippy::pedantic fires to keep
// the suppression surface minimal and auditable.
#![allow(clippy::module_name_repetitions)] // `CryptoShell`, `CryptoError`, `CryptoMode` etc. repeat the crate name by design.
#![allow(clippy::doc_markdown)] // ADR refs / doc links don't need backtick formatting in prose.

// **PLATFORM:** all
// **GATING:** none (portable).

/// Sector-oriented content encryption (AES-256-GCM).
///
/// See module docs for the wire layout and AAD binding details. Content keys
/// are kept in [`pcloud_secret::secret_bytes::SecretBytes`] (zeroize on drop).
pub mod content;

/// Key derivation and wrapping (Argon2id master key, HMAC-SHA256 fingerprint).
///
/// See [`keys::KeyManager`] for the in-memory key state. Per ADR-0007 the
/// master key and password are never persisted; only the non-secret
/// [`keys::SetupFingerprint`] is written to disk.
pub mod keys;

/// Deterministic encrypted-filename encoder (HMAC-SHA256 output, hex-encoded).
///
/// Determinism is required for server-side lookup by encoded name; see the
/// module docs for the fixed label and the rationale.
pub mod metadata;

/// Password-quality scorer and passphrase→API-password derivation.
///
/// Byte-equivalent port of the C scorer plus a strictly stricter
/// secret-handling contract (all intermediate buffers zeroized on return).
pub mod password_scorer;

/// Runtime safety policy (e.g. refusal to persist master key material).
///
/// Per ADR-0007 `persist_master_key` must stay `false`; the daemon rejects
/// any config that flips it.
pub mod policy;

/// Crypto-folder sharing via temporary-password key-rewrap.
///
/// Mirrors the shape of the C `PSYNC_CRYPTO_FLAG_TEMP_PASS` flow with
/// strictly stronger secret handling (AEAD + detached HMAC signature).
pub mod share_temppass;

/// Lifecycle state machine (`NotSetup` / `Locked` / `Unlocking` / `Unlocked`).
pub mod state;

/// Shared base64 encode/decode helpers (consolidates hand-rolled base64 from
/// `password_scorer` and `share_temppass`). See LOW-3.Q in the crypto audit.
pub(crate) mod crypto_util;

pub use password_scorer::{
    psync_derive_password_from_passphrase, psync_password_quality, psync_password_quality10000,
};
pub use share_temppass::{TemppassError, TemppassWire, accept_temppass_wire, derive_temppass_wire};

use std::collections::BTreeMap;
use std::fmt;

use pcloud_secret::ExposeSecret as _;
use pcloud_secret::secret_string::SecretString;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;

/// Crate identifier used in audit/telemetry records.
///
/// ```
/// assert_eq!(pcloud_crypto::CRATE_NAME, "pcloud-crypto");
/// ```
pub const CRATE_NAME: &str = "pcloud-crypto";

/// Profile-format epoch for all on-wire and on-disk crypto labels.
///
/// Each versioned label (`"pcloud-crypto/file-key/v1"`,
/// `"pcloud-crypto/filename/v1"`, `"pcloud-crypto/fingerprint/v1"`, etc.)
/// embeds this version as a decimal suffix. When any label's semantics change
/// in a non-backwards-compatible way:
///
/// 1. Increment this constant.
/// 2. Update every label string that carries the old version.
/// 3. Add a migration note to `docs/enterprise/crypto-compat.md` explaining
///    what changed and how to re-derive or migrate existing blobs.
/// 4. Gate old-label compatibility behind a `LEGACY_C_COMPAT` feature so
///    production builds only accept the current epoch.
///
/// **Current epoch:** `1`. Corresponds to all `v1` labels introduced in the
/// initial Rust rewrite. (audit-04 LOW §3-opus L-4)
pub const PROFILE_VERSION: u32 = 1;

/// Remote folder identifier. Keeps the ids local to this crate so that the
/// crypto runtime does not pull in higher-level model types.
pub type CryptoFolderId = u64;

/// Top-level error type for the crypto subsystem.
///
/// Error messages are intentionally opaque: they never carry any part of the
/// password, derived key, nonce, ciphertext, or plaintext. Callers are
/// expected to map these variants to user-facing strings themselves rather
/// than echo the `Display` representation directly.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    /// The supplied password was empty. Rejected before any key derivation
    /// so that the empty-string case cannot be exploited as an oracle.
    #[error("crypto password must not be empty")]
    EmptyPassword,
    /// [`CryptoShell::setup`] was called when a setup fingerprint was
    /// already on record. The first setup is authoritative; use
    /// [`CryptoShell::reset`] to erase the existing fingerprint first.
    #[error("crypto is already set up")]
    AlreadySetup,
    /// An operation that requires a previously-recorded setup fingerprint
    /// was invoked before [`CryptoShell::setup`] ever ran.
    #[error("crypto has not been set up")]
    NotSetup,
    /// An operation that requires the shell to be unlocked was invoked
    /// while [`state::UnlockState`] was not `Unlocked`. No plaintext key
    /// material is resident in this state.
    #[error("crypto is locked")]
    Locked,
    /// [`CryptoShell::start`] was called on an already-started shell.
    #[error("crypto is already started")]
    AlreadyStarted,
    /// Password did not match the stored setup fingerprint. The comparison
    /// is constant-time (`subtle::ConstantTimeEq`).
    #[error("wrong password")]
    WrongPassword,
    /// Filename rejected by the metadata encoder (empty or contains `/`).
    #[error("invalid folder name")]
    InvalidName,
    /// A folder id collision occurred during local bookkeeping.
    #[error("folder already exists")]
    FolderExists,
    /// The runtime [`policy::CryptoPolicy`] would allow master-key
    /// persistence. Per ADR-0007 (password never persisted) the active
    /// Rust path refuses this configuration.
    #[error("unsafe policy: master key persistence is forbidden")]
    UnsafePolicy,
    /// New password equals the current password (constant-time byte
    /// comparison). Rejected before touching the key-derivation pipeline.
    #[error("new password must differ from the current password")]
    PasswordUnchanged,
    /// Forwarded from the sector AEAD layer. See
    /// [`content::ContentCryptoError`] for variants.
    #[error("content crypto error: {0}")]
    Content(#[from] content::ContentCryptoError),
    /// Forwarded from the filename encoder. See
    /// [`metadata::MetadataCryptoError`] for variants.
    #[error("metadata crypto error: {0}")]
    Metadata(#[from] metadata::MetadataCryptoError),
    /// The KMS-wrapped DEK path was requested but the injected
    /// [`pcloud_kms::KmsProvider`] rejected the wrap / unwrap call.
    /// The inner `KmsError` carries the provider-specific reason;
    /// callers should surface the taxonomy variant (not its Display
    /// text, which is provider-specific) to the user.
    #[error("KMS provider error")]
    Kms,
    /// [`CryptoShell::enable_kms_mode`] was called while the shell was
    /// still configured with the default [`pcloud_kms::NullKms`]. The
    /// runtime must first inject a real provider via
    /// [`CryptoShell::set_kms_provider`].
    #[error("no real KMS provider is configured")]
    NoKmsProvider,
    /// KMS mode was requested but the unwrapped DEK was the wrong size
    /// for AES-256-GCM (must be [`KMS_DEK_LEN`] = 32 bytes). Indicates
    /// a provider bug, a tampered wrapped blob, or a mismatched CMK.
    #[error("KMS returned a DEK of the wrong length")]
    KmsDekLen,
    /// Too many consecutive failed unlock attempts. The shell refuses
    /// further unlock calls until [`CryptoShell::reset`] is called.
    /// Protects against automated brute-force of the crypto password.
    #[error("brute-force lockout: too many consecutive failed unlock attempts")]
    BruteForceLockedOut,
    /// Per-session AES-256-GCM nonce budget exhausted. With 96-bit random
    /// nonces, the safe encryption budget for a single key is ~2^32
    /// operations. The shell refuses further [`CryptoShell::seal_sector`]
    /// calls when the counter approaches `u32::MAX` minus a safety margin
    /// so the daemon rotates the per-file / master key before nonce
    /// collision becomes non-negligible.
    #[error("nonce budget exhausted: key rotation required before further sector seals")]
    NonceBudgetExhausted,
}

impl From<pcloud_kms::KmsError> for CryptoError {
    fn from(_: pcloud_kms::KmsError) -> Self {
        CryptoError::Kms
    }
}

/// Output of a password-rotation operation. Both fields are hex-encoded
/// opaque blobs that are safe to transmit over the `crypto_changeuserprivate`
/// wire call; neither carries the old or new password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReencodedPrivateKey {
    /// New opaque "private key" blob (hex-encoded). In the Rust active path
    /// this encodes the new setup fingerprint plus a version tag plus the
    /// new derivation salt; in the C path it was the re-encrypted PKCS
    /// private key. In both cases the server treats it as an opaque string.
    pub private_key_hex: String,
    /// Hex-encoded HMAC signature over `private_key_hex` keyed with the
    /// still-active master key, so the server can verify the upload came
    /// from a session that currently has access to the old key.
    pub signature_hex: String,
}

/// Bookkeeping entry for an encrypted folder.
///
/// Carries only non-secret data: a folder id, an optional parent id, and the
/// deterministic encrypted name (the HMAC-SHA256 hex output from
/// [`metadata::encrypt_filename`]). Safe to log and persist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CryptoFolderEntry {
    /// Server-assigned (or locally-allocated) encrypted folder id.
    pub folder_id: CryptoFolderId,
    /// Parent encrypted folder id, or `None` for a root-level entry.
    pub parent_folder_id: Option<CryptoFolderId>,
    /// Deterministic encrypted filename (hex-encoded HMAC-SHA256 tag).
    pub encrypted_name: String,
}

/// Construct the default `Box<dyn KmsProvider>` (always [`pcloud_kms::NullKms`]).
///
/// Kept as a free function rather than a method so serde can use it as a
/// `#[serde(default = ...)]` hook for the skipped `kms` field on
/// [`CryptoShell`]. The runtime provider is injected later via
/// [`CryptoShell::with_kms_provider`].
#[must_use]
pub fn default_kms_provider() -> Box<dyn pcloud_kms::KmsProvider> {
    Box::new(pcloud_kms::NullKms)
}

/// Length of a KMS-wrapped DEK's plaintext material (32 bytes for AES-256).
pub const KMS_DEK_LEN: usize = 32;

/// Safety margin subtracted from `u32::MAX` when enforcing the per-session
/// AES-256-GCM nonce budget in [`CryptoShell::seal_sector`]. Once the
/// `sectors_sealed` counter exceeds `u32::MAX - NONCE_BUDGET_SAFETY_MARGIN`
/// the shell returns [`CryptoError::NonceBudgetExhausted`] rather than
/// issuing another sector nonce. See H-2 in the crypto audit plan.
pub const NONCE_BUDGET_SAFETY_MARGIN: u64 = 64;

/// Maximum consecutive failed unlock attempts before the shell refuses
/// further [`CryptoShell::start`] calls (brute-force lockout). Persisted
/// across daemon restarts via serde so an attacker cannot reset the
/// counter by killing the process.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 10;

/// Upper bound on the exponential-backoff wait applied after consecutive
/// failed unlock attempts (30 minutes). Backoff doubles on each failure
/// up to this cap; the shell returns [`CryptoError::BruteForceLockedOut`]
/// if [`CryptoShell::start`] is called within the backoff window.
pub const MAX_LOCKOUT_BACKOFF_SECS: u64 = 30 * 60;

/// Active DEK-sourcing mode for the sector-encryption path.
///
/// - [`CryptoMode::Raw`] — the legacy path: per-file keys are derived
///   directly from the Argon2id master key (see
///   [`content::derive_file_key`]). This remains the default for
///   single-user deployments where no external KMS is configured.
/// - [`CryptoMode::Kms`] — enterprise path: a random 32-byte DEK is
///   wrapped once by the injected [`pcloud_kms::KmsProvider`] at
///   [`CryptoShell::enable_kms_mode`] time; the wrapped blob is held
///   in memory by the shell; every sector `seal`/`open` unwraps the
///   DEK via the KMS (using the [`pcloud_kms::KmsProvider::unwrap_cached`]
///   TTL cache) and derives per-file keys from that DEK instead of
///   the master key. [`CryptoShell::stop`] evicts the cache entry so
///   the plaintext DEK does not outlive the session.
///
/// The wrapped DEK **is** serialised (it is ciphertext and only the
/// configured KMS can unwrap it), but the plaintext DEK is **not**.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CryptoMode {
    /// Legacy master-key-derived DEK path. Default for single-user
    /// deployments and for backwards compatibility.
    #[default]
    Raw,
    /// KMS-wrapped DEK path. The wrapped blob is stored in the shell;
    /// the plaintext DEK is only resident during an active sector op
    /// and is zeroized when [`CryptoShell::stop`] evicts the entry
    /// from the shared [`pcloud_kms`] cache.
    Kms {
        /// KMS key id (CMK ARN for AWS, transit key name for Vault,
        /// `CKA_LABEL` for PKCS#11).
        key_id: String,
        /// Opaque KMS-wrapped DEK. Provider-defined byte layout.
        /// Safe to persist — only the configured KMS can unwrap it.
        wrapped_dek: Vec<u8>,
        /// Optional AAD string (e.g. tenant id / folder id) passed to
        /// wrap/unwrap so a wrapped DEK cannot be replayed across
        /// contexts. `None` when not bound to a context.
        context: Option<String>,
    },
}

impl CryptoMode {
    /// Short safe-to-log tag (`"raw"` / `"kms"`).
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            CryptoMode::Raw => "raw",
            CryptoMode::Kms { .. } => "kms",
        }
    }

    /// `true` if this is the KMS-wrapped DEK mode.
    #[must_use]
    pub fn is_kms(&self) -> bool {
        matches!(self, CryptoMode::Kms { .. })
    }
}

/// Top-level crypto runtime.
///
/// Holds the active lifecycle state, key manager, metadata/content
/// configuration, and the local encrypted-folder registry. All key material
/// is wrapped in [`pcloud_secret::secret_bytes::SecretBytes`] and zeroized on
/// drop; per ADR-0007 no password, no derived master key, and no content key
/// is ever persisted by this crate.
///
/// ## KMS routing
///
/// The `kms` field holds a [`pcloud_kms::KmsProvider`] that is consulted
/// whenever a DEK needs to be wrapped / unwrapped against an enterprise
/// KMS. The default is [`pcloud_kms::NullKms`] — i.e. no KMS integration,
/// the legacy local-Argon2 path. Callers that have a real provider
/// configured (AWS / Vault / PKCS#11) must inject it via
/// [`Self::with_kms_provider`] or [`Self::set_kms_provider`] **before**
/// starting the shell so DEK operations go through the configured
/// provider. The provider is **not** serialised — deserialised
/// `CryptoShell`s come back with the default [`pcloud_kms::NullKms`] and
/// must be re-injected by the runtime.
#[derive(Serialize, Deserialize)]
pub struct CryptoShell {
    /// Key-derivation / fingerprint state. See [`keys::KeyManager`].
    pub keys: keys::KeyManager,
    /// Metadata-encryption configuration. See [`metadata::MetadataCrypto`].
    pub metadata: metadata::MetadataCrypto,
    /// Sector-encryption configuration. See [`content::ContentCrypto`].
    pub content: content::ContentCrypto,
    /// Runtime safety policy. See [`policy::CryptoPolicy`].
    pub policy: policy::CryptoPolicy,
    /// Active lifecycle state (`NotSetup` / `Locked` / `Unlocking` /
    /// `Unlocked`).
    pub unlock_state: state::UnlockState,
    /// Local registry of known encrypted folders, keyed by folder id.
    /// Populated by [`Self::mkdir`] and by refresh flows from the daemon.
    pub folders: BTreeMap<CryptoFolderId, CryptoFolderEntry>,
    /// Monotonic counter to hand out pseudo folder ids in offline/test flows
    /// where the backend hasn't yet assigned a real id.
    pub next_local_folder_id: u64,
    /// Optional user-provided password hint. Never the password itself;
    /// safe to surface in UI/logs.
    pub hint: Option<String>,
    /// Injected KMS provider. Never persisted — deserialisation always
    /// starts from [`pcloud_kms::NullKms`] until the runtime re-injects
    /// the configured provider.
    #[serde(skip, default = "default_kms_provider")]
    pub kms: Box<dyn pcloud_kms::KmsProvider>,
    /// Active DEK-sourcing mode for the sector-encryption path.
    ///
    /// Defaults to [`CryptoMode::Raw`]. Switched to [`CryptoMode::Kms`]
    /// via [`Self::enable_kms_mode`] once the runtime has verified that
    /// `[crypto.kms]` is configured and a real provider is injected.
    #[serde(default)]
    pub mode: CryptoMode,
    /// Monotonic count of sectors successfully sealed in this session.
    ///
    /// Used to detect nonce-space exhaustion: AES-256-GCM with a 96-bit
    /// random nonce is safe up to roughly 2^32 encryptions per key before
    /// collision probability becomes non-negligible. When this counter
    /// exceeds `u32::MAX` the daemon must rotate to a new per-file key or
    /// master key before sealing further sectors.
    ///
    /// Not serialised — resets to zero on each daemon restart (the count
    /// only needs to guard the in-process session).
    #[serde(skip)]
    pub sectors_sealed: std::sync::atomic::AtomicU64,
    /// Consecutive failed unlock attempts. Incremented on each wrong-password
    /// call to [`Self::start`]; reset to zero on a successful unlock.
    ///
    /// When this reaches [`MAX_CONSECUTIVE_FAILURES`] the shell returns
    /// [`CryptoError::BruteForceLockedOut`] for subsequent unlock attempts
    /// until the shell is reset.
    ///
    /// **Persisted across restarts** via the `atomic_u32_serde` shim so an
    /// attacker cannot reset the lockout counter by killing the daemon.
    /// The counter is zeroized on every successful unlock.
    #[serde(with = "atomic_u32_serde", default = "default_atomic_u32")]
    pub consecutive_failures: std::sync::atomic::AtomicU32,
    /// Wall-clock timestamp (seconds since UNIX epoch) of the most recent
    /// failed unlock attempt, or `0` if there has never been a failure or
    /// the counter has been zeroized.
    ///
    /// Used together with [`consecutive_failures`](Self::consecutive_failures)
    /// to enforce exponential backoff: each failure roughly doubles the
    /// required wait (base = 1s, `wait = 2^failures`) up to
    /// [`MAX_LOCKOUT_BACKOFF_SECS`] (30 minutes). Persisted across
    /// restarts so crash-then-retry loops cannot sidestep the backoff.
    #[serde(with = "atomic_u64_serde", default = "default_atomic_u64")]
    pub last_fail_at: std::sync::atomic::AtomicU64,
}

/// Default constructor used by serde for [`CryptoShell::consecutive_failures`]
/// when the field is absent from the serialized representation.
fn default_atomic_u32() -> std::sync::atomic::AtomicU32 {
    std::sync::atomic::AtomicU32::new(0)
}

/// Default constructor used by serde for [`CryptoShell::last_fail_at`] when
/// the field is absent from the serialized representation.
fn default_atomic_u64() -> std::sync::atomic::AtomicU64 {
    std::sync::atomic::AtomicU64::new(0)
}

/// Serde shim for [`std::sync::atomic::AtomicU32`] used by
/// [`CryptoShell::consecutive_failures`]. Snapshots the value with
/// `Relaxed` ordering on serialize and reconstructs a fresh atomic on
/// deserialize. Used so the brute-force lockout counter survives daemon
/// restart (H-5 in the crypto audit plan).
mod atomic_u32_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::atomic::{AtomicU32, Ordering};

    pub fn serialize<S: Serializer>(a: &AtomicU32, s: S) -> Result<S::Ok, S::Error> {
        a.load(Ordering::Relaxed).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<AtomicU32, D::Error> {
        Ok(AtomicU32::new(u32::deserialize(d)?))
    }
}

/// Serde shim for [`std::sync::atomic::AtomicU64`] used by
/// [`CryptoShell::last_fail_at`]. Same posture as
/// [`atomic_u32_serde`] — `Relaxed` snapshot on serialize, fresh atomic on
/// deserialize.
mod atomic_u64_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub fn serialize<S: Serializer>(a: &AtomicU64, s: S) -> Result<S::Ok, S::Error> {
        a.load(Ordering::Relaxed).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<AtomicU64, D::Error> {
        Ok(AtomicU64::new(u64::deserialize(d)?))
    }
}

impl fmt::Debug for CryptoShell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CryptoShell")
            .field("keys", &self.keys)
            .field("metadata", &self.metadata)
            .field("content", &self.content)
            .field("policy", &self.policy)
            .field("unlock_state", &self.unlock_state)
            .field("folders", &self.folders)
            .field("next_local_folder_id", &self.next_local_folder_id)
            .field("hint", &self.hint)
            .field("kms", &self.kms.name())
            .field("mode", &self.mode.tag())
            .finish()
    }
}

impl Default for CryptoShell {
    fn default() -> Self {
        Self {
            keys: keys::KeyManager::default(),
            metadata: metadata::MetadataCrypto::default(),
            content: content::ContentCrypto::default(),
            policy: policy::CryptoPolicy::default(),
            unlock_state: state::UnlockState::NotSetup,
            folders: BTreeMap::new(),
            next_local_folder_id: 1,
            hint: None,
            kms: default_kms_provider(),
            mode: CryptoMode::Raw,
            sectors_sealed: std::sync::atomic::AtomicU64::new(0),
            consecutive_failures: std::sync::atomic::AtomicU32::new(0),
            last_fail_at: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

/// Normalize a password to Unicode NFC before key derivation.
///
/// Ensures that visually-identical passwords typed on different platforms
/// (macOS default NFD vs. Linux/Windows default NFC) derive to the same
/// master key. Returns a new [`SecretString`]; the normalized form is
/// zeroized on drop just like any other [`SecretString`] (H-4 in the
/// crypto audit plan).
fn normalize_password_nfc(pw: &SecretString) -> SecretString {
    use unicode_normalization::UnicodeNormalization;
    let s: String = pw.expose_secret().nfc().collect();
    SecretString::new(s)
}

/// Seconds since the UNIX epoch, clamped to `u64::MAX` on the (unreachable
/// in practice) pre-1970 / clock-rewound case. Used by the brute-force
/// lockout to timestamp the most recent failed unlock attempt.
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Exponential backoff window (seconds) given `failures` prior consecutive
/// failed unlocks. `failures == 0 or 1` → 0s; otherwise `2^failures`
/// seconds, capped at [`MAX_LOCKOUT_BACKOFF_SECS`]. Deterministic and
/// side-effect-free; unit-tested.
fn lockout_backoff_secs(failures: u32) -> u64 {
    if failures <= 1 {
        return 0;
    }
    let shift = failures.min(40); // guard against shift overflow; 2^40 > 30min cap anyway
    let wait = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
    wait.min(MAX_LOCKOUT_BACKOFF_SECS)
}

impl CryptoShell {
    /// Short one-line state summary for logs / CLI. Contains no secret
    /// material.
    ///
    /// ```
    /// let c = pcloud_crypto::CryptoShell::default();
    /// assert!(c.summary().contains("state=NotSetup"));
    /// ```
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "crypto(state={:?}, setup={}, started={}, folders={}, metadata_names_encrypted={}, key_cache_ttl={}s, policy_safe={}, mode={}, kms={})",
            self.unlock_state,
            self.is_setup(),
            self.is_started(),
            self.folders.len(),
            self.metadata.encrypted_names_enabled,
            self.keys.cache_ttl_secs,
            self.policy.is_safe(),
            self.mode.tag(),
            self.kms.name(),
        )
    }

    /// Builder-style constructor that injects a concrete [`pcloud_kms::KmsProvider`].
    ///
    /// The default [`CryptoShell`] carries [`pcloud_kms::NullKms`], which
    /// refuses every real KMS operation ([`pcloud_kms::KmsError::NotImplemented`]).
    /// When the daemon is configured with `[crypto.kms]`, it constructs
    /// the matching provider (AWS / Vault / PKCS#11) and injects it here
    /// **before** calling [`Self::start`] so the DEK path flows through
    /// the configured provider.
    ///
    /// ```
    /// use pcloud_crypto::CryptoShell;
    /// use pcloud_kms::NullKms;
    /// let shell = CryptoShell::default().with_kms_provider(Box::new(NullKms));
    /// assert_eq!(shell.kms_provider_name(), "null");
    /// ```
    #[must_use]
    pub fn with_kms_provider(mut self, provider: Box<dyn pcloud_kms::KmsProvider>) -> Self {
        self.kms = provider;
        self
    }

    /// In-place injection of a concrete [`pcloud_kms::KmsProvider`].
    ///
    /// Used on a deserialised [`CryptoShell`] where serde has already
    /// materialised the default [`pcloud_kms::NullKms`] and the runtime
    /// now wants to route DEK operations through the real provider.
    pub fn set_kms_provider(&mut self, provider: Box<dyn pcloud_kms::KmsProvider>) {
        self.kms = provider;
    }

    /// Short name of the active KMS provider (safe to log).
    ///
    /// ```
    /// let c = pcloud_crypto::CryptoShell::default();
    /// assert_eq!(c.kms_provider_name(), "null");
    /// ```
    #[must_use]
    pub fn kms_provider_name(&self) -> &'static str {
        self.kms.name()
    }

    /// Wrap a plaintext DEK through the injected [`pcloud_kms::KmsProvider`].
    ///
    /// This is the routing call that replaces the legacy "derive DEK
    /// locally from the master key" path for deployments with a real
    /// KMS. Contextual binding (`context`) is an optional AAD string —
    /// the folder id, device id, or tenant id — so that a wrapped DEK
    /// cannot be replayed across contexts.
    ///
    /// # Errors
    /// Returns the [`pcloud_kms::KmsError`] verbatim (`Unreachable`,
    /// `AuthFailed`, `PolicyDenied`, `KeyNotFound`, `Malformed`,
    /// `NotImplemented`, or `Other`). Callers are expected to surface
    /// these as-is rather than echo the `Display` text, which is
    /// provider-specific.
    pub fn kms_wrap_dek(
        &self,
        key_id: &pcloud_kms::KeyId,
        dek: &pcloud_kms::PlaintextDek,
        context: Option<&str>,
    ) -> Result<pcloud_kms::WrappedDek, pcloud_kms::KmsError> {
        self.kms.encrypt_dek(key_id, dek, context)
    }

    /// Unwrap a wrapped DEK through the injected [`pcloud_kms::KmsProvider`].
    ///
    /// Uses [`pcloud_kms::KmsProvider::unwrap_cached`] so repeated
    /// unwraps within the default TTL
    /// ([`pcloud_kms::DEFAULT_CACHE_TTL`]) hit the process-local cache
    /// instead of round-tripping to the KMS on every sector open.
    ///
    /// # Errors
    /// Same as [`Self::kms_wrap_dek`].
    pub fn kms_unwrap_dek(
        &self,
        key_id: &pcloud_kms::KeyId,
        wrapped: &pcloud_kms::WrappedDek,
        context: Option<&str>,
    ) -> Result<pcloud_kms::PlaintextDek, pcloud_kms::KmsError> {
        self.kms
            .unwrap_cached(key_id, wrapped, context, pcloud_kms::DEFAULT_CACHE_TTL)
    }

    /// Switch this shell into [`CryptoMode::Kms`].
    ///
    /// Generates a fresh 32-byte DEK from the OS CSPRNG, wraps it
    /// through the injected [`pcloud_kms::KmsProvider`], and stores the
    /// wrapped blob on the shell. Subsequent [`Self::seal_sector`] /
    /// [`Self::open_sector`] calls unwrap the DEK via the KMS (using
    /// [`pcloud_kms::KmsProvider::unwrap_cached`] so the TTL cache
    /// amortises the round-trip) and derive per-file keys from the DEK
    /// instead of the Argon2id master key.
    ///
    /// The plaintext DEK never outlives this call — it is consumed by
    /// `encrypt_dek` and dropped (zeroized). Only the wrapped blob is
    /// kept on the shell.
    ///
    /// # Security
    /// Mitigates: plaintext-DEK residency across sessions (the cache
    /// is evicted on [`Self::stop`]); provider-substitution attacks
    /// (the wrapped blob is bound to `key_id` + optional `context`
    /// AAD so a blob from one key cannot be unwrapped under another);
    /// silent fallback to `NullKms` (this method refuses to run under
    /// `NullKms` and returns [`CryptoError::NoKmsProvider`]).
    ///
    /// # Errors
    /// - [`CryptoError::NoKmsProvider`] when the shell still carries
    ///   [`pcloud_kms::NullKms`].
    /// - [`CryptoError::Kms`] if the provider wrap call fails.
    /// - [`CryptoError::UnsafePolicy`] if the policy is not safe.
    ///
    /// # Panics
    /// Does not panic in normal operation. `getrandom` failure is
    /// treated as an unrecoverable host fault.
    pub fn enable_kms_mode(
        &mut self,
        key_id: impl Into<String>,
        context: Option<String>,
    ) -> Result<(), CryptoError> {
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if self.kms.name() == "null" {
            return Err(CryptoError::NoKmsProvider);
        }
        let key_id_s: String = key_id.into();
        // Generate a fresh DEK from the OS CSPRNG.
        let mut dek_bytes = vec![0u8; KMS_DEK_LEN];
        // INVARIANT: see keys::KeyManager::default — getrandom is always
        // available on supported targets (Linux/macOS/Windows).
        getrandom::getrandom(&mut dek_bytes)
            .expect("OS randomness should be available for DEK generation");
        let dek = pcloud_kms::PlaintextDek(dek_bytes);
        let kid = pcloud_kms::KeyId(key_id_s.clone());
        let wrapped = self.kms.encrypt_dek(&kid, &dek, context.as_deref())?;
        // `dek` zeroizes on drop here.
        drop(dek);
        self.mode = CryptoMode::Kms {
            key_id: key_id_s,
            wrapped_dek: wrapped.0,
            context,
        };
        Ok(())
    }

    /// Unwrap the current KMS-wrapped DEK through the injected provider.
    ///
    /// Used internally by [`Self::seal_sector`] / [`Self::open_sector`]
    /// when [`Self::mode`] is [`CryptoMode::Kms`]. Hits the
    /// [`pcloud_kms`] TTL cache on repeat calls.
    fn unwrap_active_dek(&self) -> Result<pcloud_kms::PlaintextDek, CryptoError> {
        match &self.mode {
            CryptoMode::Raw => Err(CryptoError::Kms), // programmer error
            CryptoMode::Kms {
                key_id,
                wrapped_dek,
                context,
            } => {
                // `KeyId` and `WrappedDek` are newtype wrappers the KMS trait
                // takes by shared reference. One clone of `key_id` (short
                // string) and one clone of `wrapped_dek` (Vec<u8>) are
                // structurally required: `CryptoMode::Kms` persists
                // `wrapped_dek: Vec<u8>` for serde compatibility; changing
                // to `WrappedDek` would require an on-disk schema migration.
                // (audit-04 P3/MEDIUM: documented — cannot eliminate without
                // schema change.)
                let kid = pcloud_kms::KeyId(key_id.clone());
                let blob = pcloud_kms::WrappedDek(wrapped_dek.clone());
                let pt = self.kms.unwrap_cached(
                    &kid,
                    &blob,
                    context.as_deref(),
                    pcloud_kms::DEFAULT_CACHE_TTL,
                )?;
                if pt.expose().len() != KMS_DEK_LEN {
                    return Err(CryptoError::KmsDekLen);
                }
                Ok(pt)
            }
        }
    }

    /// `psync_crypto_issetup` equivalent.
    ///
    /// ```
    /// let c = pcloud_crypto::CryptoShell::default();
    /// assert!(!c.is_setup());
    /// ```
    #[must_use]
    pub fn is_setup(&self) -> bool {
        self.keys.setup_fingerprint.is_some()
    }

    /// `psync_crypto_isstarted` equivalent.
    ///
    /// ```
    /// let c = pcloud_crypto::CryptoShell::default();
    /// assert!(!c.is_started());
    /// ```
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.unlock_state.is_started() && self.keys.active_key_material.is_some()
    }

    /// Returns the password hint, if any. The hint is never the password
    /// itself and is safe to surface in UI.
    ///
    /// ```
    /// let c = pcloud_crypto::CryptoShell::default();
    /// assert_eq!(c.get_hint(), None);
    /// ```
    #[must_use]
    pub fn get_hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// One-time crypto setup. Derives the master key with
    /// Argon2id (default parameters, 16-byte salt, 32-byte output, see
    /// [`keys::KeyManager::derive_key_material`]), records its
    /// HMAC-SHA256 fingerprint, and immediately drops the plaintext key
    /// material from memory. After a successful [`Self::setup`] the shell
    /// remains [`state::UnlockState::Locked`] and the caller must `start`
    /// to actually use it.
    ///
    /// Per ADR-0007 the password itself is never persisted. Only the
    /// non-secret [`keys::SetupFingerprint`] is written to disk via the
    /// profile store.
    ///
    /// # Security
    /// Mitigates: dictionary attacks on the on-disk profile (fingerprint
    /// reveals no key bits), accidental persistence of key material
    /// (`drop(derived)` runs after recording the fingerprint), and
    /// misconfiguration where a policy would persist the master key
    /// (rejected before derivation).
    ///
    /// Out of scope: kernel swap snapshotting of the in-flight Argon2
    /// buffer (the crate assumes a standard userland process); coercion
    /// of the user into typing a low-entropy password (the scorer is
    /// advisory only).
    ///
    /// # Errors
    /// - [`CryptoError::EmptyPassword`] if the password is empty.
    /// - [`CryptoError::AlreadySetup`] if a setup fingerprint is already on record.
    /// - [`CryptoError::UnsafePolicy`] if the current policy would persist the key.
    ///
    /// # Panics
    /// Does not panic. Internal `getrandom` and Argon2 calls `expect()` on
    /// OS-randomness failure, which is treated as an unrecoverable host
    /// fault rather than a recoverable error.
    ///
    /// ```
    /// use pcloud_crypto::CryptoShell;
    /// use pcloud_secret::secret_string::SecretString;
    /// let mut c = CryptoShell::default();
    /// c.setup(SecretString::new("hunter2"), None).unwrap();
    /// assert!(c.is_setup());
    /// assert!(!c.is_started()); // must call start() to activate
    /// ```
    pub fn setup(
        &mut self,
        password: SecretString,
        hint: Option<String>,
    ) -> Result<(), CryptoError> {
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if password.is_empty() {
            return Err(CryptoError::EmptyPassword);
        }
        if self.is_setup() {
            return Err(CryptoError::AlreadySetup);
        }
        // Normalize to NFC so the same visual password typed on NFD (macOS)
        // vs NFC (Linux/Windows) platforms produces the same fingerprint
        // (H-4 in the crypto audit plan).
        let normalized = normalize_password_nfc(&password);
        let derived = self.keys.derive_key_material(&normalized);
        self.keys.setup_fingerprint = Some(keys::KeyManager::fingerprint_for(&derived));
        // Intentionally do NOT retain the key material from setup; the user
        // must explicitly `start` to activate a session.
        drop(derived);
        self.hint = hint;
        self.unlock_state = state::UnlockState::Locked;
        Ok(())
    }

    /// `psync_crypto_start` equivalent. Verifies the password against the
    /// stored fingerprint in constant time (`subtle::ConstantTimeEq`) and,
    /// on success, keeps the derived 32-byte master key resident
    /// in a [`pcloud_secret::secret_bytes::SecretBytes`] (zeroize on
    /// drop) for subsequent sector/metadata operations.
    ///
    /// # Security
    /// Mitigates: timing side-channels on the fingerprint check
    /// (constant-time comparison), wrong-password unlock oracles (the
    /// shell stays `Locked` and key material is dropped without being
    /// stored on failure), and late state corruption (`Unlocking` is set
    /// before derivation so crash handlers can see the transition).
    ///
    /// Out of scope: power-analysis and cache-timing side channels on
    /// Argon2 itself — the Rust crate delegates to
    /// [`argon2::Argon2::hash_password_into`] and inherits its posture.
    ///
    /// # Errors
    /// - [`CryptoError::EmptyPassword`], [`CryptoError::NotSetup`],
    ///   [`CryptoError::AlreadyStarted`], [`CryptoError::WrongPassword`],
    ///   [`CryptoError::UnsafePolicy`].
    ///
    /// ```
    /// use pcloud_crypto::CryptoShell;
    /// use pcloud_secret::secret_string::SecretString;
    /// let mut c = CryptoShell::default();
    /// c.setup(SecretString::new("hunter2"), None).unwrap();
    /// c.start(SecretString::new("hunter2")).unwrap();
    /// assert!(c.is_started());
    /// ```
    pub fn start(&mut self, password: SecretString) -> Result<(), CryptoError> {
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if password.is_empty() {
            return Err(CryptoError::EmptyPassword);
        }
        if !self.is_setup() {
            return Err(CryptoError::NotSetup);
        }
        if self.is_started() {
            return Err(CryptoError::AlreadyStarted);
        }
        // Brute-force guard: hard cap on total consecutive failures plus
        // an exponential backoff window enforced against the persisted
        // `last_fail_at` timestamp. Both counters survive daemon restart
        // via serde so an attacker cannot reset the lockout by killing
        // the process (H-5 in the crypto audit plan).
        let failures = self
            .consecutive_failures
            .load(std::sync::atomic::Ordering::Relaxed);
        if failures >= MAX_CONSECUTIVE_FAILURES {
            return Err(CryptoError::BruteForceLockedOut);
        }
        let backoff = lockout_backoff_secs(failures);
        if backoff > 0 {
            let last = self.last_fail_at.load(std::sync::atomic::Ordering::Relaxed);
            let now = unix_now_secs();
            if last > 0 && now.saturating_sub(last) < backoff {
                return Err(CryptoError::BruteForceLockedOut);
            }
        }

        // Normalize password bytes to Unicode NFC (H-4) so the same
        // human-visible password entered on macOS (NFD) vs Linux (NFC)
        // derives to the same master key.
        let normalized = normalize_password_nfc(&password);

        self.unlock_state = state::UnlockState::Unlocking;
        let derived = self.keys.derive_key_material(&normalized);
        if !self.keys.matches_setup(&derived) {
            // Wipe the derived material before returning.
            drop(derived);
            self.unlock_state = state::UnlockState::Locked;
            self.consecutive_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.last_fail_at
                .store(unix_now_secs(), std::sync::atomic::Ordering::Relaxed);
            return Err(CryptoError::WrongPassword);
        }
        self.keys.active_key_material = Some(derived);
        self.unlock_state = state::UnlockState::Unlocked;
        self.consecutive_failures
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.last_fail_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// `psync_crypto_stop` equivalent. Drops (and zeroizes) the active key
    /// material and returns to the locked state. Idempotent.
    ///
    /// ```
    /// use pcloud_crypto::CryptoShell;
    /// use pcloud_secret::secret_string::SecretString;
    /// let mut c = CryptoShell::default();
    /// c.setup(SecretString::new("pw"), None).unwrap();
    /// c.start(SecretString::new("pw")).unwrap();
    /// c.stop();
    /// assert!(!c.is_started());
    /// assert!(c.is_setup());
    /// ```
    pub fn stop(&mut self) {
        self.keys.active_key_material = None;
        // If a KMS-wrapped DEK is resident in the process-local cache,
        // evict it now so the plaintext DEK does not outlive the
        // session. `PlaintextDek` zeroizes on drop.
        if let CryptoMode::Kms {
            key_id,
            wrapped_dek,
            context,
        } = &self.mode
        {
            let kid = pcloud_kms::KeyId(key_id.clone());
            let blob = pcloud_kms::WrappedDek(wrapped_dek.clone());
            let _ = pcloud_kms::evict_cached_dek(self.kms.name(), &kid, &blob, context.as_deref());
        }
        self.unlock_state = if self.is_setup() {
            state::UnlockState::Locked
        } else {
            state::UnlockState::NotSetup
        };
    }

    /// Back-compat shim — pre-existing callers may still use `unlock`.
    /// Treated as a `setup + start` combo only when not yet set up; on an
    /// already-set-up shell it behaves as `start`.
    ///
    /// ```
    /// use pcloud_crypto::CryptoShell;
    /// use pcloud_secret::secret_string::SecretString;
    /// let mut c = CryptoShell::default();
    /// c.unlock(SecretString::new("pw")).unwrap();
    /// assert!(c.is_started());
    /// ```
    pub fn unlock(&mut self, password: SecretString) -> Result<(), CryptoError> {
        if self.is_setup() {
            self.start(password)
        } else {
            let dup = password.clone_secret();
            self.setup(password, None)?;
            self.start(dup)
        }
    }

    /// Back-compat shim equivalent to [`Self::stop`].
    ///
    /// ```
    /// let mut c = pcloud_crypto::CryptoShell::default();
    /// c.lock(); // no-op, idempotent
    /// assert!(!c.is_started());
    /// ```
    pub fn lock(&mut self) {
        self.stop();
    }

    /// `psync_crypto_priv_key_flags` equivalent. Returns `0` when no flags
    /// are set. Mirrors `PSYNC_CRYPTO_FLAG_TEMP_PASS` via
    /// [`keys::PRIV_KEY_FLAG_TEMP_PASS`].
    ///
    /// ```
    /// assert_eq!(pcloud_crypto::CryptoShell::default().priv_key_flags(), 0);
    /// ```
    #[must_use]
    pub fn priv_key_flags(&self) -> u64 {
        self.keys.private_flags
    }

    /// `pcryptofolder_change_pass_unlocked` equivalent.
    ///
    /// Requires the shell to already be unlocked (started). Re-derives a
    /// new master key from `new_password`, replaces the setup fingerprint,
    /// rotates the Argon2 derivation salt, updates `private_flags`, and
    /// produces a signed opaque blob the caller can upload via the
    /// `crypto_changeuserprivate` wire call.
    ///
    /// After a successful call the shell remains unlocked *with the new key*
    /// so that the caller can, in a single follow-up, commit the change to
    /// the server and continue working without re-prompting the user.
    ///
    /// # Errors
    /// - [`CryptoError::Locked`] if not started.
    /// - [`CryptoError::EmptyPassword`] if `new_password` is empty.
    /// - [`CryptoError::PasswordUnchanged`] if the new password derives to
    ///   the exact same master key as the current one (constant-time check).
    /// - [`CryptoError::UnsafePolicy`] if the current policy forbids it.
    pub fn change_password_unlocked(
        &mut self,
        new_password: SecretString,
        flags: u64,
    ) -> Result<ReencodedPrivateKey, CryptoError> {
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if new_password.is_empty() {
            return Err(CryptoError::EmptyPassword);
        }
        // Caller must be unlocked — we need the *current* master key to sign
        // the new blob so the server can prove the rotation came from a
        // session that currently has access.
        let current_key = self
            .keys
            .active_key_material
            .as_ref()
            .ok_or(CryptoError::Locked)?
            .clone_secret();

        // Derive new key material under a fresh salt.
        let mut new_salt = vec![0u8; keys::DERIVATION_SALT_LEN];
        // INVARIANT: see keys::KeyManager::default — getrandom is always
        // available on supported targets (Linux/macOS/Windows).
        getrandom::getrandom(&mut new_salt)
            .expect("OS randomness should be available for crypto salt rotation");
        let new_key = keys::KeyManager::derive_key_material_with_salt(&new_password, &new_salt);

        // Note: we deliberately do NOT compare `new_key` against
        // `current_key` here. The derivation salt is rotated on each call,
        // so the derived keys differ even when the password stays the
        // same. Callers that want to reject identical passwords use
        // [`Self::change_password`], which performs a constant-time
        // byte-comparison of the two plaintext passwords up front.

        let new_fingerprint = keys::KeyManager::fingerprint_for(&new_key);

        // Build the opaque blob uploaded to the server. It is intentionally
        // version-tagged so future changes stay forward-compatible.
        //   layout = "pcrypto/v1/" || hex(salt) || "/" || hex(fingerprint) || "/" || hex(flags_le)
        let mut blob = String::from("pcrypto/v1/");
        blob.push_str(&hex_encode(&new_salt));
        blob.push('/');
        blob.push_str(&hex_encode(&new_fingerprint.0));
        blob.push('/');
        blob.push_str(&hex_encode(&flags.to_le_bytes()));

        // Signature: HMAC-SHA256(blob) under the *current* master key.
        let signature = hmac_sha256(&current_key, blob.as_bytes());

        // Stage a re-wrap of every outstanding KMS-wrapped DEK blob held
        // by this shell BEFORE we mutate any local state. This is the
        // "all-or-nothing" rewrap commit (bead pcloud-rs-a8j): if any
        // single unwrap/wrap fails, we return the KMS error and the
        // shell state is untouched — the caller sees a clean failure
        // and may safely retry with the same old-password context.
        //
        // Storage reality: `CryptoMode::Kms` persists a single master
        // wrapped DEK per shell (plus AAD `context`). There are no
        // per-folder wrapped blobs in the current serde shape, so
        // "outstanding blobs" is either zero (Raw mode) or one (Kms
        // mode). The staging vector is kept Vec-shaped so that a
        // future per-folder-DEK schema can extend this call site
        // without re-plumbing the atomicity envelope.
        let staged_mode = match &self.mode {
            CryptoMode::Raw => None,
            CryptoMode::Kms {
                key_id,
                wrapped_dek,
                context,
            } => Some(Self::rewrap_single_kms_blob(
                self.kms.as_ref(),
                key_id,
                wrapped_dek,
                context.as_deref(),
            )?),
        };

        // All rewraps succeeded (or there were none). Commit the new
        // key material and any re-wrapped KMS mode atomically.
        self.keys.derivation_salt = new_salt;
        self.keys.setup_fingerprint = Some(new_fingerprint);
        self.keys.private_flags = flags;
        self.keys.active_key_material = Some(new_key);
        if let Some(new_mode) = staged_mode {
            // Evict the process-local cache entry keyed on the OLD
            // wrapped blob so the stale plaintext DEK does not outlive
            // the rotation. The `PlaintextDek` held in the cache
            // zeroizes on drop.
            if let CryptoMode::Kms {
                key_id,
                wrapped_dek,
                context,
            } = &self.mode
            {
                let old_kid = pcloud_kms::KeyId(key_id.clone());
                let old_blob = pcloud_kms::WrappedDek(wrapped_dek.clone());
                let _ = pcloud_kms::evict_cached_dek(
                    self.kms.name(),
                    &old_kid,
                    &old_blob,
                    context.as_deref(),
                );
            }
            self.mode = new_mode;
        }

        Ok(ReencodedPrivateKey {
            private_key_hex: blob,
            signature_hex: hex_encode(&signature),
        })
    }

    /// Unwrap + re-wrap a single `(key_id, wrapped_dek, context)` tuple
    /// through the injected KMS provider and return a fresh
    /// [`CryptoMode::Kms`] carrying the new wrapped blob.
    ///
    /// Used by [`Self::change_password_unlocked`] to rotate outstanding
    /// KMS-wrapped DEK blobs atomically: the caller stages the returned
    /// value and only commits it once every blob has been re-wrapped
    /// successfully. The plaintext DEK is held in a
    /// [`pcloud_kms::PlaintextDek`] for the duration of this call and
    /// zeroizes on drop — it is never returned to the caller.
    ///
    /// Rewrap primitive: the [`pcloud_kms::KmsProvider`] trait does not
    /// expose a vendor-level `rewrap(blob, new_kek)`. We synthesise one
    /// via `decrypt_dek` followed by `encrypt_dek`. Both AWS KMS and
    /// Vault Transit produce a fresh ciphertext (new IV / version) even
    /// when the CMK is unchanged, so calling this on a shell whose KMS
    /// configuration has not moved still rotates the stored blob — a
    /// defence-in-depth property on password rotation.
    fn rewrap_single_kms_blob(
        kms: &dyn pcloud_kms::KmsProvider,
        key_id: &str,
        wrapped_dek: &[u8],
        context: Option<&str>,
    ) -> Result<CryptoMode, CryptoError> {
        let kid = pcloud_kms::KeyId(key_id.to_string());
        let old_blob = pcloud_kms::WrappedDek(wrapped_dek.to_vec());
        // Direct unwrap (not unwrap_cached): we want the fresh
        // plaintext, and we are about to evict the cache entry anyway.
        let plaintext = kms.decrypt_dek(&kid, &old_blob, context)?;
        if plaintext.expose().len() != KMS_DEK_LEN {
            return Err(CryptoError::KmsDekLen);
        }
        let new_wrapped = kms.encrypt_dek(&kid, &plaintext, context)?;
        // `plaintext` zeroizes on drop here.
        drop(plaintext);
        Ok(CryptoMode::Kms {
            key_id: key_id.to_string(),
            wrapped_dek: new_wrapped.0,
            context: context.map(str::to_owned),
        })
    }

    /// `pcryptofolder_change_pass` equivalent.
    ///
    /// Accepts the *old* password in addition to the new one, verifies the
    /// old password in constant time, starts a new session if the shell was
    /// locked, and then delegates to [`Self::change_password_unlocked`].
    ///
    /// If the shell was already unlocked when this is called, the old
    /// password is still required and is still checked against the stored
    /// setup fingerprint (constant-time); this prevents a caller that merely
    /// has a handle on a running daemon from rotating the crypto password
    /// without proving it knows the current one.
    ///
    /// WARNING: This operation re-derives the key from the new password without
    /// a key-encryption-key (KEK) layer. All existing ciphertext becomes
    /// inaccessible without the new password. There is no migration of existing
    /// encrypted data — callers must ensure all data is accessible before
    /// rotating.
    ///
    /// # Errors
    /// - [`CryptoError::WrongPassword`] if the old password fails the
    ///   fingerprint check.
    /// - Plus every error documented on [`Self::change_password_unlocked`].
    pub fn change_password(
        &mut self,
        old_password: SecretString,
        new_password: SecretString,
        flags: u64,
    ) -> Result<ReencodedPrivateKey, CryptoError> {
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if old_password.is_empty() || new_password.is_empty() {
            return Err(CryptoError::EmptyPassword);
        }
        if !self.is_setup() {
            return Err(CryptoError::NotSetup);
        }

        // Constant-time byte comparison of old and new passwords: refuse to
        // re-use the exact same password. This runs before we touch the key
        // derivation so it is cheap and does not leak via timing which part
        // of the input differs.
        {
            use pcloud_secret::ExposeSecret as _;
            let eq: bool = old_password
                .expose_secret()
                .as_bytes()
                .ct_eq(new_password.expose_secret().as_bytes())
                .into();
            if eq {
                return Err(CryptoError::PasswordUnchanged);
            }
        }

        // Verify old password against the stored setup fingerprint in
        // constant time regardless of whether the shell is already started.
        let derived_old = self.keys.derive_key_material(&old_password);
        if !self.keys.matches_setup(&derived_old) {
            drop(derived_old);
            return Err(CryptoError::WrongPassword);
        }

        // Ensure the shell is in the started state with `derived_old` as
        // the active key material so change_password_unlocked can sign the
        // new blob under the old key.
        if !self.is_started() {
            self.keys.active_key_material = Some(derived_old);
            self.unlock_state = state::UnlockState::Unlocked;
        } else {
            // Already started: we can drop `derived_old` — the shell holds
            // an equivalent key as active material.
            drop(derived_old);
        }

        self.change_password_unlocked(new_password, flags)
    }
}

/// Lowercase hex encoder. Local to avoid pulling `hex` into the dep graph.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn hmac_sha256(key: &pcloud_secret::secret_bytes::SecretBytes, msg: &[u8]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    use pcloud_secret::ExposeSecret;
    use sha2::Sha256;
    // INVARIANT: HMAC-SHA256 accepts keys of any non-zero length per RFC 2104;
    // callers always pass a 32-byte derived key.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.expose_secret())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

impl CryptoShell {
    /// `psync_crypto_reset` equivalent. Wipes all local crypto state,
    /// including the setup fingerprint and the encrypted-folder registry.
    /// This does NOT unlink remote encrypted content — that is the
    /// account-level reset which must go through the backend.
    ///
    /// ```
    /// use pcloud_crypto::CryptoShell;
    /// use pcloud_secret::secret_string::SecretString;
    /// let mut c = CryptoShell::default();
    /// c.setup(SecretString::new("pw"), None).unwrap();
    /// c.reset();
    /// assert!(!c.is_setup());
    /// ```
    pub fn reset(&mut self) {
        self.stop();
        self.keys.setup_fingerprint = None;
        self.folders.clear();
        self.next_local_folder_id = 1;
        self.hint = None;
        self.mode = CryptoMode::Raw;
        self.unlock_state = state::UnlockState::NotSetup;
    }

    /// `psync_crypto_folderid` equivalent — returns the id of an arbitrary
    /// known encrypted folder, if any.
    ///
    /// ```
    /// assert_eq!(pcloud_crypto::CryptoShell::default().any_folder_id(), None);
    /// ```
    #[must_use]
    pub fn any_folder_id(&self) -> Option<CryptoFolderId> {
        self.folders.keys().next().copied()
    }

    /// `psync_crypto_folderids` equivalent.
    ///
    /// ```
    /// assert!(pcloud_crypto::CryptoShell::default().folder_ids().is_empty());
    /// ```
    #[must_use]
    pub fn folder_ids(&self) -> Vec<CryptoFolderId> {
        self.folders.keys().copied().collect()
    }

    /// `psync_crypto_mkdir` equivalent: create an encrypted folder entry.
    ///
    /// This call is responsible for the *local* bookkeeping of an encrypted
    /// folder — encrypting the name and recording the parent link. The
    /// daemon layer is expected to pair this with a backend `createfolder`
    /// call so that the produced `encrypted_name` actually lands on the
    /// server. `local_folder_id`, when `None`, is auto-allocated.
    ///
    /// # Errors
    /// - [`CryptoError::Locked`] when crypto is not started.
    /// - [`CryptoError::InvalidName`] for empty / path-ful names.
    /// - [`CryptoError::FolderExists`] when `local_folder_id` is already taken.
    pub fn mkdir(
        &mut self,
        parent_folder_id: Option<CryptoFolderId>,
        name: &str,
        local_folder_id: Option<CryptoFolderId>,
    ) -> Result<CryptoFolderEntry, CryptoError> {
        let key = self
            .keys
            .active_key_material
            .as_ref()
            .ok_or(CryptoError::Locked)?;
        let encrypted_name = metadata::encrypt_filename(key, name)?;

        let folder_id = match local_folder_id {
            Some(id) => {
                if self.folders.contains_key(&id) {
                    return Err(CryptoError::FolderExists);
                }
                id
            }
            None => {
                let id = self.next_local_folder_id;
                self.next_local_folder_id = self.next_local_folder_id.saturating_add(1);
                id
            }
        };
        let entry = CryptoFolderEntry {
            folder_id,
            parent_folder_id,
            encrypted_name,
        };
        self.folders.insert(folder_id, entry.clone());
        Ok(entry)
    }

    /// Seal a sector for an existing file seed. Requires the shell to be
    /// started. Derives the per-file key as
    /// `HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)`
    /// and encrypts with AES-256-GCM (12-byte random nonce, 16-byte tag,
    /// sector index bound as AAD).
    ///
    /// # Security
    /// Mitigates: key reuse across files (per-file key derivation),
    /// sector reordering (sector index as AAD), ciphertext tampering
    /// (GCM tag), nonce-collision across files (per-file keyspace plus
    /// 96-bit random nonce), and plaintext exposure while locked (lock
    /// gate evaluated before any key derivation).
    ///
    /// Out of scope: nonce collisions within the same file when a caller
    /// writes `>= 2^48` sectors — sector-level rekey is expected every
    /// 2^32 sectors on the enterprise path but is not enforced here; the
    /// daemon owns the rekey schedule. Also out of scope: confidentiality
    /// of sector *length* (only the plaintext is encrypted, not the frame
    /// length).
    ///
    /// # Errors
    /// [`CryptoError::Locked`] if not started; propagates
    /// [`content::ContentCryptoError`] via `CryptoError::Content`.
    ///
    /// ```
    /// use pcloud_crypto::CryptoShell;
    /// use pcloud_secret::secret_string::SecretString;
    /// let mut c = CryptoShell::default();
    /// c.setup(SecretString::new("pw"), None).unwrap();
    /// c.start(SecretString::new("pw")).unwrap();
    /// let seed = [0u8; 32];
    /// let sealed = c.seal_sector(&seed, 0, b"hello").unwrap();
    /// let open = c.open_sector(&seed, 0, &sealed).unwrap();
    /// assert_eq!(open, b"hello");
    /// ```
    pub fn seal_sector(
        &self,
        file_seed: &[u8],
        sector_index: u32,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        // Enforce per-session AES-256-GCM 96-bit random-nonce budget
        // (H-2). Refuse new seals once the counter approaches
        // `u32::MAX - NONCE_BUDGET_SAFETY_MARGIN` — before birthday-bound
        // collision probability becomes non-negligible. The daemon must
        // rotate the per-file / master key and reset before proceeding.
        let budget_cap = u64::from(u32::MAX) - NONCE_BUDGET_SAFETY_MARGIN;
        let pre = self
            .sectors_sealed
            .load(std::sync::atomic::Ordering::Relaxed);
        if pre >= budget_cap {
            return Err(CryptoError::NonceBudgetExhausted);
        }
        let file_key = self.derive_sector_file_key(file_seed)?;
        let frame = content::seal_sector(
            &file_key,
            sector_index,
            plaintext,
            self.content.sector_size_bytes,
        )?;
        // Bump after success so a mid-seal error does not burn nonce budget.
        self.sectors_sealed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(frame)
    }

    /// Derive the per-file AES-256 key for a sector op, respecting
    /// [`Self::mode`]:
    ///
    /// - [`CryptoMode::Raw`] — derives from the Argon2id master key
    ///   via [`content::derive_file_key`] (legacy path).
    /// - [`CryptoMode::Kms`] — unwraps the session DEK via the
    ///   injected KMS (with TTL cache) and derives the per-file key
    ///   from the DEK instead. The plaintext DEK lives only inside
    ///   the cache entry; this helper returns a `SecretBytes` HKDF
    ///   output that zeroizes on drop.
    fn derive_sector_file_key(
        &self,
        file_seed: &[u8],
    ) -> Result<pcloud_secret::secret_bytes::SecretBytes, CryptoError> {
        // Caller must still be unlocked in both modes. The master key
        // is only used in Raw mode, but we keep the lock gate uniform
        // so that `stop()` still blocks sector ops end-to-end.
        let master = self
            .keys
            .active_key_material
            .as_ref()
            .ok_or(CryptoError::Locked)?;
        match &self.mode {
            CryptoMode::Raw => Ok(content::derive_file_key(master, file_seed)),
            CryptoMode::Kms { .. } => {
                let dek = self.unwrap_active_dek()?;
                // Treat the unwrapped DEK as a SecretBytes for the
                // HKDF-ish HMAC derivation in content::derive_file_key.
                let dek_secret =
                    pcloud_secret::secret_bytes::SecretBytes::new(dek.expose().to_vec());
                // `dek` zeroizes on drop here after we've copied into
                // SecretBytes (which also zeroizes on drop).
                drop(dek);
                Ok(content::derive_file_key(&dek_secret, file_seed))
            }
        }
    }

    /// Open a sector frame previously produced by [`Self::seal_sector`].
    ///
    /// Derives the same per-file key via
    /// `HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)`,
    /// checks the embedded sector index against `sector_index`, and
    /// verifies the AES-256-GCM tag.
    ///
    /// # Security
    /// Mitigates: sector swap / replay across sectors (the 4-byte sector
    /// index is bound into AAD and is checked *before* the AEAD call),
    /// ciphertext tampering (GCM tag), and decryption while locked
    /// (lock gate before any key derivation).
    ///
    /// Out of scope: cross-file replay — the caller must feed the correct
    /// `file_seed`; swapping sectors across different files with matching
    /// seeds is a higher-layer concern (file metadata consistency).
    ///
    /// # Errors
    /// [`CryptoError::Locked`] if not started, or any
    /// [`content::ContentCryptoError`] via `CryptoError::Content` on
    /// frame-shape / tag / index failures.
    pub fn open_sector(
        &self,
        file_seed: &[u8],
        sector_index: u32,
        frame: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let file_key = self.derive_sector_file_key(file_seed)?;
        let pt = content::open_sector(&file_key, sector_index, frame)?;
        Ok(pt)
    }
}

#[cfg(test)]
mod tests {
    use pcloud_secret::secret_string::SecretString;

    use super::{CryptoError, CryptoShell, state::UnlockState};

    fn pw(s: &str) -> SecretString {
        SecretString::new(s)
    }

    #[test]
    fn setup_then_start_then_stop_cycle() {
        let mut c = CryptoShell::default();
        assert_eq!(c.unlock_state, UnlockState::NotSetup);
        assert!(!c.is_setup());
        assert!(!c.is_started());

        c.setup(pw("hunter2"), Some("rhymes with gunter".into()))
            .unwrap();
        assert!(c.is_setup());
        assert!(!c.is_started());
        assert_eq!(c.unlock_state, UnlockState::Locked);

        c.start(pw("hunter2")).unwrap();
        assert!(c.is_started());
        assert_eq!(c.unlock_state, UnlockState::Unlocked);
        assert_eq!(c.get_hint(), Some("rhymes with gunter"));

        c.stop();
        assert!(!c.is_started());
        assert!(c.is_setup());
        assert_eq!(c.unlock_state, UnlockState::Locked);
        assert!(c.keys.active_key_material.is_none());
    }

    #[test]
    fn wrong_password_is_rejected_without_unlocking() {
        let mut c = CryptoShell::default();
        c.setup(pw("real"), None).unwrap();
        let err = c.start(pw("wrong")).expect_err("should fail");
        assert_eq!(err, CryptoError::WrongPassword);
        assert!(!c.is_started());
        assert_eq!(c.unlock_state, UnlockState::Locked);
    }

    #[test]
    fn double_setup_rejected() {
        let mut c = CryptoShell::default();
        c.setup(pw("a"), None).unwrap();
        assert_eq!(
            c.setup(pw("b"), None).unwrap_err(),
            CryptoError::AlreadySetup
        );
    }

    #[test]
    fn start_without_setup_rejected() {
        let mut c = CryptoShell::default();
        assert_eq!(c.start(pw("any")).unwrap_err(), CryptoError::NotSetup);
    }

    #[test]
    fn empty_password_rejected_on_setup_and_start() {
        let mut c = CryptoShell::default();
        assert_eq!(
            c.setup(pw(""), None).unwrap_err(),
            CryptoError::EmptyPassword
        );
        c.setup(pw("ok"), None).unwrap();
        assert_eq!(c.start(pw("")).unwrap_err(), CryptoError::EmptyPassword);
    }

    #[test]
    fn unsafe_policy_rejected() {
        let mut c = CryptoShell::default();
        c.policy.persist_master_key = true;
        assert_eq!(
            c.setup(pw("p"), None).unwrap_err(),
            CryptoError::UnsafePolicy
        );
    }

    #[test]
    fn reset_clears_everything() {
        let mut c = CryptoShell::default();
        c.setup(pw("pw"), Some("h".into())).unwrap();
        c.start(pw("pw")).unwrap();
        c.mkdir(None, "secrets", None).unwrap();
        c.reset();
        assert!(!c.is_setup());
        assert!(!c.is_started());
        assert!(c.folders.is_empty());
        assert!(c.get_hint().is_none());
        assert_eq!(c.unlock_state, UnlockState::NotSetup);
    }

    #[test]
    fn mkdir_requires_unlocked() {
        let mut c = CryptoShell::default();
        c.setup(pw("pw"), None).unwrap();
        assert_eq!(c.mkdir(None, "a", None).unwrap_err(), CryptoError::Locked);
        c.start(pw("pw")).unwrap();
        let entry = c.mkdir(None, "a", None).unwrap();
        assert!(!entry.encrypted_name.is_empty());
        assert_ne!(entry.encrypted_name, "a");
        assert!(c.folders.contains_key(&entry.folder_id));
    }

    #[test]
    fn mkdir_detects_collision() {
        let mut c = CryptoShell::default();
        c.setup(pw("pw"), None).unwrap();
        c.start(pw("pw")).unwrap();
        let e = c.mkdir(None, "x", Some(42)).unwrap();
        assert_eq!(e.folder_id, 42);
        assert_eq!(
            c.mkdir(None, "y", Some(42)).unwrap_err(),
            CryptoError::FolderExists
        );
    }

    #[test]
    fn folder_ids_reported() {
        let mut c = CryptoShell::default();
        c.setup(pw("pw"), None).unwrap();
        c.start(pw("pw")).unwrap();
        let a = c.mkdir(None, "a", None).unwrap().folder_id;
        let b = c.mkdir(None, "b", None).unwrap().folder_id;
        let ids = c.folder_ids();
        assert!(ids.contains(&a));
        assert!(ids.contains(&b));
        assert!(c.any_folder_id().is_some());
    }

    #[test]
    fn sector_seal_requires_unlocked_and_round_trips() {
        let mut c = CryptoShell::default();
        c.setup(pw("pw"), None).unwrap();
        assert_eq!(
            c.seal_sector(b"seed", 0, b"x").unwrap_err(),
            CryptoError::Locked
        );
        c.start(pw("pw")).unwrap();
        let frame = c.seal_sector(b"seed", 0, b"secret payload").unwrap();
        let round = c.open_sector(b"seed", 0, &frame).unwrap();
        assert_eq!(round, b"secret payload");
    }

    #[test]
    fn sector_open_rejected_when_locked() {
        let mut c = CryptoShell::default();
        c.setup(pw("pw"), None).unwrap();
        c.start(pw("pw")).unwrap();
        let frame = c.seal_sector(b"seed", 0, b"x").unwrap();
        c.stop();
        assert_eq!(
            c.open_sector(b"seed", 0, &frame).unwrap_err(),
            CryptoError::Locked
        );
    }

    #[test]
    fn priv_key_flags_defaults_to_zero() {
        let c = CryptoShell::default();
        assert_eq!(c.priv_key_flags(), 0);
    }

    #[test]
    fn change_password_unlocked_requires_started_shell() {
        let mut c = CryptoShell::default();
        c.setup(pw("orig"), None).unwrap();
        // Still locked — must refuse.
        let err = c
            .change_password_unlocked(pw("next"), 0)
            .expect_err("must fail when locked");
        assert_eq!(err, CryptoError::Locked);
    }

    #[test]
    fn change_password_unlocked_rejects_empty_password() {
        let mut c = CryptoShell::default();
        c.setup(pw("orig"), None).unwrap();
        c.start(pw("orig")).unwrap();
        assert_eq!(
            c.change_password_unlocked(pw(""), 0).unwrap_err(),
            CryptoError::EmptyPassword
        );
    }

    #[test]
    fn change_password_unlocked_rotates_fingerprint_and_records_flags() {
        let mut c = CryptoShell::default();
        c.setup(pw("orig"), None).unwrap();
        c.start(pw("orig")).unwrap();
        let old_fp = c.keys.setup_fingerprint.clone().expect("fingerprint");
        let old_salt = c.keys.derivation_salt.clone();

        let out = c
            .change_password_unlocked(pw("next"), crate::keys::PRIV_KEY_FLAG_TEMP_PASS)
            .expect("rotation ok");
        assert!(out.private_key_hex.starts_with("pcrypto/v1/"));
        assert!(!out.signature_hex.is_empty());
        assert_eq!(out.signature_hex.len(), 64); // hex of 32-byte MAC
        assert_eq!(c.priv_key_flags(), crate::keys::PRIV_KEY_FLAG_TEMP_PASS);
        assert!(c.is_started());
        assert_ne!(c.keys.setup_fingerprint.as_ref().unwrap().0, old_fp.0);
        assert_ne!(c.keys.derivation_salt, old_salt);

        // After rotation, old password must not unlock; new one must.
        c.stop();
        assert_eq!(c.start(pw("orig")).unwrap_err(), CryptoError::WrongPassword);
        c.start(pw("next")).expect("new password unlocks");
    }

    #[test]
    fn change_password_rejects_identical_password_by_constant_time_compare() {
        let mut c = CryptoShell::default();
        c.setup(pw("same"), None).unwrap();
        // Note: this goes through the old+new `change_password` entry
        // point, which does a constant-time byte-compare of the two
        // passwords before touching the key-derivation pipeline.
        let err = c
            .change_password(pw("same"), pw("same"), 0)
            .expect_err("identical pw must be rejected");
        assert_eq!(err, CryptoError::PasswordUnchanged);
        // State must be unchanged (no flags set, still only set up once).
        assert_eq!(c.priv_key_flags(), 0);
    }

    #[test]
    fn change_password_checks_old_and_reunlocks() {
        let mut c = CryptoShell::default();
        c.setup(pw("orig"), None).unwrap();

        // Wrong old password must be rejected even when shell is locked.
        assert_eq!(
            c.change_password(pw("wrong"), pw("next"), 0).unwrap_err(),
            CryptoError::WrongPassword
        );
        assert!(!c.is_started());

        // Correct old password must rotate, leaving the shell started with
        // the new key.
        let out = c
            .change_password(pw("orig"), pw("next"), 0)
            .expect("rotation ok");
        assert!(out.private_key_hex.starts_with("pcrypto/v1/"));
        assert!(c.is_started());

        // Full lock/unlock cycle with new password confirms rotation.
        c.stop();
        c.start(pw("next"))
            .expect("new password unlocks after stop");
    }

    #[test]
    fn change_password_empty_passwords_rejected() {
        let mut c = CryptoShell::default();
        c.setup(pw("orig"), None).unwrap();
        assert_eq!(
            c.change_password(pw(""), pw("next"), 0).unwrap_err(),
            CryptoError::EmptyPassword
        );
        assert_eq!(
            c.change_password(pw("orig"), pw(""), 0).unwrap_err(),
            CryptoError::EmptyPassword
        );
    }

    #[test]
    fn change_password_not_setup_rejected() {
        let mut c = CryptoShell::default();
        assert_eq!(
            c.change_password(pw("a"), pw("b"), 0).unwrap_err(),
            CryptoError::NotSetup
        );
    }

    #[test]
    fn change_password_signature_differs_between_rotations() {
        let mut c = CryptoShell::default();
        c.setup(pw("p0"), None).unwrap();
        c.start(pw("p0")).unwrap();
        let a = c.change_password_unlocked(pw("p1"), 0).unwrap();
        let b = c.change_password_unlocked(pw("p2"), 0).unwrap();
        assert_ne!(a.private_key_hex, b.private_key_hex);
        assert_ne!(a.signature_hex, b.signature_hex);
    }

    // ---- KMS re-wrap on password rotation (bead pcloud-rs-a8j) ----

    use super::{CryptoMode, KMS_DEK_LEN};
    use pcloud_kms::{KeyId, KmsError, KmsProvider, PlaintextDek, WrappedDek};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal in-memory KMS provider used to exercise the rewrap path.
    ///
    /// Wrapped layout: 4-byte LE `sequence` || plaintext bytes. Each
    /// call to `encrypt_dek` bumps `sequence` so successive wraps of
    /// the same plaintext produce distinct ciphertexts — exactly as
    /// AWS KMS and Vault Transit do in production.
    struct SeqMockKms {
        name: &'static str,
        seq: AtomicUsize,
        fail_encrypt_after: AtomicUsize,
        fail_decrypt: AtomicUsize,
    }
    impl SeqMockKms {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                seq: AtomicUsize::new(0),
                fail_encrypt_after: AtomicUsize::new(usize::MAX),
                fail_decrypt: AtomicUsize::new(0),
            }
        }
        /// Fail the Nth `encrypt_dek` call (0-indexed).
        fn fail_encrypt_at(&self, n: usize) {
            self.fail_encrypt_after.store(n, Ordering::SeqCst);
        }
    }
    impl KmsProvider for SeqMockKms {
        fn name(&self) -> &'static str {
            self.name
        }
        fn encrypt_dek(
            &self,
            _k: &KeyId,
            dek: &PlaintextDek,
            _c: Option<&str>,
        ) -> Result<WrappedDek, KmsError> {
            let n = self.seq.fetch_add(1, Ordering::SeqCst);
            if n >= self.fail_encrypt_after.load(Ordering::SeqCst) {
                return Err(KmsError::Unreachable("mock-encrypt-fail".into()));
            }
            let mut out = Vec::with_capacity(4 + dek.expose().len());
            out.extend_from_slice(&(n as u32).to_le_bytes());
            out.extend_from_slice(dek.expose());
            Ok(WrappedDek(out))
        }
        fn decrypt_dek(
            &self,
            _k: &KeyId,
            wrapped: &WrappedDek,
            _c: Option<&str>,
        ) -> Result<PlaintextDek, KmsError> {
            if self.fail_decrypt.load(Ordering::SeqCst) != 0 {
                return Err(KmsError::PolicyDenied);
            }
            if wrapped.0.len() < 4 {
                return Err(KmsError::Malformed);
            }
            Ok(PlaintextDek(wrapped.0[4..].to_vec()))
        }
        fn health_check(&self) -> Result<(), KmsError> {
            Ok(())
        }
    }

    /// Extract the wrapped-DEK bytes from a `CryptoMode::Kms` shell.
    fn kms_wrapped_bytes(c: &CryptoShell) -> Vec<u8> {
        match &c.mode {
            CryptoMode::Kms { wrapped_dek, .. } => wrapped_dek.clone(),
            CryptoMode::Raw => panic!("expected CryptoMode::Kms"),
        }
    }

    #[test]
    fn kms_rewrap_rewraps_outstanding_deks_on_password_change() {
        let mut c = CryptoShell::default().with_kms_provider(Box::new(SeqMockKms::new("mock-ok")));
        c.setup(pw("orig"), None).unwrap();
        c.start(pw("orig")).unwrap();
        c.enable_kms_mode("mock-cmk", Some("tenant-a".into()))
            .expect("enable kms");
        let before = kms_wrapped_bytes(&c);
        assert!(matches!(c.mode, CryptoMode::Kms { .. }));

        c.change_password_unlocked(pw("next"), 0)
            .expect("rotation ok");

        // Mode must still be Kms with the same key_id/context but a
        // freshly re-wrapped blob distinct from the pre-rotation one.
        match &c.mode {
            CryptoMode::Kms {
                key_id,
                wrapped_dek,
                context,
            } => {
                assert_eq!(key_id, "mock-cmk");
                assert_eq!(context.as_deref(), Some("tenant-a"));
                assert_ne!(
                    wrapped_dek, &before,
                    "wrapped DEK must rotate on password change"
                );
                // Same DEK plaintext preserved (mock layout: 4-byte seq || dek).
                assert_eq!(&wrapped_dek[4..], &before[4..]);
                assert_eq!(wrapped_dek.len(), 4 + KMS_DEK_LEN);
            }
            CryptoMode::Raw => panic!("mode must stay Kms after rewrap"),
        }

        // New password must unlock through a full stop/start cycle.
        c.stop();
        c.start(pw("next")).expect("new password unlocks");
    }

    #[test]
    fn kms_rewrap_rollback_on_mid_operation_failure() {
        let mock = Box::new(SeqMockKms::new("mock-rollback"));
        // The first encrypt (enable_kms_mode) must succeed; the second
        // encrypt (the rewrap) must fail so change_password_unlocked
        // has to roll back.
        mock.fail_encrypt_at(1);
        let mut c = CryptoShell::default().with_kms_provider(mock);
        c.setup(pw("orig"), None).unwrap();
        c.start(pw("orig")).unwrap();
        c.enable_kms_mode("cmk-rb", Some("ctx-rb".into()))
            .expect("enable kms");

        // Snapshot pre-rotation state.
        let before_wrapped = kms_wrapped_bytes(&c);
        let before_salt = c.keys.derivation_salt.clone();
        let before_fp = c.keys.setup_fingerprint.clone();
        let before_flags = c.priv_key_flags();

        // Rewrap must fail.
        let err = c
            .change_password_unlocked(pw("next"), 0x1234)
            .expect_err("rewrap must fail and roll the whole op back");
        assert!(matches!(err, CryptoError::Kms));

        // EVERY piece of state must be unchanged: key material, salt,
        // fingerprint, flags, and the wrapped DEK blob.
        assert_eq!(c.keys.derivation_salt, before_salt);
        assert_eq!(
            c.keys.setup_fingerprint.as_ref().unwrap().0,
            before_fp.unwrap().0
        );
        assert_eq!(c.priv_key_flags(), before_flags);
        assert_eq!(kms_wrapped_bytes(&c), before_wrapped);

        // The OLD password must still unlock. The new one must not.
        c.stop();
        assert_eq!(
            c.start(pw("next")).unwrap_err(),
            CryptoError::WrongPassword
        );
        c.start(pw("orig")).expect("old password still works");
    }

    #[test]
    fn unlock_shim_back_compat_setup_plus_start() {
        let mut c = CryptoShell::default();
        c.unlock(pw("pw")).unwrap();
        assert!(c.is_started());
        c.lock();
        assert!(!c.is_started());
        // Second call must succeed on an already-set-up shell.
        c.unlock(pw("pw")).unwrap();
        assert!(c.is_started());
        // Wrong password should still be rejected.
        c.lock();
        assert_eq!(c.unlock(pw("bad")).unwrap_err(), CryptoError::WrongPassword);
    }
}

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

/// Wave 1 pclsync-v2 wire-compat primitives. Gated off the default build so
/// the active Argon2/GCM path is untouched during incremental rollout.
#[cfg(feature = "pclsync-v2")]
pub mod pclsync_kdf;

/// Wave 1 / Primitive B — pclsync-compatible RSA-4096 keypair generation and
/// RSAES-OAEP (SHA-1 hash + SHA-1 MGF1, empty label) wrap/unwrap for the
/// `sym_key_ver1` symmetric key bundle. Mirrors the mbedtls defaults used by
/// the legacy C client (`pclsync/pssl.c` and `pclsync/pcryptofolder.c`).
#[cfg(feature = "pclsync-v2")]
pub mod pclsync_rsa;

/// Wave 1 / Primitive D — pclsync-compatible per-sector AEAD
/// (`pcrypto_encode_sec` / `pcrypto_decode_sec`). Byte-for-byte interop with
/// the legacy C client's sector cipher (`pclsync/pcrypto.c`).
#[cfg(feature = "pclsync-v2")]
pub mod pclsync_sector;

/// Wave 1 / Primitive C — AES-256-CTR (priv-key wrap) and AES-256-CBC-CS3
/// (per-sector data cipher) primitives.
#[cfg(feature = "pclsync-v2")]
pub mod pclsync_modes;

/// Wave 1 / Primitive E — 128-ary Merkle authentication tree over per-sector
/// tags. Mirrors the `pfs_crpt_*` tree layout in `pclsync/pfscrypto.c`.
#[cfg(feature = "pclsync-v2")]
pub mod pclsync_auth_tree;

/// Wave 1 / Primitive F — pclsync-compatible reversible filename
/// encoding (`pcrypto_encode_text` / `pcrypto_decode_text` + base32
/// envelope, `pclsync/pcrypto.c:273..390` and
/// `pclsync/putil.c:189..271`). Byte-for-byte interop with the legacy
/// C client's directory-listing wire format.
#[cfg(feature = "pclsync-v2")]
pub mod pclsync_filename;

/// Wave 2 Stage 2+3 — PclsyncCompat profile codec (priv_key_ver1 /
/// pub_key_ver1 blob build+parse), KEK-wrap roundtrip for the RSA
/// private key, and runtime state (live RSA key + folder/file sym-key
/// caches). See module docs for the C struct reference citations.
#[cfg(feature = "pclsync-v2")]
pub mod pclsync_compat_profile;

/// RSA-4096-OAEP wrap of a folder/file `SymKeyVer1` for crypto
/// share-invitation (C-interop path). Tracked under
/// `pcloud-rs-ncx.89`. See module docs for the C reference flow.
#[cfg(feature = "pclsync-v2")]
pub mod share_rsa;

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

/// Which crypto scheme a profile uses. See `docs/CRYPTO-BACKEND-PLAN.md`
/// and `docs/enterprise/crypto-compat.md`.
///
/// Wire-incompatible with each other by design: once a profile is
/// sealed under one backend, files cannot be decrypted under the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CryptoBackend {
    /// Byte-compatible with the official pCloud C client, desktop,
    /// iOS/Android apps, and web. Uses PBKDF2-HMAC-SHA512 + RSA-4096 +
    /// custom sector AEAD. **Default for new profiles.**
    PclsyncCompat,
    /// Stricter AEAD (AES-256-GCM) + Argon2id KDF. Opt-in only.
    /// Files encrypted under this backend will NOT decrypt in the
    /// official pCloud apps. Requires explicit user acknowledgement.
    Enhanced,
}

impl Default for CryptoBackend {
    fn default() -> Self {
        Self::PclsyncCompat
    }
}

impl std::fmt::Display for CryptoBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PclsyncCompat => "pclsync-compat",
            Self::Enhanced => "enhanced",
        })
    }
}

impl CryptoBackend {
    /// Returns `true` iff files encrypted under this backend can be
    /// decrypted by the official pCloud C client and apps.
    #[must_use]
    pub const fn interoperable_with_pcloud_apps(self) -> bool {
        matches!(self, Self::PclsyncCompat)
    }
}

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
    /// The profile on disk is sealed under a different backend than the
    /// caller requested at runtime. Mixing backends would silently corrupt
    /// ciphertext; the shell refuses the operation outright.
    #[error("backend mismatch: profile sealed under {expected}, caller requested {provided}")]
    BackendMismatch {
        /// Backend the persisted profile was sealed under.
        expected: CryptoBackend,
        /// Backend the caller asked the shell to use.
        provided: CryptoBackend,
    },
    /// The requested operation is not yet wired for the PclsyncCompat
    /// backend. Tracked by `bd-1du.10` Stage 4 (IPC + folder-key cache
    /// plumbing for sector/filename ops + real RSA change-password flow).
    #[error("operation not yet wired for pclsync-compat backend (bd-1du.10)")]
    NotYetWired,
    /// A PclsyncCompat sector operation was invoked without a `file_id`
    /// in its [`SectorContext`]. Unlike the Enhanced backend — which
    /// derives a per-file key from the caller-supplied seed — the
    /// PclsyncCompat backend looks up the file's `SymKeyVer1` by
    /// server-assigned `file_id`, so the caller MUST thread an id
    /// through the API. Stage 4b resolves this for every IPC caller.
    #[error("pclsync-compat sector operation requires a file_id")]
    MissingFileId,
    /// A PclsyncCompat filename / folder operation was invoked without
    /// a `folder_id`. The parent folder's `SymKeyVer1` is used to
    /// encode child filenames, so callers must pass the parent id.
    #[error("pclsync-compat filename operation requires a folder_id")]
    MissingFolderId,
    /// The PclsyncCompat sym-key cache does not contain an entry for
    /// the requested `file_id`. The daemon is expected to call
    /// `crypto_getfilekey`, RSA-OAEP-unwrap the returned blob into a
    /// `SymKeyVer1`, populate
    /// [`pclsync_compat_profile::PclsyncCompatState::cache_file_key`],
    /// and retry.
    #[error("pclsync-compat file key not cached: file_id={file_id}")]
    FileKeyNotCached {
        /// The server-assigned file id whose sym-key is missing.
        file_id: u64,
    },
    /// The PclsyncCompat sym-key cache does not contain an entry for
    /// the requested `folder_id`. The daemon is expected to call
    /// `crypto_getfolderkey`, RSA-OAEP-unwrap the result, populate
    /// [`pclsync_compat_profile::PclsyncCompatState::cache_folder_key`],
    /// and retry.
    #[error("pclsync-compat folder key not cached: folder_id={folder_id}")]
    FolderKeyNotCached {
        /// The server-assigned folder id whose sym-key is missing.
        folder_id: u64,
    },
    /// PclsyncCompat profile codec / unwrap failure. Always mapped to
    /// an opaque error variant so the underlying RSA / DER / padding
    /// taxonomy does not leak to the user.
    #[error("pclsync-compat profile error")]
    PclsyncCompat,
    /// Per-session AES-256-GCM nonce budget exhausted. With 96-bit random
    /// nonces, the safe encryption budget for a single key is ~2^32
    /// operations. The shell refuses further [`CryptoShell::seal_sector`]
    /// calls when the counter approaches `u32::MAX` minus a safety margin
    /// so the daemon rotates the per-file / master key before nonce
    /// collision becomes non-negligible.
    #[error("nonce budget exhausted: key rotation required before further sector seals")]
    NonceBudgetExhausted,
    /// Caller passed an empty plaintext to a sector-seal operation. Empty
    /// sectors are rejected explicitly rather than silently producing an
    /// all-random ciphertext, which would be undetectable by the file-system
    /// layer and produce an undecryptable blob (M-3.6 / audit-05).
    #[error("sector plaintext must not be empty")]
    EmptySector,
    /// The in-memory master key has exceeded its configured TTL
    /// ([`keys::KeyManager::cache_ttl_secs`]). The daemon must prompt the
    /// user to re-enter their crypto password ([`CryptoShell::start`]) before
    /// further encrypted operations are attempted.
    ///
    /// This error is returned by any operation that requires the master key
    /// when lazy eviction detects an expired key. The key material is
    /// zeroized before the error is surfaced.
    #[error("master key TTL expired: re-unlock required")]
    MasterKeyExpired,
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

/// Caller-supplied context for a sector operation under the PclsyncCompat
/// backend.
///
/// The Enhanced backend derives a per-file key from a caller-owned seed
/// and therefore does not need any server-assigned id. PclsyncCompat, by
/// contrast, looks up the file's `SymKeyVer1` by `file_id` in the cache
/// populated lazily from `crypto_getfilekey`. This struct threads the id
/// through the API without disturbing existing Enhanced call sites.
///
/// For Enhanced call sites, use [`SectorContext::enhanced()`] (all
/// fields `None`). For PclsyncCompat, use [`SectorContext::for_file`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SectorContext {
    /// Server-assigned file id whose `SymKeyVer1` should be fetched
    /// from the PclsyncCompat sym-key cache. Ignored by Enhanced.
    pub file_id: Option<u64>,
}

impl SectorContext {
    /// Construct a context suitable for Enhanced sector operations
    /// (all fields `None`). Accepted by PclsyncCompat only as an error
    /// sentinel — returns [`CryptoError::MissingFileId`].
    #[must_use]
    pub const fn enhanced() -> Self {
        Self { file_id: None }
    }

    /// Construct a PclsyncCompat sector context bound to `file_id`.
    #[must_use]
    pub const fn for_file(file_id: u64) -> Self {
        Self { file_id: Some(file_id) }
    }
}

/// A sealed sector frame returned by [`CryptoShell::seal_sector_with_context`].
///
/// Carries the ciphertext byte block plus, for the PclsyncCompat backend,
/// a 32-byte detached auth tag. The Enhanced backend emits a monolithic
/// AES-GCM frame (ciphertext ++ auth-tag-inline) and leaves `auth_tag`
/// set to `None`. The PclsyncCompat backend emits raw sector ciphertext
/// and a detached 32-byte tag that must be persisted alongside the
/// ciphertext in the Merkle-like auth-sector tree.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SealedSectorFrame {
    /// Sector ciphertext. For Enhanced this is the full AES-GCM frame;
    /// for PclsyncCompat it is the raw sector ciphertext (same length
    /// as plaintext).
    pub ciphertext: Vec<u8>,
    /// PclsyncCompat detached 32-byte auth tag. `None` on the Enhanced
    /// path (tag is inlined into `ciphertext`).
    pub auth_tag: Option<[u8; 32]>,
}

/// Result of [`CryptoShell::mkdir_with_context`] — a created encrypted
/// folder entry plus, for the PclsyncCompat backend, a freshly generated
/// `SymKeyVer1` that the daemon must RSA-OAEP-wrap to the user's public
/// key and upload via `crypto_mkdir` before caching it back into
/// [`pclsync_compat_profile::PclsyncCompatState::cache_folder_key`].
///
/// Enhanced callers see `sym_key = None`.
///
/// The symmetric key material is wrapped by `SymKeyVer1` (which holds
/// `Zeroizing` buffers internally) and is never logged or serialized.
#[cfg(feature = "pclsync-v2")]
#[derive(Debug)]
pub struct CreatedCryptoFolder {
    /// Local bookkeeping entry (folder id + encoded filename + parent
    /// link).
    pub entry: CryptoFolderEntry,
    /// PclsyncCompat: freshly generated folder sym-key, ready for
    /// RSA-OAEP wrap. `None` for Enhanced.
    pub sym_key: Option<pclsync_rsa::SymKeyVer1>,
}

/// Enhanced-only companion to [`CreatedCryptoFolder`] when the crate is
/// built without the `pclsync-v2` feature flag. Kept as a distinct
/// type (not `#[cfg]`-hidden fields on `CreatedCryptoFolder`) so
/// feature flips never change the public shape in a way that would
/// silently break downstream `match` arms.
#[cfg(not(feature = "pclsync-v2"))]
#[derive(Debug)]
pub struct CreatedCryptoFolder {
    /// Local bookkeeping entry.
    pub entry: CryptoFolderEntry,
}

/// Result of [`CryptoShell::change_password_with_context`] for the
/// PclsyncCompat backend.
///
/// Carries the new `priv_key_ver1` blob (RSA priv DER re-wrapped under
/// a fresh salt and the new-password KEK), the new pub-key fingerprint,
/// and the `flags` word that the daemon must post via
/// `crypto_changeuserkeys`.
///
/// The shell state (`pclsync_compat.priv_key_ver1_blob` and
/// `pclsync_compat.pub_fingerprint`) is updated in-place before this
/// is returned, so a successful call means local-side rotation has
/// already committed. The daemon is expected to upload atomically —
/// a failed upload leaves the shell ahead of the server, which is
/// safe: the stale priv blob has already been wiped, and a subsequent
/// retry is idempotent because the daemon resends the same blob.
///
/// The wrapped DER blob is held in a `Zeroizing<Vec<u8>>` so an
/// accidental copy is zeroed on drop. We never expose the plaintext
/// DER or the derived KEK to the caller.
#[cfg(feature = "pclsync-v2")]
#[derive(Debug)]
pub struct ChangePasswordResult {
    /// New `priv_key_ver1` blob to be uploaded.
    pub new_priv_key_ver1_blob: zeroize::Zeroizing<Vec<u8>>,
    /// New pub-key fingerprint (non-secret, safe to log).
    pub new_pub_fingerprint: [u8; 32],
    /// `flags` word from the priv_key_ver1 struct (caller-controlled,
    /// opaque to this layer).
    pub flags: u32,
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
    /// # What the budget is
    ///
    /// AES-256-GCM with a 96-bit random nonce is safe up to roughly `2^32`
    /// encryptions per key before birthday-bound nonce collision probability
    /// becomes non-negligible (NIST SP 800-38D §8.3). We enforce a hard cap
    /// at `u32::MAX - NONCE_BUDGET_SAFETY_MARGIN` via
    /// [`CryptoShell::seal_sector`]; once the counter crosses that threshold
    /// the shell returns [`CryptoError::NonceBudgetExhausted`] and refuses
    /// to issue further sector nonces until the key is rotated.
    ///
    /// # When the budget resets
    ///
    /// The counter is zeroed on exactly two events:
    ///
    /// 1. **Successful password change / key rotation** — the master key
    ///    that parameterises the derived per-file AES keys has changed, so
    ///    the old nonce space is cryptographically a different random
    ///    domain. See [`CryptoShell::change_password_unlocked`] (near line
    ///    1888 in this file) where `self.sectors_sealed.store(0, SeqCst)`
    ///    runs after the successful rewrap.
    /// 2. **`reset()` / fresh `setup()`** — a `reset()` drops the shell
    ///    state back to `NotSetup` and a subsequent `setup()` installs a
    ///    brand-new master key; the counter is reinitialised to zero by
    ///    the `Default` impl (line 880) so the new key gets a fresh budget.
    ///
    /// # Why reset is safe
    ///
    /// Reset is safe because the "budget" is a property of the *active key
    /// schedule*, not the persisted state. The birthday bound on AES-GCM
    /// nonce reuse applies within the same key — once the key is rotated,
    /// the counter from the previous key carries no cryptographic meaning
    /// against the new key. Resetting does **not** relax the bound; it
    /// simply starts a fresh counter for the new key. An attacker who
    /// forces frequent rotations only ever gets one budget window per key,
    /// and each budget window is independently capped.
    ///
    /// # Persistence / restart behaviour
    ///
    /// Persisted across daemon restarts via `atomic_u64_serde` so the
    /// nonce-exhaustion guard is **not** silently reset by a process
    /// restart (an attacker could otherwise force the counter back to
    /// zero by crashing/restarting the daemon under the same master key).
    /// A restart under the same key resumes from the persisted count.
    ///
    /// A restart *after* a key rotation deserialises the already-zeroed
    /// counter — the reset happened in-memory at rotation time and was
    /// then flushed to disk as part of the shell's persisted state.
    #[serde(with = "atomic_u64_serde", default = "default_atomic_u64")]
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
    /// Crypto scheme this profile is sealed under.
    ///
    /// Stored as `Option` so that historical Enhanced profiles (written
    /// before the `CryptoBackend` enum existed) can be detected by
    /// absence of the field and migrated through
    /// [`Self::effective_backend`]. Fresh profiles always set this to
    /// `Some(_)`; loaders and savers downstream of Stage 2/3 must round-
    /// trip it faithfully and never silently flip it.
    #[serde(default)]
    pub backend: Option<CryptoBackend>,
    /// PclsyncCompat persisted profile (priv_key_ver1 blob, pub_key_ver1
    /// blob, pub fingerprint, flags). `None` for profiles sealed under
    /// the Enhanced backend. See
    /// [`pclsync_compat_profile::PclsyncCompatProfile`] for the layout.
    #[cfg(feature = "pclsync-v2")]
    #[serde(default)]
    pub pclsync_compat: Option<pclsync_compat_profile::PclsyncCompatProfile>,
    /// Runtime-only PclsyncCompat state (live RSA private key + sym-key
    /// caches). Populated by `start_pclsync_compat`; cleared by `stop`.
    /// Never serialised.
    #[cfg(feature = "pclsync-v2")]
    #[serde(skip)]
    pub pclsync_compat_state: Option<pclsync_compat_profile::PclsyncCompatState>,
    /// Monotonic deadline until which further unlock attempts are rejected
    /// (brute-force backoff). Complements [`Self::last_fail_at`] (wall-clock,
    /// persisted) with a monotonic guard that survives clock rewind within
    /// the same daemon session.
    ///
    /// Set on each failed unlock to `Instant::now() + lockout_backoff`.
    /// Cleared on a successful unlock (`None`). Not serialised —
    /// process-local `Instant` values have no meaning across restarts; the
    /// persisted `last_fail_at` wall-clock timestamp handles the
    /// cross-restart case.
    ///
    /// # Security
    /// Mitigates: clock-rewind attacks where an attacker rewinds the
    /// system clock to bypass the wall-clock backoff check. The monotonic
    /// check cannot be sidestepped without killing the daemon process
    /// (at which point the wall-clock guard takes over). Uses
    /// `std::time::Instant` which is monotonic per POSIX and guaranteed
    /// not to go backward within a process lifetime.
    #[serde(skip)]
    pub lockout_monotonic_floor: Option<std::time::Instant>,
}

impl CryptoShell {
    /// Returns the effective backend for this profile.
    ///
    /// Decision: the persisted `backend` field is the **ground truth
    /// when present**. When it is absent (historical profile written
    /// before Wave 2 Stage 1) we fall back to a sentinel inference:
    /// a historical profile that has already completed `setup()` will
    /// have `keys.setup_fingerprint == Some(_)`. Those profiles were
    /// necessarily written under the Enhanced (Argon2id) path — that
    /// was the only backend that existed before Wave 2 — so we infer
    /// [`CryptoBackend::Enhanced`] in that case. If the profile has
    /// never been set up (`setup_fingerprint` is `None`) there is no
    /// historical ciphertext to honor, so we return the current
    /// [`CryptoBackend::default()`] (= `PclsyncCompat`).
    ///
    /// This method is read-only: it does not mutate `self.backend`.
    /// Stage 2 loaders are responsible for rewriting the profile with
    /// the inferred value on first load so the inference only runs
    /// once per migration.
    #[must_use]
    pub fn effective_backend(&self) -> CryptoBackend {
        if let Some(b) = self.backend {
            return b;
        }
        // Sentinel: historical Enhanced profile (pre-Wave-2) if setup
        // has already been completed.
        if self.keys.setup_fingerprint.is_some() {
            CryptoBackend::Enhanced
        } else {
            CryptoBackend::default()
        }
    }
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            #[serde(with = "super", default = "default_zero")]
            val: AtomicU32,
        }
        fn default_zero() -> AtomicU32 {
            AtomicU32::new(0)
        }

        fn round_trip(v: u32) {
            let w = Wrapper {
                val: AtomicU32::new(v),
            };
            let ser = serde_json::to_string(&w).expect("serialize");
            let back: Wrapper = serde_json::from_str(&ser).expect("deserialize");
            assert_eq!(
                back.val.load(Ordering::Relaxed),
                v,
                "AtomicU32 round-trip value mismatch for {v}"
            );
        }

        /// audit-06 P3 / pcloud-rs-ncx.37: serde shim for AtomicU32 must
        /// round-trip every value in the u32 domain exactly.
        #[test]
        fn atomic_u32_serde_round_trip_all_corners() {
            for v in [0u32, 1, 2, 42, 12345, u32::MAX / 2, u32::MAX - 1, u32::MAX] {
                round_trip(v);
            }
        }
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            #[serde(with = "super", default = "default_zero")]
            val: AtomicU64,
        }
        fn default_zero() -> AtomicU64 {
            AtomicU64::new(0)
        }

        fn round_trip(v: u64) {
            let w = Wrapper {
                val: AtomicU64::new(v),
            };
            let ser = serde_json::to_string(&w).expect("serialize");
            let back: Wrapper = serde_json::from_str(&ser).expect("deserialize");
            assert_eq!(
                back.val.load(Ordering::Relaxed),
                v,
                "AtomicU64 round-trip value mismatch for {v}"
            );
        }

        /// audit-06 P3 / pcloud-rs-ncx.37: serde shim for AtomicU64 must
        /// round-trip every value in the u64 domain exactly, including the
        /// values the shell writes (`sectors_sealed` and `last_fail_at`).
        #[test]
        fn atomic_u64_serde_round_trip_all_corners() {
            for v in [
                0u64,
                1,
                12345,
                u64::from(u32::MAX),
                u64::from(u32::MAX) + 1,
                u64::MAX / 2,
                u64::MAX - 1,
                u64::MAX,
            ] {
                round_trip(v);
            }
        }

        /// audit-06 P3 / pcloud-rs-ncx.37 explicit example from the bead:
        /// sectors_sealed=12345 must survive a full serialize→deserialize
        /// round-trip through the atomic serde shim.
        #[test]
        fn atomic_u64_serde_sectors_sealed_example() {
            round_trip(12345u64);
        }
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
            // hint: redact content; only show presence/absence to avoid leaking
            // partial password context into logs (audit-06 LOW crypto L-1 / ncx.79-f).
            .field("hint", &self.hint.as_deref().map(|_| "<set>"))
            .field("kms", &self.kms.name())
            .field("mode", &self.mode.tag())
            .field("backend", &self.backend)
            .finish_non_exhaustive()
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
            backend: None,
            #[cfg(feature = "pclsync-v2")]
            pclsync_compat: None,
            #[cfg(feature = "pclsync-v2")]
            pclsync_compat_state: None,
            lockout_monotonic_floor: None,
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
    // Invariant: `failures` should never substantially exceed
    // MAX_CONSECUTIVE_FAILURES (10) plus a small processing-race margin.
    // If it does, the counter is being incremented outside the normal lockout
    // path. Surface the anomaly in debug builds (audit-06 LOW crypto L-1).
    debug_assert!(
        failures <= MAX_CONSECUTIVE_FAILURES + 30,
        "lockout_backoff_secs: failures={} far exceeds MAX_CONSECUTIVE_FAILURES;          counter may be incremented from an unexpected path",
        failures
    );
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
        if self.keys.setup_fingerprint.is_some() {
            return true;
        }
        #[cfg(feature = "pclsync-v2")]
        {
            return self.pclsync_compat.is_some();
        }
        #[cfg(not(feature = "pclsync-v2"))]
        {
            false
        }
    }

    /// `psync_crypto_isstarted` equivalent.
    ///
    /// ```
    /// let c = pcloud_crypto::CryptoShell::default();
    /// assert!(!c.is_started());
    /// ```
    #[must_use]
    pub fn is_started(&self) -> bool {
        if !self.unlock_state.is_started() {
            return false;
        }
        if self.keys.active_key_material.is_some() {
            return true;
        }
        #[cfg(feature = "pclsync-v2")]
        {
            return self.pclsync_compat_state.is_some();
        }
        #[cfg(not(feature = "pclsync-v2"))]
        {
            false
        }
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

    /// Borrow the active master key, enforcing the configured TTL.
    ///
    /// On each call:
    /// 1. Calls [`keys::KeyManager::check_and_evict_if_stale`]; if the key
    ///    has exceeded its TTL the key material is zeroized and
    ///    [`CryptoError::MasterKeyExpired`] is returned.
    /// 2. Returns [`CryptoError::Locked`] if there is no key.
    /// 3. On success, calls [`keys::KeyManager::touch`] to slide the TTL
    ///    window forward and returns a reference to the key.
    ///
    /// This is the single choke-point that all sector-encrypt / sector-decrypt
    /// / filename-encrypt operations must go through so that `cache_ttl_secs`
    /// is actually enforced. Setting `cache_ttl_secs = 0` disables TTL.
    fn require_active_key(&mut self) -> Result<&pcloud_secret::secret_bytes::SecretBytes, CryptoError> {
        if self.keys.check_and_evict_if_stale() {
            // Key was live but has expired; drop back to Locked.
            self.unlock_state = state::UnlockState::Locked;
            return Err(CryptoError::MasterKeyExpired);
        }
        // Touch (refresh the LRU timestamp) before borrowing key material so
        // that Rust's borrow checker sees only one &mut self operation here.
        self.keys.touch();
        let key = self
            .keys
            .active_key_material
            .as_ref()
            .ok_or(CryptoError::Locked)?;
        Ok(key)
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
        // Back-compat default: preserve historical Enhanced behaviour for any
        // existing caller that didn't opt in to a specific backend. New
        // callers that want the PclsyncCompat default should use
        // [`Self::setup_with_backend`].
        self.setup_with_backend(password, hint, CryptoBackend::Enhanced)
    }

    /// Backend-aware setup. Dispatches to the per-backend setup body and
    /// persists `self.backend = Some(backend)` on success so future
    /// `start()` calls route correctly without re-running the sentinel
    /// inference.
    ///
    /// # Errors
    /// Same as [`Self::setup`], plus any backend-specific setup failures
    /// (RSA keygen / OS RNG / DER serialisation for PclsyncCompat).
    pub fn setup_with_backend(
        &mut self,
        password: SecretString,
        hint: Option<String>,
        backend: CryptoBackend,
    ) -> Result<(), CryptoError> {
        // audit-06 LOW crypto L-4 / pcloud-rs-ncx.79-g: if the shell already
        // holds a backend hint that disagrees with the caller's explicit
        // choice, warn at setup time. This is not an error (setup on a
        // not-yet-setup shell can still succeed), but it does flag
        // operator confusion before we bake the choice into the profile.
        if let Some(existing) = self.backend
            && existing != backend
        {
            log::warn!(
                target: "pcloud_crypto::setup",
                "crypto setup backend mismatch: existing={existing} requested={backend} (audit-06 LOW crypto L-4 / pcloud-rs-ncx.79-g)"
            );
        }
        match backend {
            CryptoBackend::Enhanced => self.setup_enhanced(password, hint),
            CryptoBackend::PclsyncCompat => {
                #[cfg(feature = "pclsync-v2")]
                {
                    self.setup_pclsync_compat(password, hint)
                }
                #[cfg(not(feature = "pclsync-v2"))]
                {
                    let _ = (password, hint);
                    Err(CryptoError::NotYetWired)
                }
            }
        }
    }

    /// Enhanced (Argon2id + AEAD) setup body. Formerly the whole of
    /// [`Self::setup`]. Unchanged logic.
    fn setup_enhanced(
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
        let normalized = normalize_password_nfc(&password);
        let derived = self.keys.derive_key_material(&normalized);
        self.keys.setup_fingerprint = Some(keys::KeyManager::fingerprint_for(&derived));
        drop(derived);
        self.hint = hint;
        self.unlock_state = state::UnlockState::Locked;
        self.backend = Some(CryptoBackend::Enhanced);
        Ok(())
    }

    /// PclsyncCompat setup body (Wave 2 Stage 3). Generates an RSA-4096
    /// keypair, wraps the priv DER under PBKDF2(password, salt, 20000)
    /// via AES-256-CTR (counter = 0 per C reference), and stores the
    /// resulting priv_key_ver1 / pub_key_ver1 blobs on the shell. The
    /// plaintext DER priv key is zeroised before return; only ciphertext
    /// escapes the function.
    ///
    /// The `CryptoShell` is a **data layer**: it does not upload the
    /// blobs to pCloud. The daemon's `crypto_setup` IPC handler is
    /// responsible for packaging them into `crypto_setuserkeys`
    /// (`C_CODE/pclsync/pcryptofolder.c:168` — `papi_send2(api,
    /// "crypto_setuserkeys", params)`). See `TODO(bd-1du.10)` in
    /// `crates/pcloud-daemon/src/runtime.rs`.
    #[cfg(feature = "pclsync-v2")]
    fn setup_pclsync_compat(
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
        let normalized = normalize_password_nfc(&password);
        let profile = pclsync_compat_profile::generate_profile(&normalized)
            .map_err(|_| CryptoError::PclsyncCompat)?;
        self.pclsync_compat = Some(profile);
        self.hint = hint;
        self.unlock_state = state::UnlockState::Locked;
        self.backend = Some(CryptoBackend::PclsyncCompat);
        // `setup_fingerprint` stays `None` for PclsyncCompat profiles:
        // the Enhanced fingerprint semantics do not apply. The PclsyncCompat
        // wrong-password gate lives in the profile's `pub_fingerprint`.
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
        self.start_inner(password, None)
    }

    /// Backend-pinned variant of [`Self::start`]: callers that already
    /// know which backend they expect (e.g. the daemon after loading
    /// persisted config) pass it explicitly. If the persisted profile was
    /// sealed under a different backend we refuse with
    /// [`CryptoError::BackendMismatch`] rather than silently dispatching
    /// to the wrong key schedule and corrupting ciphertext on later
    /// sector ops.
    ///
    /// # Errors
    /// - [`CryptoError::BackendMismatch`] when the on-disk profile does
    ///   not match `expected`.
    /// - Plus every error documented on [`Self::start`].
    pub fn start_with_backend(
        &mut self,
        password: SecretString,
        expected: CryptoBackend,
    ) -> Result<(), CryptoError> {
        self.start_inner(password, Some(expected))
    }

    fn start_inner(
        &mut self,
        password: SecretString,
        expected: Option<CryptoBackend>,
    ) -> Result<(), CryptoError> {
        // Preserve legacy pre-dispatch guards so callers that have never
        // run setup see `NotSetup` regardless of effective backend.
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if password.is_empty() {
            return Err(CryptoError::EmptyPassword);
        }
        if !self.is_setup() {
            return Err(CryptoError::NotSetup);
        }
        // audit-06 P1 (pcloud-rs-ncx.8): if the caller pinned a backend
        // via `start_with_backend`, the on-disk profile MUST match.
        // Dispatching a PclsyncCompat profile through the Enhanced key
        // schedule (or vice versa) would silently desync key derivation
        // from ciphertext and corrupt every subsequent sector op, so we
        // refuse here before any key material is derived.
        if let Some(expected) = expected {
            let effective = self.effective_backend();
            if effective != expected {
                return Err(CryptoError::BackendMismatch {
                    expected: effective,
                    provided: expected,
                });
            }
        }
        // Dispatch on the effective backend. Backend is inferred lazily from
        // the persisted profile; see `effective_backend` for the sentinel
        // rule. On success we write back `self.backend = Some(inferred)`
        // so the inference only runs once per migration.
        let effective = self.effective_backend();
        let migrate = self.backend.is_none();
        let res = match effective {
            CryptoBackend::Enhanced => self.start_enhanced(password),
            CryptoBackend::PclsyncCompat => {
                #[cfg(feature = "pclsync-v2")]
                {
                    self.start_pclsync_compat(password)
                }
                #[cfg(not(feature = "pclsync-v2"))]
                {
                    let _ = password;
                    Err(CryptoError::NotYetWired)
                }
            }
        };
        if res.is_ok() && migrate {
            // One-time migration: stamp the inferred backend so the
            // historical-profile sentinel never runs again.
            self.backend = Some(effective);
        }
        res
    }

    /// Enhanced (Argon2id) unlock body. Formerly the whole of `start`.
    fn start_enhanced(&mut self, password: SecretString) -> Result<(), CryptoError> {
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
        // `last_fail_at` timestamp (wall-clock, restart-persistent) AND a
        // monotonic `lockout_monotonic_floor` (process-local, clock-rewind-
        // resistant). Both must agree before unlock is attempted (H-5).
        let failures = self
            .consecutive_failures
            .load(std::sync::atomic::Ordering::SeqCst);
        if failures >= MAX_CONSECUTIVE_FAILURES {
            return Err(CryptoError::BruteForceLockedOut);
        }
        let backoff = lockout_backoff_secs(failures);
        if backoff > 0 {
            // Wall-clock check (persisted, handles cross-restart backoff).
            let last = self.last_fail_at.load(std::sync::atomic::Ordering::SeqCst);
            let now = unix_now_secs();
            if last > 0 && now.saturating_sub(last) < backoff {
                return Err(CryptoError::BruteForceLockedOut);
            }
            // Monotonic check (process-local, handles clock-rewind within
            // the same daemon session). `Instant::now()` is guaranteed not
            // to go backward within a process, so this cannot be bypassed
            // by clock manipulation without killing the daemon.
            if let Some(floor) = self.lockout_monotonic_floor {
                if std::time::Instant::now() < floor {
                    return Err(CryptoError::BruteForceLockedOut);
                }
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
            // Use SeqCst ordering so the pair of stores is totally ordered
            // with respect to any concurrent reader of these fields. A race
            // between fetch_add and store(last_fail_at) cannot cause the
            // lockout to become LESS strict (the worst case is that a read
            // races and sees incremented failures but a stale timestamp,
            // which triggers the backoff earlier — the correct direction for
            // security). SeqCst removes any store-reorder ambiguity within
            // this calling thread.
            let new_failures = self
                .consecutive_failures
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .saturating_add(1);
            self.last_fail_at
                .store(unix_now_secs(), std::sync::atomic::Ordering::SeqCst);
            // Set monotonic floor so clock rewind cannot bypass the backoff
            // within this daemon session (H-5 clock-rewind mitigation).
            let new_backoff = lockout_backoff_secs(new_failures);
            if new_backoff > 0 {
                self.lockout_monotonic_floor = Some(
                    std::time::Instant::now()
                        + std::time::Duration::from_secs(new_backoff),
                );
            }
            return Err(CryptoError::WrongPassword);
        }
        self.keys.active_key_material = Some(derived);
        self.unlock_state = state::UnlockState::Unlocked;
        // SeqCst: paired with the SeqCst writes in the failure branch so that
        // a successful unlock completely-before clears both counters for any
        // concurrent observer.
        self.consecutive_failures
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.last_fail_at
            .store(0, std::sync::atomic::Ordering::SeqCst);
        // Clear the monotonic lockout floor on successful unlock.
        self.lockout_monotonic_floor = None;
        Ok(())
    }

    /// PclsyncCompat unlock body (Wave 2 Stage 3). Requires the persisted
    /// `pclsync_compat` profile to be present. Rejects wrong passwords in
    /// constant time via the stored pub-key fingerprint **before** the
    /// RSA private key is parsed. On success populates
    /// `self.pclsync_compat_state` with the live priv key plus empty
    /// sym-key caches, to be filled lazily via `crypto_getfolderkey`.
    #[cfg(feature = "pclsync-v2")]
    fn start_pclsync_compat(&mut self, password: SecretString) -> Result<(), CryptoError> {
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if password.is_empty() {
            return Err(CryptoError::EmptyPassword);
        }
        let profile = match &self.pclsync_compat {
            Some(p) => p.clone(),
            None => return Err(CryptoError::NotSetup),
        };
        if self.pclsync_compat_state.is_some() {
            return Err(CryptoError::AlreadyStarted);
        }
        // Honor the same brute-force lockout as the Enhanced path, including
        // the monotonic floor guard (H-5 clock-rewind mitigation).
        let failures = self
            .consecutive_failures
            .load(std::sync::atomic::Ordering::SeqCst);
        if failures >= MAX_CONSECUTIVE_FAILURES {
            return Err(CryptoError::BruteForceLockedOut);
        }
        let backoff = lockout_backoff_secs(failures);
        if backoff > 0 {
            // Wall-clock check (cross-restart persistence).
            let last = self.last_fail_at.load(std::sync::atomic::Ordering::SeqCst);
            let now = unix_now_secs();
            if last > 0 && now.saturating_sub(last) < backoff {
                return Err(CryptoError::BruteForceLockedOut);
            }
            // Monotonic check (clock-rewind resistant, same session).
            if let Some(floor) = self.lockout_monotonic_floor {
                if std::time::Instant::now() < floor {
                    return Err(CryptoError::BruteForceLockedOut);
                }
            }
        }

        let normalized = normalize_password_nfc(&password);
        self.unlock_state = state::UnlockState::Unlocking;
        match pclsync_compat_profile::unlock_profile(&normalized, &profile) {
            Ok(state) => {
                self.pclsync_compat_state = Some(state);
                self.unlock_state = state::UnlockState::Unlocked;
                // SeqCst: consistent with the Enhanced path; clears lockout
                // counters in a totally-ordered fashion.
                self.consecutive_failures
                    .store(0, std::sync::atomic::Ordering::SeqCst);
                self.last_fail_at
                    .store(0, std::sync::atomic::Ordering::SeqCst);
                // Clear the monotonic lockout floor on successful unlock.
                self.lockout_monotonic_floor = None;
                Ok(())
            }
            Err(_) => {
                self.unlock_state = state::UnlockState::Locked;
                // SeqCst: see the Enhanced path for the ordering rationale.
                let new_failures = self
                    .consecutive_failures
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    .saturating_add(1);
                self.last_fail_at
                    .store(unix_now_secs(), std::sync::atomic::Ordering::SeqCst);
                // Set monotonic floor (H-5 clock-rewind mitigation).
                let new_backoff = lockout_backoff_secs(new_failures);
                if new_backoff > 0 {
                    self.lockout_monotonic_floor = Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_secs(new_backoff),
                    );
                }
                Err(CryptoError::WrongPassword)
            }
        }
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
        #[cfg(feature = "pclsync-v2")]
        {
            // Drops the live RSA private key + sym-key caches.
            self.pclsync_compat_state = None;
        }
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

        // audit-06 P1 (pcloud-rs-ncx.30): the per-session AES-256-GCM
        // nonce budget is scoped to the active master key. The rotation
        // just replaced that key, so the old nonce space is logically
        // a different random domain — we MUST reset the counter or the
        // rotated session will prematurely exhaust the budget based on
        // seals done under the previous key schedule.
        self.sectors_sealed
            .store(0, std::sync::atomic::Ordering::SeqCst);

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
        // Preserve historical guards before dispatch so existing tests that
        // expect `NotSetup`/`EmptyPassword`/`UnsafePolicy` on pre-flight
        // inputs keep working regardless of effective backend.
        if !self.policy.is_safe() {
            return Err(CryptoError::UnsafePolicy);
        }
        if old_password.is_empty() || new_password.is_empty() {
            return Err(CryptoError::EmptyPassword);
        }
        if !self.is_setup() {
            return Err(CryptoError::NotSetup);
        }
        if matches!(self.effective_backend(), CryptoBackend::PclsyncCompat) {
            #[cfg(feature = "pclsync-v2")]
            {
                // PclsyncCompat password-rotation: re-wrap priv_key_ver1
                // DER under a fresh salt and the new-password KEK, then
                // return a synthetic `ReencodedPrivateKey` carrying the
                // new blob (hex-encoded) and an HMAC-SHA-256 signature
                // over the blob under the *old* derived KEK — the
                // daemon posts `crypto_changeuserkeys` with these two
                // fields. The priv_key_ver1 blob on the shell is updated
                // in place so local state never lags behind the server
                // on the happy path. See
                // [`Self::change_password_pclsync_compat`] for the full
                // derivation; the returned `ChangePasswordResult` is
                // repackaged into a `ReencodedPrivateKey` for signature
                // compatibility with the Enhanced path — this keeps
                // existing SDK / daemon callers untouched. Stage 4b
                // wires the actual `crypto_changeuserkeys` RPC.
                return self.change_password_pclsync_compat_reencoded(
                    old_password,
                    new_password,
                    flags,
                );
            }
            #[cfg(not(feature = "pclsync-v2"))]
            {
                let _ = (old_password, new_password, flags);
                return Err(CryptoError::NotYetWired);
            }
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

    /// PclsyncCompat helper used by [`Self::change_password`]. Re-wraps
    /// the priv_key_ver1 DER under a fresh PBKDF2 salt and the new
    /// KEK, rotates `self.pclsync_compat`, and returns a synthetic
    /// `ReencodedPrivateKey` shaped for the existing daemon/SDK callers.
    ///
    /// Flow:
    ///   1. Unlock the current profile under `old_password` to recover
    ///      the live RSA private key (constant-time pub fingerprint
    ///      check — wrong password is rejected before DER parse).
    ///   2. Re-derive a fresh 64-byte salt, derive the new KEK.
    ///   3. Serialize the live priv DER, XOR-wrap it via AES-256-CTR
    ///      with counter=0 (matches the C client's setup path).
    ///   4. Rebuild the priv_key_ver1 blob.
    ///   5. Recompute the pub fingerprint under the new KEK.
    ///   6. Commit `pclsync_compat.priv_key_ver1_blob`,
    ///      `pclsync_compat.pub_fingerprint`, and `flags` atomically.
    ///   7. Sign the hex-encoded new blob under an HMAC-SHA-256 keyed
    ///      by the *old* KEK bytes so the server can verify the
    ///      rotation came from a holder of the pre-rotation KEK.
    #[cfg(feature = "pclsync-v2")]
    fn change_password_pclsync_compat_reencoded(
        &mut self,
        old_password: SecretString,
        new_password: SecretString,
        flags: u64,
    ) -> Result<ReencodedPrivateKey, CryptoError> {
        use pcloud_secret::ExposeSecret as _;
        use zeroize::Zeroize as _;
        // Reject identical passwords early (constant-time byte compare).
        {
            let eq: bool = old_password
                .expose_secret()
                .as_bytes()
                .ct_eq(new_password.expose_secret().as_bytes())
                .into();
            if eq {
                return Err(CryptoError::PasswordUnchanged);
            }
        }
        let new_password_norm = normalize_password_nfc(&new_password);
        let old_password_norm = normalize_password_nfc(&old_password);

        let profile = self
            .pclsync_compat
            .as_ref()
            .ok_or(CryptoError::NotSetup)?
            .clone();
        // Constant-time wrong-password reject via profile unlock.
        let state = pclsync_compat_profile::unlock_profile(&old_password_norm, &profile)
            .map_err(|_| CryptoError::WrongPassword)?;

        // Derive OLD KEK for signature key (safe: still authenticated).
        let (_typ_old, _flags_old, old_salt, _ct_old) =
            pclsync_compat_profile::PclsyncCompatProfile::parse_priv_blob(
                &profile.priv_key_ver1_blob,
            )
            .map_err(|_| CryptoError::PclsyncCompat)?;
        let old_kek = pclsync_kdf::derive_kek(&old_password_norm, &old_salt);

        // Fresh salt + new KEK.
        // SAFETY: see keys::KeyManager::default — getrandom is always
        // available on supported targets (Linux/macOS/Windows).
        let mut new_salt = [0u8; pclsync_compat_profile::PCLSYNC_PBKDF2_SALT_LEN];
        getrandom::getrandom(&mut new_salt)
            .expect("OS randomness for PclsyncCompat salt rotation");
        let new_kek = pclsync_kdf::derive_kek(&new_password_norm, &new_salt);

        // Serialize live priv key to DER, AES-256-CTR wrap it in place.
        let mut priv_der = pclsync_rsa::serialize_priv_key_der(state.priv_key())
            .map_err(|_| CryptoError::PclsyncCompat)?;
        pclsync_modes::aes256_ctr_pclsync_xor_inplace(
            &new_kek.key,
            &new_kek.iv,
            0,
            &mut priv_der,
        );

        let flags_u32 = u32::try_from(flags & u64::from(u32::MAX))
            .unwrap_or(0);
        let new_priv_blob = pclsync_compat_profile::PclsyncCompatProfile::build_priv_blob(
            flags_u32,
            &new_salt,
            &priv_der,
        );
        priv_der.zeroize();

        // New fingerprint is an HMAC over the (unchanged) pub blob under
        // the new KEK — reuse pub blob bytes verbatim.
        let mut new_fpr = [0u8; 32];
        {
            use hmac::{Hmac, Mac};
            // SAFETY: HMAC-SHA-256 accepts any non-zero key length; the
            // fixed 32-byte KEK value is never empty.
            let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(&new_kek.key)
                .expect("HMAC-SHA-256 accepts 32-byte key");
            mac.update(&profile.pub_key_ver1_blob);
            new_fpr.copy_from_slice(&mac.finalize().into_bytes());
        }

        // HMAC signature over the hex-encoded new blob under the OLD KEK.
        let new_priv_hex = hex_encode(&new_priv_blob);
        let signature = {
            use hmac::{Hmac, Mac};
            // SAFETY: HMAC-SHA-256 accepts any non-zero key length; the
            // fixed 32-byte KEK value is never empty.
            let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(&old_kek.key)
                .expect("HMAC-SHA-256 accepts 32-byte key");
            mac.update(new_priv_hex.as_bytes());
            let out = mac.finalize().into_bytes();
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&out);
            buf
        };

        // Commit new profile in place.
        if let Some(ref mut p) = self.pclsync_compat {
            p.priv_key_ver1_blob = new_priv_blob;
            p.pub_fingerprint = new_fpr;
            p.flags = flags_u32;
        }

        Ok(ReencodedPrivateKey {
            private_key_hex: new_priv_hex,
            signature_hex: hex_encode(&signature),
        })
    }

    /// PclsyncCompat + context-aware change-password. Returns the
    /// structured [`ChangePasswordResult`] instead of the Enhanced-style
    /// [`ReencodedPrivateKey`]. Stage 4b daemon callers will migrate
    /// to this once the `crypto_changeuserkeys` RPC is wired.
    #[cfg(feature = "pclsync-v2")]
    pub fn change_password_with_context(
        &mut self,
        old_password: SecretString,
        new_password: SecretString,
        flags: u32,
    ) -> Result<ChangePasswordResult, CryptoError> {
        // audit-06 P1-3 (Opus §3 C-1): cross-backend dispatch must surface
        // the actual backend mismatch instead of the generic `NotYetWired`.
        // This helper is PclsyncCompat-only; an Enhanced-sealed profile
        // landing here means the caller is using the wrong entry point.
        let effective = self.effective_backend();
        if !matches!(effective, CryptoBackend::PclsyncCompat) {
            return Err(CryptoError::BackendMismatch {
                expected: effective,
                provided: CryptoBackend::PclsyncCompat,
            });
        }
        let rekeyed = self.change_password_pclsync_compat_reencoded(
            old_password,
            new_password,
            u64::from(flags),
        )?;
        // At this point pclsync_compat has been rotated; pull the new blob out.
        let (blob, fpr, new_flags) = {
            let p = self
                .pclsync_compat
                .as_ref()
                .ok_or(CryptoError::PclsyncCompat)?;
            (
                p.priv_key_ver1_blob.clone(),
                p.pub_fingerprint,
                p.flags,
            )
        };
        let _ = rekeyed;
        Ok(ChangePasswordResult {
            new_priv_key_ver1_blob: zeroize::Zeroizing::new(blob),
            new_pub_fingerprint: fpr,
            flags: new_flags,
        })
    }

    /// PclsyncCompat mkdir body. Encodes the child filename under the
    /// parent folder's cached `SymKeyVer1`, generates a fresh child
    /// `SymKeyVer1`, records the local bookkeeping entry, and returns
    /// both to the caller. The daemon is responsible for RSA-OAEP
    /// wrapping the returned sym-key to the user's public key and
    /// uploading via `crypto_mkdir`, then re-caching it on the
    /// `PclsyncCompatState` under the server-assigned folder id.
    ///
    /// # Errors
    /// - [`CryptoError::Locked`] if PclsyncCompat runtime state is absent.
    /// - [`CryptoError::MissingFolderId`] if `parent_folder_id` is `None`.
    /// - [`CryptoError::FolderKeyNotCached`] if the parent's sym-key is
    ///   not populated in the cache yet.
    /// - [`CryptoError::InvalidName`] / [`CryptoError::FolderExists`]
    ///   on local bookkeeping failures.
    #[cfg(feature = "pclsync-v2")]
    fn mkdir_pclsync_compat(
        &mut self,
        parent_folder_id: Option<CryptoFolderId>,
        name: &str,
        local_folder_id: Option<CryptoFolderId>,
    ) -> Result<CreatedCryptoFolder, CryptoError> {
        // Lock gate first so pre-setup callers see the historical
        // `Locked` error shape (matches the Enhanced path and the
        // existing integration suite).
        if self.pclsync_compat_state.is_none() {
            return Err(CryptoError::Locked);
        }
        if name.is_empty() || name.contains('/') {
            return Err(CryptoError::InvalidName);
        }
        let parent_id = parent_folder_id.ok_or(CryptoError::MissingFolderId)?;
        // Encode the filename under the parent sym-key.
        let encoded_name = {
            let state = self
                .pclsync_compat_state
                .as_ref()
                .ok_or(CryptoError::Locked)?;
            let parent_sym = state
                .folder_key(parent_id)
                .ok_or(CryptoError::FolderKeyNotCached { folder_id: parent_id })?;
            // `SymKeyVer1::hmac_key` is exactly `PCLSYNC_HMAC_KEY_LEN`
            // bytes (= `pclsync_filename::HMAC_KEY_LEN`), so we can
            // pass it to `FilenameKeys` by reference directly.
            let keys = pclsync_filename::FilenameKeys {
                aes_key: &parent_sym.aes_key,
                hmac_key: &parent_sym.hmac_key,
            };
            pclsync_filename::encode_filename(keys, name)
                .map_err(|_| CryptoError::PclsyncCompat)?
        };

        // Generate a fresh SymKeyVer1 for the new folder (aes=32 B,
        // hmac=128 B; matches the C `sym_key_ver1` layout).
        let sym_key = {
            use rand_core::RngCore as _;
            let mut sym = pclsync_rsa::SymKeyVer1::new(0);
            rand_core::OsRng
                .try_fill_bytes(&mut sym.aes_key)
                .map_err(|_| CryptoError::PclsyncCompat)?;
            rand_core::OsRng
                .try_fill_bytes(&mut sym.hmac_key)
                .map_err(|_| CryptoError::PclsyncCompat)?;
            sym
        };

        // Allocate the local folder id.
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
            encrypted_name: encoded_name,
        };
        self.folders.insert(folder_id, entry.clone());
        Ok(CreatedCryptoFolder {
            entry,
            sym_key: Some(sym_key),
        })
    }

    /// PclsyncCompat + context-aware mkdir. Returns the richer
    /// [`CreatedCryptoFolder`] carrying the freshly generated
    /// `SymKeyVer1` that the daemon must RSA-OAEP-wrap and upload.
    ///
    /// Enhanced callers can also use this: they get
    /// `sym_key: None` and the same `entry` as [`Self::mkdir`].
    #[cfg(feature = "pclsync-v2")]
    pub fn mkdir_with_context(
        &mut self,
        parent_folder_id: Option<CryptoFolderId>,
        name: &str,
        local_folder_id: Option<CryptoFolderId>,
    ) -> Result<CreatedCryptoFolder, CryptoError> {
        if matches!(self.effective_backend(), CryptoBackend::PclsyncCompat) {
            return self.mkdir_pclsync_compat(parent_folder_id, name, local_folder_id);
        }
        let entry = self.mkdir(parent_folder_id, name, local_folder_id)?;
        Ok(CreatedCryptoFolder {
            entry,
            sym_key: None,
        })
    }

    // --------------------------------------------------------------------
    // Stage 4b.3 additive helpers — daemon-side dispatch plumbing.
    //
    // These helpers are the PclsyncCompat-only cache population glue that
    // the daemon's `CryptoGetFolderKey` / `CryptoGetFileKey` IPC dispatch
    // arms need. They are strictly additive:
    //
    //   - `cache_folder_key` / `cache_file_key` accept a plaintext
    //     `SymKeyVer1` (e.g. freshly generated on mkdir) and insert it
    //     into the cache.
    //   - `unwrap_and_cache_folder_key` / `unwrap_and_cache_file_key`
    //     accept an RSA-OAEP-wrapped blob fresh from the server, unwrap
    //     it using the unlocked private key, and insert the result.
    //
    // All helpers refuse with `CryptoError::Locked` when the PclsyncCompat
    // runtime state is not resident (i.e. the shell is not unlocked), and
    // with `CryptoError::BackendMismatch` when the shell is configured for
    // the Enhanced backend — these helpers have no meaning on that path.
    // --------------------------------------------------------------------

    /// Insert (or overwrite) a plaintext folder sym-key into the
    /// PclsyncCompat cache. Used after a local `mkdir_with_context` which
    /// already produced a fresh [`pclsync_rsa::SymKeyVer1`], so the caller
    /// does not need to round-trip it through the server.
    ///
    /// # Errors
    /// - [`CryptoError::BackendMismatch`] on Enhanced shells.
    /// - [`CryptoError::Locked`] if the PclsyncCompat runtime state is
    ///   absent (shell never unlocked in this process lifetime).
    #[cfg(feature = "pclsync-v2")]
    pub fn cache_folder_key(
        &mut self,
        folder_id: u64,
        sym: pclsync_rsa::SymKeyVer1,
    ) -> Result<(), CryptoError> {
        // audit-06 P1-3 (Opus §3 C-1): raise `BackendMismatch` so the
        // caller learns that the shell is sealed under a different backend
        // rather than swallowing the gap behind the generic `NotYetWired`.
        let effective = self.effective_backend();
        if !matches!(effective, CryptoBackend::PclsyncCompat) {
            return Err(CryptoError::BackendMismatch {
                expected: effective,
                provided: CryptoBackend::PclsyncCompat,
            });
        }
        let state = self
            .pclsync_compat_state
            .as_mut()
            .ok_or(CryptoError::Locked)?;
        state.cache_folder_key(folder_id, sym);
        Ok(())
    }

    /// Insert (or overwrite) a plaintext file sym-key into the
    /// PclsyncCompat cache.
    ///
    /// # Errors
    /// Same taxonomy as [`Self::cache_folder_key`].
    #[cfg(feature = "pclsync-v2")]
    pub fn cache_file_key(
        &mut self,
        file_id: u64,
        sym: pclsync_rsa::SymKeyVer1,
    ) -> Result<(), CryptoError> {
        // audit-06 P1-3 (Opus §3 C-1): surface `BackendMismatch` rather than
        // `NotYetWired` for Enhanced-sealed profiles so the caller sees the
        // real reason a PclsyncCompat-only helper is being refused.
        let effective = self.effective_backend();
        if !matches!(effective, CryptoBackend::PclsyncCompat) {
            return Err(CryptoError::BackendMismatch {
                expected: effective,
                provided: CryptoBackend::PclsyncCompat,
            });
        }
        let state = self
            .pclsync_compat_state
            .as_mut()
            .ok_or(CryptoError::Locked)?;
        state.cache_file_key(file_id, sym);
        Ok(())
    }

    /// RSA-OAEP-unwrap a server-returned wrapped sym-key blob and cache
    /// it as the folder's `SymKeyVer1`. Mirrors the C post-processing at
    /// `pcryptofolder.c:848-859` (`download_fldr_enckey`): decode the
    /// base64 `"key"` field, decrypt with the user's private key, parse
    /// the 168-byte `sym_key_ver1` structure, then commit to the cache.
    ///
    /// Base64 decoding is performed by the daemon before this call (the
    /// `crypto_getfolderkey` proto response already delivers raw bytes).
    ///
    /// # Errors
    /// - [`CryptoError::BackendMismatch`] on Enhanced shells.
    /// - [`CryptoError::Locked`] when the shell is not unlocked.
    /// - [`CryptoError::PclsyncCompat`] on RSA-OAEP or sym-key parse failure
    ///   (wire-level taxonomy is deliberately collapsed to an opaque
    ///   error variant so OAEP padding details do not leak to the user).
    #[cfg(feature = "pclsync-v2")]
    pub fn unwrap_and_cache_folder_key(
        &mut self,
        folder_id: u64,
        wrapped: &[u8],
    ) -> Result<(), CryptoError> {
        // audit-06 P1-3 (Opus §3 C-1): raise `BackendMismatch` on an
        // Enhanced shell so the daemon's OAEP-unwrap entry point cannot
        // silently fail with the generic `NotYetWired`.
        let effective = self.effective_backend();
        if !matches!(effective, CryptoBackend::PclsyncCompat) {
            return Err(CryptoError::BackendMismatch {
                expected: effective,
                provided: CryptoBackend::PclsyncCompat,
            });
        }
        let state = self
            .pclsync_compat_state
            .as_mut()
            .ok_or(CryptoError::Locked)?;
        let sym = pclsync_rsa::oaep_unwrap(state.priv_key(), wrapped)
            .map_err(|_| CryptoError::PclsyncCompat)?;
        state.cache_folder_key(folder_id, sym);
        Ok(())
    }

    /// RSA-OAEP-unwrap a server-returned wrapped file-key blob and cache
    /// it as the file's `SymKeyVer1`. Mirrors `download_file_enckey` at
    /// `pcryptofolder.c:890-909`.
    ///
    /// The `hash` argument is the server-reported file-version hash; it
    /// is recorded via `CryptoShell::cache_file_key` so subsequent
    /// seal/open ops can cross-check the file version. The current cache
    /// backing structure keys only by `file_id`; the hash is accepted
    /// here for API compatibility with the IPC surface and will be wired
    /// into cache invalidation in a follow-up (TODO(bd-1du.10)).
    ///
    /// # Errors
    /// Same taxonomy as [`Self::unwrap_and_cache_folder_key`].
    #[cfg(feature = "pclsync-v2")]
    pub fn unwrap_and_cache_file_key(
        &mut self,
        file_id: u64,
        hash: u64,
        wrapped: &[u8],
    ) -> Result<(), CryptoError> {
        // TODO(bd-1du.10): thread `hash` through the cache so stale
        // entries can be invalidated when the server bumps file version.
        let _ = hash;
        // audit-06 P1-3 (Opus §3 C-1): surface `BackendMismatch` instead
        // of `NotYetWired` so cross-backend dispatch is diagnosable.
        let effective = self.effective_backend();
        if !matches!(effective, CryptoBackend::PclsyncCompat) {
            return Err(CryptoError::BackendMismatch {
                expected: effective,
                provided: CryptoBackend::PclsyncCompat,
            });
        }
        let state = self
            .pclsync_compat_state
            .as_mut()
            .ok_or(CryptoError::Locked)?;
        let sym = pclsync_rsa::oaep_unwrap(state.priv_key(), wrapped)
            .map_err(|_| CryptoError::PclsyncCompat)?;
        state.cache_file_key(file_id, sym);
        Ok(())
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
        self.backend = None;
        #[cfg(feature = "pclsync-v2")]
        {
            self.pclsync_compat = None;
            self.pclsync_compat_state = None;
        }
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
        if matches!(self.effective_backend(), CryptoBackend::PclsyncCompat) {
            #[cfg(feature = "pclsync-v2")]
            {
                // PclsyncCompat mkdir: encode the child's filename under
                // the parent folder's cached `SymKeyVer1`. The parent id
                // comes in as `parent_folder_id` (`None` is treated as
                // `MissingFolderId` for the PclsyncCompat path — every
                // crypto folder lives under a parent folder keyed by a
                // server-assigned id). See
                // [`Self::mkdir_pclsync_compat`] for the full body.
                let created = self.mkdir_pclsync_compat(parent_folder_id, name, local_folder_id)?;
                return Ok(created.entry);
            }
            #[cfg(not(feature = "pclsync-v2"))]
            {
                return Err(CryptoError::NotYetWired);
            }
        }
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
        &mut self,
        file_seed: &[u8],
        sector_index: u32,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        // Backend dispatch. PclsyncCompat sector ops require folder/file
        // id + cached SymKeyVer1 plumbing (Stage 4: daemon-side
        // `crypto_getfolderkey` wiring + extended API that threads the
        // ids into the data layer). Until that lands, honestly refuse.
        if matches!(self.effective_backend(), CryptoBackend::PclsyncCompat) {
            // Lock gate first to preserve the historical `Locked` shape
            // for pre-setup callers.
            #[cfg(feature = "pclsync-v2")]
            if self.pclsync_compat_state.is_none() {
                return Err(CryptoError::Locked);
            }
            // PclsyncCompat seal path REQUIRES a `file_id` — the legacy
            // 3-arg signature (file_seed, sector_index, plaintext) does
            // not thread one through, so it refuses with
            // [`CryptoError::MissingFileId`]. Callers that need PclsyncCompat
            // sector ops must use [`Self::seal_sector_with_context`] and
            // supply a [`SectorContext::for_file`]. Stage 4b migrates
            // every IPC caller.
            let _ = (file_seed, sector_index, plaintext);
            return Err(CryptoError::MissingFileId);
        }
        // Enforce per-session AES-256-GCM 96-bit random-nonce budget
        // (H-2). Refuse new seals once the counter approaches
        // `u32::MAX - NONCE_BUDGET_SAFETY_MARGIN` — before birthday-bound
        // collision probability becomes non-negligible. The daemon must
        // rotate the per-file / master key and reset before proceeding.
        //
        // audit-06 P1 (pcloud-rs-ncx.19): reserve a budget slot via a
        // `compare_exchange_weak` loop BEFORE doing the crypto work.
        // The previous `load(Relaxed) + fetch_add(Relaxed)` pattern
        // permitted N concurrent threads to all pass the cap check and
        // then increment, overshooting the ceiling by N-1. The CAS
        // loop gives us a hard monotonic bound even under concurrent
        // seal calls — the fast-path is a single uncontended CAS so
        // the throughput cost is nil in the common case.
        let budget_cap = u64::from(u32::MAX) - NONCE_BUDGET_SAFETY_MARGIN;
        let reserved = loop {
            let cur = self
                .sectors_sealed
                .load(std::sync::atomic::Ordering::Acquire);
            if cur >= budget_cap {
                return Err(CryptoError::NonceBudgetExhausted);
            }
            match self.sectors_sealed.compare_exchange_weak(
                cur,
                cur + 1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => break cur + 1,
                Err(_) => continue,
            }
        };
        let _ = reserved;
        let file_key = self.derive_sector_file_key(file_seed)?;
        let frame = match content::seal_sector(
            &file_key,
            sector_index,
            plaintext,
            self.content.sector_size_bytes,
        ) {
            Ok(frame) => frame,
            Err(e) => {
                // Crypto work failed AFTER we reserved a budget slot.
                // Give the slot back so a transient error doesn't burn
                // nonce budget. `fetch_sub(Release)` pairs with the
                // next seal's `load(Acquire)` above.
                self.sectors_sealed
                    .fetch_sub(1, std::sync::atomic::Ordering::Release);
                return Err(e.into());
            }
        };
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
        &mut self,
        file_seed: &[u8],
    ) -> Result<pcloud_secret::secret_bytes::SecretBytes, CryptoError> {
        // Enforce TTL: evict the key if it has exceeded `cache_ttl_secs`
        // (lazy eviction — no background thread required). Returns
        // `CryptoError::MasterKeyExpired` if stale, `CryptoError::Locked`
        // if absent, and slides the TTL window forward on success.
        let master = self.require_active_key()?.clone_secret();
        match &self.mode {
            CryptoMode::Raw => Ok(content::derive_file_key(&master, file_seed)),
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
        &mut self,
        file_seed: &[u8],
        sector_index: u32,
        frame: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if matches!(self.effective_backend(), CryptoBackend::PclsyncCompat) {
            #[cfg(feature = "pclsync-v2")]
            if self.pclsync_compat_state.is_none() {
                return Err(CryptoError::Locked);
            }
            // See [`Self::seal_sector`] — the legacy 3-arg open path
            // refuses PclsyncCompat with [`CryptoError::MissingFileId`].
            // Use [`Self::open_sector_with_context`] with a PclsyncCompat
            // `SectorContext::for_file(file_id)` plus the detached auth tag.
            let _ = (file_seed, sector_index, frame);
            return Err(CryptoError::MissingFileId);
        }
        let file_key = self.derive_sector_file_key(file_seed)?;
        let pt = content::open_sector(&file_key, sector_index, frame)?;
        Ok(pt)
    }

    /// PclsyncCompat-aware sector seal.
    ///
    /// Enhanced call sites: pass [`SectorContext::enhanced()`] and keep
    /// using `file_seed` as before — the returned [`SealedSectorFrame`]
    /// has `auth_tag: None` and its `ciphertext` is the monolithic
    /// AES-GCM frame.
    ///
    /// PclsyncCompat call sites: pass [`SectorContext::for_file(file_id)`].
    /// The shell looks up `SymKeyVer1` for that `file_id` in the
    /// PclsyncCompat sym-key cache and invokes
    /// [`pclsync_sector::seal_sector`]. The returned
    /// [`SealedSectorFrame`] carries raw sector ciphertext plus the
    /// detached 32-byte `auth_tag` (which the caller must persist into
    /// the auth-sector Merkle tree alongside the ciphertext).
    ///
    /// # Errors
    /// - [`CryptoError::Locked`] if the shell is not started.
    /// - [`CryptoError::NonceBudgetExhausted`] (Enhanced only) once the
    ///   per-session 96-bit random-nonce budget is depleted.
    /// - [`CryptoError::MissingFileId`] (PclsyncCompat only) when the
    ///   caller passes `SectorContext::enhanced()`.
    /// - [`CryptoError::FileKeyNotCached`] (PclsyncCompat only) when
    ///   the sym-key cache has no entry for the requested `file_id`.
    /// - [`CryptoError::NotSetup`] / [`CryptoError::PclsyncCompat`]
    ///   if the PclsyncCompat runtime state is absent or malformed.
    pub fn seal_sector_with_context(
        &mut self,
        file_seed: &[u8],
        sector_index: u64,
        plaintext: &[u8],
        context: SectorContext,
    ) -> Result<SealedSectorFrame, CryptoError> {
        if matches!(self.effective_backend(), CryptoBackend::PclsyncCompat) {
            #[cfg(feature = "pclsync-v2")]
            {
                let file_id = context.file_id.ok_or(CryptoError::MissingFileId)?;
                let state = self
                    .pclsync_compat_state
                    .as_ref()
                    .ok_or(CryptoError::Locked)?;
                let sym = state
                    .file_key(file_id)
                    .ok_or(CryptoError::FileKeyNotCached { file_id })?;
                let keys = pclsync_sector::SectorKeys {
                    aes_key: &sym.aes_key,
                    hmac_key: &sym.hmac_key,
                };
                let sealed = pclsync_sector::seal_sector(keys, sector_index, plaintext)
                    .map_err(|e| match e {
                        pclsync_sector::SectorError::EmptySector => CryptoError::EmptySector,
                        _ => CryptoError::PclsyncCompat,
                    })?;
                return Ok(SealedSectorFrame {
                    ciphertext: sealed.ciphertext,
                    auth_tag: Some(sealed.auth_tag),
                });
            }
            #[cfg(not(feature = "pclsync-v2"))]
            {
                let _ = (file_seed, sector_index, plaintext, context);
                return Err(CryptoError::NotYetWired);
            }
        }
        // Enhanced path: ignore `context`, mimic legacy seal_sector.
        let _ = context;
        // Enhanced legacy sector-index is 32-bit; high u64 values would
        // silently truncate, so refuse them. Out-of-range sector indices
        // are protocol errors at the caller layer.
        let sector_index_u32: u32 = u32::try_from(sector_index)
            .map_err(|_| CryptoError::Content(content::ContentCryptoError::SectorIndexMismatch))?;
        let frame = self.seal_sector(file_seed, sector_index_u32, plaintext)?;
        Ok(SealedSectorFrame {
            ciphertext: frame,
            auth_tag: None,
        })
    }

    /// PclsyncCompat-aware sector open. Mirror of
    /// [`Self::seal_sector_with_context`].
    ///
    /// For Enhanced: `auth_tag` is ignored; `ciphertext` is the
    /// monolithic AES-GCM frame returned by
    /// [`Self::seal_sector_with_context`] / [`Self::seal_sector`].
    ///
    /// For PclsyncCompat: `auth_tag` MUST be the detached 32-byte tag
    /// produced by the corresponding seal call.
    ///
    /// # Errors
    /// Same taxonomy as [`Self::seal_sector_with_context`].
    pub fn open_sector_with_context(
        &mut self,
        file_seed: &[u8],
        sector_index: u64,
        ciphertext: &[u8],
        auth_tag: Option<&[u8; 32]>,
        context: SectorContext,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, CryptoError> {
        if matches!(self.effective_backend(), CryptoBackend::PclsyncCompat) {
            #[cfg(feature = "pclsync-v2")]
            {
                let file_id = context.file_id.ok_or(CryptoError::MissingFileId)?;
                let tag = auth_tag.ok_or(CryptoError::PclsyncCompat)?;
                let state = self
                    .pclsync_compat_state
                    .as_ref()
                    .ok_or(CryptoError::Locked)?;
                let sym = state
                    .file_key(file_id)
                    .ok_or(CryptoError::FileKeyNotCached { file_id })?;
                let keys = pclsync_sector::SectorKeys {
                    aes_key: &sym.aes_key,
                    hmac_key: &sym.hmac_key,
                };
                let pt = pclsync_sector::open_sector(keys, sector_index, ciphertext, tag)
                    .map_err(|_| CryptoError::PclsyncCompat)?;
                return Ok(pt);
            }
            #[cfg(not(feature = "pclsync-v2"))]
            {
                let _ = (file_seed, sector_index, ciphertext, auth_tag, context);
                return Err(CryptoError::NotYetWired);
            }
        }
        // Enhanced path.
        let _ = (auth_tag, context);
        // Enhanced legacy sector-index is 32-bit; high u64 values would
        // silently truncate, so refuse them. Out-of-range sector indices
        // are protocol errors at the caller layer.
        let sector_index_u32: u32 = u32::try_from(sector_index)
            .map_err(|_| CryptoError::Content(content::ContentCryptoError::SectorIndexMismatch))?;
        // audit-06 P1 (pcloud-rs-ncx.31): wrap Enhanced plaintext in
        // `Zeroizing` so the return type is uniform across backends
        // and the caller's plaintext zeroes on drop.
        self.open_sector(file_seed, sector_index_u32, ciphertext)
            .map(zeroize::Zeroizing::new)
    }
}

#[cfg(test)]
mod tests {
    use pcloud_secret::secret_string::SecretString;

    use super::{CryptoBackend, CryptoError, CryptoShell, state::UnlockState};

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

    // ---- Wave 2 / Stage 1: CryptoBackend enum + profile field ----------

    #[test]
    fn crypto_backend_default_is_pclsync_compat() {
        assert_eq!(CryptoBackend::default(), CryptoBackend::PclsyncCompat);
    }

    #[test]
    fn crypto_backend_display_kebab_case() {
        assert_eq!(CryptoBackend::PclsyncCompat.to_string(), "pclsync-compat");
        assert_eq!(CryptoBackend::Enhanced.to_string(), "enhanced");
    }

    #[test]
    fn crypto_backend_interop_flag() {
        assert!(CryptoBackend::PclsyncCompat.interoperable_with_pcloud_apps());
        assert!(!CryptoBackend::Enhanced.interoperable_with_pcloud_apps());
    }

    #[test]
    fn crypto_backend_serde_roundtrip_kebab_case() {
        let j = serde_json::to_string(&CryptoBackend::PclsyncCompat).unwrap();
        assert_eq!(j, "\"pclsync-compat\"");
        let back: CryptoBackend = serde_json::from_str(&j).unwrap();
        assert_eq!(back, CryptoBackend::PclsyncCompat);

        let j = serde_json::to_string(&CryptoBackend::Enhanced).unwrap();
        assert_eq!(j, "\"enhanced\"");
        let back: CryptoBackend = serde_json::from_str(&j).unwrap();
        assert_eq!(back, CryptoBackend::Enhanced);
    }

    #[test]
    fn profile_roundtrip_preserves_backend() {
        let mut c = CryptoShell::default();
        c.backend = Some(CryptoBackend::Enhanced);
        let json = serde_json::to_string(&c).unwrap();
        let back: CryptoShell = serde_json::from_str(&json).unwrap();
        assert_eq!(back.backend, Some(CryptoBackend::Enhanced));
        assert_eq!(back.effective_backend(), CryptoBackend::Enhanced);

        c.backend = Some(CryptoBackend::PclsyncCompat);
        let json = serde_json::to_string(&c).unwrap();
        let back: CryptoShell = serde_json::from_str(&json).unwrap();
        assert_eq!(back.backend, Some(CryptoBackend::PclsyncCompat));
        assert_eq!(back.effective_backend(), CryptoBackend::PclsyncCompat);
    }

    #[test]
    fn profile_without_backend_field_historical_enhanced_is_inferred() {
        // Simulate a historical Enhanced profile: serialize a current
        // shell that has completed setup() (so it has a
        // setup_fingerprint), then strip the `backend` field from the
        // JSON the way a pre-Wave-2 profile on disk looks.
        let mut c = CryptoShell::default();
        c.setup(pw("history"), None).unwrap();
        assert!(c.keys.setup_fingerprint.is_some());
        // Simulate "was never written with the field" by clearing it
        // before serializing (None serializes to `null` with default
        // serde; absence is handled by #[serde(default)]).
        c.backend = None;
        let json = serde_json::to_string(&c).unwrap();
        // Also test the truly-absent case: strip "backend":null entirely.
        let stripped = json.replace(",\"backend\":null", "");
        assert!(!stripped.contains("\"backend\""));
        let back: CryptoShell = serde_json::from_str(&stripped).unwrap();
        assert_eq!(back.backend, None);
        // Sentinel inference: historical fingerprint => Enhanced.
        assert_eq!(back.effective_backend(), CryptoBackend::Enhanced);
    }

    #[test]
    fn profile_without_backend_and_no_setup_defaults_to_pclsync_compat() {
        // Fresh shell, never set up: no historical ciphertext exists,
        // so the inference falls through to CryptoBackend::default().
        let c = CryptoShell::default();
        assert!(c.keys.setup_fingerprint.is_none());
        assert_eq!(c.backend, None);
        assert_eq!(c.effective_backend(), CryptoBackend::PclsyncCompat);
    }

    // --- Wave 2 Stage 2+3: PclsyncCompat dispatch tests ---

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_compat_setup_then_start_roundtrip() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pclsync-pw"), Some("my hint".into()), CryptoBackend::PclsyncCompat)
            .expect("setup pclsync-compat");
        assert!(c.is_setup());
        assert!(!c.is_started());
        assert_eq!(c.backend, Some(CryptoBackend::PclsyncCompat));
        assert!(c.pclsync_compat.is_some());
        assert_eq!(c.get_hint(), Some("my hint"));

        c.start(pw("pclsync-pw")).expect("start pclsync-compat");
        assert!(c.is_started());
        assert_eq!(c.unlock_state, UnlockState::Unlocked);
        assert!(c.pclsync_compat_state.is_some());

        c.stop();
        assert!(!c.is_started());
        assert!(c.is_setup()); // profile blob still on disk
        assert!(c.pclsync_compat_state.is_none());
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_compat_wrong_password_rejected() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("right"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        let err = c.start(pw("wrong")).expect_err("wrong pw");
        assert_eq!(err, CryptoError::WrongPassword);
        assert!(!c.is_started());
        assert_eq!(c.unlock_state, UnlockState::Locked);
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_compat_sector_ops_without_context_return_missing_file_id() {
        // Stage 4a: the legacy 3-arg seal_sector/open_sector signatures
        // now surface MissingFileId on the PclsyncCompat path — callers
        // must migrate to *_with_context. See also
        // pclsync_sector_roundtrip_via_shell for the happy-path proof.
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        let seed = [0u8; 32];
        assert_eq!(
            c.seal_sector(&seed, 0, b"hi").unwrap_err(),
            CryptoError::MissingFileId
        );
        assert_eq!(
            c.open_sector(&seed, 0, &[]).unwrap_err(),
            CryptoError::MissingFileId
        );
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_compat_mkdir_without_parent_returns_missing_folder_id() {
        // Stage 4a: mkdir on PclsyncCompat now requires a parent id
        // (the parent's SymKeyVer1 drives filename encoding).
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        assert_eq!(
            c.mkdir(None, "docs", None).unwrap_err(),
            CryptoError::MissingFolderId
        );
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_compat_change_password_rewraps_priv_key_ver1() {
        // Stage 4a: change_password now re-wraps priv_key_ver1 under a
        // fresh salt + new KEK. The returned ReencodedPrivateKey carries
        // the new blob (hex) + an HMAC signature keyed by the OLD KEK.
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("old-pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("old-pw")).expect("start");
        let old_blob = c
            .pclsync_compat
            .as_ref()
            .expect("profile present")
            .priv_key_ver1_blob
            .clone();
        let rekeyed = c
            .change_password(pw("old-pw"), pw("new-pw"), 0)
            .expect("change_password");
        assert!(!rekeyed.private_key_hex.is_empty());
        assert_eq!(rekeyed.signature_hex.len(), 64); // 32 bytes hex
        // Shell state has been rotated: new priv_key_ver1 blob differs.
        let new_blob = &c
            .pclsync_compat
            .as_ref()
            .expect("profile retained")
            .priv_key_ver1_blob;
        assert_ne!(&old_blob, new_blob);
        // Unlock with the new password succeeds; old is rejected.
        c.stop();
        c.start(pw("new-pw")).expect("unlock under new pw");
        c.stop();
        assert_eq!(
            c.start(pw("old-pw")).unwrap_err(),
            CryptoError::WrongPassword
        );
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_compat_reset_clears_profile_and_state() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        c.reset();
        assert!(!c.is_setup());
        assert!(!c.is_started());
        assert!(c.pclsync_compat.is_none());
        assert!(c.pclsync_compat_state.is_none());
        assert_eq!(c.backend, None);
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn setup_enhanced_stamps_backend_enhanced() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::Enhanced)
            .expect("setup");
        assert_eq!(c.backend, Some(CryptoBackend::Enhanced));
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn historical_enhanced_profile_start_migrates_backend() {
        // Simulate a historical Enhanced profile written before Wave 2:
        // backend = None, setup_fingerprint present.
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::Enhanced)
            .expect("setup");
        // Force backend=None to emulate pre-Stage-1 persistence.
        c.backend = None;
        assert_eq!(c.effective_backend(), CryptoBackend::Enhanced);
        c.start(pw("pw")).expect("start");
        // Migration should have stamped backend on success.
        assert_eq!(c.backend, Some(CryptoBackend::Enhanced));
    }

    // ------------------------------------------------------------------
    // Wave 2 / Stage 4a: widened PclsyncCompat sector/filename/mkdir API
    // ------------------------------------------------------------------

    /// Build a deterministic synthetic `SymKeyVer1` for tests — uses a
    /// fixed byte pattern for both AES and HMAC keys so expectations are
    /// reproducible. Not a secret; test-only.
    #[cfg(feature = "pclsync-v2")]
    fn synth_sym_key() -> super::pclsync_rsa::SymKeyVer1 {
        let mut s = super::pclsync_rsa::SymKeyVer1::new(0);
        s.aes_key.fill(0x11);
        s.hmac_key.fill(0x22);
        s
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_sector_roundtrip_via_shell() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        // Inject a synthetic file sym-key into the cache.
        c.pclsync_compat_state
            .as_mut()
            .expect("state")
            .cache_file_key(42, synth_sym_key());

        let plaintext = vec![0xABu8; 4096];
        let ctx = super::SectorContext::for_file(42);
        let sealed = c
            .seal_sector_with_context(&[], 7, &plaintext, ctx)
            .expect("seal");
        assert!(sealed.auth_tag.is_some());
        assert_eq!(sealed.ciphertext.len(), plaintext.len());

        let opened = c
            .open_sector_with_context(&[], 7, &sealed.ciphertext, sealed.auth_tag.as_ref(), ctx)
            .expect("open");
        assert_eq!(opened.as_slice(), plaintext.as_slice());
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_sector_missing_file_id_errors() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        let ctx = super::SectorContext::enhanced();
        assert_eq!(
            c.seal_sector_with_context(&[], 0, b"hi", ctx).unwrap_err(),
            CryptoError::MissingFileId
        );
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_sector_file_key_not_cached_errors() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        let ctx = super::SectorContext::for_file(999);
        match c.seal_sector_with_context(&[], 0, b"hi", ctx) {
            Err(CryptoError::FileKeyNotCached { file_id }) => assert_eq!(file_id, 999),
            other => panic!("expected FileKeyNotCached, got {other:?}"),
        }
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_filename_roundtrip_via_shell_mkdir() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        // Cache a parent folder key so filename encode can proceed.
        c.pclsync_compat_state
            .as_mut()
            .expect("state")
            .cache_folder_key(100, synth_sym_key());
        let created = c
            .mkdir_with_context(Some(100), "my-folder", None)
            .expect("mkdir");
        assert!(created.sym_key.is_some());
        assert!(!created.entry.encrypted_name.is_empty());
        // Encoded name round-trips through decode with the same parent
        // sym-key (proves we used pclsync_filename::encode_filename).
        let sym = synth_sym_key();
        let keys = super::pclsync_filename::FilenameKeys {
            aes_key: &sym.aes_key,
            hmac_key: &sym.hmac_key,
        };
        let decoded = super::pclsync_filename::decode_filename(keys, &created.entry.encrypted_name)
            .expect("decode");
        assert_eq!(decoded, "my-folder");
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_mkdir_folder_key_not_cached_errors() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        match c.mkdir_with_context(Some(7777), "x", None) {
            Err(CryptoError::FolderKeyNotCached { folder_id }) => assert_eq!(folder_id, 7777),
            other => panic!("expected FolderKeyNotCached, got {other:?}"),
        }
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_mkdir_enhanced_returns_none_sym_key() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("pw"), None, CryptoBackend::Enhanced)
            .expect("setup");
        c.start(pw("pw")).expect("start");
        let created = c
            .mkdir_with_context(None, "top", None)
            .expect("mkdir enhanced");
        assert!(created.sym_key.is_none());
        assert!(!created.entry.encrypted_name.is_empty());
    }

    #[cfg(feature = "pclsync-v2")]
    #[test]
    fn pclsync_change_password_with_context_produces_zeroizing_blob() {
        let mut c = CryptoShell::default();
        c.setup_with_backend(pw("old"), None, CryptoBackend::PclsyncCompat)
            .expect("setup");
        c.start(pw("old")).expect("start");
        let res = c
            .change_password_with_context(pw("old"), pw("new"), 0)
            .expect("change_password_with_context");
        assert!(!res.new_priv_key_ver1_blob.is_empty());
        assert_eq!(res.flags, 0);
        // New blob unlocks with new pw.
        c.stop();
        c.start(pw("new")).expect("unlock under new");
    }
}

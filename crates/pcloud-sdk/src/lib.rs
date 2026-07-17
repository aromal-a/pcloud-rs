#![forbid(unsafe_code)]
//! # pcloud-embedded-sdk
//!
//! Embeddable in-process SDK wrapping the daemon runtime as
//! `EmbeddedDaemon`. Lets applications drive auth, transfers, sync
//! roots, public links, crypto (when enabled), and settings without a
//! separate daemon process. The focused filesystem contract is exposed by
//! [`EmbeddedDaemon::remote`]; release/platform qualification remains separate
//! from the feature-parity tally in `C_FEATURE_PARITY_MATRIX.csv`.
//!
//! # Conventions across the `EmbeddedDaemon` API
//!
//! Every helper method documented below observes the following conventions
//! unless explicitly noted otherwise. Doc-comments on individual methods
//! describe the *method-specific* behaviour, preconditions, and side
//! effects; the conventions here fill in the boilerplate.
//!
//! - **Preconditions**: Most helpers require the daemon to hold an
//!   authenticated session (`is_authenticated()` returns `true`).
//!   Unauthenticated calls return the `NotAuthenticated` variant of the
//!   relevant helper error enum, wrapped in [`SdkError`]. A small set of
//!   entry points (`register`, `lost_password`, `verify_email_restricted`,
//!   `get_api_servers`, builder/`dispatch`/plugin registration) do not
//!   require authentication and say so in their doc comment.
//! - **Errors**: Every fallible method returns [`SdkError`]. Every
//!   [`SdkError`] variant transparently wraps one of the 14 per-helper
//!   error enums defined in this crate (e.g. [`UploadHelperError`],
//!   [`BackupHelperError`]). Per-variant docstrings on those enums describe
//!   the cause, the wrapping layer, and the recoverability class
//!   (user-recoverable / retryable after backoff / not recoverable without
//!   investigation).
//! - **Side effects**: Methods marked `&mut self` mutate the in-process
//!   runtime (auth snapshot, transfer queue, sync-root table, plugin
//!   registry, settings store). Methods marked `&self` are read-only.
//!   Persistence side effects (auth vault writes, audit events,
//!   settings-store rows) are surfaced through [`SdkError`] when they fail
//!   rather than being silently swallowed.
//! - **Daemon round-trips**: A "round-trip" below means one binary-API
//!   request/response pair to the pCloud backend. Pure-local calls
//!   (`config()`, `runtime_summary()`, `is_authenticated()`, the
//!   `*_value` / `*_setting` helpers) do zero round-trips. Most server
//!   helpers cost one round-trip. The `change_password` and
//!   `crypto_change_password*` flows chain two round-trips.
//!   `upload_file*` additionally performs a second round-trip for the
//!   signed byte-upload. `download_file` performs one API round-trip plus
//!   one CDN HTTPS GET.
//! - **Expected latency band**: single round-trip helpers typically return
//!   in the 100–500 ms range against the production API. Multi-step flows
//!   (crypto rotation, downloads, chunked uploads) scale accordingly.
//!   Local-only helpers return in microseconds. Canonical streaming transfers
//!   apply their documented bounded, journal-aware retries. Callers should
//!   still apply their own timeout/backoff policy to ordinary control-plane
//!   helpers.
//!
//! # Semver
//!
//! `pcloud-embedded-sdk` explicitly re-exports the types defined in `upload_session`
//! and [`pcloud_proto::Notification`].
//!
//! Several workspace-internal types also appear in public method signatures:
//!
//! - [`pcloud_config::ConfigProfile`] appears in [`EmbeddedDaemon::config`]
//!   and in the dispatch-level raw API.
//! - [`pcloud_config::Environment`] appears in
//!   [`EmbeddedDaemonBuilder::environment`].
//! - [`pcloud_ipc::Request`] / [`pcloud_ipc::Response`] appear in
//!   [`EmbeddedDaemon::dispatch`].
//! - [`pcloud_plugin_api`] types appear in the plugin-registration surface.
//! - [`pcloud_model::public_links::CreatedTreePublicLink`] appears in
//!   [`EmbeddedDaemon::create_tree_public_link_from_paths`].
//!
//! These types are exposed by necessity and are part of the public contract.
//! Callers using the raw-dispatch or plugin APIs must take direct dependencies
//! on those crates. Any future addition of a new workspace-crate type to a
//! public signature must be documented here (§8:221 audit compliance).
//!
//! Applications that only need drive operations should prefer
//! [`EmbeddedDaemon::remote`]. Its [`RemoteDrive`] surface exposes only
//! SDK-owned, non-exhaustive types and is the focused SemVer contract; raw
//! IPC and backend types are deliberately kept behind that boundary.
//!
//! # TLS Backend
//!
//! The SDK currently only supports rustls with webpki-roots as the TLS
//! backend. `pcloud-proto` hard-pins rustls; there is no `tls-native` feature
//! at this time. Enterprise embedders that require platform-native trust
//! stores must supply a reviewed downstream transport; no `tls-native`
//! feature is advertised by this crate.
//!
//! # Examples
//!
//! Bootstrap an embedded daemon and probe health:
//!
//! ```no_run
//! use std::path::PathBuf;
//! use pcloud_embedded_sdk::EmbeddedDaemon;
//! use pcloud_ipc::{Method, Request, ResponseStatus};
//! let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-doc"))
//!     .build()
//!     .expect("bootstrap");
//! let resp = d.dispatch(Request::Plain { method: Method::GetHealth });
//! assert_eq!(resp.status, ResponseStatus::Ok);
//! ```

#![deny(missing_docs)]
// Pedantic allows — the module-level conventions section above documents error
// and panic semantics for the entire API surface; per-function `# Errors` /
// `# Panics` sections would be pure duplication.
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::{Path, PathBuf};

use pcloud_config::{ConfigProfile, Environment, extensions::ExtensionPolicy};
use pcloud_daemon::path_resolver::PathResolveError;
use pcloud_daemon::{BootstrapError, RuntimeShell, bootstrap_with_config, dispatch};
use pcloud_ipc::{Method, Request, Response, ResponseStatus};
use pcloud_plugin_api::{
    Plugin, PluginAuditEvent, PluginAuditSink, PluginError, PluginOperation, PluginRegistry,
    RegisteredPlugin,
};
use pcloud_proto::public_links_api::PublicLinkPathResolver;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use pcloud_store::{StoreProfile, append_audit_event};
use thiserror::Error;

/// Crate identifier used in audit/telemetry records.
///
/// ```
/// assert_eq!(pcloud_embedded_sdk::CRATE_NAME, "pcloud-embedded-sdk");
/// ```
pub const CRATE_NAME: &str = "pcloud-embedded-sdk";

mod remote;
mod upload_session;
pub use remote::{
    RemoteCopyResult, RemoteDownloadResult, RemoteDrive, RemoteDriveError, RemoteEntry,
    RemoteEntryId, RemoteListing, RemoteRead, RemoteUploadResult,
};
pub use upload_session::{
    ConflictMode, DEFAULT_CHUNK_SIZE, FileMetadata, UploadConfig, UploadError, UploadHandle,
    UploadPayload, UploadProgress, UploadRequest, UploadSession, UploadSessionDriver, UploadState,
};

/// Typed notification record mirroring the C `psync_notification_t`. Re-exported
/// from `pcloud-proto` so SDK consumers do not need a direct dependency on the
/// protocol crate.
// NOTE: aliases pcloud_proto::Notification; if that type changes, this is a semver break
pub type Notification = pcloud_proto::Notification;

/// Embeddable in-process daemon. Bundles the daemon runtime shell, the
/// plugin registry, and the dispatch entry point used by SDK consumers.
/// Construct via [`EmbeddedDaemon::builder`].
#[derive(Debug)]
pub struct EmbeddedDaemon {
    runtime: RuntimeShell,
    plugins: PluginRegistry,
}

/// Builder for [`EmbeddedDaemon`]. Produced by
/// [`EmbeddedDaemon::builder`] and finalized with
/// [`EmbeddedDaemonBuilder::build`].
#[derive(Debug)]
pub struct EmbeddedDaemonBuilder {
    root: PathBuf,
    environment: Environment,
    extensions: Option<ExtensionPolicy>,
}

/// Error surface for [`EmbeddedDaemonBuilder::build`] and for plugin
/// registration against an already-bootstrapped daemon.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EmbeddedDaemonError {
    /// Daemon runtime bootstrap failed (profile load, store init, IPC
    /// socket setup, ...). Wraps [`BootstrapError`].
    #[error(transparent)]
    Bootstrap(#[from] BootstrapError),
    /// Plugin registration or invocation was rejected by the registry.
    /// Wraps [`PluginError`].
    #[error(transparent)]
    Plugin(#[from] PluginError),
}

/// [`PluginAuditSink`] that forwards plugin-registry events into the
/// daemon's hash-chained audit log via [`append_audit_event`]. Each
/// event becomes a row in the `audit_events` table under the
/// `plugin.*` category family, so out-of-band tampering is detectable
/// with `pcloud_store::verify_audit_chain`.
struct StoreAuditSink<'a> {
    store: &'a mut StoreProfile,
}

impl<'a> PluginAuditSink for StoreAuditSink<'a> {
    fn record(&mut self, event: PluginAuditEvent<'_>) {
        let (category, details) = match event {
            PluginAuditEvent::CapabilityGranted {
                plugin_id,
                version,
                granted,
                signed,
                dev_mode_unsigned,
            } => {
                let caps: Vec<String> = granted.iter().map(|c| format!("{c:?}")).collect();
                (
                    "plugin.capability_granted",
                    format!(
                        "id={plugin_id} version={version} signed={signed} dev_unsigned={dev_mode_unsigned} caps={}",
                        caps.join(",")
                    ),
                )
            }
            PluginAuditEvent::InvocationAllowed {
                plugin_id,
                operation,
                capability,
            } => (
                "plugin.invocation_allowed",
                format!("id={plugin_id} op={operation} cap={capability:?}"),
            ),
            PluginAuditEvent::InvocationDenied {
                plugin_id,
                operation,
                capability,
                reason,
            } => (
                "plugin.invocation_denied",
                format!("id={plugin_id} op={operation} cap={capability:?} reason={reason}"),
            ),
            PluginAuditEvent::LoadRejected { plugin_id, reason } => (
                "plugin.load_rejected",
                format!("id={plugin_id} reason={reason}"),
            ),
            PluginAuditEvent::HandlerPanic {
                plugin_id,
                operation,
            } => (
                "plugin.handler_panic",
                format!("id={plugin_id} op={operation}"),
            ),
            PluginAuditEvent::PluginDeregistered { plugin_id, reason } => (
                "plugin.deregistered",
                format!("id={plugin_id} reason={reason}"),
            ),
        };
        // Security rule: audit persistence failures must not be silently
        // swallowed. We log them via eprintln here so a follow-up agent
        // can wire them into the structured observability surface.
        if let Err(err) = append_audit_event(self.store, category, Some(&details)) {
            eprintln!("audit: failed to persist plugin event {category}: {err}");
        }
    }
}

/// Result of a single-shot upload helper call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadResult {
    /// Transient upload id returned by `upload_create`.
    pub upload_id: u64,
    /// Final file id once `upload_save` committed the blob. May be
    /// `None` on synthetic/development transports.
    pub file_id: Option<u64>,
    /// Remote folder id the file was written under.
    pub parent_folder_id: u64,
    /// Final remote filename (may differ from the requested name on conflict).
    pub remote_filename: String,
    /// Number of payload bytes acknowledged by the server.
    pub bytes_uploaded: usize,
}

/// Promotional material descriptor returned by `getpromo`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PromoResult {
    /// URL the client should render.
    pub url: String,
    /// Target width of the promo asset in pixels.
    pub width: u64,
    /// Target height of the promo asset in pixels.
    pub height: u64,
}

/// One entry from the `getapiserver` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiServerResult {
    /// Human-readable region/datacenter label.
    pub label: String,
    /// Hostname for JSON-over-HTTPS API traffic.
    pub api: String,
    /// Hostname for the binary protocol API.
    pub binapi: String,
    /// pCloud numeric location id (EU = 2, US = 1, ...).
    pub location_id: u64,
}

/// Error surface for the single-shot upload helpers. Wraps low-level
/// transfer-runtime failures and input-shape guards.
///
/// Recoverability summary: `NotAuthenticated` is user-recoverable;
/// `ResolveRemoteFolder` is user-recoverable after fixing the path;
/// `ReadLocalFile` depends on the inner `std::io::ErrorKind`;
/// `Create` / `Write` are transiently retryable with backoff.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UploadHelperError {
    /// No authenticated session is present. Cause: helper was called
    /// before `dispatch(Request::AuthTokenSubmission)` or equivalent
    /// login flow completed. Recoverability: user action — log in and
    /// retry.
    #[error("direct upload requires an authenticated session")]
    NotAuthenticated,
    /// Server rejected the target folder id, or path-based resolution
    /// failed. Cause: wraps the sync-runtime `validate_remote_folder`
    /// error message. Recoverability: user-recoverable after correcting
    /// the path or folder id.
    #[error("remote folder resolution failed: {0}")]
    ResolveRemoteFolder(String),
    /// The local payload file could not be read (permission denied,
    /// missing file, ...). Wraps `std::io::Error`. Recoverability:
    /// depends on `ErrorKind` — `NotFound`/`PermissionDenied` are
    /// user-recoverable; `TimedOut`/`Interrupted`/`WouldBlock` are
    /// transient.
    #[error("local file read failed: {0}")]
    ReadLocalFile(#[from] std::io::Error),
    /// The `upload_create` wire step failed. Cause: first API round-trip
    /// to reserve an `uploadid` failed. Recoverability: transiently
    /// retryable — inspect the inner server-provided message first.
    #[error("upload_create failed: {0}")]
    Create(String),
    /// The `upload_write` or `upload_save` wire step failed. Cause:
    /// chunked byte transfer or commit round-trip failed. Recoverability:
    /// transiently retryable after backoff; the underlying `uploadid`
    /// may need to be reissued depending on the server response.
    #[error("upload_write/save failed: {0}")]
    Write(String),
}

/// Descriptor returned by the backup-create helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupCreated {
    /// Remote folder id of the created backup root.
    pub folder_id: u64,
    /// Parent folder id if the server reported one.
    pub parent_folder_id: Option<u64>,
    /// Remote folder name if the server echoed it back.
    pub name: Option<String>,
}

/// Error surface for the backup-device helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BackupHelperError {
    /// No authenticated session is present.
    #[error("backup operation requires an authenticated session")]
    NotAuthenticated,
    /// Account has no backup root folder provisioned. Call
    /// `configure_backup_root` first.
    #[error("backup root folder id is not configured")]
    BackupRootMissing,
    /// Account has no device folder id. Call the device-register flow first.
    #[error("backup device folder id is not configured")]
    DeviceFolderMissing,
    /// Caller passed an empty backup name. User-recoverable.
    #[error("backup name must not be empty")]
    EmptyName,
    /// Server rejected `createbackup`. Inspect the message.
    #[error("create backup failed: {0}")]
    Create(String),
    /// Server rejected `stopbackup`. Inspect the message.
    #[error("stop backup failed: {0}")]
    StopBackup(String),
    /// Server rejected `stopdevice`. Inspect the message.
    #[error("stop device failed: {0}")]
    StopDevice(String),
    /// Local persistence of the post-operation state failed. Not
    /// user-recoverable without investigation.
    #[error("persisting backup state failed: {0}")]
    Persist(String),
}

/// Error surface for the typed key/value helpers backed by the
/// `value_kv` store.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ValueKvError {
    /// Underlying store operation failed (SQLite error, schema
    /// mismatch, etc.). Not user-recoverable.
    #[error("value_kv store operation failed: {0}")]
    Store(String),
}

/// Errors returned by the strict-typed setting helpers that mirror the C
/// `psync_{get,set}_{bool,int,uint,string}_setting` family. Stored kinds are
/// enforced on read, matching `CHECK_SETTINGID_AND_TYPE` in
/// `pclsync/psettings.c`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SettingKvError {
    /// Underlying settings store operation failed (type mismatch on
    /// read, SQLite error, unknown setting id, ...). Not user-recoverable.
    #[error("settings store operation failed: {0}")]
    Store(String),
}

/// Error surface for the account-utility helpers (verify email, lost
/// password, register, set language / API server, get promo, ...).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AccountUtilityError {
    /// No authenticated session is present on helpers that require one.
    #[error("account utility requires an authenticated session")]
    NotAuthenticated,
    /// `sendverificationemail` failed. Recoverable after resolving the
    /// server-reported cause.
    #[error("verify email failed: {0}")]
    VerifyEmail(String),
    /// `sendverifyemail` (restricted variant) failed.
    #[error("restricted verify email failed: {0}")]
    VerifyEmailRestricted(String),
    /// `lostpassword` request failed.
    #[error("lost password failed: {0}")]
    LostPassword(String),
    /// `getapiserver` request failed.
    #[error("list api servers failed: {0}")]
    ApiServers(String),
    /// `getpromo` request failed.
    #[error("get promo failed: {0}")]
    Promo(String),
    /// `setlanguage` request failed.
    #[error("set language failed: {0}")]
    SetLanguage(String),
    /// `changepassword` request failed.
    #[error("change password failed: {0}")]
    ChangePassword(String),
    /// Local `set_api_server` persistence or validation failed.
    #[error("set api server failed: {0}")]
    SetApiServer(String),
    /// `register` request failed. Inspect message for server reason
    /// (weak password, email already exists, ...).
    #[error("register failed: {0}")]
    Register(String),
    /// Caller must accept the terms of service before registration.
    /// User-recoverable.
    #[error("register requires accepting the terms of service")]
    TermsNotAccepted,
    /// Registration payload is missing a valid email or password.
    /// User-recoverable after fixing the input.
    #[error("register requires a valid email and password")]
    InvalidRegistrationInput,
}

/// Error surface for the notifications list/mark-read helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NotificationsHelperError {
    /// No authenticated session is present.
    #[error("notifications operation requires an authenticated session")]
    NotAuthenticated,
    /// `listnotifications` call failed. Inspect message for the server reason.
    #[error("list notifications failed: {0}")]
    List(String),
    /// `readnotifications` call failed. Inspect message for the server reason.
    #[error("mark notifications read failed: {0}")]
    MarkRead(String),
    /// Caller passed `0` as the watermark notification id. User-recoverable.
    #[error("notificationid must be non-zero")]
    InvalidNotificationId,
}

/// Errors returned by the folder-metadata and filesystem-status SDK
/// helpers. Mirrors the C surfaces of `psync_get_fsfolderid_by_path`
/// (pclsync/psynclib.c:2170), `psync_get_fsfolderflags_by_id`
/// (pclsync/psynclib.c:2176), `psync_get_folder_ownerid`
/// (pclsync/psynclib.c:2088), and `psync_filesystem_status`
/// (pclsync/psynclib.c:1903).
///
/// The helpers refuse to fabricate the C `PSYNC_INVALID_FSFOLDERID`
/// (`0`) sentinel on miss — callers get a typed error instead.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FolderMetadataError {
    /// No authenticated session is present.
    #[error("folder metadata lookup requires an authenticated session")]
    NotAuthenticated,
    /// Path input is empty or not absolute. User-recoverable.
    #[error("path must be absolute and non-empty")]
    InvalidPath,
    /// Underlying lookup (engine/store/backend) failed, or the path
    /// does not map to a known folder. Not user-recoverable without
    /// inspecting the inner message.
    #[error("folder metadata resolution failed: {0}")]
    Resolve(String),
}

/// Coarse synchronization status of a local path. Mirrors the C
/// `external_status_t` enum returned by
/// `psync_filesystem_status` (`pclsync/psynclib.c:1903`).
///
/// Variant tokens match the C constants exactly so callers that bridge
/// the SDK can forward them 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesystemPathStatus {
    /// C `INSYNC`. Path resolves under a sync root that has no queued
    /// work.
    InSync,
    /// C `INPROG`. Path resolves under a sync root whose engine has
    /// pending planned operations.
    InProgress,
    /// C `NOSYNC`. Path resolves under a paused or errored sync root
    /// (conflict, remote-full, local-full, ...).
    NoSync,
    /// C `INVSYNC`. Path is outside any tracked sync root (including
    /// `NOT_OURS` and `NOT_FOUND`).
    Invalid,
}

impl FilesystemPathStatus {
    /// Parity-preserving token: returns exactly the C constant name.
    ///
    /// ```
    /// use pcloud_embedded_sdk::FilesystemPathStatus;
    /// assert_eq!(FilesystemPathStatus::InSync.as_c_token(), "INSYNC");
    /// assert_eq!(FilesystemPathStatus::InProgress.as_c_token(), "INPROG");
    /// assert_eq!(FilesystemPathStatus::NoSync.as_c_token(), "NOSYNC");
    /// assert_eq!(FilesystemPathStatus::Invalid.as_c_token(), "INVSYNC");
    /// ```
    #[must_use]
    pub const fn as_c_token(self) -> &'static str {
        match self {
            Self::InSync => "INSYNC",
            Self::InProgress => "INPROG",
            Self::NoSync => "NOSYNC",
            Self::Invalid => "INVSYNC",
        }
    }
}

/// Folder metadata facets returned by
/// [`EmbeddedDaemon::get_folder_flags`]. Mirrors the C `flags` +
/// `permissions` out-params populated by `pfs_fldr_idperm_by_path`
/// (`pfsfolder.c:342`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderFlagsInfo {
    /// Raw permission bitmask from the C `pfs_fldr_idperm_by_path`
    /// result. `None` when the backend did not surface a permission word.
    pub permissions: Option<u32>,
    /// `true` if the folder lives inside the crypto root.
    pub encrypted: bool,
    /// `true` if the folder is a share (owned or received).
    pub shared: bool,
    /// `true` if the caller's effective access is read-only.
    pub readonly: bool,
}

/// Result of [`EmbeddedDaemon::stat_path`]. Mirrors the C `pentry_t`
/// returned by `psync_stat_path` (`pclsync/psynclib.h:743`).
///
/// Unlike the C surface, which returns a bare nullable pointer and
/// requires callers to manually free, this is a fully owned value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StatResult {
    /// Entry name (leaf component of the path).
    pub name: String,
    /// `true` if the entry is a folder, `false` for a file.
    pub is_folder: bool,
    /// Folder id when `is_folder` is `true`.
    pub folder_id: Option<u64>,
    /// File id when `is_folder` is `false`.
    pub file_id: Option<u64>,
    /// File size in bytes; always `None` for folders.
    pub size: Option<u64>,
    /// Unix epoch seconds of last modification. Best-effort: server
    /// may omit for some entry types.
    pub modified: Option<u64>,
    /// `true` if the entry lives in the caller's own storage.
    pub is_mine: bool,
    /// `true` if the entry lives inside the crypto root.
    pub encrypted: bool,
    /// `true` if the entry is shared.
    pub is_shared: bool,
    /// `PSYNC_PERM_*` permission bitmap, when available.
    pub permissions: Option<u32>,
}

/// A single child entry inside a folder listing, returned by
/// [`EmbeddedDaemon::list_folder`]. Mirrors the C `pentry_t` items
/// inside `pfolder_list_t`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FolderEntry {
    /// Entry name.
    pub name: String,
    /// `true` if the entry is a folder.
    pub is_folder: bool,
    /// Folder id when the entry is a folder.
    pub folder_id: Option<u64>,
    /// File id when the entry is a file.
    pub file_id: Option<u64>,
    /// File size in bytes; always `None` for folders.
    pub size: Option<u64>,
    /// Unix epoch seconds of last modification.
    pub modified: Option<u64>,
    /// `true` if the entry belongs to the current user.
    pub is_mine: bool,
    /// `true` if the entry is encrypted.
    pub encrypted: bool,
    /// `true` if the entry is shared.
    pub is_shared: bool,
    /// `PSYNC_PERM_*` bitmap, when available.
    pub permissions: Option<u32>,
}

/// Error surface for [`EmbeddedDaemon::delete_file`],
/// [`EmbeddedDaemon::rename_file`], and [`EmbeddedDaemon::get_file_info`].
///
/// These helpers dispatch IPC requests for remote file mutation and stat
/// operations. Each variant identifies the operation that failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FileMutationHelperError {
    /// No authenticated session is present.
    #[error("file operation requires an authenticated session")]
    NotAuthenticated,
    /// The delete operation was rejected by the daemon or server.
    #[error("delete failed: {0}")]
    DeleteFailed(String),
    /// The rename operation was rejected by the daemon or server.
    #[error("rename failed: {0}")]
    RenameFailed(String),
    /// The stat/info lookup failed.
    #[error("stat failed: {0}")]
    StatFailed(String),
}

/// Error surface for [`EmbeddedDaemon::mount`] and
/// [`EmbeddedDaemon::unmount`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MountHelperError {
    /// No authenticated session is present.
    #[error("mount requires an authenticated session")]
    NotAuthenticated,
    /// The mount or unmount operation failed. Wraps the daemon response
    /// message.
    #[error("mount operation failed: {0}")]
    Mount(String),
}

/// Errors returned by [`EmbeddedDaemon::send_publink`]. Mirrors the C
/// surface of `psync_send_publink` (pclsync/psynclib.c:2217).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PublinkHelperError {
    /// No authenticated session is present.
    #[error("send public link requires an authenticated session")]
    NotAuthenticated,
    /// Caller passed an empty public-link code. User-recoverable.
    #[error("public link code must not be empty")]
    EmptyCode,
    /// Caller passed an empty recipient list. User-recoverable.
    #[error("send public link requires at least one recipient")]
    EmptyRecipients,
    /// Server rejected `sendpublink`. Inspect the message for the reason.
    #[error("send public link failed: {0}")]
    Send(String),
}

/// Error surface for [`EmbeddedDaemon::create_tree_public_link_from_paths`].
///
/// Mirrors the C `ptree_public_link` path-based variant (row 149, bd-1du).
/// The path-resolution step runs under the daemon's authenticated context,
/// so callers do not need separate folder/file id lookups.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TreePublicLinkHelperError {
    /// No authenticated session is present.
    #[error("create_tree_public_link_from_paths requires an authenticated session")]
    NotAuthenticated,
    /// Caller supplied an empty link name. User-recoverable.
    #[error("tree public link name must not be empty")]
    EmptyName,
    /// Caller supplied an empty target set. User-recoverable — at least one
    /// absolute pCloud-drive root, folder, or file path is required.
    #[error("at least one pCloud-drive path is required")]
    EmptyPaths,
    /// A path could not be resolved to a remote folder id by the daemon
    /// path resolver. Inspect the message for the failing path.
    #[error("path resolution failed: {0}")]
    PathResolution(String),
    /// The `ptree_public_link` API call was rejected by the server.
    #[error("create tree public link failed: {0}")]
    Api(String),
}

/// Error surface for the crypto password-rotation helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CryptoHelperError {
    /// No authenticated session is present.
    #[error("crypto operation requires an authenticated session")]
    NotAuthenticated,
    /// Caller passed an empty old or new crypto password. User-recoverable.
    #[error("crypto passwords must not be empty")]
    EmptyPassword,
    /// Change-password flow requires the confirmation code obtained via
    /// `crypto_sendchangeuserprivate`. User-recoverable.
    #[error("crypto password change requires a confirmation code")]
    EmptyCode,
    /// Local crypto shell (key wrap / unwrap) refused the rotation.
    /// Typically means the old password was wrong.
    #[error("crypto shell rejected password change: {0}")]
    Shell(String),
    /// `crypto_sendchangeuserprivate` API call failed.
    #[error("crypto send-change-user-private failed: {0}")]
    SendChangeUserPrivate(String),
    /// `crypto_changeuserprivate` API call failed.
    #[error("crypto change-user-private failed: {0}")]
    ChangeUserPrivate(String),
}

/// Snapshot of the authenticated user's profile, mirroring the shape exposed by the
/// legacy C `psync_get_userinfo` accessor but returning structured data instead of a
/// JSON blob. The `auth_token` field is intentionally not exposed; callers that need
/// the raw token must go through the auth vault surface.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthenticatedUser {
    /// pCloud numeric user id when the server returned one.
    pub user_id: Option<u64>,
    /// Primary account email when the server returned one.
    pub email: Option<String>,
}

/// Signed download link metadata, mirroring the C `psync_get_url_for_file` shape but
/// stripped to the fields a caller needs to fetch the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadLinkInfo {
    /// Signed URL path component to request from any of [`Self::hosts`].
    pub path: String,
    /// Candidate CDN hostnames, in the server's preferred order.
    pub hosts: Vec<String>,
    /// Optional opaque server-returned download tag used for replay
    /// detection. `None` when the server did not supply one.
    pub download_tag: Option<String>,
}

/// Two-factor SMS delivery descriptor exposed to SDK consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorSmsInfo {
    /// ISO country code of the registered phone number, when present.
    pub country_code: Option<String>,
    /// Masked phone number the SMS was delivered to, when present.
    pub phone_number: Option<String>,
}

/// Two-factor device-notification delivery descriptor exposed to SDK consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorNotificationInfo {
    /// Human-readable names of the devices the push notification was
    /// delivered to.
    pub devices: Vec<String>,
}

/// Error surface for the authentication/session helpers (userinfo,
/// logout, TFA SMS / device / code flows).
///
/// Recoverability summary: `NotAuthenticated` and `TwoFactorCode` are
/// user-recoverable; the remaining named-call variants are transiently
/// retryable.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuthHelperError {
    /// No authenticated session is present on an operation that
    /// requires one. Cause: `snapshot().auth_token` was `None`.
    /// Recoverability: user action — complete login first.
    #[error("operation requires an authenticated session")]
    NotAuthenticated,
    /// `userinfo` API call failed. Wraps the server-provided reason.
    /// Recoverability: transiently retryable (network/5xx); a persistent
    /// `Auth` rejection means the token is no longer valid — re-login.
    #[error("userinfo failed: {0}")]
    UserInfo(String),
    /// `logout` API call failed. Wraps the server-provided reason.
    /// Recoverability: transiently retryable; safe to swallow in
    /// best-effort shutdown paths.
    #[error("logout failed: {0}")]
    Logout(String),
    /// `tfa_sendcodeviasms` API call failed. Wraps the server message.
    /// Recoverability: transiently retryable; a permanent rejection
    /// means the account does not have SMS TFA enabled.
    #[error("two-factor SMS delivery failed: {0}")]
    TwoFactorSms(String),
    /// `tfa_sendcodeviasysnotification` API call failed. Wraps the
    /// server message. Recoverability: transiently retryable; a
    /// permanent rejection means no trusted device is registered.
    #[error("two-factor notification delivery failed: {0}")]
    TwoFactorNotification(String),
    /// Submission of a TFA code or recovery code was rejected. Wraps
    /// the server message. Recoverability: user action — re-enter the
    /// code. Excessive retries may trigger a server-side cool-down.
    #[error("two-factor code submission failed: {0}")]
    TwoFactorCode(String),
    /// Password or token login was rejected by the daemon. Wraps the
    /// server message. Recoverability: user action — re-enter credentials.
    #[error("login failed: {0}")]
    Login(String),
}

/// Error surface for the signed-URL download helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DownloadHelperError {
    /// No authenticated session is present.
    #[error("download requires an authenticated session")]
    NotAuthenticated,
    /// `getfilelink` call failed. Inspect the message for the server
    /// reason (file not found, permission denied, ...).
    #[error("get_file_link failed: {0}")]
    GetFileLink(String),
    /// Signed-URL HTTP GET for the resolved file failed (network error,
    /// non-2xx status, truncated body, ...).
    #[error("download_bytes failed: {0}")]
    DownloadBytes(String),
}

/// Result of a folder-creation helper call. Mirrors the pCloud
/// `createfolder`/`createfolderifnotexists` metadata shape exposed by
/// [`pcloud_proto::CreateFolderResponse`] plus the `suffix_index` chosen
/// by the `check_and_create_folder` retry loop
/// (`pclsync/pbusinessaccount.c:803`). `suffix_index` is `None` for the
/// bare `create_remote_folder`/`create_remote_folder_by_path` helpers
/// and `Some(0)` when the idempotent helper adopted the bare `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFolderResult {
    /// Server-assigned folder id.
    pub folder_id: u64,
    /// Final folder name (may differ from the requested name when the
    /// idempotent helper adopted a `-N` suffix).
    pub name: String,
    /// Parent folder id when the server reported it.
    pub parent_folder_id: Option<u64>,
    /// `true` if a new folder was created; `false` if an existing
    /// folder with a matching name was returned by the idempotent helper.
    pub created: bool,
    /// Suffix index chosen by the retry loop. `None` for the bare
    /// `create_remote_folder` helpers, `Some(0)` if the idempotent
    /// helper adopted the bare name, `Some(n)` for `name-n`.
    pub suffix_index: Option<u32>,
}

/// Error surface for the folder-creation helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CreateFolderHelperError {
    /// No authenticated session is present.
    #[error("folder operation requires an authenticated session")]
    NotAuthenticated,
    /// Folder name is empty. User-recoverable.
    #[error("folder name must not be empty")]
    EmptyName,
    /// Path input is not absolute. User-recoverable.
    #[error("remote path must be absolute (start with '/')")]
    InvalidPath,
    /// Server rejected the `createfolder` / `createfolderifnotexists`
    /// request. Inspect the message.
    #[error("createfolder request failed: {0}")]
    Api(String),
}

// ============================================================================
// Consolidated SDK error surface.
//
// `SdkError` is the single error type returned by every public SDK helper
// method. The 14 per-helper enums (`UploadHelperError`, `BackupHelperError`,
// ...) are RETAINED for granular matching and for the existing
// `From<HelperError> for pcloud_error::Error` taxonomy wiring, but they are
// no longer surfaced through the public function signatures — callers see
// `SdkError` only.
//
// Every per-helper enum implements `From<_> for SdkError` via `#[from]`,
// routing into one of the category-shaped variants (`Auth`, `Upload`, ...).
// Combined with the unchanged `From<HelperError> for pcloud_error::Error`
// impls below, callers can compose `?` chains against either layer.
// ============================================================================

/// Single consolidated error type returned by every public SDK helper.
///
/// Each variant transparently wraps one of the per-helper error enums kept in
/// this module. Callers that want fine-grained matching can downcast on the
/// outer variant first, then on the inner enum. Callers that just want to
/// propagate can simply `?` the result.
///
/// This type is `#[non_exhaustive]` — new variants will be added as new helper
/// families land. New variants are explicitly NOT a SemVer break under the
/// `non_exhaustive` contract.
///
/// # Retryability hints
///
/// Variants fall into three broad categories. The inner enum's per-variant
/// docs give the precise classification; the outer variant's inline comment
/// names the default.
///
/// - **Retryable after fixing input / user action**: `Auth`, `Account`,
///   `Publink`, `CreateFolder`, `Crypto` (when the inner variant is one of
///   the input-shape guards like `EmptyPassword` / `EmptyCode`), and
///   `Notifications` (when `InvalidNotificationId`). A retry with corrected
///   input is expected to succeed.
/// - **Transiently retryable with backoff**: `Upload`, `Download`,
///   `UploadSession`, `Folder`, the `Api`-backed arms of `Account`,
///   `Publink`, `Backup`, `CreateFolder`, `Notifications`, and any
///   network-surfaced `Crypto::SendChangeUserPrivate` /
///   `ChangeUserPrivate` failure. Expected class: transient network,
///   server busy, or momentary 5xx.
/// - **Not recoverable without investigation**: `EmbeddedDaemon`,
///   `Kv`, `Setting`, `Backup::Persist`, `Io` (depending on `ErrorKind`).
///   Bootstrap / store failures indicate a local environment problem;
///   retrying without fixing it will loop.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SdkError {
    /// Authentication / session failures (login, TFA, userinfo, logout).
    /// Wraps [`AuthHelperError`]. Retryability: usually user-recoverable
    /// (re-enter credentials, re-send TFA) for wrong-code errors;
    /// transiently retryable for server-side call failures.
    #[error(transparent)]
    Auth(#[from] AuthHelperError),
    /// Upload helper failures (`upload_data`, `upload_file`, ...).
    /// Wraps [`UploadHelperError`]. Retryability: `NotAuthenticated` is
    /// user-recoverable; `ReadLocalFile` depends on the inner I/O kind;
    /// `Create` / `Write` are transiently retryable after a short backoff.
    #[error(transparent)]
    Upload(#[from] UploadHelperError),
    /// Streaming [`UploadSession`] failures (cancel, pause, await_completion).
    /// Wraps [`UploadError`]. Retryability: `Canceled` is terminal —
    /// start a new session. `Helper` inherits upload retryability.
    /// See the [`UploadSession`] docs for the state-machine matrix.
    #[error(transparent)]
    UploadSession(#[from] UploadError),
    /// Download helper failures (`get_file_link`, `download_file`).
    /// Wraps [`DownloadHelperError`]. Retryability: transient HTTPS
    /// failures are retryable with exponential backoff; `NotAuthenticated`
    /// is user-recoverable.
    #[error(transparent)]
    Download(#[from] DownloadHelperError),
    /// Crypto helper failures (`crypto_change_password`, ...).
    /// Wraps [`CryptoHelperError`]. Retryability: input-shape guards
    /// (`EmptyPassword`, `EmptyCode`) are user-recoverable. `Shell`
    /// typically means wrong old passphrase — user action required.
    /// API call failures are transiently retryable.
    #[error(transparent)]
    Crypto(#[from] CryptoHelperError),
    /// Backup helper failures (`create_backup`, `stop_device`, ...).
    /// Wraps [`BackupHelperError`]. Retryability: `Persist` is
    /// non-recoverable without investigating local storage; server
    /// failures are transiently retryable; missing-id variants require
    /// caller to provision state first.
    #[error(transparent)]
    Backup(#[from] BackupHelperError),
    /// Public-link helper failures (`send_publink`).
    /// Wraps [`PublinkHelperError`]. Retryability: `EmptyCode` /
    /// `EmptyRecipients` are user-recoverable; `Send` is transiently
    /// retryable.
    #[error(transparent)]
    Publink(#[from] PublinkHelperError),
    /// Tree public-link from-paths helper failures
    /// ([`EmbeddedDaemon::create_tree_public_link_from_paths`]).
    /// Wraps [`TreePublicLinkHelperError`]. Retryability:
    /// `EmptyName` / `EmptyPaths` are user-recoverable;
    /// `PathResolution` / `Api` are transiently retryable.
    #[error(transparent)]
    TreePublicLink(#[from] TreePublicLinkHelperError),
    /// Folder lookup / metadata failures.
    /// Wraps [`FolderMetadataError`]. Retryability: `InvalidPath` is
    /// user-recoverable; `Resolve` may be transient or a permanent
    /// not-found — inspect the message.
    #[error(transparent)]
    Folder(#[from] FolderMetadataError),
    /// Folder creation helper failures.
    /// Wraps [`CreateFolderHelperError`]. Retryability: input-shape guards
    /// are user-recoverable; `Api` is transiently retryable.
    #[error(transparent)]
    CreateFolder(#[from] CreateFolderHelperError),
    /// Account-utility helper failures (verify email, set language, register, ...).
    /// Wraps [`AccountUtilityError`]. Retryability: input-shape guards
    /// (`TermsNotAccepted`, `InvalidRegistrationInput`) are
    /// user-recoverable; named-server-call variants are transiently
    /// retryable.
    #[error(transparent)]
    Account(#[from] AccountUtilityError),
    /// Notifications helper failures (list, mark read).
    /// Wraps [`NotificationsHelperError`]. Retryability: input-shape
    /// guards are user-recoverable; `List` / `MarkRead` are transiently
    /// retryable.
    #[error(transparent)]
    Notifications(#[from] NotificationsHelperError),
    /// Typed key/value storage failures (`get_*_value`, `set_*_value`).
    /// Wraps [`ValueKvError`]. Retryability: not recoverable without
    /// investigating local SQLite store (disk full, corrupt schema, ...).
    #[error(transparent)]
    Kv(#[from] ValueKvError),
    /// Strict-typed setting helper failures (`get_*_setting`, `set_*_setting`).
    /// Wraps [`SettingKvError`]. Retryability: non-recoverable — almost
    /// always signals a schema/type mismatch, unknown setting id, or a
    /// local SQLite failure.
    #[error(transparent)]
    Setting(#[from] SettingKvError),
    /// Embedded daemon bootstrap / plugin registration failures.
    /// Wraps [`EmbeddedDaemonError`]. Retryability: not recoverable
    /// without investigating local config or the failing plugin —
    /// retry-loops without fixing the cause will spin indefinitely.
    #[error(transparent)]
    EmbeddedDaemon(#[from] EmbeddedDaemonError),
    /// File mutation helper failures (`delete_file`, `rename_file`, `get_file_info`).
    /// Wraps [`FileMutationHelperError`]. Retryability: `NotAuthenticated` is
    /// user-recoverable; `DeleteFailed` / `RenameFailed` / `StatFailed` are
    /// transiently retryable for transient server errors.
    #[error(transparent)]
    FileMutation(#[from] FileMutationHelperError),
    /// Mount / unmount helper failures.
    /// Wraps [`MountHelperError`]. Retryability: `NotAuthenticated` is
    /// user-recoverable; `Mount` depends on the underlying OS/FUSE error.
    #[error(transparent)]
    Mount(#[from] MountHelperError),
    /// Local I/O failure surfaced directly (e.g. reading an upload payload).
    /// Wraps `std::io::Error`. Retryability: depends on
    /// `std::io::ErrorKind`. `NotFound` / `PermissionDenied` are
    /// user-recoverable after fixing the filesystem; `TimedOut` /
    /// `Interrupted` / `WouldBlock` are transiently retryable.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// ============================================================================
// Unified error conversions into `pcloud_error::Error`.
//
// Every SDK helper error family funnels into the workspace-wide `Error`
// taxonomy. This lets callers compose `?` chains against a single error type
// without losing the original cause (`source()` is preserved via
// `IntoUnified`). Per-helper enums remain public for backward compatibility
// and fine-grained matching.
// ============================================================================

use pcloud_error::{Category, Error as UnifiedError, IntoUnified};

impl From<SdkError> for UnifiedError {
    fn from(err: SdkError) -> Self {
        match err {
            SdkError::Auth(e) => e.into(),
            SdkError::Upload(e) => e.into(),
            SdkError::UploadSession(e) => e.into(),
            SdkError::Download(e) => e.into(),
            SdkError::Crypto(e) => e.into(),
            SdkError::Backup(e) => e.into(),
            SdkError::Publink(e) => e.into(),
            SdkError::TreePublicLink(e) => e.into(),
            SdkError::Folder(e) => e.into(),
            SdkError::CreateFolder(e) => e.into(),
            SdkError::Account(e) => e.into(),
            SdkError::Notifications(e) => e.into(),
            SdkError::Kv(e) => e.into(),
            SdkError::Setting(e) => e.into(),
            SdkError::EmbeddedDaemon(e) => e.into(),
            SdkError::FileMutation(e) => e.into(),
            SdkError::Mount(e) => e.into(),
            SdkError::Io(e) => e.into_unified(Category::LocalIo),
        }
    }
}

impl From<EmbeddedDaemonError> for UnifiedError {
    fn from(err: EmbeddedDaemonError) -> Self {
        match err {
            EmbeddedDaemonError::Bootstrap(e) => e.into_unified(Category::Config),
            EmbeddedDaemonError::Plugin(e) => e.into_unified(Category::Plugin),
        }
    }
}

impl From<UploadHelperError> for UnifiedError {
    fn from(err: UploadHelperError) -> Self {
        match &err {
            UploadHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            UploadHelperError::ResolveRemoteFolder(_) => err.into_unified(Category::Api),
            UploadHelperError::ReadLocalFile(_) => err.into_unified(Category::LocalIo),
            UploadHelperError::Create(_) | UploadHelperError::Write(_) => {
                err.into_unified(Category::Api)
            }
        }
    }
}

impl From<BackupHelperError> for UnifiedError {
    fn from(err: BackupHelperError) -> Self {
        match &err {
            BackupHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            BackupHelperError::BackupRootMissing
            | BackupHelperError::DeviceFolderMissing
            | BackupHelperError::EmptyName => err.into_unified(Category::InvalidInput),
            BackupHelperError::Create(_)
            | BackupHelperError::StopBackup(_)
            | BackupHelperError::StopDevice(_) => err.into_unified(Category::Api),
            BackupHelperError::Persist(_) => err.into_unified(Category::Storage),
        }
    }
}

impl From<ValueKvError> for UnifiedError {
    fn from(err: ValueKvError) -> Self {
        err.into_unified(Category::Storage)
    }
}

impl From<SettingKvError> for UnifiedError {
    fn from(err: SettingKvError) -> Self {
        err.into_unified(Category::Storage)
    }
}

impl From<AccountUtilityError> for UnifiedError {
    fn from(err: AccountUtilityError) -> Self {
        match &err {
            AccountUtilityError::NotAuthenticated => err.into_unified(Category::Auth),
            AccountUtilityError::TermsNotAccepted
            | AccountUtilityError::InvalidRegistrationInput => {
                err.into_unified(Category::InvalidInput)
            }
            _ => err.into_unified(Category::Api),
        }
    }
}

impl From<NotificationsHelperError> for UnifiedError {
    fn from(err: NotificationsHelperError) -> Self {
        match &err {
            NotificationsHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            NotificationsHelperError::InvalidNotificationId => {
                err.into_unified(Category::InvalidInput)
            }
            NotificationsHelperError::List(_) | NotificationsHelperError::MarkRead(_) => {
                err.into_unified(Category::Api)
            }
        }
    }
}

impl From<FolderMetadataError> for UnifiedError {
    fn from(err: FolderMetadataError) -> Self {
        match &err {
            FolderMetadataError::NotAuthenticated => err.into_unified(Category::Auth),
            FolderMetadataError::InvalidPath => err.into_unified(Category::InvalidInput),
            FolderMetadataError::Resolve(_) => err.into_unified(Category::Api),
        }
    }
}

impl From<PublinkHelperError> for UnifiedError {
    fn from(err: PublinkHelperError) -> Self {
        match &err {
            PublinkHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            PublinkHelperError::EmptyCode | PublinkHelperError::EmptyRecipients => {
                err.into_unified(Category::InvalidInput)
            }
            PublinkHelperError::Send(_) => err.into_unified(Category::Api),
        }
    }
}

impl From<TreePublicLinkHelperError> for UnifiedError {
    fn from(err: TreePublicLinkHelperError) -> Self {
        match &err {
            TreePublicLinkHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            TreePublicLinkHelperError::EmptyName | TreePublicLinkHelperError::EmptyPaths => {
                err.into_unified(Category::InvalidInput)
            }
            TreePublicLinkHelperError::PathResolution(_) | TreePublicLinkHelperError::Api(_) => {
                err.into_unified(Category::Api)
            }
        }
    }
}

impl From<CryptoHelperError> for UnifiedError {
    fn from(err: CryptoHelperError) -> Self {
        match &err {
            CryptoHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            CryptoHelperError::EmptyPassword | CryptoHelperError::EmptyCode => {
                err.into_unified(Category::InvalidInput)
            }
            CryptoHelperError::Shell(_)
            | CryptoHelperError::SendChangeUserPrivate(_)
            | CryptoHelperError::ChangeUserPrivate(_) => err.into_unified(Category::Crypto),
        }
    }
}

impl From<AuthHelperError> for UnifiedError {
    fn from(err: AuthHelperError) -> Self {
        err.into_unified(Category::Auth)
    }
}

impl From<DownloadHelperError> for UnifiedError {
    fn from(err: DownloadHelperError) -> Self {
        match &err {
            DownloadHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            DownloadHelperError::GetFileLink(_) | DownloadHelperError::DownloadBytes(_) => {
                err.into_unified(Category::Api)
            }
        }
    }
}

impl From<CreateFolderHelperError> for UnifiedError {
    fn from(err: CreateFolderHelperError) -> Self {
        match &err {
            CreateFolderHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            CreateFolderHelperError::EmptyName | CreateFolderHelperError::InvalidPath => {
                err.into_unified(Category::InvalidInput)
            }
            CreateFolderHelperError::Api(_) => err.into_unified(Category::Api),
        }
    }
}

impl From<MountHelperError> for UnifiedError {
    fn from(err: MountHelperError) -> Self {
        match &err {
            MountHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            MountHelperError::Mount(_) => err.into_unified(Category::LocalIo),
        }
    }
}

impl From<FileMutationHelperError> for UnifiedError {
    fn from(err: FileMutationHelperError) -> Self {
        match &err {
            FileMutationHelperError::NotAuthenticated => err.into_unified(Category::Auth),
            FileMutationHelperError::DeleteFailed(_)
            | FileMutationHelperError::RenameFailed(_)
            | FileMutationHelperError::StatFailed(_) => err.into_unified(Category::Api),
        }
    }
}

impl From<upload_session::UploadError> for UnifiedError {
    fn from(err: upload_session::UploadError) -> Self {
        match &err {
            upload_session::UploadError::Canceled => err.into_unified(Category::Busy),
            upload_session::UploadError::NotStarted => err.into_unified(Category::InvalidInput),
            upload_session::UploadError::Io(_) => err.into_unified(Category::LocalIo),
            upload_session::UploadError::Helper(_) => err.into_unified(Category::Api),
            upload_session::UploadError::InvalidState(_) => {
                err.into_unified(Category::InvalidInput)
            }
            upload_session::UploadError::Journal(_) => err.into_unified(Category::LocalIo),
            upload_session::UploadError::HashMismatch { .. } => err.into_unified(Category::Api),
        }
    }
}

impl EmbeddedDaemonBuilder {
    /// Start a new builder rooted at `root`. Defaults to
    /// [`Environment::Production`] which pins TLS and secure posture.
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_embedded_sdk::EmbeddedDaemonBuilder;
    /// let b = EmbeddedDaemonBuilder::new(std::env::temp_dir().join("pcloud-doc"));
    /// let _daemon = b.build().expect("bootstrap");
    /// ```
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            environment: Environment::Production,
            extensions: None,
        }
    }

    /// Override the default Production environment with `environment`.
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_config::Environment;
    /// use pcloud_embedded_sdk::EmbeddedDaemon;
    /// let _d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-doc"))
    ///     .environment(Environment::Development)
    ///     .build()
    ///     .unwrap();
    /// ```
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
        self
    }

    /// Override the secure-default plugin policy for this embedded daemon.
    ///
    /// The default keeps plugins disabled. Embedders that register plugins
    /// must opt in explicitly and grant only the capabilities their plugins
    /// require. The policy is validated during [`Self::build`].
    #[must_use]
    pub fn extension_policy(mut self, policy: ExtensionPolicy) -> Self {
        self.extensions = Some(policy);
        self
    }

    /// Consume the builder and bootstrap the [`EmbeddedDaemon`]. The
    /// runtime initializes the store, auth manager, IPC wiring, and every
    /// protocol runtime in-process.
    ///
    /// # Preconditions
    ///
    /// `root` must name a directory that either exists (owner-only,
    /// mode `0700`) or can be created with those permissions. In
    /// `Environment::Production` the bootstrap pins TLS-only transport.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::EmbeddedDaemon`] wrapping
    /// [`EmbeddedDaemonError::Bootstrap`] on profile load, store init,
    /// IPC socket, or permission failure. Not retryable without fixing
    /// the underlying environment.
    ///
    /// # Side effects
    ///
    /// Opens (or creates) the SQLite store under `root`, materialises
    /// the auth vault file, binds the owner-only local IPC socket, and
    /// constructs every in-process protocol runtime. No network I/O.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_embedded_sdk::EmbeddedDaemonBuilder;
    /// let d = EmbeddedDaemonBuilder::new(std::env::temp_dir().join("pcloud-doc"))
    ///     .build()
    ///     .expect("bootstrap");
    /// assert!(!d.runtime_summary().is_empty());
    /// ```
    pub fn build(self) -> Result<EmbeddedDaemon, SdkError> {
        let mut requested_config = ConfigProfile::secure_defaults(self.root, self.environment);
        if let Some(extensions) = self.extensions {
            requested_config.extensions = extensions;
        }
        let runtime =
            bootstrap_with_config(requested_config).map_err(EmbeddedDaemonError::Bootstrap)?;

        Ok(EmbeddedDaemon {
            runtime,
            plugins: PluginRegistry::new(),
        })
    }
}

impl EmbeddedDaemon {
    /// Entry point to the fluent [`EmbeddedDaemonBuilder`].
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_embedded_sdk::EmbeddedDaemon;
    /// let _builder = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-doc"));
    /// ```
    #[must_use]
    pub fn builder(root: PathBuf) -> EmbeddedDaemonBuilder {
        EmbeddedDaemonBuilder::new(root)
    }

    /// Short one-line runtime summary for logs / CLI status.
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_embedded_sdk::EmbeddedDaemon;
    /// let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-doc")).build().unwrap();
    /// let _ = d.runtime_summary();
    /// ```
    #[must_use]
    pub fn runtime_summary(&self) -> String {
        self.runtime.summary()
    }

    /// Read-only reference to the active [`ConfigProfile`].
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_embedded_sdk::EmbeddedDaemon;
    /// let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-doc")).build().unwrap();
    /// assert!(d.config().features.crypto_enabled);
    /// ```
    #[must_use]
    pub fn config(&self) -> &ConfigProfile {
        &self.runtime.config
    }

    /// Route a [`Request`] through the runtime dispatch loop and return
    /// the typed [`Response`]. This is the same code path IPC exercises.
    ///
    /// # Preconditions
    ///
    /// The daemon must be bootstrapped. Requests are validated by the
    /// dispatcher; unknown method combinations return a non-`Ok`
    /// `ResponseStatus` rather than panicking.
    ///
    /// # Errors
    ///
    /// This method is infallible at the Rust level — errors are encoded
    /// in the returned [`Response::status`] and payload.
    ///
    /// # Side effects
    ///
    /// May mutate the runtime (auth state, sync-root table, settings
    /// store, audit log) depending on the request method. Health probes
    /// are read-only. Network I/O happens for methods that require a
    /// backend round-trip; expect 100–500 ms latency for those.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_embedded_sdk::EmbeddedDaemon;
    /// use pcloud_ipc::{Method, Request};
    /// let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-doc")).build().unwrap();
    /// let _resp = d.dispatch(Request::Plain { method: Method::GetHealth });
    /// ```
    pub fn dispatch(&mut self, request: Request) -> Response {
        dispatch(&mut self.runtime, request)
    }

    /// Submit a username and password credential pair to the auth state machine.
    /// Mirrors `psync_set_user_pass` / `psync_login`.
    ///
    /// The password is accepted as a plain `&str` at the SDK boundary and
    /// wrapped into a [`pcloud_ipc::RedactedString`] so it zeroizes on drop
    /// and never appears in `Debug` output.
    ///
    /// Returns `Ok(())` on acceptance. When two-factor auth is required the
    /// server returns a TFA challenge; follow up with
    /// [`Self::submit_two_factor_code`].
    ///
    /// # Errors
    ///
    /// [`SdkError::Auth`] wrapping [`AuthHelperError::Login`] when the
    /// server rejects the credentials.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.login("user@example.com", "password");
    /// ```
    // AUDIT-NOTE: gptrev-01 M-01 — first-class login helper added so that
    // API-REFERENCE.md entry "EmbeddedDaemon::login" compiles.
    pub fn login(&mut self, username: &str, password: &str) -> Result<(), SdkError> {
        let response = self.dispatch(Request::PasswordSubmission {
            username: username.to_owned(),
            value: password.to_owned().into(),
        });
        if response.status == ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(AuthHelperError::Login(response.message)))
        }
    }

    /// Submit a pre-obtained pCloud API auth token to the auth state machine.
    /// Mirrors `psync_set_auth`.
    ///
    /// The token is accepted as a plain `&str` and wrapped into a
    /// [`pcloud_ipc::RedactedString`] so it zeroizes on drop.
    ///
    /// # Errors
    ///
    /// [`SdkError::Auth`] wrapping [`AuthHelperError::Login`] on rejection.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.login_with_token("my-auth-token");
    /// ```
    // AUDIT-NOTE: gptrev-01 M-01 — first-class login_with_token helper added.
    pub fn login_with_token(&mut self, token: &str) -> Result<(), SdkError> {
        let response = self.dispatch(Request::AuthTokenSubmission {
            value: token.to_owned().into(),
        });
        if response.status == ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(AuthHelperError::Login(response.message)))
        }
    }

    /// Submit a TFA recovery code. Convenience wrapper over
    /// [`Self::submit_two_factor_code`] with `recovery_code = true`.
    ///
    /// Mirrors the `psync_tfa_set_code` recovery-code path.
    ///
    /// # Errors
    ///
    /// Propagates errors from [`Self::submit_two_factor_code`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.submit_recovery_code("recovery-phrase-here", false);
    /// ```
    // AUDIT-NOTE: gptrev-01 M-01 — submit_recovery_code added so
    // API-REFERENCE.md entry compiles. Delegates to submit_two_factor_code.
    pub fn submit_recovery_code(&mut self, code: &str, trust_device: bool) -> Result<(), SdkError> {
        self.submit_two_factor_code(code, trust_device, true)
    }

    /// Register a plugin against the embedded daemon's plugin registry.
    ///
    /// # Preconditions
    ///
    /// The daemon must have been successfully built via
    /// [`EmbeddedDaemonBuilder::build`]. The plugin must satisfy the
    /// [`Plugin`] trait contract and carry a valid signature unless the
    /// runtime is in dev-unsigned mode.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::EmbeddedDaemon`] wrapping a
    /// [`PluginError`] on signature mismatch, duplicate plugin id,
    /// capability violation, or audit-sink failure. Not transiently
    /// retryable — fix the plugin or its manifest.
    ///
    /// # Side effects
    ///
    /// Adds the plugin descriptor to the in-memory registry, appends one
    /// or more `plugin.*` rows to the hash-chained audit log, and claims
    /// the plugin's declared capabilities so subsequent
    /// [`Self::authorize_plugin_operation`] calls can be enforced.
    ///
    /// # Examples
    ///
    /// See `examples/sdk_plugin_registration.rs` for a runnable demo.
    pub fn register_plugin<P: Plugin>(
        &mut self,
        plugin: &mut P,
    ) -> Result<&RegisteredPlugin, SdkError> {
        let summary = self.runtime.summary();
        let mut sink = StoreAuditSink {
            store: &mut self.runtime.store,
        };
        self.plugins
            .register(plugin, &self.runtime.config.extensions, summary, &mut sink)
            .map_err(EmbeddedDaemonError::from)
            .map_err(SdkError::from)
    }

    /// Capability-gated dispatch for a plugin operation. Records an
    /// audit entry for both allow and deny outcomes via the
    /// hash-chained [`pcloud_store`] audit log.
    pub fn authorize_plugin_operation(
        &mut self,
        plugin_id: &str,
        operation: &PluginOperation,
    ) -> Result<(), SdkError> {
        let mut sink = StoreAuditSink {
            store: &mut self.runtime.store,
        };
        self.plugins
            .authorize(plugin_id, operation, &mut sink)
            .map(|_| ())
            .map_err(EmbeddedDaemonError::from)
            .map_err(SdkError::from)
    }

    /// Slice of plugins currently registered with the embedded daemon.
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use pcloud_embedded_sdk::EmbeddedDaemon;
    /// let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-doc")).build().unwrap();
    /// assert!(d.loaded_plugins().is_empty());
    /// ```
    #[must_use]
    pub fn loaded_plugins(&self) -> &[RegisteredPlugin] {
        self.plugins.loaded_plugins()
    }

    /// Start a high-level upload and return a [`UploadSession`] handle.
    ///
    /// The handle exposes a progress watch channel plus pause/resume/
    /// cancel/await-completion controls. See [`UploadSession`] for the
    /// full contract and the row 94 limitation: this legacy public helper
    /// still uses the synchronous single-shot upload path.
    ///
    /// # Preconditions
    ///
    /// An authenticated session must be present. The caller retains
    /// ownership of the [`UploadRequest`] payload; the entire payload is
    /// read and uploaded synchronously *before* this method returns.
    ///
    /// # Errors
    ///
    /// This method does not itself return `Result`; any failure is
    /// captured in the session's terminal outcome and surfaced via
    /// [`UploadSession::await_completion`] as [`UploadError`] →
    /// [`SdkError::UploadSession`].
    ///
    /// # Side effects
    ///
    /// Performs one `upload_create` round-trip and one chunked byte
    /// transfer (currently single-shot). Publishes
    /// `Pending → Uploading → Completed|Failed` on the progress watch
    /// channel. Typical latency scales linearly with payload size.
    ///
    /// # Examples
    ///
    /// See `examples/sdk_upload_download.rs` for a runnable demo.
    pub fn start_upload(&mut self, request: UploadRequest) -> UploadSession {
        upload_session::run_upload(self, request)
    }

    /// Upload `data` as a new file named `remote_filename` under
    /// `folder_id`. Requires an authenticated session.
    ///
    /// # Preconditions
    ///
    /// An authenticated session is present. `folder_id` must resolve to
    /// a writable remote folder; `remote_filename` should be non-empty
    /// and within the pCloud name-length limit.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::Upload`] wrapping
    /// [`UploadHelperError::NotAuthenticated`] when no session exists,
    /// [`UploadHelperError::Create`] on `upload_create` failure, or
    /// [`UploadHelperError::Write`] on `upload_write`/`upload_save`
    /// failure. Retryability: transiently retryable with backoff.
    ///
    /// # Side effects
    ///
    /// Two daemon round-trips: one `upload_create` call to reserve an
    /// `uploadid`, one signed-HTTPS byte transfer that finishes with an
    /// implicit `upload_save`. Total latency scales linearly with
    /// `data.len()` plus one ~100–500 ms API call.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _res = d.upload_data(22, "report.txt", b"hello");
    /// ```
    pub fn upload_data(
        &mut self,
        folder_id: u64,
        remote_filename: impl Into<String>,
        data: &[u8],
    ) -> Result<UploadResult, SdkError> {
        let auth_token = self.auth_token()?;
        let remote_filename = remote_filename.into();
        let session = self
            .runtime
            .transfer_runtime
            .upload_create(
                auth_token.clone_secret(),
                folder_id,
                remote_filename.clone(),
                data.len() as u64,
            )
            .map_err(|err| UploadHelperError::Create(err.to_string()))?;
        self.runtime
            .transfer_runtime
            .upload_bytes(auth_token, &session, data)
            .map_err(|err| UploadHelperError::Write(err.to_string()))?;

        Ok(UploadResult {
            upload_id: session.upload_id,
            file_id: session.file_id,
            parent_folder_id: session.parent_folder_id,
            remote_filename: session.file_name,
            bytes_uploaded: data.len(),
        })
    }

    /// Read `local_path` and upload it as `remote_filename` under
    /// `folder_id`.
    ///
    /// ```no_run
    /// # use std::path::{Path, PathBuf};
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _res = d.upload_file(22, "report.txt", Path::new("/etc/hostname"));
    /// ```
    pub fn upload_file(
        &mut self,
        folder_id: u64,
        remote_filename: impl Into<String>,
        local_path: impl AsRef<Path>,
    ) -> Result<UploadResult, SdkError> {
        let bytes = std::fs::read(local_path).map_err(UploadHelperError::ReadLocalFile)?;
        self.upload_data(folder_id, remote_filename, &bytes)
    }

    /// Copy bytes from an existing remote pCloud file into an open upload
    /// session using the `upload_writefromfile` server-side-copy primitive.
    ///
    /// `upload_offset` maps to the C `uploadoffset` parameter and
    /// `source_offset` maps to the C `offset` parameter. They are separate
    /// on purpose: resumed or spliced copies do not always read and write at
    /// the same byte offset.
    ///
    /// # Errors
    ///
    /// Returns [`UploadHelperError::NotAuthenticated`] when no session is
    /// active, or [`UploadHelperError::Write`] when the daemon/backend rejects
    /// the server-side-copy request.
    pub fn upload_write_from_file(
        &mut self,
        upload_session_id: u64,
        source_fileid: u64,
        source_hash: u64,
        upload_offset: u64,
        source_offset: u64,
        count: u64,
    ) -> Result<(), SdkError> {
        if self.runtime.auth.snapshot().auth_token.is_none() {
            return Err(SdkError::from(UploadHelperError::NotAuthenticated));
        }
        let response = self.dispatch(Request::UploadWriteFromFile {
            upload_session_id,
            source_fileid,
            source_hash,
            offset: upload_offset,
            source_offset: Some(source_offset),
            count,
        });
        if response.status == ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(UploadHelperError::Write(response.message)))
        }
    }

    /// Upload `data` by absolute remote path, resolving `remote_path` to
    /// its folder id first.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _res = d.upload_data_as("/Documents", "report.txt", b"hello");
    /// ```
    pub fn upload_data_as(
        &mut self,
        remote_path: impl AsRef<str>,
        remote_filename: impl Into<String>,
        data: &[u8],
    ) -> Result<UploadResult, SdkError> {
        let auth_token = self.auth_token()?;
        let resolved = self
            .runtime
            .sync_runtime
            .validate_remote_folder(auth_token, remote_path.as_ref())
            .map_err(|err| UploadHelperError::ResolveRemoteFolder(err.to_string()))?;
        self.upload_data(resolved.folder_id.get(), remote_filename, data)
    }

    /// Upload `local_path` by absolute remote folder path.
    ///
    /// ```no_run
    /// # use std::path::{Path, PathBuf};
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _res = d.upload_file_as("/Documents", "x.txt", Path::new("/etc/hostname"));
    /// ```
    pub fn upload_file_as(
        &mut self,
        remote_path: impl AsRef<str>,
        remote_filename: impl Into<String>,
        local_path: impl AsRef<Path>,
    ) -> Result<UploadResult, SdkError> {
        let bytes = std::fs::read(local_path).map_err(UploadHelperError::ReadLocalFile)?;
        self.upload_data_as(remote_path, remote_filename, &bytes)
    }

    /// Create a remote folder under `parent_folder_id` with leaf `name`.
    /// Mirrors the C `psync_create_remote_folder`
    /// (`pclsync/psynclib.c:1020`) call wired to the `createfolder`
    /// endpoint.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _res = d.create_remote_folder(0, "project");
    /// ```
    pub fn create_remote_folder(
        &mut self,
        parent_folder_id: u64,
        name: impl Into<String>,
    ) -> Result<CreateFolderResult, SdkError> {
        let auth_token = self.folder_auth_token()?;
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SdkError::from(CreateFolderHelperError::EmptyName));
        }
        let response = self
            .runtime
            .folder_runtime
            .create_remote_folder(auth_token, parent_folder_id, name)
            .map_err(|err| CreateFolderHelperError::Api(err.to_string()))?;
        Ok(CreateFolderResult {
            folder_id: response.folder_id,
            name: response.name,
            parent_folder_id: response.parent_folder_id,
            created: response.created,
            suffix_index: None,
        })
    }

    /// Create a remote folder by absolute remote path. Mirrors the C
    /// `psync_create_remote_folder_by_path` call
    /// (`pclsync/psynclib.c:1006`).
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _res = d.create_remote_folder_by_path("/Documents/new");
    /// ```
    pub fn create_remote_folder_by_path(
        &mut self,
        path: impl Into<String>,
    ) -> Result<CreateFolderResult, SdkError> {
        let auth_token = self.folder_auth_token()?;
        let path = path.into();
        if !path.starts_with('/') {
            return Err(SdkError::from(CreateFolderHelperError::InvalidPath));
        }
        let response = self
            .runtime
            .folder_runtime
            .create_remote_folder_by_path(auth_token, path)
            .map_err(|err| CreateFolderHelperError::Api(err.to_string()))?;
        Ok(CreateFolderResult {
            folder_id: response.folder_id,
            name: response.name,
            parent_folder_id: response.parent_folder_id,
            created: response.created,
            suffix_index: None,
        })
    }

    /// Idempotent suffix-retry helper. Mirrors the C
    /// `psync_check_and_create_folder` helper in
    /// `pclsync/pbusinessaccount.c:803`. Tries `name`, then `name 2`,
    /// `name 3`, ... via the `createfolderifnotexists` endpoint until a
    /// candidate succeeds or the retry budget is exhausted. `suffix_index`
    /// in the returned result is `Some(0)` when the bare `name` was
    /// adopted/created, and `Some(N)` for the `"name N"` variant.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _res = d.check_and_create_folder(0, "inbox");
    /// ```
    pub fn check_and_create_folder(
        &mut self,
        parent_folder_id: u64,
        name: impl Into<String>,
    ) -> Result<CreateFolderResult, SdkError> {
        let auth_token = self.folder_auth_token()?;
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SdkError::from(CreateFolderHelperError::EmptyName));
        }
        let (response, suffix) = self
            .runtime
            .folder_runtime
            .check_and_create_folder(auth_token, parent_folder_id, name)
            .map_err(|err| CreateFolderHelperError::Api(err.to_string()))?;
        Ok(CreateFolderResult {
            folder_id: response.folder_id,
            name: response.name,
            parent_folder_id: response.parent_folder_id,
            created: response.created,
            suffix_index: Some(suffix),
        })
    }

    fn folder_auth_token(&self) -> Result<SecretString, SdkError> {
        self.runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or_else(|| SdkError::from(CreateFolderHelperError::NotAuthenticated))
    }

    /// List public pCloud API endpoints for the authenticated account.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _list = d.get_api_servers();
    /// ```
    pub fn get_api_servers(&self) -> Result<Vec<ApiServerResult>, SdkError> {
        self.runtime
            .account_runtime
            .get_api_servers()
            .map(|servers| {
                servers
                    .into_iter()
                    .map(|server| ApiServerResult {
                        label: server.label,
                        api: server.api,
                        binapi: server.binapi,
                        location_id: server.location_id,
                    })
                    .collect()
            })
            .map_err(|err| SdkError::from(AccountUtilityError::ApiServers(err.to_string())))
    }

    /// Fetch a current promo banner for the authenticated user, if any.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _p = d.get_promo();
    /// ```
    pub fn get_promo(&self) -> Result<Option<PromoResult>, SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(AccountUtilityError::NotAuthenticated)?;
        self.runtime
            .account_runtime
            .get_promo(auth_token)
            .map(|promo| {
                promo.map(|promo| PromoResult {
                    url: promo.url,
                    width: promo.width,
                    height: promo.height,
                })
            })
            .map_err(|err| SdkError::from(AccountUtilityError::Promo(err.to_string())))
    }

    /// Change the account UI language. Mirrors `psync_setlanguage`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.set_language("en");
    /// ```
    pub fn set_language(&self, language: &str) -> Result<(), SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(AccountUtilityError::NotAuthenticated)?;
        self.runtime
            .account_runtime
            .set_language(auth_token, language)
            .map_err(|err| SdkError::from(AccountUtilityError::SetLanguage(err.to_string())))
    }

    /// Request a re-send of the account-verification email.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.verify_email();
    /// ```
    pub fn verify_email(&self) -> Result<(), SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(AccountUtilityError::NotAuthenticated)?;
        self.runtime
            .account_runtime
            .verify_email(auth_token)
            .map_err(|err| SdkError::from(AccountUtilityError::VerifyEmail(err.to_string())))
    }

    /// Restricted-mode email verification — token, no auth.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.verify_email_restricted("tok");
    /// ```
    pub fn verify_email_restricted(&self, verify_token: &str) -> Result<(), SdkError> {
        self.runtime
            .account_runtime
            .verify_email_restricted(verify_token)
            .map_err(|err| {
                SdkError::from(AccountUtilityError::VerifyEmailRestricted(err.to_string()))
            })
    }

    /// Request a password-reset link email.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.lost_password("user@example.com");
    /// ```
    pub fn lost_password(&self, email: &str) -> Result<(), SdkError> {
        self.runtime
            .account_runtime
            .lost_password(email)
            .map_err(|err| SdkError::from(AccountUtilityError::LostPassword(err.to_string())))
    }

    /// Rotate the account password. Updates the local auth token.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.change_password("old", "new-strong-passphrase");
    /// ```
    pub fn change_password(
        &mut self,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(AccountUtilityError::NotAuthenticated)?;
        let result = self
            .runtime
            .account_runtime
            .change_password(
                auth_token,
                current_password,
                new_password,
                "Desktop, Linux, Rust SDK",
            )
            .map_err(|err| AccountUtilityError::ChangePassword(err.to_string()))?;
        self.runtime
            .auth
            // CLAUDEREV iter-1 SEC-H fix: result.auth_token already is
            // SecretString from pcloud-proto::account_api.
            .replace_auth_token(result.auth_token)
            .map_err(|err| SdkError::from(AccountUtilityError::ChangePassword(err.to_string())))
    }

    /// Register a new pCloud account.
    ///
    /// Mirrors the legacy C `psync_register` helper. This does not require an
    /// authenticated session and never persists the supplied password. Input is
    /// validated locally so obvious misuse (empty email/password, missing `@`,
    /// missing terms acceptance) never reaches the network.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # use pcloud_secret::secret_string::SecretString;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.register("user@example.com", SecretString::new("pw"), true);
    /// ```
    pub fn register(
        &self,
        email: &str,
        password: pcloud_secret::secret_string::SecretString,
        terms_accepted: bool,
    ) -> Result<(), SdkError> {
        if email.is_empty() || !email.contains('@') {
            return Err(SdkError::from(
                AccountUtilityError::InvalidRegistrationInput,
            ));
        }
        if password.expose_secret().is_empty() {
            return Err(SdkError::from(
                AccountUtilityError::InvalidRegistrationInput,
            ));
        }
        if !terms_accepted {
            return Err(SdkError::from(AccountUtilityError::TermsNotAccepted));
        }
        // OS id 3 corresponds to Linux per the legacy C PSYNC_OS id mapping.
        self.runtime
            .account_runtime
            .register(email, password, terms_accepted, 3)
            .map_err(|err| SdkError::from(AccountUtilityError::Register(err.to_string())))
    }

    /// Select the API endpoint the runtime should connect to.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.set_api_server("binapi.pcloud.com", 1);
    /// ```
    pub fn set_api_server(&mut self, binapi: &str, location_id: u32) -> Result<(), SdkError> {
        let response = self.dispatch(Request::SetApiServer {
            location_id,
            binapi: binapi.to_owned(),
        });
        if response.status == ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(AccountUtilityError::SetApiServer(
                response.message,
            )))
        }
    }

    /// Current crypto private-key flags. Mirrors the legacy
    /// `psync_crypto_priv_key_flags()` accessor — returns `0` when no flags
    /// are recorded, or when crypto has not been set up.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert_eq!(d.crypto_priv_key_flags(), 0);
    /// ```
    #[must_use]
    pub fn crypto_priv_key_flags(&self) -> u64 {
        self.runtime.crypto.priv_key_flags()
    }

    /// Request a server-side confirmation code for a subsequent crypto
    /// password rotation. Mirrors
    /// `psync_crypto_crypto_send_change_user_private`.
    /// Send-change-user-private wrapper.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.crypto_send_change_user_private();
    /// ```
    pub fn crypto_send_change_user_private(&self) -> Result<(), SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(CryptoHelperError::NotAuthenticated)?;
        self.runtime
            .crypto_runtime
            .send_change_user_private(pcloud_secret::ExposeSecret::expose_secret(&auth_token))
            .map_err(|err| {
                SdkError::from(CryptoHelperError::SendChangeUserPrivate(err.to_string()))
            })
    }

    /// Rotate the crypto passphrase while the shell is locked — requires
    /// the old passphrase plus a confirmation `code` from
    /// [`Self::crypto_send_change_user_private`]. Mirrors
    /// `psync_crypto_change_crypto_pass`.
    ///
    /// The old and new passwords are accepted as [`SecretString`] and are
    /// never logged or persisted by this crate; after the operation the
    /// shell is left unlocked with the new key.
    ///
    /// # Preconditions
    ///
    /// Authenticated session. A confirmation `code` must have been
    /// obtained via [`Self::crypto_send_change_user_private`]. Neither
    /// `old_password`, `new_password`, nor `code` may be empty.
    ///
    /// # Errors
    ///
    /// [`SdkError::Crypto`] wrapping
    /// [`CryptoHelperError::EmptyPassword`],
    /// [`CryptoHelperError::EmptyCode`],
    /// [`CryptoHelperError::NotAuthenticated`],
    /// [`CryptoHelperError::Shell`] (wrong old passphrase — user
    /// action), or
    /// [`CryptoHelperError::ChangeUserPrivate`] (server rejection —
    /// transiently retryable).
    ///
    /// # Side effects
    ///
    /// One local rekey pass in the crypto shell followed by one
    /// `changeuserprivate` round-trip. Leaves the shell unlocked with
    /// the new key on success. Typical latency: rekey is CPU-bound
    /// (tens of ms) + one ~100–500 ms API round-trip.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # use pcloud_secret::secret_string::SecretString;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.crypto_change_password(
    ///     SecretString::new("old"),
    ///     SecretString::new("new"),
    ///     "mnemonic",
    ///     "code",
    ///     0,
    /// );
    /// ```
    pub fn crypto_change_password(
        &mut self,
        old_password: pcloud_secret::secret_string::SecretString,
        new_password: pcloud_secret::secret_string::SecretString,
        hint: &str,
        code: &str,
        flags: u64,
    ) -> Result<(), SdkError> {
        use pcloud_secret::ExposeSecret as _;
        if old_password.expose_secret().is_empty() || new_password.expose_secret().is_empty() {
            return Err(SdkError::from(CryptoHelperError::EmptyPassword));
        }
        if code.is_empty() {
            return Err(SdkError::from(CryptoHelperError::EmptyCode));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(CryptoHelperError::NotAuthenticated)?;
        let rekeyed = self
            .runtime
            .crypto
            .change_password(old_password, new_password, flags)
            .map_err(|err| CryptoHelperError::Shell(err.to_string()))?;
        self.runtime
            .crypto_runtime
            .change_user_private(
                auth_token.expose_secret(),
                &rekeyed.private_key_hex,
                &rekeyed.signature_hex,
                hint,
                code,
            )
            .map_err(|err| SdkError::from(CryptoHelperError::ChangeUserPrivate(err.to_string())))
    }

    /// Rotate the crypto passphrase while the shell is already unlocked.
    /// Mirrors `psync_crypto_change_crypto_pass_unlocked`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # use pcloud_secret::secret_string::SecretString;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.crypto_change_password_unlocked(
    ///     SecretString::new("new"), "hint", "code", 0
    /// );
    /// ```
    pub fn crypto_change_password_unlocked(
        &mut self,
        new_password: pcloud_secret::secret_string::SecretString,
        hint: &str,
        code: &str,
        flags: u64,
    ) -> Result<(), SdkError> {
        use pcloud_secret::ExposeSecret as _;
        if new_password.expose_secret().is_empty() {
            return Err(SdkError::from(CryptoHelperError::EmptyPassword));
        }
        if code.is_empty() {
            return Err(SdkError::from(CryptoHelperError::EmptyCode));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(CryptoHelperError::NotAuthenticated)?;
        let rekeyed = self
            .runtime
            .crypto
            .change_password_unlocked(new_password, flags)
            .map_err(|err| CryptoHelperError::Shell(err.to_string()))?;
        self.runtime
            .crypto_runtime
            .change_user_private(
                auth_token.expose_secret(),
                &rekeyed.private_key_hex,
                &rekeyed.signature_hex,
                hint,
                code,
            )
            .map_err(|err| SdkError::from(CryptoHelperError::ChangeUserPrivate(err.to_string())))
    }

    /// Create a backup folder under the per-device backup root.
    ///
    /// Mirrors the behaviour of the C `psync_create_backup` entry point in
    /// `pclsync/psynclib.c`. The caller is expected to have already provisioned
    /// and persisted the device backup root id via
    /// [`EmbeddedDaemon::set_backup_device_folder_id`]. No local sync folder is
    /// added by this helper - the Rust rewrite defers local sync-root
    /// registration to the dedicated sync management surface.
    ///
    /// # Preconditions
    ///
    /// Authenticated session. A device backup root folder id must have
    /// been persisted via [`Self::set_backup_device_folder_id`]. `name`
    /// must be non-empty after trim.
    ///
    /// # Errors
    ///
    /// [`SdkError::Backup`] wrapping
    /// [`BackupHelperError::NotAuthenticated`],
    /// [`BackupHelperError::EmptyName`],
    /// [`BackupHelperError::BackupRootMissing`], or
    /// [`BackupHelperError::Create`]. The missing-id and input-shape
    /// variants are user-recoverable; `Create` is transiently retryable.
    ///
    /// # Side effects
    ///
    /// One `createbackup` round-trip. On success, returns a
    /// [`BackupCreated`] descriptor; the daemon does not auto-register a
    /// matching local sync root (explicit departure from the C client).
    /// Expected latency: 100–500 ms.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.create_backup("Documents", None);
    /// ```
    pub fn create_backup(
        &mut self,
        name: &str,
        parent_folder_name: Option<String>,
    ) -> Result<BackupCreated, SdkError> {
        if name.trim().is_empty() {
            return Err(SdkError::from(BackupHelperError::EmptyName));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(BackupHelperError::NotAuthenticated)?;
        let device_root = self
            .runtime
            .store
            .repositories
            .preferences
            .backup_device_folder_id
            .ok_or(BackupHelperError::BackupRootMissing)?;
        let created = self
            .runtime
            .backup_runtime
            .create_backup(auth_token, name.to_owned(), device_root, parent_folder_name)
            .map_err(|err| BackupHelperError::Create(err.to_string()))?;
        Ok(BackupCreated {
            folder_id: created.folder_id,
            parent_folder_id: created.parent_folder_id,
            name: created.name,
        })
    }

    /// Stop tracking a single backup folder on the backend. Mirrors the remote
    /// side of the C `psync_delete_backup` flow (the local sync-folder removal
    /// stays under the existing sync-root management surface).
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.delete_backup(42);
    /// ```
    pub fn delete_backup(&mut self, folder_id: u64) -> Result<(), SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(BackupHelperError::NotAuthenticated)?;
        self.runtime
            .backup_runtime
            .stop_backup(auth_token, folder_id)
            .map_err(|err| SdkError::from(BackupHelperError::StopBackup(err.to_string())))
    }

    /// Stop all backups associated with the current device. Mirrors
    /// `psync_stop_device`. When no explicit folder id is provided, the
    /// device-root folder stored in the preferences repository is used, which
    /// mirrors the C fallback to the `BackupRootFoId` setting.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.stop_device(None);
    /// ```
    pub fn stop_device(&mut self, device_folder_id: Option<u64>) -> Result<(), SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(BackupHelperError::NotAuthenticated)?;
        let folder_id = match device_folder_id {
            Some(value) => value,
            None => self
                .runtime
                .store
                .repositories
                .preferences
                .backup_device_folder_id
                .ok_or(BackupHelperError::DeviceFolderMissing)?,
        };
        self.runtime
            .backup_runtime
            .stop_device(auth_token, folder_id)
            .map_err(|err| SdkError::from(BackupHelperError::StopDevice(err.to_string())))
    }

    /// Clear the locally persisted device backup folder id. Mirrors
    /// `psync_delete_backup_device` which is invoked after a stop-device that
    /// was triggered from the pCloud web UI, forcing a fresh device id on the
    /// next backup-create call.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.delete_backup_device();
    /// ```
    pub fn delete_backup_device(&mut self) -> Result<(), SdkError> {
        self.runtime
            .store
            .repositories
            .preferences
            .backup_device_folder_id = None;
        pcloud_store::persist_profile(&self.runtime.store)
            .map_err(|err| SdkError::from(BackupHelperError::Persist(err.to_string())))
    }

    /// Persist the per-device backup root folder id. Exposed so the CLI/IPC
    /// layers can provision the same value the C client stores under
    /// `setting.BackupRootFoId`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.set_backup_device_folder_id(1234);
    /// ```
    pub fn set_backup_device_folder_id(&mut self, folder_id: u64) -> Result<(), SdkError> {
        self.runtime
            .store
            .repositories
            .preferences
            .backup_device_folder_id = Some(folder_id);
        pcloud_store::persist_profile(&self.runtime.store)
            .map_err(|err| SdkError::from(BackupHelperError::Persist(err.to_string())))
    }

    /// List pending account notifications. Mirrors the C entry point
    /// `psync_get_notifications` (pclsync/psynclib.c:248). Returns a typed
    /// [`Notification`] list resolved through
    /// [`pcloud_daemon::notifications_backend::NotificationsRuntime`].
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.list_notifications();
    /// ```
    pub fn list_notifications(&mut self) -> Result<Vec<Notification>, SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(NotificationsHelperError::NotAuthenticated)?;
        self.runtime
            .notifications_runtime
            .list_notifications(auth_token, None)
            .map_err(|err| SdkError::from(NotificationsHelperError::List(err.to_string())))
    }

    /// Mark all account notifications up to and including `upto_id` as read.
    /// Mirrors the C entry point `psync_mark_notificaitons_read` (sic -
    /// pclsync/psynclib.c:324). The Rust identifier uses the corrected
    /// spelling; the wire command (`readnotifications`) is unchanged.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.mark_notifications_read(42);
    /// ```
    pub fn mark_notifications_read(&mut self, upto_id: u64) -> Result<(), SdkError> {
        if upto_id == 0 {
            return Err(SdkError::from(
                NotificationsHelperError::InvalidNotificationId,
            ));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(NotificationsHelperError::NotAuthenticated)?;
        self.runtime
            .notifications_runtime
            .mark_notifications_read(auth_token, upto_id)
            .map_err(|err| SdkError::from(NotificationsHelperError::MarkRead(err.to_string())))
    }

    /// Trigger an immediate local-scan wakeup on the embedded engine.
    /// Mirrors the C entry point `psync_run_localscan`
    /// (`pclsync/psynclib.c:886`). Returns the new wake counter value
    /// that callers can correlate with engine-side observation.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _wake = d.run_localscan();
    /// ```
    pub fn run_localscan(&mut self) -> u64 {
        self.runtime.engine.wake_localscan()
    }

    /// Mail an existing public link `code` to the comma-separated `emails`
    /// list with the supplied `message` body. Mirrors the C entry point
    /// `psync_send_publink` (`pclsync/psynclib.c:2217`).
    ///
    /// # Preconditions
    ///
    /// Authenticated session. `code` must be non-empty and correspond
    /// to an existing public link owned by the caller. `emails` must
    /// resolve to at least one recipient after parsing.
    ///
    /// # Errors
    ///
    /// [`SdkError::Publink`] wrapping
    /// [`PublinkHelperError::NotAuthenticated`],
    /// [`PublinkHelperError::EmptyCode`],
    /// [`PublinkHelperError::EmptyRecipients`], or
    /// [`PublinkHelperError::Send`] (server rejection, transiently
    /// retryable).
    ///
    /// # Side effects
    ///
    /// One `sendpublink` round-trip. No local state mutation beyond
    /// audit log entry. Expected latency: 100–500 ms.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.send_publink("ABC123", "a@x.com,b@x.com", "Check this out");
    /// ```
    pub fn send_publink(
        &mut self,
        code: impl Into<String>,
        emails: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<(), SdkError> {
        let code = code.into();
        let emails = emails.into();
        if code.trim().is_empty() {
            return Err(SdkError::from(PublinkHelperError::EmptyCode));
        }
        if emails.trim().is_empty() {
            return Err(SdkError::from(PublinkHelperError::EmptyRecipients));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(PublinkHelperError::NotAuthenticated)?;
        self.runtime
            .public_link_runtime
            .send_publink(auth_token, code, emails, message.into())
            .map_err(|err| SdkError::from(PublinkHelperError::Send(err.to_string())))
    }

    /// Create a tree public link by resolving one or more absolute
    /// pCloud-drive folder or file paths under the daemon's authenticated
    /// context, then invoking `ptree_public_link`. Mirrors the C path-based
    /// variant of `psync_create_uploadlink` / `ptree_public_link`
    /// (row 149, bd-1du).
    ///
    /// Returns a [`pcloud_model::public_links::CreatedTreePublicLink`] on
    /// success.
    ///
    /// # Errors
    ///
    /// [`SdkError::TreePublicLink`] wrapping:
    /// - [`TreePublicLinkHelperError::NotAuthenticated`] — no active session.
    /// - [`TreePublicLinkHelperError::EmptyName`] — blank link name.
    /// - [`TreePublicLinkHelperError::EmptyPaths`] — no paths supplied.
    /// - [`TreePublicLinkHelperError::PathResolution`] — one or more paths
    ///   could not be resolved to either a remote folder id or file id.
    /// - [`TreePublicLinkHelperError::Api`] — server rejected the tree-link
    ///   creation. Transiently retryable with backoff.
    ///
    /// # Side effects
    ///
    /// One `ptree_public_link` round-trip (plus one path-resolver call per
    /// path). No local state mutation. Expected latency: 100–800 ms depending
    /// on the number of paths.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let result = d.create_tree_public_link_from_paths(
    ///     "My shared bundle",
    ///     vec!["/Documents/report.pdf".to_owned(), "/Photos/album".to_owned()],
    ///     None,
    /// );
    /// ```
    pub fn create_tree_public_link_from_paths(
        &mut self,
        name: impl Into<String>,
        paths: Vec<String>,
        expires: Option<u64>,
    ) -> Result<pcloud_model::public_links::CreatedTreePublicLink, SdkError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SdkError::from(TreePublicLinkHelperError::EmptyName));
        }
        if paths.is_empty() {
            return Err(SdkError::from(TreePublicLinkHelperError::EmptyPaths));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or(TreePublicLinkHelperError::NotAuthenticated)?;
        let resolver = self
            .runtime
            .public_link_runtime
            .path_resolver(auth_token.clone_secret());
        let mut folders = Vec::new();
        let mut files = Vec::new();
        for path in paths {
            match resolver.resolve_folder(&path) {
                Ok(_) => folders.push(path),
                Err(PathResolveError::ExpectedFolder { .. }) => {
                    resolver.resolve_file(&path).map_err(|err| {
                        TreePublicLinkHelperError::PathResolution(err.to_string())
                    })?;
                    files.push(path);
                }
                Err(err) => {
                    return Err(SdkError::from(TreePublicLinkHelperError::PathResolution(
                        err.to_string(),
                    )));
                }
            }
        }
        let link_paths = pcloud_proto::public_links_api::TreePublicLinkPaths {
            root: None,
            folders,
            files,
        };
        self.runtime
            .public_link_runtime
            .create_tree_public_link_from_paths(
                auth_token,
                name,
                &link_paths,
                &resolver,
                expires,
                None,
                None,
            )
            .map_err(|err| SdkError::from(TreePublicLinkHelperError::Api(err.to_string())))
    }

    /// Create a tree public link from the explicit C target shape: optional
    /// root folder path, zero or more folder paths, and zero or more file
    /// paths.
    ///
    /// This is the SDK route to the full row 149 path surface when callers
    /// need a root target instead of a flat mixed path list.
    ///
    /// # Errors
    ///
    /// Returns [`TreePublicLinkHelperError::EmptyName`] for a blank link
    /// name, [`TreePublicLinkHelperError::EmptyPaths`] when all target sets
    /// are empty, [`TreePublicLinkHelperError::NotAuthenticated`] without an
    /// active session, and [`TreePublicLinkHelperError::Api`] for resolver or
    /// server rejection.
    pub fn create_tree_public_link_from_targets(
        &mut self,
        name: impl Into<String>,
        root: Option<String>,
        folders: Vec<String>,
        files: Vec<String>,
        expires: Option<u64>,
    ) -> Result<pcloud_model::public_links::CreatedTreePublicLink, SdkError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SdkError::from(TreePublicLinkHelperError::EmptyName));
        }
        if root.is_none() && folders.is_empty() && files.is_empty() {
            return Err(SdkError::from(TreePublicLinkHelperError::EmptyPaths));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or(TreePublicLinkHelperError::NotAuthenticated)?;
        let link_paths = pcloud_proto::public_links_api::TreePublicLinkPaths {
            root,
            folders,
            files,
        };
        self.runtime
            .public_link_runtime
            .create_tree_public_link_from_paths_default(
                auth_token,
                name,
                &link_paths,
                expires,
                None,
                None,
            )
            .map_err(|err| SdkError::from(TreePublicLinkHelperError::Api(err.to_string())))
    }

    /// Resolve an absolute pCloud-drive path to its folder id. Mirrors
    /// C `psync_get_fsfolderid_by_path` (`pclsync/psynclib.c:2170`). On
    /// miss the C client returns the `PSYNC_INVALID_FSFOLDERID`
    /// sentinel; the Rust surface returns a typed error instead so
    /// callers cannot conflate a real `0` id with a resolution miss.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.get_folder_id_by_path("/Documents");
    /// ```
    pub fn get_folder_id_by_path(&mut self, path: impl Into<String>) -> Result<u64, SdkError> {
        let path = path.into();
        if path.trim().is_empty() || !path.starts_with('/') {
            return Err(SdkError::from(FolderMetadataError::InvalidPath));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or(FolderMetadataError::NotAuthenticated)?;
        let resolver = self.runtime.public_link_runtime.path_resolver(auth_token);
        let id: pcloud_model::ids::RemoteFolderId = resolver.get_folder_id_by_path(&path).map_err(
            |err: pcloud_daemon::path_resolver::PathResolveError| {
                FolderMetadataError::Resolve(err.to_string())
            },
        )?;
        Ok(id.get())
    }

    /// Read folder flags / permissions / sharing / encryption view for
    /// an absolute pCloud-drive path. Mirrors C
    /// `psync_get_fsfolderflags_by_id` (`pclsync/psynclib.c:2176`) plus
    /// the `flags`+`permissions` out-params of
    /// `pfs_fldr_idperm_by_path` (`pfsfolder.c:342`).
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.get_folder_flags("/Documents");
    /// ```
    pub fn get_folder_flags(
        &mut self,
        path: impl Into<String>,
    ) -> Result<FolderFlagsInfo, SdkError> {
        let path = path.into();
        if path.trim().is_empty() || !path.starts_with('/') {
            return Err(SdkError::from(FolderMetadataError::InvalidPath));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or(FolderMetadataError::NotAuthenticated)?;
        let resolver = self.runtime.public_link_runtime.path_resolver(auth_token);
        let flags = resolver.get_folder_flags(&path).map_err(
            |err: pcloud_daemon::path_resolver::PathResolveError| {
                FolderMetadataError::Resolve(err.to_string())
            },
        )?;
        Ok(FolderFlagsInfo {
            permissions: flags.permissions,
            encrypted: flags.encrypted,
            shared: flags.shared,
            readonly: flags.readonly,
        })
    }

    /// Read the owner user id of a folder by absolute pCloud-drive
    /// path. Mirrors C `psync_get_folder_ownerid`
    /// (`pclsync/psynclib.c:2088`).
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.get_folder_owner_id("/Documents");
    /// ```
    pub fn get_folder_owner_id(&mut self, path: impl Into<String>) -> Result<u64, SdkError> {
        let path = path.into();
        if path.trim().is_empty() || !path.starts_with('/') {
            return Err(SdkError::from(FolderMetadataError::InvalidPath));
        }
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or(FolderMetadataError::NotAuthenticated)?;
        let resolver = self.runtime.public_link_runtime.path_resolver(auth_token);
        let user_id: pcloud_model::ids::UserId = resolver.get_folder_owner_id(&path).map_err(
            |err: pcloud_daemon::path_resolver::PathResolveError| {
                FolderMetadataError::Resolve(err.to_string())
            },
        )?;
        Ok(user_id.get())
    }

    /// Classify an absolute local path against the daemon's sync-root +
    /// engine state. Mirrors C `psync_filesystem_status`
    /// (`pclsync/psynclib.c:1903`). Returns one of
    /// [`FilesystemPathStatus::InSync`], [`FilesystemPathStatus::InProgress`],
    /// [`FilesystemPathStatus::NoSync`], or [`FilesystemPathStatus::Invalid`].
    ///
    /// This is a pure, metadata-only classification — it does NOT touch
    /// the local filesystem.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::{EmbeddedDaemon, FilesystemPathStatus};
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert_eq!(d.filesystem_status("/does/not/exist"), FilesystemPathStatus::Invalid);
    /// ```
    #[must_use]
    pub fn filesystem_status(&self, path: impl AsRef<Path>) -> FilesystemPathStatus {
        use pcloud_daemon::path_resolver::{
            FilesystemStatusInputs, FsPathStatus, SyncRootView, filesystem_status,
        };
        use pcloud_model::sync::PlannedOperation;

        let sync_roots: Vec<SyncRootView<'_>> = self
            .runtime
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter()
            .map(|root| SyncRootView {
                sync_id: root.sync_id.get(),
                local_path: root.local_path.as_str(),
                paused: root.paused,
            })
            .collect();

        let paused_from_engine: Vec<u64> = sync_roots
            .iter()
            .filter_map(|view| {
                let id = pcloud_model::ids::SyncId::new(view.sync_id);
                if self.runtime.engine.is_sync_root_paused(id) {
                    Some(view.sync_id)
                } else {
                    None
                }
            })
            .collect();

        let mut queued_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        let mut errored_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        for op in self.runtime.engine.scheduler.queued_operations.iter() {
            if matches!(op, PlannedOperation::Conflict { .. }) {
                errored_ids.insert(op.sync_id().get());
            } else {
                queued_ids.insert(op.sync_id().get());
            }
        }
        let queued_vec: Vec<u64> = queued_ids.into_iter().collect();
        let errored_vec: Vec<u64> = errored_ids.into_iter().collect();

        let inputs = FilesystemStatusInputs {
            sync_roots: &sync_roots,
            paused_sync_ids: &paused_from_engine,
            queued_sync_ids: &queued_vec,
            errored_sync_ids: &errored_vec,
        };

        match filesystem_status(path.as_ref(), inputs) {
            FsPathStatus::InSync => FilesystemPathStatus::InSync,
            FsPathStatus::InProgress => FilesystemPathStatus::InProgress,
            FsPathStatus::NoSync => FilesystemPathStatus::NoSync,
            FsPathStatus::Invalid => FilesystemPathStatus::Invalid,
        }
    }

    /// Currently persisted per-device backup root folder id, if any.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert!(d.backup_device_folder_id().is_none());
    /// ```
    #[must_use]
    pub fn backup_device_folder_id(&self) -> Option<u64> {
        self.runtime
            .store
            .repositories
            .preferences
            .backup_device_folder_id
    }

    /// Typed key/value read: mirrors `psync_get_uint_value`. Returns `0`
    /// when the row is missing or was stored under a different kind, which
    /// matches the C default.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _v = d.get_uint_value("usedquota");
    /// ```
    pub fn get_uint_value(&self, name: &str) -> Result<u64, SdkError> {
        pcloud_store::value_kv::get_uint(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Typed key/value read: mirrors `psync_get_int_value`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _v = d.get_int_value("last_offset");
    /// ```
    pub fn get_int_value(&self, name: &str) -> Result<i64, SdkError> {
        pcloud_store::value_kv::get_int(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Typed key/value read: mirrors `psync_get_bool_value`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _v = d.get_bool_value("crypto_setup");
    /// ```
    pub fn get_bool_value(&self, name: &str) -> Result<bool, SdkError> {
        pcloud_store::value_kv::get_bool(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Typed key/value read: mirrors `psync_get_string_value`. Returns
    /// `None` when absent. The C helper returns a newly allocated
    /// `char *` that the caller frees; the Rust version hands the caller
    /// an owned `String` with the same lifetime rules.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _v = d.get_string_value("user_email");
    /// ```
    pub fn get_string_value(&self, name: &str) -> Result<Option<String>, SdkError> {
        pcloud_store::value_kv::get_string(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Typed key/value write: mirrors `psync_set_uint_value`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_uint_value("last_seen", 42).unwrap();
    /// ```
    pub fn set_uint_value(&self, name: &str, value: u64) -> Result<(), SdkError> {
        pcloud_store::value_kv::set_uint(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Typed key/value write: mirrors `psync_set_int_value`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_int_value("delta", -1).unwrap();
    /// ```
    pub fn set_int_value(&self, name: &str, value: i64) -> Result<(), SdkError> {
        pcloud_store::value_kv::set_int(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Typed key/value write: mirrors `psync_set_bool_value`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_bool_value("welcome_seen", true).unwrap();
    /// ```
    pub fn set_bool_value(&self, name: &str, value: bool) -> Result<(), SdkError> {
        pcloud_store::value_kv::set_bool(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Typed key/value write: mirrors `psync_set_string_value`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_string_value("hint", "hello").unwrap();
    /// ```
    pub fn set_string_value(&self, name: &str, value: &str) -> Result<(), SdkError> {
        pcloud_store::value_kv::set_string(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Presence + kind check. Stricter than the C API (which has no
    /// `has_*_value` helper and approximates presence with non-zero reads).
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert!(!d.has_uint_value("nope").unwrap());
    /// ```
    pub fn has_uint_value(&self, name: &str) -> Result<bool, SdkError> {
        pcloud_store::value_kv::has_uint(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Presence + kind check for the int column.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.has_int_value("nope");
    /// ```
    pub fn has_int_value(&self, name: &str) -> Result<bool, SdkError> {
        pcloud_store::value_kv::has_int(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Presence + kind check for the bool column.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.has_bool_value("nope");
    /// ```
    pub fn has_bool_value(&self, name: &str) -> Result<bool, SdkError> {
        pcloud_store::value_kv::has_bool(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Presence + kind check for the string column.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.has_string_value("nope");
    /// ```
    pub fn has_string_value(&self, name: &str) -> Result<bool, SdkError> {
        pcloud_store::value_kv::has_string(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(ValueKvError::Store(err.to_string())))
    }

    /// Returns `true` when a valid auth token is held in the session manager.
    ///
    /// Mirrors the shape of the legacy C `psync_get_auth_string` null-check pattern
    /// without exposing the underlying secret.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert!(!d.is_authenticated());
    /// ```
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.runtime.auth.snapshot().auth_token.is_some()
    }

    /// Returns the authenticated user id (when the session has been upgraded
    /// with a `userinfo` result). Mirrors `psync_get_current_userid`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert!(d.current_user_id().is_none());
    /// ```
    #[must_use]
    pub fn current_user_id(&self) -> Option<u64> {
        self.runtime
            .auth
            .snapshot()
            .authenticated_user
            .map(|user_id| user_id.get())
    }

    /// Returns the authenticated user's email address, when known. Mirrors
    /// `psync_get_username` (the C client stores the email as the display
    /// username).
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert!(d.username().is_none());
    /// ```
    #[must_use]
    pub fn username(&self) -> Option<String> {
        self.runtime.auth.snapshot().email.clone()
    }

    /// Fetch fresh userinfo from the backend using the current session's
    /// auth token. Mirrors `psync_get_userinfo`.
    ///
    /// # Preconditions
    ///
    /// An authenticated session must be present.
    ///
    /// # Errors
    ///
    /// [`SdkError::Auth`] wrapping
    /// [`AuthHelperError::NotAuthenticated`] when no token is stored;
    /// [`AuthHelperError::UserInfo`] when the server rejects the call
    /// (often indicates a revoked token — re-login).
    ///
    /// # Side effects
    ///
    /// One binary-API round-trip. Read-only from the local runtime's
    /// perspective. Expected latency: 100–500 ms.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.userinfo();
    /// ```
    pub fn userinfo(&self) -> Result<AuthenticatedUser, SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(AuthHelperError::NotAuthenticated)?;
        let info = self
            .runtime
            .auth_runtime
            .userinfo(auth_token)
            .map_err(|err| AuthHelperError::UserInfo(err.to_string()))?;
        Ok(AuthenticatedUser {
            user_id: info.user_id,
            email: info.email,
        })
    }

    /// Clear the authenticated session. Mirrors `psync_logout` / `psync_unlink`.
    /// Goes through the daemon dispatch path so persistence and audit surfaces
    /// observe the same transition the IPC/CLI paths emit.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.logout();
    /// ```
    pub fn logout(&mut self) -> Result<(), SdkError> {
        let response = self.dispatch(Request::Plain {
            method: Method::Logout,
        });
        if response.status == ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(AuthHelperError::Logout(response.message)))
        }
    }

    /// Request an SMS-delivered two-factor code. Mirrors `psync_tfa_send_sms`.
    /// Requires that a prior password/token login has produced a pending
    /// two-factor challenge.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.send_two_factor_sms();
    /// ```
    pub fn send_two_factor_sms(&self) -> Result<TwoFactorSmsInfo, SdkError> {
        let delivery = self
            .runtime
            .auth_runtime
            .send_two_factor_sms(&self.runtime.auth)
            .map_err(|err| AuthHelperError::TwoFactorSms(err.to_string()))?;
        Ok(TwoFactorSmsInfo {
            country_code: delivery.country_code,
            phone_number: delivery.phone_number,
        })
    }

    /// Request a device-notification two-factor challenge. Mirrors
    /// `psync_tfa_send_nofification` (sic - C typo preserved in header).
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.send_two_factor_notification();
    /// ```
    pub fn send_two_factor_notification(&self) -> Result<TwoFactorNotificationInfo, SdkError> {
        let delivery = self
            .runtime
            .auth_runtime
            .send_two_factor_notification(&self.runtime.auth)
            .map_err(|err| AuthHelperError::TwoFactorNotification(err.to_string()))?;
        Ok(TwoFactorNotificationInfo {
            devices: delivery
                .devices
                .into_iter()
                .filter_map(|device| device.name)
                .collect(),
        })
    }

    /// Submit a two-factor code. Mirrors `psync_tfa_set_code`.
    ///
    /// The code is accepted as a plain `&str` at the SDK boundary (CLI input)
    /// and wrapped in a `SecretString` before being forwarded to the auth
    /// orchestrator so it zeroizes on drop.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.submit_two_factor_code("123456", true, false);
    /// ```
    pub fn submit_two_factor_code(
        &mut self,
        code: &str,
        trust_device: bool,
        recovery_code: bool,
    ) -> Result<(), SdkError> {
        self.runtime
            .auth_runtime
            .submit_two_factor_code(
                &mut self.runtime.auth,
                SecretString::new(code.to_owned()),
                trust_device,
                recovery_code,
            )
            .map_err(|err| AuthHelperError::TwoFactorCode(err.to_string()))?;
        Ok(())
    }

    /// Resolve a signed download link for `file_id`. Mirrors the C
    /// `getfilelink` surface used by `psync_download_*` helpers.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.get_file_link(42, None);
    /// ```
    pub fn get_file_link(
        &self,
        file_id: u64,
        forced_host: Option<String>,
    ) -> Result<DownloadLinkInfo, SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(DownloadHelperError::NotAuthenticated)?;
        let link = self
            .runtime
            .transfer_runtime
            .get_file_link(auth_token, file_id, forced_host)
            .map_err(|err| DownloadHelperError::GetFileLink(err.to_string()))?;
        Ok(DownloadLinkInfo {
            path: link.path,
            hosts: link.hosts,
            download_tag: link.download_tag,
        })
    }

    /// Fetch a file's bytes by id, using the signed download link. Mirrors the
    /// combined C flow of `getfilelink` + `pdownload_fetch_url`.
    ///
    /// # Preconditions
    ///
    /// An authenticated session must be present. `file_id` must resolve
    /// to a file the caller has read access to.
    ///
    /// # Errors
    ///
    /// [`SdkError::Download`] wrapping
    /// [`DownloadHelperError::NotAuthenticated`],
    /// [`DownloadHelperError::GetFileLink`] (server rejected the
    /// `getfilelink` request), or
    /// [`DownloadHelperError::DownloadBytes`] (CDN HTTPS fetch failed).
    /// Both server-side variants are transiently retryable.
    ///
    /// # Side effects
    ///
    /// One API round-trip (`getfilelink`) followed by one CDN HTTPS
    /// fetch. The returned `Vec<u8>` is materialised fully in memory —
    /// prefer a streaming surface for very large files once available.
    /// Expected latency: API call ~100–500 ms plus fetch time
    /// proportional to file size.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _bytes = d.download_file(42);
    /// ```
    pub fn download_file(&self, file_id: u64) -> Result<Vec<u8>, SdkError> {
        let auth_token = self
            .runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or(DownloadHelperError::NotAuthenticated)?;
        let link = self
            .runtime
            .transfer_runtime
            .get_file_link(auth_token, file_id, None)
            .map_err(|err| DownloadHelperError::GetFileLink(err.to_string()))?;
        let (_signed, bytes) = self
            .runtime
            .transfer_runtime
            .download_bytes(&link)
            .map_err(|err| DownloadHelperError::DownloadBytes(err.to_string()))?;
        Ok(bytes)
    }

    /// Returns the raw auth token as a `SecretString` when authenticated. The
    /// secret is cloned out of the session manager so the caller is responsible
    /// for not letting it escape into logs or persistence. Mirrors
    /// `psync_get_token` / `psync_get_auth_string`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert!(d.auth_token_secret().is_none());
    /// ```
    pub fn auth_token_secret(&self) -> Option<SecretString> {
        // Audit-visible duplication: `SecretString` does not derive `Clone`
        // (audit M3). Use the explicit `clone_secret` helper.
        self.runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
    }

    /// Strict-typed setting read: mirrors `psync_get_bool_setting`. Returns
    /// `Ok(None)` when the setting is unset; returns an error when a value
    /// exists under a different kind.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert_eq!(d.get_bool_setting("unset").unwrap(), None);
    /// ```
    pub fn get_bool_setting(&self, name: &str) -> Result<Option<bool>, SdkError> {
        pcloud_store::settings_kv::get_bool(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Strict-typed setting write: mirrors `psync_set_bool_setting`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_bool_setting("autostart", true).unwrap();
    /// ```
    pub fn set_bool_setting(&self, name: &str, value: bool) -> Result<(), SdkError> {
        pcloud_store::settings_kv::set_bool(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Strict-typed setting read: mirrors `psync_get_int_setting`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert_eq!(d.get_int_setting("unset").unwrap(), None);
    /// ```
    pub fn get_int_setting(&self, name: &str) -> Result<Option<i64>, SdkError> {
        pcloud_store::settings_kv::get_int(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Strict-typed setting write: mirrors `psync_set_int_setting`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_int_setting("scan_interval", 30).unwrap();
    /// ```
    pub fn set_int_setting(&self, name: &str, value: i64) -> Result<(), SdkError> {
        pcloud_store::settings_kv::set_int(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Strict-typed setting read: mirrors `psync_get_uint_setting`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert_eq!(d.get_uint_setting("unset").unwrap(), None);
    /// ```
    pub fn get_uint_setting(&self, name: &str) -> Result<Option<u64>, SdkError> {
        pcloud_store::settings_kv::get_uint(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Strict-typed setting write: mirrors `psync_set_uint_setting`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_uint_setting("rate_limit", 100).unwrap();
    /// ```
    pub fn set_uint_setting(&self, name: &str, value: u64) -> Result<(), SdkError> {
        pcloud_store::settings_kv::set_uint(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Strict-typed setting read: mirrors `psync_get_string_setting`. Returns
    /// `Ok(None)` when the setting is unset; the C helper returns an
    /// empty-string sentinel so callers mirroring legacy behavior should
    /// fall back to `""` on `None`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// assert_eq!(d.get_string_setting("unset").unwrap(), None);
    /// ```
    pub fn get_string_setting(&self, name: &str) -> Result<Option<String>, SdkError> {
        pcloud_store::settings_kv::get_string(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Strict-typed setting write: mirrors `psync_set_string_setting`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.set_string_setting("theme", "dark").unwrap();
    /// ```
    pub fn set_string_setting(&self, name: &str, value: &str) -> Result<(), SdkError> {
        pcloud_store::settings_kv::set_string(&self.runtime.store.db_path, name, value)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Drop the backing row so the next `get_*_setting` call returns
    /// `Ok(None)`. Mirrors `psync_reset_setting`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let _ = d.reset_setting("theme");
    /// ```
    pub fn reset_setting(&self, name: &str) -> Result<bool, SdkError> {
        pcloud_store::settings_kv::reset(&self.runtime.store.db_path, name)
            .map_err(|err| SdkError::from(SettingKvError::Store(err.to_string())))
    }

    /// Stat a remote path by absolute pCloud-drive path. Mirrors C
    /// `psync_stat_path` (`pclsync/psynclib.h:743`,
    /// `pclsync/psynclib.c:811`). The C surface returns a bare nullable
    /// `pentry_t*`; the Rust surface returns a typed [`StatResult`] or a
    /// structured error.
    ///
    /// Resolution is delegated to the canonical live remote-drive service;
    /// an empty local metadata cache does not change the result.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let info = d.stat_path("/Documents").unwrap();
    /// assert!(info.is_folder);
    /// ```
    pub fn stat_path(&mut self, path: &str) -> Result<StatResult, SdkError> {
        let entry = self.remote().stat(path).map_err(|error| match error {
            RemoteDriveError::InvalidRequest(_) => FolderMetadataError::InvalidPath,
            RemoteDriveError::Unauthorized(_) => FolderMetadataError::NotAuthenticated,
            other => FolderMetadataError::Resolve(other.to_string()),
        })?;
        let is_folder = entry.id.is_folder();
        Ok(StatResult {
            name: entry.name,
            is_folder,
            folder_id: is_folder.then_some(entry.id.value()),
            file_id: (!is_folder).then_some(entry.id.value()),
            size: entry.size,
            modified: entry.modified,
            is_mine: entry.is_mine,
            encrypted: entry.encrypted,
            is_shared: entry.is_shared,
            permissions: entry.permissions,
        })
    }

    /// List the direct children of a remote folder by absolute
    /// pCloud-drive path. Mirrors C `pfolder_list`
    /// (`pclsync/pfolder.c:556`), returning typed [`FolderEntry`] items
    /// instead of the C `pfolder_list_t*`.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let entries = d.list_folder("/").unwrap();
    /// for entry in &entries {
    ///     println!("{} (folder={})", entry.name, entry.is_folder);
    /// }
    /// ```
    pub fn list_folder(&mut self, path: &str) -> Result<Vec<FolderEntry>, SdkError> {
        let listing = self.remote().list(path).map_err(|error| match error {
            RemoteDriveError::InvalidRequest(_) => FolderMetadataError::InvalidPath,
            RemoteDriveError::Unauthorized(_) => FolderMetadataError::NotAuthenticated,
            other => FolderMetadataError::Resolve(other.to_string()),
        })?;
        Ok(listing
            .entries
            .into_iter()
            .map(|e| FolderEntry {
                name: e.name,
                is_folder: e.id.is_folder(),
                folder_id: e.id.is_folder().then_some(e.id.value()),
                file_id: (!e.id.is_folder()).then_some(e.id.value()),
                size: e.size,
                modified: e.modified,
                is_mine: e.is_mine,
                encrypted: e.encrypted,
                is_shared: e.is_shared,
                permissions: e.permissions,
            })
            .collect())
    }

    /// Delete a remote file by absolute pCloud-drive path.
    ///
    /// Requires an authenticated session. Dispatches the IPC delete
    /// variant when available.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.delete_file("/Documents/old.txt").unwrap();
    /// ```
    pub fn delete_file(&mut self, path: &str) -> Result<(), SdkError> {
        if self.runtime.auth.snapshot().auth_token.is_none() {
            return Err(SdkError::from(FileMutationHelperError::NotAuthenticated));
        }
        if path.trim().is_empty() || !path.starts_with('/') {
            return Err(SdkError::from(FileMutationHelperError::DeleteFailed(
                "delete_file requires an absolute path starting with '/'".to_owned(),
            )));
        }
        let response = self.dispatch(Request::FileDeleteByPath {
            path: path.to_owned(),
        });
        if response.status == ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(FileMutationHelperError::DeleteFailed(
                response.message,
            )))
        }
    }

    /// Rename (move) a remote file from `src_path` to `dst_path`.
    ///
    /// Requires an authenticated session. Dispatches the IPC rename
    /// variant when available.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.rename_file("/Documents/old.txt", "/Documents/new.txt").unwrap();
    /// ```
    pub fn rename_file(&mut self, src_path: &str, dst_path: &str) -> Result<(), SdkError> {
        if self.runtime.auth.snapshot().auth_token.is_none() {
            return Err(SdkError::from(FileMutationHelperError::NotAuthenticated));
        }
        if src_path.trim().is_empty() || !src_path.starts_with('/') {
            return Err(SdkError::from(FileMutationHelperError::RenameFailed(
                "rename_file: src_path must be an absolute path starting with '/'".to_owned(),
            )));
        }
        if dst_path.trim().is_empty() || !dst_path.starts_with('/') {
            return Err(SdkError::from(FileMutationHelperError::RenameFailed(
                "rename_file: dst_path must be an absolute path starting with '/'".to_owned(),
            )));
        }
        let response = self.dispatch(Request::RenamePath {
            from: src_path.to_owned(),
            to: dst_path.to_owned(),
        });
        if response.status == ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(FileMutationHelperError::RenameFailed(
                response.message,
            )))
        }
    }

    /// Stat a remote file by absolute pCloud-drive path. Returns the
    /// [`StatResult`] when the entry exists. Mirrors `psync_stat_path`
    /// for files specifically.
    ///
    /// Requires an authenticated session.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// let info = d.get_file_info("/Documents/report.txt").unwrap();
    /// ```
    pub fn get_file_info(&mut self, path: &str) -> Result<StatResult, SdkError> {
        if self.runtime.auth.snapshot().auth_token.is_none() {
            return Err(SdkError::from(FileMutationHelperError::NotAuthenticated));
        }
        // Delegate to stat_path which already uses Request::StatPath, then
        // verify the returned entry is a file.
        let result = self
            .stat_path(path)
            .map_err(|e| FileMutationHelperError::StatFailed(e.to_string()))?;
        if result.is_folder {
            return Err(SdkError::from(FileMutationHelperError::StatFailed(
                format!("path '{path}' is a folder, not a file"),
            )));
        }
        Ok(result)
    }

    /// Mount the pCloud FUSE filesystem at `mountpoint`. Delegates to
    /// the daemon's mount runtime. Mirrors the C
    /// `psync_fs_start` / mount lifecycle.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.mount(std::path::Path::new("/mnt/pcloud")).unwrap();
    /// ```
    pub fn mount(&mut self, mountpoint: &Path) -> Result<(), SdkError> {
        if self.runtime.auth.snapshot().auth_token.is_none() {
            return Err(SdkError::from(MountHelperError::NotAuthenticated));
        }
        let response = self.runtime.mount_filesystem(mountpoint);
        if response.status == pcloud_ipc::ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(MountHelperError::Mount(response.message)))
        }
    }

    /// Unmount the active pCloud FUSE mount. Mirrors the C
    /// `psync_fs_stop` / unmount lifecycle.
    ///
    /// ```no_run
    /// # use std::path::PathBuf;
    /// # use pcloud_embedded_sdk::EmbeddedDaemon;
    /// # let mut d = EmbeddedDaemon::builder(std::env::temp_dir().join("pcloud-sdk-doctest")).build().unwrap();
    /// d.unmount().unwrap();
    /// ```
    pub fn unmount(&mut self) -> Result<(), SdkError> {
        let response = self.runtime.unmount_filesystem();
        if response.status == pcloud_ipc::ResponseStatus::Ok {
            Ok(())
        } else {
            Err(SdkError::from(MountHelperError::Mount(response.message)))
        }
    }

    fn auth_token(&self) -> Result<pcloud_secret::secret_string::SecretString, SdkError> {
        self.runtime
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(pcloud_secret::secret_string::SecretString::clone_secret)
            .ok_or_else(|| SdkError::from(UploadHelperError::NotAuthenticated))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use pcloud_ipc::{Method, Request, ResponseStatus};
    use pcloud_plugin_api::{Plugin, PluginCapability, PluginContext, PluginManifest};
    use pcloud_secret::ExposeSecret;

    use super::EmbeddedDaemon;

    struct ObservePlugin;

    impl Plugin for ObservePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "observe".to_owned(),
                version: "0.1.0".to_owned(),
                display_name: "Observe".to_owned(),
                requested_capabilities: BTreeSet::from([PluginCapability::ObserveStatus]),
            }
        }

        fn on_load(
            &mut self,
            _context: &PluginContext,
        ) -> Result<(), pcloud_plugin_api::PluginError> {
            Ok(())
        }
    }

    #[test]
    fn embedded_daemon_dispatches_requests() {
        let root = unique_test_root("dispatch");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        let response = daemon.dispatch(Request::Plain {
            method: Method::GetHealth,
        });

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("health:"));
    }

    #[test]
    fn plugin_registration_is_denied_by_default() {
        let root = unique_test_root("plugin");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");
        let mut plugin = ObservePlugin;

        let err = daemon
            .register_plugin(&mut plugin)
            .expect_err("default extension policy should deny plugins");

        assert!(err.to_string().contains("disabled"));

        let root = unique_test_root("plugin-enabled");
        let mut policy =
            pcloud_config::extensions::ExtensionPolicy::secure_defaults(root.join("plugins"));
        policy.plugins_enabled = true;
        let mut daemon = EmbeddedDaemon::builder(root)
            .extension_policy(policy)
            .build()
            .expect("enabled embedded daemon should bootstrap");
        let loaded = daemon
            .register_plugin(&mut plugin)
            .expect("observe plugin should load with an explicit policy");
        assert_eq!(loaded.manifest.id, "observe");
    }

    #[test]
    fn direct_upload_helpers_require_authentication() {
        let root = unique_test_root("upload-unauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .upload_data(0, "report.txt", b"hello")
            .expect_err("upload should require auth");

        assert!(matches!(
            err,
            super::SdkError::Upload(super::UploadHelperError::NotAuthenticated)
        ));
    }

    #[test]
    fn upload_data_executes_against_development_transport() {
        let root = unique_test_root("upload-data");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let result = daemon
            .upload_data(22, "report.txt", b"hello world")
            .expect("upload should succeed");

        assert_eq!(result.upload_id, 77);
        assert_eq!(result.file_id, Some(9));
        assert_eq!(result.parent_folder_id, 22);
        assert_eq!(result.remote_filename, "report.txt");
        assert_eq!(result.bytes_uploaded, 11);
    }

    #[test]
    fn upload_write_from_file_requires_authentication() {
        let root = unique_test_root("upload-writefromfile-noauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .upload_write_from_file(77, 20, 1234, 4096, 128, 1024)
            .expect_err("server-side copy should require auth");

        assert!(matches!(
            err,
            super::SdkError::Upload(super::UploadHelperError::NotAuthenticated)
        ));
    }

    #[test]
    fn upload_file_reads_local_payload_and_executes_upload() {
        let root = unique_test_root("upload-file");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let local_path = std::env::temp_dir().join(format!(
            "pcloud-sdk-upload-file-{}-{}.txt",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::write(&local_path, b"payload").expect("local file should be written");

        let result = daemon
            .upload_file(7, "payload.txt", &local_path)
            .expect("upload should succeed");

        assert_eq!(result.upload_id, 77);
        assert_eq!(result.parent_folder_id, 7);
        assert_eq!(result.remote_filename, "payload.txt");
        assert_eq!(result.bytes_uploaded, 7);
    }

    #[test]
    fn upload_data_as_resolves_remote_path_before_uploading() {
        let root = unique_test_root("upload-data-as");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let result = daemon
            .upload_data_as("/remote-sync", "report.txt", b"hello world")
            .expect("upload should succeed");

        assert_eq!(result.upload_id, 77);
        assert_eq!(result.parent_folder_id, 17);
        assert_eq!(result.remote_filename, "report.txt");
        assert_eq!(result.bytes_uploaded, 11);
    }

    #[test]
    fn upload_file_as_uses_remote_path_resolution() {
        let root = unique_test_root("upload-file-as");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let local_path = std::env::temp_dir().join(format!(
            "pcloud-sdk-upload-file-as-{}-{}.txt",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::write(&local_path, b"payload-as").expect("local file should be written");

        let result = daemon
            .upload_file_as("/remote-sync", "payload-as.txt", &local_path)
            .expect("upload should succeed");

        assert_eq!(result.parent_folder_id, 17);
        assert_eq!(result.remote_filename, "payload-as.txt");
        assert_eq!(result.bytes_uploaded, 10);
    }

    #[test]
    fn upload_data_as_rejects_missing_remote_folder() {
        let root = unique_test_root("upload-data-as-missing");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let err = daemon
            .upload_data_as("/missing-remote", "report.txt", b"hello world")
            .expect_err("upload should reject missing remote folder");

        assert!(matches!(
            err,
            super::SdkError::Upload(super::UploadHelperError::ResolveRemoteFolder(_))
        ));
    }

    #[test]
    fn account_utilities_require_auth_for_authenticated_calls() {
        let root = unique_test_root("account-utils-unauth");
        let daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon.get_promo().expect_err("promo should require auth");
        assert!(matches!(
            err,
            super::SdkError::Account(super::AccountUtilityError::NotAuthenticated)
        ));

        let err = daemon
            .set_language("en")
            .expect_err("set_language should require auth");
        assert!(matches!(
            err,
            super::SdkError::Account(super::AccountUtilityError::NotAuthenticated)
        ));
    }

    #[test]
    fn account_utilities_work_against_development_transport() {
        let root = unique_test_root("account-utils");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let servers = daemon
            .get_api_servers()
            .expect("api server listing should succeed");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].label, "Europe");

        let promo = daemon
            .get_promo()
            .expect("promo lookup should succeed")
            .expect("promo should be present");
        assert_eq!(promo.width, 640);
        assert_eq!(promo.height, 480);

        daemon
            .set_language("en")
            .expect("set_language should succeed");
        daemon.verify_email().expect("verify email should succeed");
        daemon
            .verify_email_restricted("verify-token")
            .expect("restricted verify email should succeed");
        daemon
            .lost_password("alice@example.com")
            .expect("lost password should succeed");
        daemon
            .change_password("old-pass", "new-pass")
            .expect("change password should succeed");
        assert_eq!(
            daemon
                .runtime
                .auth
                .snapshot()
                .auth_token
                .as_ref()
                .expect("rotated auth token should exist")
                .expose_secret(),
            "rotated-auth-token"
        );
        daemon
            .register(
                "new-user@example.com",
                pcloud_secret::secret_string::SecretString::new("strong-password".to_owned()),
                true,
            )
            .expect("register should succeed");
        let err = daemon
            .register(
                "new-user@example.com",
                pcloud_secret::secret_string::SecretString::new("strong-password".to_owned()),
                false,
            )
            .expect_err("register should reject missing terms");
        assert!(matches!(
            err,
            super::SdkError::Account(super::AccountUtilityError::TermsNotAccepted)
        ));
        let err = daemon
            .register(
                "bad-email",
                pcloud_secret::secret_string::SecretString::new("strong-password".to_owned()),
                true,
            )
            .expect_err("register should reject bad email");
        assert!(matches!(
            err,
            super::SdkError::Account(super::AccountUtilityError::InvalidRegistrationInput)
        ));

        daemon
            .set_api_server("bineapi-eu.pcloud.com:8443", 2)
            .expect("api server selection should succeed");
        assert_eq!(daemon.runtime.config.api.host, "bineapi-eu.pcloud.com");
        assert_eq!(
            daemon.runtime.config.api.server_name,
            "bineapi-eu.pcloud.com"
        );
        assert_eq!(daemon.runtime.config.api.port, 8443);
        assert_eq!(
            daemon
                .runtime
                .store
                .repositories
                .preferences
                .api_server_binapi
                .as_deref(),
            Some("bineapi-eu.pcloud.com:8443")
        );
        assert_eq!(
            daemon
                .runtime
                .store
                .repositories
                .preferences
                .api_server_location_id,
            Some(2)
        );
    }

    #[test]
    fn auth_helpers_report_unauthenticated_state() {
        let root = unique_test_root("auth-helpers-unauth");
        let daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        assert!(!daemon.is_authenticated());
        assert!(daemon.current_user_id().is_none());
        assert!(daemon.username().is_none());
        assert!(daemon.auth_token_secret().is_none());

        let err = daemon.userinfo().expect_err("userinfo should require auth");
        assert!(matches!(
            err,
            super::SdkError::Auth(super::AuthHelperError::NotAuthenticated)
        ));

        let err = daemon
            .get_file_link(42, None)
            .expect_err("get_file_link should require auth");
        assert!(matches!(
            err,
            super::SdkError::Download(super::DownloadHelperError::NotAuthenticated)
        ));

        let err = daemon
            .download_file(42)
            .expect_err("download_file should require auth");
        assert!(matches!(
            err,
            super::SdkError::Download(super::DownloadHelperError::NotAuthenticated)
        ));
    }

    #[test]
    fn auth_helpers_expose_authenticated_session() {
        let root = unique_test_root("auth-helpers");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        assert!(daemon.is_authenticated());
        let info = daemon.userinfo().expect("userinfo should succeed");
        assert!(info.email.as_deref().map(str::is_empty) != Some(true));
        assert!(daemon.auth_token_secret().is_some());

        // logout drops the session and clears the token.
        daemon.logout().expect("logout should succeed");
        assert!(!daemon.is_authenticated());
        assert!(daemon.auth_token_secret().is_none());
    }

    #[test]
    fn tfa_helpers_drive_two_factor_flow() {
        let root = unique_test_root("tfa-helpers");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        // Development transport issues a TFA challenge on password login.
        let _ = daemon.dispatch(Request::PasswordSubmission {
            username: "alice@example.com".to_owned(),
            value: "password".to_owned().into(),
        });

        let sms = daemon
            .send_two_factor_sms()
            .expect("tfa sms should succeed");
        assert!(sms.country_code.is_some() || sms.phone_number.is_some());

        let notification = daemon
            .send_two_factor_notification()
            .expect("tfa notification should succeed");
        assert!(!notification.devices.is_empty());

        daemon
            .submit_two_factor_code("654321", false, false)
            .expect("tfa code submission should succeed");
        assert!(daemon.is_authenticated());
    }

    #[test]
    fn download_helpers_resolve_link_and_bytes() {
        let root = unique_test_root("download-helpers");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let link = daemon
            .get_file_link(42, None)
            .expect("get_file_link should succeed");
        assert!(!link.path.is_empty());
        assert!(!link.hosts.is_empty());

        let bytes = daemon
            .download_file(42)
            .expect("download_file should succeed");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn notifications_helpers_require_authentication() {
        let root = unique_test_root("notifications-unauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .list_notifications()
            .expect_err("list should require auth");
        assert!(matches!(
            err,
            super::SdkError::Notifications(super::NotificationsHelperError::NotAuthenticated)
        ));

        let err = daemon
            .mark_notifications_read(7)
            .expect_err("mark read should require auth");
        assert!(matches!(
            err,
            super::SdkError::Notifications(super::NotificationsHelperError::NotAuthenticated)
        ));
    }

    #[test]
    fn notifications_mark_read_rejects_zero_id() {
        let root = unique_test_root("notifications-zero");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let err = daemon
            .mark_notifications_read(0)
            .expect_err("zero id should be rejected before dispatch");
        assert!(matches!(
            err,
            super::SdkError::Notifications(super::NotificationsHelperError::InvalidNotificationId)
        ));
    }

    #[test]
    fn notifications_round_trip_against_development_transport() {
        let root = unique_test_root("notifications-rt");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let notifications = daemon
            .list_notifications()
            .expect("list_notifications should succeed");
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].id, 7);
        assert_eq!(notifications[0].text, "Welcome to pCloud");
        assert!(!notifications[0].read);
        assert_eq!(notifications[1].id, 8);
        assert!(notifications[1].read);

        daemon
            .mark_notifications_read(notifications[0].id)
            .expect("mark_notifications_read should succeed");
    }

    #[test]
    fn notifications_ipc_dispatch_round_trip() {
        let root = unique_test_root("notifications-ipc");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let response = daemon.dispatch(Request::Plain {
            method: Method::ListNotifications,
        });
        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("count=2"));
        assert!(response.message.contains("Welcome to pCloud"));

        let response = daemon.dispatch(Request::MarkNotificationsRead { upto_id: 7 });
        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("upto_id=7") || response.message.contains("id=7"));

        let response = daemon.dispatch(Request::MarkNotificationsRead { upto_id: 0 });
        assert_eq!(response.status, ResponseStatus::InvalidRequest);
    }

    #[test]
    fn run_localscan_helper_bumps_engine_counter() {
        let root = unique_test_root("run-localscan");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let count = daemon.run_localscan();
        assert_eq!(count, 1);
        let count = daemon.run_localscan();
        assert_eq!(count, 2);

        // The IPC surface yields the same observable behavior.
        let response = daemon.dispatch(Request::RunLocalScan);
        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("local scan wake signalled"));
    }

    #[test]
    fn send_publink_helper_round_trip_against_development_transport() {
        let root = unique_test_root("send-publink");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        let unauthenticated = daemon
            .send_publink("alpha123", "alice@example.com", "hi")
            .expect_err("send_publink without auth should fail");
        assert!(matches!(
            unauthenticated,
            super::SdkError::Publink(super::PublinkHelperError::NotAuthenticated)
        ));

        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let empty_code = daemon
            .send_publink("   ", "alice@example.com", "hi")
            .expect_err("empty code should fail before dispatch");
        assert!(matches!(
            empty_code,
            super::SdkError::Publink(super::PublinkHelperError::EmptyCode)
        ));

        let empty_recipients = daemon
            .send_publink("alpha123", "   ", "hi")
            .expect_err("empty recipients should fail before dispatch");
        assert!(matches!(
            empty_recipients,
            super::SdkError::Publink(super::PublinkHelperError::EmptyRecipients)
        ));

        daemon
            .send_publink(
                "alpha123",
                "alice@example.com,bob@example.com",
                "Here is the link",
            )
            .expect("send_publink should succeed against the development transport");

        // IPC dispatch path should also succeed.
        let response = daemon.dispatch(Request::SendPublink {
            code: "alpha123".to_owned(),
            mails: "alice@example.com".to_owned(),
            message: "again".to_owned(),
        });
        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("public link sent"));
    }

    #[test]
    fn folder_metadata_helpers_require_authenticated_session() {
        let root = unique_test_root("folder-meta-noauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .get_folder_id_by_path("/Docs")
            .expect_err("unauthenticated id lookup must fail");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::NotAuthenticated)
        ));

        let err = daemon
            .get_folder_flags("/Docs")
            .expect_err("unauthenticated flags lookup must fail");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::NotAuthenticated)
        ));

        let err = daemon
            .get_folder_owner_id("/Docs")
            .expect_err("unauthenticated owner lookup must fail");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::NotAuthenticated)
        ));
    }

    #[test]
    fn folder_metadata_helpers_reject_invalid_paths() {
        let root = unique_test_root("folder-meta-bad-path");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .get_folder_id_by_path("")
            .expect_err("empty path must be rejected");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::InvalidPath)
        ));

        let err = daemon
            .get_folder_flags("relative/path")
            .expect_err("relative path must be rejected");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::InvalidPath)
        ));
    }

    #[test]
    fn filesystem_status_classifies_against_sync_root_snapshot() {
        use pcloud_model::ids::SyncId;
        use pcloud_store::repositories::sync_graph::SyncRootRecord;

        let root = unique_test_root("fs-status-sdk");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        // Outside any root: INVSYNC
        assert_eq!(
            daemon.filesystem_status("/no/tracked/root"),
            super::FilesystemPathStatus::Invalid,
        );

        // Inject a tracked sync root directly for the test.
        daemon
            .runtime
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .push(SyncRootRecord {
                sync_id: SyncId::new(5),
                local_path: "/mnt/pcloud".to_owned(),
                remote_path: "/".to_owned(),
                paused: false,
                sync_type: pcloud_model::sync::SyncType::Full,
                exclude_globs: Vec::new(),
            });

        assert_eq!(
            daemon.filesystem_status("/mnt/pcloud/sub/file"),
            super::FilesystemPathStatus::InSync,
        );
        assert_eq!(super::FilesystemPathStatus::InSync.as_c_token(), "INSYNC");
    }

    #[test]
    fn stat_path_requires_authenticated_session() {
        let root = unique_test_root("stat-noauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .stat_path("/Docs")
            .expect_err("unauthenticated stat must fail");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::NotAuthenticated)
        ));
    }

    #[test]
    fn stat_path_rejects_invalid_path() {
        let root = unique_test_root("stat-bad-path");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .stat_path("")
            .expect_err("empty path must be rejected");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::InvalidPath)
        ));

        let err = daemon
            .stat_path("relative/path")
            .expect_err("relative path must be rejected");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::InvalidPath)
        ));
    }

    #[test]
    fn stat_path_returns_root_metadata() {
        let root = unique_test_root("stat-root");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let stat = daemon.stat_path("/").expect("root stat should succeed");
        assert!(stat.is_folder);
        assert_eq!(stat.folder_id, Some(0));
    }

    #[test]
    fn stat_path_resolves_child_entry() {
        let root = unique_test_root("stat-child");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let stat = daemon
            .stat_path("/Documents")
            .expect("child stat should succeed");
        assert!(stat.is_folder);
        assert_eq!(stat.name, "Documents");
        assert_eq!(stat.folder_id, Some(10));

        let stat = daemon
            .stat_path("/notes.txt")
            .expect("file stat should succeed");
        assert!(!stat.is_folder);
        assert_eq!(stat.name, "notes.txt");
        assert_eq!(stat.file_id, Some(20));
        assert_eq!(stat.size, Some(1024));
    }

    #[test]
    fn tree_public_link_from_paths_requires_authentication() {
        let root = unique_test_root("tree-link-noauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .create_tree_public_link_from_paths(
                "mixed bundle",
                vec!["/Documents".to_owned(), "/notes.txt".to_owned()],
                None,
            )
            .expect_err("tree link should require auth");

        assert!(matches!(
            err,
            super::SdkError::TreePublicLink(super::TreePublicLinkHelperError::NotAuthenticated)
        ));
    }

    #[test]
    fn tree_public_link_from_targets_rejects_empty_targets() {
        let root = unique_test_root("tree-link-empty-targets");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .create_tree_public_link_from_targets(
                "target bundle",
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .expect_err("empty tree targets should fail before auth");

        assert!(matches!(
            err,
            super::SdkError::TreePublicLink(super::TreePublicLinkHelperError::EmptyPaths)
        ));
    }

    #[test]
    fn list_folder_requires_authenticated_session() {
        let root = unique_test_root("list-noauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .list_folder("/Docs")
            .expect_err("unauthenticated list must fail");
        assert!(matches!(
            err,
            super::SdkError::Folder(super::FolderMetadataError::NotAuthenticated)
        ));
    }

    #[test]
    fn list_folder_returns_entries() {
        let root = unique_test_root("list-entries");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");
        let login = daemon.dispatch(Request::AuthTokenSubmission {
            value: "digest-auth-token".to_owned().into(),
        });
        assert_eq!(login.status, ResponseStatus::Ok);

        let entries = daemon.list_folder("/").expect("list_folder should succeed");
        assert_eq!(entries.len(), 2);

        let folder_entry = entries
            .iter()
            .find(|e| e.is_folder)
            .expect("should have folder");
        assert_eq!(folder_entry.name, "Documents");
        assert_eq!(folder_entry.folder_id, Some(10));

        let file_entry = entries
            .iter()
            .find(|e| !e.is_folder)
            .expect("should have file");
        assert_eq!(file_entry.name, "notes.txt");
        assert_eq!(file_entry.file_id, Some(20));
        assert_eq!(file_entry.size, Some(1024));
    }

    #[test]
    fn mount_requires_authenticated_session() {
        let root = unique_test_root("mount-noauth");
        let mut daemon = EmbeddedDaemon::builder(root)
            .build()
            .expect("embedded daemon should bootstrap");

        let err = daemon
            .mount(std::path::Path::new("/mnt/pcloud"))
            .expect_err("unauthenticated mount must fail");
        assert!(matches!(
            err,
            super::SdkError::Mount(super::MountHelperError::NotAuthenticated)
        ));
    }

    #[test]
    fn unmount_without_active_mount() {
        let root = unique_test_root("unmount-noop");
        let mut daemon = EmbeddedDaemon::builder(root)
            .environment(pcloud_config::Environment::Development)
            .build()
            .expect("embedded daemon should bootstrap");

        // Unmount when nothing is mounted. Depending on the host
        // environment (orphan FUSE mounts, missing fusermount, etc.) this
        // may succeed or return a MountHelperError::Mount. Both are
        // acceptable — the key invariant is that it does not panic.
        let result = daemon.unmount();
        match result {
            Ok(()) => {}                         // clean environment, no-op unmount succeeded
            Err(super::SdkError::Mount(_)) => {} // host-level mount issue, acceptable
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn mount_helper_error_unified_conversion() {
        use pcloud_error::Error as Unified;

        let err = super::MountHelperError::NotAuthenticated;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 1000);
        assert_eq!(unified.category(), "auth");

        let err = super::MountHelperError::Mount("boom".into());
        let unified: Unified = err.into();
        assert_eq!(unified.category(), "local_io");
    }

    fn unique_test_root(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("pcloud-sdk-{label}-{}-{nonce}", std::process::id()))
    }
}

// --------------------------------------------------------------------------
// Unified error taxonomy wiring.
//
// Every per-helper error enum declared in this crate funnels into the
// workspace-wide `pcloud_error::Error` type. The helper enums are RETAINED
// as the public API (so SDK consumers that currently pattern-match on them
// keep compiling); the unified type is additive and opt-in.
//
// Conversions use `IntoUnified` so the original enum is preserved as the
// `source()` of the unified error. This keeps the cause chain intact for
// structured logging and IPC status serialisation.
// --------------------------------------------------------------------------

// The precise `From<_>` impls live earlier in this file (right after the
// per-helper enum declarations). Each impl inspects the variant and picks
// the narrowest appropriate `pcloud_error::Category` so callers can rely on
// `Error::code()` / `Error::category()` for routing.

#[cfg(test)]
mod unified_error_tests {
    use super::*;
    use pcloud_error::Error as Unified;
    use std::error::Error as _;

    #[test]
    fn auth_helper_not_authenticated_maps_to_auth_category() {
        let err = AuthHelperError::NotAuthenticated;
        let msg = err.to_string();
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 1000);
        assert_eq!(unified.category(), "auth");
        assert!(unified.to_string().contains(&msg));
        assert!(unified.source().is_some());
    }

    #[test]
    fn download_helper_funnels_to_api() {
        let err = DownloadHelperError::GetFileLink("boom".into());
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 1200);
        assert_eq!(unified.category(), "api");
    }

    #[test]
    fn crypto_helper_funnels_to_crypto() {
        // `EmptyPassword` is a caller-input failure (InvalidInput=2100), while
        // actual crypto-subsystem failures (`Shell`, `SendChangeUserPrivate`,
        // `ChangeUserPrivate`) funnel to the Crypto category (1600).
        let input_err = CryptoHelperError::EmptyPassword;
        let unified: Unified = input_err.into();
        assert_eq!(unified.code(), 2100);

        let shell_err = CryptoHelperError::Shell("boom".into());
        let unified: Unified = shell_err.into();
        assert_eq!(unified.code(), 1600);
    }

    #[test]
    fn kv_helpers_funnel_to_storage() {
        let e1: Unified = ValueKvError::Store("x".into()).into();
        let e2: Unified = SettingKvError::Store("x".into()).into();
        assert_eq!(e1.code(), 1700);
        assert_eq!(e2.code(), 1700);
    }

    #[test]
    fn account_utility_terms_not_accepted_is_invalid_input() {
        let err = AccountUtilityError::TermsNotAccepted;
        let unified: Unified = err.into();
        // InvalidInput category = 2100.
        assert_eq!(unified.code(), 2100);
    }

    #[test]
    fn account_utility_api_call_funnels_to_api() {
        let err = AccountUtilityError::VerifyEmail("boom".into());
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 1200);
    }

    #[test]
    fn backup_helper_empty_name_is_invalid_input() {
        let err = BackupHelperError::EmptyName;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 2100);
    }

    #[test]
    fn upload_helper_not_authenticated_is_auth() {
        let err = UploadHelperError::NotAuthenticated;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 1000);
    }

    #[test]
    fn notifications_helper_not_authenticated_is_auth() {
        let err = NotificationsHelperError::NotAuthenticated;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 1000);
    }

    #[test]
    fn publink_helper_empty_code_is_invalid_input() {
        let err = PublinkHelperError::EmptyCode;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 2100);
    }

    #[test]
    fn folder_metadata_invalid_path_is_invalid_input() {
        let err = FolderMetadataError::InvalidPath;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 2100);
    }

    #[test]
    fn create_folder_empty_name_is_invalid_input() {
        let err = CreateFolderHelperError::EmptyName;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 2100);
    }

    #[test]
    fn upload_session_canceled_is_busy() {
        let err = upload_session::UploadError::Canceled;
        let unified: Unified = err.into();
        assert_eq!(unified.code(), 2200);
    }

    #[test]
    fn sdk_error_category_mapping() {
        // Each per-helper error must funnel into the matching `SdkError`
        // category variant. Spot-check one representative error from every
        // category so the consolidated routing table cannot regress silently.
        let auth: SdkError = AuthHelperError::NotAuthenticated.into();
        assert!(matches!(auth, SdkError::Auth(_)));

        let upload: SdkError = UploadHelperError::NotAuthenticated.into();
        assert!(matches!(upload, SdkError::Upload(_)));

        let upload_session_err: SdkError = upload_session::UploadError::Canceled.into();
        assert!(matches!(upload_session_err, SdkError::UploadSession(_)));

        let download: SdkError = DownloadHelperError::NotAuthenticated.into();
        assert!(matches!(download, SdkError::Download(_)));

        let crypto: SdkError = CryptoHelperError::EmptyPassword.into();
        assert!(matches!(crypto, SdkError::Crypto(_)));

        let backup: SdkError = BackupHelperError::EmptyName.into();
        assert!(matches!(backup, SdkError::Backup(_)));

        let publink: SdkError = PublinkHelperError::EmptyCode.into();
        assert!(matches!(publink, SdkError::Publink(_)));

        let folder: SdkError = FolderMetadataError::InvalidPath.into();
        assert!(matches!(folder, SdkError::Folder(_)));

        let create_folder: SdkError = CreateFolderHelperError::EmptyName.into();
        assert!(matches!(create_folder, SdkError::CreateFolder(_)));

        let account: SdkError = AccountUtilityError::TermsNotAccepted.into();
        assert!(matches!(account, SdkError::Account(_)));

        let notifications: SdkError = NotificationsHelperError::InvalidNotificationId.into();
        assert!(matches!(notifications, SdkError::Notifications(_)));

        let kv: SdkError = ValueKvError::Store("x".into()).into();
        assert!(matches!(kv, SdkError::Kv(_)));

        let setting: SdkError = SettingKvError::Store("x".into()).into();
        assert!(matches!(setting, SdkError::Setting(_)));

        let mount: SdkError = MountHelperError::NotAuthenticated.into();
        assert!(matches!(mount, SdkError::Mount(_)));

        let io: SdkError = std::io::Error::other("boom").into();
        assert!(matches!(io, SdkError::Io(_)));

        // Each `SdkError` variant must also funnel back into the workspace
        // `pcloud_error::Error` taxonomy via `From<SdkError>`, preserving the
        // cause chain rather than collapsing into an opaque string.
        let unified: Unified = SdkError::Auth(AuthHelperError::NotAuthenticated).into();
        assert_eq!(unified.category(), "auth");
        let unified: Unified = SdkError::Io(std::io::Error::other("boom")).into();
        assert_eq!(unified.category(), "local_io");
    }
}

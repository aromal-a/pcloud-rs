#![forbid(unsafe_code)]
//! # pcloud-backends
//!
//! Per-subsystem backend implementations split out of `pcloud-daemon`
//! (PLAN_A_PLUS P6.1): auth, account, backup, crypto, folder, mount
//! discovery, notifications, public links, shares, sync, sync suggest,
//! and transfer. `pcloud-daemon` re-exports them at their original paths
//! so external callers (CLI, SDK, tests) do not observe an API change.
//!
//! **Architecture:** see `docs/book/src/architecture/crate-map.md`. These
//! backends are orchestrated by `pcloud-daemon::runtime` and consume
//! `pcloud-proto` clients, `pcloud-store`, `pcloud-secret`, and
//! `pcloud-config`.
//!
//! **Stability:** T1 internal — not semver-stable; access via
//! `pcloud-daemon` re-exports.
//!
//! **MSRV:** Rust 1.89 for the portable crate; full workspace and release
//! validation use the repository-pinned Rust 1.96.1 toolchain.
//!
//! **Features:** none.
//!
//! **Platform:** portable.
//!
//! Intra-crate references between these modules use the usual `crate::`
//! path; cross-references back into `pcloud-daemon` private modules are
//! intentionally absent by construction (moved backends had no such
//! references at the time of the split, aside from documentation-only
//! intra-doc links).
//!
//! # Backend responsibility map
//!
//! Each backend exposes a typed entry struct (`*Runtime`) that owns a
//! pooled protocol client, dispatches IPC frames from
//! `pcloud-daemon::dispatch`, emits audit events to the shared audit
//! sink, and persists durable state to `pcloud-store`. Errors collapse
//! into a crate-local `*BackendError` enum surfaced upstream unmodified.
//!
//! - `auth_backend`: maps the pCloud `getdigest`/`login`/`tfa_login`/
//!   `tfa_loginwithrecoverycode`/`tfa_sendsms`/`tfa_sendnotification`/
//!   `logout`/`userinfo` API surface. Entry struct: [`AuthRuntime`](auth_backend::AuthRuntime).
//!   Error taxonomy: `AuthBackendError { Transport, Parse, Result,
//!   TwoFactorRequired, VaultIo, RefreshToken }`. Drives auth-vault
//!   persistence decisions and keeps `SecretString`/`SecretBytes` off
//!   any long-lived struct.
//! - `account_backend`: maps `getlocationapi`, `getpromourl`,
//!   `setlanguage`, `sendverificationemail`, `sendverificationemailrestricted`,
//!   `lostpassword`, `changepassword`, `register`. Entry struct:
//!   [`AccountRuntime`](account_backend::AccountRuntime). Error taxonomy:
//!   `AccountBackendError { Transport, Parse, Result, Account,
//!   InvalidLanguage, InvalidApiServer, EmailRequired, PasswordTooShort }`.
//!   No durable state is written here beyond the active `ConfigProfile::ApiMode`
//!   override which is applied in-memory only.
//! - `sync_backend`: maps `listfolder` (for remote-root validation),
//!   `diff`, and drives the `pcloud-engine` diff poller. Entry struct:
//!   [`SyncRuntime`](sync_backend::SyncRuntime). Error taxonomy:
//!   `SyncBackendError { Transport, Parse, Result, SyncApi, FolderApi,
//!   InvalidPath, Duplicate, NestedRoot, NotFound, Store }`. Persists
//!   `sync_roots` and `diff_state` tables; `sync_remove` evicts runtime
//!   queue entries *before* the remote call for idempotency.
//! - `transfer_backend`: maps `getfilelink`, `getpubzip`, `upload_create`,
//!   `upload_write`, `upload_save`, `uploadfile`. Entry struct:
//!   [`TransferRuntime`](transfer_backend::TransferRuntime). Error
//!   taxonomy: `TransferBackendError { Transport, Parse, Result,
//!   Transfer, Download, Upload, Journal, Io }` plus
//!   [`ChunkedUploadError`](transfer_backend::ChunkedUploadError). Writes
//!   the NDJSON upload journal (`$XDG_RUNTIME_DIR/pcloud/uploads.journal`)
//!   for crash-safe resume; see [`upload_journal`].
//! - `shares_backend`: maps `listshares`, `sharefolder`, `cancelshare`,
//!   `changeshare`, `acceptshare`, `declineshare`, `removeshare`,
//!   `listcontacts`, `account_teams`, `account_teamshare`, plus the
//!   `crypto_sendsharekey`/`crypto_getfileencoder` temppass pair. Entry
//!   struct: [`SharesRuntime`](shares_backend::SharesRuntime). Error
//!   taxonomy: `SharesBackendError { Transport, Parse, Result, Shares,
//!   Crypto(CryptoShareError) }`.
//! - `public_link_backend`: maps `getfilepublink`, `getfolderpublink`,
//!   `gettreepublink`, `listpublinks`, `showpublink`, `deletepublink`,
//!   `changepublink` (expire/password/uploadpolicy),
//!   `createuploadlink`, `listuploadlinks`, `deleteuploadlink`,
//!   `getfilepubzip`, `uploadtolink`. Entry struct:
//!   [`PublicLinkRuntime`](public_link_backend::PublicLinkRuntime). Error
//!   taxonomy: `PublicLinkBackendError { Transport, Parse, Result,
//!   PublicLink, Resolver, UnauthenticatedResolverRequired }`. Tree-path
//!   resolution goes through [`path_resolver::RemotePathResolver`].
//! - `crypto_backend`: maps `crypto_getuserkeys`, `crypto_setuserkeys`,
//!   `crypto_reset`, `crypto_createfolder`, plus the local unlock/lock
//!   state machine and `SecretBytes`-wrapped AES-256-GCM sector helpers.
//!   Entry struct: [`CryptoRuntime`](crypto_backend::CryptoRuntime). Error
//!   taxonomy: `CryptoBackendError { Transport, Parse, Result, Crypto,
//!   Locked, PasswordMismatch, FingerprintMismatch }`. Never touches the
//!   auth vault and never persists cleartext keys.
//! - `folder_backend`: maps `listfolder`, `createfolder`,
//!   `createfolderifnotexists`, `renamefolder`, `deletefolder`,
//!   `deletefolderrecursive`, `stat`. Entry struct:
//!   [`FolderRuntime`](folder_backend::FolderRuntime). Error taxonomy:
//!   `FolderBackendError { Transport, Parse, Result, Folder }`. Used by
//!   higher backends (sync, public-link) for remote path resolution.
//! - `backup_backend`: maps `backup_create`, `backup_list`,
//!   `backup_delete`, `stop_device`. Entry struct:
//!   [`BackupRuntime`](backup_backend::BackupRuntime). Error taxonomy:
//!   `BackupBackendError { Transport, Parse, Result, Backup, Cascade }`
//!   with [`SyncRootCascadeError`](backup_backend::SyncRootCascadeError)
//!   for the sync-root cleanup path. Writes `backup_devices` in the store;
//!   does **not** implicitly register or remove local sync roots as a
//!   side effect (caller must opt in).
//! - `notifications_backend`: maps `listnotifications`, `readnotifications`.
//!   Entry struct: [`NotificationsRuntime`](notifications_backend::NotificationsRuntime).
//!   Error taxonomy: `NotificationsBackendError { Transport, Parse, Result,
//!   Notifications }`. Purely read-through; no durable state is written.
//!
//! # Shared patterns
//!
//! ## `EncodedValue` wire translation
//!
//! Each typed entry struct in a backend file exposes an `encoded()` or
//! `into_fields()` helper returning a vector of `(name, EncodedValue)`
//! tuples consumable by `EncodedValue`. The
//! `EncodedValue` enum (`Number`, `String`, `Bool`, `Bytes`) mirrors the
//! binary-protocol field types defined in `pclsync/psettings.h`.
//! Strings are passed through verbatim (pCloud is UTF-8 on the wire),
//! booleans collapse to `0`/`1` numbers, and raw bytes are reserved for
//! crypto material and pre-hashed digests. Backends never transmit secrets
//! as `String` to preserve `SecretString` zeroization.
//!
//! ## Error translation
//!
//! Every dispatch function returns a crate-local error variant that
//! surfaces the upstream API numeric code plus a redacted message, never
//! the raw secret payload that produced it. Transport errors collapse
//! into `Transport` variants; `result != 0` responses collapse into
//! `Result { result, message }` variants.
//!
//! ## Side effects and rollback
//!
//! Backends that write durable state (sync roots, backups, upload journal,
//! auth vault) always execute the store mutation *after* a successful
//! remote call, so a transport failure leaves the local DB untouched. The
//! only exception is `sync_remove`, which evicts engine
//! queue entries before the API call and must be retried idempotently.
//! See the per-function docs for the exact ordering.

// **PLATFORM:** all
// **GATING:** none (portable).

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

pub mod account_backend;
#[cfg(test)]
mod account_backend_tests;
pub mod auth_backend;
pub mod backup_backend;
pub mod crypto_backend;
pub mod folder_backend;
pub mod ignore_patterns;
pub mod mock;
pub mod mount_discovery;
pub mod notifications_backend;
pub mod path_resolver;
pub mod public_link_backend;
pub mod remote_fs;
pub mod residency;
pub mod shares_backend;
pub mod snapshot;
pub mod sync_backend;
pub mod sync_suggest;
pub mod transfer_backend;
pub mod upload_journal;
pub mod upload_sessions;
pub mod upload_state;

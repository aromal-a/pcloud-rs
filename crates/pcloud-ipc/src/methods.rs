//! Typed `Method` enum: exhaustive catalog of IPC operations the daemon
//! accepts. Each variant maps 1:1 to a backend entry point in
//! `pcloud-daemon::dispatch`. Adding a new daemon capability means
//! adding a variant here first.
//!
//! **Platform banner:** Linux today; cross-platform parity is tracked
//! under `TODO(bd-xplat)`.

// **PLATFORM:** Linux
// **GATING:** none (portable; uses Linux-only idioms — see TODO(bd-xplat)).

use pcloud_model::public_links::PublicLinkUploadPolicy;
use pcloud_model::sync::SyncType;
use serde::{Deserialize, Serialize};

use crate::redacted::RedactedString;

/// Exhaustive catalog of argumentless IPC operations the daemon accepts.
///
/// Each variant maps 1:1 to a backend entry point in
/// `pcloud-daemon::dispatch` / `pcloud-daemon::runtime::handle_request`;
/// every variant below references the specific subsystem it drives
/// (auth_backend, sync_backend, crypto runtime, public_link_backend,
/// shares_backend, account_backend, transfer_backend, audit subsystem,
/// or the engine/FUSE runtime). Methods that require arguments are
/// carried on the [`Request`] enum instead (see [`Request::Plain`] for
/// the argumentless dispatch wrapper).
///
/// Wire encoding: `serde_json::to_vec(&Request::Plain { method })`.
/// Error classification (retryability) lives on [`ResponseStatus`].
///
/// Marked `#[non_exhaustive]`: new variants may be added in a future
/// revision of the IPC surface, and downstream `match` expressions must
/// accept a fallthrough arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Method {
    /// Return a short, human-readable daemon status line
    /// (`running | paused | ...`). Mirrors `pcloud-rs status`.
    GetStatus,
    /// Basic liveness probe. Returns `ok` when the daemon is running;
    /// callers who need structured diagnostics should use
    /// [`Method::Health`] instead.
    GetHealth,
    /// Enterprise health probe: returns daemon build info, uptime, the
    /// last event summary, and — when the `metrics` feature is enabled —
    /// a Prometheus text-format snapshot of the metric families.
    /// Distinct from `GetHealth` (which returns a short human summary).
    Health,
    /// List pending transfers currently queued by the engine.
    GetPending,
    /// List all registered sync roots.
    GetSyncRoots,
    /// List active public links created by this account.
    ListPublicLinks,
    /// List active upload-only links.
    ListUploadLinks,
    /// Return the authenticated user's account summary (quota, email,
    /// user id, plan).
    GetUserInfo,
    /// Pause the sync engine globally. Existing transfers are drained;
    /// new work is held until a matching [`Method::ResumeSync`].
    PauseSync,
    /// Resume a previously paused sync engine.
    ResumeSync,
    /// Begin an interactive login flow: the daemon prepares the auth
    /// state machine and advertises which credential submission is
    /// expected next.
    LoginBegin,
    /// Log the authenticated session out and destroy any in-memory
    /// credential material. Persisted tokens (opt-in) are also removed.
    Logout,
    /// Request a TFA code delivery via SMS.
    SendTwoFactorSms,
    /// Request a TFA push notification to the account's trusted device.
    SendTwoFactorNotification,
    /// Submit a previously staged password (via
    /// [`Request::PasswordSubmission`]) to the auth state machine.
    SubmitPassword,
    /// Submit a previously staged two-factor code (via
    /// [`Request::TwoFactorCodeSubmission`]) to the auth state machine.
    SubmitTwoFactorCode,
    /// Unlock an already-set-up crypto shell with the passphrase from a
    /// prior [`Request::CryptoUnlock`].
    UnlockCrypto,
    /// Lock the crypto shell and zero in-memory key material.
    LockCrypto,
    /// Report crypto setup / started / folder-count state.
    GetCryptoStatus,
    /// Reset crypto: wipes local fingerprint and folder registry.
    CryptoReset,
    /// Read the current crypto private-key flags (mirrors
    /// `psync_crypto_priv_key_flags`). The flags value is returned in the
    /// response message as a decimal integer.
    GetCryptoPrivKeyFlags,
    /// Request a server-side confirmation code to authorize a subsequent
    /// crypto password-change (mirrors
    /// `psync_crypto_crypto_send_change_user_private`).
    SendCryptoChangeUserPrivate,
    /// Cleanly shut down the daemon. All in-flight transfers are
    /// drained, caches flushed, and the IPC socket is removed.
    Shutdown,
    /// Toggle durable auth-token persistence per the flag carried in
    /// [`Request::AuthPersistence`].
    SetAuthPersistence,
    /// List pending share invitations the user has received.
    ListIncomingShares,
    /// List pending share invitations the user has sent.
    ListOutgoingShares,
    /// List *accepted* incoming share requests.
    ListIncomingShareRequests,
    /// List *accepted* outgoing share requests.
    ListOutgoingShareRequests,
    /// List the user's address-book contacts.
    ListContacts,
    /// List the business teams the user belongs to.
    ListMyTeams,
    /// List pending account notifications. Mirrors C
    /// `psync_get_notifications` (pclsync/psynclib.c:248).
    ListNotifications,
    /// Report session lifecycle status. See `Request::SessionStatus`-style
    /// documentation on the companion `Request` variant. Added as a
    /// `Method` variant so the daemon's `Plain { method }` dispatch can
    /// resolve it alongside other status probes.
    SessionStatus,
    /// Return the revision history of a file by absolute remote path.
    /// Mirrors the C `listrevisions` wire command
    /// (`pclsync/pnetlibs.c:2481`, `download_file_revisions`).
    ///
    /// **Honest scope:** pCloud does not publicly document a
    /// third-party-accessible `listrevisions` endpoint; the daemon
    /// currently returns [`ResponseStatus::Unavailable`] with a
    /// tracker pointer (`bd-1du.10`). The IPC surface is wired so the
    /// retained backend can flip it to live-dispatch once the API path
    /// is confirmed and implemented. Arguments are carried on
    /// [`Request::FileHistory`] because a path + limit pair does not
    /// fit the [`Request::Plain`] shape.
    FileHistory,
    /// H14 PR4 — return the background-integrity-sweeper progress as
    /// JSON in [`Response::message`]. Payload is a
    /// [`IntegrityStatusPayload`]. Always safe to call; returns zero
    /// progress when the sweeper has never run. Tracker: bd-1du.4.6.1.
    IntegrityStatus,
    /// Tier-2 HA status probe. Returns a JSON-serialised
    /// `HaStatusPayload` (`{mode, lease_owner, lease_age_s,
    /// lease_path}`) in `Response::message`. Always safe to call; if
    /// HA is disabled the payload simply reports `mode = "disabled"`.
    /// Rendered by `pcloudc ha status`. See
    /// `docs/enterprise/ha.md` §4.2 (Tier 2) and
    /// `crates/pcloud-daemon/src/ha_lease.rs`.
    HaStatus,
    /// Report the daemon's current drain state. JSON payload in
    /// [`Response::message`] follows [`DrainStatusPayload`]:
    /// `{state: "running"|"draining"|"stopped", in_flight: N,
    /// elapsed_drain_ms: M}`.
    ///
    /// Always safe to call, including while the daemon is draining —
    /// the drain gate explicitly admits this method so operators (and
    /// `pcloudc drain`) can poll progress up to the moment the socket
    /// is unbound. Rendered by the `pcloudc drain` CLI command and by
    /// external supervisors performing rolling upgrades.
    DrainStatus,
    /// Return the canonical Service-Level Objective report as a JSON
    /// [`SloReportPayload`] in [`Response::message`]. Always safe to
    /// call; SLOs without enough observations are reported with
    /// `status = "no_data"` so callers do not conflate "quiet" with
    /// "healthy". Rendered by `pcloudc slo`. See
    /// `crates/pcloud-observability/src/slo.rs` for the canonical SLO
    /// set and thresholds.
    GetSlo,
    /// Return the scheduled audit-chain verifier status as a JSON
    /// [`AuditVerifierStatusPayload`] in [`Response::message`]. Always
    /// safe to call; when the verifier is disabled the payload reports
    /// `enabled = false` and `last_result = "never_run"`. Rendered by
    /// `pcloudc audit-verifier status`. See
    /// `crates/pcloud-daemon/src/audit_verifier_service.rs`.
    GetAuditVerifierStatus,
    /// Return the background sync loop status as a JSON payload in
    /// [`Response::message`]. Reports the loop state
    /// (`running|paused|idle|disabled`), active root count, last cycle
    /// timestamp, duration, and pending transfer counts. Always safe to
    /// call; returns `disabled` when the sync loop config has
    /// `enabled = false`. Rendered by `pcloudc sync status`.
    GetSyncStatus,
    /// List unresolved sync conflicts from the engine scheduler. Returns
    /// a JSON array of [`ConflictEntry`] in [`Response::message`].
    /// Always safe to call; returns an empty array when no conflicts
    /// are queued. Rendered by `pcloudc conflict list`.
    ListConflicts,
    /// Stat an absolute pCloud-drive path against the local metadata
    /// cache populated by the diff engine, with API fallback. Mirrors C
    /// `psync_stat_path` (`pclsync/psynclib.h:743`,
    /// `pfolder.c:734`). Response message is a JSON
    /// [`StatPathPayload`]. Arguments are carried on
    /// [`Request::StatPath`] because a path does not fit the
    /// [`Request::Plain`] shape.
    StatPath,
    /// Return the list of available pCloud API server regions. Mirrors C
    /// `psync_get_api_servers`. No auth required. Response is a JSON
    /// array of `{label, api, binapi, location_id}` objects in
    /// [`Response::message`].
    GetApiServers,
    /// Fetch the promotional URL for this platform. Mirrors C
    /// `psync_get_promo`. Requires an authenticated session.
    /// Response is a JSON `{url,width,height}` object in
    /// [`Response::message`], or `"no promo"` when `haspromo` is false.
    GetPromo,
    /// Fetch the crypto passphrase hint stored at first-time setup.
    /// Mirrors C `psync_crypto_hint`. Requires crypto to be set up;
    /// returns the hint string in [`Response::message`].
    GetCryptoHint,
    /// Trigger a server-side verification email send for the active
    /// authenticated session. Mirrors C `psync_verify_email`. Requires
    /// an authenticated session.
    VerifyEmail,
}

/// Tamper-evident audit chain helpers. Lives outside [`Method`] so it
/// can carry optional id-range arguments without breaking the
/// `Plain { method }` shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AuditVerifyRange {
    /// Inclusive lower bound of the audit-chain id range to verify.
    /// `None` means "start at the genesis row".
    pub from: Option<i64>,
    /// Inclusive upper bound of the audit-chain id range to verify.
    /// `None` means "verify through the latest row at the time of
    /// dispatch".
    pub to: Option<i64>,
}

/// Argument-bearing IPC requests. Every variant corresponds to a
/// daemon-side handler in `pcloud-daemon::runtime::handle_request` and
/// is encoded on the wire as a JSON body following the 8-byte framing
/// header described in [`crate::protocol`]. Argumentless methods are
/// carried as [`Request::Plain`] with a [`Method`] discriminator.
///
/// Marked `#[non_exhaustive]`: new variants may be added; match arms
/// must include a fallthrough.
///
/// NOTE (audit H1): `Request` variants that carry secret material
/// (`PasswordSubmission.value`, `AuthTokenSubmission.value`,
/// `CryptoUnlock.password`, `CryptoSetup.password`,
/// `ChangePublicLinkPassword.password`) intentionally use `String` rather
/// than `SecretString`. Rationale:
///   1. This struct must implement `Serialize`/`Deserialize` for bincode/JSON
///      IPC. `SecretString` currently does not expose serde impls (by design,
///      per audit finding M3) to prevent accidental serialization leaks.
///   2. `Request` instances are constructed immediately before IPC dispatch
///      on the CLI side and immediately destructured into `SecretString`
///      on the daemon side (see `runtime.rs::handle_request`). Their
///      lifetime is send-and-forget across an owner-only Unix socket
///      (chmod 0o600, parent 0o700) with peer-uid enforcement.
///   3. The CLI-side long-lived secret storage (`SecretInputs`) already uses
///      `SecretString`; the transit copy here is ephemeral.
///
/// If this invariant regresses (e.g. `Request` values start being stored on
/// long-lived state or logged via `Debug`), these fields must be converted
/// to `SecretString` and a serde-skip or redacted-serialize wrapper added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Request {
    /// Argumentless dispatch wrapper: every [`Method`] variant that does
    /// not carry parameters is sent as `Plain { method: … }`.
    Plain {
        /// The argumentless method to dispatch.
        method: Method,
    },
    /// Stage a username+password pair for later
    /// [`Method::SubmitPassword`]. See the audit H1 note above — `value`
    /// is intentionally a transit-only `String`.
    PasswordSubmission {
        /// Account username (typically email address). Not a secret.
        username: String,
        /// Cleartext password. Transit-only; destructured into
        /// `SecretString` on the daemon side before any storage.
        /// Debug is redacted (`<redacted N bytes>`) via
        /// [`RedactedString`].
        value: RedactedString,
    },
    /// Stage a pre-obtained pCloud API auth token for persistence / use.
    AuthTokenSubmission {
        /// Opaque pCloud auth token. Transit-only secret; Debug is
        /// redacted via [`RedactedString`].
        value: RedactedString,
    },
    /// Submit a TFA code (or recovery code) to the auth state machine.
    TwoFactorCodeSubmission {
        /// The numeric TFA code or the user's recovery phrase.
        value: String,
        /// `true` to request the server to mark this device as trusted
        /// so subsequent logins skip the TFA prompt.
        trust_device: bool,
        /// `true` when `value` is a recovery code rather than an OTP.
        recovery_code: bool,
    },
    /// Unlock an already-set-up crypto shell.
    CryptoUnlock {
        /// Crypto passphrase. Transit-only secret; Debug is redacted
        /// via [`RedactedString`].
        password: RedactedString,
    },
    /// First-time crypto setup. Mirrors `psync_crypto_setup`.
    CryptoSetup {
        /// New crypto passphrase. Transit-only secret; Debug is
        /// redacted via [`RedactedString`].
        password: RedactedString,
        /// Optional password hint stored with the crypto metadata.
        hint: Option<String>,
    },
    /// Create an encrypted folder (local bookkeeping + deterministic
    /// encrypted name). Mirrors `psync_crypto_mkdir`.
    CryptoMkdir {
        /// Plaintext display name of the folder. The daemon derives the
        /// server-side encrypted name deterministically.
        name: String,
        /// Parent remote folder id; `None` means "top-level".
        parent_folder_id: Option<u64>,
        /// Local folder id when the caller already has one bound; `None`
        /// to let the daemon allocate.
        local_folder_id: Option<u64>,
    },
    /// Rotate the crypto passphrase. Mirrors
    /// `psync_crypto_change_crypto_pass`. The shell must be set up but may
    /// be locked — the old password is checked in constant time. `code` is
    /// the confirmation code delivered via
    /// [`Method::SendCryptoChangeUserPrivate`]. `flags` mirrors the
    /// `PSYNC_CRYPTO_FLAG_TEMP_PASS` bit set that the C client stores on
    /// the `crypto_private_flags` row.
    ///
    /// Audit H1: `old_password` and `new_password` follow the same transit-
    /// only secret lifetime rationale as `CryptoUnlock.password`.
    CryptoChangePassword {
        /// Existing crypto passphrase. Transit-only secret; Debug is
        /// redacted via [`RedactedString`].
        old_password: RedactedString,
        /// Replacement crypto passphrase. Transit-only secret; Debug
        /// is redacted via [`RedactedString`].
        new_password: RedactedString,
        /// Updated password hint stored with the new passphrase.
        hint: String,
        /// Server-side confirmation code obtained via
        /// [`Method::SendCryptoChangeUserPrivate`].
        code: String,
        /// `crypto_private_flags` row (e.g. `PSYNC_CRYPTO_FLAG_TEMP_PASS`).
        flags: u64,
    },
    /// Rotate the crypto passphrase without re-supplying the old one.
    /// Only valid when the shell is already unlocked. Mirrors
    /// `psync_crypto_change_crypto_pass_unlocked`.
    CryptoChangePasswordUnlocked {
        /// Replacement crypto passphrase. Transit-only secret; Debug
        /// is redacted via [`RedactedString`].
        new_password: RedactedString,
        /// Updated password hint.
        hint: String,
        /// Server-side confirmation code.
        code: String,
        /// Updated `crypto_private_flags` row.
        flags: u64,
    },
    /// Toggle durable auth-token persistence. Secure default is `false`.
    AuthPersistence {
        /// `true` to opt in to persisting the auth token in the
        /// owner-only, `0600`-mode vault file.
        enabled: bool,
    },
    /// Register a new sync root (local ↔ remote binding).
    ///
    /// The optional `sync_type` selects the direction flavor. When
    /// absent on the wire, the daemon defaults to [`SyncType::Full`]
    /// (bidirectional) to preserve wire-compatibility with pre-flavor
    /// clients that do not serialize the field. The field uses
    /// `#[serde(default, skip_serializing_if = "Option::is_none")]`
    /// so old clients and new clients interoperate.
    SyncRootAdd {
        /// Absolute local path. Canonicalized by the daemon before
        /// duplicate/nested-root checks.
        local_path: String,
        /// Absolute remote pCloud-drive path.
        remote_path: String,
        /// Optional direction flavor for the new root. `None` → `Full`
        /// (bidirectional). Wire-compat: absent on the wire when a
        /// caller did not request a specific flavor.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sync_type: Option<SyncType>,
    },
    /// Delete an existing sync root by id. Pending transfers for the
    /// removed root are evicted from the engine queue.
    SyncRootRemove {
        /// Local id of the sync root to remove.
        sync_id: u64,
    },
    /// Pause an existing sync root without deleting it.
    /// Mirrors the paused flag used by the legacy C sync lifecycle.
    SyncRootPause {
        /// Sync root id to pause.
        sync_id: u64,
    },
    /// Resume a previously paused sync root.
    SyncRootResume {
        /// Sync root id to resume.
        sync_id: u64,
    },
    /// Change the sync direction (mirrors C `psync_change_synctype`).
    SyncRootChangeType {
        /// Sync root id to mutate.
        sync_id: u64,
        /// New direction: download-only, upload-only, or bidirectional.
        sync_type: SyncType,
    },
    /// Scan a local directory and return a list of candidate sync folders.
    /// Mirrors the C `psync_get_sync_suggestions` helper's shape (shallow
    /// top-level summary). The underlying heuristic differs from the C
    /// extension-based scorer but gives the CLI actionable suggestions.
    GetSyncSuggestions {
        /// Absolute local path under which to scan.
        path: String,
        /// Optional hard cap on the number of suggestions returned.
        max: Option<usize>,
    },
    /// Classify whether a given local path can be added as a sync root.
    /// Mirrors C `psync_is_folder_syncable`: rejects nested/duplicate roots.
    IsFolderSyncable {
        /// Absolute local path to classify.
        path: String,
    },
    /// Show metadata for a public link by its short share code.
    ShowPublicLink {
        /// Short public-link code (e.g. `XYZ123`).
        code: String,
    },
    /// Delete an existing public link by id.
    DeletePublicLink {
        /// Numeric public-link id.
        link_id: u64,
    },
    /// Delete an existing public link by its short share code.
    ///
    /// Convenience variant for callers (like the CLI) that only have
    /// the code-form URL segment. The daemon resolves `code` to a
    /// numeric link id by scanning `list_public_links` and dispatches
    /// the existing delete path. This keeps byte compatibility with
    /// older peers by being additive — old servers reject the variant
    /// with `InvalidRequest`, new servers handle it.
    DeletePublicLinkByCode {
        /// Short public-link code (e.g. `XYZ123`).
        code: String,
    },
    /// Create a download link for a single file at `path`.
    CreateFilePublicLink {
        /// Absolute remote path of the file.
        path: String,
    },
    /// Create a download link for a folder at `path`.
    CreateFolderPublicLink {
        /// Absolute remote path of the folder.
        path: String,
    },
    /// Set or clear the expiry on an existing public link.
    ChangePublicLinkExpire {
        /// Public-link id.
        link_id: u64,
        /// UNIX-seconds expiry, or `None` to clear expiry.
        expire: Option<u64>,
    },
    /// Set or clear the password on an existing public link.
    ChangePublicLinkPassword {
        /// Public-link id.
        link_id: u64,
        /// Cleartext password (transit-only) or `None` to clear.
        /// Debug is redacted via [`RedactedString`].
        password: Option<RedactedString>,
    },
    /// Change the upload policy on a folder public link.
    ChangePublicLinkUpload {
        /// Public-link id.
        link_id: u64,
        /// New upload policy (disabled, owner-only, public, …).
        policy: PublicLinkUploadPolicy,
    },
    /// Create an upload-only link pointing at a folder.
    CreateUploadLink {
        /// Absolute remote path of the target folder.
        path: String,
        /// Free-text comment shown to uploaders.
        comment: String,
        /// Optional UNIX-seconds expiry.
        expire: Option<u64>,
        /// Optional aggregate byte quota.
        maxspace: Option<u64>,
        /// Optional aggregate file-count cap.
        maxfiles: Option<u64>,
    },
    /// Delete an existing upload link by id.
    DeleteUploadLink {
        /// Upload-link id.
        upload_link_id: u64,
    },
    /// Create a tree link spanning multiple folders and/or files.
    CreateTreePublicLink {
        /// Display name of the generated tree link.
        name: String,
        /// Optional root folder id; `None` means the items are picked
        /// individually rather than under a common parent.
        root_folder_id: Option<u64>,
        /// Comma-separated folder ids to include.
        folder_ids_csv: Option<String>,
        /// Comma-separated file ids to include.
        file_ids_csv: Option<String>,
        /// Optional UNIX-seconds expiry.
        expire: Option<u64>,
        /// Optional cap on the number of downloads.
        maxdownloads: Option<u64>,
        /// Optional cap on total transferred bytes.
        maxtraffic: Option<u64>,
    },
    /// Enumerate the access entries on a public link.
    ListPublicLinkAccess {
        /// Public-link id.
        link_id: u64,
    },
    /// Grant a specific email recipient access to a public link.
    AddPublicLinkAccess {
        /// Public-link id.
        link_id: u64,
        /// Recipient email address.
        email: String,
    },
    /// Revoke a previously granted public-link access entry.
    RemovePublicLinkAccess {
        /// Public-link id.
        link_id: u64,
        /// Recipient user id to revoke.
        receiver_id: u64,
    },
    /// List all pinned bookmark public links.
    ListBookmarks,
    /// Delete a pinned bookmark by link code.
    RemoveBookmark {
        /// Public-link code.
        code: String,
        /// Location id the bookmark was pinned under.
        location_id: u64,
    },
    /// Mutate a pinned bookmark's display metadata.
    ChangeBookmark {
        /// Public-link code.
        code: String,
        /// Location id the bookmark is pinned under.
        location_id: u64,
        /// New display name.
        name: String,
        /// New long-form description.
        description: String,
    },
    /// Send a share invitation for a folder to a recipient email.
    ShareFolder {
        /// Remote folder id to share.
        folder_id: u64,
        /// Display name for the share.
        name: String,
        /// Recipient email address.
        mail: String,
        /// Free-text message carried in the invitation email.
        message: String,
        /// Permission bitmask (read / create / modify / delete / manage).
        permissions_bits: u32,
        /// Optional crypto-folder password hint.
        hint: Option<String>,
    },
    /// Cancel a pending outgoing share request.
    CancelShareRequest {
        /// Share-request id to cancel.
        share_request_id: u64,
    },
    /// Decline an incoming share request.
    DeclineShareRequest {
        /// Share-request id to decline.
        share_request_id: u64,
    },
    /// Accept an incoming share request and attach it under a local folder.
    AcceptShareRequest {
        /// Share-request id to accept.
        share_request_id: u64,
        /// Destination remote folder id under which to attach the share.
        to_folder_id: u64,
        /// Optional alternative display name for the share.
        name: Option<String>,
    },
    /// Remove an active share by id.
    RemoveShare {
        /// Share id to remove.
        share_id: u64,
    },
    /// Change the permission bits on an active share.
    ModifyShare {
        /// Share id to mutate.
        share_id: u64,
        /// New permission bitmask.
        permissions_bits: u32,
    },
    /// Bulk-stop a set of user shares and team shares.
    AccountStopShare {
        /// User-share ids to stop.
        user_share_ids: Vec<u64>,
        /// Team-share ids to stop.
        team_share_ids: Vec<u64>,
    },
    /// Bulk-modify permissions on a mix of user shares and team shares.
    AccountModifyShare {
        /// List of `(share_id, permissions_bits)` tuples for user shares.
        user_shares: Vec<(u64, u32)>,
        /// List of `(share_id, permissions_bits)` tuples for team shares.
        team_shares: Vec<(u64, u32)>,
    },
    /// Share a folder with an entire business team.
    AccountTeamShare {
        /// Remote folder id to share.
        folder_id: u64,
        /// Display name for the team share.
        name: String,
        /// Recipient team id.
        team_id: u64,
        /// Free-text invitation message.
        message: String,
        /// Permission bitmask.
        permissions_bits: u32,
        /// Optional crypto-folder password hint.
        hint: Option<String>,
    },
    /// Typed key/value read. Mirrors the C
    /// `psync_get_{bool,int,uint,string}_value` helpers backed by the
    /// `setting` table.
    ValueGet {
        /// Setting key.
        name: String,
        /// Expected value kind; mismatched kinds return `InvalidRequest`.
        kind: ValueKvKind,
    },
    /// Typed key/value write. Mirrors the C
    /// `psync_set_{bool,int,uint,string}_value` helpers.
    ValueSet {
        /// Setting key.
        name: String,
        /// Typed value to persist. The variant chosen determines the
        /// column the row is stored in.
        value: ValueKvPayload,
    },
    /// Typed presence check. No direct C analogue (C callers test for
    /// non-zero reads); the Rust surface exposes a strict presence+kind
    /// match so callers do not need to round-trip.
    ValueHas {
        /// Setting key.
        name: String,
        /// Required value kind; presence-with-kind-mismatch reports
        /// absent.
        kind: ValueKvKind,
    },
    /// Report session lifecycle status — expiry timestamp, last observed
    /// activity, and whether a proactive refresh is currently in flight.
    /// Mirrors the timing state tracked by
    /// `pcloud_auth::SessionLifecycle` / `RefreshGuard`. The daemon
    /// serializes a [`SessionStatusPayload`] into
    /// [`Response::message`] as JSON, following the same convention as
    /// [`Method::Health`]. Fields are `None` when no authenticated
    /// session is attached.
    SessionStatus,
    /// Mark all account notifications up to and including `upto_id` as read.
    /// Mirrors C `psync_mark_notificaitons_read` (sic - pclsync/psynclib.c:324).
    /// The Rust identifier uses the corrected spelling; the wire command
    /// (`readnotifications`) preserves the C contract verbatim.
    MarkNotificationsRead {
        /// Inclusive upper bound of the notification id range to mark read.
        upto_id: u64,
    },
    /// Verify the tamper-evident audit chain over the optional id range.
    /// `from` defaults to the genesis row; `to` defaults to the latest
    /// row. Returns `Ok` with a short summary, or `InternalError` with
    /// the first-broken-entry detail on mismatch.
    AuditVerifyChain {
        /// Optional inclusive id range to verify.
        range: AuditVerifyRange,
    },
    /// Mount the pCloud filesystem at `path`. Mirrors the mounted-drive
    /// behaviour of the legacy C client. The daemon validates that the
    /// mountpoint exists, is owned by the daemon uid, and is not
    /// world-writable before handing off to the FUSE layer. `allow_other`
    /// is always rejected (secure default). Tracker: bd-1du.4.
    Mount {
        /// Absolute local mountpoint path.
        path: std::path::PathBuf,
    },
    /// Create a remote folder. When `parent_folder_id` is `Some`, the
    /// daemon creates the leaf folder under that parent (mirrors C
    /// `psync_create_remote_folder`). When `parent_folder_id` is `None`,
    /// the daemon resolves `path` as an absolute remote path (mirrors C
    /// `psync_create_remote_folder_by_path`). When `check_and_create` is
    /// `true`, the daemon walks the suffix-retry helper from C
    /// `psync_check_and_create_folder` (`pclsync/pbusinessaccount.c:803`)
    /// to claim a unique `"name N"` candidate.
    CreateRemoteFolder {
        /// Parent remote folder id. When `None`, `path` is resolved
        /// absolutely instead.
        parent_folder_id: Option<u64>,
        /// Leaf folder name (used when `parent_folder_id` is `Some`).
        name: String,
        /// Absolute remote path (used when `parent_folder_id` is `None`).
        path: String,
        /// When `true`, walk the "name N" suffix-retry helper from C
        /// `psync_check_and_create_folder` to claim a unique candidate
        /// name on collision.
        check_and_create: bool,
    },
    /// Unmount the active filesystem mount (if any). Triggers the
    /// drain-on-unmount hook before tearing down the FUSE session.
    /// Tracker: bd-1du.4.
    Unmount,
    /// Force-unmount a specific path that the daemon does not own.
    /// Intended for recovering from an orphan mount left behind by a
    /// previous daemon process (SIGKILL, crash, out-of-order shutdown).
    /// The daemon invokes `fusermount3 -u` / `fusermount -u` on the
    /// path and returns the outcome. Refuses to act on the currently
    /// active mount — the caller should use [`Request::Unmount`] for
    /// that. Tracker: bd-1du.4 (P1.4).
    MountForceUnmount {
        /// Absolute path to an orphan FUSE mountpoint to force-unmount.
        path: std::path::PathBuf,
    },
    /// Trigger an immediate local-scan wakeup on the engine scheduler.
    /// Mirrors C `psync_run_localscan` (`pclsync/psynclib.c:886`), which
    /// in turn calls `psync_wake_localscan`. The Rust path bumps an
    /// in-memory wake counter on `EngineShell` so callers can correlate
    /// the request with the actual scan.
    RunLocalScan,
    /// Mail an existing public-link `code` to one or more recipients.
    /// Mirrors C `psync_send_publink` (`pclsync/psynclib.c:2217`); wire
    /// command `sendpublink` with `source=1`.
    SendPublink {
        /// Public-link code to forward.
        code: String,
        /// Comma-separated list of recipient email addresses.
        mails: String,
        /// Free-text message body sent alongside the link.
        message: String,
    },
    /// Resolve an absolute pCloud-drive path to its folder id. Mirrors
    /// C `psync_get_fsfolderid_by_path` (`pclsync/psynclib.c:2170`). The
    /// daemon surface walks the canonical drive via authenticated
    /// `listfolder` so it never fabricates the `PSYNC_INVALID_FSFOLDERID`
    /// (`0`) sentinel on miss — it returns [`ResponseStatus::InvalidRequest`]
    /// / [`ResponseStatus::Unauthorized`] / [`ResponseStatus::InternalError`]
    /// instead.
    GetFolderIdByPath {
        /// Absolute pCloud-drive path to resolve.
        path: String,
    },
    /// Read folder flags / permissions / sharing / encryption view for an
    /// absolute pCloud-drive path. Mirrors C
    /// `psync_get_fsfolderflags_by_id` (`pclsync/psynclib.c:2176`) and
    /// the `flags` + `permissions` out-params of
    /// `pfs_fldr_idperm_by_path` (`pfsfolder.c:342`). Answer is
    /// serialised as a compact `key=value` string in
    /// [`Response::message`] for human / JSON-wrapping callers.
    GetFolderFlags {
        /// Absolute pCloud-drive path to inspect.
        path: String,
    },
    /// Read the owner user id of a folder by absolute pCloud-drive path.
    /// Mirrors C `psync_get_folder_ownerid` (`pclsync/psynclib.c:2088`).
    GetFolderOwnerId {
        /// Absolute pCloud-drive path to inspect.
        path: String,
    },
    /// Classify an absolute local path against the daemon's sync-root +
    /// engine state. Mirrors C `psync_filesystem_status`
    /// (`pclsync/psynclib.c:1903`). Response message is one of `INSYNC`,
    /// `INPROG`, `NOSYNC`, `INVSYNC` — identical tokens to the C
    /// `external_status_t` enum.
    FilesystemStatus {
        /// Absolute local path to classify.
        path: String,
    },
    /// List the revision history of a file by absolute remote path.
    /// Mirrors the C `listrevisions` wire command
    /// (`pclsync/pnetlibs.c:2481`, `download_file_revisions`). The
    /// response `message` is a JSON array of revision objects —
    /// `[{"rev_id":"<hex>","mtime":<unix>,"size":<bytes>,"user":"…",
    /// "comment":"…"}, …]` — when the daemon successfully resolves the
    /// history. Honest scope: the retained backend currently returns
    /// [`ResponseStatus::Unavailable`] with a tracker pointer
    /// (`bd-1du.10`) until the public API gate is cleared.
    FileHistory {
        /// Absolute pCloud-drive path of the file.
        path: String,
        /// Optional hard cap on the number of revisions returned.
        limit: Option<u32>,
    },
    /// Walk a local path and cross-check each file's SHA256 against the
    /// server-reported checksum for its mapped remote counterpart. Mirrors
    /// the R9 `pcloudc verify` CLI surface. Per-file classification is one
    /// of `OK`, `MISMATCH`, `MISSING_LOCAL`, `MISSING_REMOTE`; the
    /// daemon's response `message` carries a newline-delimited record
    /// stream the CLI renders directly.
    VerifyPath {
        /// Absolute local path to walk. When it names a regular file the
        /// verifier checks only that file; when it names a directory and
        /// `recursive` is `true`, every regular file under it is walked.
        path: String,
        /// `true` to walk directories recursively; `false` limits the
        /// verification to the immediate children (for directories) or
        /// the single file (for files).
        recursive: bool,
    },
    /// Backup-snapshot lifecycle dispatch (PR1: IPC envelope + CLI parse
    /// only). The daemon-side handler (tar | gpg pipeline, signature
    /// verify, retention sweep) lands in PR2 (H12b); the daemon currently
    /// returns [`ResponseStatus::Unavailable`] with a tracker pointer.
    ///
    /// `gpg_recipient` is the GPG `--recipient` value used to encrypt
    /// (Create) or expected for verification (Verify); it has no secret
    /// material itself (a public key id / email).
    /// `yes` carries an explicit confirmation for destructive actions
    /// (Restore, Prune) so the daemon can refuse non-interactive calls
    /// that did not pass `--yes` even if the CLI is bypassed.
    /// `retention_days` is required for [`SnapshotAction::Prune`] and is
    /// rejected by the CLI when missing for that action.
    BackupSnapshot {
        /// Which lifecycle operation to perform.
        action: SnapshotAction,
        /// Filesystem path the action targets. For `Create` / `Verify` /
        /// `Restore` this is the snapshot file; for `Prune` it is the
        /// directory containing the snapshot set.
        path: std::path::PathBuf,
        /// Optional GPG recipient (key id / email). When `Some`, the
        /// daemon wraps the zstd-compressed tar in a GPG envelope and
        /// requires the archive path to end in `.tar.zst.gpg`.
        /// When `None`, the default zstd + SHA3-sidecar pipeline is
        /// used and the archive path must end in `.tar.zst`.
        gpg_recipient: Option<String>,
        /// Operator confirmation for destructive actions
        /// (`Restore`, `Prune`). The daemon refuses destructive actions
        /// without this flag set so accidental scripted calls cannot
        /// destroy data.
        yes: bool,
        /// Retention policy in days; required for [`SnapshotAction::Prune`].
        retention_days: Option<u32>,
        /// Optional zstd compression level in `1..=22`. `None` means
        /// "use the daemon default" (currently 3, matching the upstream
        /// zstd default). Ignored by `Verify`, `Restore`, and `Prune`.
        /// Old clients that do not emit this field interoperate with
        /// modern daemons via serde's `default = None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zstd_level: Option<i32>,
    },
    /// H14 PR4 — synchronously trigger one background-integrity-sweeper
    /// cycle. The daemon blocks until the cycle completes (or until the
    /// configured rate limiter throttles every candidate, whichever is
    /// first) and returns the post-cycle progress snapshot as JSON in
    /// [`Response::message`]. Tracker: bd-1du.4.6.1.
    IntegrityRunOnce,
    /// H14 PR4 — append `path` to the configured skip-list file and
    /// reload the sweeper's in-memory glob set. `path` is a glob
    /// pattern (e.g. `**/*.tmp`). The daemon refuses the call when no
    /// `[features.integrity_sweeper] skip_list_path` is configured.
    /// Tracker: bd-1du.4.6.1.
    IntegritySkip {
        /// Glob pattern to append. Whitespace is trimmed before write.
        path: String,
    },
    /// Register a new operator-visible upload session.
    ///
    /// The daemon allocates a monotone session id, records it in the
    /// in-memory `SessionRegistry`, and returns the id as a JSON object
    /// `{"session_id": <u64>, "remote_name": "<effective name>"}` in
    /// [`Response::message`]. No bytes are transmitted by this call —
    /// it only reserves the session so the operator can reference it by
    /// id in subsequent pause/resume/cancel/list calls.
    ///
    /// The [`UploadConflictMode`] defaults to [`UploadConflictMode::Error`]
    /// (the strict parity default) when the field is absent on the wire.
    UploadCreate {
        /// Absolute local path the upload will stream from.
        local_path: std::path::PathBuf,
        /// Target remote filename.
        remote_name: String,
        /// Optional parent folder id (when absent, the caller is
        /// expected to have resolved the path on their side).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_folder_id: Option<u64>,
        /// Total byte size the caller is prepared to upload. `0` is
        /// accepted for zero-byte uploads.
        total_bytes: u64,
        /// Conflict policy. Old clients that do not emit the field
        /// get [`UploadConflictMode::Error`] via serde's default.
        #[serde(default)]
        conflict_mode: Option<UploadConflictMode>,
    },
    /// Pause an in-flight upload session. Idempotent against sessions
    /// already in the paused state; rejects terminal sessions with
    /// [`ResponseStatus::Conflict`].
    UploadPause {
        /// Session id returned by [`Request::UploadCreate`].
        session_id: u64,
    },
    /// Resume a paused upload session. Rejects sessions that are not
    /// currently paused.
    UploadResume {
        /// Session id returned by [`Request::UploadCreate`].
        session_id: u64,
    },
    /// Cancel an upload session. Non-terminal → Cancelled; idempotent
    /// against already-cancelled sessions.
    UploadCancel {
        /// Session id returned by [`Request::UploadCreate`].
        session_id: u64,
    },
    /// Enumerate all upload sessions known to the running daemon. The
    /// daemon serialises a `Vec<UploadSessionView>` JSON array into
    /// [`Response::message`]; the CLI renders it directly.
    UploadList,
    /// List unresolved sync conflicts currently queued in the engine
    /// scheduler. Returns a JSON array of `ConflictEntry` objects in
    /// [`Response::message`].
    ConflictList,
    /// Attempt to resolve a specific conflict by path. The daemon
    /// applies the requested `policy` and either promotes the conflict
    /// to a concrete operation or returns an error.
    ConflictResolve {
        /// Relative path of the conflicting file (as shown in
        /// `ConflictList`).
        path: String,
        /// Resolution policy: `"prefer_local"`, `"prefer_remote"`,
        /// `"newest_wins"`, `"rename_both"`. Overrides the config
        /// default for this single conflict.
        policy: String,
    },
    /// Stat an absolute pCloud-drive path: resolve through local
    /// metadata cache, fall back to API. Mirrors C `psync_stat_path`
    /// (`pclsync/psynclib.h:743`).
    StatPath {
        /// Absolute pCloud-drive path to stat.
        path: String,
    },
    /// List the direct children of a folder identified by its absolute
    /// pCloud-drive path. Returns a JSON array of
    /// [`ListFolderEntry`] objects in [`Response::message`]. Used by
    /// the smbr `pcloud` VFS plugin (see
    /// `crates/smb-vfs/src/backends/PCLOUD_PLUGIN.md` in the smbr tree)
    /// and any other userspace consumer that needs `readdir`-shaped
    /// access to a remote folder without first round-tripping through
    /// `GetFolderIdByPath`.
    ///
    /// Authenticated. Returns
    /// [`ResponseStatus::Unauthorized`] when the daemon is logged out
    /// and [`ResponseStatus::InvalidRequest`] when `path` is not
    /// absolute or contains traversal segments.
    ///
    /// Tracker: bd-smbr-pcloud P2.
    ListFolderByPath {
        /// Absolute pCloud-drive path of the folder to list.
        path: String,
    },
    /// Delete a remote file identified by absolute pCloud-drive path.
    /// Resolves `path` → `file_id` against the local metadata cache
    /// (with API fallback) and dispatches `deletefile`. Mirrors C
    /// `psync_delete_file`.
    ///
    /// Authenticated. Idempotent — `ResponseStatus::Ok` on success
    /// and on "already absent". Other failure modes:
    /// [`ResponseStatus::InvalidRequest`] for non-absolute paths;
    /// [`ResponseStatus::Conflict`] when the path resolves to a
    /// folder.
    ///
    /// Tracker: bd-smbr-pcloud P2.
    FileDeleteByPath {
        /// Absolute pCloud-drive path of the file to delete.
        path: String,
    },
    /// Delete a remote folder identified by absolute pCloud-drive
    /// path. With `recursive = false` the daemon dispatches
    /// `deletefolder` and the API rejects a non-empty folder
    /// (mirroring POSIX `rmdir`); with `recursive = true` it
    /// dispatches `deletefolderrecursive` which removes the entire
    /// subtree atomically server-side.
    ///
    /// Authenticated. Idempotent on the path-not-found case. Other
    /// failure modes: [`ResponseStatus::Conflict`] when
    /// `recursive = false` and the folder is non-empty.
    ///
    /// Tracker: bd-smbr-pcloud P2.
    FolderDeleteByPath {
        /// Absolute pCloud-drive path of the folder to delete.
        path: String,
        /// When `true`, delete the folder and its full subtree via
        /// `deletefolderrecursive`. When `false`, fail with
        /// [`ResponseStatus::Conflict`] if the folder is non-empty.
        recursive: bool,
    },
    /// Rename or move a file or folder identified by its absolute
    /// pCloud-drive `from` path to its new absolute path `to`. The
    /// daemon resolves both paths to ids, decides between
    /// `renamefile` and `renamefolder` based on the source kind, and
    /// dispatches a single API call.
    ///
    /// Cross-folder moves are supported when the destination's parent
    /// resolves to a different folder id than the source's parent.
    ///
    /// Authenticated. Failure modes:
    /// [`ResponseStatus::InvalidRequest`] for malformed paths;
    /// [`ResponseStatus::Conflict`] when `to` already exists with the
    /// wrong kind (e.g. renaming a file onto an existing directory);
    /// [`ResponseStatus::Unavailable`] for transient API errors.
    ///
    /// Tracker: bd-smbr-pcloud P2.
    RenamePath {
        /// Absolute pCloud-drive path of the entry to rename/move.
        from: String,
        /// Absolute pCloud-drive destination path. The destination's
        /// parent folder must already exist; the basename becomes the
        /// new entry name.
        to: String,
    },
    /// Send a password-reset email for the given account. Mirrors C
    /// `psync_lost_password`. No auth required.
    LostPassword {
        /// Email address of the account to reset.
        email: String,
    },
    /// Verify email using a restricted verify-token (not the session
    /// auth token). Mirrors C `psync_verify_email_restricted`.
    VerifyEmailRestricted {
        /// Server-issued verify token.
        verify_token: String,
    },
    /// Change the account password. Requires an authenticated session.
    /// Mirrors C `psync_change_password`. Transit-only secrets; see
    /// the audit H1 note on [`Request`].
    AccountChangePassword {
        /// Current account password. Transit-only secret; Debug is
        /// redacted via [`RedactedString`].
        current_password: RedactedString,
        /// New account password. Transit-only secret; Debug is
        /// redacted via [`RedactedString`].
        new_password: RedactedString,
    },
    /// Register a new pCloud account. Mirrors C `psync_register`.
    /// No auth required. Transit-only secret field.
    AccountRegister {
        /// New account email address.
        email: String,
        /// Account password. Transit-only secret; Debug is redacted
        /// via [`RedactedString`].
        password: RedactedString,
        /// `true` when the user has explicitly accepted the ToS.
        terms_accepted: bool,
    },
    /// Get the download link for a remote file by numeric id. Mirrors C
    /// `psync_get_file_link`. Requires an authenticated session. Response
    /// is a JSON `{hosts:[…], path:"…", download_tag:"…"}` object in
    /// [`Response::message`].
    GetFileLink {
        /// Remote file id to resolve a download link for.
        file_id: u64,
    },
    /// Download a remote file by numeric id to a local path. Mirrors the
    /// C download-file flow (getfilelink → HTTP fetch → write). Requires
    /// an authenticated session.
    DownloadFile {
        /// Remote file id to download.
        file_id: u64,
        /// Absolute local destination path.
        local_path: std::path::PathBuf,
    },
    /// Delete a backup by numeric folder id. Mirrors C
    /// `psync_delete_backup`. Requires an authenticated session. Calls
    /// `backup/stopbackup` on the server side and removes the matching
    /// local sync root when one is registered.
    DeleteBackup {
        /// Remote folder id of the backup to delete.
        backup_id: u64,
    },
    /// Create a new backup for a local folder at a remote root. Mirrors C
    /// `psync_create_backup`. Requires an authenticated session. Calls
    /// `backup/createbackup` and registers the local folder as an
    /// upload-only sync root via the SyncRootCascade adapter.
    CreateBackup {
        /// Display name for the backup.
        name: String,
        /// Remote root folder id under which the backup will be created.
        root_folder_id: u64,
        /// Absolute local path to register as an upload-only sync root.
        local_path: String,
        /// Optional parent folder display name.
        parent_folder_name: Option<String>,
    },
    /// Stop a device backup by its device folder id. Mirrors C
    /// `psync_stop_device`. Requires an authenticated session. Calls
    /// `backup/stopdevice` and removes the matching local sync root.
    StopDevice {
        /// Remote device folder id to stop.
        device_folder_id: u64,
    },
    /// Delete the local backup-device registration. Mirrors C
    /// `psync_delete_backup_device`. Local-only: clears the persisted
    /// device backup-root folder id so the next `CreateBackup` allocates
    /// a fresh device. Does not talk to the backend.
    DeleteBackupDevice,
    /// Pin the daemon to a specific API server region. Mirrors C
    /// `psync_set_api_server`. Persists to the store and updates all
    /// live protocol transports. Requires no auth but is silently
    /// rejected (InvalidRequest) when the `data_residency` policy denies
    /// the target region.
    SetApiServer {
        /// pCloud location id (from [`Method::GetApiServers`]).
        location_id: u32,
        /// Binary API hostname to pin to.
        binapi: String,
    },
    /// Set the account language preference. Mirrors C
    /// `psync_set_language`. Requires an authenticated session.
    SetLanguage {
        /// IETF language tag (e.g. `"en"`, `"de"`, `"fr"`).
        language: String,
    },
    /// Server-side copy: copy bytes from a remote pCloud file (`fileid`)
    /// into an in-progress upload session. Mirrors the C
    /// `upload_writefromfile` wire primitive at `pclsync/pupload.c:843-859`
    /// (`pupload.c:1125-1131` for the caller-side chunk loop).
    ///
    /// The C params are `uploadid` / `fileid` / `hash` / `uploadoffset` /
    /// `offset` / `count` (plus `auth` + `id`). This IPC variant carries
    /// the same information without the auth token (added by the daemon).
    ///
    /// Tracker: bd-1du row 93. Daemon handler is still a stub pending
    /// `TransferRuntime::upload_write_from_file` wiring.
    UploadWriteFromFile {
        /// Upload session id (`uploadid`) returned by a prior
        /// [`Request::UploadCreate`].
        upload_session_id: u64,
        /// Source remote file id (`fileid`) whose bytes will be copied.
        source_fileid: u64,
        /// Content hash of the source file (`hash`), as returned by the
        /// pCloud API (used for server-side integrity check).
        source_hash: u64,
        /// Byte offset into the upload session at which writing begins
        /// (`uploadoffset`). `0` for the start of a fresh session.
        offset: u64,
        /// Number of bytes to copy from the source file (`count`). Must
        /// be ≤ `PSYNC_MAX_COPY_FROM_REQ`; splitting is the caller's
        /// responsibility (`pupload.c:1125-1131`).
        count: u64,
    },
    /// Create a tree public link by resolving a list of absolute
    /// pCloud-drive paths to their remote folder ids on the daemon side.
    /// Mirrors the C `ptree_public_link` path-based variant: the daemon
    /// walks each path via `listfolder` under the authenticated session,
    /// collects the resulting folder ids, and then calls the existing
    /// `create_tree_public_link` proto path.
    ///
    /// Distinct from [`Request::CreateTreePublicLink`] which requires
    /// callers to supply pre-resolved numeric ids. Tracker: bd-1du row 149.
    CreateTreePublicLinkFromPaths {
        /// Display name of the generated tree link.
        name: String,
        /// Absolute pCloud-drive paths whose folder ids will be resolved
        /// and included in the tree link.
        paths: Vec<String>,
        /// Optional UNIX-seconds expiry.
        expires: Option<u64>,
    },
    /// Set up a crypto profile (fresh or post-password-change).
    /// Mirrors the C `crypto_setuserkeys` endpoint which is used both
    /// for initial setup and for password rotation; the daemon decides
    /// which path by inspecting `CryptoShell::is_setup()`.
    ///
    /// When `backend == Enhanced`, `acknowledge_not_interop` **must**
    /// be `true`; otherwise the daemon rejects with
    /// [`ResponseStatus::InvalidRequest`]. The flag is inert when
    /// `backend == PclsyncCompat`.
    ///
    /// Note: there is intentionally no separate `CryptoChangeUserKeys`
    /// variant — the C client reuses `crypto_setuserkeys` with
    /// overwrite semantics for password rotation. See
    /// `docs/CRYPTO-BACKEND-PLAN.md` §§3–4.
    CryptoSetupV2 {
        /// Which crypto backend to materialise. Defaults to
        /// [`CryptoBackendIpc::PclsyncCompat`] when the field is
        /// absent from a legacy caller.
        backend: CryptoBackendIpc,
        /// Required acknowledgement when `backend == Enhanced` that
        /// the resulting crypto profile is **not** interoperable with
        /// the upstream pcloudcom client. Inert for
        /// [`CryptoBackendIpc::PclsyncCompat`].
        ///
        /// # Threat model note
        ///
        /// This flag is a safeguard against **accidental** misuse (e.g. a
        /// caller that does not know it is requesting an Enhanced profile).
        /// It is **not** a replay-attack prevention mechanism. The IPC
        /// socket is owner-only (mode `0600` in a `0700` parent directory),
        /// so any process that can reach it is already running as the same
        /// uid. A same-uid attacker who can write to the socket can send
        /// `acknowledge_not_interop: true` regardless of this field.
        ///
        /// The correct mitigation for same-uid privilege escalation is the
        /// OS-level file-permission model on the socket (see `IpcServer::bind`),
        /// not a nonce or HMAC over IPC. Adding a per-request nonce tracked by
        /// the daemon would only defend against cross-uid TOCTOU on the socket
        /// path, which the `0700` parent directory already prevents.
        acknowledge_not_interop: bool,
        /// New crypto passphrase. Transit-only secret; Debug is
        /// redacted via [`RedactedString`].
        password: RedactedString,
        /// Optional password hint stored with the crypto metadata.
        hint: Option<String>,
    },
    /// Fetch a folder's RSA-OAEP-wrapped `sym_key_ver1` blob from the
    /// server, have the daemon unwrap it using the unlocked priv key,
    /// and cache the result for subsequent seal/open calls.
    ///
    /// Mirrors C `crypto_getfolderkey` — see
    /// `C_CODE/pclsync/pcryptofolder.c:826`.
    ///
    /// Hot-path read; rate-limit bucket = `Medium`.
    CryptoGetFolderKey {
        /// Remote crypto folder id whose wrapped sym-key should be
        /// fetched and unwrapped.
        folder_id: u64,
    },
    /// Fetch a file's RSA-OAEP-wrapped `sym_key_ver1` blob from the
    /// server, have the daemon unwrap it using the unlocked priv key,
    /// and cache the result for subsequent seal/open calls.
    ///
    /// Mirrors C `crypto_getfilekey` — see
    /// `C_CODE/pclsync/pcryptofolder.c:879`.
    ///
    /// Hot-path read; rate-limit bucket = `Medium`.
    CryptoGetFileKey {
        /// Remote crypto file id whose wrapped sym-key should be
        /// fetched and unwrapped.
        file_id: u64,
    },
}

/// Wire-level mirror of `pcloud_crypto::CryptoBackend`, carried on
/// IPC variants that select which crypto profile to materialise during
/// setup. The IPC crate intentionally does **not** depend on
/// `pcloud-crypto`; daemon-side code translates this enum with a manual
/// `match` when dispatching so the two surfaces can evolve
/// independently.
///
/// Default is [`CryptoBackendIpc::PclsyncCompat`] to preserve the
/// pcloudcom-compatible wire format for existing clients that do not
/// emit the field.
///
/// See `docs/CRYPTO-BACKEND-PLAN.md` §§3–4 for the rationale behind the
/// dual-backend selector and the mandatory `acknowledge_not_interop`
/// gate on the enhanced backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum CryptoBackendIpc {
    /// pcloudcom-compatible crypto profile. Wire format is
    /// bit-identical to the upstream C client so a pCloud account set up
    /// via this backend remains openable by any pcloudcom tool.
    #[default]
    PclsyncCompat,
    /// Enhanced (non-interop) crypto profile. Callers **must** set
    /// `acknowledge_not_interop = true` on the enclosing request or the
    /// daemon rejects with `InvalidRequest`. A profile set up with this
    /// backend is **not** readable by the upstream pcloudcom client.
    Enhanced,
}

/// Wire-level mirror of
/// `pcloud_backends::upload_sessions::ConflictMode`. Carried on
/// [`Request::UploadCreate`]. Old clients that do not emit the field
/// interoperate with modern daemons via serde's `default = None` on the
/// `Option<UploadConflictMode>` slot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UploadConflictMode {
    /// Refuse if the remote path exists (default / strict parity).
    #[default]
    Error,
    /// Replace the existing remote file.
    Overwrite,
    /// Treat an existing remote file as a success no-op.
    Skip,
    /// Upload under a sibling unique name (e.g. `"report (2).pdf"`).
    Rename,
}

/// Single entry in the conflict list returned by
/// [`Request::ConflictList`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictEntry {
    /// Relative path of the conflicting file within its sync root.
    pub path: String,
    /// Machine-readable conflict kind (e.g. `"LocalModifyVsRemoteModify"`).
    pub kind: String,
    /// Sync root id that owns the conflict.
    pub sync_id: u64,
}

/// Payload returned by [`Method::IntegrityStatus`] and
/// [`Request::IntegrityRunOnce`]. JSON-serialised into
/// [`Response::message`]; CLI callers parse it via
/// `serde_json::from_str`.
///
/// All counters are monotone since the daemon last started. `enabled`
/// is `false` when the operator has not opted into the sweeper. No
/// secret material is carried.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityStatusPayload {
    /// `true` when the operator has opted into
    /// `[features.integrity_sweeper] enabled`.
    pub enabled: bool,
    /// Number of files the worker has successfully hashed.
    pub files_hashed: u64,
    /// Total bytes hashed across all completed files.
    pub bytes_hashed: u64,
    /// Cumulative `IntegrityMismatch` audit rows written.
    pub mismatches_found: u64,
    /// Cumulative throttled-candidate count.
    pub throttled: u64,
    /// Cumulative audit-persistence failures observed by the worker.
    /// Non-zero values indicate the audit log refused to accept a
    /// mismatch row (audit invariant M1 — never silently dropped).
    pub audit_drops: u64,
}

/// Payload returned by [`Method::GetAuditVerifierStatus`]. JSON-serialised
/// into [`Response::message`]; CLI callers parse it via
/// `serde_json::from_str`.
///
/// All counters are monotone since the daemon last started. `enabled` is
/// `false` when the operator has disabled the scheduled verifier. No
/// secret material is carried (no HMAC bytes, no event details).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditVerifierStatusPayload {
    /// `true` when `[features.audit_verifier] enabled = true`.
    pub enabled: bool,
    /// Unix seconds of the most recent run, or `0` when no run has
    /// completed since daemon start.
    pub last_run_ts: i64,
    /// Outcome of the most recent run: `"pass"`, `"fail"`, or
    /// `"never_run"` when the scheduler has not ticked yet.
    pub last_result: String,
    /// Number of rows the last run successfully hashed and verified.
    pub chain_length: u64,
    /// First-broken-row detail on failure (`""` when `last_result !=
    /// "fail"`). Example: `"audit chain broken at id=42: entry_hash
    /// mismatch"`.
    pub last_error: String,
    /// Cumulative pass count since daemon start.
    pub total_passes: u64,
    /// Cumulative fail count since daemon start.
    pub total_failures: u64,
}

/// One canonical SLO entry in a [`SloReportPayload`].
///
/// `status` is one of `"ok"`, `"violation"`, `"no_data"`. `target` and
/// `actual` are human-readable strings (e.g. `"<100ms"`, `"3.5MBps"`,
/// `"99.97%"`) chosen so the CLI can render them directly without
/// additional unit conversion. The exact target/actual string formats
/// are **not** part of the stable wire contract — only the field names
/// and the `status` enum values are. Callers that need raw numeric
/// values should consume the Prometheus `/metrics` exposition instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SloReportEntry {
    /// Canonical dotted SLO name, e.g. `ipc.request.latency.p99`.
    pub slo_name: String,
    /// Target rendered as a human-readable string (e.g. `<100ms`).
    pub target: String,
    /// Actual measured value rendered as a human-readable string.
    pub actual: String,
    /// Status, one of `ok` / `violation` / `no_data`.
    pub status: String,
}

/// Payload returned by [`Method::GetSlo`]. JSON-serialised into
/// [`Response::message`]; CLI callers parse it via
/// `serde_json::from_str`.
///
/// `pass` is `true` when no SLO is in the `violation` state; SLOs with
/// status `no_data` do **not** flip the aggregate bit. No secret
/// material is carried.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SloReportPayload {
    /// Canonical SLO entries in stable registration order.
    pub slos: Vec<SloReportEntry>,
    /// `true` when every entry's `status` is either `ok` or `no_data`.
    pub pass: bool,
}

/// Payload returned by [`Method::DrainStatus`]. JSON-serialised into
/// [`Response::message`]; CLI callers parse it via
/// `serde_json::from_str`.
///
/// All fields are non-secret. The `state` field is the lower-case
/// machine-readable label of `pcloud_daemon::signals::DrainState`
/// (`"running"`, `"draining"`, `"stopped"`). `in_flight` is the number
/// of dispatched-but-not-yet-returned requests, including the current
/// `DrainStatus` probe if observed mid-dispatch. `elapsed_drain_ms` is
/// zero while `state == "running"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainStatusPayload {
    /// Drain state label.
    pub state: String,
    /// Number of currently-executing requests.
    pub in_flight: u32,
    /// Milliseconds elapsed since the drain transition, or `0` when
    /// `state == "running"`.
    pub elapsed_drain_ms: u64,
}

/// Payload returned by [`Request::StatPath`]. JSON-serialised into
/// [`Response::message`]; CLI callers parse it via `serde_json::from_str`.
///
/// Mirrors the C `psync_stat_path` result shape: the caller gets
/// folder/file id, parent, name, size, hash, modified/created timestamps,
/// and whether the entry is a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatPathPayload {
    /// Remote file id (for files) or folder id (for folders).
    pub file_id: u64,
    /// Parent folder's remote id. `0` for root.
    pub parent_folder_id: u64,
    /// Leaf entry name.
    pub name: String,
    /// Size in bytes. `0` for folders.
    pub size: u64,
    /// Content hash (hex string). Empty for folders.
    pub hash: String,
    /// Last-modified timestamp (unix seconds).
    pub modified: i64,
    /// Creation timestamp (unix seconds).
    pub created: i64,
    /// `true` if this entry is a folder.
    pub is_folder: bool,
    /// How the path was resolved: `"cache"` or `"api"`.
    pub source: String,
}

/// One entry in the JSON array returned for
/// [`Request::ListFolderByPath`]. Shape mirrors `StatPathPayload` so
/// readers familiar with one can consume the other.
///
/// The full payload is `Vec<ListFolderEntry>` serialised to JSON in
/// [`Response::message`]. Sort order matches what the daemon's
/// metadata cache returns, which in turn matches the API's
/// `listfolder` response (server-defined; do **not** rely on
/// alphabetical order client-side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFolderEntry {
    /// Remote file id (for files) or folder id (for folders).
    pub file_id: u64,
    /// Leaf entry name (no slashes; no trailing `/` on folders).
    pub name: String,
    /// Size in bytes. `0` for folders.
    pub size: u64,
    /// Content hash (hex string). Empty for folders.
    pub hash: String,
    /// Last-modified timestamp (unix seconds).
    pub modified: i64,
    /// Creation timestamp (unix seconds).
    pub created: i64,
    /// `true` if this entry is a folder.
    pub is_folder: bool,
}

#[cfg(test)]
mod tests {
    use super::{Request, SnapshotAction};

    /// PR1 H12 — `BackupSnapshot` IPC envelope round-trips through serde
    /// JSON without losing any field. This guards the wire shape before
    /// the PR2 daemon dispatcher (H12b) is wired.
    #[test]
    fn backup_snapshot_request_serde_roundtrip() {
        let original = Request::BackupSnapshot {
            action: SnapshotAction::Create,
            path: std::path::PathBuf::from("/var/backups/pcloud/today.tar.zst"),
            gpg_recipient: None,
            yes: false,
            retention_days: None,
            zstd_level: Some(6),
        };
        let json = serde_json::to_string(&original).expect("encode");
        let decoded: Request = serde_json::from_str(&json).expect("decode");
        assert_eq!(original, decoded);

        // Prune carries retention_days + yes; verify the heaviest variant.
        let prune = Request::BackupSnapshot {
            action: SnapshotAction::Prune,
            path: std::path::PathBuf::from("/var/backups/pcloud"),
            gpg_recipient: None,
            yes: true,
            retention_days: Some(30),
            zstd_level: None,
        };
        let json = serde_json::to_string(&prune).expect("encode");
        let decoded: Request = serde_json::from_str(&json).expect("decode");
        assert_eq!(prune, decoded);

        // Restore + Verify round-trip too.
        for action in [SnapshotAction::Restore, SnapshotAction::Verify] {
            let req = Request::BackupSnapshot {
                action,
                path: std::path::PathBuf::from("/tmp/snap.tar.zst"),
                gpg_recipient: Some("ops@example.com".to_owned()),
                yes: matches!(action, SnapshotAction::Restore),
                retention_days: None,
                zstd_level: None,
            };
            let json = serde_json::to_string(&req).expect("encode");
            let decoded: Request = serde_json::from_str(&json).expect("decode");
            assert_eq!(req, decoded);
        }
    }

    /// Old clients that serialize without `zstd_level` should still
    /// decode (serde default = None).
    #[test]
    fn backup_snapshot_request_backcompat_decode_without_zstd_level() {
        let json = r#"{"BackupSnapshot":{"action":"Create","path":"/x.tar.zst","gpg_recipient":null,"yes":false,"retention_days":null}}"#;
        let decoded: Request = serde_json::from_str(json).expect("legacy decode");
        match decoded {
            Request::BackupSnapshot { zstd_level, .. } => {
                assert_eq!(zstd_level, None, "legacy client omitted level → None");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}

/// Action discriminator for [`Request::BackupSnapshot`].
///
/// The four variants mirror the `pcloudc snapshot {create,restore,
/// verify,prune}` CLI surface. Legacy two-token forms
/// (`pcloudc backup snapshot-*`) still resolve to these actions with a
/// stderr deprecation warning emitted by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SnapshotAction {
    /// Create a new backup snapshot at `path`. Default pipeline is
    /// tar → zstd → SHA3-256-sealed sidecar; adding a GPG recipient
    /// layers on an optional envelope.
    Create,
    /// Restore an existing backup snapshot from `path`. Restores both
    /// the new `.tar.zst` / `.tar.zst.gpg` archives and legacy
    /// `.tar.gpg` archives (via the legacy path).
    Restore,
    /// Verify the integrity of an existing backup snapshot at `path`
    /// (no mutation; SHA3-sidecar + inner SHA-256 checks).
    Verify,
    /// Prune snapshots in the directory at `path` according to the
    /// `retention_days` policy. `retention_days` is required for this
    /// action; the CLI rejects the call earlier when it is missing.
    Prune,
}

/// Kind selector for the typed setting key/value store. Mirrors the
/// columns carried by the C `setting` SQLite table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueKvKind {
    /// Boolean setting.
    Bool,
    /// Signed 64-bit integer setting.
    Int,
    /// Unsigned 64-bit integer setting.
    Uint,
    /// UTF-8 string setting.
    String,
}

/// Typed payload for a [`Request::ValueSet`] call. Each variant
/// corresponds to one [`ValueKvKind`] column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueKvPayload {
    /// Boolean payload.
    Bool(bool),
    /// Signed 64-bit integer payload.
    Int(i64),
    /// Unsigned 64-bit integer payload.
    Uint(u64),
    /// UTF-8 string payload.
    String(String),
}

/// Serializable payload for [`Method::SessionStatus`]. The daemon
/// renders this as JSON in [`Response::message`]; CLI callers parse it
/// via `serde_json::from_str`.
///
/// All timestamps are seconds since the UNIX epoch (u64) so the wire
/// representation is identical to the `pcloud_auth::Clock` abstraction
/// used internally by `SessionLifecycle`. A `None` value signals "no
/// authenticated session / not yet attached".
///
/// Security: contains no secret material. Never include the auth token
/// or any derivative here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionStatusPayload {
    /// UNIX seconds at which the session expires. `None` when no
    /// lifecycle is attached (e.g. logged out).
    pub expires_at: Option<u64>,
    /// UNIX seconds of the last observed session activity.
    pub last_used_at: Option<u64>,
    /// `true` when a proactive refresh is currently holding the
    /// single-flight `RefreshGuard` slot.
    pub refresh_in_flight: bool,
}

/// Top-level classification of a daemon response. Coarse-grained
/// deliberately — callers rely on [`Response::message`] for detail, and
/// the status set is stable across the IPC schema version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ResponseStatus {
    /// Request completed successfully. Payload (if any) is carried in
    /// [`Response::message`], possibly as JSON (e.g. [`Method::Health`],
    /// [`Method::SessionStatus`]).
    ///
    /// # Recovery
    /// N/A — success. Not an error; callers proceed.
    Ok,
    /// Request is malformed or violates a precondition the daemon can
    /// state without revealing state (e.g. missing argument, invalid
    /// enum value, path not allowed, JSON decode failure, version
    /// mismatch). Maps roughly to HTTP 400 semantics.
    ///
    /// # Recovery
    /// Fatal as-is — not retryable without changing the request body.
    /// Callers should surface [`Response::message`] to the operator.
    InvalidRequest,
    /// Peer is not authorized: owner-uid mismatch on `SO_PEERCRED` /
    /// `getpeereid` / named-pipe SID check, no authenticated session, or
    /// a capability the peer does not hold.
    ///
    /// # Recovery
    /// Fatal for this connection. Not retryable without re-authenticating
    /// (or running as the daemon-owning user). See
    /// `pcloud-daemon::auth_backend` for the session lifecycle.
    Unauthorized,
    /// Operation conflicts with current state (e.g. already mounted,
    /// duplicate sync root, already logged in, crypto already unlocked).
    ///
    /// # Recovery
    /// Not retryable without first mutating the conflicting state.
    /// Callers should reconcile via the companion backend
    /// (e.g. `pcloud-daemon::sync_backend`, `pcloud-daemon::runtime`).
    Conflict,
    /// Subsystem is not available (e.g. crypto is not set up, network
    /// is unreachable, feature compiled out, FUSE runtime not started).
    ///
    /// # Recovery
    /// Transient failures (network, FUSE race) may be retried with
    /// backoff. Permanent unavailability (feature compiled out) is
    /// fatal for the request.
    Unavailable,
    /// Unexpected daemon-side failure. Emitted when a backend raises an
    /// error that does not fit a stricter classification — database
    /// error, panic-guard path, unexpected IO on the daemon side.
    ///
    /// # Recovery
    /// Opaque to the caller; surface [`Response::message`] verbatim.
    /// May be transient (retry with backoff) or persistent (escalate to
    /// operator and inspect daemon logs).
    InternalError,
    /// Request was refused by a declarative policy (e.g. data-residency
    /// allow-list, regional export control, tenant isolation rule). The
    /// `kind` discriminator identifies which policy category fired so
    /// operators and scripts can branch on it without parsing
    /// [`Response::message`].
    ///
    /// **IPC-stable wire format:** the JSON encoding is
    /// `{"PolicyViolation":{"kind":"data_residency"}}`. Consumers MUST
    /// treat unknown `kind` values as "generic policy refusal" rather
    /// than erroring out, so new categories can be introduced in minor
    /// releases.
    ///
    /// # Recovery
    /// Not retryable as-is. The operator must either adjust the policy
    /// (e.g. extend the `allowed_regions` allow-list in
    /// `[data_residency]`) or target a resource whose region/tenant is
    /// permitted.
    PolicyViolation {
        /// Discriminator identifying which policy fired. Stable, lower
        /// snake-case. Known kinds today: `"data_residency"`.
        kind: String,
    },
}

/// Envelope returned by every IPC call. Status is machine-checkable
/// and is the authoritative signal for retryability (see the
/// [`ResponseStatus`] variant docs); `message` is free-form text and is
/// sometimes a JSON-serialized payload — e.g. [`Method::Health`]
/// (Prometheus text), [`Method::SessionStatus`] (JSON
/// [`SessionStatusPayload`]), [`Request::GetFolderFlags`]
/// (`key=value` string). Callers MUST branch on `status` before
/// attempting to parse `message`.
///
/// Security: `message` never carries secret material — see the audit
/// H1 invariant on [`Request`]. Error messages are pre-screened so they
/// are safe to surface to operators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// Outcome classification.
    pub status: ResponseStatus,
    /// Human- or machine-readable detail. For JSON-carrying responses
    /// this is the serialized payload.
    pub message: String,
}

/// Transport-boundary wrapper that carries an inner [`Request`] plus an
/// optional W3C `traceparent` string. Construction sites that don't care
/// about distributed tracing keep building [`Request`] values directly
/// and rely on `From<Request> for RequestEnvelope` (or
/// [`RequestEnvelope::new`]) at the point where the request is handed to
/// the transport. Callers that DO want to propagate a trace context
/// build the envelope explicitly with
/// [`RequestEnvelope::with_traceparent`].
///
/// Wire shape (JSON):
///
/// ```json
/// { "request": { ... existing Request payload ... },
///   "traceparent": "00-..." }
/// ```
///
/// `traceparent` is omitted entirely when `None` (see the
/// `skip_serializing_if` attribute), so the envelope serializes exactly
/// like the bare request body when no trace context is attached. The
/// decoder ([`RequestEnvelope::try_from_wire`]) accepts both shapes —
/// envelope-wrapped and bare-`Request` — so old clients that have not
/// been recompiled to emit the wrapper continue to interoperate.
///
/// Validation note: this layer treats `traceparent` as an opaque string
/// and does NOT reject malformed values. Downstream observability code
/// is responsible for parsing/validating the W3C format; the envelope
/// only needs to forward whatever the producer attached.
///
/// Marked `#[non_exhaustive]` so additional context fields (baggage,
/// sampling hints, …) can be added without breaking pattern matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestEnvelope {
    /// Inner request payload, preserved verbatim from existing
    /// construction sites.
    pub request: Request,
    /// Optional W3C `traceparent` string attached at the transport
    /// boundary. Omitted from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}

impl RequestEnvelope {
    /// Wrap a [`Request`] in an envelope with no trace context. Use
    /// [`Self::with_traceparent`] to attach one.
    #[must_use]
    pub fn new(request: Request) -> Self {
        Self {
            request,
            traceparent: None,
        }
    }

    /// Builder-style setter that attaches a W3C `traceparent` to the
    /// envelope. The string is forwarded verbatim — no validation is
    /// performed at this layer.
    #[must_use]
    pub fn with_traceparent(mut self, traceparent: String) -> Self {
        self.traceparent = Some(traceparent);
        self
    }

    /// Borrow the attached `traceparent`, if any.
    #[must_use]
    pub fn traceparent(&self) -> Option<&str> {
        self.traceparent.as_deref()
    }

    /// Decode an envelope from the JSON payload bytes. Tries the
    /// envelope shape first; falls back to bare-[`Request`] for
    /// back-compat with clients that pre-date the envelope rollout.
    ///
    /// # Errors
    /// Returns the envelope-shape decode error when both shapes fail,
    /// so that diagnostics point at the modern format the daemon
    /// prefers.
    pub fn try_from_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        match serde_json::from_slice::<RequestEnvelope>(bytes) {
            Ok(envelope) => Ok(envelope),
            Err(envelope_err) => match serde_json::from_slice::<Request>(bytes) {
                Ok(req) => Ok(Self::new(req)),
                Err(_) => Err(envelope_err),
            },
        }
    }
}

impl From<Request> for RequestEnvelope {
    fn from(request: Request) -> Self {
        Self::new(request)
    }
}

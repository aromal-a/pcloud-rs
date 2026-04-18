//! Parsed-command enumeration and its mapping onto typed IPC
//! [`pcloud_ipc::Request`] envelopes.
//!
//! Every variant of [`Command`] corresponds to exactly one `pcloudc`
//! subcommand / alias. [`Command::into_request`] is the single point
//! where a parsed command, together with the collected
//! [`SecretInputs`], is lowered into the wire-level request that the
//! daemon accepts. The two common exit-code families are:
//!
//! - parsing failures never reach this module — they surface as
//!   [`crate::exit_code::ExitCode::Usage`] earlier,
//! - daemon responses are translated via
//!   [`crate::exit_code::ExitCode::from_response_status`] so each
//!   variant here inherits the standard mapping without per-command
//!   branching (`Ok → 0`, `Unauthorized → 3`, `Conflict → 7`, …).
//!
//! All variants render through the JSON envelope documented in
//! [`crate::json_output`] when `--json` is active; the shape is
//! `{kind, command, status, message, exit_code, error?}`.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::{AuditVerifyRange, Method, Request, SnapshotAction};
use pcloud_model::public_links::PublicLinkUploadPolicy;
use pcloud_model::sync::SyncType;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};

/// Parsed `pcloudc` subcommand, ready to be lowered into an IPC request.
///
/// Each variant documents: synopsis, daemon-side handler it ultimately
/// reaches, the exit-code family it maps to, and the JSON envelope
/// shape returned under `--json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `pcloudc help` — print the man-page-style usage summary.
    /// Synopsis: no arguments. Daemon handler: `GetHealth` (we reuse
    /// the cheapest ping so the local CLI can still be rendered).
    /// Exit-code: always [`crate::exit_code::ExitCode::Ok`] after the
    /// help block is flushed. JSON: `{kind:"success",command:"help",…}`.
    Help,
    /// `pcloudc status` — fetch the daemon's top-level status snapshot.
    /// Daemon handler: [`pcloud_ipc::Method::GetStatus`]. Exit codes
    /// follow the standard [`pcloud_ipc::ResponseStatus`] mapping.
    /// JSON envelope carries the daemon's `status` + `message` fields.
    Status,
    /// `pcloudc health` — cheap liveness probe used by doctor and
    /// supervisors. Daemon handler:
    /// [`pcloud_ipc::Method::GetHealth`]. Exit `0` on reachable,
    /// [`crate::exit_code::ExitCode::Network`] on transport failure.
    Health,
    /// `pcloudc pending` — enumerate in-flight transfer / sync work.
    /// Daemon handler: [`pcloud_ipc::Method::GetPending`]. Standard
    /// status-to-exit mapping.
    Pending,
    /// `pcloudc slo` — fetch the canonical Service-Level Objective
    /// report. Daemon handler: [`pcloud_ipc::Method::GetSlo`]. Response
    /// is a JSON [`pcloud_ipc::SloReportPayload`] and the command is
    /// field-selector-friendly: `pcloudc slo pass` extracts the
    /// aggregate bit, `pcloudc slo slos` returns the canonical list.
    /// Always exits [`crate::exit_code::ExitCode::Ok`] when the daemon
    /// is reachable, regardless of per-SLO status. Operators decide
    /// how to interpret violations.
    Slo,
    /// `pcloudc publink list` — list file/folder public links.
    /// Daemon handler: [`pcloud_ipc::Method::ListPublicLinks`].
    ListLinks,
    /// `pcloudc publink list-upload` — list upload-only public links.
    /// Daemon handler: [`pcloud_ipc::Method::ListUploadLinks`].
    ListUploadLinks,
    /// `pcloudc publink show <code>` — show metadata for one link.
    /// Daemon handler: `Request::ShowPublicLink`.
    ShowLink,
    /// `pcloudc publink delete <link-id>` — revoke a link.
    /// Daemon handler: `Request::DeletePublicLink`.
    DeleteLink,
    /// `pcloudc publink create-file <path>` — create a file link.
    /// Daemon handler: `Request::CreateFilePublicLink`.
    CreateFileLink,
    /// `pcloudc publink create-folder <path>` — create a folder link.
    /// Daemon handler: `Request::CreateFolderPublicLink`.
    CreateFolderLink,
    /// `pcloudc publink change-expire <id> [unix-ts]` — set/clear
    /// link expiry. Daemon handler: `Request::ChangePublicLinkExpire`.
    ChangeLinkExpire,
    /// `pcloudc publink change-password <id> [password]` — set or
    /// clear a link password. Secret flows via `SecretString`; daemon
    /// handler: `Request::ChangePublicLinkPassword`.
    ChangeLinkPassword,
    /// `pcloudc publink change-upload <id> <policy>` — change the
    /// upload policy. Daemon handler:
    /// `Request::ChangePublicLinkUpload`.
    ChangeLinkUpload,
    /// `pcloudc publink create-upload <path> [...]` — create an
    /// upload link. Daemon handler: `Request::CreateUploadLink`.
    CreateUploadLink,
    /// `pcloudc publink delete-upload <id>` — delete an upload link.
    /// Daemon handler: `Request::DeleteUploadLink`.
    DeleteUploadLink,
    /// `pcloudc publink create-tree <name> [...]` — create a tree
    /// (selection) link. Daemon handler:
    /// `Request::CreateTreePublicLink`.
    CreateTreeLink,
    /// `pcloudc publink access-list <id>` — list grant entries.
    /// Daemon handler: `Request::ListPublicLinkAccess`.
    ListLinkAccess,
    /// `pcloudc publink access-add <id> <email>` — add grantee.
    /// Daemon handler: `Request::AddPublicLinkAccess`.
    AddLinkAccess,
    /// `pcloudc publink access-remove <id> <receiver-id>` — revoke
    /// grantee. Daemon handler: `Request::RemovePublicLinkAccess`.
    RemoveLinkAccess,
    /// `pcloudc bookmark list` — list public bookmarks.
    /// Daemon handler: `Request::ListBookmarks`.
    ListBookmarks,
    /// `pcloudc bookmark remove <code> <location-id>`.
    /// Daemon handler: `Request::RemoveBookmark`.
    RemoveBookmark,
    /// `pcloudc bookmark change <code> <location-id> <name>
    /// <description>`. Daemon handler: `Request::ChangeBookmark`.
    ChangeBookmark,
    /// `pcloudc sync list` — list configured sync roots.
    /// Daemon handler: [`pcloud_ipc::Method::GetSyncRoots`].
    SyncList,
    /// `pcloudc sync add <local> <remote>` — register a new sync
    /// root. Daemon handler: `Request::SyncRootAdd` — rejects
    /// duplicate/nested roots with
    /// [`crate::exit_code::ExitCode::Conflict`].
    SyncAdd,
    /// `pcloudc sync remove <sync-id>` — unregister a sync root.
    /// Daemon handler: `Request::SyncRootRemove`.
    SyncRemove,
    /// `pcloudc sync status` — report background sync loop status.
    /// Daemon handler: [`pcloud_ipc::Method::GetSyncStatus`].
    SyncStatus,
    /// `pcloudc sync change-type <sync-id> <flavor>` — change the
    /// direction of an existing sync root. Daemon handler:
    /// `Request::SyncRootChangeType`. Accepted flavor aliases:
    /// `bilateral|full|both`, `mirror|download-only|down|remote-to-local`,
    /// `backup|upload-only|up|local-to-remote`.
    ///
    /// **Pre-alpha honesty note.** The `backup` alias currently maps
    /// to the same `UploadOnly` semantics as `upload-only` and **does**
    /// propagate local deletions to the remote. A true deletion-safe
    /// backup direction is tracked under a new bead (see `STATUS.md`
    /// open beads). For deletion-safe archival today, use
    /// `pcloudc backup snapshot-create` (GPG-encrypted tarball).
    SyncChangeType,
    /// `pcloudc userinfo` — fetch authenticated-user profile.
    /// Daemon handler: [`pcloud_ipc::Method::GetUserInfo`].
    UserInfo,
    /// `pcloudc pause` — pause all sync activity.
    /// Daemon handler: [`pcloud_ipc::Method::PauseSync`].
    Pause,
    /// `pcloudc resume` — resume sync activity.
    /// Daemon handler: [`pcloud_ipc::Method::ResumeSync`].
    Resume,
    /// `pcloudc login` (phase 1) — kick off the login state machine.
    /// Daemon handler: [`pcloud_ipc::Method::LoginBegin`].
    LoginBegin,
    /// `pcloudc logout` — drop the active session.
    /// Daemon handler: [`pcloud_ipc::Method::Logout`].
    Logout,
    /// `pcloudc login --tfa-channel sms` — request a fresh SMS OTP.
    /// Daemon handler:
    /// [`pcloud_ipc::Method::SendTwoFactorSms`].
    SendTwoFactorSms,
    /// `pcloudc login --tfa-channel notification` — resend the
    /// push-notification challenge.
    /// Daemon handler:
    /// [`pcloud_ipc::Method::SendTwoFactorNotification`].
    SendTwoFactorNotification,
    /// Submit the user's password. Secret flows via
    /// [`SecretInputs::password`] ([`SecretString`]); daemon handler:
    /// `Request::PasswordSubmission`. Auth failure →
    /// [`crate::exit_code::ExitCode::Auth`].
    SubmitPassword,
    /// Submit a persisted auth token. Secret flows via
    /// [`SecretInputs::auth_token`]; daemon handler:
    /// `Request::AuthTokenSubmission`.
    SubmitAuthToken,
    /// Submit a 6-digit TFA code. Daemon handler:
    /// `Request::TwoFactorCodeSubmission` with
    /// `recovery_code = false`.
    SubmitTwoFactorCode,
    /// Submit a one-shot TFA recovery code. Same daemon handler as
    /// `SubmitTwoFactorCode` but with `recovery_code = true`.
    SubmitRecoveryCode,
    /// Submit the crypto password to unlock the Crypto Folder.
    /// Secret flows via [`SecretInputs::crypto_password`]; daemon
    /// handler: `Request::CryptoUnlock`. Locked / wrong-password →
    /// [`crate::exit_code::ExitCode::CryptoLocked`].
    SubmitCryptoPassword,
    /// `pcloudc authsave` — toggle durable auth-token vault
    /// persistence (opt-in; see CLAUDE.md security rules).
    /// Daemon handler: `Request::AuthPersistence`.
    AuthSave,
    /// `pcloudc crypto lock` — re-lock the Crypto Folder.
    /// Daemon handler: [`pcloud_ipc::Method::LockCrypto`].
    LockCrypto,
    /// `pcloudc shutdown` — request an orderly daemon shutdown.
    /// Daemon handler: [`pcloud_ipc::Method::Shutdown`].
    Shutdown,
    /// `pcloudc drain` — operator-facing graceful-drain command. Reads
    /// the daemon pidfile from `<state_dir>/daemon.pid`, dispatches
    /// SIGTERM to that pid, and polls
    /// [`pcloud_ipc::Method::DrainStatus`] every 500 ms until the
    /// daemon reports `state == "stopped"` or the configured
    /// `upgrade.handoff_timeout_secs` expires.
    ///
    /// Entirely CLI-side: `into_request` maps to a harmless
    /// `DrainStatus` probe as a defensive fallback; the real work runs
    /// in `main.rs::run_daemon_drain`. Exit code: `Ok` on
    /// `state == "stopped"`, [`crate::exit_code::ExitCode::Unavailable`]
    /// on timeout or when the pidfile is missing.
    Drain,
    /// `pcloudc start` — local CLI-only command: spawn `pcloudd` in
    /// the background if not already running, redirect its stdio to
    /// `~/.pcloud/state/daemon.log`, and exit once the IPC socket is
    /// reachable. Handled entirely client-side; `into_request` maps
    /// this to a harmless `GetHealth` as a defensive fallback and the
    /// real work happens in `main.rs`. Exit code: `Ok` once the
    /// daemon's socket answers, `Unavailable` on spawn failure.
    Start,
    // Shares / business / teams
    /// `pcloudc shares list-incoming` —
    /// [`pcloud_ipc::Method::ListIncomingShares`].
    ListIncomingShares,
    /// `pcloudc shares list-outgoing` —
    /// [`pcloud_ipc::Method::ListOutgoingShares`].
    ListOutgoingShares,
    /// `pcloudc shares list-incoming-requests` —
    /// [`pcloud_ipc::Method::ListIncomingShareRequests`].
    ListIncomingShareRequests,
    /// `pcloudc shares list-outgoing-requests` —
    /// [`pcloud_ipc::Method::ListOutgoingShareRequests`].
    ListOutgoingShareRequests,
    /// `pcloudc contacts list` —
    /// [`pcloud_ipc::Method::ListContacts`].
    ListContacts,
    /// `pcloudc teams list` — [`pcloud_ipc::Method::ListMyTeams`].
    ListMyTeams,
    /// `pcloudc shares create <folder-id> <name> <mail> [...]`.
    /// Daemon handler: `Request::ShareFolder`. Permission bits are
    /// the C-compatible bitmask.
    ShareFolder,
    /// `pcloudc shares cancel-request <id>` —
    /// `Request::CancelShareRequest`.
    CancelShareRequest,
    /// `pcloudc shares decline-request <id>` —
    /// `Request::DeclineShareRequest`.
    DeclineShareRequest,
    /// `pcloudc shares accept-request <id> <to-folder> [name]` —
    /// `Request::AcceptShareRequest`.
    AcceptShareRequest,
    /// `pcloudc shares remove <share-id>` —
    /// `Request::RemoveShare`.
    RemoveShare,
    /// `pcloudc shares modify <share-id> <perm-bits>` —
    /// `Request::ModifyShare`.
    ModifyShare,
    /// `pcloudc account stop-share --users ... --teams ...` —
    /// `Request::AccountStopShare`.
    AccountStopShare,
    /// `pcloudc account modify-share --user-mods ... --team-mods ...`
    /// — `Request::AccountModifyShare`.
    AccountModifyShare,
    /// `pcloudc account team-share <folder-id> <name> <team-id>
    /// [...]` — `Request::AccountTeamShare`.
    AccountTeamShare,
    /// List pCloud notifications for the authenticated user.
    /// `pcloud-rs notifications list` / `list-notifications`. Mirrors
    /// C `psync_get_notifications`.
    ListNotifications,
    /// Mark all pending notifications up to and including `upto_id` as read.
    /// `pcloud-rs notifications mark-read <upto_id>`. Mirrors C
    /// `psync_mark_notificaitons_read` (header typo preserved).
    MarkNotificationsRead { upto_id: u64 },
    /// Report the daemon's current crypto lifecycle state.
    /// `pcloud-rs crypto status`. Mirrors C `psync_crypto_issetup` /
    /// `psync_crypto_isstarted`.
    CryptoStatus,
    /// Verify the tamper-evident audit chain.
    AuditVerify,
    /// Report session lifecycle status (expiry, last-used, refresh-in-flight).
    /// `pcloud-rs session status`.
    SessionStatus,
    /// Mount the pCloud filesystem at a local path (`pcloud-rs mount <path>`).
    Mount,
    /// Force-unmount an orphan pCloud FUSE mount at a local path
    /// (`pcloud-rs mount --force-umount <path>`). Used to recover after a
    /// crashed previous daemon left a mount entry behind. Tracker
    /// bd-1du.4 (P1.4).
    MountForceUnmount,
    /// Unmount the active pCloud filesystem (`pcloud-rs unmount`).
    Unmount,
    /// Trigger an immediate local-scan wakeup on the daemon engine
    /// (`pcloud-rs sync localscan`). Mirrors C `psync_run_localscan`.
    RunLocalScan,
    /// Mail an existing public-link `code` to one or more recipients
    /// (`pcloud-rs publink send <code> --to <emails> --message <text>`).
    /// Mirrors C `psync_send_publink`.
    SendPublink,
    /// Create a remote folder by absolute path
    /// (`pcloud-rs folder create <path>`). Mirrors C
    /// `psync_create_remote_folder_by_path` (`pclsync/psynclib.c:1006`).
    /// The CLI surface always uses the path-based form; the daemon
    /// accepts the parent-id + name form via the IPC payload (used by
    /// the SDK helper).
    CreateRemoteFolder,
    /// Resolve an absolute pCloud-drive path to its folder id.
    /// (`pcloud-rs folder id <path>`). Mirrors C
    /// `psync_get_fsfolderid_by_path` (`pclsync/psynclib.c:2170`).
    GetFolderIdByPath,
    /// Read folder flags/permissions for an absolute pCloud-drive path.
    /// (`pcloud-rs folder flags <path>`). Mirrors C
    /// `psync_get_fsfolderflags_by_id` (`pclsync/psynclib.c:2176`).
    GetFolderFlags,
    /// Read owner user id of a folder by absolute pCloud-drive path.
    /// (`pcloud-rs folder owner <path>`). Mirrors C
    /// `psync_get_folder_ownerid` (`pclsync/psynclib.c:2088`).
    GetFolderOwnerId,
    /// Classify a local path against the daemon's sync-root + engine
    /// state (`pcloud-rs fs status <local-path>`). Mirrors C
    /// `psync_filesystem_status` (`pclsync/psynclib.c:1903`).
    FilesystemStatus,
    /// Stat an absolute pCloud-drive path (`pcloudc stat <path>`).
    /// Mirrors C `psync_stat_path` (`pclsync/psynclib.h:743`).
    /// Resolves through local metadata cache, falls back to API.
    Stat,
    /// `pcloudc reload` — send SIGHUP to the daemon to trigger a
    /// config hot-reload. Looks up the daemon pidfile from
    /// `<state_dir>/daemon.pid`, sends SIGHUP via `kill(2)`, and
    /// exits. Handled entirely CLI-side; `into_request` maps to
    /// `GetHealth` as a defensive fallback.
    Reload,
    /// Run the `pcloudc doctor` self-diagnostic battery. Handled
    /// entirely CLI-side; never reaches [`Command::into_request`].
    Doctor,
    /// Import legacy C `pcloud-rs` client state (`~/.pcloud/.pclouddb`
    /// plus associated files) into the canonical XDG layout consumed
    /// by the Rust daemon. Handled entirely CLI-side; never reaches
    /// [`Command::into_request`]. Identified as a GA blocker by the
    /// R8 release review — users migrating from the C build must have
    /// a one-shot, non-destructive, preview-able migration path.
    MigrateFromC {
        /// When `true`, render the plan only and do not mutate the
        /// target directories.
        dry_run: bool,
        /// When `true`, allow overwriting an existing Rust
        /// `store.sqlite3` (destructive).
        force_overwrite: bool,
        /// Optional override for the legacy home (defaults to
        /// `$HOME/.pcloud`).
        from: Option<std::path::PathBuf>,
    },
    /// `pcloudc log <PATH> [--limit N]` — git-log-style revision history
    /// for a synced file. Daemon handler: `Request::FileHistory`.
    /// Honest scope: pCloud's public API does not expose the
    /// `listrevisions` endpoint to third-party clients; the daemon
    /// currently returns `Unavailable` with a tracker pointer
    /// (bd-1du.10). The CLI surface, IPC envelope and rendering are
    /// wired so enabling the endpoint is a one-change-site operation
    /// on the backend.
    /// Positional args (path, limit) live on [`SecretInputs`].
    FileHistory,
    /// `pcloudc diff <PATH> <REV_A> <REV_B>` — placeholder stub for the
    /// revision-diff follow-up. Handled entirely CLI-side; never reaches
    /// the daemon. Always exits `Unavailable` for now.
    FileDiff,
    /// `pcloudc restore <PATH> <REV>` — placeholder stub for the
    /// revision-restore follow-up. Handled entirely CLI-side; never
    /// reaches the daemon. Always exits `Unavailable` for now.
    FileRestore,
    /// `pcloudc verify <PATH> [--recursive] [--fix] [--yes]` — walk a
    /// synced tree and cross-check the local SHA256 of each file against
    /// the server-reported digest. R9 enhancement #12. Handled CLI-side
    /// via `crate::verify::run`; the companion `Request::VerifyPath`
    /// is reserved for a future daemon-walks-tree wire call. Exit codes:
    /// [`crate::exit_code::ExitCode::Ok`] when all rows match,
    /// [`crate::exit_code::ExitCode::Conflict`] on any SHA256 mismatch,
    /// [`crate::exit_code::ExitCode::Unavailable`] when every comparable
    /// row matched but at least one row was missing on one side.
    Verify {
        /// Absolute or relative local path to walk.
        path: std::path::PathBuf,
        /// When `true`, descend into subdirectories.
        recursive: bool,
        /// When `true`, attempt repairs: download server content for
        /// `[MISSING_LOCAL]`; surface a user-visible prompt for
        /// `[MISMATCH]` unless `yes` is also set.
        fix: bool,
        /// When `true` (paired with `fix`), auto-approve mismatch
        /// repairs without prompting.
        yes: bool,
    },
    /// `pcloudc snapshot create <path>
    /// [--zstd-level N] [--gpg-recipient EMAIL]` — default tar → zstd
    /// → SHA3-sealed-sidecar pipeline. When `--gpg-recipient` is set,
    /// the archive is wrapped in a GPG envelope and `<path>` must end
    /// with `.tar.zst.gpg`; otherwise it must end with `.tar.zst`.
    SnapshotCreate,
    /// `pcloudc snapshot restore <path> --yes` — destructive; requires
    /// `--yes` for non-interactive callers (or a TTY prompt in
    /// interactive mode). Accepts both new zstd archives and legacy
    /// `.tar.gpg` archives.
    SnapshotRestore,
    /// `pcloudc snapshot verify <path>` — non-mutating outer + inner
    /// integrity check (SHA3-sidecar, zstd decode, inner per-payload
    /// SHA-256).
    SnapshotVerify,
    /// `pcloudc snapshot prune <dir> --retention-days N [--yes]` —
    /// destructive retention sweep. `--retention-days` is required.
    SnapshotPrune,
    /// Deprecated alias for [`Self::SnapshotCreate`]. The CLI still
    /// accepts `pcloudc backup snapshot-create` (and the single-token
    /// `backup-snapshot-create`) for one release cycle and prints a
    /// one-line stderr warning when used.
    BackupSnapshotCreate,
    /// Deprecated alias for [`Self::SnapshotRestore`]. See
    /// [`Self::BackupSnapshotCreate`].
    BackupSnapshotRestore,
    /// Deprecated alias for [`Self::SnapshotVerify`]. See
    /// [`Self::BackupSnapshotCreate`].
    BackupSnapshotVerify,
    /// Deprecated alias for [`Self::SnapshotPrune`]. See
    /// [`Self::BackupSnapshotCreate`].
    BackupSnapshotPrune,
    /// `pcloudc integrity status` — fetch sweeper progress JSON.
    /// Daemon handler: [`pcloud_ipc::Method::IntegrityStatus`]. H14 PR4.
    /// Tracker: bd-1du.4.6.1.
    IntegrityStatus,
    /// `pcloudc integrity run-once` — synchronously trigger one
    /// background-integrity-sweeper cycle. Blocks until the cycle
    /// completes; renders the post-cycle progress summary. Daemon
    /// handler: [`pcloud_ipc::Request::IntegrityRunOnce`]. H14 PR4.
    IntegrityRunOnce,
    /// `pcloudc integrity skip <PATH>` — append a glob pattern to the
    /// configured skip-list file. Daemon handler:
    /// [`pcloud_ipc::Request::IntegritySkip`]. H14 PR4.
    IntegritySkip,
    /// `pcloudc ha status` — return the Tier-2 HA posture as
    /// `{mode, lease_owner, lease_age_s, lease_path}`. Daemon handler:
    /// [`pcloud_ipc::Method::HaStatus`]. See `docs/enterprise/ha.md`
    /// §4.2. Safe to call at any time; returns `mode = "disabled"`
    /// when HA is not configured.
    HaStatus,
    /// `pcloudc audit-verifier status` — return the scheduled
    /// audit-chain verifier status as JSON. Daemon handler:
    /// [`pcloud_ipc::Method::GetAuditVerifierStatus`]. Safe to call at
    /// any time; returns `enabled = false` when the verifier is disabled.
    AuditVerifierStatus,
    /// `pcloudc upload create <LOCAL> <REMOTE_NAME> [--parent <ID>]
    /// [--total-bytes <N>] [--conflict error|overwrite|skip|rename]` —
    /// register a new operator-visible upload session in the daemon's
    /// in-memory registry. Daemon handler: `Request::UploadCreate`.
    UploadCreate,
    /// `pcloudc upload pause <SESSION_ID>` — pause an in-flight upload
    /// session. Daemon handler: `Request::UploadPause`.
    UploadPause,
    /// `pcloudc upload resume <SESSION_ID>` — resume a paused upload
    /// session. Daemon handler: `Request::UploadResume`.
    UploadResume,
    /// `pcloudc upload cancel <SESSION_ID>` — cancel an upload session
    /// (non-terminal → Cancelled). Daemon handler:
    /// `Request::UploadCancel`.
    UploadCancel,
    /// `pcloudc upload list` — enumerate every upload session known
    /// to the running daemon. Daemon handler: `Request::UploadList`.
    UploadList,
    /// `pcloudc conflict list` — list unresolved sync conflicts queued
    /// in the engine scheduler. Daemon handler:
    /// [`pcloud_ipc::Method::ListConflicts`].
    ConflictList,
    /// `pcloudc conflict resolve <path> --prefer-local|--prefer-remote|
    /// --newest-wins|--rename-both` — manually resolve a specific
    /// conflict. Daemon handler: `Request::ConflictResolve`.
    ConflictResolve,
    // ── Crypto (Group A: IPC+handler already exist) ──────────────────────
    /// `pcloudc crypto reset` — wipe local crypto fingerprint and folder
    /// registry. Daemon handler: [`pcloud_ipc::Method::CryptoReset`].
    CryptoReset,
    /// `pcloudc crypto priv-key-flags` — return current crypto private-key
    /// flags as a decimal integer. Daemon handler:
    /// [`pcloud_ipc::Method::GetCryptoPrivKeyFlags`].
    CryptoPrivKeyFlags,
    /// `pcloudc crypto send-change-private` — request a server-side code to
    /// authorise a subsequent crypto password rotation. Daemon handler:
    /// [`pcloud_ipc::Method::SendCryptoChangeUserPrivate`].
    CryptoSendChangePrivate,
    /// `pcloudc crypto change-password` — rotate the crypto passphrase.
    /// Requires old + new password + confirmation code (from
    /// `send-change-private`). Daemon handler:
    /// `Request::CryptoChangePassword`.
    CryptoChangePassword,
    /// `pcloudc crypto change-password-unlocked` — rotate the crypto
    /// passphrase when the shell is already unlocked. Daemon handler:
    /// `Request::CryptoChangePasswordUnlocked`.
    CryptoChangePasswordUnlocked,
    /// `pcloudc crypto hint` — fetch the stored crypto passphrase hint.
    /// Daemon handler: [`pcloud_ipc::Method::GetCryptoHint`].
    CryptoHint,
    // ── Sync (Group A) ───────────────────────────────────────────────────
    /// `pcloudc sync suggest [<PATH>] [--max N]` — list candidate sync
    /// folders under PATH. Daemon handler: `Request::GetSyncSuggestions`.
    SyncSuggest,
    /// `pcloudc sync is-syncable <PATH>` — classify whether PATH can be
    /// added as a sync root. Daemon handler: `Request::IsFolderSyncable`.
    SyncIsSyncable,
    // ── Account / auth (Group B: new IPC variants) ───────────────────────
    /// `pcloudc account verify-email` — trigger a server-side verification
    /// email for the active session. Daemon handler:
    /// [`pcloud_ipc::Method::VerifyEmail`].
    AccountVerifyEmail,
    /// `pcloudc account verify-email-restricted <TOKEN>` — verify email via
    /// a restricted verify-token (no session auth required). Daemon handler:
    /// `Request::VerifyEmailRestricted`.
    AccountVerifyEmailRestricted,
    /// `pcloudc account lost-password <EMAIL>` — send a password-reset email.
    /// No auth required. Daemon handler: `Request::LostPassword`.
    AccountLostPassword,
    /// `pcloudc account change-password` — change the account password.
    /// Reads current + new password interactively. Daemon handler:
    /// `Request::AccountChangePassword`.
    AccountChangePassword,
    /// `pcloudc account register <EMAIL> [--accept-terms]` — register a new
    /// pCloud account. Daemon handler: `Request::AccountRegister`.
    AccountRegister,
    /// `pcloudc account api-servers` — list available pCloud API server
    /// regions. Daemon handler: [`pcloud_ipc::Method::GetApiServers`].
    AccountApiServers,
    /// `pcloudc account set-api-server <LOCATION_ID> <BINAPI>` — pin the
    /// daemon to a specific API region. Daemon handler:
    /// `Request::SetApiServer`.
    AccountSetApiServer,
    /// `pcloudc account set-language <LANG>` — set the account language
    /// preference. Daemon handler: `Request::SetLanguage`.
    AccountSetLanguage,
    /// `pcloudc account promo` — fetch the promotional URL for this
    /// platform. Daemon handler: [`pcloud_ipc::Method::GetPromo`].
    AccountPromo,
    // ── Transfers / downloads (Group B) ─────────────────────────────────
    /// `pcloudc download link <FILE_ID>` — resolve the download URL for a
    /// remote file. Daemon handler: `Request::GetFileLink`.
    DownloadLink,
    /// `pcloudc download file <FILE_ID> <LOCAL_PATH>` — download a remote
    /// file to a local path. Daemon handler: `Request::DownloadFile`.
    DownloadFile,
    // ── Backup (Group B) ─────────────────────────────────────────────────
    /// `pcloudc backup delete <BACKUP_ID>` — delete a backup by folder id.
    /// Daemon handler: `Request::DeleteBackup`.
    BackupDelete,
    /// `pcloudc backup create <NAME> <ROOT_FOLDER_ID> <LOCAL_PATH>` — create
    /// a new backup and register the local folder as an upload-only sync root.
    /// Daemon handler: `Request::CreateBackup`.
    BackupCreate,
    /// `pcloudc backup stop-device <DEVICE_FOLDER_ID>` — stop a device backup
    /// and remove the matching local sync root.
    /// Daemon handler: `Request::StopDevice`.
    BackupStopDevice,
    /// `pcloudc backup delete-device` — clear the local device backup
    /// registration (local-only, no network call).
    /// Daemon handler: `Request::DeleteBackupDevice`.
    BackupDeleteDevice,
    /// `pcloudc publink create-tree-from-paths <NAME> <PATHS...>` — create a
    /// tree link by resolving paths to ids via the daemon path resolver.
    /// Daemon handler: `Request::CreateTreePublicLink` (after path resolution).
    CreateTreeLinkFromPaths,
}

/// CLI-side secret-bearing state held for the duration of the interactive
/// session. Secret-sensitive fields (`password`, `auth_token`,
/// `crypto_password`, `public_link_password`) are wrapped in `SecretString`
/// so they zeroize on drop and redact in `Debug` output. This closes audit
/// finding H1 for the long-lived CLI state path.
///
/// NOTE: `Clone`/`PartialEq` are intentionally not derived. `SecretString`
/// deliberately omits `Clone` (audit M3 hardening) — use
/// `SecretString::clone_secret()` at audit-visible sites if duplication is
/// ever required.
#[derive(Debug)]
pub struct SecretInputs {
    pub username: String,
    pub password: SecretString,
    pub auth_token: SecretString,
    pub two_factor_code: String,
    pub trust_device: bool,
    pub recovery_code: bool,
    pub crypto_password: SecretString,
    pub auth_persistence_enabled: bool,
    pub local_path: String,
    pub remote_path: String,
    pub sync_id: u64,
    /// Optional sync-direction flavor for `sync add --type <FLAVOR>`.
    /// `None` → daemon default (`SyncType::Full`, bilateral).
    pub sync_type: Option<SyncType>,
    /// Required sync-direction flavor for `sync change-type
    /// <SYNC-ID> <FLAVOR>`. Populated by the parser; the dispatcher
    /// errors if the command is requested without a valid flavor.
    pub sync_type_required: Option<SyncType>,
    pub public_link_code: String,
    pub public_link_id: u64,
    pub public_link_path: String,
    pub public_link_expire: Option<u64>,
    pub public_link_password: Option<SecretString>,
    pub public_link_upload_policy: PublicLinkUploadPolicy,
    pub upload_link_comment: String,
    pub upload_link_expire: Option<u64>,
    pub upload_link_maxspace: Option<u64>,
    pub upload_link_maxfiles: Option<u64>,
    pub tree_link_name: String,
    pub tree_root_folder_id: Option<u64>,
    pub tree_folder_ids_csv: Option<String>,
    pub tree_file_ids_csv: Option<String>,
    pub tree_link_expire: Option<u64>,
    pub tree_link_maxdownloads: Option<u64>,
    pub tree_link_maxtraffic: Option<u64>,
    pub public_link_email: String,
    pub public_link_receiver_id: u64,
    pub bookmark_code: String,
    pub bookmark_location_id: u64,
    pub bookmark_name: String,
    pub bookmark_description: String,
    // Shares / business / teams
    pub share_folder_id: u64,
    pub share_name: String,
    pub share_mail: String,
    pub share_message: String,
    pub share_permissions_bits: u32,
    pub share_hint: Option<String>,
    pub share_request_id: u64,
    pub share_id: u64,
    pub share_to_folder_id: u64,
    pub share_accept_name: Option<String>,
    pub share_user_ids: Vec<u64>,
    pub share_team_ids: Vec<u64>,
    pub share_user_mods: Vec<(u64, u32)>,
    pub share_team_mods: Vec<(u64, u32)>,
    pub share_team_id: u64,
    /// `audit verify` inclusive id range. `None` means "from genesis" /
    /// "to latest".
    pub audit_from_id: Option<i64>,
    pub audit_to_id: Option<i64>,
    /// Target mountpoint for `Command::Mount`.
    pub mount_path: std::path::PathBuf,
    /// One-shot `-m/--mountpoint` override for `Command::Mount`. When
    /// present, overrides the positional path argument for this single
    /// invocation only. Parsed by `parse_inputs_for_command`; plumbed
    /// into `Request::Mount.path` in `into_request`.
    pub mount_flag_path: Option<std::path::PathBuf>,
    /// One-shot `-O/--fuse-opts` override for `Command::Mount`. Parse
    /// only in the current release: the CLI accepts and forwards the
    /// flag to the daemon, which currently logs the override and then
    /// consults its configured defaults (honest scope — see the
    /// manpage mount section). Not yet honoured at runtime.
    pub mount_flag_fuse_opts: Option<String>,
    /// One-shot `--cache-size` override for `Command::Mount`, in GiB.
    /// Parse only in the current release (same caveat as
    /// [`Self::mount_flag_fuse_opts`]).
    pub mount_flag_cache_size_gb: Option<u64>,
    /// Comma-separated recipient list for `Command::SendPublink`.
    pub send_publink_mails: String,
    /// Optional message body for `Command::SendPublink`.
    pub send_publink_message: String,
    /// Absolute remote pCloud-drive path for
    /// `Command::CreateRemoteFolder`.
    pub remote_folder_path: String,
    /// Absolute remote pCloud-drive path for folder metadata helpers
    /// (`Command::GetFolderIdByPath`, `Command::GetFolderFlags`,
    /// `Command::GetFolderOwnerId`).
    pub folder_metadata_remote_path: String,
    /// Absolute local filesystem path for `Command::FilesystemStatus`.
    pub filesystem_status_local_path: String,
    /// Absolute remote pCloud-drive path for `Command::Stat`.
    pub stat_remote_path: String,
    /// Absolute remote pCloud-drive path for `Command::FileHistory` /
    /// `Command::FileDiff` / `Command::FileRestore`.
    pub file_history_path: String,
    /// Optional row cap for `Command::FileHistory`.
    pub file_history_limit: Option<u32>,
    /// Left-hand revision hex id for `Command::FileDiff`.
    pub file_diff_rev_a: String,
    /// Right-hand revision hex id for `Command::FileDiff`.
    pub file_diff_rev_b: String,
    /// Target revision hex id for `Command::FileRestore`.
    pub file_restore_rev: String,
    /// Positional local path for `Command::Verify` (R9 #12).
    pub verify_local_path: String,
    /// `--recursive` flag for `Command::Verify`.
    pub verify_recursive: bool,
    /// `--fix` flag for `Command::Verify`.
    pub verify_fix: bool,
    /// `--yes` flag for `Command::Verify` (auto-approve fix prompts).
    pub verify_yes: bool,
    /// Positional path for `Command::BackupSnapshot{Create,Restore,
    /// Verify,Prune}` (H12 PR1).
    pub snapshot_path: std::path::PathBuf,
    /// `--gpg-recipient EMAIL` flag for backup snapshot commands.
    pub snapshot_gpg_recipient: Option<String>,
    /// `--yes` flag for destructive backup snapshot commands.
    pub snapshot_yes: bool,
    /// `--retention-days N` flag for `Command::BackupSnapshotPrune`.
    pub snapshot_retention_days: Option<u32>,
    /// `--zstd-level N` flag for `Command::SnapshotCreate` (range
    /// `1..=22`; `None` → daemon default). Ignored by other snapshot
    /// subcommands.
    pub snapshot_zstd_level: Option<i32>,
    /// Positional glob pattern for `Command::IntegritySkip` (H14 PR4).
    pub integrity_skip_pattern: String,
    /// Local path for `Command::UploadCreate`.
    pub upload_local_path: std::path::PathBuf,
    /// Remote filename for `Command::UploadCreate`.
    pub upload_remote_name: String,
    /// Optional parent folder id for `Command::UploadCreate`.
    pub upload_parent_folder_id: Option<u64>,
    /// Declared total byte size for `Command::UploadCreate`.
    pub upload_total_bytes: u64,
    /// Conflict mode for `Command::UploadCreate` (serde-compatible).
    pub upload_conflict_mode: Option<pcloud_ipc::UploadConflictMode>,
    /// Session id for upload pause/resume/cancel.
    pub upload_session_id: u64,
    /// Path for `Command::ConflictResolve`.
    pub conflict_path: String,
    /// Policy for `Command::ConflictResolve`.
    pub conflict_resolve_policy: String,
    // ── Crypto change-password fields ────────────────────────────────────
    /// New crypto passphrase for `Command::CryptoChangePassword` /
    /// `Command::CryptoChangePasswordUnlocked`. Transit-only secret.
    pub new_crypto_password: SecretString,
    /// Passphrase hint for crypto change-password commands.
    pub crypto_change_hint: String,
    /// Server-side confirmation code (from `send-change-private`) for
    /// crypto change-password commands.
    pub crypto_change_code: String,
    /// Updated `crypto_private_flags` row for crypto change-password.
    pub crypto_change_flags: u64,
    // ── Account ops ───────────────────────────────────────────────────────
    /// New account password for `Command::AccountChangePassword`. Transit-only.
    pub account_new_password: SecretString,
    /// Verify token for `Command::AccountVerifyEmailRestricted`.
    pub account_verify_token: String,
    /// ToS acceptance flag for `Command::AccountRegister`.
    pub account_terms_accepted: bool,
    /// IETF language tag for `Command::AccountSetLanguage`.
    pub account_language: String,
    // ── Sync suggestions ─────────────────────────────────────────────────
    /// Optional base path for `Command::SyncSuggest`.
    pub sync_suggest_path: Option<String>,
    /// Optional hard cap for `Command::SyncSuggest`.
    pub sync_suggest_max: Option<usize>,
    // ── Download / transfer ───────────────────────────────────────────────
    /// Remote file id for `Command::DownloadLink` / `Command::DownloadFile`.
    pub download_file_id: u64,
    /// Absolute local destination path for `Command::DownloadFile`.
    pub download_local_path: std::path::PathBuf,
    // ── API server selection ──────────────────────────────────────────────
    /// pCloud location id for `Command::AccountSetApiServer`.
    pub api_server_location_id: u32,
    /// Binary API hostname for `Command::AccountSetApiServer`.
    pub api_server_binapi: String,
    // ── Backup ────────────────────────────────────────────────────────────
    /// Backup folder id for `Command::BackupDelete`.
    pub backup_delete_id: u64,
    /// Display name for `Command::BackupCreate`.
    pub backup_create_name: String,
    /// Remote root folder id for `Command::BackupCreate`.
    pub backup_create_root_folder_id: u64,
    /// Absolute local path for `Command::BackupCreate`.
    pub backup_create_local_path: String,
    /// Optional parent folder name for `Command::BackupCreate`.
    pub backup_create_parent_folder_name: Option<String>,
    /// Device folder id for `Command::BackupStopDevice`.
    pub backup_device_folder_id: u64,
    // ── Tree link from paths ──────────────────────────────────────────────
    /// Paths to resolve for `Command::CreateTreeLinkFromPaths`.
    pub tree_link_paths: Vec<String>,
}

impl Command {
    #[must_use]
    pub fn into_request(self, inputs: &SecretInputs) -> Request {
        match self {
            Self::Help => Request::Plain {
                method: Method::GetHealth,
            },
            Self::Status => Request::Plain {
                method: Method::GetStatus,
            },
            Self::Health => Request::Plain {
                method: Method::GetHealth,
            },
            Self::Pending => Request::Plain {
                method: Method::GetPending,
            },
            Self::Slo => Request::Plain {
                method: Method::GetSlo,
            },
            Self::ListLinks => Request::Plain {
                method: Method::ListPublicLinks,
            },
            Self::ListNotifications => Request::Plain {
                method: Method::ListNotifications,
            },
            Self::MarkNotificationsRead { upto_id } => Request::MarkNotificationsRead { upto_id },
            Self::CryptoStatus => Request::Plain {
                method: Method::GetCryptoStatus,
            },
            Self::ListUploadLinks => Request::Plain {
                method: Method::ListUploadLinks,
            },
            Self::ShowLink => Request::ShowPublicLink {
                code: inputs.public_link_code.clone(),
            },
            Self::DeleteLink => {
                // The CLI parser populates `public_link_code` only
                // when the argument wasn't numeric; in that case we
                // route to the code-form delete variant so the daemon
                // can resolve via `list_public_links`.
                if !inputs.public_link_code.is_empty() {
                    Request::DeletePublicLinkByCode {
                        code: inputs.public_link_code.clone(),
                    }
                } else {
                    Request::DeletePublicLink {
                        link_id: inputs.public_link_id,
                    }
                }
            }
            Self::CreateFileLink => Request::CreateFilePublicLink {
                path: inputs.public_link_path.clone(),
            },
            Self::CreateFolderLink => Request::CreateFolderPublicLink {
                path: inputs.public_link_path.clone(),
            },
            Self::ChangeLinkExpire => Request::ChangePublicLinkExpire {
                link_id: inputs.public_link_id,
                expire: inputs.public_link_expire,
            },
            Self::ChangeLinkPassword => Request::ChangePublicLinkPassword {
                link_id: inputs.public_link_id,
                // Expose secret only for the send-and-forget IPC dispatch; the
                // String copy lives only until the transport serializes the
                // request.
                password: inputs
                    .public_link_password
                    .as_ref()
                    .map(|s| s.expose_secret().to_owned().into()),
            },
            Self::ChangeLinkUpload => Request::ChangePublicLinkUpload {
                link_id: inputs.public_link_id,
                policy: inputs.public_link_upload_policy,
            },
            Self::CreateUploadLink => Request::CreateUploadLink {
                path: inputs.public_link_path.clone(),
                comment: inputs.upload_link_comment.clone(),
                expire: inputs.upload_link_expire,
                maxspace: inputs.upload_link_maxspace,
                maxfiles: inputs.upload_link_maxfiles,
            },
            Self::DeleteUploadLink => Request::DeleteUploadLink {
                upload_link_id: inputs.public_link_id,
            },
            Self::CreateTreeLink => Request::CreateTreePublicLink {
                name: inputs.tree_link_name.clone(),
                root_folder_id: inputs.tree_root_folder_id,
                folder_ids_csv: inputs.tree_folder_ids_csv.clone(),
                file_ids_csv: inputs.tree_file_ids_csv.clone(),
                expire: inputs.tree_link_expire,
                maxdownloads: inputs.tree_link_maxdownloads,
                maxtraffic: inputs.tree_link_maxtraffic,
            },
            Self::ListLinkAccess => Request::ListPublicLinkAccess {
                link_id: inputs.public_link_id,
            },
            Self::AddLinkAccess => Request::AddPublicLinkAccess {
                link_id: inputs.public_link_id,
                email: inputs.public_link_email.clone(),
            },
            Self::RemoveLinkAccess => Request::RemovePublicLinkAccess {
                link_id: inputs.public_link_id,
                receiver_id: inputs.public_link_receiver_id,
            },
            Self::ListBookmarks => Request::ListBookmarks,
            Self::RemoveBookmark => Request::RemoveBookmark {
                code: inputs.bookmark_code.clone(),
                location_id: inputs.bookmark_location_id,
            },
            Self::ChangeBookmark => Request::ChangeBookmark {
                code: inputs.bookmark_code.clone(),
                location_id: inputs.bookmark_location_id,
                name: inputs.bookmark_name.clone(),
                description: inputs.bookmark_description.clone(),
            },
            Self::SyncList => Request::Plain {
                method: Method::GetSyncRoots,
            },
            Self::SyncStatus => Request::Plain {
                method: Method::GetSyncStatus,
            },
            Self::SyncAdd => Request::SyncRootAdd {
                local_path: inputs.local_path.clone(),
                remote_path: inputs.remote_path.clone(),
                sync_type: inputs.sync_type,
            },
            Self::SyncRemove => Request::SyncRootRemove {
                sync_id: inputs.sync_id,
            },
            Self::SyncChangeType => Request::SyncRootChangeType {
                sync_id: inputs.sync_id,
                // Parser rejects missing flavor; defensive default
                // matches the daemon default (Full / bilateral) if we
                // somehow reach this arm without a parsed flavor.
                sync_type: inputs.sync_type_required.unwrap_or(SyncType::Full),
            },
            Self::UserInfo => Request::Plain {
                method: Method::GetUserInfo,
            },
            Self::Pause => Request::Plain {
                method: Method::PauseSync,
            },
            Self::Resume => Request::Plain {
                method: Method::ResumeSync,
            },
            Self::LoginBegin => Request::Plain {
                method: Method::LoginBegin,
            },
            Self::Logout => Request::Plain {
                method: Method::Logout,
            },
            Self::SendTwoFactorSms => Request::Plain {
                method: Method::SendTwoFactorSms,
            },
            Self::SendTwoFactorNotification => Request::Plain {
                method: Method::SendTwoFactorNotification,
            },
            Self::SubmitPassword => Request::PasswordSubmission {
                username: inputs.username.clone(),
                // Secret is exposed only here, right before IPC dispatch.
                // The request is short-lived send-and-forget (see
                // pcloud-ipc/src/methods.rs NOTE on secret handling).
                value: inputs.password.expose_secret().to_owned().into(),
            },
            Self::SubmitAuthToken => Request::AuthTokenSubmission {
                value: inputs.auth_token.expose_secret().to_owned().into(),
            },
            Self::SubmitTwoFactorCode | Self::SubmitRecoveryCode => {
                Request::TwoFactorCodeSubmission {
                    value: inputs.two_factor_code.clone(),
                    trust_device: inputs.trust_device,
                    recovery_code: inputs.recovery_code,
                }
            }
            Self::SubmitCryptoPassword => Request::CryptoUnlock {
                password: inputs.crypto_password.expose_secret().to_owned().into(),
            },
            Self::AuthSave => Request::AuthPersistence {
                enabled: inputs.auth_persistence_enabled,
            },
            Self::LockCrypto => Request::Plain {
                method: Method::LockCrypto,
            },
            // `Start` is handled entirely client-side in `main.rs` and
            // must never reach this dispatch; mapping it to `Plain/Help`
            // is a defensive fallback.
            Self::Start => Request::Plain {
                method: Method::GetHealth,
            },
            Self::Shutdown => Request::Plain {
                method: Method::Shutdown,
            },
            // `Drain` is handled entirely client-side in `main.rs`
            // (pidfile lookup → SIGTERM → poll). Mapping it to a
            // harmless DrainStatus probe is a defensive fallback that
            // keeps the dispatch table total if the command somehow
            // reaches here.
            Self::Drain => Request::Plain {
                method: Method::DrainStatus,
            },
            // `Reload` is handled entirely client-side in `main.rs`
            // (pidfile lookup → SIGHUP). Mapping it to GetHealth as a
            // defensive fallback.
            Self::Reload => Request::Plain {
                method: Method::GetHealth,
            },
            Self::ListIncomingShares => Request::Plain {
                method: Method::ListIncomingShares,
            },
            Self::ListOutgoingShares => Request::Plain {
                method: Method::ListOutgoingShares,
            },
            Self::ListIncomingShareRequests => Request::Plain {
                method: Method::ListIncomingShareRequests,
            },
            Self::ListOutgoingShareRequests => Request::Plain {
                method: Method::ListOutgoingShareRequests,
            },
            Self::ListContacts => Request::Plain {
                method: Method::ListContacts,
            },
            Self::ListMyTeams => Request::Plain {
                method: Method::ListMyTeams,
            },
            Self::ShareFolder => Request::ShareFolder {
                folder_id: inputs.share_folder_id,
                name: inputs.share_name.clone(),
                mail: inputs.share_mail.clone(),
                message: inputs.share_message.clone(),
                permissions_bits: inputs.share_permissions_bits,
                hint: inputs.share_hint.clone(),
            },
            Self::CancelShareRequest => Request::CancelShareRequest {
                share_request_id: inputs.share_request_id,
            },
            Self::DeclineShareRequest => Request::DeclineShareRequest {
                share_request_id: inputs.share_request_id,
            },
            Self::AcceptShareRequest => Request::AcceptShareRequest {
                share_request_id: inputs.share_request_id,
                to_folder_id: inputs.share_to_folder_id,
                name: inputs.share_accept_name.clone(),
            },
            Self::RemoveShare => Request::RemoveShare {
                share_id: inputs.share_id,
            },
            Self::ModifyShare => Request::ModifyShare {
                share_id: inputs.share_id,
                permissions_bits: inputs.share_permissions_bits,
            },
            Self::AccountStopShare => Request::AccountStopShare {
                user_share_ids: inputs.share_user_ids.clone(),
                team_share_ids: inputs.share_team_ids.clone(),
            },
            Self::AccountModifyShare => Request::AccountModifyShare {
                user_shares: inputs.share_user_mods.clone(),
                team_shares: inputs.share_team_mods.clone(),
            },
            Self::AccountTeamShare => Request::AccountTeamShare {
                folder_id: inputs.share_folder_id,
                name: inputs.share_name.clone(),
                team_id: inputs.share_team_id,
                message: inputs.share_message.clone(),
                permissions_bits: inputs.share_permissions_bits,
                hint: inputs.share_hint.clone(),
            },
            Self::AuditVerify => Request::AuditVerifyChain {
                range: AuditVerifyRange {
                    from: inputs.audit_from_id,
                    to: inputs.audit_to_id,
                },
            },
            Self::SessionStatus => Request::Plain {
                method: Method::SessionStatus,
            },
            Self::Mount => Request::Mount {
                path: inputs.mount_path.clone(),
            },
            Self::MountForceUnmount => Request::MountForceUnmount {
                path: inputs.mount_path.clone(),
            },
            // `PCLOUD_FORCE_UMOUNT=1 pcloudc unmount` routes the plain
            // `unmount` surface to the force-unmount path for the
            // currently-active mount. When the envvar is set to a
            // truthy value and the CLI knows the mountpoint (either
            // the positional path on `inputs` or the config default),
            // we dispatch `MountForceUnmount`; otherwise we fall
            // through to the regular graceful `Unmount`.
            Self::Unmount => {
                if env_force_umount_enabled() && !inputs.mount_path.as_os_str().is_empty() {
                    Request::MountForceUnmount {
                        path: inputs.mount_path.clone(),
                    }
                } else {
                    Request::Unmount
                }
            }
            Self::RunLocalScan => Request::RunLocalScan,
            Self::SendPublink => Request::SendPublink {
                code: inputs.public_link_code.clone(),
                mails: inputs.send_publink_mails.clone(),
                message: inputs.send_publink_message.clone(),
            },
            Self::CreateRemoteFolder => Request::CreateRemoteFolder {
                parent_folder_id: None,
                name: String::new(),
                path: inputs.remote_folder_path.clone(),
                check_and_create: false,
            },
            Self::GetFolderIdByPath => Request::GetFolderIdByPath {
                path: inputs.folder_metadata_remote_path.clone(),
            },
            Self::GetFolderFlags => Request::GetFolderFlags {
                path: inputs.folder_metadata_remote_path.clone(),
            },
            Self::GetFolderOwnerId => Request::GetFolderOwnerId {
                path: inputs.folder_metadata_remote_path.clone(),
            },
            Self::FilesystemStatus => Request::FilesystemStatus {
                path: inputs.filesystem_status_local_path.clone(),
            },
            Self::Stat => Request::StatPath {
                path: inputs.stat_remote_path.clone(),
            },
            // `Doctor` is handled entirely CLI-side in `main.rs` and
            // must never reach this dispatch; mapping it to `GetHealth`
            // is a defensive fallback equivalent to `Start`.
            Self::Doctor => Request::Plain {
                method: Method::GetHealth,
            },
            // `MigrateFromC` is likewise CLI-side only; mapping to a
            // harmless `GetHealth` defends against a main.rs regression.
            Self::MigrateFromC { .. } => Request::Plain {
                method: Method::GetHealth,
            },
            Self::FileHistory => Request::FileHistory {
                path: inputs.file_history_path.clone(),
                limit: inputs.file_history_limit,
            },
            // `FileDiff` / `FileRestore` are CLI-side stubs; they never
            // reach the daemon. Mapping to `GetHealth` is a defensive
            // fallback equivalent to `Doctor`.
            Self::FileDiff | Self::FileRestore => Request::Plain {
                method: Method::GetHealth,
            },
            // `Verify` is handled CLI-side by `crate::verify::run`. The
            // matching `Request::VerifyPath` variant is constructed here
            // so a future daemon-walks-tree implementation has a
            // one-change-site enable path. The command variant itself
            // carries defaults from `parse_single_token`; the actual
            // positional and flag values flow through `SecretInputs`
            // (populated by `parse_inputs_for_command`).
            Self::Verify { .. } => Request::VerifyPath {
                path: inputs.verify_local_path.clone(),
                recursive: inputs.verify_recursive,
            },
            Self::SnapshotCreate | Self::BackupSnapshotCreate => Request::BackupSnapshot {
                action: SnapshotAction::Create,
                path: inputs.snapshot_path.clone(),
                gpg_recipient: inputs.snapshot_gpg_recipient.clone(),
                yes: inputs.snapshot_yes,
                retention_days: inputs.snapshot_retention_days,
                zstd_level: inputs.snapshot_zstd_level,
            },
            Self::SnapshotRestore | Self::BackupSnapshotRestore => Request::BackupSnapshot {
                action: SnapshotAction::Restore,
                path: inputs.snapshot_path.clone(),
                gpg_recipient: inputs.snapshot_gpg_recipient.clone(),
                yes: inputs.snapshot_yes,
                retention_days: inputs.snapshot_retention_days,
                zstd_level: None,
            },
            Self::SnapshotVerify | Self::BackupSnapshotVerify => Request::BackupSnapshot {
                action: SnapshotAction::Verify,
                path: inputs.snapshot_path.clone(),
                gpg_recipient: inputs.snapshot_gpg_recipient.clone(),
                yes: inputs.snapshot_yes,
                retention_days: inputs.snapshot_retention_days,
                zstd_level: None,
            },
            Self::SnapshotPrune | Self::BackupSnapshotPrune => Request::BackupSnapshot {
                action: SnapshotAction::Prune,
                path: inputs.snapshot_path.clone(),
                gpg_recipient: inputs.snapshot_gpg_recipient.clone(),
                yes: inputs.snapshot_yes,
                retention_days: inputs.snapshot_retention_days,
                zstd_level: None,
            },
            // H14 PR4 — integrity sweeper subcommands. bd-1du.4.6.1.
            Self::IntegrityStatus => Request::Plain {
                method: Method::IntegrityStatus,
            },
            Self::IntegrityRunOnce => Request::IntegrityRunOnce,
            Self::IntegritySkip => Request::IntegritySkip {
                path: inputs.integrity_skip_pattern.clone(),
            },
            Self::HaStatus => Request::Plain {
                method: Method::HaStatus,
            },
            Self::AuditVerifierStatus => Request::Plain {
                method: Method::GetAuditVerifierStatus,
            },
            Self::UploadCreate => Request::UploadCreate {
                local_path: inputs.upload_local_path.clone(),
                remote_name: inputs.upload_remote_name.clone(),
                parent_folder_id: inputs.upload_parent_folder_id,
                total_bytes: inputs.upload_total_bytes,
                conflict_mode: inputs.upload_conflict_mode,
            },
            Self::UploadPause => Request::UploadPause {
                session_id: inputs.upload_session_id,
            },
            Self::UploadResume => Request::UploadResume {
                session_id: inputs.upload_session_id,
            },
            Self::UploadCancel => Request::UploadCancel {
                session_id: inputs.upload_session_id,
            },
            Self::UploadList => Request::UploadList,
            Self::ConflictList => Request::ConflictList,
            Self::ConflictResolve => Request::ConflictResolve {
                path: inputs.conflict_path.clone(),
                policy: inputs.conflict_resolve_policy.clone(),
            },
            // ── Crypto (Group A) ─────────────────────────────────────────
            Self::CryptoReset => Request::Plain {
                method: Method::CryptoReset,
            },
            Self::CryptoPrivKeyFlags => Request::Plain {
                method: Method::GetCryptoPrivKeyFlags,
            },
            Self::CryptoSendChangePrivate => Request::Plain {
                method: Method::SendCryptoChangeUserPrivate,
            },
            Self::CryptoChangePassword => Request::CryptoChangePassword {
                old_password: inputs.crypto_password.expose_secret().to_owned().into(),
                new_password: inputs.new_crypto_password.expose_secret().to_owned().into(),
                hint: inputs.crypto_change_hint.clone(),
                code: inputs.crypto_change_code.clone(),
                flags: inputs.crypto_change_flags,
            },
            Self::CryptoChangePasswordUnlocked => Request::CryptoChangePasswordUnlocked {
                new_password: inputs.new_crypto_password.expose_secret().to_owned().into(),
                hint: inputs.crypto_change_hint.clone(),
                code: inputs.crypto_change_code.clone(),
                flags: inputs.crypto_change_flags,
            },
            Self::CryptoHint => Request::Plain {
                method: Method::GetCryptoHint,
            },
            // ── Sync (Group A) ───────────────────────────────────────────
            Self::SyncSuggest => Request::GetSyncSuggestions {
                path: inputs.sync_suggest_path.clone().unwrap_or_default(),
                max: inputs.sync_suggest_max,
            },
            Self::SyncIsSyncable => Request::IsFolderSyncable {
                path: inputs.local_path.clone(),
            },
            // ── Account (Group B) ────────────────────────────────────────
            Self::AccountVerifyEmail => Request::Plain {
                method: Method::VerifyEmail,
            },
            Self::AccountVerifyEmailRestricted => Request::VerifyEmailRestricted {
                verify_token: inputs.account_verify_token.clone(),
            },
            Self::AccountLostPassword => Request::LostPassword {
                email: inputs.username.clone(),
            },
            Self::AccountChangePassword => Request::AccountChangePassword {
                current_password: inputs.password.expose_secret().to_owned().into(),
                new_password: inputs
                    .account_new_password
                    .expose_secret()
                    .to_owned()
                    .into(),
            },
            Self::AccountRegister => Request::AccountRegister {
                email: inputs.username.clone(),
                password: inputs.password.expose_secret().to_owned().into(),
                terms_accepted: inputs.account_terms_accepted,
            },
            Self::AccountApiServers => Request::Plain {
                method: Method::GetApiServers,
            },
            Self::AccountSetApiServer => Request::SetApiServer {
                location_id: inputs.api_server_location_id,
                binapi: inputs.api_server_binapi.clone(),
            },
            Self::AccountSetLanguage => Request::SetLanguage {
                language: inputs.account_language.clone(),
            },
            Self::AccountPromo => Request::Plain {
                method: Method::GetPromo,
            },
            // ── Transfers / downloads (Group B) ──────────────────────────
            Self::DownloadLink => Request::GetFileLink {
                file_id: inputs.download_file_id,
            },
            Self::DownloadFile => Request::DownloadFile {
                file_id: inputs.download_file_id,
                local_path: inputs.download_local_path.clone(),
            },
            // ── Backup (Group B) ─────────────────────────────────────────
            Self::BackupDelete => Request::DeleteBackup {
                backup_id: inputs.backup_delete_id,
            },
            Self::BackupCreate => Request::CreateBackup {
                name: inputs.backup_create_name.clone(),
                root_folder_id: inputs.backup_create_root_folder_id,
                local_path: inputs.backup_create_local_path.clone(),
                parent_folder_name: inputs.backup_create_parent_folder_name.clone(),
            },
            Self::BackupStopDevice => Request::StopDevice {
                device_folder_id: inputs.backup_device_folder_id,
            },
            Self::BackupDeleteDevice => Request::DeleteBackupDevice,
            // ── Tree link from paths ──────────────────────────────────────
            // Resolves each pCloud-drive path to a remote folder id on the
            // daemon side via the authenticated path resolver, then creates
            // the tree public link. bd-1du row 149.
            Self::CreateTreeLinkFromPaths => Request::CreateTreePublicLinkFromPaths {
                name: inputs.tree_link_name.clone(),
                paths: inputs.tree_link_paths.clone(),
                expires: inputs.tree_link_expire,
            },
        }
    }
}

/// Read the `PCLOUD_FORCE_UMOUNT` environment variable and decide
/// whether a bare `pcloudc unmount` invocation should be promoted to a
/// forceful `MountForceUnmount` request.
///
/// Truthy tokens: `1`, `true`, `yes`, `on` (case-insensitive). Any other
/// value — including empty string — is treated as "disabled" so the
/// graceful `Unmount` path remains the default.
///
/// Kept in this module so `Command::into_request` can reach it without
/// depending on the higher-level CLI parsing logic in `app.rs`.
pub(crate) fn env_force_umount_enabled() -> bool {
    match std::env::var("PCLOUD_FORCE_UMOUNT") {
        Ok(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_model::shares::SharePermissions;

    /// Shared guard to serialize env-var mutation in this module's
    /// tests (same pattern as `globals::tests::trace_env_guard`).
    fn force_umount_env_guard() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    fn fresh_inputs() -> SecretInputs {
        SecretInputs {
            username: String::new(),
            password: SecretString::new(String::new()),
            auth_token: SecretString::new(String::new()),
            two_factor_code: String::new(),
            trust_device: false,
            recovery_code: false,
            crypto_password: SecretString::new(String::new()),
            auth_persistence_enabled: false,
            local_path: String::new(),
            remote_path: String::new(),
            sync_id: 0,
            sync_type: None,
            sync_type_required: None,
            public_link_code: String::new(),
            public_link_id: 0,
            public_link_path: String::new(),
            public_link_expire: None,
            public_link_password: None,
            public_link_upload_policy: PublicLinkUploadPolicy::Disabled,
            upload_link_comment: String::new(),
            upload_link_expire: None,
            upload_link_maxspace: None,
            upload_link_maxfiles: None,
            tree_link_name: String::new(),
            tree_root_folder_id: None,
            tree_folder_ids_csv: None,
            tree_file_ids_csv: None,
            tree_link_expire: None,
            tree_link_maxdownloads: None,
            tree_link_maxtraffic: None,
            public_link_email: String::new(),
            public_link_receiver_id: 0,
            bookmark_code: String::new(),
            bookmark_location_id: 0,
            bookmark_name: String::new(),
            bookmark_description: String::new(),
            share_folder_id: 0,
            share_name: String::new(),
            share_mail: String::new(),
            share_message: String::new(),
            share_permissions_bits: SharePermissions::READ,
            share_hint: None,
            share_request_id: 0,
            share_id: 0,
            share_to_folder_id: 0,
            share_accept_name: None,
            share_user_ids: Vec::new(),
            share_team_ids: Vec::new(),
            share_user_mods: Vec::new(),
            share_team_mods: Vec::new(),
            share_team_id: 0,
            audit_from_id: None,
            audit_to_id: None,
            mount_path: std::path::PathBuf::new(),
            mount_flag_path: None,
            mount_flag_fuse_opts: None,
            mount_flag_cache_size_gb: None,
            send_publink_mails: String::new(),
            send_publink_message: String::new(),
            remote_folder_path: String::new(),
            folder_metadata_remote_path: String::new(),
            filesystem_status_local_path: String::new(),
            file_history_path: String::new(),
            file_history_limit: None,
            file_diff_rev_a: String::new(),
            file_diff_rev_b: String::new(),
            file_restore_rev: String::new(),
            verify_local_path: String::new(),
            verify_recursive: false,
            verify_fix: false,
            verify_yes: false,
            snapshot_path: std::path::PathBuf::new(),
            snapshot_gpg_recipient: None,
            snapshot_yes: false,
            snapshot_retention_days: None,
            snapshot_zstd_level: None,
            integrity_skip_pattern: String::new(),
            upload_local_path: std::path::PathBuf::new(),
            upload_remote_name: String::new(),
            upload_parent_folder_id: None,
            upload_total_bytes: 0,
            upload_conflict_mode: None,
            upload_session_id: 0,
            conflict_path: String::new(),
            conflict_resolve_policy: String::new(),
            stat_remote_path: String::new(),
            new_crypto_password: SecretString::new(String::new()),
            crypto_change_hint: String::new(),
            crypto_change_code: String::new(),
            crypto_change_flags: 0,
            account_new_password: SecretString::new(String::new()),
            account_verify_token: String::new(),
            account_terms_accepted: false,
            account_language: String::new(),
            sync_suggest_path: None,
            sync_suggest_max: None,
            download_file_id: 0,
            download_local_path: std::path::PathBuf::new(),
            api_server_location_id: 0,
            api_server_binapi: String::new(),
            backup_delete_id: 0,
            backup_create_name: String::new(),
            backup_create_root_folder_id: 0,
            backup_create_local_path: String::new(),
            backup_create_parent_folder_name: None,
            backup_device_folder_id: 0,
            tree_link_paths: Vec::new(),
        }
    }

    #[test]
    fn unmount_without_env_stays_graceful() {
        let _g = force_umount_env_guard();
        // SAFETY: env-var mutation is serialized via `force_umount_env_guard`.
        unsafe { std::env::remove_var("PCLOUD_FORCE_UMOUNT") };
        let mut inputs = fresh_inputs();
        inputs.mount_path = std::path::PathBuf::from("/home/alice/pCloudDrive");
        let req = Command::Unmount.into_request(&inputs);
        assert!(matches!(req, Request::Unmount), "got {req:?}");
    }

    #[test]
    fn unmount_with_force_env_and_path_routes_to_force() {
        let _g = force_umount_env_guard();
        unsafe { std::env::set_var("PCLOUD_FORCE_UMOUNT", "1") };
        let mut inputs = fresh_inputs();
        inputs.mount_path = std::path::PathBuf::from("/home/alice/pCloudDrive");
        let req = Command::Unmount.into_request(&inputs);
        match req {
            Request::MountForceUnmount { path } => {
                assert_eq!(path, std::path::PathBuf::from("/home/alice/pCloudDrive"));
            }
            other => panic!("expected MountForceUnmount, got {other:?}"),
        }
        unsafe { std::env::remove_var("PCLOUD_FORCE_UMOUNT") };
    }

    #[test]
    fn unmount_with_force_env_but_no_path_falls_back_to_graceful() {
        // When `PCLOUD_FORCE_UMOUNT=1` is set but we don't know the
        // path, the CLI cannot safely dispatch the force-umount
        // variant. Fall back to the regular graceful `Unmount` so the
        // daemon can use its own bookkeeping.
        let _g = force_umount_env_guard();
        unsafe { std::env::set_var("PCLOUD_FORCE_UMOUNT", "yes") };
        let inputs = fresh_inputs(); // mount_path empty
        let req = Command::Unmount.into_request(&inputs);
        assert!(matches!(req, Request::Unmount), "got {req:?}");
        unsafe { std::env::remove_var("PCLOUD_FORCE_UMOUNT") };
    }

    #[test]
    fn env_force_umount_truthy_tokens() {
        let _g = force_umount_env_guard();
        for tok in ["1", "true", "yes", "on", "TRUE", "YES", "On"] {
            unsafe { std::env::set_var("PCLOUD_FORCE_UMOUNT", tok) };
            assert!(env_force_umount_enabled(), "{tok} should be truthy");
        }
        for tok in ["", "0", "false", "no", "off", "maybe"] {
            unsafe { std::env::set_var("PCLOUD_FORCE_UMOUNT", tok) };
            assert!(!env_force_umount_enabled(), "{tok} should not be truthy");
        }
        unsafe { std::env::remove_var("PCLOUD_FORCE_UMOUNT") };
    }
}

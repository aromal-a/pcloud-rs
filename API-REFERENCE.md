# API REFERENCE

Reference for the pCloud protocol methods implemented on the Rust path,
cross-referenced against their C counterparts and the parity matrix.

The source of truth for parity is `C_FEATURE_PARITY_MATRIX.csv`; the
current tally lives in `STATUS.md` (single source of truth — do not
duplicate counts here). The source of truth for the C capability
inventory is `pclsync/psynclib.h`.

Rust entry points:

- Protocol clients: `crates/pcloud-proto/src/*`
- Daemon backends:  `crates/pcloud-daemon/src/*_backend.rs`
- SDK surface:      `crates/pcloud-sdk/src/lib.rs`
  (`EmbeddedDaemon::*`)
- IPC methods:      `crates/pcloud-ipc/src/methods.rs`

Legend: **I** Implemented · **P** Partial · **M** Missing · **R**
intentionally Rejected. See the CSV for full notes per row.

## Auth (`pcloud-proto::auth_api`)

| pCloud method | Rust entry | C counterpart (psynclib.h) | Status |
|---------------|-----------|-----------------------------|--------|
| `userinfo` (login) | `EmbeddedDaemon::login(user, pass)` / `dispatch(Request::PasswordSubmission)` | `psync_login` | I |
| `userinfo` (token) | `EmbeddedDaemon::login_with_token(token)` / `dispatch(Request::AuthTokenSubmission)` | `psync_set_auth` | I |
| `userinfo` (authed) | `EmbeddedDaemon::userinfo` | `psync_get_userinfo` | I |
| TFA code | `EmbeddedDaemon::submit_two_factor_code(code, trust, false)` | `psync_tfa_*` | I |
| TFA recovery | `EmbeddedDaemon::submit_recovery_code(code, trust)` / `submit_two_factor_code(code, trust, true)` | `psync_tfa_*` | I |
| TFA SMS resend | `EmbeddedDaemon::send_two_factor_sms` | `psync_tfa_send_sms` | I |
| TFA push resend | `EmbeddedDaemon::send_two_factor_notification` | `psync_tfa_send_notif` | I |
| `verifyemail` | `AccountApi::verify_email` | `psync_verify_email` | I |
| `verifyemail` (restricted) | `AccountApi::verify_email_restricted` | `psync_verify_email_restricted` | I |
| `lostpassword` | `AccountApi::lost_password` | `psync_lost_password` | I |
| `changepassword` | `AccountApi::change_password` | `psync_change_password` | I |
| `logout` | `EmbeddedDaemon::logout` | `psync_unlink` | I |
| `register` | `AccountApi::register` via `EmbeddedDaemon::register` | `psync_register` | I |
| notification cb | — | `psync_set_notification_callback` | R (event stream) |
| `tfa_has_devices` | — | `psync_tfa_has_devices` | R (row 23, flipped Rejected audit-06 ncx.4) — C desktop-UI helper; device list returned in-band by `send_two_factor_notification` |
| `tfa_type` | — | `psync_tfa_type` | R (row 24, flipped Rejected audit-06 ncx.4) — C desktop-UI helper; method dispatch is caller-driven |

## Sync root management (`pcloud-daemon::sync_backend`)

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| add sync | `SyncRuntime::add_root` | `psync_add_sync_by_path` / `_by_folderid` | I |
| list     | `SyncRuntime::list_roots` | `psync_get_sync_list` | I |
| remove   | `SyncRuntime::remove_root` | `psync_delete_sync` | I |
| pause    | IPC `SyncRootPause` | `psync_sync_pause` (per-root) | I |
| resume   | IPC `SyncRootResume` | `psync_sync_resume` | I |
| change type | IPC `SyncRootChangeType` | `psync_change_synctype` | I |
| is-syncable helpers | `sync_backend.rs` / `mount_discovery.rs` | `psync_is_folder_syncable` family | I |
| suggestions | `sync_suggest.rs` | `psync_suggest_folders` | I |
| global pause/resume | — | `psync_pause` / `psync_resume` | R (per-root only) |

## Transfers (`pcloud-proto::transfer_api`, `async_transfer`, `http_download`)

Row 93 (`upload_writefromfile`) is **Implemented** as of audit-06 stream-c
(2026-04-26). `TransferRuntime::upload_write_from_file` drives server-side
copy against the live `BinaryApiTransport`; daemon handler
`upload_write_from_file_ipc` routes `Request::UploadWriteFromFile` to it;
CLI exposes `pcloudc upload write-from-file`. See `STATUS.md`.

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| `getfilelink` | `TransferApi::get_file_link` / `EmbeddedDaemon::get_file_link` | `psync_get_file_link` | I |
| `upload_create` | `TransferApi::upload_create` | `psync_upload_create` | I |
| `upload_write` | `TransferApi::upload_write` | `psync_upload_write` | I |
| `upload_save` | `TransferApi::upload_save` | `psync_upload_save` | I |
| `upload_writefromfile` (server-side copy) | `TransferRuntime::upload_write_from_file` / `pcloudc upload write-from-file` | `upload_writefromfile` | I (row 93 closed, audit-06 stream-c 2026-04-26) |
| signed HTTP download | `http_download::execute` | internal | I |
| SDK `upload_file` / `_as` | `EmbeddedDaemon::upload_file{_as}` | convenience | I |
| SDK `upload_data` / `_as` | `EmbeddedDaemon::upload_data{_as}` | convenience | I |
| SDK `download_file` | `EmbeddedDaemon::download_file` | convenience | I |
| per-download state query | — | `psync_download_state` | R (C body is a `return 0` stub; Rust transfer runtime exposes real per-transfer state via IPC) |

## Public links (`pcloud-proto::public_links_api`)

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| file link create | `create_file_public_link` | `psync_file_public_link` | I |
| folder link create | `create_folder_public_link{_with_options}` | `psync_folder_public_link{_full}` | I |
| tree link create (ids) | `create_tree_public_link` | `psync_tree_public_link` | I |
| tree link create (paths) | `Request::CreateTreePublicLinkFromPaths` / `PublicLinkPathResolver` | path-based shape | I |
| list publinks | `list_public_links` | `psync_list_publinks` | I |
| list uploadlinks | `list_upload_links` | `psync_list_uploadlinks` | I |
| show link | `show_public_link` | `psync_show_publink` | I |
| delete link | `delete_public_link` | `psync_delete_publink` | I |
| changepublink expire/password/limits/upload | `change_public_link::*` | `psync_change_publink` | I |
| upload link create | `create_upload_link` | `psync_upload_link` | I |
| upload link delete | `delete_upload_link` | `psync_delete_uploadlink` | I |
| folder link with options | backend `create_folder_public_link_with_options` only; no IPC/daemon/CLI | `psync_folder_public_link_full` | P (row 147 — reachability gap, gptrev-01 H-01, 2026-04-29) |
| upload access / send-email | backend `create_folder_updownlink` only; no IPC/daemon/CLI | `psync_folder_updownlink` | P (row 148 — reachability gap, gptrev-01 H-01, 2026-04-29) |
| screenshot link | backend `create_screenshot_public_link` only; no IPC/daemon/CLI | `psync_screenshot_publink` | P (row 168 — reachability gap, gptrev-01 H-01, 2026-04-29) |
| bookmarks / pins | helpers in `public_link_backend` | `psync_*_bookmark` | I |
| link cache warmup | — | C-internal helper | R |

## Crypto (`pcloud-crypto`, `pcloud-proto::crypto_api`)

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| setup | `CryptoShell::setup` / `Request::CryptoSetup` | `psync_crypto_setup` | I (gated) |
| start / unlock | `CryptoShell::start` | `psync_crypto_start` | I (gated) |
| stop / lock | `CryptoShell::stop` / `Method::LockCrypto` | `psync_crypto_stop` | I |
| mkdir (encrypted) | `CryptoShell::mkdir` / `Request::CryptoMkdir` | `psync_crypto_mkdir` | I |
| change password (locked) | `Request::CryptoChangePassword` | `psync_crypto_change_password` | I |
| change password (unlocked) | `Request::CryptoChangePasswordUnlocked` | same | I |
| priv key flags | `Request::GetCryptoPrivKeyFlags` | `psync_crypto_priv_key_flags` | I |
| hint | `CryptoShell::get_hint` | `psync_crypto_hint` | I |
| status | `Request::GetCryptoStatus` | `psync_crypto_*` getters | I |
| encrypted file content path | FUSE-integrated (Linux live; macOS/Windows scaffold) | FUSE-integrated | I |
| reset | `CryptoShell::reset` / stop+setup | `psync_crypto_reset` | I |

## Shares / business / teams (`pcloud-proto::shares_api`)

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| list share requests | `SharesApi::list_share_requests` | `psync_list_share_requests` | I |
| list shares | `SharesApi::list_shares` | `psync_list_shares` | I |
| share folder | `SharesApi::share_folder` | `psync_account_sharefolder` | I |
| crypto share folder (RSA path) | `SharesRuntime::crypto_share_folder_rsa` backend only; no IPC route | `psync_crypto_sharefolder` | P (rows 124+138) — RSA backend landed; `Request::CryptoShareFolder` IPC missing; live E2E: `bd-1du.5`/`pcloud-rs-ncx.89-e2e` |
| account team share | `SharesRuntime::account_team_share` | `psync_account_teamshare` | I |
| crypto account team share (RSA path) | `SharesRuntime::crypto_account_team_share_rsa` backend only; no IPC route | `psync_crypto_account_teamshare` | P (row 142) — RSA backend landed; `Request::CryptoAccountTeamShare` IPC missing; live E2E: `bd-1du.5`/`pcloud-rs-ncx.89-e2e` |
| contacts | `SharesRuntime::list_contacts` | `psync_contactlist` | I |
| my teams | `SharesRuntime::list_my_teams` | `psync_list_myteams` | I |
| stop share (multi-id) | `SharesApi::account_stop_share` | `psync_account_stopshare` | I |
| per-share permission modify | `SharesApi::modify_share` | `psync_modify_share` | I |
| accept/decline/cancel share requests | `SharesApi::accept/decline/cancel_share_request` | `psync_accept/decline/cancel_share_request` | I |

## Backup / device (`pcloud-proto::backup_api`)

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| create backup | `BackupApi::create_backup` / SDK `create_backup` | `psync_create_backup` | I (no auto local sync-root) |
| stop device | `BackupApi::stop_device` / SDK `stop_device` | `psync_stopdevice` | I |
| delete backup device | SDK `delete_backup_device` | `psync_delete_backup_device` | I (local-state only) |
| delete backup | `BackupApi::stop_backup` / `BackupRuntime::delete_backup_with_cascade` / SDK `delete_backup` | `psync_delete_backup` | I |
| list devices | — | commented-out in C header | R (ghost) |
| device monitor cb | — | commented-out in C header | R (ghost) |
| update check | — | declared in C header | R (no body linked in this fork) |

## Account / settings / values (`pcloud-proto::account_api`)

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| get API servers | `AccountApi::get_api_servers` | `psync_get_api_servers` | I |
| set API server | `AccountApi::set_api_server` | `psync_set_api_server` | I |
| set language | `AccountApi::set_language` | `psync_set_language` | I |
| get promo | `AccountApi::get_promo` | `psync_get_promo` | I |
| settings get/set (typed) | `{get,set}_{bool,int,uint,string}_setting` | `psync_get/set_*_setting` | I |
| reset setting | `reset_setting` | — | I (stricter) |
| values get/set (typed) | `ValuesRepository::{get,set}_*` | `psync_get/set_*_value` | I (stricter) |
| has value | `ValuesRepository::has(kind)` | approximate in C | I (stricter) |
| has subscription / billing expiry | — | `psync_has_subscription`, etc. | R (account/userinfo path) |

## Filesystem / mounted drive (`pcloud-fs`)

Linux FUSE read+write is live-verified end-to-end. macOS (`fuse-t`) and Windows
(WinFSP) adapters are scaffolded and compile-tested; hardware verification is
pending (`bd-1du.4` cross-platform proof). The parity matrix carries one row
for this subsystem (`fs,mounted pcloud filesystem`) and it is `Implemented`
on the Linux path.

| Operation | Rust entry | C counterpart | Status |
|-----------|-----------|---------------|--------|
| mount (Linux) | `MountService::mount` / `MountService::mount_fuser` | `pfs_*` | I (Linux live; macOS/Windows scaffold) |
| unmount | `MountHandle::Drop` / `MountService::unmount` | `pfs_unmount` | I (Linux live; macOS/Windows scaffold) |
| readdir | `FuseAdapter::readdir` / `lookup` / `getattr` | `pfs_readdir` | I |
| read | `FuseAdapter::read` / `open` / `release` | `pfs_read` | I |
| write / flush / fsync | `WritePathService` via `PcloudFsShim` / `ProtoFuseAdapter` | `pfs_write` etc. | I (chunked multi-GiB pipelining is a perf follow-up, not a parity gap) |
| stat | `FuseAdapter::getattr` / SDK `stat_path` | `pfs_stat` | I |

## IPC method index

`pcloud-ipc::methods::Method` enumerates the IPC-visible operations.
Each method maps to a backend call listed above. See
`crates/pcloud-ipc/src/methods.rs` for the authoritative list
and `crates/pcloud-daemon/src/dispatch.rs` for the routing
table.

## Parity matrix cross-reference

To inspect a given row:

```bash
grep '<c-symbol-or-method>' C_FEATURE_PARITY_MATRIX.csv
```

To count statuses:

```bash
awk -F',' 'NR>1 {gsub(/"/,"",$5); print $5}' \
  C_FEATURE_PARITY_MATRIX.csv | sort | uniq -c
```

Current snapshot: see [`STATUS.md`](./STATUS.md). The one-liner above
regenerates it from the CSV; do not duplicate the counts here.

# Section 1: C-to-Rust Feature Parity
## Date: 2026-04-17
## Auditor scope: auth, transfers, public links, shares/business/teams, crypto, sync-root, backup/device/account, CLI surface

This audit cross-references `C_FEATURE_PARITY_MATRIX.csv` (186 rows, matrix line count
1..187), `C_FEATURE_PARITY_REVIEW.md`, `REJECTED-RATIONALES-14042026.md`,
`STATUS.md`, and every source file cited by the rows against the on-disk
implementation in `crates/*`. The matrix self-reports **157 Implemented / 0
Partial / 0 NotImplemented / 28 Rejected** (186 rows). Raw `awk` over col 5
confirms 157 Implemented + 28 Rejected + one matrix row whose description
contains a comma breaks the naive tally — the review banner `158 / 0 / 0 /
28` in STATUS.md is therefore off-by-one from the CSV contents.

## Findings

### CRITICAL [0]

None. No row in scope was found to be functionally absent while claiming
Implemented. All advertised auth/TFA/account/crypto/share/publink/backup
functions reach a real backend through an IPC handler or SDK helper. The
only functional regression is localised (see HIGH H3 on upload chunk
pipelining).

### HIGH [5]

- H1 — Matrix cites `crates/pcloud-daemon/src/{auth_backend,backup_backend,shares_backend,public_link_backend,account_backend,sync_backend,transfer_backend}.rs` across ≥50 rows, but those files do not exist at those paths. The actual modules live under `crates/pcloud-backends/src/` (verified `ls` on both directories). Any reviewer, static analyzer, or grader that follows the CSV citations will fail every lookup. See *Detailed Findings → Citation drift* for per-row impact.
- H2 — `psync_create_backup` (CSV row 95) is claimed Implemented via `backup_backend.rs` + SDK, but is **not reachable from live callers over IPC**. There is no `Request::CreateBackup` or `Method::CreateBackup` variant in `crates/pcloud-ipc/src/methods.rs` (grep for `CreateBackup` returns only `CreateBackupCascadeError` inside the backend crate) and no matching CLI command (`crates/pcloud-cli/src/commands.rs:552` exposes only `BackupDelete`). Only SDK embedders can call `create_backup`; `pcloudc` operators cannot.
- H3 — `psync_stop_device` / `psync_delete_backup_device` (CSV rows 97, 98) are SDK-only for the same reason: no IPC variant, no CLI command. Row 97 claims parity is "live" but operators can only reach delete_backup (row 96). Row 98 admits "local-only cleanup hook exposed via SDK" which is the correct scope, but it is inconsistent with CLI gap H2.
- H4 — `upload_writefromfile` (server-side copy; CSV row 93) is claimed Implemented at proto level (`crates/pcloud-proto/src/transfer_api.rs:481 encode_upload_write_from_file`) but is **not reachable from any runtime caller**. `grep -n upload_writefromfile` in `crates/pcloud-backends/src/transfer_backend.rs` returns zero matches; `grep -n UploadWriteFromFile` in `crates/pcloud-daemon/src/runtime.rs` returns zero matches. The CSV note says "upload_writefromfile (server-side copy) is wired at the proto level and exposed through ProtoUploadBackend" — ProtoUploadBackend does not expose it. Per audit rule ("Claimed Implemented but unreachable from live caller = HIGH"), this is H.
- H5 — `psync_tfa_send_nofification_res` (CSV row 25, recovery-code/notification resend) cites `crates/pcloud-auth/src/orchestrator.rs` generically but no distinct "resend" function exists beyond `send_two_factor_notification` (orchestrator.rs:568) and `send_two_factor_sms` (:532). There is no `_res` helper and no `Method::ResendTwoFactorNotification`/`ResendTwoFactorSms` variant in the IPC Method enum (`crates/pcloud-ipc/src/methods.rs:73-76` exposes `SendTwoFactorSms` and `SendTwoFactorNotification` only). If the C `_res` semantic is "force a fresh delivery while a challenge is already active", idempotent re-dispatch of the existing sender may satisfy it — but the parity claim is opaque and untested. Downgrade to MEDIUM if the project treats "resend == repeat the send call".

### MEDIUM [4]

- M1 — `psync_crypto_reset` (CSV row 116). Rust `CryptoShell::reset` (pcloud-crypto/src/lib.rs:1005) is local-only and the CSV note says "does not talk to the backend (local scope only, matches the local-state portion of psync_crypto_reset)". The C function also issues a server-side teardown of the user-private key (per upstream `pclsync/psynclib.c:1707`-range). Reducing scope to "local state only" without a linked bead/ADR approving the scope reduction is a partial-without-linked-bead case. Add a rationale link to REJECTED-RATIONALES or an ADR entry; per audit rule, "Partial without a linked bead = HIGH" — but because the CSV marks it Implemented (not Partial) and the scope cut is *documented in the note*, this is MEDIUM pending an ADR pointer.
- M2 — `psync_crypto_share_folder` / `psync_crypto_account_teamshare` (CSV rows 124, 138, 142). Rows claim Implemented but explicitly note "HMAC signature substitutes for RSA until RSA keypair mirroring lands" and "RSA swap tracked under bd-1du.5 keypair work". This is a *cryptographic substitution*, not a parity match. The wire format the server receives is different from what the legacy C client sends. Downstream compatibility with a server that expects RSA-signed blobs may fail silently. The bead `bd-1du.5` is *closed* per STATUS.md (it addresses deletion-safe backup-archive, not the crypto RSA swap). The actual tracker for the RSA keypair work is not linked from these rows.
- M3 — Rejected rows `psync_list_devices` (row 101) and `psync_add_device_monitor_callback` (row 100) have rationales in `REJECTED-RATIONALES-14042026.md` stating the C declarations are commented out. If the enterprise product requires per-device management ("list devices a user has authorized"), this is a retained product feature the C codebase stops short of. Needs explicit product-owner sign-off; current rationale only proves that the legacy C header did not compile them.
- M4 — Matrix self-count drift. `STATUS.md:68` and `STATUS.md:4-47` say `158 / 0 / 0 / 28` (186 rows). `awk -F, 'NR>1{print $5}' C_FEATURE_PARITY_MATRIX.csv | sort | uniq -c` yields `157 Implemented, 28 Rejected, 1 malformed row (comma inside description)`. Either the CSV has 158 Implemented and my tally is fooled by a quoted comma, or STATUS.md is off by one. Reconcile before next audit pass.

### LOW [6]

- L1 — CSV row 11 (`psync_get_status`) cites `runtime.rs:1008` for `crypto_status`; actual definition is at `runtime.rs:2627`. Cosmetic drift, no functional gap.
- L2 — CSV row 11 cites `runtime.rs:1406` for `pending_transfers`; actual definition is at `runtime.rs:3048`. Cosmetic drift.
- L3 — CSV row 71 cites `runtime.rs:868` for `pause_sync`; grep shows `PauseSync` dispatch in runtime.rs handler table near line 415 but no standalone `fn pause_sync` at :868. Cosmetic.
- L4 — CSV row 184 cites `runtime.rs:1100` for authsave path; actual handler `fn set_auth_persistence` is at `runtime.rs:2844`. Cosmetic.
- L5 — `Method::UnlockCrypto` (ipc methods.rs:85) is dispatched but immediately returns `InvalidRequest` at `runtime.rs:435-438` with message "use structured CryptoUnlock / CryptoSetup request variant". This is correct (unlock carries a password that must ride on `Request::CryptoUnlock`, not `Method::Plain`), but keeping the argumentless variant in the public enum is a footgun for CLI authors who may spawn `Method::UnlockCrypto` and get a puzzling error. Add `#[deprecated]` or remove.
- L6 — `Method::SetAuthPersistence` has the same dead-argumentless-variant pattern at `runtime.rs:439-442`. Same fix.

## Detailed Findings

### Auth parity (CSV rows 14-35, IPC methods.rs, auth_backend.rs)

All 22 auth rows wire through to concrete backends:

- `login_with_password` / `login_with_token` — `crates/pcloud-backends/src/auth_backend.rs:306-325` (`AuthRuntime::login_with_password`, `login_with_token`). Dispatched by `Request::PasswordSubmission` / `Request::AuthTokenSubmission` in methods.rs:272-286 via `Method::SubmitPassword` (methods.rs:79) and runtime.rs handler table near line 415.
- TFA code / recovery code submission — `AuthRuntime::submit_two_factor_code` (auth_backend.rs:331). IPC: `Request::TwoFactorCodeSubmission` (methods.rs:287-296) flagged with `recovery_code: bool`. CLI tfa alias: `crates/pcloud-cli/src/app.rs:182`.
- TFA SMS resend — `AuthRuntime::send_two_factor_sms` (auth_backend.rs:372). IPC: `Method::SendTwoFactorSms` (methods.rs:74).
- TFA notification resend — `AuthRuntime::send_two_factor_notification` (auth_backend.rs:383). IPC: `Method::SendTwoFactorNotification` (methods.rs:76).
- `userinfo` — `AuthRuntime::userinfo` (auth_backend.rs:361), refreshable via `refresh_token` (:407). IPC: `Method::GetUserInfo` (methods.rs:60).
- `verify_email` — `Method::VerifyEmail` (methods.rs:215) → `runtime.rs:488 → fn verify_email runtime.rs:2094` → `AccountRuntime::verify_email` (account_backend.rs:291) → SDK `EmbeddedDaemon::verify_email` (sdk/lib.rs:1709) → CLI `Command::AccountVerifyEmail` (commands.rs:514). Complete chain.
- `verify_email_restricted` — `Request::VerifyEmailRestricted` (methods.rs:955) → runtime `verify_email_restricted` (runtime.rs:2145) → backend (account_backend.rs:302) → SDK (sdk/lib.rs:1732) → CLI (commands.rs:518). Complete.
- `lost_password` — `Request::LostPassword` (methods.rs:949) → runtime (2124) → backend (313) → SDK (1749) → CLI (521). Complete.
- `change_password` — `Request::AccountChangePassword` (methods.rs:962) → runtime (2166) → backend (321) → SDK (1764) → CLI (525). Complete.
- `register` — `Request::AccountRegister` (methods.rs:972) with `terms_accepted: bool` gate → runtime (2229) → backend (340) → SDK (1809) → CLI (528). Complete.
- `get_promo` — `Method::GetPromo` (methods.rs:207) → runtime (2031) → backend (268) → SDK (1656) → CLI (541). Complete.
- `get_api_servers` — `Method::GetApiServers` (methods.rs:202) → runtime (2003) → backend (258) → SDK (1630) → CLI (531). Complete.
- `set_language` — `Request::SetLanguage` (methods.rs:1019) → runtime → backend (279) → SDK (1686) → CLI (538). Complete.
- `set_api_server` — `Request::SetApiServer` (methods.rs:1011) → runtime `set_api_server_ipc` (runtime.rs:2396) with data-residency policy gate → SDK (1843) → CLI (535). Complete. Residency enforcement at runtime.rs:270 and :3311.

Auth parity verdict: functional parity is end-to-end for every advertised
primitive. The only gap is H5 (the `_res` resend variant has no distinct
resend symbol and may conflate with the send variant).

### Transfers parity (CSV rows 87-94, transfer_api.rs, transfer_backend.rs, sdk/lib.rs)

- `getfilelink` — `pcloud-proto/src/transfer_api.rs:200 TransferApi::get_file_link` → `pcloud-backends/src/transfer_backend.rs:306 TransferRuntime::get_file_link`. Surfaced via IPC `Request::GetFileLink` (methods.rs:985) → runtime.rs:800 → CLI `Command::DownloadLink` (commands.rs:545). Complete.
- `signed-HTTP download execution` — `TransferRuntime::download_bytes` called from `runtime.rs:2334` inside `download_file_ipc`. Executes `HttpDownloadConfig` signed GET against the `DownloadLink` returned by `getfilelink`. Complete.
- `upload_create / upload_write / upload_save` — `TransferApi::upload_create` (transfer_api.rs:249), chunked state machine in `pcloud-backends/src/upload_state.rs:UploadStateMachine`, driven by `TransferRuntime::upload_bytes_chunked` (transfer_backend.rs:474). IPC `Request::UploadCreate` wired per runtime.rs handler table. SQLite resume persistence via `UploadResumeRepository`, retry with backoff via `RetryPolicy`, auth refresh on 2000 class.
- `upload_data`, `upload_data_as`, `upload_file`, `upload_file_as` — all four SDK helpers at `sdk/lib.rs:1413, 1473, 1454, 1496`. Reachable from embedders. **Not exposed over IPC or CLI** — the CLI has `Command::UploadCreate/Pause/Resume/Cancel/List` which drive the operator-session registry, not single-shot uploads. Not called out as a gap in the matrix, but note for operators: there is no `pcloudc upload file <local> <remote>` single-shot command; users must either go through the operator-session lifecycle or a FUSE mount.
- `upload_writefromfile` — see H4 above.
- Chunked upload / resumability / idempotency on retry — `UploadStateMachine` in `upload_state.rs` persists resume offsets in SQLite, retries with configurable backoff, refreshes auth tokens on 2000-class errors. CSV row 92 notes "Sequential write (one request-response per chunk) is used rather than C PSYNC_MAX_PENDING_UPLOAD_REQS=16 pipelining" — this is honest about reduced throughput; it is *not* a correctness regression.

Transfers verdict: functionally complete for single-chunk and multi-chunk
uploads. H4 (upload_writefromfile unreachable) is the only parity gap in
scope.

### Public links parity (CSV rows 145-168, public_links_api.rs, public_link_backend.rs)

- File public link create — `PublicLinkRuntime::create_file_public_link` (public_link_backend.rs:700).
- Folder public link create — `:716 create_folder_public_link`.
- Folder public link with options (expire/maxdownloads/maxtraffic/linkpassword) — `:995 create_folder_public_link_with_options`.
- Folder updownlink — `:1042 create_folder_updownlink` (publink/createfolderlinkandsend).
- Tree public link — `:835 create_tree_public_link` and `:1064 create_tree_public_link_from_paths` (backed by `PublicLinkPathResolver`).
- List / show / delete — `:672 list_public_links`, `crypto_show_link` handled via direct API, `:732 delete_public_link`.
- Change expire / password / upload policy — `:745 change_public_link_expire`, `:759 change_public_link_password`, `:773 change_public_link_upload`.
- Upload-link CRUD — `:798 create_upload_link`, `:787 list_upload_links`, `:821 delete_upload_link`.
- Upload-access — `:list_email_with_access`, `link_add_access`, `link_remove_access` (grep confirms presence in the file).
- Bookmarks — `:914 remove_bookmark`, `:928 change_bookmark`.
- Screenshot — `:1018 create_screenshot_public_link`.
- Send publink (row 42) — `:957 send_publink`, exposed via IPC `Request::SendPublink`, SDK, and CLI `publink send`.

Rejected: bookmark cache warmup (row 169), link cache warmup (row 167),
`delete_all_links_folder`/`_file` (rows 151, 152) all have rationales in
REJECTED-RATIONALES-14042026.md (cache warmup / internal cache scanner
not mirrored). Correct scope reduction.

Public links verdict: full parity; no gaps.

### Shares / business / teams parity (CSV rows 130-144, shares_api.rs, shares_backend.rs)

- `share_folder` — `SharesRuntime::share_folder` (shares_backend.rs:307).
- `list_shares` / `list_share_requests` — `:294, :281`.
- `cancel_share_request` — `:332`, `decline_share_request` — `:345`, `accept_share_request` — `:358`.
- `remove_share` — `:377`, `modify_share` — `:389`, `account_modify_share` — `:417`.
- `account_teamshare` — `:432`, `account_stopshare` — wired (grep confirms `account_stopshare` is whitelisted as a dev command name at shares_backend.rs:96).
- `crypto_share_folder` — `:461` (rejects locked crypto pre-wire).
- `crypto_account_teamshare` — `:490`.
- `list_contacts` — `:528`, `list_my_teams` — `:540`.

Caveat M2 applies to crypto variants (HMAC-vs-RSA signature substitution).

### Crypto parity (CSV rows 107-129, pcloud-crypto/src/lib.rs)

- `setup` — `CryptoShell::setup` (lib.rs:659). Argon2 derivation, HMAC fingerprint only.
- `start` — `:713`. Constant-time fingerprint verification.
- `stop` — `:753`. Zeroizes key material.
- `unlock` — `:786`, `lock` — `:803`. Same key-material lifecycle.
- `reset` — `:1005`. Local only; see M1 for scope-reduction concern.
- `mkdir` — HMAC-SHA256 deterministic filename + local bookkeeping; requires unlocked state.
- `is_setup` — `:590`, `is_started` — `:601`, `get_hint` — `:613`.
- `any_folder_id`, `folder_ids`, `has_crypto_folders` — all present.
- `change_password` — `:914`, `change_password_unlocked` — `:837`. Argon2 salt + fingerprint rotation, HMAC-SHA256 version-tagged blob.
- `priv_key_flags` — `:815`. Default 0, TEMP_PASS=1.
- `send_change_user_private` — `pcloud-proto/src/crypto_api.rs` + `CryptoRuntime` (crypto_backend.rs) + `Method::SendCryptoChangeUserPrivate`.
- Content sector encryption — AES-256-GCM per sector, 12-byte random nonce, 16-byte tag, sector-index in AAD; per-file keys derived via HMAC-SHA256 from master + random file seed. `crates/pcloud-crypto/src/content.rs`.
- Metadata filename encryption — deterministic HMAC-SHA256 keyed by master key. `crates/pcloud-crypto/src/metadata.rs`.

Crypto parity verdict: complete, modulo M1 (reset scope) and M2 (HMAC
signature substitute pending RSA keypair work). The persist_master_key
flag rejection (matrix row 107) is a *security upgrade*, not a parity
gap — correctly called out.

### Sync-root management parity (CSV rows 65-75, sync_backend.rs)

- `start_sync` — `ReconcileWorker` in `pcloud-engine/src/reconcile_worker.rs`.
- `change_synctype` / `delete_sync` / `pause_resume_root` — all three dispatched from the runtime handler table: runtime.rs:598-606 (`Request::SyncRootAdd/Remove/Pause/Resume/ChangeType`).
- `get_sync_suggestions` — `pcloud-backends/src/sync_suggest.rs` (166 extension entries ported).
- `is_folder_syncable` — `sync_backend.rs:232` (`classify_folder_syncability_with_lists`).
- `pause` / `stop` / `resume` daemon-wide — runtime.rs handler table for `Method::PauseSync`/`ResumeSync`/`Shutdown`.
- `run_localscan` — `EngineShell::wake_localscan` in `pcloud-engine/src/lib.rs:91`; daemon `Request::RunLocalScan`.
- Diff polling (row 75) — `DiffWorker` in `sync_backend.rs` (full diff loop, SQLite cursor persistence, retry backoff, event dispatcher).

Sync-root verdict: complete. No functional gaps.

### Backup / device / account utility parity (CSV rows 95-98, backup_api.rs, backup_backend.rs, account_backend.rs)

- `create_backup` — `BackupRuntime::create_backup` (backup_backend.rs:457) and `create_backup_with_cascade` (:512) — the latter registers the local sync root. **See H2.**
- `stop_backup` / `delete_backup` — `:476` and `:553` (`delete_backup_with_cascade`). IPC: `Request::DeleteBackup` (methods.rs:1002). Complete.
- `stop_device` — `:488` and `:579` (`stop_device_with_cascade`). **See H3.**
- `delete_backup_device` — SDK-only (`sdk/lib.rs delete_backup_device`). Per CSV row 98, correctly scoped to "local cleanup hook". OK.
- Account utilities (`userinfo`, `verify_email`, `lost_password`, `change_password`, `register`, `get_promo`, `get_api_servers`, `set_language`, `set_api_server`) — all fully end-to-end, see Auth parity section.

Backup verdict: SDK surface is complete; IPC/CLI surface is incomplete
(H2/H3).

### CLI coverage (commands.rs, app.rs)

`Command` enum in `crates/pcloud-cli/src/commands.rs:35` exposes **111+
variants** across auth/crypto/shares/publink/account/upload/download/
backup/sync. Spot-check against C `control_tools.cpp`:

- `status`, `pending`, `sync list/add/remove/pause/resume`, `crypto start/stop`, `tfa`, `auth`, `authsave`, `finalize`, `quit`, `help` — all wired (CSV rows 172-186). Legacy aliases (`?`, `st`, `p`, `f`, `q`, single-token `s`, `c`) advertised in help string (`crates/pcloud-cli/src/app.rs:86-242`).
- `publink send` (row 42), `publink create-upload`, `publink change-link`, `publink show`, `publink list`, `publink list-upload`, `publink delete`, `publink screenshot`, `publink tree` — grep of `app.rs:281-294` confirms.
- `download link`, `download file` — app.rs:681-683.
- `upload create/pause/resume/cancel/list` — app.rs:832-843.
- `account verify-email/verify-email-restricted/lost-password/change-password/register/api-servers/set-api-server/set-language/promo` — commands.rs:514-541. Two-token aliases at app.rs:654-666.
- `backup delete` — app.rs:739-746. **No `backup create`, `backup stop-device`, `backup list`** — see H2, H3.
- `crypto setup/mkdir/reset/priv-key-flags/send-change-private/change-password[-unlocked]/hint` — commands.rs:479-502.
- `sync suggest`, `sync is-syncable` — commands.rs:504-509.

CLI verdict: broad coverage; the only gaps are H2 (missing `backup create`)
and H3 (missing `backup stop-device`).

## Concrete Remediation Steps

- H1 (citation drift, 50+ rows): replace `crates/pcloud-daemon/src/` with `crates/pcloud-backends/src/` across the CSV. Add a regression test in the parity harness that `fs::metadata` exists for every non-empty `rust_reference` file path. Ship a one-line `sed` patch to the CSV plus a grep-based CI gate.
- H2 (no `CreateBackup` IPC variant): add `Request::CreateBackup { name: String, device_root: Option<String>, parent_folder_name: Option<String> }` in `crates/pcloud-ipc/src/methods.rs`, wire a runtime handler that calls `BackupRuntime::create_backup_with_cascade`, and expose `Command::BackupCreate` in `crates/pcloud-cli/src/commands.rs`. Mirror the CLI flag surface of `backup delete`. Tests: add `account_backend_tests.rs`-style coverage with the dev transport.
- H3 (no `StopDevice` IPC variant): add `Request::StopDevice { folder_id: Option<u64> }`, `Command::BackupStopDevice`, and an equivalent runtime handler that reuses `BackupRuntime::stop_device_with_cascade`. Provide a `pcloudc backup stop-device [<folder-id>]` CLI subcommand.
- H4 (`upload_writefromfile` unreachable): in `crates/pcloud-backends/src/transfer_backend.rs`, add `TransferRuntime::upload_write_from_file(&self, auth_token, upload_id, file_ids)` that wraps `transfer_api.rs:481 encode_upload_write_from_file`. Expose it through `ProtoUploadBackend` so the upload state machine can take the server-side-copy shortcut when a duplicate hash is detected. Add a dev-transport test.
- H5 (`_res` resend semantics): either (a) document that `SendTwoFactorSms` / `SendTwoFactorNotification` are idempotent resend-capable and flip the CSV note accordingly, or (b) add `Method::ResendTwoFactor{Sms,Notification}` variants that route to the same orchestrator functions but with a distinct audit event type ("auth.tfa.resend" vs "auth.tfa.send").
- M1 (crypto reset scope): file an ADR stating "psync_crypto_reset is deliberately local-only; server-side teardown of the user private key is covered by the password-rotation family". Link the ADR from CSV row 116.
- M2 (HMAC-vs-RSA signature substitute): either (a) add the RSA keypair port to `pcloud-crypto::share_temppass` and flip rows 124/138/142 back to "Implemented-with-RSA", or (b) flag those rows Partial and link a live bead tracker. Currently they are Implemented with a hidden crypto substitute — enterprise-breaking if the pCloud server expects RSA.
- M3 (device list/monitor rejections): add a one-paragraph product decision to REJECTED-RATIONALES-14042026.md confirming the C ghost status is also the right scope for the Rust rewrite.
- M4 (count drift): reconcile STATUS.md counter with a `jq`/`awk`-based CI check. The count drift is small (1 row) but it is the kind of drift that masks H/M regressions.
- L1-L6 (cosmetic): search-and-replace the line numbers in the CSV, and mark `Method::UnlockCrypto` / `Method::SetAuthPersistence` `#[deprecated]` with a rust-doc pointer to the `Request` variants.

## Verdict

**Section 1 is MOSTLY GREEN.** Auth, shares, public-links, crypto (modulo
RSA note), sync-root, and account utilities are fully wired end-to-end.
Transfers are functionally complete except for the unreachable
`upload_writefromfile` optimisation (H4). Backup has an IPC/CLI hole for
`CreateBackup` / `StopDevice` (H2, H3) that blocks operator use cases.
Matrix file-path citations are pervasively wrong (H1) and need a global
fix before the next audit wave.

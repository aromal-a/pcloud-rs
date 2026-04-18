# Section 1: C-to-Rust Feature Parity
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 1)

Read-only audit of the parity truth artifacts against actual crate implementations. All citations are `file:line` against the tree at
`/home/ezechiel203/Projects/FORKS/pcloud-rs`.

---

## Findings

### CRITICAL [0]

No findings that would make the daemon unsafe, silently lose data, or
invalidate a security claim. Every Implemented row I spot-checked is
reachable from at least one live caller (IPC, SDK, or CLI).

### HIGH [3]

- **H1** Matrix row 93 claims `upload_writefromfile` is Implemented, but
  the symbol is only wired at the proto encode layer. It is not called
  from any daemon/backend/SDK/CLI path. This is the exact class the
  rubric marks HIGH ("claimed Implemented but not reachable from a
  live IPC/CLI/SDK caller").
- **H2** Backup lifecycle asymmetry. `Request::DeleteBackup` is an IPC
  variant reachable via `pcloudc backup delete <id>`, but
  `create_backup`, `stop_device`, and `delete_backup_device` exist
  *only* as SDK in-process helpers — there is no IPC variant, no daemon
  dispatch, and no CLI command. Matrix rows 95, 97, 98 are marked
  Implemented, which is technically true (the SDK surface is wired end
  to end), but the claim is reachability-asymmetric and should be
  flagged explicitly in the row notes.
- **H3** Tree-link path-resolver variant is proto/backend only. Matrix
  row 149 says `create_tree_public_link_from_paths` is Implemented; the
  CLI only wires the numeric id form (`Self::CreateTreeLink =>
  Request::CreateTreePublicLink` at commands.rs:834). Path-based
  callers must use the SDK directly.

### MEDIUM [2]

- **M1** `STATUS.md:68` and subsequent banners claim **158 / 0 / 0 / 28**
  (Implemented / Partial / Missing / Rejected, total 186). The matrix
  body agrees — however, my automated counter (`awk -F,` on
  `C_FEATURE_PARITY_MATRIX.csv`) shows 158 Implemented and 28 Rejected
  *after* accounting for one quoted-comma parse artefact in row 93
  (`"pclsync/pupload.c:694,881,1281,843; …"` — the embedded commas
  push the status cell to column 5 of a downstream field and
  naive parsing yields a spurious `"1281"` status). The counts are
  self-consistent but the CSV is brittle to any tool that does not
  honour RFC 4180 quoting. Consider renormalising the offending
  cells to avoid future false-positive mismatches.
- **M2** `REJECTED-RATIONALES-14042026.md:5` enumerates exactly 28
  rejected rows matching the CSV, and all 28 are present as `###
  Row <n>` sections. No unjustified Rejected rows detected. Keeping
  MEDIUM as a reminder only — no change needed now.

### LOW [4]

Citation drift across the matrix. The notes cite stale `file:line`
anchors throughout. Concrete examples:

- **L1** Row 11 (`psync_get_status`) cites
  `crates/pcloud-daemon/src/runtime.rs:1008` (`crypto_status`). The
  actual location is `runtime.rs:2630` (`fn crypto_status(&self) ->
  Response` at line 2630).
- **L2** Row 11 + 174 also cite `runtime.rs:1406` (`pending_transfers`).
  Actual location is `runtime.rs:3051` (`fn pending_transfers` at line
  3051).
- **L3** Rows 71 and 73 cite `runtime.rs:868` / `runtime.rs:877` for
  `pause_sync` / `resume_sync`. Actual locations are `runtime.rs:2455`
  and `runtime.rs:2467`.
- **L4** Rows 77, 78, 84, 86 cite
  `crates/pcloud-daemon/src/path_resolver.rs` but this file does not
  exist on the Rust path. The actual file is
  `crates/pcloud-backends/src/path_resolver.rs`. The same crate-name
  drift applies to every `*_backend.rs` citation in the matrix: the
  matrix says `crates/pcloud-daemon/src/{auth,crypto,transfer,shares,
  backup,public_link,account,notifications,folder}_backend.rs`, but
  every one of those files lives in `crates/pcloud-backends/src/` per
  `find` on the tree. This is the single largest source of citation
  drift and affects ~30+ rows.

---

## Detailed Findings

### 1. Auth (rows 14–35)

Every row spot-checked is wired end-to-end.

- `psync_tfa_send_sms` / `_notification` / `_set_code` are reachable
  via `Method::SendTwoFactorSms`
  (`crates/pcloud-cli/src/commands.rs:902`),
  `Method::SendTwoFactorNotification` (commands.rs:905), and
  `Request::TwoFactorCodeSubmission` (ipc methods.rs:288).
- `psync_set_user_pass` lands on `Method::SubmitPassword`
  (`crates/pcloud-cli/src/commands.rs:177` + daemon dispatch in
  `runtime.rs`).
- Account helpers `verify_email`, `lost_password`, `change_password`,
  `register`, `get_promo`, `get_api_servers`, `set_language`,
  `set_api_server` all surface both SDK and IPC (ipc methods.rs:947-1020).
- Password scorer ports are implemented under
  `crates/pcloud-crypto/src/password_scorer.rs` (rows 33–35).

No parity gap detected.

### 2. Transfers (rows 87–94)

- SDK upload helpers live at `crates/pcloud-sdk/src/lib.rs:1413`
  (`upload_data`), 1454 (`upload_file`), 1473 (`upload_data_as`),
  1496 (`upload_file_as`).
- `getfilelink` is wired: `Request::GetFileLink`
  (`crates/pcloud-ipc/src/methods.rs:983`), CLI `DownloadLink`
  (commands.rs:545), SDK `get_file_link`.
- `upload_create`/`upload_write`/`upload_save` are wired through
  `UploadStateMachine` and `UploadSession` end to end (proto, backends,
  SDK). IPC surface for operator control exists as
  `Request::UploadCreate`/`UploadPause`/`UploadResume`/`UploadCancel`/
  `UploadList` (methods.rs:882-920), with CLI counterparts
  (commands.rs:457-470).
- **`upload_writefromfile` is not reachable** — the encoder exists
  only at `crates/pcloud-proto/src/transfer_api.rs:481`
  (`encode_upload_write_from_file`), plus the wire method definition
  at `crates/pcloud-proto/src/methods/upload.rs:260`. No caller in
  `pcloud-backends`, `pcloud-daemon`, `pcloud-sdk`, or `pcloud-cli`
  invokes it. Matrix row 93 implies this is functional; it is dead
  code at the protocol layer only. **→ H1.**
- Chunked resumability: `UploadStateMachine` at
  `crates/pcloud-backends/src/upload_state.rs` is the correct
  implementation focus. Persistence to the `UploadResumeRepository`
  matches the description in the row 92 notes. No gap observed at the
  functional level; sequential (non-pipelined) write is noted as a
  performance follow-up.

### 3. Public links (rows 145–168)

- File/folder/tree creation + expire/password/upload policy mutation +
  access add/remove + upload-link family + bookmarks all wired via
  `Request::CreateFilePublicLink` (methods.rs:451) through
  `Request::DeleteUploadLink` (methods.rs:496), with CLI counterparts
  (commands.rs:79-122).
- **Tree-link path variant (row 149) is not CLI-reachable** — only
  the id form is wired. `create_tree_public_link_from_paths` is
  referenced only in the proto test helpers and the public-link
  backend. **→ H3.**
- `send_publink` (row 42 but conceptually a publink surface) is fully
  wired: proto → backend → daemon → IPC → SDK → CLI. Confirmed at
  `crates/pcloud-ipc/src/methods.rs:739` (`Request::SendPublink`),
  `runtime.rs:729` (dispatch), `commands.rs:1048` (CLI into_request).

### 4. Shares / business / teams (rows 130–145)

All wired. Specific spot-checks:

- `Request::ShareFolder` (methods.rs:558), `CancelShareRequest`,
  `AcceptShareRequest`, `RemoveShare`, `ModifyShare`,
  `AccountStopShare` (methods.rs:604), `AccountModifyShare`,
  `AccountTeamShare` (methods.rs:618) are all present and CLI-reachable
  (commands.rs:246-270).
- `psync_crypto_share_folder` uses HMAC-SHA256 signatures as a
  substitute for RSA (row 138 notes), and this is documented honestly
  — flagged as "RSA swap tracked under bd-1du.5". Acceptable.
- No unjustified `Partial`; rows are consistent.

### 5. Crypto (rows 107–129)

- Setup / start / stop / reset / mkdir / hint all wired through
  `CryptoShell` (`crates/pcloud-crypto/src/lib.rs`) and IPC
  (`CryptoSetup`, `CryptoUnlock`, `CryptoMkdir`,
  `LockCrypto`, `CryptoReset`). CLI surface
  (`SubmitCryptoPassword` commands.rs:193, `LockCrypto` commands.rs:200,
  `CryptoReset` commands.rs:482, `CryptoPrivKeyFlags` 486,
  `CryptoSendChangePrivate` 490, `CryptoChangePassword` 495,
  `CryptoChangePasswordUnlocked` 499, `CryptoHint` 502) is complete
  **except** `CryptoMkdir` is not exposed as a CLI command (it is only
  reachable via SDK/IPC).
- `change_crypto_pass`, `send_change_user_private`, `priv_key_flags`
  (rows 119–122) all verified as live — IPC variants
  `Request::CryptoChangePassword` (methods.rs:333),
  `CryptoChangePasswordUnlocked` (methods.rs:351),
  `Method::SendCryptoChangeUserPrivate` (methods.rs:99),
  `Method::GetCryptoPrivKeyFlags` (methods.rs:95), dispatched in
  `runtime.rs`, exposed in the SDK at `EmbeddedDaemon::
  crypto_send_change_user_private` / `crypto_priv_key_flags` /
  `crypto_change_password` / `crypto_change_password_unlocked`.
  CLAUDE.md's "partially implemented — full wire-through and live
  verification pending" statement is **out of date** — the IPC/SDK/CLI
  wiring is all present. CLAUDE.md should be tightened to match
  STATUS.md.

### 6. Sync-root management

- `SyncRootAdd` / `SyncRootRemove` / `SyncRootPause` / `SyncRootResume`
  / `SyncRootChangeType` are all wired (methods.rs:376-411).
- CLI has `SyncList`, `SyncAdd`, `SyncRemove`, `SyncStatus`,
  `SyncChangeType`, `SyncSuggest`, `SyncIsSyncable` (commands.rs:124-509).
- Suggestions (`GetSyncSuggestions`, methods.rs:416) and syncability
  (`IsFolderSyncable`, methods.rs:424) are both CLI-reachable via
  `SyncSuggest`/`SyncIsSyncable` at commands.rs:506/509. Row 69/70 claims
  verified.

### 7. Backup / device / account (rows 95–104)

- **`CreateBackup` is not wired as an IPC variant or CLI command.** It
  exists only as an SDK method at `crates/pcloud-sdk/src/lib.rs:2070`
  and is implemented at `crates/pcloud-backends/src/backup_backend.rs:457`.
- Same for `stop_device` (`sdk/lib.rs:2141`,
  `backup_backend.rs:488`) and `delete_backup_device`
  (`sdk/lib.rs:2177`, no proto/daemon dispatch — SDK-local only).
- `DeleteBackup` is IPC-reachable (methods.rs:1000, `runtime.rs:807`
  dispatch, CLI `BackupDelete` at `commands.rs:552`).
- Matrix row 95-98 statuses are technically correct (SDK surface is a
  legitimate reachable caller), but **row 95/97/98 notes do not make
  the "SDK-only, not IPC-reachable" scope explicit.** **→ H2.**

Account helpers (rows 29–42 family, also row 38-41 for language /
promo / api servers / set-api-server) are fully wired.

### 8. CLI coverage (rows 172–186)

- CLI `Command` enum catalogued at
  `crates/pcloud-cli/src/commands.rs:41-552`. Every Method / Request
  variant I sampled had a corresponding `Command` or
  `Request::`-producing arm in `into_request` (commands.rs:1025-1250).
- Missing explicit CLI surfaces (acceptable but worth noting):
  `CryptoMkdir`, `ValueGet/Set/Has` (settings), `CreateBackup`,
  `StopDevice`, `DeleteBackupDevice`, `AccountRegister` (SDK only for
  the latter per auth H1 secret-handling).
- All `Method::*` variants in methods.rs:37-216 appear to have at
  least one IPC call site — verified by searching for `method:
  Method::` in `commands.rs`.

### 9. SDK breadth (row 187)

- The SDK surface at `crates/pcloud-sdk/src/lib.rs` (line 2033-2177
  spans the backup helpers; 1413-1496 spans upload helpers; etc.) is
  broad and covers everything the matrix claims.
- Doc coverage: every public fn I spot-checked has a `///` comment
  with an example; this matches the row 187 claim.

### 10. Matrix self-consistency

- CSV has 186 data rows + 1 header row = 187 lines (`wc -l` confirms
  187). Claimed split 158 / 0 / 0 / 28 == 186, consistent.
- No `Partial` rows in the CSV. No `Missing` rows. All non-Implemented
  rows are `Rejected` with a note.
- 28 `Rejected` rows in CSV; 28 `### Row N —` sections in
  `REJECTED-RATIONALES-14042026.md`. Rationales present for all.
- **Citation drift is systemic.** The wrong-crate citation for
  `*_backend.rs` files (daemon vs backends) affects every row that
  lists a backend path, which is ~30+ rows. See L4.

### 11. Stale CLAUDE.md vs STATUS.md

CLAUDE.md (project instructions in this session's context) still says
for crypto: "Partially implemented — see crates/pcloud-daemon/src/runtime.rs and crates/pcloud-backends/src/crypto_backend.rs. Full wire-through and live verification pending as part of bd-1du.10: change_crypto_pass family, send_change_user_private, priv_key_flags."

The matrix rows 119-122 all show Implemented, with live IPC + SDK +
CLI wiring. This is a documentation drift between CLAUDE.md and the
authoritative matrix — it is not a parity gap but the project lead
should reconcile the two documents. (This is not a rubric-class
finding, noted for the `bd-1du.10` closure audit.)

---

## Summary

The implementation quality of the Rust tree is materially higher than
the parity artifacts advertise. The three HIGH findings all boil down
to a single class of problem: matrix rows claim "Implemented" for
surfaces that are only partially plumbed through the daemon — proto
layer only (H1), SDK-only without IPC/CLI (H2), or backend-only
without CLI (H3). These should be either:

1. split into sub-rows so the IPC/SDK/CLI dimensions are auditable
   independently, or
2. downgraded to `Partial` with a linked bead (per the
   `bd-1du.10` gate criteria).

Citation drift (LOW) is purely a doc-hygiene concern but is pervasive;
a one-shot pass to rebase all `runtime.rs:<line>` and
`pcloud-daemon/src/*_backend.rs` citations to current truth would
eliminate the class.

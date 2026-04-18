## Section 1. C-to-Rust Feature Parity & API Coverage

Auditor: Dimension 1 specialist (parity matrix + API coverage only).
Scope: `/home/ezechiel203/Projects/FORKS/pcloud-rs` as of 2026-04-17.
Read-only audit. No source modified.

This section assesses whether the parity-matrix (`C_FEATURE_PARITY_MATRIX.csv`) and narrative (`C_FEATURE_PARITY_REVIEW.md`) truthfully reflect the Rust code that currently ships, whether all `Implemented` rows have live callers, whether every IPC variant has a CLI or SDK surface, and whether the CLAUDE.md handoff and STATUS.md tallies are internally consistent. Nine specific findings are raised; a parity-matrix gap table for Appendix C is at the end.

---

### 1.1 Executive summary

- The retained surface genuinely exists. I was able to locate live Rust implementations for **every** `Implemented` row I spot-checked (auth, transfers, public links, shares, crypto lifecycle, backup, account utilities, notifications, settings, folder metadata). The matrix is **honest about breadth**.
- `CLAUDE.md` is **lagging code reality** in four places. It lists `change_crypto_pass`, `send_change_user_private`, `priv_key_flags`, and `psync_send_publink` as "still missing" while the matrix (and the code) record them as Implemented (rows 119–122 and 42). The drift is a documentation discipline failure flagged in `CLAUDE.md` itself ("Documentation Discipline" section).
- The matrix's `rust_reference` column contains **pervasive citation drift** (stale paths, wrong line numbers). Many `*_backend.rs` files moved from `crates/pcloud-daemon/src/…` to `crates/pcloud-backends/src/…` but the matrix still cites the daemon location. Auth orchestrator line numbers are off by hundreds. This does not flip any status verdict but actively undermines the matrix as an audit artifact.
- There is **one CRITICAL orphan IPC variant** (`Request::VerifyPath`). It is constructed by the CLI (`commands.rs:1102`) but has **zero handler** in the daemon. Also two CLI commands (`FileDiff`, `FileRestore`) route deliberately to `Method::GetHealth` as "defensive fallbacks" — they are documented stubs, not parity rows, but they look like working commands to users.
- Two Implemented rows (117 `psync_crypto_folderid`, 118 `psync_crypto_folderids`) exist on `CryptoShell` but are **not exposed to any external caller** — no IPC, CLI, or SDK surface exposes `any_folder_id()` / `folder_ids()`. Only `CryptoStatus` returns the *count*, not the ids. These rows are unreachable-from-live-callers while marked Implemented.
- `Rejected` rows (28) are all backed by rationale in `REJECTED-RATIONALES-14042026.md`. I verified the cross-reference: every matrix `Rejected` row listed in the doc header ("rows 2, 5, 6, 10, 12, 13, 43, 44, 45, 46, 99, 100, 101, 102, 103, 104, 105, 106, 113, 114, 115, 126, 151, 152, 157, 160, 167, 169") is a real matrix row with `Rejected` status.
- The SDK surface is wide (≈80 public functions across `EmbeddedDaemon`) but only **one example** (`crates/pcloud-sdk/examples/login_and_list.rs`, 73 lines) and no module-level user guide. Docs are dense at the function level but thin at the walkthrough level. The only cross-crate `pub use` leak (`pcloud_proto::Notification` at `lib.rs:105`) is intentional and documented.
- Zero `Partial` rows, zero `Missing` rows — the matrix is in the "defend or flip" rather than "still triaging" state. The final parity gate is `bd-1du.10` (Prove and gate final C parity claims).

---

### 1.2 Matrix spot-check — 30 rows across all subsystems

All rows below were verified by locating the claimed function/struct in the Rust source. Citations are file:line in the current tree.

#### Auth / account (matrix rows 14–42)

| Row | Symbol | Matrix status | Code reality | Verdict |
|-----|--------|---------------|--------------|---------|
| 15 | `psync_set_user_pass` | Implemented @ auth_api.rs:115 | `pub fn login_with_password` at `crates/pcloud-proto/src/auth_api.rs:115`; orchestrator at `crates/pcloud-auth/src/orchestrator.rs:219` | OK |
| 17 | `psync_set_auth` | Implemented @ orchestrator.rs:39 | `pub fn login_with_token` at `crates/pcloud-auth/src/orchestrator.rs:188` — matrix line **39** is stale; real line **188** | Status OK, citation drifted |
| 20 | `psync_tfa_send_sms` | Implemented @ orchestrator.rs:248 | `pub fn send_two_factor_sms` at `crates/pcloud-auth/src/orchestrator.rs:532` — matrix line **248** is stale; real line **532** | Status OK, citation drifted |
| 21 | `psync_tfa_set_code` | Implemented @ orchestrator.rs:119 | `pub fn submit_two_factor_code` at `crates/pcloud-auth/src/orchestrator.rs:324` — matrix line **119** is stale (that is a doc comment); real line **324** | Status OK, citation drifted |
| 22 | `psync_tfa_send_nofification` | Implemented @ orchestrator.rs:262 | `pub fn send_two_factor_notification` at `crates/pcloud-auth/src/orchestrator.rs:568` — matrix line **262** is stale | Status OK, citation drifted |
| 28 | `psync_register` | Implemented | `pub fn register` at `crates/pcloud-sdk/src/lib.rs:1809` (validates email, password non-empty, terms_accepted=true; hands off to `AccountRuntime::register`); `Request::AccountRegister` wired at `crates/pcloud-daemon/src/runtime.rs` | OK |
| 33 | `psync_derive_password_from_passphrase` | Implemented @ password_scorer.rs:471 | Present — `crates/pcloud-crypto/src/password_scorer.rs` has the PBKDF2 path | OK |
| 42 | `psync_send_publink` | Implemented | `pub fn send_publink` at `crates/pcloud-proto/src/public_links_api.rs:875`, `crates/pcloud-backends/src/public_link_backend.rs:956`, SDK at `crates/pcloud-sdk/src/lib.rs:2311`, CLI via `Command::SendPublink` at `crates/pcloud-cli/src/commands.rs:1048`, daemon dispatch at `crates/pcloud-daemon/src/runtime.rs:726`. **Fully wired end-to-end** | OK — contradicts CLAUDE.md "still missing" claim |

#### Transfers (matrix rows 87–94)

| Row | Symbol | Matrix status | Code reality | Verdict |
|-----|--------|---------------|--------------|---------|
| 87–90 | `psync_upload_data{,_as}`, `psync_upload_file{,_as}` | Implemented @ sdk/lib.rs | `pub fn upload_data` at `crates/pcloud-sdk/src/lib.rs:1413`; `upload_file` @:1454; `upload_data_as` @:1473; `upload_file_as` @:1496 | OK |
| 91 | `getfilelink` | Implemented @ transfer_api.rs:62 | `pub fn get_file_link` at `crates/pcloud-backends/src/transfer_backend.rs:305`; SDK @ `crates/pcloud-sdk/src/lib.rs:2903` | OK |
| 92 | `upload_create/write/save` | Implemented | `UploadStateMachine` in `crates/pcloud-backends/src/upload_state.rs`, `upload_bytes_chunked` in `crates/pcloud-backends/src/transfer_backend.rs`. Cited path in matrix (`crates/pcloud-daemon/src/transfer_backend.rs`) is **stale** — that file does not exist; real file is under `pcloud-backends` | Status OK, citation path **wrong** |
| 93 | upload wire methods family | Implemented @ methods/upload.rs | Exists | OK |
| 94 | SDK `UploadSession` | Implemented @ upload_session.rs | `crates/pcloud-sdk/src/upload_session.rs` exists; see `pub use upload_session::{…}` at `lib.rs:97` | OK |

#### Public links (matrix rows 145–168)

| Row | Symbol | Matrix status | Code reality | Verdict |
|-----|--------|---------------|--------------|---------|
| 145–148 | create file/folder/folder_full/updownlink public links | Implemented @ `crates/pcloud-daemon/src/public_link_backend.rs` | **File does not exist at that path**. Real file is `crates/pcloud-backends/src/public_link_backend.rs`. The `PublicLinkRuntime` struct is there (line 624), and `fn send_publink` at line 956 | Status OK, citation path **wrong** for ~20 rows in this block |
| 158 | `psync_show_link` | Implemented @ public_link_backend | Daemon dispatches `Request::ShowPublicLink` at `crates/pcloud-daemon/src/runtime.rs:613`; backend in `pcloud-backends` | OK |
| 161–163 | `psync_change_link_expire/password/enable_upload` | Implemented @ public_link_backend | `Request::ChangePublicLinkExpire/Password/Upload` dispatched at `runtime.rs:619–626`; daemon forwards to `public_link_runtime` | OK |
| 168 | `psync_screenshot_public_link` | Implemented | Matrix note describes a composite `getfilepublink` + `changepublink` with now+delay (30d default). Implementation exists in `pcloud-backends/src/public_link_backend.rs`. | OK |

#### Shares / business / teams (matrix rows 130–144)

| Row | Symbol | Matrix status | Code reality | Verdict |
|-----|--------|---------------|--------------|---------|
| 138 | `psync_crypto_share_folder` | Implemented | `pub fn crypto_share_folder` at `crates/pcloud-backends/src/shares_backend.rs:460` + `crates/pcloud-proto/src/shares_api.rs:392`; temppass in `crates/pcloud-crypto/src/share_temppass.rs` | OK |
| 141 | `psync_account_teamshare` | Implemented | `pub fn account_team_share` at `shares_backend.rs:431` and `shares_api.rs:350` | OK |
| 142 | `psync_crypto_account_teamshare` | Implemented | `pub fn crypto_account_team_share` at `shares_backend.rs:489` and `shares_api.rs:432` | OK |
| 143/144 | `psync_list_contacts`, `psync_list_myteams` | Implemented | `crates/pcloud-backends/src/shares_backend.rs` — `contactlist` wire filtered by `type!=3`/`type==3` | OK |

#### Crypto (matrix rows 107–129)

| Row | Symbol | Matrix status | Code reality | Verdict |
|-----|--------|---------------|--------------|---------|
| 107 | `psync_crypto_setup` | Implemented | `CryptoShell::setup` in `crates/pcloud-crypto/src/lib.rs`; `Request::CryptoSetup` dispatched in `runtime.rs` | OK |
| 117 | `psync_crypto_folderid` | Implemented | `CryptoShell::any_folder_id` at `crates/pcloud-crypto/src/lib.rs:1022`. **No IPC/CLI/SDK caller** reaches this — only test-local callers at `lib.rs:1336` and `tests/integration.rs:134`. See Finding HIGH-2 below. | Status wrong — should be `Partial` or dedicated note |
| 118 | `psync_crypto_folderids` | Implemented | `CryptoShell::folder_ids` at `crates/pcloud-crypto/src/lib.rs:1032`. Same problem as 117 — no external caller. | Same |
| 119 | `psync_crypto_crypto_send_change_user_private` | Implemented | `crates/pcloud-proto/src/crypto_api.rs`, `crates/pcloud-proto/src/methods/crypto.rs`, SDK helper at `crypto_send_change_user_private`, dispatch via `Method::SendCryptoChangeUserPrivate` (runtime.rs:447) | OK (contradicts CLAUDE.md "still missing") |
| 120 | `psync_crypto_change_crypto_pass` | Implemented | `CryptoShell::change_password` at `crates/pcloud-crypto/src/lib.rs:914`. Dispatch at `runtime.rs:571`; SDK at `crypto_change_password` in `crates/pcloud-sdk/src/lib.rs:1940`; integration test suite in `crates/pcloud-daemon/tests/crypto_change_password.rs` (10+ cases). | OK (contradicts CLAUDE.md) |
| 121 | `psync_crypto_change_crypto_pass_unlocked` | Implemented | `CryptoShell::change_password_unlocked` at `crates/pcloud-crypto/src/lib.rs:837`. Dispatch at `runtime.rs:584`; SDK at `crypto_change_password_unlocked` in `crates/pcloud-sdk/src/lib.rs:1992`. | OK |
| 122 | `psync_crypto_priv_key_flags` | Implemented | `CryptoShell::priv_key_flags` at `crates/pcloud-crypto/src/lib.rs:815`; `KeyManager.private_flags` at `crates/pcloud-crypto/src/keys.rs:72`; dispatch `Method::GetCryptoPrivKeyFlags` → `runtime.rs:446` → `crypto_priv_key_flags` at `runtime.rs:2658`; SDK at `crypto_priv_key_flags` in `crates/pcloud-sdk/src/lib.rs:1860`. | OK (contradicts CLAUDE.md) |

#### Sync (matrix rows 65–86)

| Row | Symbol | Matrix status | Code reality | Verdict |
|-----|--------|---------------|--------------|---------|
| 65 | `psync_start_sync` | Implemented @ pcloud-engine/src/reconcile_worker.rs | File exists (not re-read here — matrix note is specific). | OK |
| 74 | `psync_run_localscan` | Implemented @ pcloud-engine/src/lib.rs:91 + runtime.rs:475 | Matrix concedes "actual scan loop is still pending Agent A (bd-1du.3/4)"; only the wake signal is retained. **This is honest annotation — a caveated Implemented**. Rust also emits `sync.localscan.wake` audit | Honest; note matches `bd-1du.4` scope |
| 75 | diff polling | Implemented @ sync_backend.rs | `DiffWorker` exists in `crates/pcloud-backends/src/sync_backend.rs` (not daemon) | OK, citation **wrong path** (pcloud-backends not pcloud-daemon) |
| 76 | `psync_stat_path` | Implemented | `StatPath` request at `runtime.rs`, `StatPathPayload` in ipc `methods.rs:1174` | OK |
| 85 | mounted pcloud filesystem | Implemented | FUSE shim + `PcloudFsShim`; still gated to `PCLOUD_FUSE_TEST=1` for kernel proofs. Matrix annotation explicitly calls this out. `bd-1du.4` is still open per STATUS.md — **the "Implemented" verdict here is aggressive given bd-1du.4 is still open**. The annotation concedes "chunked upload pipelining is a performance follow-up, not a parity gap", which is defensible; the kernel-level mount proof is gated on an env-var, which is a **test-coverage caveat** worth flagging for `bd-1du.10` review | Status defensible but aggressive; see Finding MEDIUM-3 |

#### Backup (matrix rows 95–106)

| Row | Symbol | Matrix status | Code reality | Verdict |
|-----|--------|---------------|--------------|---------|
| 95 | `psync_create_backup` | Implemented | `BackupRuntime::create_backup` at `crates/pcloud-backends/src/backup_backend.rs:456`; `create_backup_with_cascade` at :511. Cited path in matrix (`crates/pcloud-daemon/src/backup_backend.rs:*`) is **wrong**; file is in `pcloud-backends` | Status OK, citation path wrong |
| 96 | `psync_delete_backup` | Implemented | `stop_backup` at :475, `delete_backup_with_cascade` at :552 | OK, citation path wrong |
| 97 | `psync_stop_device` | Implemented | `stop_device` at :487, `stop_device_with_cascade` at :578 | OK, citation path wrong |
| 98 | `psync_delete_backup_device` | Implemented | SDK-only at `crates/pcloud-sdk/src/lib.rs:2177` (`delete_backup_device`). **No IPC or CLI surface**, so not reachable from the daemon/CLI user path; only embedded SDK consumers get this. | Status defensible for SDK-only exposure; see Finding MEDIUM-4 |

---

### 1.3 Findings

#### CRITICAL-1 — `Request::VerifyPath` is an orphan IPC variant

**What.** The IPC crate declares `Request::VerifyPath { path, recursive }` at `crates/pcloud-ipc/src/methods.rs:805`. The CLI constructs this request at `crates/pcloud-cli/src/commands.rs:1102`:

```rust
Self::Verify { .. } => Request::VerifyPath {
    path: inputs.verify_local_path.clone(),
    recursive: inputs.verify_recursive,
},
```

**Problem.** There is **no handler** for this request variant anywhere under `crates/pcloud-daemon/`. A grep across the daemon tree returns zero matches. The runtime's top-level dispatcher at `crates/pcloud-daemon/src/runtime.rs:710`+ does not reference `VerifyPath`. Any client that actually sends this frame will receive the default "unsupported ipc request (newer client than daemon?)" response — the same error path reserved for unknown variants.

**Why it matters.** This is a **user-visible silent regression**: the `pcloudc verify <path>` CLI command currently runs a CLI-side walker (`crates/pcloud-cli/src/verify.rs`) over a mock `ServerHashResolver`. The production path that would route through the daemon is not wired. The IPC variant exists *only* as a protocol placeholder; there is no parity matrix row tracking its missing daemon handler. Either:

1. the CLI should not construct `Request::VerifyPath` at all (it should do everything CLI-side explicitly), or
2. the daemon should implement the handler.

The current "dispatch a variant the daemon will refuse" is the worst of both. **Recommend raising a new `bd-1du.*` child bead** or folding this into `bd-1du.10` proof.

**References.**
- Declaration: `crates/pcloud-ipc/src/methods.rs:805` (with 10 lines of docstring that promise full walk-tree behaviour).
- Construction: `crates/pcloud-cli/src/commands.rs:1102`.
- Absence of handler: `grep VerifyPath crates/pcloud-daemon/src/**` returns zero matches.

#### HIGH-1 — CLAUDE.md contradicts the matrix on four symbols

**What.** `CLAUDE.md` at "## What Has Been Done → ### Crypto parity progress → Still missing:" enumerates:

- `change_crypto_pass` family — **actually Implemented** at matrix rows 120, 121 (code at `crates/pcloud-crypto/src/lib.rs:837, 914`).
- `send_change_user_private` — **actually Implemented** at matrix row 119 (code: SDK helper `crypto_send_change_user_private`, daemon dispatch `Method::SendCryptoChangeUserPrivate`, dedicated test file `crates/pcloud-daemon/tests/crypto_change_password.rs`).
- `priv_key_flags` — **actually Implemented** at matrix row 122 (code at `crates/pcloud-crypto/src/lib.rs:815`, `KeyManager.private_flags` at `crates/pcloud-crypto/src/keys.rs:72`, SDK helper `crypto_priv_key_flags` at `crates/pcloud-sdk/src/lib.rs:1860`).

And at "### Backup / device / account utility progress → Still partial because:":

- "`psync_send_publink` remains missing" — **actually Implemented** at matrix row 42 (fully wired: `crates/pcloud-proto/src/public_links_api.rs:875`, `crates/pcloud-backends/src/public_link_backend.rs:956`, CLI `Command::SendPublink`, SDK `send_publink`, end-to-end tests enumerated in the matrix annotation).

**Why it matters.** CLAUDE.md is the declared primary handoff document ("current handoff and execution dossier"). If a follow-on agent trusts it, they will duplicate already-done work and mis-scope `bd-1du.10`. This directly violates the "Documentation Discipline" section of CLAUDE.md itself.

**Recommendation.** Strike the obsolete "Still missing" and "Still partial" bullets. Either delete the sub-sections entirely or rewrite them to reflect the matrix. Ideally this file should also link `STATUS.md` for numeric tallies (as STATUS.md itself asks) and stop itemising the "still missing" list.

#### HIGH-2 — Two `Implemented` rows have no live callers (crypto folder ids)

**What.** Matrix rows **117** (`psync_crypto_folderid`) and **118** (`psync_crypto_folderids`) are marked `Implemented` with a bare citation to `crates/pcloud-crypto/src/lib.rs`. The functions `CryptoShell::any_folder_id` (line 1022) and `CryptoShell::folder_ids` (line 1032) are real, public, and compile. However:

- no `Request::*` or `Method::*` variant carries them to the IPC surface,
- no CLI command maps to them (`grep any_folder_id|folder_ids\(` across `crates/pcloud-cli` returns zero),
- no SDK helper exposes them (`grep` across `crates/pcloud-sdk/src/lib.rs` returns zero),
- only `CryptoStatus` reports a folder *count* (`runtime.rs:2635`) and does not enumerate ids.

Consumers are: one doctest at `lib.rs:1019`, one inner unit test at `lib.rs:1336`, and one integration test at `crates/pcloud-crypto/tests/integration.rs:134`.

**Why it matters.** The C surfaces `psync_crypto_folderid()` / `psync_crypto_folderids()` return a folder id / the list of encrypted folder ids to *external callers* (tools like file browsers and the pfs mount layer). The Rust analogues are internally reachable only; no live caller receives the enumeration. Marking this as `Implemented` is a narrow-sense truth (the function exists) that hides the user-facing gap.

**Recommendation.** Either (a) flip both rows to `Partial` with a pointer to `bd-1du.10` and a TODO to add an IPC surface (`Method::GetCryptoFolderIds` → `Response::message` as JSON array), or (b) explicitly add an SDK helper `fn crypto_folder_ids(&self) -> Vec<u64>` and keep them `Implemented`. Current state violates the spirit of the "line-by-line audit" rule in CLAUDE.md ("confirm Rust is stricter than C where appropriate… confirm no cleartext secret persistence is reintroduced"), because one would expect an enumerate surface to actually be enumerable.

#### HIGH-3 — Two CLI commands are deliberate no-ops masquerading as features

**What.** `crates/pcloud-cli/src/commands.rs:1092` contains:

```rust
// `FileDiff` / `FileRestore` are CLI-side stubs; they never
// reach the daemon. Mapping to `GetHealth` is a defensive
// fallback equivalent to `Doctor`.
Self::FileDiff | Self::FileRestore => Request::Plain {
    method: Method::GetHealth,
},
```

Both `Command::FileDiff` (commands.rs:370) and `Command::FileRestore` (:374) are public, parseable CLI commands. A user running `pcloudc file diff <path> <rev_a> <rev_b>` or `pcloudc file restore <path> <rev>` will see the daemon return the `GetHealth` payload ("ok"), not a "not implemented" error.

**Why it matters.** This is a **silent UX lie**. The parity matrix does not track these — there is no matrix row for `file diff` or `file restore` C ops because the C client does not have them (they are new Rust-only proposals). That said, they should not compile as working CLI subcommands if they are stubs. Users get an unambiguous liveness probe back from what they think is a restore.

**Recommendation.** Either (a) remove the `Command::FileDiff` / `Command::FileRestore` variants and the parser arms that produce them, or (b) have `into_request` return a typed "unimplemented" response so the CLI prints a clear error. Current comments admit the problem ("CLI-side stubs; they never reach the daemon") but do not expose that to the user.

#### HIGH-4 — `Method::FileHistory` Plain dispatch is dead

**What.** The IPC crate declares `Method::FileHistory` at `crates/pcloud-ipc/src/methods.rs:138` with extensive docstring. However the daemon runtime at `crates/pcloud-daemon/src/runtime.rs:379`+ never matches `Method::FileHistory` in its `Request::Plain { method }` arm. The only handler is for the parameter-bearing `Request::FileHistory { path, limit }` at `runtime.rs:736`. So:

- `Request::Plain { method: Method::FileHistory }` → falls through to the `_ => { status: InvalidRequest, message: "unsupported ipc method (newer client than daemon?)" }` arm at `runtime.rs:498`.

**Why it matters.** `Method::FileHistory` is effectively **unused**. It is a leftover from a refactor where the argumentless and argument-bearing shapes both existed. The matrix does not say `Method::FileHistory` is Implemented (matrix row 76 is `psync_stat_path`, row for file history would be separate), so this isn't a matrix gap — but it is a live-caller-reachability gap on the public IPC surface.

**Recommendation.** Remove `Method::FileHistory` from the `Method` enum (it is `#[non_exhaustive]` so this is safe under the evolvability contract), or add a handler that returns `"use structured FileHistory request variant"` like the `StatPath` arm at `runtime.rs:481`.

#### MEDIUM-1 — Pervasive citation drift in the parity matrix

**What.** The matrix's `rust_reference` column has two systematic drift patterns:

1. **Stale path prefix.** Multiple rows cite `crates/pcloud-daemon/src/<X>_backend.rs` where the actual file is `crates/pcloud-backends/src/<X>_backend.rs`. Examples:
   - Row 70 cites `crates/pcloud-daemon/src/sync_backend.rs` — real file is `crates/pcloud-backends/src/sync_backend.rs`.
   - Row 81 cites `crates/pcloud-daemon/src/folder_backend.rs` — real file is `crates/pcloud-backends/src/folder_backend.rs`.
   - Rows 95–98 (backup family) cite `crates/pcloud-daemon/src/backup_backend.rs` — real file is `crates/pcloud-backends/src/backup_backend.rs`.
   - Rows 131–144 (shares family) cite `crates/pcloud-daemon/src/shares_backend.rs` — real file is `crates/pcloud-backends/src/shares_backend.rs`.
   - Rows 145–167 (public links family) cite `crates/pcloud-daemon/src/public_link_backend.rs` — real file is `crates/pcloud-backends/src/public_link_backend.rs`.

2. **Wrong line numbers.** Auth orchestrator rows (17, 20, 21, 22) cite lines 39, 248, 119, 262 when the real public-function definitions are at lines 188, 532, 324, 568. The matrix lines look like stale pre-refactor pointers.

A daemon-sources listing (`ls crates/pcloud-daemon/src/`) confirms only the following `*_backend.rs` file is under `pcloud-daemon`: **none**. All backends have been relocated to `pcloud-backends`. The matrix has not been updated.

**Why it matters.** The matrix is the parity audit's source of truth. Citation drift means every reviewer has to `grep` to validate a row. It also means the CLAUDE.md "Line-by-Line Capability Audit Rule" ("cite exact C and Rust files") is partially broken in its own reference CSV. This **must be fixed before `bd-1du.10` can close** in good faith.

**Recommendation.** A one-pass mechanical update. For each `rust_reference` cell, run `grep -rn "<symbol name>" crates/` and rewrite the cell. The matrix annotations (the `notes` column) are still largely accurate and should be preserved. Expect this to touch roughly 60–80 rows.

#### MEDIUM-2 — Row 85 (FUSE mount) is marked Implemented while `bd-1du.4` is still open

**What.** Matrix row 85 (`fs,mounted pcloud filesystem`) is `Implemented` with a thorough annotation. STATUS.md still lists `bd-1du.4 — Replace filesystem shell with real mounted-drive parity` as open. The annotation concedes:

- "Chunked `upload_write` pipelining for sustained multi-GiB writes is a performance follow-up, not a parity gap."
- "Gated integration: `PCLOUD_FUSE_TEST=1` exercises kernel create+write+fsync+unlink+rename+unmount …"

**Why it matters.** The `bd-1du.4` bead description says "Replace filesystem shell with real mounted-drive parity … integration tests for mounted-drive behavior." STATUS.md says the bead is still open. Having the matrix row already flipped to `Implemented` muddies the gate. Either the row is honest (a happy-path mount + kernel round-trip test is enough for parity, and `bd-1du.4` is about hardening/perf), or it is aggressive (a real-world mount hardening is part of parity, and the row should still be `Partial`).

**Recommendation.** `bd-1du.10` should explicitly document the scope split between row 85 (Implemented) and `bd-1du.4` (still open). STATUS.md's statement that `bd-1du.4` is still open and the matrix's `Implemented` row are both defensible, but the inconsistency invites confusion. A one-line clarification in the matrix note ("live-mount integration gated by PCLOUD_FUSE_TEST; hardening/perf tracked under bd-1du.4") would suffice.

#### MEDIUM-3 — `psync_delete_backup_device` SDK-only exposure not reflected in matrix note

**What.** Matrix row 98 cites only `crates/pcloud-sdk/src/lib.rs` and the annotation says "local-only cleanup hook exposed via SDK `delete_backup_device`". The function lives at `crates/pcloud-sdk/src/lib.rs:2177`. There is no `Request::DeleteBackupDevice` variant, no `Method::*` probe, and no `Command::BackupDeleteDevice` CLI command. A pCloud user running `pcloudc` cannot invoke this cleanup; only an embedded-SDK consumer can.

**Why it matters.** The matrix annotation is partially honest ("SDK"); but it does not mention that this is **not** available on the daemon/CLI control path. A reviewer looking at the row would assume the usual CLI wiring. Given backup/device lifecycle is a user-facing concern, SDK-only exposure is a parity narrowing, not a full parity match.

**Recommendation.** Extend the matrix note: "SDK-only; intentionally not exposed via IPC/CLI since the operation is purely local-side cleanup. C users reach this indirectly via psynclib; Rust users must use the embedded SDK." Or add a CLI shim for operator UX.

#### MEDIUM-4 — SDK breadth has one example and no consumer guide

**What.** `crates/pcloud-sdk/src/lib.rs` is 4437 lines with roughly 80 `pub fn` on `EmbeddedDaemon`. The only example is `crates/pcloud-sdk/examples/login_and_list.rs` (73 lines). There is no README, no module-level walkthrough beyond the top-of-file `//!` conventions section, and no `docs/sdk/` guide.

**Why it matters.** SDK breadth is claimed in CLAUDE.md "What Has Been Done → Rust foundation" ("embeddable SDK surface") and matrix row 187 (`sdk,embedded library shell`, Implemented). But the lack of examples/guides means external integrators would struggle to exercise the surface. This is **not** a parity gap against the C client (which has no analogous SDK), but it *is* a documentation gap against the Rust claim of "enterprise ready" (which STATUS.md correctly tells us not to claim yet).

**Recommendation.** Not blocking for `bd-1du.10`, but should be tracked as a post-parity polish bead. At minimum, split the current monolithic `lib.rs` into themed submodules (auth, transfers, public_links, crypto, backup, settings) and add one focused example per theme.

#### LOW-1 — `Rejected` rationale document has tight 1:1 coverage

**What.** `REJECTED-RATIONALES-14042026.md` lists 28 Rejected rows explicitly: rows 2, 5, 6, 10, 12, 13, 43, 44, 45, 46, 99, 100, 101, 102, 103, 104, 105, 106, 113, 114, 115, 126, 151, 152, 157, 160, 167, 169. I cross-checked each row in the matrix: every one is a real matrix row with status `Rejected` and the cited C path matches. Categories (Ghost, Stub, Replaced, Billing-out-of-scope, C-internal-plumbing, Typo-duplicate) are coherent. No rationale row refers to a row status that is not actually `Rejected`. **This is solid** and should not be touched.

**Good practice.** The document's "How to read" preamble (especially the "Rejected ≠ broken" paragraph and the "counts live in STATUS.md, STATUS.md wins" line) is exemplary project-discipline writing.

#### LOW-2 — `Method` enum has redundant variants (`SessionStatus` in both Method and Request)

**What.** `Method::SessionStatus` (methods.rs:125) and `Request::SessionStatus` (methods.rs:668) both exist. The daemon handles both: `runtime.rs:458` matches `Method::SessionStatus`, `runtime.rs:724` matches `Request::SessionStatus`. Both route to the same `self.session_status()` handler. This is by design (both wire shapes accept the probe) but it is redundant — the `Request::Plain { method: Method::SessionStatus }` form and the bare `Request::SessionStatus` form carry the same payload.

**Why it matters.** Not a bug. But `Method` is documented as "exhaustive catalog of **argumentless** IPC operations". `SessionStatus` is argumentless, fine. Having it also be a bare `Request::` variant is legacy. A future cleanup could remove one.

**Recommendation.** Track as a minor IPC cleanup; not `bd-1du.10` blocking.

---

### 1.4 IPC → CLI coverage matrix

Summary of mapping between `pcloud-ipc::Request` variants (80 variants, `#[non_exhaustive]`) and CLI surfaces.

- Every variant in the IPC `Request` enum has a handler in `crates/pcloud-daemon/src/runtime.rs::handle_request` **except** `Request::VerifyPath` (see CRITICAL-1 above). I confirmed this by listing the 80 `Request::X` variants and grepping `Request::X` against runtime.rs + dispatch.rs.
- Every `Method` variant has a handler in the `Plain { method: … }` arm of `handle_request` **except** `Method::FileHistory` (see HIGH-4 above) and `Method::StatPath` / `Method::SubmitPassword` / `Method::SubmitTwoFactorCode`, all three of which are *intentional* rejects with explanatory messages ("use structured … request variant"). Not orphan, not bugs.
- Every CLI `Command` variant has an `into_request` arm — verified via exhaustive match in `crates/pcloud-cli/src/commands.rs`.
- Two CLI commands (`Command::FileDiff`, `Command::FileRestore`) route to `GetHealth` as a deliberate no-op (see HIGH-3 above).

Unmapped IPC surfaces (exist on wire, no CLI subcommand at all):
- None found — every Request variant has either a direct CLI command or is reachable via one (e.g. `MountForceUnmount` via `Command::MountForceUnmount` in commands.rs:294).

Internal-only IPC surfaces:
- `Request::AuditVerifyChain` — has `Command::AuditVerify` so reachable.
- `Request::AuthPersistence` — has `Command::AuthSave` so reachable.

---

### 1.5 SDK coverage assessment

- Approximately **80 public methods** on `EmbeddedDaemon` (`crates/pcloud-sdk/src/lib.rs`) covering auth, TFA, transfers, uploads (chunked session + direct helpers), folder metadata, account utilities, backup, crypto rotation, notifications, settings (typed KV and `setting` table), and filesystem helpers (stat, list_folder, mount, unmount).
- **1 pub use leak** (`pub use pcloud_proto::Notification` at line 105), explicitly documented as intentional to avoid forcing downstream consumers to depend on `pcloud-proto`. Defensible.
- **1 example** (`examples/login_and_list.rs`, 73 lines). Covers login + list-folders. No examples for: crypto setup/unlock/rotate, upload session with pause/resume, share creation, public-link lifecycle, mount lifecycle, notification stream, settings round-trip.
- Docs: per-method docstrings are dense and reference specific C surfaces. The module-level conventions section at `lib.rs:10–64` is high quality. But no `docs/sdk/GUIDE.md` or similar walkthrough.
- **No leaked internal types** detected beyond the documented `Notification` re-export. Constructors for `EmbeddedDaemon` go through `EmbeddedDaemonBuilder` which enforces validation.

---

### 1.6 Rejected-row rationale coverage

`REJECTED-RATIONALES-14042026.md` cross-references 28 rows. I confirmed each:

| Row | Symbol | Category | Present in CSV as Rejected? |
|-----|--------|----------|------------------------------|
| 2 | psync_set_alloc | Ghost + C-internal | Yes |
| 5 | psync_set_notification_callback | Replaced | Yes |
| 6 | psync_init_data_event_handler | Replaced + C-internal | Yes |
| 10 | psync_download_state | Stub | Yes |
| 12 | psync_get_last_error | Replaced + Insecure-legacy | Yes |
| 13 | psync_network_exception | Replaced | Yes |
| 43 | psync_ptools_create_backend_event | C-internal | Yes |
| 44 | psync_register_account_events_callback | Replaced | Yes |
| 45 | psync_register_backup_events_callback | Replaced + Ghost | Yes |
| 46 | psync_async_ui_callback | C-internal (UI bridge) | Yes |
| 99 | psync_send_backup_del_event | C-internal (UI event) | Yes |
| 100 | psync_add_device_monitor_callback | Ghost (commented out in C) | Yes |
| 101 | psync_list_devices | Ghost (commented out in C) | Yes |
| 102–106 | psync_check_new_version* family | Ghost (header-only in C) | Yes |
| 113–115 | psync_crypto_{hassubscription,isexpired,expires} | Billing-out-of-scope | Yes |
| 126 | psync_update_cryptostatus | C-internal refresh | Yes |
| 151/152 | psync_delete_all_links_{folder,file} | C-internal (cache) | Yes |
| 157 | psync_sow_link | Typo-duplicate + Ghost | Yes |
| 160 | psync_psync_change_link | Typo-duplicate | Yes |
| 167 | psync_cache_links_all | C-internal (cache warmup) | Yes |
| 169 | psync_cache_bookmarks | C-internal (cache warmup) | Yes |

**All 28 cross-references resolve.** No orphan rejections, no missing rationales.

---

### 1.7 Parity Matrix Gap Table (Appendix C input)

Proposed matrix changes flowing from this audit. Format: `row | current_status | proposed_status | reason`.

| Row | Symbol | Current | Proposed | Reason |
|-----|--------|---------|----------|--------|
| 17 | psync_set_auth | Implemented @ orchestrator.rs:39 | Implemented @ `crates/pcloud-auth/src/orchestrator.rs:188` | Citation drift fix |
| 20 | psync_tfa_send_sms | Implemented @ orchestrator.rs:248 | Implemented @ `crates/pcloud-auth/src/orchestrator.rs:532` | Citation drift fix |
| 21 | psync_tfa_set_code | Implemented @ orchestrator.rs:119 | Implemented @ `crates/pcloud-auth/src/orchestrator.rs:324` | Citation drift fix |
| 22 | psync_tfa_send_nofification | Implemented @ orchestrator.rs:262 | Implemented @ `crates/pcloud-auth/src/orchestrator.rs:568` | Citation drift fix |
| 65 | psync_start_sync | Implemented @ pcloud-engine/src/reconcile_worker.rs | No change (path is correct; engine is where it belongs) | Keep |
| 69 | psync_get_sync_suggestions | Implemented @ pcloud-daemon/src/sync_suggest.rs | Implemented @ `crates/pcloud-backends/src/sync_suggest.rs` | Citation drift fix |
| 70 | psync_is_folder_syncable | Implemented @ pcloud-daemon/src/sync_backend.rs | Implemented @ `crates/pcloud-backends/src/sync_backend.rs` | Citation drift fix |
| 75 | diff polling | Implemented @ pcloud-daemon/src/sync_backend.rs | Implemented @ `crates/pcloud-backends/src/sync_backend.rs` | Citation drift fix |
| 81 | psync_check_and_create_folder | Implemented @ pcloud-daemon/src/folder_backend.rs:239 | Implemented @ `crates/pcloud-backends/src/folder_backend.rs:…` | Citation drift fix |
| 82/83 | create_remote_folder + _by_path | Implemented @ pcloud-daemon/src/folder_backend.rs:… | Implemented @ `crates/pcloud-backends/src/folder_backend.rs:…` | Citation drift fix |
| 85 | mounted pcloud filesystem | Implemented | Implemented *with explicit scope note*: "live-mount kernel round-trip gated behind `PCLOUD_FUSE_TEST=1`; sustained-perf pipelining tracked under bd-1du.4" | Scope note clarification |
| 92 | upload_create/write/save | Implemented @ `crates/pcloud-daemon/src/transfer_backend.rs`… | Implemented @ `crates/pcloud-backends/src/transfer_backend.rs`; `crates/pcloud-backends/src/upload_state.rs` | Citation drift fix |
| 95–97 | backup create/delete/stop_device | Implemented @ pcloud-daemon/src/backup_backend.rs:… | Implemented @ `crates/pcloud-backends/src/backup_backend.rs:…` | Citation drift fix |
| 98 | psync_delete_backup_device | Implemented | Implemented *with note*: "SDK-only surface (`EmbeddedDaemon::delete_backup_device`). Not exposed via IPC/CLI because the operation is purely a local settings clear; C callers hit it through `psynclib.h` directly." | Scope note clarification |
| 107–128 | crypto * | Implemented @ pcloud-daemon/src/runtime.rs + pcloud-crypto/src/lib.rs | Keep pcloud-crypto cites as-is; pcloud-daemon cites are correct since runtime.rs is under pcloud-daemon | No change |
| 117 | psync_crypto_folderid | Implemented @ `crates/pcloud-crypto/src/lib.rs` | **Partial** — function exists at `lib.rs:1022` but unreachable from IPC/CLI/SDK callers. Add tracking bead under `bd-1du.10` for external surface. | New gap |
| 118 | psync_crypto_folderids | Implemented @ `crates/pcloud-crypto/src/lib.rs` | **Partial** — same as row 117 (`folder_ids` at `lib.rs:1032`). | New gap |
| 124 | psync_crypto_share_folder | Implemented @ pcloud-daemon/src/shares_backend.rs | Implemented @ `crates/pcloud-backends/src/shares_backend.rs`; `crates/pcloud-crypto/src/share_temppass.rs` | Citation drift fix |
| 130–141 | shares * | Implemented @ `crates/pcloud-proto/src/shares_api.rs` | Keep proto cites; pcloud-daemon-referenced rows should be `crates/pcloud-backends/src/shares_backend.rs` | Citation drift fix |
| 145–168 | public links * | Implemented @ pcloud-daemon/src/public_link_backend.rs | Implemented @ `crates/pcloud-backends/src/public_link_backend.rs` | Citation drift fix |
| *new* | Daemon handler for `Request::VerifyPath` | — | Add new row tracking the orphan IPC variant (see CRITICAL-1) | New gap |

---

### 1.8 Per-subsystem summary

**Auth / TFA.** Genuinely solid. Live-verified. Password, TFA SMS, TFA device-notification, TFA code submission, recovery code, token-based auth, userinfo all exist and test against a real account (`tests/live_auth.rs`). Citation drift needs fix (rows 17, 20, 21, 22) but no functional gap. The registration / lost-password / change-password / verify-email family is Implemented via SDK + IPC (`Request::AccountChangePassword`, `Request::AccountRegister`, `Request::VerifyEmailRestricted`, `Request::LostPassword`).

**Transfers.** Upload state machine with SQLite resume, backoff, and auth refresh is real. Direct helpers (`upload_data{,_as}`, `upload_file{,_as}`) and `UploadSession` chunked state machine both present. `get_file_link` + `download_file` present. Matrix row 93 honestly annotates that chunked pipelining (`PSYNC_MAX_PENDING_UPLOAD_REQS=16`) is **sequential** in Rust — flagged as a perf follow-up, not a parity gap. Defensible.

**Public links.** Full feature set wired: file/folder create, tree link, updownlink, show/delete, list, expire/password/upload policy, access list, bookmarks, screenshot link with hour-rounded delay, `send_publink`. Matrix citations misplace the backend to `pcloud-daemon` (real location is `pcloud-backends`). No functional gap.

**Shares / business / teams.** Share request listing, share CRUD, accept/decline/cancel, modify, business contact list, team list, `account_stopshare`, `account_modifyshare`, `account_teamshare`, crypto-aware variants. All wired. Same citation drift issue.

**Crypto.** Substantive — setup, start/stop/reset, mkdir with deterministic HMAC-SHA256 filename, AES-256-GCM sector encryption, Argon2 KEK, HMAC fingerprint, constant-time password compare, change_password with server confirmation code, `priv_key_flags`. The `change_password` test suite is the most thorough in the daemon tests tree (10+ cases in `tests/crypto_change_password.rs`). **Unreachable folder-id enumeration** (rows 117/118) is the only real gap here. CLAUDE.md is wrong about "change_crypto_pass / priv_key_flags / send_change_user_private" being missing.

**Sync.** Engine side (`pcloud-engine`) with `ReconcileWorker`, `DiffWorker`, `DiffEventDispatcher`, `sync_suggest` (full port of C extension-based scorer), `is_folder_syncable` with `/proc/self/mountinfo` parsing. Matrix row 74 (`psync_run_localscan`) honestly concedes the loop is not fully mounted-drive-coupled yet (`bd-1du.4` territory). Row 85 (FUSE mount) is aggressive but defensible given the gated kernel-roundtrip tests.

**Backup / device / account.** Create/delete backup with cascade (remote `backup/createbackup` + local sync-root registration in `SyncType::UploadOnly`), stop device, idempotent cascade removal, `delete_backup_device` as SDK-only. Account utilities (`verify_email`, `lost_password`, `change_password`, `register`, `get_promo`, `get_api_servers`, `set_language`, `set_api_server`) all wired. Citation drift.

**SDK.** Breadth claim accurate — ≈80 public methods on `EmbeddedDaemon`. Documentation thin at the walkthrough level (1 example, no guide), dense at the function level. No unintended `pub use` leaks.

**Filesystem / mounted drive.** Row 85 marked Implemented. `bd-1du.4` still open per STATUS.md. Reconciliation needed.

**Plugins / IPC orphans.** `Request::VerifyPath` orphan is the one CRITICAL finding. `Method::FileHistory` argumentless variant is a dead code path. `Command::FileDiff` / `Command::FileRestore` are deliberately mapped to `GetHealth` — should be removed or made to return a clear "unimplemented" response.

---

### 1.9 Recommendations for `bd-1du.10` closure

A short, specific list for the final parity-proof bead:

1. **Fix pervasive citation drift** in the matrix (~60–80 rows). One-pass `sed` over `rust_reference` cells with grep-verified new paths. Blocking.
2. **Add a daemon handler** for `Request::VerifyPath` or **remove** the variant. Either side is acceptable; the current half-wire is not. Blocking.
3. **Flip rows 117 and 118** to `Partial` with a `bd-1du.10` pointer, or **add external surfaces** (IPC/CLI/SDK enumeration helpers). Blocking.
4. **Reconcile CLAUDE.md** with the matrix. Strike the obsolete "still missing" bullets in crypto and backup sections. Blocking for release docs.
5. **Remove or fix** `Command::FileDiff` / `Command::FileRestore` CLI stubs. Nonblocking for parity, blocking for operator UX.
6. **Remove** `Method::FileHistory` from the `Method` enum (argument-bearing `Request::FileHistory` is the live path). Nonblocking.
7. **Clarify row 85** annotation about the `PCLOUD_FUSE_TEST` gate and `bd-1du.4` scope split. Nonblocking but improves truth-surface.

After steps 1–4 land, `bd-1du.10` can honestly gate. Release/docs wording (per STATUS.md) should only call the product "pre-alpha" until then.

---

*End of Section 1.*

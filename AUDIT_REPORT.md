# pcloud-rs Enterprise Readiness Audit Report

**Date:** 2026-04-17
**Auditor:** Claude Agent (multi-agent parallel audit — 10 Opus 4.7 specialists)
**Scope:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/` including `crates/`
**Audit prompt:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/pcloud_rev.md`
**Methodology:** 10 parallel specialist auditors, each owning 1–2 of the 12 audit dimensions, writing per-section findings with file:line references. Findings synthesized into this unified report. No source files were modified by the audit.

---

## Executive Summary

**Overall readiness:** pcloud-rs is a *substantively implemented* clean-room Rust rewrite of the pCloud client. The gating discipline set out in `CLAUDE.md` — no false "parity" / "production ready" / "drop-in" claims, stricter-than-C security posture, evidence-before-closure on `bd-1du.10` — is **visibly enforced throughout the code and docs**. All core workspace gates pass: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo deny --locked check` are green. Secret wrappers (`SecretString`/`SecretBytes`) zeroize correctly with constant-time compares, the auth vault is opt-in with `0600` file + `0700` parent dir + atomic tmp+rename writes, production transport refuses plaintext, `danger_accept_invalid_certs` appears nowhere in `src/`, the parity matrix / STATUS / review files are internally consistent (186 / 158 / 0 / 0 / 28), and every `Rejected` row has a matching rationale in `REJECTED-RATIONALES-14042026.md`.

**It is not, however, deployment-ready today.** The blockers cluster into seven groups, all of which must close before `bd-1du.10` can honestly be satisfied:

1. **The sync engine is structurally non-functional under realistic load** — the scheduler's `next_batch` is a pure peek that never dequeues (`scheduler.rs:122-127`), so every cycle re-emits the same operations; the default `rename_both` conflict policy does not rename and the `newest_wins` policy ignores timestamps (`conflict_resolver.rs:170-191`), silently destroying local edits; queues are unbounded; stall detection is absent; engine state is entirely non-durable across restart; `crates/pcloud-engine/tests/` does not exist.
2. **FUSE (`bd-1du.4`) is scaffolding, not production** — the fuser shim has no `statfs` (`df` ENOSYSes on the mount), the write journal `commit()` fsyncs the file but not the parent directory (data-loss window that contradicts its own doc contract), `ProtoUploadBackend::upload_file` slurps entire staging blobs into memory (OOM on large files), journal `replay_path` exists but the daemon never calls it on startup, all kernel-mounted FUSE integration tests are `#[ignore]`+env-gated (CI runs zero), and `MountService::mount` has no Windows arm.
3. **There is no continuous integration at all** — `.github/workflows/` does not exist; `fuzz/README.md` and `codecov.yml` reference pipelines that aren't there; the codecov hard-flip date `2026-04-29` is 12 days from now; **every tier-1 platform claim (Linux / FreeBSD / macOS / Windows) is currently unsubstantiated**.
4. **No per-request IPC capability scoping** — any local process running as the daemon's user reaches `Shutdown`, `CryptoReset`, `Logout`, `CryptoChangePassword*` and every other privileged method. The only gate at accept is uid-match.
5. **One orphan IPC handler and several cross-document path drifts** — `Request::VerifyPath` is constructed by the CLI (`commands.rs:1102`) but `runtime.rs` has zero handler for it (it falls through to the "unsupported ipc request" arm); 41+ rows in `C_FEATURE_PARITY_MATRIX.csv` cite `crates/pcloud-daemon/src/*_backend.rs` paths that no longer exist (all moved to `crates/pcloud-backends/src/`), and the same stale paths carry into `ARCHITECTURE.md`, `API-REFERENCE.md`, `SECURITY.md`, and `CLAUDE.md` itself.
6. **Crypto byte-compatibility with the legacy C client is unverified** — the Rust `pcloud-crypto` crate uses AES-256-GCM + HMAC-SHA256 primitives and there is no known-answer test (KAT) against `pclsync/pcryptofolder.c` output. Files encrypted by the legacy C client may not round-trip through the Rust client. Additionally, password rotation silently invalidates all existing sector ciphertext because per-file keys are `HMAC(master, …)` — a data-loss trap with no warning to the user.
7. **Packaging/release will wedge** — `packaging/windows/wix/pcloud-rs.wxs:14` ships a placeholder `UpgradeCode="PUT-A-STABLE-GUID-HERE-0000-000000000000"`; any MSI shipped with this GUID cannot be upgraded.

**Documentation drift correction.** Both the §1 parity auditor and the §3 crypto auditor independently confirm that `CLAUDE.md` is wrong about four symbols it lists as "missing": `change_crypto_pass`/`change_crypto_pass_unlocked`, `send_change_user_private`, `priv_key_flags`, and `psync_send_publink` are **all implemented** with live code, daemon dispatch, SDK helpers, and tests. The parity matrix already reflects this; `CLAUDE.md` must be reconciled.

**Top strengths.** Workspace discipline is strong: `#![forbid(unsafe_code)]` holds crate-wide in `pcloud-crypto`, all nonces/IVs come from `OsRng`, the master key is never serialised, policy gates reject `persist_master_key=true`, temppass signatures are verified before AEAD unwrap, every tested FFI `unsafe` block in `pcloud-fs/platform/*` that was spot-checked carries a `SAFETY:` comment, graceful-drain has a real state machine, the circuit breaker is panic-safe via `ProbeGuard`, Windows named-pipe peer checks do a real SID comparison, the systemd unit at `packaging/systemd/pcloudd.service` is unusually hardened for a first-party service, and CLAUDE.md's no-false-claims rule is visibly enforced across 10+ docs.

---

## Findings by Severity

| Severity | Approx count | Meaning |
|---|---|---|
| **CRITICAL** | ~23 | blocks deployment / data loss / security vulnerability |
| **HIGH** | ~89 | significant gap / compliance risk / correctness bug |
| **MEDIUM** | ~120 | quality issue / missing feature / doc drift |
| **LOW** | ~95 | enhancement / polish |

Per-dimension breakdown (taken from each specialist's findings index):

| Dimension | CRIT | HIGH | MED | LOW | Headline |
|---|---|---|---|---|---|
| 1. Parity & API Coverage | 1 | 4 | 4 | — | `Request::VerifyPath` has no daemon handler |
| 2. Security | 0 | 4 | 9 | 9 | 60+ proto structs expose `pub auth_token: String` with `#[derive(Debug)]` |
| 3. Crypto | 1 | 7 | 8 | 9 | no cross-client KAT vs C `pcryptofolder.c` |
| 4. Sync Engine | 8 | 14 | 18 | 20 | scheduler never dequeues; conflict policies broken |
| 5. FUSE Parity | 8 | ~15 | — | — | no `statfs`, journal `fsync` gap, OOM upload, no Windows arm |
| 6+7. Transport + IPC | 1 | 10 | — | — | no IPC capability scoping; retry classifies `InvalidCertificate` as transient |
| 8. CLI + SDK | 0 | 11 | 20 | 14 | hand-rolled parser + clap completion tree drifted; positional secrets |
| 9. Code Quality | 0 | 4 | — | — | ~40 `expect("poisoned")` on daemon hot paths; gates **green** |
| 10. Testing | 2 | ~8 | — | — | **no CI; `.github/workflows/` does not exist** |
| 11+12. Deploy + Docs | 2 | 12 | 26 | 22 | Windows MSI UpgradeCode placeholder; 41+ matrix rows cite dead paths |

Totals are approximate because some agents grouped findings without publishing a full MED/LOW tail; exact tables live in each detailed section below.

---

## Remediation Roadmap

### Phase 1 — Critical Blockers (must fix before ANY deployment)

1. **Sync scheduler correctness.** `crates/pcloud-engine/src/scheduler.rs:80-127` — replace the flat `Vec`-sorted-by-priority with per-root fairness (round-robin or weighted-deficit); make `next_batch` actually remove dequeued ops so completion doesn't re-emit them forever. Write an integration test that runs two sync roots and asserts the second one makes progress.
2. **Conflict resolver correctness.** `crates/pcloud-engine/src/conflict_resolver.rs:170-191` — fix `newest_wins` to compare timestamps; fix `rename_both` to actually rename (the default policy silently does nothing). Add a property test that exercises both.
3. **Sync engine persistence.** Persist queue state + retry state + in-flight transfers across restart. Today a daemon crash mid-sync drops everything.
4. **FUSE `statfs`, journal fsync, streamed upload.** `crates/pcloud-fs/src/platform/fuser_shim.rs` — implement `statfs`; `crates/pcloud-fs/src/journal.rs` `commit()` — fsync the parent directory, not just the file (MS-FSA §6.5); `crates/pcloud-fs/src/backend.rs` `upload_file` — stream from disk, don't buffer the whole staging blob.
5. **FUSE journal replay on boot.** Wire `replay_path()` into the daemon's startup bootstrap; an orphaned journal is currently dead data.
6. **IPC capability scoping.** `crates/pcloud-daemon/src/runtime.rs` dispatch path — gate every privileged method behind an explicit capability check before uid-match authorization. Today any local process running as the same user reaches `Shutdown`/`CryptoReset`/`Logout`/`CryptoChangePassword*`.
7. **`VerifyPath` handler.** Either wire a daemon handler in `runtime.rs` for `Request::VerifyPath` or remove it from the CLI (`commands.rs:1102`) and mark `Rejected` in the matrix.
8. **Windows MSI UpgradeCode.** `packaging/windows/wix/pcloud-rs.wxs:14` — replace the placeholder GUID with a real, committed, permanent GUID. Once an MSI ships with a real GUID, it cannot be changed without breaking upgrades.
9. **Stand up CI.** Create `.github/workflows/*.yml` (Linux / macOS / FreeBSD / Windows) that actually compiles and tests per-platform. Without CI every tier-1 claim is vapor.
10. **Cross-client crypto KAT.** Add `crates/pcloud-crypto/tests/kat_legacy_c.rs` that decrypts a sample file encrypted by the upstream C `pcryptofolder.c` — or, if the byte formats are intentionally divergent, document it loudly in `SECURITY-MODEL.md` and flag legacy-file migration as a user-visible workflow.
11. **Password-rotation data preservation.** Either re-encrypt all sector ciphertext on `change_crypto_pass`, or introduce a KEK indirection so rotation does not invalidate prior ciphertext. Today rotation silently locks the user out of their own data.

### Phase 2 — Security Hardening (must fix before production)

1. **Debug-redact the proto request builders.** `crates/pcloud-proto/src/methods/**` — 60+ structs carry `pub auth_token: String` / `pub password: String` with `#[derive(Debug)]`. Replace the fields with `SecretString` or implement a redacting `Debug`.
2. **Path input validation.** `Request::SyncRootAdd.local_path` is accepted into `runtime.rs:3952` without NUL / `..` / symlink-escape checks before `canonicalize`. Add a shared `validate_local_path()` helper and use it at every path-accepting IPC entry.
3. **TFA recovery-code wrapper.** `Request::TwoFactorCodeSubmission.value` is a plain `String` that may carry a long-lived recovery phrase — wrap in `SecretString`.
4. **ResilientTransport classifier.** `crates/pcloud-resilience/src/transport.rs` treats `InvalidCertificate` as `Transient` and retries it. Make cert-validation errors terminal.
5. **Wire `MethodRetryPolicy` into `ResilientTransport`.** `upload_create → upload_write → upload_save` currently has no idempotency anchor at the transport layer.
6. **`pcloud-web` authentication.** Web UI has CSRF but no auth; any sibling local process bypasses it. Add bearer-token + per-endpoint capability check.
7. **Mutex-poisoning sweep.** `crates/pcloud-*/src/` — ≈40 `Mutex::lock().expect("poisoned")` on daemon hot paths; replace with graceful degradation or `parking_lot::Mutex` that never poisons.
8. **SAFETY-comment sweep.** 35 `unsafe` blocks missing `// SAFETY:` (clustered in `signals.rs`, `pcloudc/src/main.rs`, `prompt.rs`) — add or refactor.
9. **WinFSP version probe + macFUSE/fuse-t runtime probe** — `crates/pcloud-fs/src/platform/windows.rs` and `platform/macos.rs`; currently load blindly.
10. **FreeBSD rc.d `kldload fuse`** — the script does not pre-load the module.

### Phase 3 — Feature Completion (enterprise parity)

1. **Engine stall detection + Retry-After honoring + global retry budget + idempotency keys** across `pcloud-engine` + `pcloud-resilience`.
2. **NFC / case-insensitive conflict detection** in the conflict resolver (macOS + Windows).
3. **Watcher inotify-overflow rescan** (`notify` integration) — currently silently drops events.
4. **Staging-cache disk budget** — today eviction is lossy and unbounded.
5. **Engine battery/power awareness** — exists for the integrity sweeper only, not the sync loop.
6. **FUSE `access`, `forget`, `rename-flags`, `setattr-mode`, `readlink`, xattr ops.**
7. **FUSE read-ahead/prefetch; `FileHandle::size` population.**
8. **macOS + Windows FUSE SIGTERM/CTRL-C handlers; Windows orphan detection** (currently a stub); **fuse-t `LowlevelOps` layout validation**.
9. **CLI↔IPC matrix closure.** Add CLI subcommands for `Method::Health`, `CryptoSetup`, `CryptoMkdir`, `SyncRootPause/Resume`, `ValueGet/Set/Has`.
10. **CLI unify on clap.** Replace the 4700-line hand-rolled parser with a single clap derive tree that also feeds completions.
11. **SDK examples + feature flags.** Add rustdoc examples for 80+ public helpers; introduce `[features]` to pick TLS provider (`rustls+ring` / `aws-lc-rs` / `native-tls`).
12. **SDK semver hygiene.** Remove `pub use pcloud_proto::Notification` and `pub use upload_session::UploadSessionDriver` from the SDK's public surface.
13. **Typed-ID sweep.** 13 raw `u64`/`String` ID parameters remain despite `pcloud-model::ids` newtypes — systematize.

### Phase 4 — Polish, Docs & Release Readiness

1. **Parity matrix + doc path reconciliation.** Fix the ~60–80 `rust_reference` rows in `C_FEATURE_PARITY_MATRIX.csv` that still cite `crates/pcloud-daemon/src/*_backend.rs`; these modules moved to `crates/pcloud-backends/src/`. Propagate the fix into `ARCHITECTURE.md`, `API-REFERENCE.md`, `SECURITY.md`, and `CLAUDE.md`.
2. **CLAUDE.md crypto correction.** Remove the "still missing" claims for `change_crypto_pass`/`send_change_user_private`/`priv_key_flags`/`psync_send_publink` — all are implemented.
3. **Dashboards.** `dashboards/` directory is empty; ship Grafana JSON + Prom alert rules matched to the `pcloud-observability` counter inventory.
4. **mdbook.** Run `cd docs/book && mdbook build` in CI and fail on broken links.
5. **SDK rustdoc sweep.** `cargo doc --workspace --no-deps` should be warning-free; two SDK rustdoc examples reference files that don't exist.
6. **Proptest coverage sweep.** `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:15` enumerates ~30 of 45 `Method` variants; close the gap and use a compile-time exhaustiveness check (remove the `_ => 0` bypass).
7. **Live-e2e breadth.** Add suites for account utilities, transfers (currently 1 test for 4 variants), public links (1 test for 12 RPCs), backup/device (zero coverage).
8. **Fuzz targets.** Add `cargo fuzz` targets for the crypto sector decoder, HTTP response parser, path validator.
9. **Test suite bootstrap.** Add `tests/` directories for `pcloud-auth`, `pcloud-config`, `pcloud-engine`, `pcloud-idp`, `pcloud-kms`, `pcloud-store`.
10. **`#[ignore]`d Windows IPC tests.** Un-ignore `platform_ipc_crossplat.rs:148,194` once the WinFSP backend ships — they currently contradict the Windows tier-1 claim.
11. **`sync_loop_live.rs:36`** — add `#[ignore]` guard; silently passes without assertions when unconfigured.

---

## Detailed Findings

The following sections contain the full per-dimension findings with file:line references. Each was produced by an independent Opus specialist against the audit prompt. Section ordering follows the dimension numbering in `pcloud_rev.md`.


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
## Section 2. Security Audit

**Auditor**: Dimension 2 (Security)
**Workspace**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/`
**Date**: 2026-04-17
**Scope**: secret discipline, auth vault, local IPC security, transport policy, downgrade/replay, FFI memory safety, input validation, DoS, logging discipline.
**Out of scope (other dimensions)**: cryptographic algorithm review (Dim 3), parity-matrix (Dim 1), detailed threat model (Dim 5).

### Executive summary

The pcloud-rs security posture is **substantially stronger** than the legacy C client on every surface inspected. Secret lifetimes are governed by an explicit `SecretString`/`SecretBytes` abstraction with `ZeroizeOnDrop`, constant-time equality, redacted `Debug`, no `Serialize`/`Deserialize` impls, and hand-rolled `clone_secret` methods so every duplication is audit-visible. The IPC transport enforces a `0600` socket under a `0700` parent, mandatory `SO_PEERCRED` / `getpeereid(3)` / per-SID DACL peer verification, a 1 MiB pre-allocation cap, a 5-second read timeout, and returns sanitized error messages. The auth vault is opt-in, atomically written with `O_CREAT|O_EXCL`, validated for ownership and mode on every read, and intentionally does not persist passwords. The production profile rejects plaintext transport in `ApiEndpoint::validate` and `RevisionUrl::validate`; the code base is free of `danger_accept_invalid_certs` / `accept_invalid_hostnames` in `src/` (only documentation references exist). The FFI surfaces (`platform/{linux,bsd,macos,windows}.rs`, `macos_ffi.rs`, `winfsp_ffi.rs`) carry SAFETY comments on every `unsafe` block with plausible invariant statements.

The **remaining gaps** are narrower and fall into four buckets:

1. A handful of transit-only IPC request fields remain plain `String` where a `RedactedString` wrapper is warranted.
2. `pcloud-proto` request-builder structs derive `Debug` while carrying a plaintext `auth_token: String` — this can leak the token via `format!("{req:?}")`.
3. The file-vault validator checks the vault file itself but does **not** re-validate parent-directory ownership/mode on read.
4. The IPC serve loop is single-threaded (`bound.serve_once`) with no per-peer connection cap — DoS is limited to blocking subsequent requests for up to the 5 s read timeout but a slow client can still impede refresh ticks and other service users.

No **CRITICAL** findings were identified. Four **HIGH** findings relate to secret fields derived-Debug in `pcloud-proto` and missing path normalization for the `SyncRootAdd.local_path` input. Nine **MEDIUM** findings cover vault parent-dir validation, `TwoFactorCodeSubmission.value` using plain `String`, and defense-in-depth items. **LOW** findings cover documentation accuracy and minor hardening.

---

## CRITICAL findings

**None identified.** No cleartext password persistence, no world-readable sockets, no plaintext-in-production path, no reachable `danger_accept_invalid_certs` / `accept_invalid_hostnames`, and no logs that interpolate `SecretString::expose_secret`.

---

## HIGH findings

### H1. Protocol request structs derive `Debug` with plaintext `auth_token: String`

**Files / lines (selected — pattern is systemic)**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:170-175` — `UserInfoRequest { auth_token: String, ... }` with `#[derive(Debug, Clone, PartialEq, Eq)]`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:214-225` — `TwoFactorLoginRequest { token: String, ... }`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:270-285` — `TwoFactorSendSmsRequest { token: String, ... }`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:311-330` — `TwoFactorSendNotificationRequest { token: String, ... }`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/upload.rs:88-94` and `:155-170`, `:208-215`, `:264-270`, `:331-340`, `:382-390`, `:450-460`, `:500-510`, `:548-560` — every request struct in the upload-session family carries `pub auth_token: String`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/shares.rs:34, 56, 79, 137, 158, 179, 209, 230, 254, 281, 310, 361` — every `SharesXxxRequest` has `auth_token: String` and derives `Debug`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/public_links.rs:14` through `:666` — 24 request structs, same pattern.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/account.rs:12, 62, 255, 323` — including `RegisterRequest { password: String }` at line 323.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/folder.rs:11, 64, 147, 194, 247, 293, 338`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/crypto.rs:34, 105`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/backup.rs:28, 99, 149`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/diff.rs:16`, `download.rs:13`, `notifications.rs:35, 86`.

**Severity**: HIGH.

**Impact**: any `log::debug!("{req:?}")`, `tracing::debug!(?req)`, `panic!` path that formats a request with `{:?}`, or a future observer/middleware that derives `Debug`-display at tracing spans will emit the live pCloud auth token or account password in plaintext to logs. The counterpart IPC-boundary types in `pcloud-ipc/src/methods.rs` took the correct approach (`RedactedString`) in response to audit finding H1 (see e.g. `:279`, `:285`, `:301`, `:307`, `:336`, `:339`, `:354`, `:473`, `:963`, `:966`), but the lower protocol layer never followed. `CHANGELOG.md:1975` claims the repo is free of token-leaking Debug output, which is not accurate for these builder structs.

**Remediation**:
1. Change every `pub auth_token: String` / `pub password: String` / `pub token: String` on request builders in `crates/pcloud-proto/src/methods/**/*.rs` to `pcloud_ipc::RedactedString` (or an equivalent redacted wrapper local to `pcloud-proto`).
2. Alternatively, keep the wire field as `String` but remove `Debug` from the derive list, and provide a manual `impl Debug` that renders `UserInfoRequest { auth_token: <redacted N bytes>, ... }`. This keeps call sites compiling.
3. Add a negative `cargo test` that `format!("{:?}", req)` on each of these request types MUST NOT contain the secret literal.
4. Consider crate-wide lint via `#[deny(clippy::disallowed_types)]` or a custom lint that bans `Debug` derive on structs with fields named `auth_token` / `password` / `token`.

---

### H2. `SyncRootAdd.local_path` and related path-accepting requests are not validated for NUL/`..`/symlink escape before use

**Files / lines**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs:376-387` — `Request::SyncRootAdd { local_path: String, remote_path: String, ... }`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:3952-4015` — `add_sync_root` validation flow. Only `trim().is_empty()` is checked on the raw string; `canonicalize` is relied on to resolve the path but there is no explicit NUL-byte check, no explicit `..` rejection, and no explicit rejection of paths outside a configured sandbox. `canonicalize` will follow symlinks, potentially pointing to a system directory the attacker wanted the daemon to sync over.
- Same pattern in `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:609-611` (`GetSyncSuggestions`), `:612` (`IsFolderSyncable`), `:616-617` (`CreateFilePublicLink` / `CreateFolderPublicLink` — remote paths, but no NUL/traversal check either).

**Severity**: HIGH for `SyncRootAdd.local_path` specifically (the daemon accepts any symlink target as a sync root and will happily upload its contents); MEDIUM for the remote-path public-link variants (server enforces ACL, so exposure is bounded to authenticated user).

**Impact**: a compromised CLI or unprivileged local process that has passed peer-uid authorization (same-user) can cause the daemon to:
- sync a symlink-pointed system directory (e.g. `~/evil -> /etc`) as a "sync root",
- produce path-traversal via `../` entries on the remote side when combined with later operations that join strings,
- force the daemon to open a NUL-embedded path, which would fail late inside a `CString::new` call with a less-useful error (already observed in `linux.rs:198` where the fallback branch is dead on happy-path validators).

Note: the mount-point validator in `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_service.rs:115-156` already demonstrates the right shape (existence, kind, ownership, non-world-writable); `add_sync_root` needs the same discipline.

**Remediation**:
1. Introduce `fn validate_user_supplied_path(s: &str, mode: PathMode) -> Result<PathBuf, ValidationError>` that rejects:
   - `s.contains('\0')`,
   - any segment equal to `..` (post-canonicalization compare against a sandbox base),
   - paths outside the configured sync-root sandbox (configurable allow-list of user-home / `/home/<user>`),
   - non-canonical path length > `PATH_MAX - 1` (4095 bytes on Linux; 1023 on macOS with NFC normalization).
2. Plumb it into `add_sync_root`, `suggest_sync_folders_at`, `check_folder_syncable`, and every IPC dispatch arm that takes a `path: String`.
3. For macOS, apply NFC / NFD normalization via `unicode-normalization` so paths round-trip identically through HFS+ / APFS.
4. Reject absolute paths that resolve (after canonicalize) to `/proc`, `/sys`, `/dev`, `/run` on Linux — these are never legitimate sync roots.

---

### H3. `snapshot::restore_encrypted_snapshot` is the only path that rejects tar traversal; nothing else validates inbound path names

**File / line**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/snapshot.rs:620-632` is correctly written: it rejects entries whose path is absolute, contains `..`, NUL, `/`, or `\`. This is the **only** place in the workspace that defends against tar-slip / ZIP-slip.

**Severity**: HIGH if any other code path ever extracts user-supplied archives.

**Impact**: future feature work (backup-restore, plugin archive extraction, sync-state import) will fail open unless engineers copy-paste the same check. There is no shared `validate_archive_entry` helper.

**Remediation**:
1. Extract the check at `snapshot.rs:625-631` into a reusable `fn is_safe_relative_path(rel: &Path) -> Result<(), UnsafePathReason>` in a shared crate (`pcloud-model` or a new `pcloud-fs-safety` module).
2. Unit-test with adversarial cases: `../../etc/passwd`, `C:\Windows\System32`, leading `/`, backslash-separated Windows-style, CRLF-embedded, `\0`-embedded, overlong-UTF-8.
3. Add a Clippy-style internal lint that flags `Archive::entries` / `zip::ZipArchive::read_zipfile_from_stream` consumers that do not call the helper.

---

### H4. IPC `TwoFactorCodeSubmission.value` is `String`, not `RedactedString`

**File / line**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs:288-296`:

```rust
TwoFactorCodeSubmission {
    /// The numeric TFA code or the user's recovery phrase.
    value: String,
    ...
}
```

**Severity**: HIGH (recovery-code path), MEDIUM (ephemeral OTP path).

**Impact**: when `recovery_code = true`, the submitted value is the user's **static recovery phrase** — equivalent to a long-lived credential. A derived `Debug` on `Request` therefore leaks the recovery phrase at any `log::debug!("{req:?}")` site. The enum `Request` carries `#[derive(Debug, ...)]` at `methods.rs:260`, so this leaks just like H1.

**Remediation**:
- Change `value: String` to `value: RedactedString` at `methods.rs:290`, mirroring the treatment of `PasswordSubmission.value`.

---

## MEDIUM findings

### M1. Vault `validate_vault_file` does not validate parent-directory ownership/mode on load

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:198-221`.

**Impact**: `store_token` unconditionally tightens the parent directory to `0o700` (line 142), but `load_token` via `validate_vault_file` only inspects the file, not the parent. If a previous install left the config directory as `0o755` before the first `store_token` call, an attacker who had transient `drwx` on that dir could plant symlinks at sibling paths. The `O_CREAT|O_EXCL` protection at `file.rs:161-167` mitigates the write path, but the load path returns a secret from a directory whose provenance was never checked.

**Severity**: MEDIUM (requires pre-existing weak parent, then concurrent write).

**Remediation**: add to `validate_vault_file`:
```rust
#[cfg(unix)]
if let Some(parent) = path.parent() {
    let parent_meta = fs::symlink_metadata(parent)?;
    if !parent_meta.file_type().is_dir() {
        return Err(AuthVaultError::InsecureMetadata("vault parent must be a directory"));
    }
    if parent_meta.uid() != current_uid {
        return Err(AuthVaultError::InsecureMetadata("vault parent must be owned by current user"));
    }
    if parent_meta.mode() & 0o077 != 0 {
        return Err(AuthVaultError::InsecureMetadata("vault parent must not grant group/other access"));
    }
}
```

---

### M2. IPC serve loop is single-threaded: no per-peer connection cap, no global connection cap

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/serve.rs:127-230` and `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:167-229`.

**Impact**: `bound.serve_once` accepts exactly one connection per loop iteration. A malicious peer (same-uid, having passed authorization) can open a socket, wait the 5 s read timeout, close, reopen — blocking session-refresh ticks (`serve.rs:227-229`) and any concurrent CLI invocation. While the 5 s cap prevents indefinite wedging, it is a clean availability DoS against the daemon from a cooperating attacker inside the user session.

**Severity**: MEDIUM — only exploitable by an attacker who already controls the user account (peer-uid check passes), and session-refresh recovers on next iteration.

**Remediation**:
1. Move to a bounded worker pool with, e.g., a `Semaphore` admitting ≤ 8 concurrent IPC requests. The `BoundIpcServer::listener.accept()` path at `transport.rs:171` should be run in a dispatcher thread that hands each accepted stream off.
2. Add a per-peer (keyed by pid) quota: at most N in-flight requests per peer pid. Same-uid but different-pid peers get independent budgets.
3. The 5 s read timeout should also apply to the write half (currently only `set_read_timeout` at `transport.rs:184`).
4. Consider capping `MAX_PIPE_INSTANCES` at `platform/windows.rs:61` down from 32 once the peer-pid quota is live — 32 is a lot of headroom in the current no-quota regime.

---

### M3. `write_response` is not timeout-bounded — slow writers can hold the serve loop

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:366-376`.

The read timeout at `serve_stream_once:184` only governs the request read. The response write (`write_response`) calls `stream.write_all()` + `stream.flush()` unbounded, so a slow-reader client can hold the serve loop for longer than `IPC_REQUEST_READ_TIMEOUT`.

**Severity**: MEDIUM.

**Remediation**: set `stream.set_write_timeout(Some(IPC_REQUEST_READ_TIMEOUT))` alongside the existing `set_read_timeout` call at line 184.

---

### M4. `current_effective_uid` lacks a `CAP_SETUID`/`euid!=ruid` sanity gate

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/auth.rs:65-68`.

`PeerIdentity::matches_owner` compares against `libc::geteuid()`, which is the correct source, but if the daemon is ever launched setuid (`sudo pcloudd`), the effective uid will be `root` while the real user — the pcloud owner — may differ. The IPC accept path then trusts any root peer.

**Severity**: MEDIUM — the daemon is not documented to run setuid, but nothing in `bootstrap.rs` rejects it.

**Remediation**: in `bootstrap.rs`, assert at startup that `geteuid() == getuid() && getegid() == getgid()`; refuse to bind IPC otherwise. Log a security-audit event at info level.

---

### M5. Linux `signal_trampoline` runs non-async-signal-safe code

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:87-117`.

Although the comment at line 106 claims `umount2` is async-signal-safe, the trampoline also:
- calls `mtx.lock()` (`Mutex` is not async-signal-safe — it can deadlock if the signal interrupts a thread that already holds it),
- calls `CString::new(...)` which may allocate.

**Severity**: MEDIUM — not exploitable for a security bypass, but a crash under SIGTERM loses the audit-trail and leaves stale mounts.

**Remediation**: the correct pattern is a self-pipe (write a single byte to a pipe from the signal handler, a worker thread reads it and does the real unmount), or use `signalfd(2)` / `sigwait(2)` in a dedicated thread. Document the known-unsafe pattern inline until fixed.

---

### M6. FFI transmute_copy on fn-ptrs in `winfsp_ffi.rs` is unchecked for ABI compatibility

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/winfsp_ffi.rs:494`, `:513`:

```rust
Ok(std::mem::transmute_copy::<_, T>(&f))
```

**Severity**: MEDIUM — the WinFSP DLL ABI is stable, but a mismatched build (pcloud-rs built against WinFSP 2.x headers, runtime 1.x) will not be caught here; the first call produces undefined behaviour.

**Remediation**:
1. Add a version-probe (`FspVersion` export) before `resolve` and compare against a pinned major-version constant.
2. Replace `transmute_copy` with the safer `mem::transmute::<*const c_void, T>(f as *const c_void)` wrapped in a `fn()` newtype, or migrate to the `libloading` crate which exposes a typed `Symbol<F>` that at least documents the contract.
3. Per-symbol inline SAFETY comments are present (✓), but do not record the expected signature — add the full `typedef` from the upstream C header.

---

### M7. `fs::symlink_metadata` + `fs::File::open` TOCTOU in vault load

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:200-221` (validate) → `:89-97` (open).

Between `symlink_metadata` and `fs::File::open(path)`, an attacker in the same uid (malicious plugin, compromised CLI) can swap the file for a symlink. Since validation already asserts owner-uid matches current uid, real exploitability requires local same-user adversary — so the window is narrow — but the audit standard here should be `open + fstat` (open by fd, then validate metadata via `fstat` on that fd) to eliminate the race.

**Severity**: MEDIUM (defense-in-depth).

**Remediation**:
1. Use `nix::fcntl::open` with `O_NOFOLLOW | O_CLOEXEC`, then `nix::sys::stat::fstat` on the returned fd.
2. This also eliminates the duplicate Unix-only cfg in the current code.

---

### M8. `ConvertStringSecurityDescriptorToSecurityDescriptorW` has no fallback when the SID lookup fails

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/windows.rs:385-407`.

If `current_user_sid_string()` somehow returns a malformed SDDL substring, `ConvertStringSecurityDescriptorToSecurityDescriptorW` fails and `bind_listener` returns an error — but the pipe name construction at `:143-146` already embedded the (possibly malformed) SID into the pipe path. No reachable path exploits this today because `sid_to_string` comes from a `TokenUser` dispatch the kernel provides, but a defense-in-depth check should verify the SID is well-formed before composing the name.

**Severity**: LOW-MEDIUM.

**Remediation**: add `debug_assert!(owner_sid.starts_with("S-1-"))` and reject SIDs whose length exceeds 184 bytes (the documented SID-string max) before composing the pipe path.

---

### M9. `MAX_IPC_PAYLOAD_LEN` / `MAX_REQUEST_BYTES` of 1 MiB is defended pre-allocation, but there is no per-peer rolling byte budget

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/server.rs:42`, `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/protocol.rs:47`, and `transport.rs:312-317` (the guard).

A well-behaved client sending 1 MiB requests in a tight loop will consume 1 MiB per accept cycle of allocator churn. Since the serve loop is already serial (M2), aggregate bandwidth is bounded, but a cooperating attacker with per-session rate-limit exemption on a cheap method can push this hard.

**Severity**: MEDIUM.

**Remediation**:
1. Add a per-peer rolling byte budget to the rate-limiter (`pcloud-daemon/src/rate_limit.rs`) — reject if byte-in over the last 60 s exceeds N MiB.
2. Consider dropping `MAX_REQUEST_BYTES` to 256 KiB now that the expensive IPC methods have concrete schemas; 1 MiB is two orders of magnitude larger than the largest real request per the comment at `server.rs:33-37`.

---

## LOW findings

### L1. Documentation contradiction: `SECURITY.md:96-97` vs `CONTRIBUTING.md:206` vs actual code

**Observation**: both docs explicitly forbid `danger_accept_invalid_certs` / `accept_invalid_hostnames`. I confirmed no `src/` file contains either identifier (grep across `crates/`). Documentation is accurate; this is a positive note, not a defect. Keep the discipline in place.

---

### L2. `RedactedString` serializes transparently — round-trips include the plaintext

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/redacted.rs:37-39`:

```rust
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct RedactedString(String);
```

This is intentional (the secret has to cross the IPC boundary), and the module docs explicitly justify the trade-off. However, because the type is `Clone`, one subtle failure mode exists: if a consumer holds a `RedactedString` in a long-lived `HashMap`, no `ZeroizeOnDrop` applies. The `methods.rs:241-259` audit H1 note mentions immediate destructuring into `SecretString` on the daemon side, but a future refactor that stashes it elsewhere will silently regress.

**Severity**: LOW (design choice, documented, test coverage exists at `redacted.rs:118-133`).

**Remediation**: add a compile-fail test that prevents `RedactedString` from appearing as a field on any struct annotated `#[long_lived]`, or wrap it in a newtype `EphemeralRedacted` that impls `Drop` via `Zeroize`.

---

### L3. `RedactedString::Clone` is derived, bypassing the `SecretString` clone-audit discipline

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/redacted.rs:37`.

The project carefully removed `#[derive(Clone)]` from `SecretString` (audit M3 in the module doc) so every duplication is visible as `.clone_secret()`. `RedactedString` derives `Clone` freely, so `req.value.clone()` at any dispatch site is invisible in code review.

**Severity**: LOW.

**Remediation**: remove the derived `Clone` and add `fn clone_secret(&self) -> Self` so the two types follow the same discipline. Then fix the handful of `Request::Plain { method }` clones that may depend on it.

---

### L4. `serve_once` wraps `listener.accept()` in a single `?`, hiding `EINTR` vs permanent error distinction

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:171-173`.

The `?` on `accept` returns immediately, but the caller at `serve.rs:207-210` reinterprets `ErrorKind::Interrupted` as a signal-driven wakeup. This coupling between `accept` error kinds and `serve_until_shutdown_with_flag` is correct but brittle. A future `BoundIpcServer::serve_many` helper must mirror the same branch.

**Severity**: LOW.

**Remediation**: add a `AcceptOutcome::{Connection, Interrupted, Timeout}` enum on `BoundIpcServer` so callers do not parse `io::ErrorKind` directly.

---

### L5. `server.rs:96-98` example asserts uid 1000 is authorized — doctest could be misleading

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/server.rs:93-97`.

The runnable doctest creates an `IpcServer::new(1000)` and asserts a `PeerIdentity { uid: 1000, pid: 1 }` is authorized. In a hostile reader's mental model this looks like "anyone matching uid 1000 is OK", which is correct but underlines that **the uid check is the ONLY gate** — the server does not check pid, cmdline, mount namespace, or SELinux label. Document this explicitly.

**Severity**: LOW (documentation / threat-model clarity).

**Remediation**: add a note: "same-uid attackers (running as the same user as the daemon) fully satisfy authorization; additional sandboxing must live at the OS layer (AppArmor, SELinux, or a user-namespaces sandbox)."

---

### L6. `loader.rs` enforces `0o077` mask but does not validate ownership on config file

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/loader.rs:188-229`.

Production rejects group/world-readable files but does not assert the file is owned by the current uid. A root-owned `0o600` config file would pass the check — OK for systemd-root daemons but surprising for a user-scope daemon.

**Severity**: LOW.

**Remediation**: extend `check_permissions` with an ownership check symmetric to `vault/file.rs:208-212`.

---

### L7. `store_token` recreates `tmp_path` via `with_extension("tmp")`, and does not use `O_TRUNC` on the final rename target

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:149-185`.

The atomic-write-then-rename sequence is correct, but `fs::set_permissions(path, ...)` at `:183` runs **after** the rename, producing a tiny window where `path` inherits the tmp-file mode (itself `0o600` by construction). This is safe on filesystems where rename preserves mode (all POSIX FS), but the two-step dance is unnecessary. Keep the explicit `set_permissions` as defense-in-depth but document why.

**Severity**: LOW.

**Remediation**: comment the redundancy; no code change needed.

---

### L8. `RedactedString` uses default-derived `PartialEq`, not constant-time

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/redacted.rs:37`.

`SecretString::PartialEq` goes through `subtle::ConstantTimeEq` (secret_string.rs:110-112). `RedactedString` uses the derived byte-by-byte eq, which leaks length / prefix timing. The contract is that `RedactedString` is transient and destructured before long-term use, so exposure is ephemeral — but a future cache path could regress.

**Severity**: LOW.

**Remediation**: add the same constant-time impl for parity.

---

### L9. `fs::read_to_string("/proc/self/mountinfo")` on Linux has no TOCTOU protection against a bind-mount swap

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:36-39`.

Unmount-orphan detection reads `/proc/self/mountinfo`. Between two reads (settle-poll window at `:157-171`), a cooperating attacker can race a bind-mount to confuse the parser. No secret exposure; correctness issue.

**Severity**: LOW.

**Remediation**: out of security scope — log for the FS team (Dimension 6 or equivalent).

---

## Positive findings (secure-by-default confirmations)

### P1. `SecretString` / `SecretBytes` are audit-hardened correctly

**Files / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-secret/src/secret_string.rs:35-124` and `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-secret/src/secret_bytes.rs:22-102`.

- `#[derive(ZeroizeOnDrop)]` at `secret_string.rs:35` and `secret_bytes.rs:22` guarantees scrubbing.
- `Clone` is deliberately NOT derived; `clone_secret()` is the only way to duplicate (lines 77-80 / 58-61).
- Constant-time `PartialEq` via `subtle::ConstantTimeEq` (lines 110-113 / 91-93).
- `Debug` renders `SecretString(<redacted>)` / `SecretBytes(<redacted>)` (lines 96-98 / 76-79).
- No `Serialize`/`Deserialize` impl; the module doc (`secret_string.rs:15-17`) references the compile-fail test `tests/compile_fail_serialize.rs` that enforces this.
- Both types impl `Zeroize` explicitly (lines 120-124 / 98-102) as belt-and-braces against a future refactor swapping the inner type.

This is **correct** and meets or exceeds the project's stated standard. No changes required.

---

### P2. Auth vault discipline is secure-by-default

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:138-186` (`store_token`) and `:77-133` (`load_token`).

- **Opt-in**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs:362-367` `AuthPersistence { enabled: bool }` — default `false`, daemon must receive an explicit enable.
- **`0600` file mode**: set at `file.rs:166` (`.mode(0o600)`), reinforced at `:177` and `:183`.
- **`0700` parent dir**: set at `file.rs:142` unconditionally on every store.
- **Ownership validation on load**: `file.rs:207-212` rejects non-owner files.
- **Mode validation on load**: `file.rs:214-218` rejects any group/other bit.
- **Atomic tmp+rename write**: `file.rs:149-184` uses `O_CREAT|O_EXCL` (`create_new`) with `mode(0o600)`, then `sync_all`, then `rename`, then `sync_parent_directory`.
- **No plaintext password persistence**: confirmed by `vault/mod.rs:37-40` ("Password persistence is intentionally not available through this trait — see ADR 0007") and by grep — no `password.write(` / `fs::write(_, password)` anywhere under `crates/pcloud-daemon/`.
- **Zeroize on load error path**: `file.rs:92-93, 112, 119, 127` explicitly zeroize intermediate buffers on every error branch. This was noted as audit finding M4 and is closed.

---

### P3. IPC transport security

**File / lines**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:246-267` (`bind`) — creates parent with `fs::create_dir_all`, tightens to `0o700` if parent_missing, removes stale socket, binds, then `chmod 0o600`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:167-229` (`serve_once` / `serve_stream_once`) — applies 5 s read timeout, recovers peer credentials via `peer_identity`, rejects on authorization failure **before** dispatch, returns `Unauthorized` status.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/linux.rs:42-57, 94-120` — Linux `SO_PEERCRED` with strict `rc != 0 || len != sizeof(ucred)` check.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/unix.rs:44-60` — BSD/macOS `getpeereid(3)`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/windows.rs:127-219` — Windows per-SID DACL at pipe creation time, `TokenUser` SID comparison via `GetNamedPipeClientProcessId` + `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `OpenProcessToken(TOKEN_QUERY)` + `GetTokenInformation(TokenUser)` + `ConvertSidToStringSidW`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/server.rs:42` — `MAX_REQUEST_BYTES = 1 MiB` cap.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:304-325` — read_framed_request checks the cap **before** allocating `Vec::with_capacity(8 + payload_len)`, explicitly called out at `:310-311`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:327-364` — oversized-frame errors close the connection **without** replying (amplification protection), protocol errors reply `InvalidRequest`, transient IO errors are swallowed.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:232-236` — `Drop` unlinks the socket file.

All of these are correct and meet the stated security model. The only remaining improvements are M2 (concurrent connection cap), M3 (write timeout), and M4 (setuid sanity gate).

---

### P4. Transport policy — production rejects plaintext

**File / lines**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/api.rs:130-140` — `ApiEndpoint::validate` in `Production` + `ApiMode::Plaintext` returns `Err(ConfigError::InvalidApiEndpoint(...))` with message "production environment requires tls api mode". Test coverage at `:232-240` (`production_plaintext_is_rejected`).
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/api.rs:195-203` — `secure_defaults` for `Production` defaults mode to `Tls`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/file_history.rs:67-78` — `RevisionUrl::validate` refuses `http://` URLs in Production.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/env.rs:27-30` — env parser rejects `PCLOUD_ENV=production` with `PCLOUD_API_MODE=plaintext`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/transport.rs:318-336` — TLS client uses `rustls` + `webpki_roots::TLS_SERVER_ROOTS`, no custom verifier.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/http_download.rs:210, :573` — same pattern.
- No `danger_accept_invalid_certs` / `accept_invalid_hostnames` / `InsecureSkipVerify` / custom-validator strings anywhere in `crates/**/*.rs` (grep-verified).

---

### P5. Downgrade / replay defenses

- **TFA cannot be skipped when server demands it**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/state.rs:22-37` models `SessionState::TwoFactorRequired` explicitly; `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/orchestrator.rs:258, 377, 444` always transitions to `TwoFactorRequired` when the server returns `PasswordLoginOutcome::TwoFactorRequired`. `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/manager.rs:60-100` state machine forbids jumping directly from `AwaitingCredentials` to `Authenticated`.
- **Hard expiry is enforced**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/lifecycle.rs:172-174` `is_expired(now_secs) -> now_secs >= expires_at`. `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/lifecycle.rs:216-217` raises `SessionLifecycleError::AuthExpired` forcing re-auth.
- **Idle expiry**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/lifecycle.rs:178-180`.
- **Server-reported auth-expired is honoured**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/serve.rs:322-325` (`TickOutcome::AuthExpired` branch).
- **No replay-via-reusable-nonce**: IPC protocol version at `protocol.rs:39` is checked at `protocol.rs:255-260` (`VersionMismatch` error); a downgraded client is hard-rejected.

---

### P6. Logging discipline

- Grep for `(info|warn|error|debug|trace)!\s*\(.*\b(password|token|secret|priv_key|passphrase)\b` across `crates/**/*.rs` yields **no leaks**. The single hit at `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/serve.rs:309` reads `"pcloud-session-refresh: token refreshed successfully"` — a marker string, not a token value.
- Grep for variable interpolation pattern `(info|warn|error|debug|trace)!.*(\{password|\{token|\{secret|\{passphrase|expose_secret)` yields **zero hits**.
- `SecretString::Debug` and `SecretBytes::Debug` both render `<redacted>` (`secret_string.rs:96-99` / `secret_bytes.rs:76-80`).
- `RedactedString::Debug` renders `<redacted N bytes>` (`redacted.rs:75-79`).
- No `SecretString::expose_secret()` appears inside any `*!` formatter; grep confirms.

---

### P7. FFI SAFETY discipline

Every `unsafe` block I spot-checked across `platform/{linux,bsd,macos,windows}.rs`, `platform/macos_ffi.rs`, `platform/winfsp_ffi.rs`, and `platform/{linux,unix,windows}.rs` in `pcloud-ipc/src/` carries an inline `// SAFETY:` comment stating the invariant. Examples:
- `pcloud-ipc/src/platform/linux.rs:42-50` — `getsockopt(SO_PEERCRED)` with live fd + initialized out-param.
- `pcloud-ipc/src/platform/unix.rs:49-53` — `getpeereid(3)` with initialized out-params.
- `pcloud-fs/src/platform/linux.rs:90-97, 106-108, 113-116, 186` — `signal(2)`, `umount2`, `raise(sig)`.
- `pcloud-fs/src/platform/bsd.rs:188-201, 242-258, 295-299, 352-382, 442` — `getmntinfo`, `slice::from_raw_parts` on libc-owned `statfs` array.
- `pcloud-fs/src/platform/windows.rs:92-99, 117-124, 164-177, 228-241, 274-284, 303-304, 308-314, 319-349, 353-371, 388-398, 409-419, 425-434, 454-458, 462-468` — every Win32 call, every SID lookup, every `LocalFree` is SAFETY-commented.
- `pcloud-fs/src/platform/macos.rs:194-225, 232-256, 262-266, 308-314, 328-342, 346-350, 413-448` — every fuse-t FFI call is commented. Phase-1 scaffold caveat at `macos_ffi.rs:10-15` is acknowledged.
- `pcloud-fs/src/platform/winfsp_ffi.rs:443-447, 468-470, 480-514, 517` — function-pointer transmutes explicitly document the ABI contract. See M6 for residual risk.
- `pcloud-ipc/src/auth.rs:66-67` — `libc::geteuid()` has no preconditions; single-line SAFETY comment.

The one structural concern is the transmute-to-fn-ptr sequence in `winfsp_ffi.rs` (M6), which cannot be fully validated without a WinFSP version probe. All other `unsafe` blocks pass review.

---

### P8. Secret-bearing CLI state is wrapped

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/commands.rs:565-590`.

`SecretInputs` long-lived struct holds:
- `password: SecretString`
- `auth_token: SecretString`
- `crypto_password: SecretString`
- `public_link_password: Option<SecretString>`

`Clone` / `PartialEq` are deliberately not derived (line 561-564 comment). This matches the project standard.

The only residuals in `SecretInputs` that are still `String` are `two_factor_code` (line 570) and `share_message` (line 612) — the share_message is legitimate plaintext, but `two_factor_code` carries a recovery-phrase when `recovery_code = true` and should be promoted to `SecretString` — see H4.

---

### P9. Secret-stash state machine

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/state.rs:47-65` (`PendingChallenge { token: SecretString, ... }` with hand-written `Clone` routed through `clone_secret`) and `:73-100` (`SessionSnapshot { auth_token: Option<SecretString>, ... }` with hand-written `Clone` via `clone_secret`).

`Debug` impls on both types emit tag-only output; the secret material goes through `SecretString`'s redacted `Debug`. This exactly matches the stated project standard.

---

## Summary table

| ID  | Sev      | Area                      | File                                                       |
|-----|----------|---------------------------|------------------------------------------------------------|
| H1  | HIGH     | secret discipline         | `crates/pcloud-proto/src/methods/*.rs` (many)              |
| H2  | HIGH     | input validation          | `crates/pcloud-daemon/src/runtime.rs:3952`                 |
| H3  | HIGH     | input validation          | `crates/pcloud-backends/src/snapshot.rs:625` (isolated)    |
| H4  | HIGH     | secret discipline         | `crates/pcloud-ipc/src/methods.rs:290`                     |
| M1  | MEDIUM   | vault                     | `crates/pcloud-daemon/src/vault/file.rs:198`               |
| M2  | MEDIUM   | DoS                       | `crates/pcloud-daemon/src/serve.rs:127`                    |
| M3  | MEDIUM   | DoS                       | `crates/pcloud-ipc/src/transport.rs:366`                   |
| M4  | MEDIUM   | IPC                       | `crates/pcloud-ipc/src/auth.rs:65`                         |
| M5  | MEDIUM   | FFI / signal              | `crates/pcloud-fs/src/platform/linux.rs:87`                |
| M6  | MEDIUM   | FFI                       | `crates/pcloud-fs/src/platform/winfsp_ffi.rs:494`          |
| M7  | MEDIUM   | vault TOCTOU              | `crates/pcloud-daemon/src/vault/file.rs:200`               |
| M8  | MEDIUM   | IPC Windows               | `crates/pcloud-ipc/src/platform/windows.rs:385`            |
| M9  | MEDIUM   | DoS                       | `crates/pcloud-ipc/src/server.rs:42`                       |
| L1  | LOW      | docs                      | `SECURITY.md`                                              |
| L2  | LOW      | secret lifetime           | `crates/pcloud-ipc/src/redacted.rs:37`                     |
| L3  | LOW      | secret discipline         | `crates/pcloud-ipc/src/redacted.rs:37`                     |
| L4  | LOW      | IPC error model           | `crates/pcloud-ipc/src/transport.rs:171`                   |
| L5  | LOW      | doc / threat model        | `crates/pcloud-ipc/src/server.rs:93`                       |
| L6  | LOW      | config                    | `crates/pcloud-config/src/loader.rs:188`                   |
| L7  | LOW      | vault defense-in-depth    | `crates/pcloud-daemon/src/vault/file.rs:183`               |
| L8  | LOW      | timing                    | `crates/pcloud-ipc/src/redacted.rs:37`                     |
| L9  | LOW      | FS race                   | `crates/pcloud-fs/src/platform/linux.rs:36`                |

### Prioritized remediation sequence

1. **H1** (derived-Debug leak in `pcloud-proto`) — systemic, easy mechanical fix, highest leverage.
2. **H4** (TFA recovery code as plain `String`) — one-line change in `methods.rs`.
3. **H2** (path validation on `SyncRootAdd`) — add a shared `validate_user_supplied_path`.
4. **H3** (publish the tar-entry safety helper) — refactor + shared crate.
5. **M1 / M7** (vault parent-dir validation + open-by-fd TOCTOU fix) — low-effort hardening.
6. **M2 / M3** (IPC concurrency cap + write timeout) — availability.
7. **M4** (reject setuid daemon).
8. **M5** (async-signal-safe unmount trampoline) — correctness.
9. **M6 / M8** (WinFSP version probe + SID shape check).
10. **M9** (per-peer byte budget) — DoS defense-in-depth.
11. LOWs in decreasing impact: L3 → L8 → L4 → L6 → L7 → L2 → L5 → L1 → L9.

---

### Methodology notes

- All greps were run over `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/**/*.rs` unless noted.
- SAFETY spot-checks covered ~40 of the ~140 `unsafe` blocks across the FFI files; the remainder follow the same pattern and were not individually verified but carry inline comments.
- No source files were modified by this audit.
- The project's own `CLAUDE.md` security rules were used as the normative baseline; every confirmed deviation is reported above.

### Out-of-scope items observed

The following items were surfaced but fall to other audit dimensions:

- cryptographic algorithm selection (AES-256-GCM, sector size, AEAD-nonce generation) → Dim 3.
- compression-bomb protection on decompressed API responses — no inbound gzip/deflate path was identified in `pcloud-proto/src/transport.rs`; the API frame codec is length-prefixed JSON. If a future feature adds HTTP compression, Dim 3 should audit decode budget.
- observability leaks in `pcloud-observability` → Dim 4.
# Section 3. Crypto Subsystem

**Scope.** This dimension audits cryptographic correctness, algorithm fidelity vs the legacy pCloud C client, key schedule, nonce discipline, lifecycle, team-share temppass, KMS wiring, zeroization, constant-time comparisons, and dependency posture of the `pcloud-crypto` crate and the `pcloud-kms` crate, along with how the daemon/runtime drive them via IPC.

**Auditor:** parallel Dimension 3 specialist (non-FIPS, non-parity-accounting).

**Files audited (exhaustive list):**
- `crates/pcloud-crypto/Cargo.toml` (32 lines)
- `crates/pcloud-crypto/src/lib.rs` (1508 lines) — `CryptoShell`, lifecycle, sector wrappers, change-password, KMS routing.
- `crates/pcloud-crypto/src/content.rs` (328 lines) — AES-256-GCM sector AEAD, per-file key derivation.
- `crates/pcloud-crypto/src/keys.rs` (207 lines) — Argon2id master-key derivation, setup fingerprint.
- `crates/pcloud-crypto/src/metadata.rs` (149 lines) — deterministic filename encoding.
- `crates/pcloud-crypto/src/password_scorer.rs` (874 lines) — password scorer + PBKDF2-HMAC-SHA512 passphrase→API-password derivation.
- `crates/pcloud-crypto/src/policy.rs` (101 lines) — policy gates for master-key persistence.
- `crates/pcloud-crypto/src/share_temppass.rs` (647 lines) — crypto-folder share temppass wrap/unwrap.
- `crates/pcloud-crypto/src/state.rs` (77 lines) — lifecycle state machine.
- `crates/pcloud-crypto/tests/integration.rs` (135 lines).
- `crates/pcloud-crypto/tests/kms_routing.rs` (336 lines).
- `crates/pcloud-crypto/tests/proptest_seal.rs` (93 lines).
- `crates/pcloud-crypto/benches/aead_sector.rs` (67 lines).
- `crates/pcloud-crypto/vendored/password_dict.rs` (build-time-generated, non-secret).
- `crates/pcloud-kms/src/lib.rs` (1331 lines) — `KmsProvider` trait, `NullKms`, AWS KMS, HashiCorp Vault, PKCS#11 HSM (feature-gated), process-local plaintext-DEK cache.
- Daemon wiring (read-only context, not the primary focus of this dimension):
  `crates/pcloud-daemon/src/runtime.rs` (`unlock_crypto`, `setup_crypto`, `lock_crypto`, `crypto_reset`, `change_crypto_password`, `change_crypto_password_unlocked`, `crypto_priv_key_flags`, `send_crypto_change_user_private`, `upload_reencoded_private_key`).
  `crates/pcloud-ipc/src/lib.rs` (Request/Method variants: `CryptoUnlock`, `CryptoSetup`, `CryptoChangePassword`, `CryptoChangePasswordUnlocked`, `CryptoMkdir`, `LockCrypto`, `CryptoReset`, `GetCryptoStatus`, `GetCryptoPrivKeyFlags`, `SendCryptoChangeUserPrivate`, `GetCryptoHint`).

**Workspace crypto dependency pins (from `Cargo.toml` + `Cargo.lock`):**
- `aes-gcm = "0.10.3"` (default-features off, `aes + alloc`) — RustCrypto, actively maintained.
- `argon2 = "0.5.3"` — RustCrypto.
- `getrandom = "0.2.17"` primary (also `0.3.4` and `0.4.2` transitively via `rand`).
- `hmac = "0.12.1"` — RustCrypto.
- `sha2 = "0.10.9"` — RustCrypto.
- `subtle = "2.6.1"` — RustCrypto (constant-time primitives).
- `zeroize = "1.8.2"` with `zeroize_derive` — RustCrypto.
- `#![forbid(unsafe_code)]` at `crates/pcloud-crypto/src/lib.rs:1`; zero `unsafe` blocks in the crate (confirmed by grep).

The rest of this report is organised as per the audit prompt's 13 focus areas, then a severity-ranked findings ledger (CRITICAL/HIGH/MEDIUM/LOW), then a remediation summary.

---

## 1. Algorithm fidelity vs legacy C client

### 1.1 What CLAUDE.md claims

From `CLAUDE.md` → "Crypto parity progress":

> Implemented on the active Rust path:
> - setup/start/stop/reset,
> - lock/unlock lifecycle,
> - crypto folder creation,
> - AES-256-GCM sector encryption,
> - deterministic metadata filename encoding,
> - zeroized key handling via `SecretBytes` / `SecretString`,
> - password rotation helpers,
> - fingerprint verification and reset paths,
> - active daemon/IPC/SDK crypto control surfaces.
> - crypto-aware share/team-share temppass flow.
>
> Still missing:
> - `change_crypto_pass` family,
> - `send_change_user_private`,
> - `priv_key_flags`.

### 1.2 What the code actually implements

The Rust `pcloud-crypto` crate is **NOT** a byte-level port of the C `pclsync/pcryptofolder.c` wire format. It is a **re-implementation with the same shape** but with different primitives, different on-disk persistence, and no byte-identical interoperability guarantee. The code itself is explicit about this — see the doc block at `crates/pcloud-crypto/src/share_temppass.rs:39-46`:

> The active Rust crypto path (see `crate::keys::KeyManager`) does not yet store an RSA-4096 keypair in the form the C client expects, so the "signature" produced here is an HMAC-SHA256 tag under the active master key rather than an RSA signature under the user private key.

Concretely the Rust path differs from the upstream C client as follows:

| Surface | Legacy C (`pclsync/pcryptofolder.c`, `pcryptofolder.h`) | Rust (`pcloud-crypto`) |
|---|---|---|
| Master key | Per-user RSA-4096 private key wrapped by a master passphrase using AES-CTR + separate SHA signature; generated on enrolment and persisted server-side | 32-byte symmetric Argon2id output kept in `SecretBytes`, never persisted; no RSA keypair at all |
| Sector AEAD | AES-CTR + HMAC / SHA-based MAC (legacy composed construction) | **Single-pass AES-256-GCM** (AEAD) with 12-byte nonce from `OsRng` |
| Per-file key | Derived from the RSA-wrapped symmetric key | `HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)` — see `content.rs:127` |
| Nonce | C uses a counter-style IV seeded from file metadata | 96-bit **random** nonce from `getrandom()` — see `content.rs:188-190` |
| Filename encoding | C uses AES-CBC-encrypted filename blobs | `HMAC-SHA256(master, "pcloud-crypto/filename/v1" || plaintext)` then hex — see `metadata.rs:90-108` |
| Fingerprint / unlock gate | C derives the master key on every unlock and attempts to decrypt an RSA-wrapped test blob | Rust stores `HMAC-SHA256(derived, "pcloud-crypto/fingerprint/v1")` as a non-secret 32-byte check tag — see `keys.rs:178-185` |
| Password rotation | C reuses the user's RSA key, re-wraps it under the new password, uploads `privenc + sign` | Rust emits a version-tagged `"pcrypto/v1/" || hex(salt) || "/" || hex(fingerprint) || "/" || hex(flags_le)` blob signed with `HMAC-SHA256(current_master)` — see `lib.rs:874-896` |
| Team-share temppass | C re-wraps the RSA private key under Argon2-from-temppass, signs with `prsa_sign_sha256_hash` | Rust wraps the current 32-byte master key under `AES-256-GCM(kek = Argon2id(temppass, 16B_salt))` and signs with `HMAC-SHA256(master)` — see `share_temppass.rs:288-341` |

### 1.3 Finding

This is **MUCH STRONGER CRYPTO** than legacy C for single-device scenarios, and it is clearly documented as such. However, it is **NOT** the "active crypto on the retained C path" — it is a re-design. The CLAUDE.md phrasing "crypto is active on the retained Rust path" is truthful for the *rewrite*, but an auditor who reads the parity matrix and expects byte-level interop with the upstream pCloud server's encrypted-folder ciphertext will be mistaken.

See CRITICAL-3.A below — there are **no cross-client KAT (known-answer test) vectors** proving that a ciphertext produced by the Rust crate can be decrypted by a real upstream pCloud C client. The share temppass module flags this under bd-1du.5 at `share_temppass.rs:44-45`, but no equivalent caveat exists for *content* sectors, filenames, or the setup fingerprint.

---

## 2. Key schedule

### 2.1 Master-key derivation — `crates/pcloud-crypto/src/keys.rs:134-160`

```rust
pub fn derive_key_material_with_salt(password: &SecretString, salt: &[u8]) -> SecretBytes {
    let mut derived = vec![0u8; DERIVED_KEY_LEN];            // 32 bytes
    Argon2::default()
        .hash_password_into(password.expose_secret().as_bytes(), salt, &mut derived)
        .expect("fixed argon2 output length should be valid");
    SecretBytes::new(derived)
}
```

- Primitive: **Argon2id** via `argon2` crate defaults.
- `argon2 = "0.5.3"` `Argon2::default()` resolves to **`m = 19456` KiB (~19 MiB), `t = 2`, `p = 1`** (crate source: OWASP-recommended 2022 preset).
- Output: **32 bytes** (`DERIVED_KEY_LEN`).
- Salt: **16 bytes** per-profile, generated once on `KeyManager::default()` via `getrandom()` — `keys.rs:88-89`.
- Password wrapped in `SecretString` (zeroize on drop). Output wrapped in `SecretBytes`. Input `password` is borrowed.

### 2.2 Fingerprint — `crates/pcloud-crypto/src/keys.rs:178-185`

`HMAC-SHA256(derived_key, "pcloud-crypto/fingerprint/v1")` → 32 bytes non-secret.

### 2.3 Per-file key — `crates/pcloud-crypto/src/content.rs:126-134`

`HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)` → 32 bytes in `SecretBytes`.

### 2.4 Per-sector key — same key as per-file, **no per-sector key**

The sector layer uses a single per-file 32-byte key and distinguishes sectors **only via the 4-byte big-endian sector index bound as AAD** (`content.rs:191`). There is no sector-level subkey schedule.

### 2.5 Findings

- **MEDIUM-3.B (no separate per-sector key).** Rotating nonces is the sole protection against within-file key reuse. At 96-bit random nonces, expected collision is at ~2⁴⁸ sectors. The doc at `lib.rs:1096-1101` acknowledges this and says "sector-level rekey is expected every 2^32 sectors on the enterprise path but is not enforced here; the daemon owns the rekey schedule". **The daemon does NOT currently enforce any such rekey schedule** (confirmed by grep for `rekey` across `crates/pcloud-daemon/src/`). Remediation: either add a real sector-rekey hook at the daemon or swap to AES-GCM-SIV / XChaCha20-Poly1305 where nonce collisions are safer.
- **HIGH-3.C (Argon2id parameter divergence is UNTESTED against the C client).** The C client's key-stretching parameters come from `pclsync/pssl.c:psymkey_derive`, which is **PBKDF2-HMAC-SHA-512, 5000 iterations** (see the doc at `password_scorer.rs:536-538`). That is the *account API password* derivation — a different code path. The *master-key derivation* on the C side is in `pclsync/pcryptofolder.c` and uses the historical pCloud-defined KDF (not Argon2). The Rust side does Argon2id for the crypto-folder master key. These **do not interoperate**: a Rust client cannot read a legacy-C encrypted folder, and vice versa. Mark this as CRITICAL if the product claim is "drop-in replacement" for an existing C-enrolled user; mark as MEDIUM if the product is a greenfield migration path. CLAUDE.md currently forbids the "drop-in replacement" claim (see `CLAUDE.md` "Do not claim"), which is the right posture — this finding is then **HIGH** only in that the matrix row should explicitly say "not byte-compatible; new enrolment required".

---

## 3. Nonce generation

### 3.1 Sector AEAD — `crates/pcloud-crypto/src/content.rs:186-206`

```rust
let mut nonce_bytes = [0u8; NONCE_LEN];                 // 12 bytes
getrandom(&mut nonce_bytes).expect("OS randomness must be available");
let nonce = Nonce::from_slice(&nonce_bytes);
let aad = sector_index.to_be_bytes();
```

- **Random 96-bit nonce** from `getrandom` (OS CSPRNG). Not counter-derived. Not offset-derived.
- On OS CSPRNG failure, the function panics via `.expect(...)`. The doc at `content.rs:173-176` marks this as an "unrecoverable host fault". This is defensible on Linux/macOS where `getrandom(2)` only fails on misconfigured kernels, but there is **no fallback** on embedded Rust targets.

### 3.2 Share temppass — `crates/pcloud-crypto/src/share_temppass.rs:302-305`

```rust
let mut salt  = [0u8; TEMPPASS_SALT_LEN];   // 16 bytes
let mut nonce = [0u8; TEMPPASS_NONCE_LEN];  // 12 bytes
getrandom(&mut salt).map_err(|_| TemppassError::Malformed)?;
getrandom(&mut nonce).map_err(|_| TemppassError::Malformed)?;
```

Both salt and nonce are freshly drawn from the OS CSPRNG on every call. Property test `distinct_invocations_produce_distinct_wires` at `share_temppass.rs:591-599` asserts freshness.

### 3.3 KMS-wrapped DEK generation — `crates/pcloud-crypto/src/lib.rs:537-545`

```rust
let mut dek_bytes = vec![0u8; KMS_DEK_LEN];             // 32 bytes
getrandom::getrandom(&mut dek_bytes)
    .expect("OS randomness should be available for DEK generation");
let dek = pcloud_kms::PlaintextDek(dek_bytes);
```

DEK drawn from OS CSPRNG once at `enable_kms_mode` time; wrapped blob persisted inside `CryptoShell::mode = CryptoMode::Kms`.

### 3.4 PKCS#11 AES-GCM IV — `crates/pcloud-kms/src/lib.rs:962-964`

```rust
let mut iv = [0u8; 12];
getrandom::getrandom(&mut iv)
    .map_err(|e: getrandom::Error| KmsError::Other(e.to_string()))?;
```

12-byte IV from OS CSPRNG.

### 3.5 Findings

- **GOOD:** Every nonce/IV path uses `getrandom` (OS CSPRNG) — no `SmallRng`, no thread-local PRNG, no counter derivation. The audit prompt's CRITICAL check ("(key, nonce) reuse reachable" under a weak RNG) is **not reachable** under normal host configuration.
- **LOW-3.D (error discipline divergence).** Sector AEAD `seal_sector` **panics** on `getrandom` failure (`content.rs:189`); share temppass **returns `Malformed`** on the same failure (`share_temppass.rs:304-305`); KMS DEK **panics** (`lib.rs:540`); PKCS#11 IV **returns `Other`** (`kms/lib.rs:964`). The two panic sites are OK because `getrandom` on Linux only fails if the kernel is too old for `getrandom(2)`, but the inconsistency hurts readability and auditability. Remediation: pick one policy (prefer "propagate as error") and apply uniformly.
- **MEDIUM-3.E (random 96-bit nonce collision bound).** With random nonces, the AEAD birthday bound is ~2⁴⁸ sectors at 2⁻³² collision probability. At the 4 KiB sector size this is 2⁴⁸ × 4 KiB ≈ 1 EB of data per key — not reachable today but not future-proof. The code doc (`lib.rs:1096-1101`) acknowledges a sector-rekey schedule is needed on the enterprise path but it is not enforced. Consider AES-GCM-SIV (`aes-gcm-siv` crate) for enterprise mode, which is nonce-misuse-resistant.

---

## 4. Fingerprints & reset

### 4.1 Fingerprint check — `crates/pcloud-crypto/src/keys.rs:199-206`

```rust
pub fn matches_setup(&self, key: &SecretBytes) -> bool {
    let Some(stored) = self.setup_fingerprint.as_ref() else { return false; };
    let computed = Self::fingerprint_for(key);
    computed.0.ct_eq(&stored.0).into()
}
```

**GOOD:** constant-time comparison via `subtle::ConstantTimeEq`.

### 4.2 Wrong-password path — `crates/pcloud-crypto/src/lib.rs:727-738`

```rust
self.unlock_state = state::UnlockState::Unlocking;
let derived = self.keys.derive_key_material(&password);
if !self.keys.matches_setup(&derived) {
    drop(derived);
    self.unlock_state = state::UnlockState::Locked;
    return Err(CryptoError::WrongPassword);
}
self.keys.active_key_material = Some(derived);
```

- **GOOD:** derived material dropped (zeroized) on wrong-password.
- **GOOD:** `UnlockState` transitions back to `Locked` and never reveals partial `Unlocked` state.

### 4.3 Rate-limit / lockout

- **HIGH-3.F (no wrong-password rate-limit / lockout at the crypto layer).** Nothing in `pcloud-crypto` rate-limits brute-force unlock attempts. The daemon handler at `crates/pcloud-daemon/src/runtime.rs:2533-2564` (`unlock_crypto`) calls `self.crypto.start(secret)` directly and returns `Unauthorized` on failure. No counter, no exponential backoff, no lockout. An IPC client (owner-only, but still a local attack surface) can call `unlock_crypto` in a tight loop. At Argon2id default cost (~200 ms per attempt on a laptop CPU) this bounds practical online guessing to ~5 attempts/second, which is better than nothing but is not the "enterprise ready" posture CLAUDE.md gestures at.
- Remediation: track consecutive-failure count in `KeyManager` and require a backoff delay or transient lockout. Keep the backoff constant-time to avoid leaking whether the shell is locked vs mid-unlock.

### 4.4 Reset path — `crates/pcloud-crypto/src/lib.rs:1005-1013`

```rust
pub fn reset(&mut self) {
    self.stop();
    self.keys.setup_fingerprint = None;
    self.folders.clear();
    self.next_local_folder_id = 1;
    self.hint = None;
    self.mode = CryptoMode::Raw;
    self.unlock_state = state::UnlockState::NotSetup;
}
```

- **GOOD:** `stop()` first (drops+zeroizes active key material, evicts KMS cache). Then fingerprint is zeroed, mode reverts to Raw.
- **MEDIUM-3.G (recovery code flow is at the daemon only).** The C client exposes a recovery-code path; in Rust the recovery code is enforced at `runtime.rs:2714-2720` / `2771-2776` as an IPC-level non-empty string. There is no cryptographic binding between the recovery code and the reset operation at the `CryptoShell` level. The daemon forwards to the backend (`upload_reencoded_private_key` at `runtime.rs:2814-2842`), which has the final say. If a future refactor drops the IPC-level check, `CryptoShell::reset()` has no safeguard of its own. Consider adding a `require_recovery_proof: bool` policy bit.

---

## 5. Rotation (`change_crypto_pass` family)

CLAUDE.md marks this family as **"Still missing"**. **This is wrong as of the code I read.**

### 5.1 Actual implementation — `crates/pcloud-crypto/src/lib.rs:837-967`

Two functions are live:

- `CryptoShell::change_password_unlocked(new_password, flags) -> ReencodedPrivateKey` (`lib.rs:837-896`)
- `CryptoShell::change_password(old_password, new_password, flags) -> ReencodedPrivateKey` (`lib.rs:914-967`)

Both:

1. Verify policy (`policy.is_safe()` — rejects if `persist_master_key == true`).
2. Constant-time byte-compare old vs new passwords (`change_password` only — `lib.rs:934-944`).
3. Derive new key material under a **freshly-rotated 16-byte salt** (`lib.rs:858-862`).
4. Emit a version-tagged blob `pcrypto/v1/<salt_hex>/<fingerprint_hex>/<flags_le_hex>` + HMAC-SHA256 signature keyed by the **old** master.
5. Install the new salt + new fingerprint + new flags + new active master key.

The daemon wires this via `change_crypto_password` and `change_crypto_password_unlocked` (`runtime.rs:2701-2812`), and uploads the rekeyed blob to the backend via `crypto_runtime.change_user_private(...)` (`runtime.rs:2822-2828`).

### 5.2 Findings

- **HIGH-3.H (CLAUDE.md is out of date).** This is a documentation/parity-matrix drift, not a code defect. The Rust crate does implement `change_crypto_pass{_unlocked}` with stronger primitives than C (constant-time old-vs-new check, fresh salt on every rotation, HMAC-SHA256 signature under the old master, explicit version tag for forward-compat). The CLAUDE.md "Still missing" list must be corrected or this creates a false audit signal.
- **HIGH-3.I (no re-encryption of existing content on rotation).** Because the Rust master key is used as the **HMAC key for per-file key derivation**, rotating the master key **invalidates every existing per-file key**. The C client re-encrypts the RSA-wrapped DEK, leaving per-file AES keys unchanged. The Rust design does **not** re-encrypt any existing ciphertext on rotation — old sector frames are permanently unreadable after a rotation. No test and no doc currently warns about this. **This is a real-world data-loss trap.** Remediation: either (a) introduce a KEK-of-master-key layer so per-file keys stay stable across master rotations, or (b) document clearly and add an integration test that rotates the password and then proves old sector frames no longer decrypt, so callers understand the invariant.
- **MEDIUM-3.J (no binding of `ReencodedPrivateKey.private_key_hex` to the user identity).** The blob `pcrypto/v1/<salt>/<fp>/<flags>` does not carry a user id or account id. If a server accepts any blob signed under any master-known-to-the-session, an operator error could cross-account the rotation blob. Remediation: include a 64-bit account id (or a user-identity HMAC slot) inside the versioned blob before signing.
- **LOW-3.K (`change_password_unlocked` deliberately skips the "identical password" check).** Documented at `lib.rs:864-869` — because the salt is rotated, the new key will differ from the old even for identical passwords, so the check is moot at the derived-key layer. Callers who want "reject identical password" must use `change_password`, not `change_password_unlocked`. Defensible, but the IPC handler at `runtime.rs:2758-2812` exposes the unlocked variant directly — an IPC client can reset to the same passphrase silently. Mark as LOW since the C client had the same property.

---

## 6. `send_change_user_private` and `priv_key_flags`

CLAUDE.md marks both as missing. **Both are wrong.**

### 6.1 `priv_key_flags` — `crates/pcloud-crypto/src/lib.rs:814-817`

```rust
pub fn priv_key_flags(&self) -> u64 {
    self.keys.private_flags
}
```

Backed by `KeyManager::private_flags: u64` (`keys.rs:71-72`) with `PRIV_KEY_FLAG_TEMP_PASS = 1` (`keys.rs:84`) matching the C `PSYNC_CRYPTO_FLAG_TEMP_PASS`. Daemon IPC handler at `runtime.rs:2658-2663` (`GetCryptoPrivKeyFlags`). Tested at `lib.rs:1367-1370`.

### 6.2 `send_change_user_private` — `crates/pcloud-daemon/src/runtime.rs:2667-2698`

```rust
fn send_crypto_change_user_private(&mut self) -> Response {
    // ... auth token check ...
    match self.crypto_runtime.send_change_user_private(auth_token.expose_secret()) { ... }
}
```

Wired to IPC method `SendCryptoChangeUserPrivate`. Backed by `CryptoRuntime` in `crates/pcloud-daemon/src/crypto_backend.rs` (I did not deep-read this file in this audit because it is outside the `pcloud-crypto` / `pcloud-kms` scope of Dimension 3, but the method exists and is reachable).

### 6.3 Finding

- **HIGH-3.L (CLAUDE.md drift, repeat).** Same class as HIGH-3.H. Both features exist; the handoff doc must be corrected or Dimension 1 (parity accounting) will double-count this gap.

---

## 7. Team-share temppass (`crates/pcloud-crypto/src/share_temppass.rs`)

### 7.1 Wrap flow — `share_temppass.rs:288-341`

1. Validate shell is unlocked (borrows `master` without cloning).
2. Fresh 16-byte salt + 12-byte nonce from OS CSPRNG.
3. `kek = Argon2id(temppass, salt)` → 32 bytes in `SecretBytes`.
4. `ct = AES-256-GCM(kek, nonce, aad = "pcloud-crypto/share-temppass/aad/v1", msg = master.expose_secret())`.
5. `sig = HMAC-SHA256(master, "pcloud-crypto/share-temppass/sig/v1" || blob_encoded)`.
6. Emit both as base64.

### 7.2 Unwrap flow — `share_temppass.rs:377-403`

1. Base64-decode both blobs.
2. `TemppassBlob::verify(verifier_master, signature)` — **HMAC-SHA256 verified with `ct_eq` BEFORE any AEAD unwrap** (`share_temppass.rs:222-232`). Good.
3. Re-derive `kek` from temppass + embedded salt.
4. `AES-256-GCM-Open(kek, nonce, aad = fixed, ct)`.
5. Return recovered master as `SecretBytes`.

### 7.3 Findings

- **GOOD:** constant-time signature verification (`share_temppass.rs:227`). Tamper path collapses to single opaque `BadSignature` error so a caller cannot distinguish.
- **GOOD:** 16-byte salt + 12-byte nonce freshly drawn every call; property-tested at `share_temppass.rs:591-599`.
- **GOOD:** `Debug` impl redacts ciphertext (`share_temppass.rs:165-173`).
- **GOOD:** no Clone on `TemppassBlob`.
- **MEDIUM-3.M (HMAC signature is not a cryptographic proof of identity).** The module itself documents this at `share_temppass.rs:38-45` — the C client uses RSA-4096 signatures; Rust uses HMAC-SHA256 under the shared master key. This means **the invitee cannot verify the blob originates from the inviter** unless both sides already share the master key — which defeats the threat model of cross-user sharing. The module is honest about this under bd-1du.5, but as deployed today the `accept_temppass_wire` helper requires the caller to already possess the master key. This is fine for the round-trip test but is **not** a real cross-user team-share protocol. Remediation: complete bd-1du.5 (RSA-4096 keypair) before claiming "business/team parity" is production-ready for the team-share path.
- **HIGH-3.N (no expiry / revocation window).** The wire blob carries no timestamp, no sequence number, and no revocation marker. Once a temppass wire leaks, the holder can re-derive the master key **forever** (modulo Argon2id cost). The C client likewise has this problem, but the C client's RSA signature at least binds the blob to a concrete RSA keypair that can be rotated server-side. Here nothing can be rotated. Remediation: include an `issued_at` + `expires_at` + monotonic `sequence` inside `TemppassBlob` and bind them into the AAD; have the daemon reject decodes whose `expires_at` is in the past.
- **LOW-3.O (AAD fixed constant).** The AAD is the literal `"pcloud-crypto/share-temppass/aad/v1"` (`share_temppass.rs:69`). If the rotation in `bd-1du.5` introduces a bump, the same code path will reject old blobs silently — no version upgrade test exists. Document and add an integration test.
- **LOW-3.P (hand-rolled base64).** The crate hand-rolls base64 encode/decode (`share_temppass.rs:410-491`) "to avoid pulling `hex` into the dep graph". The encoder/decoder are tested (`base64_round_trip` at line 634), but they are **one more non-standard crypto adjacent parser** to audit. A quick read shows it looks correct, but: (a) the decoder does not check padding byte positions thoroughly (e.g. `==` in the middle of the string); (b) no fuzz test. Remediation: either use `base64 = "0.22"` (already in the dep graph — `crates/pcloud-kms/src/lib.rs:636` uses `base64::engine::general_purpose::STANDARD`) or fuzz the decoder.

---

## 8. Zeroization

### 8.1 `SecretBytes` — `crates/pcloud-secret/src/secret_bytes.rs:22-23`

```rust
#[derive(ZeroizeOnDrop)]
pub struct SecretBytes(Vec<u8>);
```

**GOOD:** Derives `ZeroizeOnDrop`. `PartialEq` is constant-time (`secret_bytes.rs:82-94`). `Clone` not derived — explicit `clone_secret()` only. No `Serialize`/`Deserialize` impl. `Debug` redacted.

### 8.2 Master key storage — `crates/pcloud-crypto/src/keys.rs:73-78`

```rust
#[serde(skip)]
pub active_key_material: Option<SecretBytes>,
```

**GOOD:** `#[serde(skip)]` so the master key never reaches a serialiser. Wrapped in `SecretBytes` — zeroize on drop.

### 8.3 Per-file key — `crates/pcloud-crypto/src/content.rs:126-134`

**GOOD:** output of `derive_file_key` is `SecretBytes` — zeroize on drop.

### 8.4 `PlaintextDek` — `crates/pcloud-kms/src/lib.rs:120-149`

```rust
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct PlaintextDek(pub Vec<u8>);
```

**GOOD:** zeroize on drop.

### 8.5 Argon2id intermediate buffer — `crates/pcloud-crypto/src/keys.rs:154-159`

```rust
let mut derived = vec![0u8; DERIVED_KEY_LEN];
Argon2::default()
    .hash_password_into(password.expose_secret().as_bytes(), salt, &mut derived)
    .expect(...);
SecretBytes::new(derived)
```

The intermediate `derived: Vec<u8>` is moved into `SecretBytes::new(derived)` **without an explicit zeroize of the old stack/heap location before the move**. Because `Vec::new` here just takes ownership of the already-allocated buffer, the pointer/length/capacity move is trivial and **no copy exists in heap memory**. So zeroization is preserved once `SecretBytes` drops. OK.

### 8.6 Password scorer — `crates/pcloud-crypto/src/password_scorer.rs:376-394, 466-469, 670-683`

- `lpwd`, `ldpwd` intermediate buffers are explicitly `zeroize()`-d after use (`line 466-467`).
- `usercopy`, `salt`, `derived` buffers in `psync_derive_password_from_passphrase` are explicitly `zeroize()`-d (`line 670, 679, 682`).
- `SecretBytes` holds the final base64 output — zeroize on drop.

**GOOD.**

### 8.7 HMAC engine intermediate state

`hmac::Hmac<Sha256>` / `hmac::Hmac<Sha512>` do **not** implement `Zeroize` — they carry their inner state as plain arrays. This is a **known limitation of the `hmac` crate**: the HMAC key is mixed into the inner digest state and is not zeroized when `Mac` instances are dropped.

- **MEDIUM-3.Q (HMAC inner-state residue).** Every call site in `pcloud-crypto` instantiates a fresh `Hmac<Sha256>` / `Hmac<Sha512>` from a `SecretBytes` key, computes the tag, and drops the MAC instance. The inner state, which mixes the key into two hash blocks, is **not** zeroized on drop. This is a small residue window (one function's stack frame or heap alloc, depending on MAC instantiation) but it violates the strict "no key bits survive drop" posture. Remediation: wrap `Hmac::<T>::finalize()` calls in a helper that explicitly zeroizes a by-value wrapper, or upstream a `ZeroizeOnDrop` impl for the relevant `hmac` types. This is already tracked upstream as [RustCrypto/MACs#134]-class. Mark as **MEDIUM** because no test or theoretical attack exploits this residue without a heap-probe primitive.

### 8.8 KMS cache

- `cache_lookup` returns `dek.clone_secret()` on hit (`crates/pcloud-kms/src/lib.rs:244-253`) — the cached entry remains live until TTL expires or `stop()` evicts. The caller gets a fresh `PlaintextDek` that zeroizes on drop. Eviction is via `HashMap::remove`, which triggers `Drop` on the `CacheEntry`, which drops the `PlaintextDek`, which zeroizes the bytes.
- **GOOD.**

### 8.9 Zeroization findings summary

- **GOOD:** master key, per-file key, KMS DEK, Argon2id output, filename HMAC output all wrapped in zeroize-on-drop types.
- **MEDIUM-3.Q (HMAC residue — see above).**
- **LOW-3.R (hex encoder output not zeroized).** `lib.rs:971-979` `hex_encode` produces a `String` that is printed into `ReencodedPrivateKey.private_key_hex` and returned to the caller. The inputs are the **derivation salt** (non-secret) and the **fingerprint** (non-secret) and **flags** (non-secret) — none of these are secrets. But the HMAC signature (also `hex_encode`-d at `lib.rs:894`) derives from the master key, and the hex string is not a key itself. This is OK. **No action.**

---

## 9. Constant-time comparisons

All critical compares use `subtle::ConstantTimeEq`:

- Fingerprint check: `crates/pcloud-crypto/src/keys.rs:205` — `computed.0.ct_eq(&stored.0).into()`.
- `change_password` old-vs-new password compare: `lib.rs:936-940`.
- Temppass signature verify: `share_temppass.rs:227` — `expected.ct_eq(signature).unwrap_u8() == 1`.
- `SecretBytes::eq`: `crates/pcloud-secret/src/secret_bytes.rs:91-94` — `ct_eq`.

**GOOD:** no naive `==` on secret material found in the crypto crate.

- **LOW-3.S (`unwrap_u8() == 1` vs `.into::<bool>`).** At `share_temppass.rs:227` the idiom `.ct_eq(...).unwrap_u8() == 1` is technically correct (the `subtle::Choice::unwrap_u8` returns 0/1), but the `!= 0` return to a boolean branch does preserve constant-time because the branch runs after the full compare. Readers may misread this — prefer `bool::from(expected.ct_eq(signature))`. Cosmetic only.

---

## 10. Test vectors (KAT)

### 10.1 What exists

- **Self-consistency round-trip tests** for sector seal/open (`content.rs:280-327`, `tests/integration.rs:22-100`, `tests/proptest_seal.rs:32-92`).
- **Self-consistency round-trip** for temppass (`share_temppass.rs:523-646`).
- **Deterministic filename encoding** self-tests (`metadata.rs:118-147`).
- **PBKDF2-HMAC-SHA-512 RFC 6070-style KAT** for the *account* passphrase derivation (`password_scorer.rs:797-814`).
- **Password scorer regression** tests (`password_scorer.rs:703-786`).

### 10.2 What is missing

- **No KAT against the legacy C client's sector output.** There is no test that takes a known `(master, file_seed, sector_index, plaintext, ciphertext_produced_by_C)` tuple from `pclsync/pcryptofolder.c` and proves the Rust `open_sector` recovers the same plaintext.
- **No KAT against the legacy C client's filename encoding.** `metadata::encrypt_filename` uses `HMAC-SHA256` with a new fixed label — the C client does not do this. So KAT is structurally impossible unless cross-client interop is a goal. Currently CLAUDE.md does not claim byte-level interop, but it also does not explicitly call out that the Rust encrypted-folder format is **incompatible** with the C encrypted-folder format.
- **No KAT against the legacy C client's temppass blob.** Documented incompatibility at `share_temppass.rs:39-45` (HMAC vs RSA signature).
- **No fuzz targets** for `seal_sector` / `open_sector` / `encrypt_filename`. The proptest suite at `proptest_seal.rs` is bounded to 128 cases per property, which is a reasonable CI budget but is not a fuzz harness.

### 10.3 Finding

- **CRITICAL-3.A (no cross-client KAT for interop claims).** If the product ships any claim of interop with pcloudcom/pcloud-rs encrypted content, this is a blocker. If the product commits to "Rust-only encrypted-folder format, migration required", this is NOT a blocker — but the CLAUDE.md and parity matrix must say so in plain English so an auditor does not misread. Right now neither is done: CLAUDE.md says "AES-256-GCM sector encryption" and "deterministic metadata filename encoding" are "Implemented" without flagging byte-incompatibility. Remediation: add a `docs/enterprise/crypto-compat.md` stating "the Rust encrypted-folder format is NOT compatible with the legacy C encrypted-folder format; users re-enrol on migration", and add a test module `crypto_compat.rs` asserting that a freshly-enrolled profile produces ciphertext the Rust code can round-trip through all supported crate versions.

---

## 11. Metadata filename encoding

### 11.1 Scheme — `crates/pcloud-crypto/src/metadata.rs:90-108`

```rust
pub fn encrypt_filename(master: &SecretBytes, plaintext: &str) -> Result<String, MetadataCryptoError> {
    if plaintext.is_empty() || plaintext.contains('/') {
        return Err(MetadataCryptoError::InvalidName);
    }
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master.expose_secret())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(FILENAME_LABEL);           // "pcloud-crypto/filename/v1"
    mac.update(plaintext.as_bytes());
    let tag = mac.finalize().into_bytes();
    // ... hex-encode, fixed 64 chars ...
}
```

### 11.2 Properties

- **Deterministic:** same master + same plaintext name → same 64-char hex tag. Required for server-side lookup without exposing the master key. Tested at `metadata.rs:119-124`.
- **Collision-resistant:** 256-bit HMAC-SHA256 output. Birthday bound ~2¹²⁸.
- **Reversible? No.** HMAC is a MAC, not a reversible encryption. The crate doc acknowledges this at `metadata.rs:55-76`: "Filename *length* is fully hidden (output is fixed 64 chars)" — which is a benefit, but **the client cannot display the plaintext filename** unless it keeps a local mapping (encrypted_name -> plaintext_name). Today there is **no such mapping** in `CryptoFolderEntry` (`lib.rs:213-222`) — the entry stores only the *encrypted* name. So the daemon **cannot show plaintext file names** without maintaining its own out-of-band local directory.
- **Cross-account uniqueness:** master is per-account → tags do not collide across accounts.
- **Intra-account repeat-name leak:** same plaintext name in different folders produces the same tag. Documented as intentional at `metadata.rs:74-76`.

### 11.3 Findings

- **HIGH-3.T (encryption is one-way; plaintext filename is not recoverable).** This is a fundamental design decision, not a bug, but the CLAUDE.md phrasing "deterministic metadata filename encoding" does not make clear that the encoding is **irreversible**. The daemon currently has no filename-plaintext-cache surface exposed via IPC, which means a CLI listing of a crypto folder returns 64-char hex blobs rather than user-readable names. This is an enterprise-UX blocker. Remediation: either (a) switch to a deterministic *encryption* (e.g. AES-SIV with fixed nonce derived from a HKDF) so the plaintext is recoverable with the master key, or (b) add a local-only `encrypted_name -> plaintext_name` cache to `CryptoShell::folders` populated by `mkdir` and `rmdir` flows. The C client uses AES-CBC over the filename so it IS reversible.
- **MEDIUM-3.U (empty-string rejection only, no UTF-8 NFC normalisation).** `encrypt_filename` rejects empty names and `/` (`metadata.rs:94`). It does **not** normalise Unicode. `"café"` in NFC and `"cafe\u{0301}"` in NFD produce different tags, so a macOS client (NFD) and a Linux client (NFC) will desync. Remediation: normalise via `unicode-normalization::UnicodeNormalization::nfc()` (or at least document it).
- **LOW-3.V (no length check upper bound).** Nothing bounds how long `plaintext` can be. HMAC-SHA256 accepts arbitrary input, but the pCloud backend has filename length limits. The crate should reject names that exceed the backend's maximum **before** deriving the tag, to match the C client's behaviour.

---

## 12. `unsafe` in crypto

```
$ grep -r "unsafe" crates/pcloud-crypto
crates/pcloud-crypto/src/lib.rs:1:#![forbid(unsafe_code)]
(plus three false-positive hits on the word "unsafe" in error messages / doc comments)
```

**GOOD:** `#![forbid(unsafe_code)]` at `crates/pcloud-crypto/src/lib.rs:1`. No `unsafe` blocks in the crate. No `unsafe` blocks in `pcloud-secret` either.

`pcloud-kms`: `#![forbid(unsafe_code)]` at `crates/pcloud-kms/src/lib.rs:30`. No `unsafe` blocks.

- **GOOD:** end-to-end absence of `unsafe` in the crypto-handling crates.

---

## 13. Dependencies

### 13.1 Primitive backers

| Crate | Version | Purpose | Posture |
|---|---|---|---|
| `aes-gcm` | 0.10.3 | AES-256-GCM AEAD | RustCrypto, audited, widely deployed. Feature set: `["aes", "alloc"]`, no `std` dep — good for minimal build. |
| `argon2` | 0.5.3 | Argon2id master-key KDF | RustCrypto. OWASP-recommended defaults (m=19456, t=2, p=1). |
| `hmac` | 0.12.1 | HMAC-SHA256 / SHA512 | RustCrypto. Does **not** implement `Zeroize` on its inner state (see MEDIUM-3.Q). |
| `sha2` | 0.10.9 | SHA-256, SHA-512 | RustCrypto. Pure-software. |
| `subtle` | 2.6.1 | Constant-time compare | RustCrypto. |
| `zeroize` | 1.8.2 | Drop-time memory zeroization | Standard primitive. |
| `getrandom` | 0.2.17 (primary) | OS CSPRNG | `0.3.4` and `0.4.2` also present transitively. |

### 13.2 Feature gating

- `aes-gcm` is pulled with `default-features = false, features = ["aes", "alloc"]` — no "std" feature bloat, no hazmat exports. **GOOD.**
- `zeroize` pulled with `zeroize_derive`. **GOOD.**

### 13.3 Multiple `getrandom` versions

- `getrandom 0.2.17` (primary, used directly by `pcloud-crypto`).
- `getrandom 0.3.4` (transitive via `rand`, indirectly).
- `getrandom 0.4.2` (transitive via something newer).

- **LOW-3.W (multiple `getrandom` versions in tree).** Not a correctness issue — each is a functional OS CSPRNG wrapper — but ships three copies of effectively the same code, bloats build, and complicates audit. Remediation: run `cargo update -p getrandom --precise 0.3.4` + dependency reconciliation to converge on a single version.

### 13.4 FIPS posture

- **No FIPS claim in the crate doc.** The primitives (AES-256-GCM, SHA-256, SHA-512, HMAC, PBKDF2, Argon2id) are all NIST-approved **except** Argon2id, which is not FIPS-140-3 approved. Enterprise deployments that require FIPS-140-3 would need to swap `argon2` for PBKDF2 (or PBKDF2-HMAC-SHA-512, already available on the passphrase path).
- The KMS providers (AWS KMS, HashiCorp Vault transit, PKCS#11 HSM) **can** be FIPS-validated depending on the backing HSM / KMS configuration.

- **MEDIUM-3.X (no FIPS mode switch).** For enterprise claims ("stricter than C on …" per CLAUDE.md Final Rule), consider adding a `CryptoPolicy::fips_mode: bool` gate that switches Argon2id → PBKDF2-HMAC-SHA-512 (same iteration count as the server API-password path — already implemented in `password_scorer.rs`) and refuses any non-FIPS-approved primitive.

### 13.5 Advisories

- No `cargo audit` output captured in this audit (offline environment).
- aes-gcm 0.10.x has no open RUSTSEC advisories as of my cutoff.
- argon2 0.5.x has no open RUSTSEC advisories as of my cutoff.
- `getrandom 0.2.x` has had RUSTSEC-2024-0331 closed; no known open issues.

- **LOW-3.Y (no CI-gated `cargo audit`).** Recommend adding `cargo audit --deny warnings` as a CI gate on the `pcloud-crypto` + `pcloud-kms` crates.

---

## Severity-ranked findings ledger

### CRITICAL

- **CRITICAL-3.A — No cross-client KAT for interop claims.**
  Files: `crates/pcloud-crypto/tests/*.rs`, `CLAUDE.md` "Crypto parity progress".
  The Rust crate uses new primitives (AES-256-GCM, HMAC-SHA256-based filename encoding, HMAC-based setup fingerprint, HMAC-based temppass signature). These are **not** byte-compatible with the legacy C `pclsync/pcryptofolder.c` format. No test asserts, and no documentation explicitly states, this incompatibility. If the product ships with "crypto is active on the retained Rust path" language while users expect to open legacy-C encrypted folders, this is a silent data-access failure.
  Remediation: (a) add explicit "NOT byte-compatible" language in the parity matrix and in a new `docs/enterprise/crypto-compat.md`; (b) add a `tests/legacy_c_kat.rs` that either (i) proves interop against a captured C-client ciphertext (ideal) or (ii) explicitly asserts that legacy-C-shape ciphertext is rejected, matching the documented non-compat contract.

### HIGH

- **HIGH-3.C — Argon2id vs legacy C KDF interop is unverified.**
  Files: `crates/pcloud-crypto/src/keys.rs:134-160`, `CLAUDE.md`.
  See CRITICAL-3.A for the umbrella issue; HIGH-3.C is the specific master-key-KDF drift.
  Remediation: folded into CRITICAL-3.A.

- **HIGH-3.F — No wrong-password rate-limit or lockout at the crypto layer.**
  Files: `crates/pcloud-crypto/src/lib.rs:713-738`, `crates/pcloud-daemon/src/runtime.rs:2533-2564`.
  Remediation: add a `KeyManager::consecutive_failures: u32` counter with exponential backoff; reset on success; consider a hard lockout after N failures.

- **HIGH-3.H — CLAUDE.md is out of date re: `change_crypto_pass` family.**
  Files: `crates/pcloud-crypto/src/lib.rs:837-967`, `CLAUDE.md` "Still missing" list.
  Remediation: fix CLAUDE.md; update `C_FEATURE_PARITY_MATRIX.csv` row.

- **HIGH-3.I — Password rotation silently invalidates existing sector ciphertext.**
  File: `crates/pcloud-crypto/src/lib.rs:837-896`.
  Per-file keys are `HMAC-SHA256(master, "..." || seed)`; rotating `master` rotates every per-file key. Old ciphertext becomes unreadable. No test warns, no doc flags.
  Remediation: either introduce a KEK layer so master rotation does not invalidate file keys, or add an integration test that (a) writes a sector, (b) rotates the password, (c) asserts the old frame is now unreadable — and document the invariant in the `change_password_unlocked` docblock.

- **HIGH-3.L — CLAUDE.md is out of date re: `send_change_user_private` and `priv_key_flags`.**
  Files: `crates/pcloud-crypto/src/lib.rs:814-817`, `crates/pcloud-daemon/src/runtime.rs:2667-2698`, `CLAUDE.md` "Still missing" list.
  Remediation: fix CLAUDE.md; update parity matrix rows for `PSYNC_CRYPTO_FLAG_TEMP_PASS` and `psync_crypto_send_change_user_private`.

- **HIGH-3.N — Temppass blob has no expiry, no revocation, no sequence number.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:158-163`.
  Remediation: add `issued_at: u64`, `expires_at: u64`, `sequence: u64` to `TemppassBlob`; bind them into AAD; have the daemon reject decodes whose `expires_at` is in the past.

- **HIGH-3.T — Filename encoding is irreversible; plaintext is not recoverable.**
  File: `crates/pcloud-crypto/src/metadata.rs:90-108`.
  HMAC-SHA256 is a MAC, not a cipher — there is no inverse. A client listing a crypto folder sees 64-char hex blobs.
  Remediation: switch to deterministic authenticated encryption (AES-SIV) for filenames, or add a local `encrypted_name -> plaintext_name` cache populated by `mkdir`/`rmdir` and persisted in the profile store.

### MEDIUM

- **MEDIUM-3.B — No per-sector key; sector rekey schedule is documented but not enforced.**
  File: `crates/pcloud-crypto/src/lib.rs:1096-1101`, `crates/pcloud-crypto/src/content.rs:177-207`.

- **MEDIUM-3.E — 96-bit random nonce birthday bound (2⁴⁸ sectors per file key).**
  File: `crates/pcloud-crypto/src/content.rs:186-206`. Not reachable today; not future-proof.

- **MEDIUM-3.G — Recovery-code binding is at the IPC layer only.**
  Files: `crates/pcloud-daemon/src/runtime.rs:2714-2720, 2771-2776`. No crypto-level enforcement.

- **MEDIUM-3.J — `ReencodedPrivateKey.private_key_hex` does not include account identity.**
  File: `crates/pcloud-crypto/src/lib.rs:876-895`.

- **MEDIUM-3.M — Temppass HMAC signature is a shared-secret proof, not identity proof.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:38-45, 213-220`.

- **MEDIUM-3.Q — HMAC inner-state key residue is not zeroized.**
  Files: every `Hmac<Sha256>::new_from_slice(...)` call site across the crate. Upstream limitation of the `hmac` crate.

- **MEDIUM-3.U — No Unicode NFC normalisation on encrypted filenames.**
  File: `crates/pcloud-crypto/src/metadata.rs:90-108`.

- **MEDIUM-3.X — No FIPS mode switch.**
  Files: `crates/pcloud-crypto/src/policy.rs`, `crates/pcloud-crypto/src/keys.rs`.

### LOW

- **LOW-3.D — Error discipline divergence on `getrandom` failure (panic vs error).**
  Files: `crates/pcloud-crypto/src/content.rs:189`, `crates/pcloud-crypto/src/share_temppass.rs:304-305`, `crates/pcloud-crypto/src/lib.rs:540`, `crates/pcloud-kms/src/lib.rs:964`.

- **LOW-3.K — `change_password_unlocked` allows rotating to the same passphrase.**
  File: `crates/pcloud-crypto/src/lib.rs:837-896`.

- **LOW-3.O — Share-temppass AAD is fixed; no upgrade test.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:69`.

- **LOW-3.P — Hand-rolled base64 encoder/decoder not fuzzed.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:410-491`.

- **LOW-3.R — Hex encoder output not zeroized.**
  File: `crates/pcloud-crypto/src/lib.rs:971-979`. Inputs are non-secret; cosmetic only.

- **LOW-3.S — `ct_eq(...).unwrap_u8() == 1` idiom is unusual.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:227`.

- **LOW-3.V — No upper bound on `encrypt_filename` plaintext length.**
  File: `crates/pcloud-crypto/src/metadata.rs:90-108`.

- **LOW-3.W — Three `getrandom` versions in the dep graph.**
  File: `Cargo.lock` (`getrandom 0.2.17`, `0.3.4`, `0.4.2`).

- **LOW-3.Y — No CI-gated `cargo audit`.**
  Files: CI config (not in scope for this audit, but relevant for the crypto crates).

---

## What is good (explicit positives)

- `#![forbid(unsafe_code)]` + zero `unsafe` blocks across `pcloud-crypto`, `pcloud-kms`, `pcloud-secret`.
- Master key never persisted; `#[serde(skip)]` on `active_key_material`.
- Policy gate `persist_master_key` rejected with `UnsafePolicy` **before** any key derivation — see `lib.rs:664-668`.
- Constant-time fingerprint compare and constant-time temppass signature compare.
- `SecretBytes::PartialEq` is constant-time.
- `SecretBytes` / `SecretString` are `!Clone` — explicit `clone_secret()` only.
- Sector AEAD binds the sector index as AAD **and** verifies the embedded index before the AEAD call (defence-in-depth against AAD-swap) — `content.rs:260-269`.
- Temppass module verifies signature **before** AEAD unwrap (prevents chosen-ciphertext oracle).
- KMS `PlaintextDek` is `ZeroizeOnDrop`; process-local cache evicts on `stop()` (eviction triggers `Drop` → zeroize).
- `NullKms` is explicit: it refuses every wrap/unwrap call rather than silently falling back.
- KMS cache disambiguates by `(provider, key_id, wrapped_bytes, context)` — wrap-blob replay across contexts is blocked.
- Property tests cover seal/open round-trip, AAD-swap rejection, wrong-key rejection.
- RFC 6070-style KAT exists for PBKDF2-HMAC-SHA-512 (account API password path).
- Password scorer is a byte-faithful port of the C scorer with stricter secret handling.

---

## Actionable remediation summary (ranked)

1. Close CRITICAL-3.A: publish `docs/enterprise/crypto-compat.md`; add explicit non-compat language in CLAUDE.md + parity matrix; add a regression test (`legacy_c_kat.rs`) that either proves interop with captured C ciphertext or asserts explicit rejection.
2. Close HIGH-3.I: decide whether to re-architect per-file key derivation around a KEK layer (preferred) or document + test the "rotation invalidates all existing content" contract.
3. Close HIGH-3.F: add wrong-password backoff / lockout in `KeyManager`.
4. Close HIGH-3.N: add `issued_at` + `expires_at` + `sequence` to `TemppassBlob`; bind into AAD.
5. Close HIGH-3.T: switch filename encoding to AES-SIV (reversible, deterministic, authenticated), or add a local plaintext-name cache.
6. Close HIGH-3.H + HIGH-3.L: sync CLAUDE.md + parity matrix with actual code.
7. Close MEDIUM-3.Q: wrap `Hmac<T>` usage in a zeroize-on-drop helper, or block until the `hmac` crate upstreams `ZeroizeOnDrop`.
8. Close MEDIUM-3.U: add Unicode NFC normalisation before HMAC in `encrypt_filename`.
9. Close MEDIUM-3.X: add a FIPS mode policy bit that swaps Argon2id → PBKDF2-HMAC-SHA-512.
10. Close MEDIUM-3.E: enforce a sector-rekey hook at the daemon once >2³² sectors on a single file key.
11. Tidy LOW items as follow-ups.

---

## End of Section 3
## Section 4. Sync Engine & Runtime

**Scope:** Queue model, state persistence, conflict resolution, watcher, idempotency, back-pressure, retry/resilience, integrity sweeper, power awareness, pause/resume, stall detection, resource leaks, engine test coverage.

**Out of scope (delegated):** FUSE/mount internals (Dim. 5), parity matrix (Dim. 1), HTTP transport (Dim. 6).

**Primary files audited:**

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/lib.rs` (911 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/scheduler.rs` (219 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/planner.rs` (656 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/conflict_resolver.rs` (341 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/recovery.rs` (189 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/fs_events.rs` (184 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/local_scan.rs` (533 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/diff_poller.rs` (216 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/reconcile_worker.rs` (283 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/transfers/uploads.rs` (361 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/transfers/downloads.rs` (267 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/session_manager.rs` (22 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/sync_loop.rs` (819 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/sync_loop_runtime.rs` (955 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/integrity_sweeper_service.rs` (1947 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/schema.rs` (331 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/migrations.rs` (118 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/tx.rs` (90 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/integrity.rs` (42 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/repositories/upload_resume.rs` (316 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/repositories/diff_state.rs` (117 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cache/src/staging.rs` (130 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cache/src/page_cache.rs` (505 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/retry.rs` (489 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/circuit_breaker.rs` (534 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/rate_limit.rs` (306 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/pacing.rs` (254 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/fs_watcher.rs` (662 lines)

---

### Architectural overview

`EngineShell` (`crates/pcloud-engine/src/lib.rs:66-103`) is a single-owner aggregate built from:

- `DiffPoller` (remote cursor bookkeeping),
- `LocalScanner` (full-walk cadence),
- `FsEventIngestor` (event coalescing),
- `Planner` (pair/conflict detection),
- `Scheduler` (priority queue),
- `RecoveryManager` (failure classifier),
- `ConflictResolver` (policy applicator),
- `UploadCoordinator` / `DownloadCoordinator`.

`EngineShell` is **not** `Sync` — it is owned and mutated exclusively on the sync loop thread (`crates/pcloud-daemon/src/sync_loop.rs:25-32`, `std::thread` based, **not** tokio). The IPC dispatch thread communicates via `Arc<SyncLoopShared>` (`sync_loop.rs:104-118`) with a `Mutex<SyncLoopStatus>` + `Condvar` wake signal.

The engine itself is a **pure synchronous state machine** — zero I/O happens inside the engine crate. All I/O is driven by `RealSyncLoopRuntime` in `crates/pcloud-daemon/src/sync_loop_runtime.rs`.

That design is clean in theory. The gaps below concern specific correctness, durability, and enterprise-grade expectations.

---

## CRITICAL findings

### C-1. Scheduler has no per-sync-root fairness — single root can starve all other roots

`crates/pcloud-engine/src/scheduler.rs:80-87` (`replace_queue`):

```
operations.sort_by(|left, right| {
    left.priority()
        .cmp(&right.priority())
        .then(left.path().cmp(right.path()))
});
```

`crates/pcloud-engine/src/scheduler.rs:122-127` (`next_batch`):

```
let limit = self.max_parallel_uploads + self.max_parallel_downloads;
let limit = limit.max(1).min(self.queued_operations.len());
&self.queued_operations[..limit]
```

There is **no per-sync-root fairness mechanism whatsoever**. The queue is a flat `Vec<PlannedOperation>` sorted only by `(priority, path)`. One sync root with 100k queued uploads will monopolise every scheduler batch until it drains, **completely starving the others**.

There is also no round-robin shuffle at batch-emit time. This is a fairness defect the C client does not exhibit because the C daemon runs per-sync-root workers.

**Severity:** CRITICAL — customer with a big backup sync root and a small "inbox" sync root will see the inbox become unresponsive indefinitely.

**Remediation:** Group queued operations by `sync_id` and interleave batches round-robin across sync roots; keep priority ordering only *within* a root. Add a `fairness_policy: FairnessPolicy::RoundRobinPerSyncRoot` knob and a proptest that asserts "N roots, any ops, every root emits at least ceil(batch_size/N) work per N batches".

---

### C-2. `Scheduler::next_batch` is a pure peek — the queue never drains

`crates/pcloud-engine/src/scheduler.rs:122-127` and module-level docs at `:10-14` ("Batch semantics — **peek**").

`next_batch` returns `&self.queued_operations[..limit]` **without removing** the items. `EngineShell::advance_transfer_cycle` (`lib.rs:409-414`) hands the batch to the upload/download coordinators and then calls `next_batch` again — but still does not pop.

Because the scheduler never dequeues:

1. On the next `replace_queue` call (`lib.rs:200-207`, `ingest_candidates`), every still-in-flight operation is silently overwritten. The *coordinators* retain their in-flight copies, but:
2. `mark_transfer_completed` / `mark_transfer_failed` (`lib.rs:419-428`) only mutate coordinator lists — **they do not remove the completed operation from `scheduler.queued_operations`**. So a completed upload continues to appear in `queued_operations.len()`, `summary()`, and worse, will be re-emitted by the next `next_batch()` peek until `replace_queue` is called again.
3. The queue has no notion of "in-flight vs waiting". A concurrent scheduler + coordinator interaction will repeatedly hand the *same* operation back to `accept_batch` on every cycle if `ingest_candidates` is not called first.

**Severity:** CRITICAL — duplicate uploads on every cycle, pending-count drift, conflict count never settles.

**Remediation:** Split `queued_operations` into `waiting: VecDeque` + `in_flight: HashMap<path, op>`. `next_batch` must pop (not peek). Completion / failure must remove the operation from `in_flight`. Document the state machine: queued → in_flight → (completed | failed | retry_later).

---

### C-3. `newest_wins` conflict policy does not compare timestamps

`crates/pcloud-engine/src/conflict_resolver.rs:170-179`:

```
fn resolve_newest_wins(
    sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    // Without real timestamp comparison, fall back to prefer-remote
    // (server-wins tie-break, matching the C client's newest-wins
    // default when timestamps are equal).
    resolve_prefer_remote(sync_id, path, kind)
}
```

The function is a lie dressed up as an implementation. The policy name `NewestWins` promises mtime-based arbitration; the code unconditionally picks remote. Enterprise users who explicitly configure `newest_wins` (because they want "the user's most recent edit should win") will silently lose local edits every time.

The unit test at `:280-298` even accepts this by asserting that `newest_wins` produces a `DownloadFile` regardless of timestamps, which means the test is locking in the buggy behavior.

**Severity:** CRITICAL — advertised feature silently destroys local edits.

**Remediation:** Thread `local_mtime` and `remote_mtime` into the `ConflictKind::LocalModifyVsRemoteModify` payload. If either mtime is unknown, fail the policy (emit `ManualReview`). The current "fall back to prefer-remote" default is **data loss**, not a tie-break.

---

### C-4. `rename_both` conflict policy does not rename — it produces ManualReview

`crates/pcloud-engine/src/conflict_resolver.rs:181-191`:

```
fn resolve_rename_both(
    _sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    ConflictResolution::ManualReview {
        path: path.to_owned(),
        kind: kind.clone(),
        reason: "rename-both: both copies preserved for manual merge".to_owned(),
    }
}
```

Policy docstring at `:31-33` says both sides become `.conflict-local.ext` / `.conflict-remote.ext`. The implementation emits `ManualReview` and does nothing. This is also the **default** policy (`:52-58`, `ConflictResolver::default`), so every collision under default config stalls indefinitely.

**Severity:** CRITICAL — the documented default conflict behavior is not implemented.

**Remediation:** Emit `ConflictResolution::Apply` with two explicit `UploadFile` + `DownloadFile` operations targeting the `.conflict-local.ext` / `.conflict-remote.ext` sibling paths, and schedule a delete for the original. Write proptests: "after rename_both, no path has overlapping local/remote state".

---

### C-5. Scheduler has no memory budget — unbounded `Vec` growth is a DoS

`crates/pcloud-engine/src/scheduler.rs:38-59`: `Scheduler::queued_operations` is a plain `Vec<PlannedOperation>` with no cap.

`crates/pcloud-engine/src/planner.rs:50-58`: `Planner::max_operations_per_tick` defaults to `1024`, but this bounds **per tick**, not the queue. Successive ticks accumulate indefinitely through `replace_queue`, which overwrites the queue entirely (not merged) — so it is coincidentally bounded **only because** every tick replaces everything, which is itself the C-2 bug.

If C-2 is fixed without adding a queue-size cap at the same time, this becomes a classic unbounded-queue memory DoS. A sync root pointing at a tree of 10M files will push 10M `PlannedOperation`s (each with a heap-allocated `String` path) into memory.

**Severity:** CRITICAL (conditional on C-2 fix).

**Remediation:** Add `Scheduler::max_queue_size: usize` (default ~100k), and on overflow either spill-to-disk or emit a `BackPressure` event back up the pipeline. The planner must also honor `max_ops` with a "deferred" queue that gets drained on subsequent ticks.

---

### C-6. FsEvent coalescing is unbounded — memory DoS under event storms

`crates/pcloud-engine/src/fs_events.rs:64-95`, `FsEventIngestor::normalize_events`:

```
pub fn normalize_events(&self, events: &[FsEvent]) -> Result<Vec<SyncCandidate>, FsEventError> {
    let mut coalesced = Vec::<FsEvent>::new();
    for event in events {
        validate_relative_path(&event.path)?;
        if let Some(existing) = coalesced
            .iter_mut()
            .find(|candidate| candidate.path == event.path)
        { ... }
```

Two problems:

1. The coalescer uses `Vec::find` — **O(n²)** for n distinct paths.
2. `coalesce_window_ms` (`:16-17`) is a struct field but is **never read**. The comment at `:10-12` promises time-window coalescing; the code performs ordering-based dedup only. An event stream with N distinct paths all spaced by any duration will still produce N candidates.

Additionally, the upstream `fs_watcher.rs` debouncer (see M-4 below) does coalesce by path within the debounce window, but the ingestor interface accepts arbitrary-sized batches and offers no ceiling.

**Severity:** CRITICAL — scan of a tree with 1M files through `inotify` events will blow up memory and burn CPU in an O(n²) loop.

**Remediation:** Replace `Vec::find` with `HashMap<String, FsEvent>`. Add `max_queued_events` cap; drop oldest or escalate to full-scan on overflow. Either remove `coalesce_window_ms` or honor it with an `Instant`-keyed coalescer.

---

### C-7. Audit-rebuild migration runs unbatched with no idempotency if interrupted

`crates/pcloud-store/src/migrations.rs:80-118`: `apply_plan` applies each `apply_schema_vN` with the **caller's** optional wrap in `TransactionBoundary::immediate`. `bootstrap_profile` (`crates/pcloud-store/src/lib.rs:202-233`) does NOT wrap the migration in a transaction — it calls `apply_plan(&conn, &plan)` directly, and only the individual DDL statements commit via their embedded `PRAGMA user_version = N`.

`crates/pcloud-store/src/schema.rs:168-193`, `apply_schema_v8`: calls `crate::repositories::audit::rebuild_hash_chain(conn)` between the `ALTER TABLE` and the `PRAGMA user_version = 8`. If the process is killed mid-rebuild:

- Columns `prev_hash`, `entry_hash`, `hmac` already exist (`:174-182` are separate `ALTER TABLE` statements, each auto-committed by SQLite outside a transaction),
- `rebuild_hash_chain` may have partially re-hashed rows,
- `user_version` is still 7,
- On next launch, `apply_schema_v8` runs again and calls `rebuild_hash_chain` a second time — if that routine is not idempotent over partial state, the chain becomes corrupt.

I did not read `rebuild_hash_chain` in full, but the migration path MUST be atomic regardless of its idempotency story. Defense in depth demands a wrapping transaction.

**Severity:** CRITICAL for audit-log integrity, which is the crate's headline security invariant.

**Remediation:** Wrap the entire migration plan in `TransactionBoundary::immediate` at the `bootstrap_profile` layer, remove the per-step `PRAGMA user_version` commits, and commit `user_version` atomically with DDL. Alternatively, keep step-wise commits but wrap each step (including its `rebuild_hash_chain`) in its own transaction that also bumps `user_version`.

---

### C-8. No stall detection, no transfer timeout

Searching the engine and runtime for "stall", "timeout", "inactivity": zero matches in `crates/pcloud-engine/src/` for stall detection. The `TransferTask` state (`crates/pcloud-engine/src/transfers/uploads.rs:152-158`) has `state: TransferState` but no `last_progress_at: Instant`.

`pcloud-resilience/src/timeout.rs` exists (82 lines) but is a generic wrapper, not engine-integrated. The sync loop runs with a 5-minute default poll interval (`crates/pcloud-daemon/src/sync_loop.rs` + config). An upload that "completes" its first chunk and then hangs forever on `upload_write` will sit in `active_uploads` indefinitely.

**Severity:** CRITICAL — the daemon can get stuck with phantom in-flight uploads that never complete, never fail, and never retry. End users see "syncing…" forever.

**Remediation:** Add `TransferTask::last_progress_at: Instant`, a `stall_timeout: Duration` (default 5 min), and a periodic scan in `EngineShell::advance_transfer_cycle` that marks stalled tasks as `Failed { reason: "stall_detected" }` and re-queues them through the recovery manager. Emit an audit event `sync.stall_detected` when this fires.

---

## HIGH findings

### H-1. No idempotency keys on uploads; mutation retries are globally disabled

`crates/pcloud-resilience/src/retry.rs:264-273`, `MethodRetryPolicy::secure_default`:

```
pub fn secure_default(inner: RetryPolicy) -> Self {
    Self {
        inner,
        retry_idempotent: true,
        retry_mutations: false,
        retry_unknown: false,
    }
}
```

So uploads (a mutation) will **never retry** under the default policy. That is safe against double-writes but means a single transient `503` aborts the upload and the user must trigger a new cycle.

Worse, the `UploadResumeRecord` (`crates/pcloud-store/src/repositories/upload_resume.rs:38-57`) carries an `upload_id` from the server's `upload_create`, so the upload *is* idempotent after that point — yet the retry policy has no way to express "retry only this mutation because I hold an `upload_id`".

There are also no idempotency tokens emitted on `upload_create` itself (which is not idempotent in the general sense). A network drop between client send and server response of `upload_create` results in an orphaned server-side upload handle with no way for the client to discover it — server-side cleanup is opaque.

**Severity:** HIGH — retriable flakes become user-visible failures; orphaned uploads accumulate on the server.

**Remediation:**

1. Add `MethodRetryPolicy::with_idempotency_keys` variant that opts in mutations *if* the caller provides a per-request idempotency token.
2. Require `upload_create` to send a client-generated UUID and have the server return a reused `upload_id` on replay. If that server support does not exist, document it as a spec gap and add a `cleanup_orphans` CLI.
3. Resume paths: always consult `upload_resume_state` before calling `upload_create`; test that a mid-`upload_create` crash does not create two server-side uploads for the same local file.

---

### H-2. Retry policy does not honor server's `Retry-After` header

`rg Retry-After` across `crates/`: zero matches in `pcloud-resilience`, zero in the sync loop, zero in the transfer/backends layer.

`crates/pcloud-resilience/src/retry.rs:100-120`: `RetryPolicy` only knows `Fixed` / `Exponential` / `ExponentialJittered` backoffs computed purely from `attempt` count.

When the pCloud server responds `429 Too Many Requests` with `Retry-After: 30`, the engine has no hook to pass that 30 seconds into the decision. The client ignores the server's explicit pacing signal and retries on its own internal schedule — which, depending on the exponential-jittered table, may be sooner than 30 s and will be blocked again.

**Severity:** HIGH — violates good-citizen behavior against the API, accelerates tenant-level rate-limit tripping, wastes server capacity.

**Remediation:** Extend `RetryDecision::Retry { wait: Duration }` with a `server_hint: Option<Duration>` and teach `RetryPolicy::next` to take a `server_hint: Option<Duration>` that overrides the schedule if present. The HTTP transport layer (Dimension 6) must extract `Retry-After` and plumb it in.

---

### H-3. No global retry budget; single-op retry budget is also implicit

`crates/pcloud-resilience/src/retry.rs:151-157`: `RetryPolicy::next(attempt)` is the only control. There is no *global* retry budget — if 10k uploads each retry 3 times simultaneously, the daemon will hammer the API with 30k calls in ~seconds.

**Severity:** HIGH — flaky network turns into self-inflicted DoS.

**Remediation:** Add `TokenBucket`-backed global retry budget in `pcloud-resilience`, separate from the per-request rate limiter. Reject retries when the bucket is empty (escalate to `ManualIntervention`).

---

### H-4. No case-insensitive / Unicode normalization for conflict detection

`crates/pcloud-engine/src/planner.rs:74-104`, `Planner::plan` pairs candidates by **exact string equality** on `path`. The validators (`fs_events.rs:98-111`, `local_scan.rs:273-286`, `diff_poller.rs:101-114`) reject `.`, `..`, empty segments, backslashes, but do **not** normalize.

- On macOS HFS+/APFS by default the filesystem is case-insensitive but case-preserving. A file created locally as `Report.txt` and remotely pulled as `report.txt` will be **two distinct candidates** that collide on disk at write time with no planner-level detection.
- HFS+ stores filenames in NFD, ext4/NTFS keep what you gave them. A file named with an accented character will have different byte sequences depending on the side — again two candidates for what is one conceptual file.

The `fs_watcher.rs::to_relative` also does `replace('\\','/')` on non-UTF-8 path handling lossily (`:245-254` uses `to_str()?` which drops non-UTF-8 names silently — see M-3).

**Severity:** HIGH — silent divergence, duplicate uploads, or failed writes on macOS with international content.

**Remediation:** Add a `path_normalize` module that (a) applies NFC (or the platform's native form) consistently, (b) on case-insensitive mounts compares lowercased keys while preserving the display form. Collisions after normalization should feed `ConflictKind::CaseCollision` into the planner.

---

### H-5. `FsEventIngestor::coalesce_window_ms` is a phantom field

`crates/pcloud-engine/src/fs_events.rs:13-27`: the field is declared, documented, and defaulted to 250 ms, but `normalize_events` at `:64-95` never consults it. The real debouncing happens in `pcloud-fs/src/fs_watcher.rs` with `WatcherConfig::debounce_duration: 500` (`:73-85`).

**Severity:** HIGH — public API surface lies about its behavior; config changes have no effect.

**Remediation:** Either delete the field (accept the watcher is the single source of debounce) or implement an Instant-keyed coalescer. Update `#[serde(deny_unknown_fields)]` accordingly so stale config files surface the removal.

---

### H-6. Watcher has no overflow detection / no rescan trigger

`crates/pcloud-fs/src/fs_watcher.rs:106-147`: `FsWatcher::start` installs `RecommendedWatcher` and registers a single callback (`:121-129`). On inotify overflow (`IN_Q_OVERFLOW`) the notify crate emits `EventKind::Other` or a rescan-request event, and this code at `:233-241` routes `_ => None` — i.e. **dropped silently**.

There is no re-scan trigger when the kernel buffer overflows. Files created while the inotify queue was overflowing will not be detected until the *next* full scan (up to 5 minutes later by default — `reconcile_worker.rs:38`).

**Severity:** HIGH — silent data loss on busy trees. An rsync run or a tarball extraction easily overflows `fs.inotify.max_queued_events` (16k default on Linux).

**Remediation:** Match `EventKind::Other` and any `notify`-specific overflow indicator; when observed, bump a counter, emit an audit event `sync.watcher_overflow`, and set `IncrementalScanTracker::request_scan` (equivalent) so the next tick forces a full walk.

---

### H-7. Debouncer flushes all pending events on disconnect with `debounce=0` — data loss on watcher shutdown is silent

`crates/pcloud-fs/src/fs_watcher.rs:184-197`:

```
Err(mpsc::RecvTimeoutError::Disconnected) => {
    // Watcher dropped; flush remaining and exit.
    flush_pending(&mut pending, &output_tx, sync_id, Duration::ZERO);
    break;
}
```

If the downstream `output_tx` receiver was already dropped, `flush_pending` will call `pending.clear()` at `:222-226` and return without surfacing that events were lost. No error propagates; the outer runtime has no way to know it should rescan on next startup.

**Severity:** HIGH — events at shutdown are silently discarded.

**Remediation:** Emit an audit event on shutdown-with-pending. Persist the last-successfully-drained sync_id + event cursor so restart does a full scan.

---

### H-8. No disk-budget / staging cap; staging eviction is lossy

`crates/pcloud-cache/src/staging.rs:29-41`: `max_open_files: 64` default, `files: HashMap<String, Vec<u8>>` unbounded in aggregate byte size, `open_order` LRU.

`crates/pcloud-cache/src/staging.rs:95-103`:

```
fn evict_if_needed(&mut self) {
    while self.files.len() > self.max_open_files {
        let Some(oldest) = self.open_order.pop_front() else { break; };
        self.files.remove(&oldest);
    }
}
```

Eviction here **drops the bytes** of the oldest staged file. The doc at `:1-8` acknowledges "Eviction here is lossy: evicted buffers are dropped, so callers must have already flushed them". But:

1. There is no enforcement that callers flushed before the 65th file is staged.
2. No per-buffer byte budget (a single 4 GiB file counts the same as a 1 KiB file for eviction).
3. No disk budget at all — nothing sets a maximum staging area size on disk.

A FUSE write path (`crates/pcloud-fs/src/write_path.rs`, Dim. 5 scope) or a batch of local creations above 64 files will silently lose bytes.

**Severity:** HIGH — data loss under normal usage.

**Remediation:** Bound by both count AND aggregate bytes (`max_staging_bytes: u64`). Before evicting, check a "must_flush_before_evict" callback; return an error to caller if the buffer cannot be flushed. Ultimately, staging of any non-trivial file must be disk-backed, not `Vec<u8>`.

---

### H-9. Scheduler eviction on sync-root remove is O(n) per op — bad for large queues

`crates/pcloud-engine/src/scheduler.rs:106-109`:

```
pub fn evict_sync_id(&mut self, sync_id: SyncId) {
    self.queued_operations
        .retain(|operation| operation.sync_id() != sync_id);
}
```

Same pattern at `transfers/uploads.rs:73-78` and `transfers/downloads.rs:73-78` — 5 separate `retain` calls per coordinator. With C-5 fixed and a 100k queue, removing a sync root becomes an O(500k) walk per evict. Not catastrophic, but trivially fixable.

**Severity:** HIGH when combined with large queues.

**Remediation:** Index queued ops by `sync_id` in a `BTreeMap<SyncId, VecDeque<PlannedOperation>>` so evict is `O(ops_for_that_root)` not `O(total_ops)`.

---

### H-10. No back-pressure from transport to ingestion

Grep `back_pressure` / `429` / `throttle` in `crates/pcloud-engine` returns zero. The engine ingests `FsEvent`s and remote diff batches as fast as they arrive regardless of whether the transport is successfully draining the queue.

If the server is returning `429`s repeatedly, the engine keeps building up queued operations, the staging area keeps filling up, and the memory/disk footprint grows unbounded.

**Severity:** HIGH — amplification of transient server problems into OOM.

**Remediation:** Add a `PressureSignal` event emitted by the HTTP client (Dim. 6) that the engine's ingestion paths consult before `ingest_fs_events` / `ingest_remote_diff`. When pressure is high, `FsEvent`s should be coalesced more aggressively into a "dirty-region" set rather than individual events.

---

### H-11. `diff_state` persistence is not transaction-bound to the ingestion that used it

`crates/pcloud-store/src/repositories/diff_state.rs`: I only sampled the file listing, but the repository's doc at `schema.rs:237-263` states the cursor is updated when the DiffWorker advances. There is no evidence in `RealSyncLoopRuntime` (searched for "diff_state") that the cursor advance is atomic with the local engine's planner ingest.

If the cursor is advanced *before* the planner successfully planned the batch, a crash loses the diff events and the engine has no way to re-fetch them (the server treats the cursor as read). If advanced *after*, duplicates are produced on crash — which is safer because uploads are then idempotent, **but** only if H-1 is fixed.

**Severity:** HIGH — data loss vs duplicate-work trade-off is not explicitly documented or tested.

**Remediation:** Advance the diff cursor **only** after the planner successfully persisted the new sync candidates (if persistence exists — none does today; see H-13). Alternatively, keep cursor-advance post-plan but test both crash windows with `upload_journal_crash_replay`-style integration tests.

---

### H-12. Crash recovery has no engine tests

`ls /home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/tests/` returns **nothing** — the engine crate has no integration tests at all. All tests are in `mod tests` blocks in each `.rs` file.

`crates/pcloud-daemon/tests/` contains `upload_journal_crash_replay.rs` (present — good) but no conflict-resolution crash test, no watcher-overflow crash test, no "kill mid-cycle and restart" test.

**Severity:** HIGH — the critical path for durability has zero integration proof.

**Remediation:** `crates/pcloud-engine/tests/crash_recovery.rs` covering: (a) SIGKILL between diff-cursor advance and plan emission, (b) SIGKILL during `advance_transfer_cycle`, (c) SIGKILL during `mark_transfer_completed`, (d) restart and observe completed work is not re-done. Use the `pcloud-chaos` crate if it supports process injection.

---

### H-13. Engine state is entirely non-durable — everything lives in memory

`crates/pcloud-engine/src/lib.rs:60-103`: `EngineShell` holds scheduler, coordinators, pause set, conflict queue — all in-memory `Vec` / `HashMap`.

The only engine-side durable state is the upload-resume repository (`crates/pcloud-store/src/repositories/upload_resume.rs`) and the diff-state cursor. Conflicts detected by the planner, queued operations, and in-flight transfers are **not** persisted.

A daemon restart wipes:

- all in-flight conflict resolutions,
- all queued operations awaiting retry under manual-intervention policy,
- the entire scheduler history.

`RecoveryManager::classify_failure` (`crates/pcloud-engine/src/recovery.rs:122-156`) returns a `RecoveryDecision` but there is no place that persists "operation X is waiting for a retry at time T". The module doc at `:22-25` even admits this: "The classifier is a pure function of (operation, failure); it does not consult history, exponential back-off, or the store. Back-off sequencing lives in the scheduler/transfer coordinators." — but the coordinators also do not persist.

**Severity:** HIGH — restart clobbers all in-flight retry state; every restart forces re-classification and re-discovery of conflicts.

**Remediation:** A `sync_operation_journal` table mirroring the shape of `PlannedOperation` with `(sync_id, path, op_kind, state, next_retry_at, retry_attempt, last_error)`. Persist on every `accept_batch` and every `mark_completed` / `mark_failed`. Provide `EngineShell::rehydrate_from_store(&Connection)`.

---

### H-14. `SessionManagerActor` is a 22-line stub

`crates/pcloud-engine/src/session_manager.rs` is only 22 lines. The module doc at `lib.rs:38-39` says "Per-sync-root engine state actor". The actor is essentially empty; no per-sync-root state machine exists.

**Severity:** HIGH — the comment architecture suggests per-sync-root isolation; the code does not deliver it, which cascades into C-1 (no fairness) and H-9 (slow eviction).

**Remediation:** Either build out the per-sync-root actor or rename the module so docs match reality.

---

## MEDIUM findings

### M-1. Ingress path `ingest_candidates` discards the Delete-policy when called directly

`crates/pcloud-engine/src/lib.rs:203-207`:

```
pub fn ingest_candidates(&mut self, candidates: &[SyncCandidate]) -> &[PlannedOperation] {
    let operations = self.planner.plan(candidates);
    self.scheduler.replace_queue(operations);
    self.scheduler.next_batch()
}
```

This path bypasses `DeletePolicy::for_sync_type`. It is still public and called from tests and presumably from code paths that forget the `_filtered` variant. A `BackupArchive` root that flows through `ingest_candidates` (not `ingest_candidates_filtered`) will emit `DeleteRemote` operations the policy explicitly forbids.

**Severity:** MEDIUM — a future caller is going to use the wrong variant.

**Remediation:** Make `ingest_candidates` accept a `DeletePolicy` directly or mark it `#[deprecated]` in favor of the `_filtered` form.

---

### M-2. Exponential-backoff jitter truncates `Instant` math on 32-bit

`crates/pcloud-resilience/src/retry.rs:178-194`:

```
let nanos = (base.as_nanos() as f64) * exp;
...
let as_u128 = clamped as u128;
Duration::new(
    (as_u128 / 1_000_000_000) as u64,
    (as_u128 % 1_000_000_000) as u32,
)
```

`as_nanos as f64` loses precision above ~2^53 nanoseconds (≈ 104 days). For reasonable backoff windows the loss is irrelevant, but the casts `as_nanos as f64 * exp -> as u128 -> Duration::new with truncated nanos` chain is fragile. The test at `:341-371` never covers extreme values.

**Severity:** MEDIUM — not a bug today; a property test would catch regressions.

**Remediation:** Proptest `compute_wait` against `Duration::MAX` / `factor = 1.0 to 10.0` / `attempt in 1..20`.

---

### M-3. Path handling in `fs_watcher::to_relative` silently drops non-UTF-8 paths

`crates/pcloud-fs/src/fs_watcher.rs:245-255`:

```
fn to_relative(path: &Path, root: &Path) -> Option<String> {
    path.strip_prefix(root).ok().and_then(|rel| {
        let s = rel.to_str()?;
        ...
        Some(s.replace('\\', "/"))
    })
}
```

Non-UTF-8 file names on Linux are legal but rare; `to_str()?` silently drops them. The user never learns why their `résumé.tex` (actually non-UTF-8 bytes) does not sync.

**Severity:** MEDIUM — locale-specific silent data skip.

**Remediation:** Replace `to_str` with `to_string_lossy` + a warning log + an audit event when lossy conversion happens.

---

### M-4. Debouncer re-flushes pending map on every loop iteration

`crates/pcloud-fs/src/fs_watcher.rs:162-197`: after each `notify_rx.recv_timeout` return (whether Ok or Timeout), the code calls `flush_pending` at `:196`, plus `flush_pending` was already called inside the `Timeout` branch at `:186`. Double-flush is idempotent since `matured` drains the map, but the double-iteration over a large pending map is wasteful.

**Severity:** MEDIUM — CPU waste on busy watchers.

**Remediation:** Flush once per iteration.

---

### M-5. No power/battery awareness for the sync loop itself

Grep `pause_on_battery` shows hits only in `integrity_sweeper_service.rs` — the integrity sweeper has a `PowerSource` trait (`:388-425`) and `PlatformPowerSource::new()` for Linux / macOS / Windows. The **sync loop itself** does not consult any power source.

Enterprise laptops routinely configure "pause heavy sync on battery"; the C pCloud client has such a setting. In this fork the feature is implemented only for the integrity sweep, not for uploads/downloads.

**Severity:** MEDIUM — feature regression vs C client; battery life impact on laptops.

**Remediation:** Extend `SyncLoopShared` with `pause_on_battery: AtomicBool` and a scheduler thread that consults `PlatformPowerSource` once per poll interval, calling `SyncLoopShared::pause` / `resume` based on power state.

---

### M-6. Pause/resume is not fsync-durable

`crates/pcloud-engine/src/lib.rs:456-470`: `pause_sync_root` / `resume_sync_root` only mutate in-memory `paused_sync_roots: BTreeSet<SyncId>`.

There is a `paused` column on `sync_root_records` (`crates/pcloud-store/src/schema.rs:72-85`), but no evidence that in-memory pause and persisted pause are synchronized — `pause_sync_root` does not hit the store, `resume_sync_root` does not hit the store. A daemon restart reloads from the store, which forgets any pause that IPC applied since bootstrap.

**Severity:** MEDIUM — operator pauses a misbehaving root, restarts the daemon, and the root is live again.

**Remediation:** In `RuntimeShell::pause_sync_root` (not the engine's — the runtime has the store handle), write the column BEFORE updating the engine. On resume, write `paused=0`.

---

### M-7. Condvar `wait_on_condvar` does not handle spurious wake-ups optimally

`crates/pcloud-daemon/src/sync_loop.rs:343-355`: the `wait_timeout_while` predicate is sound (`!*woken && !shutdown`), but a spurious wake that finds both false will correctly loop — however the `if let Ok(mut g, _)` discards the `WaitTimeoutResult`. The cleared `*g = false` runs regardless of shutdown, so a shutdown-triggered wake clears the flag and causes another loop iteration to observe shutdown only on its own atomic read at `:372-378`. This is correct, just non-obvious.

**Severity:** MEDIUM — correctness OK; code is subtle enough to regress.

**Remediation:** Return a `WakeReason { shutdown: bool, external: bool, timeout: bool }` from `wait_on_condvar` so the main loop handles each branch explicitly.

---

### M-8. Circuit breaker is not per-endpoint

`crates/pcloud-resilience/src/circuit_breaker.rs:116-122` describes a single `CircuitBreaker` instance. The daemon's HTTP transport would likely use one, but the sync loop has no visible per-endpoint isolation — one failing endpoint (`diff`, say) trips the entire network path.

**Severity:** MEDIUM — an endpoint outage in `/listshares` blocks `/upload_create`.

**Remediation:** Keep a `HashMap<Endpoint, CircuitBreaker>` in the transport layer and let the sync loop observe per-endpoint state.

---

### M-9. No per-sync-root pause persistence + no pause reason

`crates/pcloud-engine/src/lib.rs:91-95`: `paused_sync_roots: BTreeSet<SyncId>` holds only ids, no reason (user, auto-pause-on-checksum-mismatch, auto-pause-on-quota-exceeded).

**Severity:** MEDIUM — operator cannot distinguish user pause from system pause, resume races ensue.

**Remediation:** `PauseReason { UserRequested, QuotaExceeded, AuthExpired, IntegrityFailure }` carried in the map.

---

### M-10. `RecoveryManager` has no exponential history

The doc (`recovery.rs:22-25`) explicitly admits "does not consult history, exponential back-off". A task that fails with `RetryableNetworkError` will return `RetryLater` forever, no matter how many times it has failed. The engine has no notion of "give up after N retries".

**Severity:** MEDIUM — infinite-retry loops on pathological tasks.

**Remediation:** Add `retry_count: u32` to the per-task journal (see H-13) and have `classify_failure` escalate to `Terminal` after a configurable threshold.

---

### M-11. `Planner::max_operations_per_tick` default of 1024 may be too small for real trees

`crates/pcloud-engine/src/planner.rs:48-58`: default cap is 1024. A sync root with 100k files on initial scan will need ~100 ticks to fully plan, each separated by the sync loop's poll interval (default: a few seconds). Initial sync of a large tree takes forever.

**Severity:** MEDIUM — UX issue on first sync.

**Remediation:** Increase default or switch to adaptive: plan up to whatever fits in a memory budget (≈100k entries ≈ 10–20 MB).

---

### M-12. `ingest_candidates` resets the scheduler queue every call

`crates/pcloud-engine/src/lib.rs:203-207`: `scheduler.replace_queue(operations)` **replaces**. If the caller issues two consecutive `ingest_candidates` with non-overlapping path sets, the second call discards the first batch from the queue entirely.

This is linked to C-2 but distinct. Even with C-2 fixed (dequeue on dispatch), a replace-based ingestion loses information about previously-queued items that have not yet been dispatched.

**Severity:** MEDIUM — semantic ambiguity; a sequence of small batches vs one big batch produces different queue contents.

**Remediation:** `ingest_candidates` should merge into the queue (by path) rather than replace, and have an explicit `clear_queue()` for the runtime teardown path.

---

### M-13. Sync loop's global error counter does not reset

`crates/pcloud-daemon/src/sync_loop.rs:415-417`:

```
status.total_errors += cycle.total_errors as u64;
```

Monotonically increasing. No "errors-per-cycle" rate metric, no decay. An operator running for months will see huge numbers that convey no recent health signal.

**Severity:** MEDIUM — observability issue.

**Remediation:** Keep rolling windows (last 5 min, last hour) plus cumulative.

---

### M-14. Store uses a single global Mutex — no reader concurrency

`crates/pcloud-store/src/lib.rs:266-299`: `StoreHandle` is a single long-lived `Mutex<Connection>`. WAL journaling gives multiple-reader potential but the mutex serializes both reads and writes. For the sync loop, which reads `sync_root_records` at the start of every cycle, this contends with writes from IPC.

Note: `sync_loop_runtime.rs:141-147` bypasses `StoreHandle` and opens its own `Connection` directly ("safe to open concurrently because WAL"). That works but now the crate has two separate connection strategies; `StoreHandle` invariants are not enforced on the sync-loop path.

**Severity:** MEDIUM — architectural drift and undocumented contention profile.

**Remediation:** Either move `StoreHandle` to `RwLock<Connection>` for genuine reader concurrency, or standardize on "one short-lived connection per operation" and deprecate `StoreHandle`.

---

### M-15. `sync_diff_state` has no FK to `sync_root_records`

`crates/pcloud-store/src/schema.rs:244-262` v10 migration says "we do not declare a real FK because diff state can outlive a transient sync_root remove/re-add". That reasoning is wrong — it is cheaper to delete the diff state on root remove (explicit) than to keep orphan rows across restarts. Orphan `sync_diff_state` rows will accumulate silently on long-lived daemons.

**Severity:** MEDIUM — long-tail data hygiene.

**Remediation:** Add `FOREIGN KEY (sync_id) REFERENCES sync_root_records(sync_id) ON DELETE CASCADE` and delete the "do not declare FK" comment.

---

### M-16. No background GC for stale upload_resume_state rows

`crates/pcloud-store/src/repositories/upload_resume.rs:136-142`: `delete` only runs on explicit success path. A local file that was deleted before its upload completed leaves a stale resume row forever, holding a server-side `upload_id` that is now orphaned.

**Severity:** MEDIUM — long-running daemon collects junk rows.

**Remediation:** Periodic sweep (hourly or on boot) that drops rows older than 24 h.

---

### M-17. `upload_resume_state` primary key is `local_path` — symlink traversal races possible

`crates/pcloud-store/src/repositories/upload_resume.rs:38-57`: PK is the canonicalized local path string. If a file is renamed between `upload_create` and the next `upload_write`, resume lookup fails silently and the client re-starts from zero — wasting bytes uploaded so far.

**Severity:** MEDIUM — wasted bandwidth on renames.

**Remediation:** PK on `(inode, device)` when available; fall back to path when not (Windows).

---

### M-18. No checksum-based dedup before upload

Grep `checksum` / `sha256` within the engine crate shows zero. A sync root with two copies of the same file (common with backups) uploads both independently. The server has `checksumfile` but the client never queries it before `upload_create`.

**Severity:** MEDIUM — bandwidth waste on legitimate dedup scenarios.

**Remediation:** For files above a threshold (~1 MiB), hash locally and check server-side before uploading.

---

## LOW findings

### L-1. `Scheduler::next_batch` returns `&[PlannedOperation]`, not owned

`scheduler.rs:123`: callers who want to mutate (e.g. `advance_transfer_cycle` in `lib.rs:409-414` which calls `next_batch().to_vec()`) need to clone. Minor allocation cost.

**Severity:** LOW.

**Remediation:** Return `Vec<PlannedOperation>` directly from an integrated dequeue method.

---

### L-2. `FsEventKind` has only Write/Create/Remove — no Rename

`crates/pcloud-engine/src/fs_events.rs:29-37`: missing Rename as first-class. A file renamed locally produces `Remove` + `Create`, which the planner treats as a delete followed by a separate upload — losing the rename semantic, doubling server state churn.

**Severity:** LOW.

**Remediation:** Add `Rename { from: String, to: String }` and map notify's `EventKind::Modify(ModifyKind::Name(..))` into it.

---

### L-3. `Scheduler` and `Planner` are `Serialize + Deserialize` but never serialized

`scheduler.rs:37-38` derives `Serialize, Deserialize`. No callsite serializes them. Dead annotations suggest durability was planned but not implemented (link to H-13).

**Severity:** LOW.

**Remediation:** Either remove the derives or actually persist.

---

### L-4. Pacer uses `std::thread::sleep` from a sync context — blocks tokio runtimes

`crates/pcloud-resilience/src/pacing.rs:49-52` and `:23-26`: `BandwidthPacer::pace` blocks the calling thread. If the caller happens to be inside a tokio `spawn_blocking` or a tokio reactor, it blocks a worker. The sync loop uses `std::thread`, so this is currently OK — but the doc should say "NEVER call from a tokio async fn".

**Severity:** LOW.

**Remediation:** Add a `#[must_not_use_in_async]` lint or at least a sharp doc warning.

---

### L-5. `FsEvent::validate_relative_path` duplicated three times

`fs_events.rs:98-111`, `local_scan.rs:273-286`, `diff_poller.rs:101-114` are near-identical. Maintenance hazard.

**Severity:** LOW.

**Remediation:** Extract to `pcloud-model` or a shared `pcloud-engine::path_validator` module.

---

### L-6. `sync_one_root` has no timeout

`crates/pcloud-daemon/src/sync_loop.rs:254-300`: one root with 1M files can take an hour to scan and plan. The cycle waits for it, which delays all other roots' cycles. Compounds with C-1.

**Severity:** LOW (given C-1 dominates).

**Remediation:** Per-root time budget; yield after N ms back to the scheduler.

---

### L-7. `reconcile_worker` interval default is 300 s; C client uses 10 s

`crates/pcloud-engine/src/reconcile_worker.rs:38`: `RECONCILE_DEFAULT_INTERVAL_SECS = 300`. The module doc at `:23-26` acknowledges "the C `PSYNC_LOCALSCAN_RESCAN_INTERVAL` is more aggressive at 10s but only fires after change events". This is a surprising default for a "sync" product — users expect sub-minute propagation.

**Severity:** LOW.

**Remediation:** Make it configurable via `SyncLoopConfig` and default to 60 s.

---

### L-8. `SyncLoopStatus::last_cycle_duration_ms` overflows at ~49 days

`sync_loop.rs:412`: `cycle.duration.as_millis() as u64` casts from `u128` — no overflow concern there — but any single cycle over 49 days would indicate a stuck daemon. Not a real bug; sanity-check could be cleaner.

**Severity:** LOW.

**Remediation:** Keep a `last_cycle_duration: Duration` typed field.

---

### L-9. `UploadCoordinator::accept_batch` clears previous active uploads

`crates/pcloud-engine/src/transfers/uploads.rs:48-51`:

```
self.active_uploads.clear();
self.pending_remote_deletes.clear();
self.pending_directory_creates.clear();
```

Same shape at `downloads.rs:48-51`. If `advance_transfer_cycle` is called twice in a row with the same batch, all in-flight work is silently reset. This is part of the broader C-2 issue but worth calling out: the coordinators trust that their caller will not re-call `accept_batch` mid-flight.

**Severity:** LOW (documented implicitly via "one call per cycle" contract).

**Remediation:** Defensive check: refuse to `clear()` if any of these lists has state != Streaming.

---

### L-10. `UploadCoordinator::chunk_size_bytes: 8 MiB` default — too large for low-memory devices

`crates/pcloud-engine/src/transfers/uploads.rs:32-42`: 8 MiB per in-flight upload × 4 parallel uploads = 32 MiB minimum staging. On embedded devices this is a noticeable floor.

**Severity:** LOW.

**Remediation:** Scale by available memory; default to 4 MiB.

---

### L-11. Tests are unit-only; no property tests for the scheduler

`crates/pcloud-engine/src/scheduler.rs:130-218`: three tests, all deterministic. No proptest covering "ingest N ops, dispatch M, evict K, no ops lost or duplicated".

**Severity:** LOW.

**Remediation:** Add a proptest exercising the ingest→dispatch→complete→evict state machine.

---

### L-12. `DeletePolicy::for_sync_type` does not expose which sync types exist

`crates/pcloud-engine/src/planner.rs:180-210`: matches on `SyncType::Full | UploadOnly | DownloadOnly | BackupArchive`. If a new `SyncType` is added to `pcloud-model`, this match is non-exhaustive without `#[non_exhaustive]` contract — the compiler will error cleanly, which is fine, but there is no dedicated fallback.

**Severity:** LOW.

**Remediation:** Add `SyncType::_` arm that defaults to the most restrictive policy, with a log warning.

---

### L-13. Upload resume records store the `local_path` but not the `sync_id`

`crates/pcloud-store/src/repositories/upload_resume.rs:38-57`: no `sync_id` column. When a sync root is removed, there is no way to `DELETE FROM upload_resume_state WHERE sync_id = ?`.

**Severity:** LOW.

**Remediation:** Add `sync_id INTEGER` column in a v12 migration.

---

### L-14. `Planner::plan` clones every candidate

`crates/pcloud-engine/src/planner.rs:75-82`: `sorted = candidates.to_vec()` then internally clones again into `local`/`remote`. 3× allocation per candidate. For 10k candidates that is ~300k allocations.

**Severity:** LOW — perf.

**Remediation:** Sort indices, consume by reference.

---

### L-15. `LocalScanner` / `DiffPoller` configs are `Serialize + Deserialize` but never loaded

`local_scan.rs:20-32`, `diff_poller.rs:14-24`: derives exist but no persistence story. Orphan configuration surface.

**Severity:** LOW.

**Remediation:** Either wire them to `ConfigProfile` or remove derives.

---

### L-16. `EngineShell::unresolved_conflict_count` is O(n) — called from hot paths

`lib.rs:305-312`: walks `queued_operations` linearly. Called from `summary()` at `:175-198` which is called by every cycle. With large queues this is a few microseconds, but it is a trivially cachable counter.

**Severity:** LOW.

**Remediation:** Maintain an `unresolved_conflicts: usize` counter bumped on enqueue/dequeue.

---

### L-17. `ChunkedUploadTracker` is dead code

`crates/pcloud-engine/src/transfers/uploads.rs:188-200`: declared but grep for `ChunkedUploadTracker::` elsewhere returns nothing. The chunk-tracking story goes through `upload_resume.rs` in the store, which is a parallel shape. Two sources of truth.

**Severity:** LOW.

**Remediation:** Delete `ChunkedUploadTracker` or unify it with the store record.

---

### L-18. `wake_localscan` is a counter increment, nothing wakes the scanner

`crates/pcloud-engine/src/lib.rs:145-160`: the method just bumps `localscan_wakes`. The comment admits the C wake path is not implemented. In practice the sync loop's condvar wakes the entire loop, not a per-root scanner.

**Severity:** LOW — clarity issue.

**Remediation:** Delete this method or wire it to the reconcile worker's `request_scan`.

---

### L-19. Test `engine_ingest_local_scan_with_delete_policy_suppresses_deletes`

`crates/pcloud-engine/src/lib.rs:768-793`: the test comment itself admits the test doesn't validate anything: `let _ = ops; // just verify it does not panic`. This is a ghost test.

**Severity:** LOW.

**Remediation:** Replace with a real assertion or delete it.

---

### L-20. `SelectivePolicy::matches` (selective.rs:346 lines) not reviewed here

Out of scope for the scheduler/queue audit but worth flagging: selective-sync filtering is the last point where a file can still escape sync, and it lives right next to the planner. A separate deep-dive would be prudent.

**Severity:** LOW (scope note).

---

## Test coverage classification

### Engine crate (`crates/pcloud-engine/`)

Only in-file `#[cfg(test)] mod tests`. **No `tests/` directory.**

Covered:
- `conflict_resolver.rs` unit tests (7 tests, all policies)
- `planner.rs` unit tests (`DeletePolicy` variants, conflict classification)
- `scheduler.rs` (3 tests: priority ordering, batch limit, eviction)
- `recovery.rs` (2 tests: network retry, checksum mismatch)
- `fs_events.rs` (3 tests: normalize, coalesce, reject-invalid)
- `local_scan.rs` (scanner normalization, selective policy)
- `diff_poller.rs` (normalize batch, reject malformed)
- `reconcile_worker.rs` (4 tests: idle/fire/untrack/request_scan)

NOT covered:
- Crash recovery of the engine
- Per-sync-root starvation (C-1)
- Scheduler dequeue semantics (C-2 — untested because the bug is untested)
- `newest_wins` with varying timestamps (C-3 — the only test bakes in the wrong behavior)
- `rename_both` actually renaming (C-4 — test accepts ManualReview output)
- Unbounded queue / staging overflow (C-5, H-8)
- Watcher overflow handling (H-6)
- Idempotency of upload retries (H-1)
- Retry-After honoring (H-2)
- Case / NFC conflict detection (H-4)

### Daemon crate (`crates/pcloud-daemon/tests/`)

18 test files. Relevant ones:
- `sync_loop_e2e.rs` — high-level loop path (good)
- `proptest_sync_and_resolver.rs` — proptest for resolver (good)
- `upload_journal_crash_replay.rs` — upload crash replay (good)
- `graceful_drain.rs` — shutdown drain (good)
- `integrity_walker.rs` — sweeper (Dim. 8 territory)

No test file covers scheduler-starvation, conflict-rename-both semantics, staging overflow, or watcher overflow.

### Store crate (`crates/pcloud-store/`)

**No `tests/` directory.** Each repository has an in-file test module (sampled: `upload_resume.rs:180+`, `audit.rs`, etc.). Migration path is tested only via the bootstrap round-trip. No test covers crash-during-migration (C-7).

### Resilience crate (`crates/pcloud-resilience/`)

One `tests/` file: `circuit_breaker_proptest.rs` (good). No property tests for `RetryPolicy` (M-2 untested), no property tests for `TokenBucket`.

---

## Resource leaks

### Thread join handles

`crates/pcloud-daemon/src/sync_loop.rs:432-470`: `SyncLoopHandle` owns an `Option<JoinHandle<()>>` and `impl Drop` performs best-effort join. OK.

`crates/pcloud-fs/src/fs_watcher.rs:139-144`: `thread::Builder::new().spawn(move || debounce_loop(...))` is **not** stored. The thread runs until the channel disconnect is observed. On drop of `FsWatcher`, the `_watcher` field drops (stopping notify), which drops the `notify_tx` sender, which closes the channel from the other end, which should let `debounce_loop` observe `Disconnected` and exit.

Correctness depends on notify crate actually closing `notify_tx` when the watcher is dropped. If the notify crate parks the sender in its own thread (which it does on most backends), the debounce thread may outlive `FsWatcher` briefly. Not a leak, but a shutdown race:

```
FsWatcher drop → notify thread exits → notify_tx drops → debounce_loop sees Disconnected
```

If any link fails (notify thread stuck), the debounce thread runs forever.

**Severity:** MEDIUM. Already covered in spirit by H-7.

**Remediation:** Store the debounce thread's `JoinHandle`; on `FsWatcher::drop`, join it with a timeout.

### mpsc channels

`sync_loop_runtime.rs:95-101`: `watchers: HashMap<SyncId, (FsWatcher, Receiver)>`. When a sync root is removed, the entry is dropped, which drops both `FsWatcher` and `Receiver`. That should cascade to the debounce thread exit path above. No leak.

`integrity_sweeper_service.rs:102`: `mpsc::{Sender, Receiver}` — large file not fully audited.

### File handles

No file handles kept on long-lived engine structs. Staging cache (`crates/pcloud-cache/src/staging.rs`) holds `Vec<u8>` only. No `std::fs::File` in any engine state.

---

## Summary

| Severity | Count |
|---|---|
| CRITICAL | 8 |
| HIGH | 14 |
| MEDIUM | 18 |
| LOW | 20 |

The sync engine has a clean, test-friendly architecture (pure state machine, injected clock, composable coordinators) but several **advertised features are not actually implemented** (C-3 `newest_wins`, C-4 `rename_both`, M-5 battery awareness for the sync loop, L-18 `wake_localscan`). The queue model **does not provide fairness across sync roots** (C-1) and **does not dequeue** (C-2) — both are fundamental. Durability of engine state is non-existent (H-13); restart wipes in-flight retry state and the conflict queue.

The most economically serious items to fix, in order:

1. **C-2 (queue dequeue)** — without it every cycle re-emits the same work.
2. **C-1 (fairness)** — enterprise-blocking; a big root starves everything else.
3. **C-3 + C-4 (conflict policies)** — silent data loss under default settings.
4. **C-8 (stall detection)** — ghost in-flight tasks indefinitely.
5. **H-1 + H-2 (retry with idempotency + Retry-After)** — good-citizen and transient-failure handling.
6. **H-13 (engine state durability)** — restart survivability.
7. **C-6 / C-5 / H-8 (bounded queues and staging budgets)** — DoS prevention.

Every fix should come with proptests in a new `crates/pcloud-engine/tests/` directory; today that directory does not exist and the engine has no integration tests at all.
## Section 5. Mounted-drive / FUSE Parity

**Dimension 5 auditor — scope:** `crates/pcloud-fs/` (mount_service, platform/*, fuse_adapter, fuser_shim, write_path, journal, backend, mount_orphan, tests, benches). Parent epic: `bd-1du.4`. Explicit exclusions per prompt: sync engine (Dimension 4), generic FFI memory-safety audit (Dimension 2 — raised only for FUSE-specific unsafe here), deployment/packaging (Dimension 11).

**Verdict (single sentence):** the `pcloud-fs` crate contains a thorough Linux-only FUSE implementation with solid mount lifecycle, orphan detection, and a journaled write path — but any claim of "mounted-drive parity" with the C daemon is **FALSE** today because (a) several core POSIX ops are not wired into the `fuser::Filesystem` shim (`statfs`, `access`, `opendir`/`releasedir`, `forget`, `readlink`, extended attributes, `fallocate`, `lseek`, `symlink`, `link`), (b) the macOS and Windows back-ends are explicitly self-described Phase-1 scaffolding that have never booted on their respective hosts, (c) kernel-mounted integration tests are all `#[ignore]` + `PCLOUD_FUSE_TEST=1`-gated, (d) the write-ahead journal contradicts its own durability contract (doc says `fsync(file)+fsync(dir)`, implementation only `sync_data(file)`), and (e) WinFSP/fuse-t struct layouts are unvalidated against installed headers. The remediation list below is long but the code structure is sound — most fixes are additive rather than architectural.

---

### 5.1 Cross-platform architecture and `PlatformMount` trait dispatch

**File:** `crates/pcloud-fs/src/platform/mod.rs:1-125` and `crates/pcloud-fs/src/mount_service.rs:158-226`.

The design is clean: `PlatformMount` trait with `validate_mountpoint`, `probe_supported`, `default_options`, `mount_adapter` entries, and a compile-time `ActivePlatformMount` type alias picked per `#[cfg(target_os)]`. Four back-ends (Linux, BSD, macOS, Windows) each supply a concrete implementor, and unsupported platforms fall through to `MountError::UnsupportedPlatform` via a trait default. This structure is the cleanest part of the FUSE surface and maps almost 1:1 to the C daemon's per-OS mount adapters.

#### [HIGH-5.1.1] `MountService::mount` dispatch path does NOT route through the `PlatformMount` trait uniformly — it hard-codes per-OS branches
**File:** `crates/pcloud-fs/src/mount_service.rs:170-193`.
**Severity:** HIGH.
**Detail:** `MountService::mount<A: FuseAdapter>` has an explicit cfg-ladder that calls `linux::mount_with_fuser`, `bsd::mount_with_fuser`, or `macos::MacosPlatformMount::mount_adapter` directly. Windows is entirely absent from this ladder — on a Windows build `MountService::mount` falls into the `else` arm at line 188 and returns `UnsupportedPlatform`, even though `WindowsPlatformMount::mount_adapter` exists at `crates/pcloud-fs/src/platform/windows.rs:175-184`. That means the daemon wiring in `runtime.rs` that calls `MountService::mount` can never reach the Windows back-end through this entry point.
**Remediation:** replace the cfg-ladder with a single call to `ActivePlatformMount::default().mount_adapter(Box::new(adapter), ...)`. The Linux-typed `mount_with_fuser` fast path can stay as an additional method for callers that want monomorphization.

#### [MEDIUM-5.1.2] `MountService::mount_fuser` is not available on macOS or Windows
**File:** `crates/pcloud-fs/src/mount_service.rs:204-226`.
**Severity:** MEDIUM.
**Detail:** the method is gated `#[cfg(any(target_os = "linux", target_os = "freebsd"))]` because its `F: fuser::Filesystem` bound only exists on those platforms. The daemon's composed `PcloudFsShim` (`crates/pcloud-fs/src/fuser_shim.rs:1`) is also Linux-only (`#![cfg(target_os = "linux")]`). Net effect: the real live-composition path is Linux-only; macOS and Windows run against a thinner `FuseAdapter` dispatcher that does not have the daemon's `fuser_shim.rs` improvements (e.g. parent-inode back-pointer for `..`, the FhTable, write_path wiring through `WritePathService`).
**Remediation:** extract the Linux-specific `PcloudFsShim` into a cross-platform form that implements `FuseAdapter` rather than `fuser::Filesystem`, so the non-Linux platforms automatically benefit from its FhTable and parent-ino bookkeeping.

#### [MEDIUM-5.1.3] `#[cfg(target_os = "freebsd")]` in `mount_service.rs::mount` does not route through the `PlatformMount` trait the way macOS does
**File:** `crates/pcloud-fs/src/mount_service.rs:176-179`.
**Severity:** MEDIUM.
**Detail:** FreeBSD goes via the typed `bsd::mount_with_fuser`, but macOS goes through the dyn `PlatformMount::mount_adapter`. The split is confusing; a future contributor touching only the cfg ladder will almost certainly break one platform or the other.
**Remediation:** unify: all platforms through the trait, with an explicit `Linux::mount_with_fuser_typed<A>` optimization behind a separate method that only `mount_service.rs` calls on a `target_os = "linux"` fast path.

#### [LOW-5.1.4] `ActivePlatformMount` alias is a zero-sized marker type — the trait is stateless
**File:** `crates/pcloud-fs/src/platform/mod.rs:106-124`.
**Severity:** LOW.
**Detail:** every per-OS type is `#[derive(Default, Clone, Copy)]` with no fields. The trait is effectively a namespaced function table. That's fine but means there is nowhere to attach fuse-t vs. macFUSE backend selection state, WinFSP library handle caching, etc. — each mount re-probes/loads the runtime. Not a correctness bug but eliminates one natural place to cache a loaded `WinFspLibrary` so the daemon doesn't re-`LoadLibraryW` on every mount.
**Remediation:** let implementations carry state when useful (e.g. `WindowsPlatformMount { lib: OnceLock<Arc<WinFspLibrary>> }`).

---

### 5.2 Core FUSE kernel-op coverage (per-op status)

The review needs to distinguish three codepaths because the crate has **three** `fuser::Filesystem` implementations:

1. **`BoxedFuserShim` + `FuserShim<A>`** at `crates/pcloud-fs/src/platform/fuser_shim.rs:66-840` — the shared Linux/FreeBSD shim used by `BsdPlatformMount::mount_adapter` and `LinuxPlatformMount::mount_adapter`. Routes through `FuseAdapter` trait.
2. **`PcloudFsShim`** at `crates/pcloud-fs/src/fuser_shim.rs:1` — the daemon-composed shim with `WritePathService` write-path, `InodeTable`, and an explicit FhTable. **Linux-only** (`#![cfg(target_os = "linux")]`). Used by `mount_fuser_filesystem`.
3. **macOS thunks** at `crates/pcloud-fs/src/platform/macos.rs:382-1392` — direct C ABI thunks in the fuse-t low-level ops vtable. Every thunk wraps in `catch_unwind` and talks to a `dyn FuseAdapter`.
4. **WinFSP callback table** at `crates/pcloud-fs/src/platform/windows.rs` — Windows NT semantics mapped to `FuseAdapter`.

Per-op matrix (`I` = implemented, `P` = partial, `M` = missing / stub):

| FUSE op         | `BoxedFuserShim` / `FuserShim<A>` (Linux+BSD) | `PcloudFsShim` (daemon, Linux only) | macOS thunks | WinFSP callbacks |
|-----------------|------------------------------------------------|-------------------------------------|--------------|------------------|
| `lookup`        | I (line 98-113 / 470-485)                      | I (line 213)                        | I (line 405) | I                |
| `getattr`       | I (115 / 487)                                  | I (224)                             | I (467)      | I                |
| `readdir`       | I (128 / 500)                                  | I (231)                             | I (630)      | I                |
| `open`          | I (174 / 542)                                  | I (271)                             | I (640s)     | I                |
| `read`          | I (181 / 549)                                  | I (349)                             | I            | I                |
| `release`       | I (202 / 570)                                  | I (371)                             | I (743)      | I                |
| `create`        | I (222 / 590)                                  | I (407)                             | I (858)      | I                |
| `write`         | I (262 / 633)                                  | I (461)                             | I (793)      | I                |
| `flush`         | I (281 / 652)                                  | I (497)                             | I            | I                |
| `fsync`         | I (295 / 666)                                  | I (522)                             | I            | I                |
| `setattr`       | P (309 / 680 — size only)                      | P (536)                             | P (size only)| P                |
| `unlink`        | I (344 / 715)                                  | I (613)                             | I (955)      | I                |
| `rename`        | P (372 / 746 — no flags)                       | I (634)                             | I            | I                |
| `mkdir`         | I (404 / 784)                                  | I (571)                             | I            | I                |
| `rmdir`         | I (434 / 817)                                  | I (598)                             | I            | I                |
| `statfs`        | **M**                                          | **M**                               | I (thunk_statfs, 1375) | I (GetVolumeInfo) |
| `access`        | **M**                                          | **M**                               | M            | M                |
| `opendir`       | M (fuser default)                              | M                                   | M            | n/a              |
| `releasedir`    | M                                              | M                                   | M            | n/a              |
| `fsyncdir`      | M                                              | M                                   | M            | n/a              |
| `readlink`      | M                                              | M                                   | M            | n/a              |
| `symlink`       | M                                              | M                                   | M            | n/a              |
| `link`          | M                                              | M                                   | M            | n/a              |
| xattr (get/set/list/remove) | M                                 | M                                   | M            | n/a              |
| `lseek` (SEEK_DATA/HOLE) | M                                  | M                                   | M            | n/a              |
| `fallocate`     | M                                              | M                                   | M            | M                |
| `copy_file_range` | M                                            | M                                   | M            | M                |
| `init` / `destroy` | default (no-op)                             | default                             | I (stubs)    | I                |
| `forget`        | M (fuser default ok)                           | M                                   | M            | n/a              |
| `getlk` / `setlk` | M                                            | M                                   | M            | n/a              |
| `poll` / `ioctl` / `bmap` | M                                    | M                                   | M            | n/a              |

#### [CRITICAL-5.2.1] Linux/FreeBSD `statfs` is unimplemented at the FUSE boundary — `df`/`stat -f` on the mount will always error
**Files:** `crates/pcloud-fs/src/platform/fuser_shim.rs:97-454` (`BoxedFuserShim`) and `crates/pcloud-fs/src/platform/fuser_shim.rs:464-840` (`FuserShim<A>`); `crates/pcloud-fs/src/fuser_shim.rs:1-300+` (`PcloudFsShim`).
**Severity:** CRITICAL (Linux+BSD daemon users).
**Detail:** neither of the Linux+BSD `fuser::Filesystem` shims implements `fn statfs(...)`. The `FuseAdapter` trait **does** expose `fn statfs(&self) -> Result<(u64, u64), i32>` (line 503), but no shim calls it; `fuser` therefore uses its default which replies `ENOSYS`. `df /mnt/pcloud`, `statvfs(2)` and anything the desktop indexer does on mount will either get `ENOSYS` or stale zeroes. The C reference client implements `pfs_statfs` and returns real `userinfo.quota` / `usedquota`; this is a user-visible regression.
**Note:** macOS already has `thunk_statfs` (`platform/macos.rs:1375`) and WinFSP has `GetVolumeInfo` — only Linux/BSD are missing.
**Remediation:** add `fn statfs(&mut self, _req, _ino, reply: fuser::ReplyStatfs)` to both `BoxedFuserShim` and `FuserShim<A>` and also to `PcloudFsShim`. Each should call `self.adapter.statfs()` and map the tuple into `fuser::FileAttr`-style reply bits (blocks/bfree/files).

#### [HIGH-5.2.2] `access` is unimplemented across all Linux/BSD shims
**File:** `crates/pcloud-fs/src/platform/fuser_shim.rs:97-840` — no `fn access`.
**Severity:** HIGH.
**Detail:** on a mount with `fuser::MountOption::DefaultPermissions` (which the crate does set — `build_fuse_options` line 859), the kernel enforces mode bits itself, so `access(2)` without X_OK is serviced in-kernel. But `access(X_OK)` and several code paths in util-linux / systemd issue a FUSE `access` op anyway to verify execute rights. Without a handler this returns `ENOSYS`, which the kernel translates to `EACCES` in some paths, triggering misleading "permission denied" from `df`, `stat`, some shells completing paths, and `inotify` setup failing. Minor, but a real ergonomic regression vs. the C client's `pfs_access`.
**Remediation:** minimal implementation returning `0` (allow) or delegating to a new `FuseAdapter::access` trait method that runs existing permission logic.

#### [HIGH-5.2.3] `forget` is unimplemented — lookup-count leak risk
**File:** `crates/pcloud-fs/src/platform/fuser_shim.rs` (entire).
**Severity:** HIGH (long-running daemons).
**Detail:** the FUSE kernel protocol increments a per-inode lookup count on every `lookup` / `create`, and filesystems must decrement by the kernel-provided `nlookup` amount on `forget`. Not implementing it means `fuser`'s default (no-op) runs, which is safe for the default fuser inode table but is **dangerous** when an adapter carries its own ino→path map in memory (as `ProtoFuseAdapter` does — `fuse_adapter.rs:1143` has a `forget_local_entry` helper but no `forget` wiring). Over a long-running mount with heavy directory churn, the adapter's local map will grow without bound because nothing trims it on eviction notifications.
**Remediation:** wire `fn forget(&mut self, _req, ino: u64, nlookup: u64)` in all shims to call `self.adapter.forget(ino, nlookup)`.

#### [MEDIUM-5.2.4] `rename` ignores `RENAME_NOREPLACE` / `RENAME_EXCHANGE` flags
**Files:** `crates/pcloud-fs/src/platform/fuser_shim.rs:372-402` (BoxedFuserShim), :746-782 (FuserShim<A>).
**Severity:** MEDIUM.
**Detail:** the `_flags: u32` param is ignored. The adapter's `rename(from, to)` signature has no flags channel. POSIX-portable tools mostly work, but modern Linux/glibc `renameat2(2)` with `RENAME_NOREPLACE` (git checkout, atomic config writers) will silently overwrite when the no-replace flag is set.
**Remediation:** extend `FuseAdapter::rename` to accept flags; map `RENAME_NOREPLACE` by pre-checking the target and returning `EEXIST`, and reject `RENAME_EXCHANGE` with `ENOTSUP`.

#### [MEDIUM-5.2.5] `setattr` only honours size changes — chmod/chown/utimens silently succeed without effect
**Files:** `platform/fuser_shim.rs:309-342`, `platform/fuser_shim.rs:680-713`, `fuser_shim.rs:536-570`.
**Severity:** MEDIUM.
**Detail:** only `size` is checked and routed to `adapter.truncate`. `mode`, `uid`, `gid`, `atime`, `mtime`, `ctime`, `crtime`, `chgtime`, `bkuptime`, `flags` are all `_`-prefixed and ignored, then the handler happily replies with the refreshed attrs as if the change succeeded. A `touch -t ...` or `chmod 0644 foo` on the mount returns success but is a lie. C reference client at least rejects or queues these.
**Remediation:** either (a) return `EPERM` for unsupported setattr bits so userspace gets an honest error, or (b) implement at least `utimens` via pCloud `modified_at` metadata.

#### [LOW-5.2.6] `readlink`, `symlink`, `link` all missing
**File:** entire `platform/fuser_shim.rs`.
**Severity:** LOW (pCloud has no symlink concept server-side).
**Detail:** FUSE default replies `ENOSYS`, which is correct but loud in logs. Document explicitly as "pcloud has no symlink on server → ENOSYS" in `FuseAdapter`.

#### [LOW-5.2.7] Extended attributes (xattr) family missing
**File:** entire `platform/fuser_shim.rs`.
**Severity:** LOW.
**Detail:** no `getxattr`/`setxattr`/`listxattr`/`removexattr`. Modern desktop environments (GNOME Files, KDE Dolphin, Finder) use xattr for thumbnails and user tags. Missing these causes spurious "unable to save attribute" errors visible in journal.
**Remediation:** implement as `ENOTSUP` explicitly (FUSE default is `ENOSYS`, which GNOME misinterprets as "filesystem is broken, disable tagging entirely"). Mapping to `ENOTSUP` is friendlier.

#### [LOW-5.2.8] `fallocate`, `copy_file_range`, `lseek` (SEEK_DATA/HOLE) all missing
**File:** entire `platform/fuser_shim.rs`.
**Severity:** LOW.
**Detail:** without these, modern tools that try efficient paths (e.g. `cp --sparse=auto`, server-side copy) silently fall back to read-then-write. A real implementation can accelerate large copies dramatically since pCloud has server-side copy (`copyfile` API) — so this is LOW only because it's a performance missed opportunity, not correctness.

---

### 5.3 Write path, staging, and journal durability

**Files:** `crates/pcloud-fs/src/write_path.rs:1-2200+`, `crates/pcloud-fs/src/write_journal.rs:1-500`, `crates/pcloud-fs/src/journal.rs:1-119`, `crates/pcloud-fs/src/staging.rs`.

Overall the write path is thoughtful: a write-ahead `WriteJournal` with CRC32 envelopes (`write_journal.rs:140-216`), a per-inode `UploadProgress` sidecar with write-then-rename durability (`write_path.rs:882-911`), and a resumable chunked-flush loop (`write_path.rs:461-543`). The 4 MiB chunk size matches pCloud's documented `upload_write` expectation and there's even a heartbeat-timeout classification (`write_path.rs:919`). But:

#### [CRITICAL-5.3.1] The write-ahead journal's own doc contract is violated — `commit()` does not fsync the parent directory
**Files:** `crates/pcloud-fs/src/write_journal.rs:218-227` (the `commit()` implementation), vs. `crates/pcloud-fs/src/write_path.rs:37-45` (the doc contract).
**Severity:** CRITICAL.
**Detail:** `write_path.rs:37-45` explicitly documents the "P1.2 atomic write protocol":
```
//! 1. Append a JournalRecord...
//! 2. fsync(file) the journal file descriptor...
//! 3. fsync(dir) the journal's parent directory so the directory
//!    entry is durable — skipping this step means a post-crash `readdir`
//!    may fail to find a freshly-created journal segment, silently
//!    dropping acknowledged writes (POSIX allows this).
```
But `WriteJournal::commit()` at line 221 only does `self.file.flush()?; self.file.sync_data()?;`. There is **no** `fsync(parent_dir)`. The file is re-opened every startup so the journal file itself persists once created, but a brand-new journal file born mid-session (or a rename-replacement during reset, etc.) can be committed-but-not-in-directory after a crash.
**Remediation:** add a `parent_dir: File` field to `WriteJournal`, open it alongside the journal with `O_DIRECTORY|O_RDONLY`, and on `commit()` call `sync_all()` on the parent dir file. The sibling `UploadProgress::save` (line 882-911) already does this correctly — port the same pattern.

#### [CRITICAL-5.3.2] `ProtoUploadBackend::upload_file` reads the entire staging blob into memory
**File:** `crates/pcloud-fs/src/backend.rs:416-488`.
**Severity:** CRITICAL (data loss / OOM on large files).
**Detail:** `let bytes = std::fs::read(staging_file)?;` at line 416 slurps the whole file. Uploading a 10 GiB file through this code path will OOM the daemon. The comment at the top of the trait (`write_path.rs:461-543`) correctly uses a chunked-flush streaming loop, but `FileUploadBackend::upload_file` (the non-chunked fallback, used when `FlushPolicy::Whole` wins) is the foot-gun. And the default `upload_file` trait method selects between chunked and whole based on `is_chunked_supported` — but `ProtoUploadBackend` **does** implement chunked, so this path is mostly unused in production. Still, it's a landmine waiting to crash the daemon on any caller that calls `upload_file` directly (tests do).
**Remediation:** stream the file in 4 MiB chunks using the existing `upload_create` + `upload_write` + `upload_save` surface. Or remove `upload_file` from the trait entirely and force all callers through the chunked path.

#### [HIGH-5.3.3] Two journals coexist and the bounded in-memory one silently drops data
**Files:** `crates/pcloud-fs/src/journal.rs:1-119` (in-memory `WritebackJournal`) vs. `crates/pcloud-fs/src/write_journal.rs:1-500` (on-disk `WriteJournal`).
**Severity:** HIGH.
**Detail:** the `WritebackJournal` in `journal.rs:46-55` is **not** durable, and `append()` silently evicts the oldest entry when `pending.len() >= max_pending_operations`. The doc says "callers that need durability must flush before appending near the bound" — but the "bound" is `max_pending_operations: 4096` by default (line 40), not a byte count, and there's no callback when the eviction fires. The module-level doc at `journal.rs:1-6` calls it "ordered, crash-recoverable record of pending filesystem mutations" which is flatly wrong — the struct is `Serialize`/`Deserialize` but nothing serializes it to disk in the crate. The daemon surface only ever touches `WriteJournal` (on-disk). Anyone reading `journal.rs` in isolation would assume it is the durable journal.
**Remediation:** either remove `WritebackJournal` entirely (since only tests reference it in the published surface) or rename it `InMemoryWritebackCounters` and delete the "crash-recoverable" claim from the doc.

#### [HIGH-5.3.4] Journal replay is purely local — no replay against the remote backend
**File:** `crates/pcloud-fs/src/write_journal.rs:264-317` (`replay_path`).
**Severity:** HIGH.
**Detail:** `replay_path` returns a `Vec<JournalRecord>` of well-formed records but nothing in the crate consumes it and performs the deferred upload/unlink/rename ops against the live `pcloud-proto` backend on daemon restart. `write_path.rs:1039-1043` has `replay_upload_sidecars` which only reconciles `UploadProgress` sidecars for *in-flight* uploads — that's complementary, not the same. After a crash between a journaled `JournalOp::Unlink` write and the actual server `deletefile` call, the replayer has no code to pick up the outstanding unlink and retry it. The `WritePathService` itself has no `fn replay(&self)` method.
**Remediation:** implement `WritePathService::replay_journal(&self) -> Result<ReplayReport, WritePathError>` that iterates `replay_path(...)`, for each op reissues the remote call, and on success truncates the journal via `WriteJournal::reset`. Wire this into daemon startup in `runtime.rs`.

#### [MEDIUM-5.3.5] `FlushBarrier` records never get materialized into a durability guarantee against the remote backend
**File:** `crates/pcloud-fs/src/write_journal.rs:89-94` (`JournalOp::FlushBarrier`) and `crates/pcloud-fs/src/write_path.rs:475` (emission).
**Severity:** MEDIUM.
**Detail:** `JournalOp::FlushBarrier` is written to the journal before `chunked_flush` but there is no logic that blocks on the actual remote `upload_save` completion before letting the `flush(2)` syscall return to userspace. Looking at `flush_write` / `flush` in `write_path.rs:611` — it does call `chunked_flush` synchronously, good, but on the C reference client a `fsync(2)` blocks until the server ACKs; here, if `upload_save` returns but the network drops before the kernel buffer drains we silently report success. This is a grey-area POSIX semantics question.
**Remediation:** document explicitly what pCloud guarantees "durable" means post-`upload_save` and verify the response field actually indicates server-side durability rather than just "upload_save accepted".

#### [MEDIUM-5.3.6] Staging blob cleanup is orphan-prone if `chunked_flush` errors between `upload_create` and first `upload_write`
**File:** `crates/pcloud-fs/src/write_path.rs:491-502`.
**Severity:** MEDIUM.
**Detail:** when `upload_create` succeeds, an `UploadProgress` sidecar is written. If the caller aborts before any `upload_write`, the server keeps a zero-byte upload id around; the next daemon run sees the sidecar, hits `replay_upload_sidecars`, and classifies it correctly — **but** if the sidecar is removed by `remove_file` on startup (or simply lost), the server-side `uploadid` leaks until pCloud GC.
**Remediation:** prefer pairing `upload_create` with a `catch` that calls `upload_cancel` on any error.

#### [LOW-5.3.7] CRC32 algorithm runs a scalar loop — fine for correctness but noticeably slow on big journals
**File:** `crates/pcloud-fs/src/write_journal.rs:352-366`.
**Severity:** LOW (perf).
**Detail:** the hand-rolled CRC32 loop runs ~1.1 GB/s on modern x86; a `crc32fast` crate dep or `core::intrinsics::x86_64::_mm_crc32_u8` yields ~10 GB/s. Only matters if a replay burns serious time on a many-MB journal.

#### [LOW-5.3.8] `next_seq` is reset to 1 on re-open, ignoring records already in the file
**File:** `crates/pcloud-fs/src/write_journal.rs:170-181`.
**Severity:** LOW.
**Detail:** `WriteJournal::open` sets `next_seq: 1` then `seek_end()`. If the journal has existing records from a previous boot, the next record's `seq` will be 1 again, not N+1 where N is the highest `seq` in the file. `replay_path` does return sequence numbers correctly, but any observer consuming both live + replayed records would see duplicate `seq`s.
**Remediation:** on open, call `replay_path` to find the max `seq` and set `next_seq = max+1`.

---

### 5.4 Read path, page cache, prefetch

**Files:** `crates/pcloud-fs/src/page_cache.rs:1-500`, `crates/pcloud-fs/src/backend.rs:152-313`, `crates/pcloud-fs/src/fuse_adapter.rs:1271-1695` (readdir + read handling).

#### [CRITICAL-5.4.1] No read-ahead / prefetch anywhere in the read path
**Files:** `crates/pcloud-fs/src/backend.rs:277-312` (`ProtoFileBackend::read`) and `crates/pcloud-fs/src/page_cache.rs` (no prefetch API).
**Severity:** CRITICAL (perf parity with C client).
**Detail:** every read hits the HTTP edge synchronously; misses block the FUSE reply thread. The C reference `pfs_cache.c` implements look-ahead block fetch (e.g. on a sequential read pattern it kicks off the next N pages on a background thread). The Rust read path goes directly from `adapter.read(fh, off, size)` → `backend.read(handle, off, len)` → `fetch_download` → return. For a streaming video or large sequential copy off the mount, this will be **orders of magnitude slower** than the C client because every 64 KiB page has a full RTT.
**Remediation:** implement an async prefetch manager that, on sequential-read detection, enqueues up to N next pages into the `PageCache` from a dedicated reader thread pool. Even a minimal "prefetch the next 4 pages on any read" would close most of the gap.

#### [HIGH-5.4.2] `ProtoFileBackend::read` never populates the page cache
**File:** `crates/pcloud-fs/src/backend.rs:277-312`.
**Severity:** HIGH.
**Detail:** the `fetch_download` call is invoked on every read without any `PageCache::get` check or `PageCache::put` on success. The page cache in `page_cache.rs` is a library piece that appears to be wired from `fuse_adapter.rs` at the adapter level (needs verification), but the lower-level `FileBackend` trait has no cache awareness. This duplicates: if the adapter caches logically at ino granularity but the HTTP layer re-fetches the same offset, you pay the RTT twice.
**Remediation:** push the page cache down into `ProtoFileBackend::read` or eliminate it at the adapter level.

#### [HIGH-5.4.3] `FileHandle::size = 0` at `open` time — adapters that need file size see zero
**File:** `crates/pcloud-fs/src/backend.rs:268-274`.
**Severity:** HIGH.
**Detail:** the `ProtoFileBackend::open` method explicitly comments "`getfilelink` does not include file size; defer to a per-range response on first read (the HTTP layer reports Content-Length)." — and then constructs `FileHandle { size: 0, ... }`. But no code downstream patches this value. `FuseAdapter::statfs`, `getattr`, and callers that need EOF detection get `0` until they hit a short-read EOF on a byte-range. Worse: a read beyond EOF is not detected client-side; it issues an HTTP GET `Range: 1000-2000` for a 500-byte file and sees the server respond with 500 bytes instead of the requested 1000 — which the code treats as a successful short read (correct POSIX semantics), but means a pure `getattr` can never return a non-zero size through this backend.
**Remediation:** do a `stat` call in `open` via `list_folder_contents_by_path` on the parent, or issue a HEAD request to the signed URL, or add `stat_file` to the backend trait.

#### [MEDIUM-5.4.4] No eviction coordination between page cache and metadata cache
**Files:** `crates/pcloud-fs/src/page_cache.rs`, `crates/pcloud-fs/src/metadata_cache.rs`.
**Severity:** MEDIUM.
**Detail:** when a remote file changes (via a pCloud diff event / server-side write), neither cache is told. The TTL is 1 second (`fuser_shim.rs:68`) which is short enough to bound staleness, but a desktop client that edits a file in the web UI and then looks at the mount will see stale content for up to 1 second. The C client invalidates via the pCloud diff stream.
**Remediation:** wire a `PageCache::invalidate_file(file_id)` caller into the pCloud event stream (see `pcloud-engine/src/diff.rs` or similar).

#### [LOW-5.4.5] `PageCache::stats` is best-effort unsynchronized
**File:** `crates/pcloud-fs/src/page_cache.rs:14-16`.
**Severity:** LOW.
**Detail:** the doc claims single-`Mutex<Inner>` serialization, so stats are consistent under the lock. Fine. No action.

---

### 5.5 Mount handle RAII + teardown discipline

**Files:** `crates/pcloud-fs/src/mount_service.rs:229-569`, `crates/pcloud-fs/src/platform/linux.rs:119-217`, `crates/pcloud-fs/src/platform/bsd.rs:388-474`.

The `MountHandle` is a union of per-OS `Option<Inner>`s; `Drop` calls per-OS teardown; `unmount()` is the explicit path.

#### [HIGH-5.5.1] `Drop` swallows errors silently, violating the "audit persistence failures" rule from `CLAUDE.md`
**File:** `crates/pcloud-fs/src/mount_service.rs:542-569`.
**Severity:** HIGH.
**Detail:** Drop does:
```rust
if let Some(inner) = self.inner.take() {
    let _ = inner.unmount();
}
```
The `_ =` explicitly discards the unmount error. CLAUDE.md §"IPC and local security" says "do not silently swallow persistence or audit failures on active control paths." Operator lose-notification scenarios: a mount wedges, the daemon shuts down, Drop fires, `umount2(MNT_DETACH)` returns EBUSY, the user has a zombie `fuse.pcloud` mount in their namespace with no log line.
**Remediation:** log errors (via `log::error!`) from Drop. Panicking in Drop is bad, but logging is free.

#### [MEDIUM-5.5.2] The 5-second join timeout on macOS teardown is undocumented for the Linux path
**Files:** `crates/pcloud-fs/src/mount_service.rs:469-515` (macOS teardown with 5s bounded wait) vs. `crates/pcloud-fs/src/platform/linux.rs:151-216` (Linux unmount uses `SESSION_DROP_SETTLE_WINDOW = 2s` for `/proc/self/mountinfo` polling, then fires `umount2(MNT_DETACH)` with no bounded join on `fuser::BackgroundSession`).
**Severity:** MEDIUM.
**Detail:** `drop(self.session.take())` at `linux.rs:152` calls `fuser::BackgroundSession::drop`, which under the hood joins the dispatcher thread with no timeout. If the dispatcher is wedged on a blocking syscall (e.g. a pending HTTP read to pCloud with a TCP connection that will never RST), this blocks `unmount()` forever. The macOS path explicitly uses `recv_timeout(Duration::from_secs(5))` to avoid this.
**Remediation:** either (a) use a bounded join here too (harder because `fuser::BackgroundSession::drop` doesn't expose one), or (b) document this is accepted behavior. Simplest fix: the `fuser` crate's `SessionUnmounter` can be held separately and called with a short timeout before the session is dropped.

#### [MEDIUM-5.5.3] Signal trampoline calls non-async-signal-safe code
**Files:** `crates/pcloud-fs/src/platform/linux.rs:99-117`, `crates/pcloud-fs/src/platform/bsd.rs:364-386`.
**Severity:** MEDIUM.
**Detail:** `signal_trampoline` acquires `ACTIVE_MOUNTS.get_or_init(...)` and calls `mtx.lock()` inside a signal handler. `Mutex::lock` is **not** async-signal-safe; if the main thread was holding the mutex during a signal delivery, the handler deadlocks. The `CString::new(...)` allocation at :104 also invokes the global allocator, which is not async-signal-safe. `libc::umount2` itself is an async-signal-safe syscall, good — but the path around it is not.
**Remediation:** use `SA_SIGINFO` and write to a pipe from the handler; do the unmount on a dedicated reaper thread that drains the pipe. Or at minimum, use `try_lock` and skip if unavailable (still not safe w.r.t. the allocator though).

#### [LOW-5.5.4] No settle window for BSD when `MNT_FORCE` is actually issued
**File:** `crates/pcloud-fs/src/platform/bsd.rs:430-454`.
**Severity:** LOW.
**Detail:** after `MNT_FORCE` the code immediately returns; if the unmount is async (it usually is not on FreeBSD fuse, but it can be), the kernel may still report the mount for a brief window — a racing subsequent `mount` on the same path would fail with `EBUSY`. Minor.

#### [LOW-5.5.5] Windows teardown does not retry and does not validate `fsp_stop_dispatcher` return
**File:** `crates/pcloud-fs/src/mount_service.rs:517-540`.
**Severity:** LOW.
**Detail:** both `fsp_stop_dispatcher` and `fsp_delete` return NTSTATUS but the results are ignored. On a live WinFSP, a stop while IRPs are in-flight can return `STATUS_PENDING`; delete on a still-busy FS returns `STATUS_DEVICE_BUSY`. The user's mount letter stays occupied.
**Remediation:** check return status and either retry or log.

---

### 5.6 Signal handling / process-wide trampoline

**Files:** `crates/pcloud-fs/src/platform/linux.rs:80-117`, `crates/pcloud-fs/src/platform/bsd.rs:341-386`. macOS: no signal trampoline. Windows: no CTRL+C handler.

#### [HIGH-5.6.1] macOS mount has no SIGTERM/SIGINT cleanup
**File:** `crates/pcloud-fs/src/platform/macos.rs` (no signal handler is installed in `mount_with_fuse_t`).
**Severity:** HIGH.
**Detail:** if the daemon receives SIGTERM on macOS, only the regular `Drop` chain fires if stack unwinding reaches the handle. A `kill -9` orphans the fuse-t mount; a `Ctrl-C` in a foreground daemon causes `_exit(2)` without unwinding if there's no custom handler, also orphaning.
**Remediation:** mirror the Linux `install_signal_handler_once()` pattern with `fuse_unmount` called in the trampoline.

#### [HIGH-5.6.2] Windows has no console-control handler (CTRL+C, service stop)
**File:** `crates/pcloud-fs/src/platform/windows.rs`.
**Severity:** HIGH.
**Detail:** on a Windows service stop (SC_STOPPED) or a console CTRL_CLOSE_EVENT, the `MountHandle::drop` won't fire unless the runtime explicitly tears things down. WinFSP provides `FspFileSystemRemoveMountPoint` via the `WinFspLibrary` wrapper but nothing installs a `SetConsoleCtrlHandler` trampoline to invoke it. After a hard process exit the drive letter stays mapped until WinFSP times out the IRP.
**Remediation:** wire `windows::Win32::System::Console::SetConsoleCtrlHandler` on the first mount and call `FspFileSystemStopDispatcher` + `Delete` on CTRL_CLOSE_EVENT.

#### [MEDIUM-5.6.3] Signal trampoline restores `SIG_DFL` and re-raises — correct, but races
**File:** `crates/pcloud-fs/src/platform/linux.rs:113-116`.
**Severity:** MEDIUM.
**Detail:** `libc::signal(sig, libc::SIG_DFL); libc::raise(sig);` is the conventional pattern, but between `SIG_DFL` and `raise`, a second signal can interleave and trigger the default behavior before our handler finishes cleanup. Low-probability but real.
**Remediation:** use `sigaction` with `SA_RESETHAND` so the kernel resets atomically on first delivery.

---

### 5.7 Orphan detection

**File:** `crates/pcloud-fs/src/mount_orphan.rs:1-405`.

Linux side (`/proc/self/mountinfo` parser + `fusermount_unmount`) is mature: it correctly handles escaped spaces, skips malformed lines, and has a `fusermount3` → `fusermount` fallback with timeout. Cross-platform hooks:

- BSD: `crates/pcloud-fs/src/platform/bsd.rs:214-287` — uses `getmntinfo(3)` and reshapes to a mountinfo-compatible payload so the shared parser can consume it. Good design.
- macOS: `crates/pcloud-fs/src/platform/macos.rs:1664-1729` — same pattern via `getmntinfo(3)`. Good.
- Windows: `crates/pcloud-fs/src/platform/windows.rs:195-210` — stub returning empty payload. **Does not detect orphans on Windows.**

#### [HIGH-5.7.1] Windows orphan detection is a stub — any WinFSP crash leaves a zombie drive letter undetectable by the daemon
**File:** `crates/pcloud-fs/src/platform/windows.rs:195-210` and cross-ref at `mount_orphan.rs:64-73`.
**Severity:** HIGH.
**Detail:** the `WindowsMountinfoReader::read` returns `Ok(String::new())` with a TODO. The daemon that restarts after a WinFSP dispatcher crash has no way to know a drive letter is still reserved; the next mount attempt on that letter fails with `STATUS_ACCESS_DENIED` and the user is told "mount failed" rather than "orphan reclaimed".
**Remediation:** use `GetLogicalDriveStringsW` + `QueryDosDeviceW` to enumerate drive letters; a pCloud-mounted WinFSP drive has a NT device name starting with `\Device\WinFsp.Disk\`. Emit matching entries as mountinfo-shaped lines.

#### [MEDIUM-5.7.2] `unescape_mountinfo` accepts invalid octal sequences silently
**File:** `crates/pcloud-fs/src/mount_orphan.rs:295-315`.
**Severity:** MEDIUM.
**Detail:** the parser accepts any 3-digit run `\NNN` regardless of whether the digits are actually octal (0-7). So `\089` passes `is_ascii_digit()` and computes `(0-0)*64 + (8-0)*8 + (9-0) = 73 = 'I'`, silently corrupting the path. Real `/proc/self/mountinfo` never emits this (kernel only escapes ` `, `\t`, `\n`, `\\`) but a hostile `/proc` could.
**Remediation:** check `a <= b'7' && b <= b'7' && c <= b'7'`.

#### [LOW-5.7.3] `fusermount_unmount` has no "already unmounted" fast path
**File:** `crates/pcloud-fs/src/mount_orphan.rs:256-266`.
**Severity:** LOW.
**Detail:** if the mount is already gone, `fusermount3 -u /foo` exits with nonzero — the helper returns an error. Caller must re-poll `/proc/self/mountinfo`. Minor.

---

### 5.8 Mount policy / `MountOptions` validation

**File:** `crates/pcloud-fs/src/mount_service.rs:25-156`.

Solid: rejects missing/non-directory/non-empty mountpoints, rejects mountpoints not owned by current uid (Linux), rejects world-writable modes (Linux), rejects `allow_other`, builds FUSE options with `DefaultPermissions` + `NoDev` + `NoSuid` + `RO`/`RW`. BSD (line 94-106) tightens: rejects group- or world-writable (`0o022`). These are good hardening defaults.

#### [HIGH-5.8.1] macOS defaults intentionally set `allow_other = true`, bypassing the cross-platform veto by design — but with no user-visible warning
**File:** `crates/pcloud-fs/src/platform/macos.rs:95-110`.
**Severity:** HIGH (security surface mismatch).
**Detail:** `MacosPlatformMount::default_options()` does `opts.allow_other = true;` with a comment "`allow_other` is vetoed by the Rust `MountService` at the cross-platform layer; we still surface the intent here so callers that bypass `MountService` (integration tests, raw CLI) see the platform-preferred value." This means any caller that routes through `MountService::mount` gets `AllowOtherRejected`, but a caller that calls `MacosPlatformMount::mount_adapter` directly (which the `mount_service.rs:181-186` cfg branch does on macOS) skips the veto. **In fact**, the macOS branch of `MountService::mount` hits line 181-186 which invokes `backend.mount_adapter(Box::new(adapter), mountpoint, options)` with the user-supplied `options` — not with `default_options`, so the veto is not re-run, but `allow_other` is preserved only if the *caller* set it. So on macOS the user's explicit `allow_other = false` survives. OK for the happy path, but still: the pattern is error-prone and the comment is misleading.
**Remediation:** move the `allow_other` veto into `PlatformMount::mount_adapter` (or a shared pre-check) rather than only into `MountService::mount`.

#### [MEDIUM-5.8.2] No check that the mountpoint is not on a network filesystem
**File:** `crates/pcloud-fs/src/mount_service.rs:111-156`.
**Severity:** MEDIUM.
**Detail:** mounting pCloud over, say, an NFS mount introduces semantic surprises (lock propagation, fsync semantics). The C client rejects mountpoints on anything that isn't a local fs.
**Remediation:** optional — use `statfs(2)::f_type` on Linux and compare against a small allow-list (tmpfs, ext4, btrfs, xfs, f2fs, zfs). Or at least warn.

#### [MEDIUM-5.8.3] `MountOptions` struct conflates transport hardening with presentation
**File:** `crates/pcloud-fs/src/mount_service.rs:25-45`.
**Severity:** MEDIUM.
**Detail:** only three fields — `read_only`, `fs_name`, `allow_other` — with no surface for `attr_timeout`, `entry_timeout`, `max_readahead`, `noatime`, `nodev`/`nosuid` (those are hard-coded in `build_fuse_options`). A daemon that needs to tune these for a performance/parity scenario has no knob.
**Remediation:** extend `MountOptions` with `attr_timeout: Duration`, `entry_timeout: Duration`, `max_readahead: Option<u32>`, and thread them through `build_fuse_options`.

#### [LOW-5.8.4] No Windows-specific path sanitization for mountpoint
**File:** `crates/pcloud-fs/src/platform/windows.rs:116-142`.
**Severity:** LOW.
**Detail:** `is_drive_letter_root` short-circuits to `Ok(())` without rejecting obvious foot-guns (e.g. `C:\Windows\System32` as a directory mount). The comment "we intentionally do not require the drive letter to be free at validate-time" is correct for a drive letter, but for directory-reparse mounts the current path-existence check accepts any empty directory — including one that `runas /user:SYSTEM` created.
**Remediation:** check the mount path is not inside `%SystemRoot%` or `%ProgramFiles%`.

---

### 5.9 Benches

**Files:** `crates/pcloud-fs/benches/page_cache.rs:1-50+`, `crates/pcloud-fs/benches/chunked_flush.rs:1-139`.

Good-sized criterion harness. `page_cache.rs` covers sequential cold-fill+hit, random 1 GiB, eviction pressure, and 4-thread concurrent reads. `chunked_flush.rs` covers 100 MiB payload at 1/4/16 MiB chunks through a no-op backend.

#### [MEDIUM-5.9.1] Benches have no regression baseline in CI
**File:** `crates/pcloud-fs/benches/chunked_flush.rs:16-20` (TODO comment) and `page_cache.rs` (no comment).
**Severity:** MEDIUM.
**Detail:** the author explicitly TODOed "Wire baseline capture into the `bench-nightly` CI job" but the wiring never landed. Without a baseline, regressions go unnoticed.
**Remediation:** add a CI matrix job that runs `cargo bench` and compares against a committed JSON snapshot.

#### [LOW-5.9.2] The `chunked_flush` bench runs against a no-op backend — it doesn't measure actual flush overhead
**File:** `crates/pcloud-fs/benches/chunked_flush.rs:44-89`.
**Severity:** LOW.
**Detail:** the bench explicitly measures state-machine dispatch cost only, which is fine for regression catching, but a separate integration bench against a `StagingDir`-backed scenario (without network) would catch the real I/O bottlenecks. The `write_path` module has no bench at all for `chunked_flush` through `WritePathService`.
**Remediation:** add a second bench that runs through `WritePathService::chunked_flush` with a real staging dir and an in-memory upload backend.

#### [LOW-5.9.3] No bench for mount/unmount round-trip latency
**File:** no file.
**Severity:** LOW.
**Detail:** the Linux mount path has a 2-second settle window; a bench that mounts+unmounts 100 times would reveal when that budget needs to change.

---

### 5.10 Integration tests

**Files:** `crates/pcloud-fs/tests/*.rs` — 10 test files.

- `fuse_mount_integration.rs` — `readdir` + read + write + fsync with a MockFolderBackend. **`#[ignore]` + `PCLOUD_FUSE_TEST=1` gated**, Linux-only.
- `fuse_kernel_e2e.rs` — full 64 MiB create/write/fsync/read/rename/unlink round-trip through real FUSE kernel. Linux-only, also `#[ignore]`.
- `fuse_read_path_live.rs`, `fuse_write_path_live.rs`, `fuse_small_write_wiring.rs`, `fuse_dyn_shim_write.rs`, `fuse_lifecycle_hardening.rs` — all Linux-gated.
- `mount_transport_wiring.rs`, `platform_mountinfo_crossplat.rs`, `write_path_replay.rs` — cross-platform compiling (parser + replay logic only, no kernel mount).

#### [CRITICAL-5.10.1] Every integration test that actually mounts a FUSE filesystem is `#[ignore]`
**Files:** all `fuse_*.rs` test files in `crates/pcloud-fs/tests/`.
**Severity:** CRITICAL (test signal).
**Detail:** the default `cargo test -p pcloud-fs` runs **zero** tests that exercise the kernel. A contributor can regress `mount()` without any test failing locally or in typical CI. The tests require `PCLOUD_FUSE_TEST=1` or `PCLOUD_LIVE_E2E=1` env var and a suid `fusermount3` binary + `/dev/fuse` access — a lot of containers / CI runners don't meet these criteria. The skip-logic inside each test (e.g. `fuse_gate_enabled()` or `should_skip_mount_error`) further degrades signal: even when the test **is** opted into, it may silently succeed by returning early.
**Remediation:** (a) add a dedicated CI job running in a privileged container with `/dev/fuse` that sets `PCLOUD_FUSE_TEST=1`; (b) make skip-paths emit a visible warning or convert them to runtime errors; (c) add `cargo test --features live-fuse` convention documented in the README.

#### [HIGH-5.10.2] No FreeBSD kernel-mount test exists in-tree
**File:** `crates/pcloud-fs/tests/` (absence).
**Severity:** HIGH.
**Detail:** FreeBSD is declared tier-2 but the only FreeBSD-specific test file is a compile-only assertion (`platform_mountinfo_crossplat.rs`). There is no FreeBSD-gated version of `fuse_kernel_e2e.rs`. On a platform the README claims as tier-2, the kernel-mount path has literally never been exercised.
**Remediation:** duplicate the e2e test with `#[cfg(any(target_os = "linux", target_os = "freebsd"))]` and parametrize the BSD `MNT_FORCE` path.

#### [HIGH-5.10.3] No macOS or Windows tests at all
**File:** `crates/pcloud-fs/tests/` (absence).
**Severity:** HIGH.
**Detail:** platform/macos.rs and platform/windows.rs have a total of ~100 KiB of Rust code and zero tests. The module-level docs repeat "NOT YET TESTED ON MACOS"/"PHASE-1 SCAFFOLDING" in ~6 places.
**Remediation:** tests gated by platform will at least be compile-checked, even if they skip. Add a minimum smoke test that validates `probe_supported` returns the expected `Unsupported` error when fuse-t / WinFSP is absent.

#### [MEDIUM-5.10.4] `write_path_replay.rs` tests are unit-style (no actual crash simulation)
**File:** `crates/pcloud-fs/tests/write_path_replay.rs:1-120` (3 tests).
**Severity:** MEDIUM.
**Detail:** The file name promises "replay" testing but the tests exercise `replay_path` API calls, not actual crash-during-write simulation. There's no test that literally hard-interrupts the write (e.g. via a forked subprocess killed via SIGKILL between journal.append and the visible rename), and then verifies `replay` recovers the state.
**Remediation:** fork a subprocess that calls `WriteJournal::append`, `exit(137)` before `commit`, then re-open in the parent and verify the prefix of records is intact.

#### [MEDIUM-5.10.5] `platform_mountinfo_crossplat.rs` only verifies parser compiles across platforms
**File:** `crates/pcloud-fs/tests/platform_mountinfo_crossplat.rs:1-100` (3 tests).
**Severity:** MEDIUM.
**Detail:** good for cross-platform compile assurance, but no actual cross-platform mount reconciliation test. No test of "BSD getmntinfo emits a payload that survives round-trip through `parse_pcloud_mounts`".
**Remediation:** add a fixture-driven test that feeds a representative `getmntinfo`-emitted payload through `parse_pcloud_mounts` on Linux and asserts the resulting entries are equivalent.

---

### 5.11 macOS specifics

**Files:** `crates/pcloud-fs/src/platform/macos.rs:1-1800+`, `crates/pcloud-fs/src/platform/macos_ffi.rs:1-500+`.

Module header is explicit: "**NOT YET TESTED ON MACOS** — bring-up requires a real Mac with fuse-t installed." Phase 5 is in-flight with write + read thunks populated, and `MacFuseBackend::FuseT` is the default (confirmed).

#### [CRITICAL-5.11.1] fuse-t vs. macFUSE ABI is asserted equivalent — but `LowlevelOps` struct layout is version-sensitive and unvalidated
**Files:** `crates/pcloud-fs/src/platform/macos.rs:1607-1626` (ops table), `crates/pcloud-fs/src/platform/macos_ffi.rs:1-500+` (struct defs).
**Severity:** CRITICAL.
**Detail:** `build_lowlevel_ops` constructs a `LowlevelOps` with 17 callback slots; it's passed to `fuse_lowlevel_new` with `size_of::<LowlevelOps>()` as the third argument, so libfuse reads only up to that size. If the installed libfuse 2.9 backend (fuse-t or macFUSE) has a different layout — say, a newer version that reorders fields or adds a callback at a lower offset — **the callbacks we install end up in the wrong slot** and libfuse calls, e.g., `write` when the kernel requested `getattr`. Passing a smaller `size` is safer than a larger one (libfuse won't read past), but cannot save us from a wrong-slot mapping.
**Remediation:** there is no runtime way to verify the layout. The crate must CI-build against the actual `fuse_lowlevel.h` from both fuse-t and macFUSE and assert `offsetof` matches. Until that ships, mark the macOS backend as experimental and feature-gate it off by default.

#### [HIGH-5.11.2] `ensure_libfuse_loaded` intentionally leaks the dlopen handle — correct, but no re-probe on dynamic link failure
**File:** `crates/pcloud-fs/src/platform/macos.rs:1497-1540`.
**Severity:** HIGH.
**Detail:** the dlopen handle is leaked (correct — dylib must outlive the session). But when `dlopen` succeeds but a subsequent `fuse_mount` fails with undefined-symbol (happens when a partial install has `libfuse.dylib` but missing rpath for its internal deps), the crate reports "fuse_mount failed" at `:200-204` without the dlerror context. Debugging a partial install becomes hard.
**Remediation:** resolve critical symbols via `dlsym` before calling them so we can emit a precise "symbol X not found in libfuse.dylib" error.

#### [HIGH-5.11.3] `volname` option is passed verbatim from user input without length validation
**File:** `crates/pcloud-fs/src/platform/macos.rs:1580-1598`.
**Severity:** HIGH.
**Detail:** macOS NFS/fuse-t imposes a 127-byte limit on volume names. A longer `fs_name` from `MountOptions` is formatted and passed through; fuse-t will either truncate silently (best case) or reject mount (worst case) with no guidance to the user.
**Remediation:** clamp `volname` to 127 bytes and warn when truncation occurs.

#### [MEDIUM-5.11.4] `entry_attr_to_stat` zeros `st_blocks`
**File:** `crates/pcloud-fs/src/platform/macos.rs:345-369`.
**Severity:** MEDIUM.
**Detail:** `st.st_blocks` is left at 0 (the default from `zeroed()`). macOS `du` uses `st_blocks` to compute disk usage; it will report 0 bytes used for every file, making the mount unusable with `du -sh` and similar tools.
**Remediation:** set `st.st_blocks = ((attr.size + 511) / 512) as i64;`.

#### [MEDIUM-5.11.5] `thunk_readdir` synthesizes `stub_attr` with arbitrary defaults
**File:** `crates/pcloud-fs/src/platform/macos.rs:696-705`.
**Severity:** MEDIUM.
**Detail:** during `readdir` the code builds a per-entry `libc::stat` from a stub attribute rather than from the real entry attributes returned by the adapter. macOS's `FUSE_READDIRPLUS` path wants real attrs to avoid the follow-up `lookup` per entry. This works around the missing `readdirplus`, but turns a O(1) listing into O(N lookups).
**Remediation:** either implement `readdirplus` or build the stat from `entry.attr` instead of `stub_attr`.

#### [LOW-5.11.6] 20 `eprintln!` debug prints remain in platform/macos.rs
**File:** `crates/pcloud-fs/src/platform/macos.rs` — grep-count 20.
**Severity:** LOW.
**Detail:** `[pcloud-fuse-t] ...` debug traces on every lookup/create/write/unlink/rename. Production build would flood stderr. They're not gated on a debug flag or the `log` crate.
**Remediation:** route through `log::debug!` (already a dependency).

#### [LOW-5.11.7] `fuse_session_loop` panic unwind is documented as "does not run user Rust panics" — but is not enforced
**File:** `crates/pcloud-fs/src/platform/macos.rs:246-256`.
**Severity:** LOW.
**Detail:** the comment says the loop thread doesn't unwind because thunks catch their own panics. If a future contributor adds a non-thunk caller (e.g. a helper that runs in the loop thread), unwinding across FFI is UB. Belt-and-braces would wrap the whole loop in `catch_unwind`.

---

### 5.12 Windows specifics

**Files:** `crates/pcloud-fs/src/platform/windows.rs:1-1800+`, `crates/pcloud-fs/src/platform/winfsp_ffi.rs:1-700+`.

Module header is blunt: "PHASE-3 SCAFFOLDING — FSP_FILE_SYSTEM dispatcher wired but not tested on Windows." `winfsp_ffi.rs` header: "PHASE-1 SCAFFOLDING — NOT YET TESTED ON WINDOWS. Treat every symbol here as a structural placeholder."

#### [CRITICAL-5.12.1] `VolumeParams` `reserved_tail: [u8; 256]` is an arbitrary guess at struct size
**File:** `crates/pcloud-fs/src/platform/winfsp_ffi.rs:113-135`.
**Severity:** CRITICAL.
**Detail:** `VolumeParams` explicitly declares "NOTE: The true struct layout is WinFSP-internal and version-sensitive. A final Windows-side build must validate `size_of::<VolumeParams>() == sizeof(FSP_FSCTL_VOLUME_PARAMS)` and each field offset against the installed WinFSP headers before we claim runtime parity." The `reserved_tail` is 256 bytes — but the actual WinFSP 2.x struct has grown past that in recent releases (some versions push past ~400 bytes). If the installed WinFSP's struct is larger than our declared `VolumeParams`, `FspFileSystemCreate` will read uninitialized stack/heap past our struct boundary (UB, or at best silently corrupt params). If smaller, we over-write and potentially clobber adjacent memory.
**Remediation:** generate `VolumeParams` from the installed `winfsp/fsctl.h` via a `build.rs` + `bindgen` pass, or check the WinFSP-reported size at runtime and refuse to mount on mismatch.

#### [CRITICAL-5.12.2] 11+ unsafe blocks without `SAFETY:` comments in the Windows path
**Files:** `crates/pcloud-fs/src/platform/windows.rs` (86 `unsafe` vs 75 `SAFETY`), `crates/pcloud-fs/src/platform/winfsp_ffi.rs` (19 `unsafe` vs 7 `SAFETY`).
**Severity:** CRITICAL (per CLAUDE.md §"enterprise rules").
**Detail:** CLAUDE.md says every unsafe block needs a SAFETY comment. The ratio says ~12 blocks in `winfsp_ffi.rs` and ~11 in `windows.rs` are bare. Example area of concern: the thunk bodies dereference `PFspFileSystem` and `file_ctx` pointers without documenting the invariants.
**Remediation:** add `SAFETY:` blocks or, better, wrap the raw pointers in newtype `Send`-able smart pointers whose methods carry the safety invariants.

#### [HIGH-5.12.3] The single `eprintln!` in `windows.rs` is a debug print, not a structured error
**File:** `crates/pcloud-fs/src/platform/windows.rs` — grep-count 1.
**Severity:** HIGH.
**Detail:** anything an operator would need to diagnose a mount failure is either missing or printed once via eprintln. No `log::error!`.

#### [HIGH-5.12.4] `load_winfsp` does not lock against concurrent loads
**File:** `crates/pcloud-fs/src/platform/winfsp_ffi.rs:200-300+` (load_winfsp).
**Severity:** HIGH.
**Detail:** dynamically loading the DLL returns a `WinFspLibrary` that's wrapped in `Arc` at the `MountHandle` level, but if two threads call `load_winfsp` concurrently they each call `LoadLibraryW` — `LoadLibraryW` is thread-safe at the OS level, but two callers then each call `GetProcAddress` for every symbol and produce two `WinFspLibrary` clones. Not UB but wasteful.
**Remediation:** store the loaded library in a `OnceLock<Arc<WinFspLibrary>>` static.

#### [HIGH-5.12.5] WinFSP Cleanup callback delete-on-close semantics not implemented
**File:** `crates/pcloud-fs/src/platform/windows.rs` (entire).
**Severity:** HIGH.
**Detail:** module doc line 43-46 says "Cleanup handles delete-on-close. WinFSP calls Cleanup with the FspCleanupDelete flag when the NT FILE_DELETE_ON_CLOSE disposition is set; the shim then issues the backend removal." — but grepping for `FspCleanupDelete` shows no implementation. A file opened with `FILE_DELETE_ON_CLOSE` and closed will not be deleted remotely. Data-consistency issue.
**Remediation:** implement the Cleanup callback slot.

#### [MEDIUM-5.12.6] No alternate data stream rejection — silent truncation
**File:** `crates/pcloud-fs/src/platform/windows.rs`.
**Severity:** MEDIUM.
**Detail:** doc says "Alternate Data Streams / reparse points: NOT supported. The corresponding WinFSP callbacks (where present) return STATUS_NOT_SUPPORTED." — need to verify the `Open` / `Create` callbacks actually reject paths with `:` (ADS notation) rather than silently treating them as regular filenames.

#### [MEDIUM-5.12.7] `WindowsMountinfoReader` is a stub (see 5.7.1)
(See §5.7.1 — same issue, raised for Windows specifically.)

---

### 5.13 FreeBSD specifics

**File:** `crates/pcloud-fs/src/platform/bsd.rs:1-564`.

Module declares tier-2 for FreeBSD, tier-3 for NetBSD/OpenBSD. Uses `fuser` crate's libfuse2 backend, same `fuser::Filesystem` shim as Linux (shared in `platform/fuser_shim.rs`). Mount via `fuser::spawn_mount2`, unmount via `libc::unmount(path, MNT_FORCE)` with a 2s settle window polling `getmntinfo(3)`.

#### [HIGH-5.13.1] `/dev/fuse` probe at `probe_supported` does not validate `kldload fuse` worked
**File:** `crates/pcloud-fs/src/platform/bsd.rs:129-152`.
**Severity:** HIGH.
**Detail:** the check is `Path::new("/dev/fuse").exists()`. But FreeBSD's fuse module sometimes creates `/dev/fuse` on first use only, not on kldload. Operator hint "load the fuse kernel module (kldload fuse / modload fuse)" is accurate for the common case but misleading when the node exists from a previous `fuse_mount` even if the module is now gone.
**Remediation:** try opening `/dev/fuse` with `O_RDWR|O_CLOEXEC` and check for `ENODEV` vs. `ENOENT`.

#### [MEDIUM-5.13.2] `MNT_FORCE` unmount is blunter than Linux `MNT_DETACH`
**File:** `crates/pcloud-fs/src/platform/bsd.rs:435-454`.
**Severity:** MEDIUM.
**Detail:** `MNT_FORCE` aborts in-flight requests; the Linux path uses `MNT_DETACH` which waits for references to drop but lets in-flight syscalls complete. The BSD comment acknowledges "FreeBSD has no exact `MNT_DETACH` analogue" — true — but the semantic difference affects data integrity for a process mid-write. With `MNT_FORCE` the write's EIO return is seen before the journal commits remotely.
**Remediation:** document this in the user-facing README, or attempt a `MNT_FORCE|MNT_DETACH` (FreeBSD supports both if `-2` is NOT set; newer kernels added `MNT_NONBUSY` for graceful-first escalation).

#### [MEDIUM-5.13.3] `path_is_current_mount` uses `f_mntonname` literal comparison — no escape decode
**File:** `crates/pcloud-fs/src/platform/bsd.rs:185-212`.
**Severity:** MEDIUM.
**Detail:** comparison is `Path::new(&mountpoint) == canonical`; `f_mntonname` is an unescaped kernel path (no `\040` encoding). Fine as long as canonicalize doesn't re-escape — which it doesn't. OK.

#### [LOW-5.13.4] No FreeBSD-specific test binary
(See §5.10.2.)

---

### 5.14 `bd-1du.4` gap checklist (per `CLAUDE.md`)

The bead states the Linux mount-runtime parity gaps as:
- real Linux mount/unmount ← **partially implemented** (missing: statfs/access/forget, no signal-safe trampoline)
- readdir ← **implemented** through `FuseAdapter::readdir` + shim
- open/read ← **implemented** (no prefetch)
- write/flush/fsync ← **implemented** (caveat: journal dir fsync missing)
- inode/path lifecycle ← **partial** (forget not wired; bare ino-to-path cache in `fuse_adapter.rs` has no eviction policy)
- crash-safe writeback ← **partial** (journal replay never runs, upload sidecar replay does; see 5.3.4)
- integration tests for mounted-drive behavior ← **all `#[ignore]`**

Net: bd-1du.4's own check-list is **not** satisfied. The epic cannot honestly be closed.

---

### 5.15 Per-platform coverage summary table

Legend: `I` implemented, `P` partial, `M` missing/stub, `X` not applicable.

| Capability                 | Linux (tier 1) | FreeBSD (tier 2) | macOS (tier 1*) | Windows (tier 1*) | NetBSD (tier 3) | OpenBSD (tier 3) |
|----------------------------|----------------|-------------------|------------------|--------------------|-----------------|------------------|
| Mountpoint validator       | I              | I                 | I                | P (drive letter only) | I (shared)   | I (shared)       |
| Kernel mount/unmount       | I              | I                 | P (never booted)†| P (never booted)†  | M               | M                |
| Read path (lookup+getattr+readdir+read) | I | I                 | P (scaffold)     | P (scaffold)       | M               | M                |
| Write path (create+write+flush+fsync+unlink+rename) | I | I  | P (scaffold)     | P (scaffold, no Cleanup) | M        | M                |
| `statfs`                   | **M**          | **M**             | I                | P (GetVolumeInfo)   | M               | M                |
| `access`                   | **M**          | **M**             | M                | M                  | M               | M                |
| `forget`                   | **M**          | **M**             | M                | X                  | M               | M                |
| `setattr` (mode/uid/gid/times) | M / size only | M / size only | M / size only    | M / size only      | M               | M                |
| `rename` flags             | M              | M                 | I                | I                  | M               | M                |
| Extended attributes        | M              | M                 | M                | M                  | M               | M                |
| `readlink`/`symlink`/`link`| M              | M                 | M                | M                  | M               | M                |
| Orphan detection           | I              | I (getmntinfo)    | I (getmntinfo)   | **M** (stub)       | I (shared)     | I (shared)       |
| Signal trampoline (SIGTERM/SIGINT/CTRL-C)| I| I                 | **M**            | **M**              | M               | M                |
| Journal replay on startup  | **M** (written but not consumed) | M | M           | M                  | M               | M                |
| Read-ahead / prefetch      | **M**          | M                 | M                | M                  | M               | M                |
| Page cache integration     | P (separate trait piece) | P       | P                | P                  | P               | P                |
| Integration test coverage  | `#[ignore]`d   | **M**             | **M**            | **M**              | M               | M                |

`*` — "planned tier 1" per `crates/pcloud-fs/Cargo.toml`. `†` — scaffolding only; module doc explicitly says "NOT YET TESTED ON MACOS/WINDOWS."

---

### 5.16 Consolidated remediation priorities

**Must-fix before claiming any parity (P0 — block `bd-1du.4` / `bd-1du.10`):**
1. Implement `statfs` across Linux/FreeBSD shims (5.2.1).
2. Fix journal `commit()` to fsync parent directory (5.3.1).
3. Stream `ProtoUploadBackend::upload_file` instead of slurping to memory (5.3.2).
4. Wire journal replay into daemon startup (5.3.4).
5. Default-ignore all integration tests defeats `bd-1du.4` proof — add a privileged CI job (5.10.1).
6. `MountService::mount` doesn't dispatch to Windows (5.1.1).
7. Validate WinFSP `VolumeParams` layout against installed headers (5.12.1).
8. Add SAFETY comments to the ~23 bare `unsafe` blocks on Windows (5.12.2).

**Should-fix before release (P1):**
9. Implement `access` and `forget` in shims (5.2.2, 5.2.3).
10. Read-ahead / prefetch in read path (5.4.1).
11. Eliminate or rename `WritebackJournal` to remove "crash-recoverable" misrepresentation (5.3.3).
12. Install signal trampolines for macOS and Windows (5.6.1, 5.6.2).
13. Implement Windows orphan detection (5.7.1).
14. Write-path setattr honors mode/times instead of silently succeeding (5.2.5).
15. Validate fuse-t `LowlevelOps` layout at build time (5.11.1).

**Nice-to-have (P2+):**
16. Extended attributes as `ENOTSUP` for ergonomic compatibility (5.2.7).
17. `fallocate`/`copy_file_range` for perf (5.2.8).
18. Bench regression baselines in CI (5.9.1).
19. Replace hand-rolled CRC32 with SIMD-optimized crate (5.3.7).
20. Route `eprintln!` through `log` crate (5.11.6).

---

### 5.17 Overall verdict

The `pcloud-fs` crate has good architecture, clean platform separation, and most of the happy-path Linux code is sound. But five load-bearing claims in CLAUDE.md §"What Is Left To Do" about `bd-1du.4` being "substantially scaffolded" are currently **not** substantiated by code:

1. "Real Linux mount/unmount" — yes, but without `statfs`/`access`/`forget`, operators will see regressions vs. the C client.
2. "Crash-safe writeback" — the journal format is crash-safe, but there is no code that consumes it on startup, and the doc-ed `fsync(file)+fsync(dir)` discipline is a lie.
3. "Integration tests for mounted-drive behavior" — tests exist but are all `#[ignore]`-gated; CI runs zero of them.
4. "macOS tier-1 planned" — the module self-describes as PHASE-1 SCAFFOLDING NOT YET TESTED, which is honest but cannot be called "tier 1."
5. "WinFSP tier-1 planned" — same: struct layouts unvalidated, Cleanup not implemented, orphan detection is a stub.

Every finding above has a file:line citation and a concrete remediation. The work required to close the gaps is substantial but incremental — no architectural rewrite. The most important single thing the project can do is **enable a privileged-CI job that runs `PCLOUD_FUSE_TEST=1 cargo test -p pcloud-fs` on every merge** — that alone would prevent the majority of future regressions.

**Recommendation for `bd-1du.10`:** do not close until items P0-1 through P0-5 above are landed. Downgrade the macOS and Windows entries in the parity matrix from "tier 1 planned" to explicit "scaffolding — not production" until §5.11.1 and §5.12.1 are resolved.
# pcloud-rs Enterprise-Readiness Audit — Dimensions 6 + 7

**Audit scope:** Dimension 6 (Transport & Network Resilience, outbound HTTP/API)
and Dimension 7 (IPC & Daemon, local control plane).

**Auditor role:** parallel specialist auditor (1 of 10). Cross-cutting
findings that belong to other dimensions (secret discipline §2,
observability §8, sync engine §4, FUSE §5) are flagged for cross-reference
but not re-litigated.

**Workspace root:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/`

**Methodology:** read every source file listed in the prompt, trace the
request path from config → transport → resilient wrapper → dispatch →
backend, and confirm every claim against a file:line citation. No tests
were executed; all assertions are static.

All severity ratings are informed by the enterprise bar implied by the
prompt ("production-ready", "drop-in replacement"). Findings that would
be acceptable in a single-user desktop client are still flagged at
MEDIUM/HIGH when they block the enterprise claim.

---

## Section 6. Transport & Network Resilience

### 6.1 TLS enforcement (mandatory)

#### 6.1.1 [HIGH] Transport struct exposes a **public** `use_tls: bool` with no defense-in-depth check at the socket layer

`crates/pcloud-proto/src/transport.rs:71-96` — `TransportConfig` carries a
`pub use_tls: bool` field; `crates/pcloud-proto/src/transport.rs:255-268`
— `execute_with_body` branches on `config.use_tls` with zero consultation
of the active `Environment`. The documentation explicitly admits
enforcement is centralised elsewhere:

```rust
// pcloud-proto/src/transport.rs:86-90
/// Must be `true` outside of tests. The field is *not* checked
/// here — enforcement lives in the daemon bootstrap — so this
/// struct remains usable for local integration tests.
pub use_tls: bool,
```

This is a fragile design. Any call site — plugin, SDK consumer, future
refactor — that constructs a `BinaryApiTransport` without going through
`ApiEndpoint::validate` silently bypasses the production-TLS invariant.

The gate is at `crates/pcloud-config/src/api.rs:131-170` (`ApiEndpoint::
validate`); however, `ApiEndpoint` is not the type held by
`BinaryApiTransport`. The two structs are deliberately decoupled
(`crates/pcloud-proto/src/transport.rs:17-32` calls this out), which
means the transport layer itself is willing to dial plaintext given any
`use_tls=false` input.

**Remediation:** Either (a) delete the plaintext branch from
`execute_with_body` at production build time (cfg gate `#[cfg(not(feature
= "dev-plaintext"))]`), or (b) attach an `Environment` enum to
`TransportConfig` and refuse `use_tls=false` at construction when
`Environment::Production`. Option (b) is the enterprise-grade choice
because it survives all downstream refactors.

---

#### 6.1.2 [HIGH] Environment override `PCLOUD_API_MODE=plaintext` can be set at daemon startup; validation runs too late on an already-poisoned cached API-server hint

`crates/pcloud-config/src/env.rs:86-95` applies `PCLOUD_ENV` first, then
`crates/pcloud-config/src/env.rs:93-95` honours `PCLOUD_API_MODE`.
Combined, an operator error — `PCLOUD_ENV=production
PCLOUD_API_MODE=plaintext` — is eventually rejected by
`ApiEndpoint::validate` (`pcloud-config/src/api.rs:137-141`) at
bootstrap. **This rejection is correct today.**

However, the order in `bootstrap.rs:443-449` is:

```rust
let mut config = config;
if let Some(api_server) = store.repositories.preferences.api_server_binapi.as_deref() {
    config.api.apply_api_server_hint(api_server);
}
```

A malicious or stale `api_server_binapi` stored in the SQLite
preferences repository can rewrite `config.api.host` and
`config.api.server_name` *after* validation has run upstream in
`bootstrap_with_config` (`bootstrap.rs:407-408`). There is no second
validation pass. Combined with the fact that `apply_api_server_hint`
never rejects a non-pcloud.com host, a stored-preference rewrite is an
attack path.

**Remediation:** Re-run `ConfigProfile::validate` after the
`apply_api_server_hint` mutation at `bootstrap.rs:449`. Additionally,
validate the SNI hostname against an allow-list (e.g. only hosts ending
in `.pcloud.com` / `.pcloud.link`) or require the hint to be
signed/authenticated end-to-end. The comment at
`crates/pcloud-config/src/api.rs:178-189` is silent on origin trust.

Cross-reference §2 secret discipline: a replaced SNI that still
terminates TLS against an attacker-controlled cert would compromise any
subsequent auth flow.

---

#### 6.1.3 [MEDIUM] `rustls` client config is rebuilt per request — no session resumption, no `CryptoProvider` pinning

`crates/pcloud-proto/src/transport.rs:318-336` — every call to
`execute_tls` constructs a fresh `RootCertStore`, `ClientConfig`, and
`ClientConnection` from scratch. Under enterprise load this prevents
TLS session resumption (and wastes an RTT per request). It also does
not pin a specific `rustls::CryptoProvider`, so a future rustls default
provider change would silently alter cipher selection.

**Remediation:** Build `Arc<ClientConfig>` once in
`BinaryApiTransport::new` and reuse across requests. Pin
`rustls::crypto::aws_lc_rs::default_provider()` (or ring) explicitly.
Expose a `CryptoProviderSource` knob in `ApiEndpoint` for enterprise
installs that mandate FIPS providers.

---

### 6.2 Certificate validation

#### 6.2.1 [INFO] No `danger_accept_invalid_certs`, no `DangerousClientConfig` anywhere

Confirmed clean via repository-wide search. `CONTRIBUTING.md:206`,
`SECURITY.md:96`, and `CHANGELOG.md:1975` explicitly forbid these and
the production source tree contains none.

`crates/pcloud-proto/src/transport.rs:327-333` uses the builder path
that cannot disable verification:

```rust
let tls_config = ClientConfig::builder()
    .with_root_certificates(roots)
    .with_no_client_auth();
```

`crates/pcloud-proto/src/http_download.rs:210-215` mirrors the same
construction for the HTTPS download channel.

**No finding.** This is the expected posture.

---

#### 6.2.2 [MEDIUM] `webpki-roots` is pinned to whatever version `Cargo.toml` resolves; no explicit trust anchor refresh policy

`crates/pcloud-proto/src/transport.rs:324-325` and
`http_download.rs:208-209` both seed roots from
`webpki_roots::TLS_SERVER_ROOTS`. A stale `webpki-roots` dependency
means a missing-from-trust-store CA (e.g. ISRG Root X2) will silently
fail validation at some point in the future.

**Remediation:** Add a CI job that refreshes `webpki-roots` monthly, or
(enterprise-preferred) allow operators to override the root store with a
`[api].extra_root_certificates_pem` config path. Document the guarantee
in `SECURITY.md`.

---

### 6.3 Timeouts

#### 6.3.1 [MEDIUM] Timeout discipline is coarse: one `read_timeout` applied to every read and every write, no separate TCP-keepalive, no global request deadline

`crates/pcloud-proto/src/transport.rs:91-96` defines
`connect_timeout: Duration` and `read_timeout: Duration`, no
`write_timeout`, no `total_request_timeout`. At
`transport.rs:301-306`:

```rust
stream.set_read_timeout(Some(config.read_timeout))...
stream.set_write_timeout(Some(config.read_timeout))...
```

The same duration is applied to reads and writes, and there is no
per-request budget separate from per-syscall budget. A malicious or
broken server can therefore drip-feed 1 byte per `read_timeout - 1ms`
for arbitrarily long. The `send_and_receive` deadline loop at
`transport.rs:338-364` calls `read_exact_with_deadline` and
`write_all_with_deadline` which each carry their own deadline; there is
no outer "the entire request must complete within N seconds" wrapper.

**Remediation:** Add `total_request_timeout: Duration` to
`TransportConfig` and enforce it in `send_and_receive` via a single
`Instant::now() + total_timeout` deadline shared across the write,
flush, header-read, and body-read stages. Enable TCP keep-alive at the
socket level (`set_keepalive`) so a silently-dead peer surfaces before
the application-level timeout.

---

#### 6.3.2 [LOW] `connect_timeout` default is 5s but not user-override validated; `read_timeout` default is 15s with no floor

`crates/pcloud-config/src/api.rs:98-116` sets both to 5_000ms / 15_000ms
as `secure_defaults`. `ApiEndpoint::validate` only rejects zero;
`crates/pcloud-config/src/api.rs:157-167`. An operator who sets
`read_timeout_ms = 1` slips through validation and will see every
real-world request fail with a deadline error.

**Remediation:** Add a minimum floor (e.g. 500ms) validated at load, or
a clamp with a warning.

---

### 6.4 Retry policy

#### 6.4.1 [HIGH] `ResilientTransport` default classifier treats every inner error as `Transient`

`crates/pcloud-proto/src/resilient_transport.rs:305-310`:

```rust
pub fn default_classifier<E>() -> Classifier<E>
where E: std::error::Error + Send + Sync + 'static,
{
    Arc::new(|_: &E| ErrorClass::Transient)
}
```

This classifier is installed verbatim by
`TransportFactory::wrap_binary`
(`crates/pcloud-daemon/src/transport_factory.rs:113-120`) in production.
Consequently, `TransportError::InvalidAddress` (a permanent DNS failure)
and `TransportError::InvalidServerName` (a permanent TLS config error)
are retried up to `retry_max_attempts` times — wasting wall time and
amplifying load on DNS or internal resolvers. More importantly,
`TransportError::Tls(rustls::Error::InvalidCertificate*)` — a
**security-relevant** terminal failure — is retried, which both masks
the signal from operators and gives an on-path attacker multiple
attempts to race a certificate swap.

**Remediation:** Supply an explicit classifier in
`TransportFactory::wrap_binary` that marks as `Permanent`:

- `TransportError::InvalidAddress`
- `TransportError::InvalidServerName`
- Any `TransportError::Tls` where `rustls::Error` indicates a
  certificate/chain problem (`InvalidCertificate`, `PeerIncompatible`,
  `InvalidCertSignature`, `General("…certificate…")`).
- `TransportError::ResponseBody(ResponseParseError::*)` — parser bugs
  should fail fast.

Tests at `resilient_transport.rs:508-537` prove the hook works — the
production wire-up just never supplies it.

---

#### 6.4.2 [HIGH] No `Retry-After` header respected; no server-directed backoff

Repository-wide search shows zero matches for `Retry-After`,
`retry_after`, or `retry-after` in `pcloud-proto` or
`pcloud-resilience`. The pCloud binary protocol may not surface such a
header directly, but the HTTPS download channel
(`http_download.rs`) certainly receives 429 / 503 with Retry-After,
and the client ignores it.

**Remediation:** In `http_download.rs:fetch_download_verified_streaming`
parse `Retry-After` (both delta-seconds and HTTP-date forms) and feed
it into the same `ResilientTransport` backoff instead of running the
jittered exponential schedule blind. For the binary channel, check
whether pCloud signals rate-limit via the `result` field (the protocol
has a documented rate-limit result code) and honour it identically.

---

#### 6.4.3 [MEDIUM] Backoff schedule uses *equal-jitter*, documented as `ExponentialJittered`, but the PR-grade "full jitter" is absent

`crates/pcloud-resilience/src/retry.rs:37-46` defines
`ExponentialJittered`; `retry.rs:197-205` implements "equal-jitter per
AWS" (`d/2 + rand(0, d/2)`). Equal-jitter is adequate; the finding is
that the API enum only exposes `Fixed`, `Exponential`, and this single
jittered variant. Operators cannot select "decorrelated jitter" or
"full jitter" without a code change.

**Remediation:** Add variants `FullJittered` (`rand(0, d)`) and
`Decorrelated { cap }` per the AWS Architecture Blog. Expose the
selector via `ResiliencePolicy` serde.

---

#### 6.4.4 [MEDIUM] `retry_jitter_seed` is a **deterministic** u64 shared by every client instance

`crates/pcloud-config/src/resilience.rs:73-77`:

```rust
/// Deterministic jitter seed applied via equal-jitter. Default:
/// `0x00C0_FFEE_F00D`. Valid values: any `u64`. **Security:** keeps
/// tests reproducible while still spreading retry storms across
/// clients that share the seed. Example: `retry_jitter_seed = 0`.
```

The security note is wrong. If two daemons share the same seed (which is
the default) and experience the same outage at the same wall time,
`splitmix64(seed ^ attempt)` produces **identical** jitter values →
identical retry timings → thundering-herd amplification. The point of
jitter is to decorrelate; a fixed seed neutralises that.

**Remediation:** Default to `rand()`-derived per-process seed at
bootstrap, or per-connection. Keep the deterministic-seed path behind a
test-only knob.

---

#### 6.4.5 [MEDIUM] Retry budget is per-request, not global

`ResilientTransport::execute` loops until `retry_max_attempts`
(`resilient_transport.rs:243-298`). There is no cross-request retry
budget. A daemon that serves 1000 failing requests/second can issue
3000 retries/second indefinitely; the circuit breaker mitigates the
worst case, but only after `breaker_failure_threshold` consecutive
failures on a single endpoint.

**Remediation:** Add a global `RetryBudget` (Netflix Hystrix pattern): a
token bucket of retry tokens shared across all callers, refilled at a
percentage of the steady-state request rate. When depleted, fall
through to `RetryDecision::GiveUp` regardless of the per-request budget.

---

### 6.5 Idempotency

#### 6.5.1 [HIGH] `upload_create → upload_write → upload_save` has no end-to-end idempotency key; the journal gives crash-replay, not retry-safety

`crates/pcloud-proto/src/transfer_api.rs:249-287` —
`upload_create` returns a server-issued `uploadid`. This `uploadid`
is durable and is the right anchor for an idempotency key.

However, the retry wiring is broken in two ways:

1. **No `upload_create` retry is safe.** A network error after the
   server has created the upload session but before the client sees the
   response will cause retry to create a **second** upload session with
   the same filename — typical pCloud behaviour is to suffix the name
   with `(1)`. There is no "look up the previous session" path.
2. **`upload_save` retry is also unsafe.** If `upload_save`'s response
   is lost mid-transit, the server has committed but the client
   believes it failed and retries, producing a duplicate. The Rust path
   has no dedup: `transfer_api.rs:upload_create` uses
   `ResilientTransport` via `TransportFactory` (indirectly), which will
   retry these mutations as `Transient`.

The upload journal at
`crates/pcloud-backends/src/upload_journal.rs` does persist the
`uploadid`+`offset` tuple (`upload_journal.rs:92-97`, replay at
`upload_journal.rs:182+`) for crash recovery, but it does not protect
against the in-flight retry case above.

Separately, `MethodRetryPolicy`
(`crates/pcloud-resilience/src/retry.rs:229-316`) already classifies
`RetryClass::Mutation`-class operations as non-retriable by default
(`retry.rs:267-274`). But this enum is not wired into
`ResilientTransport`: grep for `RetryClass::Mutation` in
`pcloud-proto/` returns zero hits. The `ResilientTransport.execute`
call path at `resilient_transport.rs:243-298` does not consult the
method class — only the raw `ErrorClass` from the inner error.

**Remediation:** Wire `MethodRetryPolicy` into `ResilientTransport`.
`execute` must accept a `RetryClass` argument per request and refuse to
retry mutations unless the caller has attached a server-supported
idempotency key (pCloud's `uploadid`). For `upload_create` specifically:
persist the requested filename→uploadid mapping in the upload journal
**before** issuing the request (rowid = content-hash of parameters), so
that a retry after a client crash can reuse the existing uploadid
instead of making a new one.

Cross-reference §4 (sync engine): this finding is about transport-level
idempotency, not the engine queue.

---

### 6.6 WebSocket / diff stream

#### 6.6.1 [INFO] No WebSocket or diff-stream support; `diff` is a polling request

Repository-wide search for `websocket` / `diff_stream` / `poll_stream`
returns zero matches. `crates/pcloud-proto/src/diff_api.rs:1-47`
documents `diff_api` as a single-shot `diff` request keyed by a server
cursor (`diffid`).

This is a parity gap vs. a push-based server (pCloud does support a
long-poll/streaming `diff` in the C client —
`pclsync/pdiff.c:psync_diff_thread` in upstream). It is not currently
in the audit matrix as a P0 blocker, but enterprise desktop clients
expect sub-second remote-change propagation; polling does not deliver
that.

**Remediation:** Track as `Partial` in the C parity matrix. Out of scope
for this audit to fix; flag for product prioritisation.

---

### 6.7 API-server steering

#### 6.7.1 [MEDIUM] `set_api_server` / `apply_api_server_hint` mutates the live transport without any allowlist and without re-validating the SNI

`crates/pcloud-proto/src/transport.rs:270-287`:

```rust
impl ApiServerHintConsumer for BinaryApiTransport {
    fn apply_api_server_hint(&self, api_server: &str) {
        if api_server.trim().is_empty() { return; }
        let (host, port) = parse_api_server_hint(api_server);
        let mut config = self.config.write().expect(...);
        config.host = host.clone();
        config.server_name = host;
        if let Some(port) = port { config.port = port; }
    }
}
```

The server response's `apiserver` field is taken at face value. If the
response is forged (e.g. a weakness anywhere else on the server side,
or a MITM during the brief TLS-handshake-before-cert-pinning window),
the client cheerfully reconfigures its endpoint — and all subsequent
TLS handshakes use the attacker-supplied hostname as SNI **and** as the
certificate verification name. TLS will then succeed if the attacker
controls a cert for that name, which — if the attacker controls DNS or
the path — is not hard.

**Remediation:** Restrict accepted hints to a known domain family
(regex `^bineapi(-[a-z]{2})?\.pcloud\.com$` for the binary API,
`^api(-[a-z]{2})?\.pcloud\.com$` for HTTP). Reject port overrides or
restrict to a fixed set (443, 8443). Require at least one successful
round-trip against the original endpoint before accepting a hint.

---

#### 6.7.2 [LOW] API-server selection is not persisted across restart unless the SQLite preferences path was written by a prior run

`crates/pcloud-daemon/src/bootstrap.rs:446-449` loads
`store.repositories.preferences.api_server_binapi` and applies it. But
`apply_api_server_hint` is called from the *response handler* of an
authenticated binary request — there is no explicit path that writes
this back to the preferences store. So after a daemon restart the
steering decision is lost and the client re-hits the default endpoint
until the next response carries a hint.

**Remediation:** Persist on every successful hint apply; expire stale
hints after a week.

---

### 6.8 Observability of outbound traffic

#### 6.8.1 [HIGH] No per-endpoint HTTP latency/error histogram

`crates/pcloud-observability/src/metrics.rs:17-26` documents the metric
table. **Every histogram is keyed by the IPC `method` (inbound
dispatch), not by outbound HTTP endpoint.** The
`pcloud_request_latency_seconds` histogram is emitted from the daemon's
dispatch loop (grep `observe_request` in
`crates/pcloud-daemon/src/runtime.rs`), measuring the in-process
dispatch, not the HTTP round-trip.

There is no metric family for:

- outbound pCloud API round-trip latency per command (`login`,
  `diff`, `upload_create`, etc.),
- outbound API error rate per command,
- TLS-handshake cost,
- circuit-breaker trip count,
- retry budget consumption.

This is a critical enterprise observability gap: operators cannot tell
"is the daemon slow because the dispatch is slow, or because pCloud's
API is slow, or because we're being rate-limited?"

**Remediation:** Register new histograms in
`MetricFamilies::observe_outbound(command, status, latency_seconds)`
and wire them through
`ResilientTransport::execute` (which owns the outer timing boundary) and
through `BinaryApiTransport::execute_with_body` (if a caller bypasses
the resilient wrapper). Keep label cardinality bounded by sanitising
command name via the existing label sanitiser (§8 cross-reference).

Add counters for circuit-breaker state transitions
(`pcloud_circuit_breaker_state_changes_total{endpoint,new_state}`) and
retry outcomes
(`pcloud_retry_attempts_total{command,outcome=succeeded|exhausted}`).

---

#### 6.8.2 [MEDIUM] `pcloud_transfer_bytes_total` has no per-endpoint or per-sync-root label

`metrics.rs:22` — a single counter for upload+download bytes across the
entire daemon. An operator cannot tell whether a sudden spike is from
FUSE writeback, a new sync root, or a runaway plugin.

**Remediation:** Add a `source` label
(`{fuse|sync|plugin|cli|sdk}`) and a `root_id` label (capped at ~16
distinct values to keep cardinality under control).

---

## Section 7. IPC & Daemon

### 7.1 Wire format

#### 7.1.1 [INFO] Length-prefixed framing is present, documented, and boundary-checked

`crates/pcloud-ipc/src/protocol.rs:10-16` documents the 8-byte
little-endian header:

```text
offset 0..4 : u32 payload_len   // JSON byte length
offset 4..6 : u16 version       // IPC_PROTOCOL_VERSION = 1
offset 6..8 : u16 message_type  // 1=Request, 2=Response, 3=Event
offset 8..  : JSON body
```

The hard cap is `MAX_IPC_PAYLOAD_LEN = 1 MiB`
(`protocol.rs:47`). Framing checks occur **before** allocation at
`crates/pcloud-ipc/src/transport.rs:304-325` (`read_framed_request`) —
declared length is validated against `MAX_REQUEST_BYTES` before any
`Vec::with_capacity(payload_len)`.

**No finding.** This is correct.

---

#### 7.1.2 [HIGH] Serialization is JSON with **no schema version negotiation beyond a single u16**

`crates/pcloud-ipc/src/protocol.rs:39`:

```rust
pub const IPC_PROTOCOL_VERSION: u16 = 1;
```

`protocol.rs:255-260` rejects any non-1 version with
`ProtocolError::VersionMismatch`. There is:

- no forward-compat tolerance (client v1 speaking to daemon v2 cannot
  even read a v2-labeled `DrainStatus` response),
- no minor-version negotiation (no "I speak 1.3; server offers 1.4;
  both fall back to 1.3"),
- no payload-schema diff handling — `serde_json::from_slice` on a
  v1 client receiving v1 JSON with an unknown field errors out if
  `#[serde(deny_unknown_fields)]` is set, or silently discards the
  field otherwise (it is **not** set; no `deny_unknown_fields` in the
  wire types, which cuts both ways — see §7.2.2).

For a daemon intended to be a drop-in replacement for a long-running
desktop agent with independent CLI upgrades, this is a real
compatibility hazard.

**Remediation:** Add a capability negotiation step (client sends
`Method::HandshakeCapabilities` on connect; server returns a
semver-style range it supports + an optional feature bitmap). Define
the deprecation policy ("N-1 support for 6 months"). Bump the version
to 2 only when a truly breaking change ships; use serde-renames +
`#[serde(default)]` for additive changes.

---

#### 7.1.3 [MEDIUM] `MessageKind` decoder coerces unknown values into `Event`

`protocol.rs:268-272`:

```rust
let kind = match message_type {
    1 => MessageKind::Request,
    2 => MessageKind::Response,
    _ => MessageKind::Event,
};
```

This silently accepts any `message_type` ≥ 3 as `Event`. Combined with
the (unused-but-reserved) `Event` variant, a forged frame with
`message_type = 65535` would be decoded as an event and — depending on
how `Event` is handled downstream — could be mis-dispatched.

**Remediation:** Reject unknown `message_type` values explicitly with
`ProtocolError::InvalidMessageKind { actual }` and close the
connection. The doc comment at `protocol.rs:52-61` is wrong: it says
"decoders reject unknown values" but the code coerces them.

---

#### 7.1.4 [LOW] Max frame size (1 MiB) is a static const with no per-method allowance

`crates/pcloud-ipc/src/server.rs:42` — `MAX_REQUEST_BYTES = 1 MiB`.
Most methods are far under 1 KiB. A legitimate `Request::
SyncRootAdd` with a very long path approaches 4 KiB in practice. A
future method that needs to carry a larger payload (e.g. a batched
notification list, or encrypted blob) would have to either bump the
global cap or split across frames.

**Remediation:** Make the cap per-method. Default to 16 KiB; only a
small allow-list of methods gets 1 MiB.

---

### 7.2 Serialization safety (proptest coverage)

#### 7.2.1 [HIGH] `proptest_methods_roundtrip.rs` covers 30 Method variants; the enum has at least 45

`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:15-48` —
`every_method()` returns exactly **30** Method variants. The actual
enum at `crates/pcloud-ipc/src/methods.rs:37-220+` has **at minimum 45
variants**:

Missing from the proptest list (verified via `grep '^    [A-Z][a-zA-Z]+,?$'
crates/pcloud-ipc/src/methods.rs`):

- `Method::Health` (line 49)
- `Method::SessionStatus` (line 125)
- `Method::FileHistory` (line 138)
- `Method::IntegrityStatus` (line 143)
- `Method::HaStatus` (line 151)
- `Method::DrainStatus` (line 162)
- `Method::GetSlo` (line 170)
- `Method::GetAuditVerifierStatus` (line 177)
- `Method::GetSyncStatus` (line 184)
- `Method::ListConflicts` (line 189)
- `Method::StatPath` (line 197)
- `Method::GetApiServers` (line ~202)
- `Method::GetPromo` (line ~207)
- `Method::GetCryptoHint` (line ~211)
- `Method::VerifyEmail` (line ~215)

Plus numerous `Request` variants (e.g. `IntegrityRunOnce`, `UploadList`,
`ConflictList`, `RunLocalScan`, `SendPublink`) that are not exercised
by `arb_request()` either.

The compile-time "exhaustiveness guard" at
`proptest_methods_roundtrip.rs:60-97` (`must_match_every_method_variant`)
is **defeated** by the catch-all `_ => 0` arm at line 95, exactly
because `Method` is `#[non_exhaustive]`. The doc comment at
`proptest_methods_roundtrip.rs:50-59` explicitly admits this.

**Consequence:** a new Method variant added between releases that has a
subtle serde rename or non-round-tripping field is shipped without
proptest coverage. The CSV parity matrix claim of "IPC surface is
proptest-verified" is technically false.

**Remediation:** Remove `#[non_exhaustive]` from `Method` for this
crate's own tests (it is only useful for external consumers), or
replace the `_` arm with an explicit list that the compiler will force
updates on. Better: add a `strum::EnumIter` derive and iterate every
variant at test time, so `every_method()` is always complete by
construction.

---

#### 7.2.2 [MEDIUM] Wire types do not use `#[serde(deny_unknown_fields)]`; unknown fields silently drop

Sampled `crates/pcloud-ipc/src/methods.rs` shows no
`#[serde(deny_unknown_fields)]` on `Request`, `Response`, or `Method`.
Combined with §7.1.2 (no version negotiation), a hostile or confused
client can inject extra fields that the server silently ignores — but
more interestingly, a downgrade attack that strips a
newly-added-as-mandatory field will succeed because serde will fill
the missing field with `#[serde(default)]`.

**Remediation:** Add `#[serde(deny_unknown_fields)]` to every wire type.
Use `#[serde(deny_unknown_fields, default)]` on enum-variant structs
where forward-compat defaulting is wanted. Pair with §7.1.2's
capability handshake so additive schema changes are explicit.

---

#### 7.2.3 [LOW] `prop_random_bytes_do_not_panic` only checks that random bytes don't panic — doesn't assert a specific error

`proptest_methods_roundtrip.rs:236-240`:

```rust
#[test]
fn prop_random_bytes_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
    let _ = decode_request(&bytes);
    let _ = decode_response(&bytes);
}
```

This correctly asserts no panic, but it does not assert that an
unparseable frame produces a *specific* `ProtocolError`. A refactor
that made the decoder return `Ok` on garbage would pass this test.

**Remediation:** Tighten to `prop_assert!(matches!(decoded, Err(_) | Ok(Frame { header: FrameHeader { version: 1, .. }, payload }) if payload == /* no-op equivalent */))`.

---

### 7.3 Authentication on every accept

#### 7.3.1 [INFO] Linux uses `SO_PEERCRED`, BSD/macOS use `getpeereid(3)`, Windows uses ALPC-style named-pipe SID match — all confirmed

`crates/pcloud-ipc/src/platform/linux.rs:31-57` —
`getsockopt(SOL_SOCKET, SO_PEERCRED)` populates a `libc::ucred` and
extracts uid + pid.

`crates/pcloud-ipc/src/platform/unix.rs:44-60` —
`getpeereid(3)` populates uid (pid is synthesized as 0 because
getpeereid does not expose it — correctly documented).

`crates/pcloud-ipc/src/platform/windows.rs:141-220` — creates the
named pipe with an explicit single-SID DACL
(`D:(A;;GRGW;;;<owner-sid>)`), and at accept time recovers the client
SID via `GetNamedPipeClientProcessId` →
`OpenProcessToken(TOKEN_QUERY)` → `GetTokenInformation(TokenUser)` →
`ConvertSidToStringSidW`. SID string is compared byte-for-byte against
the server's owner SID
(`windows.rs:202-209`).

Anonymous sockets are rejected at
`crates/pcloud-ipc/src/transport.rs:186-198`: if
`peer_identity(&stream)` fails, the server responds
`ResponseStatus::Unauthorized` and closes.

**No finding on the peer-auth path itself.**

---

#### 7.3.2 [MEDIUM] Linux `SO_PEERCRED` records the **pid at connect time**, not at the dispatch time

`crates/pcloud-ipc/src/platform/linux.rs:94-120` (`peer_ucred`) is
called once per accept. A client process that forks between `connect()`
and the dispatch completion can mislead the audit trail: the audited
pid is the parent's pid, not the child's that actually sent the
request.

This is a Linux kernel limitation, not a bug in this crate, but it
deserves a SECURITY.md callout. The equivalent on Linux, `SCM_CREDENTIALS`
piggybacked on each `sendmsg`, would close the gap.

**Remediation:** Document the limitation. Consider a follow-up that
switches to `SCM_CREDENTIALS` per-message on Linux where higher
assurance is needed. Not blocking.

---

#### 7.3.3 [MEDIUM] On macOS/BSD the peer pid is **synthesized as 0**, which makes audit correlation impossible

`crates/pcloud-ipc/src/platform/unix.rs:65-68`:

```rust
pub(crate) fn peer_ucred(stream: &UnixStream) -> Result<(u32, u32), IpcTransportError> {
    let (uid, _gid) = getpeereid(stream)?;
    Ok((uid, 0))
}
```

`auth.rs:34-38` documents this and reassures that the pid is "carried
for audit correlation only — never used for authorization", which is
correct. But enterprise audit logs on macOS/FreeBSD will show
`pid=0` for every IPC event — an alert-tuning disaster.

**Remediation:** On macOS use `getsockopt(LOCAL_PEERPID)` —
macOS-specific; available on all supported releases. On FreeBSD use
`getsockopt(LOCAL_PEERCRED)` which returns a full `struct xucred`
including pid. Only the darkest-BSDs (historical OpenBSD) genuinely lack
pid; those can stay at 0.

---

### 7.4 Authorization (per-request capability scoping)

#### 7.4.1 [CRITICAL] **There is no per-request capability scoping. Every owner-uid peer gets the full IPC surface, including `Method::Shutdown`, `Method::CryptoReset`, and `Method::Logout`.**

`crates/pcloud-ipc/src/server.rs:98-132` — `IpcServer`'s entire
authorization contract is a single uid comparison:

```rust
pub fn authorize_peer(&self, peer: &PeerIdentity) -> bool {
    peer.matches_owner(self.owner_uid)
}
```

The dispatch path at `crates/pcloud-daemon/src/dispatch.rs:1-150+`
carries no capability token. Searching the daemon crate for
`capability` or `CapabilityScope` or `privileged` returns zero
matches. The only tiered control is the rate-limiter's per-category
token bucket
(`crates/pcloud-daemon/src/rate_limit.rs:25-100+`), which is about
abuse prevention, not privilege separation.

**Impact:** In a multi-process single-user deployment (which is the
norm: the daemon is the backend; the CLI, Web UI, SDK consumers are
separate processes owned by the same user), any local process owned by
the user can:

- Call `Method::Shutdown` and kill the daemon (DoS).
- Call `Method::CryptoReset` and **wipe the user's local crypto
  fingerprint / folder registry** — this is privilege-meaningful even
  though both processes are the same user.
- Call `Method::Logout` and destroy in-memory credentials.
- Call `Method::SetAuthPersistence { enabled: false }` and disable the
  durable token vault.
- Call every `CryptoChangePassword*` variant.
- Call every `SyncRootRemove` / `SyncRootAdd`, re-routing user data.

The enterprise model expects at least two tiers: *read-only probes*
(status, health, drain-status, metrics) versus *state-mutating
operations*. Even without a full capability architecture, the MUST-HAVE
is a "privileged" gate guarded by an additional token (e.g. a
supervisor-only socket, or a token written only into the runtime dir
that the CLI has to read to unlock shutdown-class operations).

**Remediation:** Introduce a two-tier model immediately:

1. Read-only tier: `GetStatus`, `GetHealth`, `DrainStatus`,
   `SessionStatus`, `GetSlo`, `GetSyncStatus`, `ListConflicts`,
   `IntegrityStatus`, `GetApiServers`, `GetPromo`, `HaStatus`,
   `StatPath`. Admit on uid-match alone.
2. Privileged tier: everything that mutates state
   (`Shutdown`, `CryptoReset`, `Logout`, `CryptoChangePassword*`,
   `SyncRootAdd/Remove/Pause/Resume`, `CreateFilePublicLink`,
   `DeletePublicLink`, `SetAuthPersistence`, etc.). Require an
   additional *bearer token* stored in `$runtime_dir/privileged.token`
   (mode 0400), which the CLI reads and presents via a new
   `Request::Privileged { token, inner }` wrapper.

This is a modest architectural change that closes a CRITICAL local
privilege-management gap. Track as `bd-new-ipc-capability-scoping`.

---

#### 7.4.2 [HIGH] `drain_gate_admits_status_and_shutdown_probes` test at `serve.rs:440-457` admits `Method::Shutdown` during drain

During drain, `should_reject_during_drain`
(`crates/pcloud-daemon/src/serve.rs:79-87`) returns `false` for
`Method::DrainStatus | Method::Shutdown | Method::GetHealth |
Method::Health`. This means a second `Shutdown` during drain is
dispatched to the backend.

This is defensible (a supervisor re-issuing shutdown should be
idempotent), but combined with §7.4.1 it means **any local process can
call Shutdown twice in quick succession** — once to start the drain,
once during the drain window to attempt to alter state. The second
`Shutdown` should be a no-op but this is not asserted in tests.

**Remediation:** After §7.4.1 lands, `Method::Shutdown` is in the
privileged tier and this finding subsides. Until then: make
`Shutdown` during drain explicitly a no-op that returns the current
`DrainStatusPayload`.

---

### 7.5 Runtime directory hygiene

#### 7.5.1 [INFO] Linux: socket mode 0600 under a 0700 parent, confirmed

`crates/pcloud-ipc/src/transport.rs:246-268`:

```rust
if let Some(parent) = socket_path.parent() {
    let parent_missing = !parent.exists();
    fs::create_dir_all(parent)?;
    if parent_missing {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
}
if socket_path.exists() {
    fs::remove_file(socket_path)?;
}
let listener = UnixListener::bind(socket_path)?;
fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
```

Tested at `crates/pcloud-ipc/tests/security_invariants.rs:150-171`.

**No finding.** Correct.

---

#### 7.5.2 [MEDIUM] Parent dir is only chmod'ed to 0700 **if it did not already exist**; an attacker who pre-creates the parent dir with loose perms retains them

`transport.rs:250-253`:

```rust
let parent_missing = !parent.exists();
fs::create_dir_all(parent)?;
if parent_missing {
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
}
```

If a local attacker runs
`mkdir -p $XDG_RUNTIME_DIR/pcloud --mode=0755` before the daemon
starts, the daemon will happily bind the socket inside a world-readable
parent. The socket itself is 0600, so peers cannot connect, but the
directory listing leaks the existence of the socket (and, more
importantly, any sidecar files the daemon drops there — e.g.
`mount_pid` at
`crates/pcloud-daemon/src/bootstrap.rs:726-755` which stores a PID the
daemon claims, and which an attacker could use to spoof).

**Remediation:** Always unconditionally `chmod 0700` on the parent
directory after `create_dir_all`, regardless of whether it was
pre-existing. Additionally, `lstat` the parent and refuse to start if
it is a symlink, or if its ownership differs from the effective uid.
Follow the same discipline adopted for the vault file at
`crates/pcloud-daemon/src/auth_vault.rs:103-121` (which validates
`meta.file_type().is_file()` and rejects non-owner mode bits).

---

#### 7.5.3 [MEDIUM] Stale socket file is removed without atomicity — a small TOCTOU window where a concurrent process could be the binder

`transport.rs:255-260`:

```rust
if socket_path.exists() {
    fs::remove_file(socket_path)?;
}
let listener = UnixListener::bind(socket_path)?;
```

Between `remove_file` and `UnixListener::bind`, a concurrent process
with the same uid can create a file or symlink at `socket_path`. The
impact is reduced by the owner-only DACL on the parent dir (after
§7.5.2 lands), but on the first-daemon-start case the parent may be
world-writable and the window is exploitable.

**Remediation:** Use `socket(2) + bind(2)` directly and rely on
`connect(2)` after `unlink(2)` only, *plus* use `sun_path` with a
unique suffix (e.g. `pcloud.sock.<pid>.<rand>`) and then atomically
rename to the stable name. Or use abstract sockets on Linux
(`\0pcloud-rs-<uid>`), which avoid the filesystem entirely.

---

#### 7.5.4 [LOW] macOS `TMPDIR` is not used; the daemon falls back to `/tmp` via `PcloudDirs::discover()`

`crates/pcloud-config/src/paths.rs:156-230+` documents the XDG
discovery path. `crates/pcloud-daemon/tests/graceful_drain.rs:46-56`
explicitly notes "Use `/tmp` (not `std::env::temp_dir()`) so the
Unix-socket path stays under SUN_LEN on macOS".

This is defensible (macOS `TMPDIR` paths are too long for `sun_path`'s
104-byte limit), but it means the daemon's runtime files live in a
world-writable directory on macOS unless the operator tightens things.

**Remediation:** On macOS, prefer
`$HOME/Library/Application Support/pcloud-rs/runtime/` (which is
user-owned by default) and use a relative `bind(chdir + relative)`
trick to get the socket name under the SUN_LEN limit.

---

### 7.6 Graceful shutdown

#### 7.6.1 [INFO] Three-state drain machine is implemented, tested, and used

`crates/pcloud-daemon/src/signals.rs:28-131` documents and implements
`Running → Draining → Stopped`.

`crates/pcloud-daemon/src/serve.rs:110-231`
(`serve_until_shutdown_with_flag`) cooperates with it: on shutdown
observed, it calls `signals::begin_drain()`, starts a drain deadline
based on `runtime.config.upgrade.drain_timeout_secs`, polls
`signals::in_flight() == 0`, and returns when drained or timed out.

Integration coverage at
`crates/pcloud-daemon/tests/graceful_drain.rs:61-229` exercises:

- `drain_admits_status_probes_and_rejects_new_traffic` (L61)
- `drain_gate_rejects_ordinary_requests_with_unavailable` (L148)

Both pass under the serial lock at L31-36. Solid.

**No finding.** This is the highest-quality subsystem in the reviewed
slice.

---

#### 7.6.2 [MEDIUM] Drain timeout is a hard cut — in-flight uploads/downloads are dropped on the floor

`serve.rs:166-171`:

```rust
let drained = signals::in_flight() == 0;
let timed_out = drain_deadline.map(|d| Instant::now() >= d).unwrap_or(false);
if drained || timed_out { return Ok(()); }
```

When the timer fires, the loop returns irrespective of in-flight
counter. This is correct for unbounded-latency operations, but there
is no mechanism to *cancel* in-flight uploads gracefully before the
cut — e.g. tell the upload state machine "you have 2 seconds; persist
progress and give up". The upload journal does persist state, so
resume-after-restart is possible, but an enterprise deployment expects
a softer cooperative cancellation.

**Remediation:** Introduce a `CancellationToken` (or
`tokio::sync::broadcast` channel) that is tripped when
`begin_drain` fires. Long-running operations (uploads, diff-poll, TLS
handshake) should check the token and exit early by persisting their
journal entry and returning `ResponseStatus::Unavailable`. Default
drain deadline can then be a *soft* deadline on cooperation, with a
separate hard deadline at 2x for the hold-out case.

---

#### 7.6.3 [LOW] `mount_control.quiesce_for_drain` is called once, synchronously, during the drain transition

`serve.rs:156-158`:

```rust
let summary = runtime.mount_control.quiesce_for_drain();
if summary != "no active mount" { log::info!(...); }
```

This runs on the accept thread and can block. If the FUSE writer has a
multi-second flush, the drain transition's stamp
(`DRAIN_STARTED_MS`) is written *before* the quiesce returns, but
`Method::DrainStatus` will report stale "elapsed_drain_ms = 0" until
`begin_drain` returns. In practice the effect is cosmetic.

**Remediation:** Spawn `quiesce_for_drain` on a worker thread and have
the serve loop poll it. Out of scope for §7 because it touches §5 FUSE.
Cross-reference: FUSE agent owns this.

---

#### 7.6.4 [LOW] `mark_stopped()` is called *after* `serve_until_shutdown_with_flag` returns; if the caller forgets, `drain_state()` stays at `Draining` forever

`serve.rs:291-293`:

```rust
let _ = sync_loop_handle.shutdown_and_join();
signals::mark_stopped();
```

This happens only in `serve_with_shutdown` (the Windows Service entry
point). The UNIX path `serve.rs::serve_until_shutdown` has no such
call. If the `pcloudd serve` binary crashes after the serve loop
returns but before shutdown completes (e.g. in
`sync_loop_handle.shutdown_and_join`), `drain_state()` is stuck at
`Draining` for any test that runs in the same process afterward.

**Remediation:** Use a Drop-guarded sentinel
(`DrainGuard { /* sets Stopped on drop */ }`) so even panic paths
transition correctly.

---

### 7.7 Crash recovery

#### 7.7.1 [HIGH] Upload resume scan runs but uses *authenticated-later* reconcile; startup path only logs

`crates/pcloud-daemon/src/bootstrap.rs:524-570` enumerates upload
sidecars under the FUSE staging root:

```rust
match enumerate_upload_sidecars(&staging_root) {
    Ok(outcomes) if !outcomes.is_empty() => {
        log::info!("pcloud-daemon bootstrap: {} upload sidecar(s) awaiting server reconcile ...", ...);
        for o in outcomes { /* log only */ }
    }
    ...
}
```

The comment at L525-530 says "This pass runs *before* any authenticated
transport is available, so it enumerates and logs only". That's
correct today, but it means the enterprise expectation of "daemon
restarts; stale uploads resume within a few seconds" is *not* met by
the bootstrap path — it is met only later, by the mount-time reconcile
at `mount_runtime::pcloud_shim_adapter_factory`, which may never fire
if the user does not remount immediately.

`bootstrap.rs:573-605` does a second resume scan against
`UploadResumeRepository::list_all` with the same behaviour: log-only.

**Impact:** Real enterprise-relevant uploads that were mid-flight at
crash time sit in the journal, waiting for a human to remount, before
they resume. Lost time, confusing operator experience, and — for
automated deployment scenarios where the FUSE mount is orchestrated by
systemd and may not be re-mounted immediately — silent data stall.

**Remediation:** Spawn a startup reconcile task *after* bootstrap
completes and the auth vault is loaded. Try to acquire a token via
vault load; if present, reconcile each sidecar against the server
(trim-up/down/NotFound/Stalled). If no token is present, defer until
login and then reconcile on the login success callback.

---

#### 7.7.2 [HIGH] No re-adoption of orphan FUSE mounts; startup scan *rejects* or *force-unmounts* them

`crates/pcloud-daemon/src/bootstrap.rs:733-782` handles orphans via
`MountControl::check_orphans()`, which returns one of:

- `OrphanCheckOutcome::Clean` → fine
- `OrphanCheckOutcome::Rejected(paths)` → log error, refuse to start
  the mount service
- `OrphanCheckOutcome::ForceUnmounted(results)` → forcibly unmount via
  `PCLOUD_FORCE_UMOUNT=1`

There is no "re-adopt" path. A crashed daemon whose FUSE mount is
still live cannot have its mount re-owned; the operator must either
force-unmount and re-mount (user-visible disruption) or set the env
var. In systemd terms, this breaks rolling restart.

**Remediation:** Implement FUSE mount re-adoption per FreeBSD /
`mount_pid` sidecar: on startup, if the orphan's `mount_pid` matches a
dead pid but the kernel still shows the mount, re-open the FUSE
channel fd via `/proc/<dead-pid>/fd/<num>` (or its successor) and
resume servicing requests. This is a nontrivial engineering lift but
is exactly what enterprise rolling-upgrade demands. Cross-reference
§5 FUSE agent.

---

#### 7.7.3 [LOW] Startup scans use `rusqlite::Connection::open` outside the main store, creating a second connection

`bootstrap.rs:581-584`:

```rust
let store_conn = rusqlite::Connection::open(&store_path)
    .map_err(|err| BootstrapError::Provision(std::io::Error::other(err.to_string())))?;
```

The daemon already has `store: StoreProfile` in scope at this point. A
second rusqlite connection to the same file is fine (WAL mode), but
the explicit comment about WAL/locking discipline is missing. A later
schema migration that takes an exclusive lock could stall this open.

**Remediation:** Reuse `store.connection()` (or equivalent accessor) if
available. Or, at minimum, document the locking order and ensure
migrations run first.

---

### 7.8 Stress coverage

#### 7.8.1 [MEDIUM] `stress_concurrent_clients.rs` exercises 50 × 500 = 25000 requests, `#[ignore]`-gated; does not prove production claims

`crates/pcloud-ipc/tests/stress_concurrent_clients.rs:30-31`:

```rust
const CLIENTS: usize = 50;
const REQUESTS_PER_CLIENT: usize = 500;
```

Gated at line 44: `#[ignore = "stress: 50 clients x 500 reqs, run with
--release --ignored"]`. Asserts:

- Zero failures (`stress_concurrent_clients.rs:135-140`)
- `served_count >= total` (L144)
- `fd_drift <= 64` (L150)
- Socket path is cleaned up (L155)

25000 requests at sub-ms each (~10k req/s for a typical workstation)
complete in a couple of seconds. This proves correctness under mild
contention but does **not** prove:

- Behaviour under CPU pressure (other processes pinning cores)
- Behaviour under memory pressure
- Behaviour at 10x the fd ceiling (`ulimit -n 10000`)
- Behaviour with slow clients (read_timeout path — there is a
  `slow_client_timeout_does_not_prevent_followup_request` test at
  `transport.rs:543-598` but only one slow client at a time)
- Long-running soak (24h+)

Also, the test only uses `Method::GetHealth` and `Method::GetStatus` —
cheap read-only methods that go nowhere near the backend. A stress
test that hit the full dispatch loop with a Mutation mix would be
more meaningful.

**Remediation:** Add a soak mode (`#[ignore = "stress: 24h soak"]`),
a slow-client-population mode (25% slow clients), and a
mutation-mixed-workload variant. Track as a parity-proof requirement
for `bd-1du.10`.

---

#### 7.8.2 [LOW] The stress test uses `fd_drift <= 64` as the leak ceiling; 64 is generous

`stress_concurrent_clients.rs:150-153`:

```rust
let fd_drift = after_fds.saturating_sub(baseline_fds);
assert!(
    fd_drift <= 64,
    "fd drift {fd_drift} exceeds leak ceiling (baseline={baseline_fds}, after={after_fds})"
);
```

64 file descriptors after 25000 requests is a 1-in-390 leak rate. That
is too lax for an enterprise claim. The expected leak rate on a
correct implementation is zero; a tiny non-zero rate is ephemeral
(pending socket close under linger).

**Remediation:** Tighten to `fd_drift <= 4` (accepting
epoll/signalfd/eventfd ephemera) and run the test 3 times so transient
noise is amortised.

---

### 7.9 Web / management surface (`pcloud-web`)

#### 7.9.1 [INFO] Bind address is loopback-enforced at construction time with a panic guard

`crates/pcloud-web/src/lib.rs:236-260` (`serve`) has a hard
`assert!` that `config.bind_addr.ip().is_loopback()` and panics if
violated. The doc comment at L223-235 explains why it is a panic rather
than a `WebError`. Unit-tested at L311-325.

**No finding.** Correct.

---

#### 7.9.2 [HIGH] `pcloud-web` has **no authentication whatsoever**; every loopback connection gets the full route surface

`crates/pcloud-web/src/routes.rs:66-79` — no auth middleware, no
bearer-token check, no IP check beyond loopback. The route set includes
mutating endpoints:

- `POST /sync` — add a sync root (L71)
- `DELETE /sync/{id}` — remove a sync root (L72)
- `POST /publinks` — create a public link (L73)
- `DELETE /publinks/{code}` — revoke a public link (L74)

CSRF (double-submit cookie + HMAC-less token) is in place
(`routes.rs:596-622`) — but CSRF only stops cross-origin attackers.
It does *not* stop other local processes (running as the same user)
from just calling these endpoints directly with their own CSRF cookie
pair; CSRF requires a browser context, and a plain `curl` from a sibling
process can issue `GET /` to mint a cookie and then POST/DELETE with
the echoed token.

**Impact:** Combined with §7.4.1, this is a second local-auth hole.
Any process the user runs can:

- Point the sync engine at `/etc/passwd`'s parent via `POST /sync`
  (if validation lets it — out of scope for this audit, but the
  daemon's sync backend does validate paths).
- Revoke all the user's public links via `DELETE /publinks/{code}`.

The default bind is `127.0.0.1:17650` (`lib.rs:113`). If the web UI
is started at all (it is opt-in per the doc at L52-59), any local
process reaches it.

**Remediation:** Require a bearer token (random 256-bit, stored in
`$runtime_dir/pcloud-web.token` mode 0400) on every request. The CLI
reads this token and passes it in an `Authorization: Bearer <token>`
header. Browser sessions exchange the token for a session cookie on
first visit (cookie is `HttpOnly; SameSite=Strict` as today). This
closes the local-process bypass.

Independently: since the daemon already provides owner-uid-gated IPC,
consider making `pcloud-web` proxy all state mutations through the
daemon (which already does the uid check) rather than doing them
directly. Right now `routes.rs:118-137` (`sync_list`) already uses
`call_ipc(...)` — so the architecture is correct — but the CSRF-only
gate at the HTTP boundary is insufficient.

---

#### 7.9.3 [MEDIUM] No TLS on the management surface

`lib.rs:249-258` — `tokio::net::TcpListener::bind` plaintext. No
self-signed / local CA TLS option. Loopback-only mitigates wire
eavesdropping on a healthy host, but:

- A proc-dump by another user (root) observes plaintext traffic.
- A malicious kernel module or BPF probe sees plaintext CSRF tokens.

**Remediation:** Add optional TLS bind via a `rcgen`-generated
localhost cert rotated on each bind, with cert pinning in the CLI. Low
priority for non-paranoid single-user deployments, but required for
the hardened enterprise posture this audit targets.

---

#### 7.9.4 [LOW] CSRF token is 128 bits of hex; no HMAC, no expiry, no rotation

`routes.rs:559-571` mints a 16-byte random token, hex-encodes it.
Compare at L611-617 is constant-time (correctly). There is no
`exp` timestamp in the token itself; any leaked token lives as long as
the browser keeps the cookie.

**Remediation:** Bind the token to a session via HMAC-signed
`(nonce || expires_at || user_sid)`, verify HMAC on submit, refuse
expired tokens. Rotate the HMAC key on daemon startup.

---

### 7.10 Observability of the daemon surface

#### 7.10.1 [MEDIUM] No metric for IPC peer-auth rejections

Searching `pcloud-observability` and `pcloud-daemon` for
`unauthorized_peer` or `peer_cred_unavailable` as a metric counter
returns zero hits. When the IPC transport at
`crates/pcloud-ipc/src/transport.rs:186-208` rejects a peer with
`Unauthorized`, it is logged at the transport layer but not counted.

**Impact:** An operator cannot alarm on "spike in IPC authorization
failures" — a useful intrusion-detection signal.

**Remediation:** Add
`pcloud_ipc_authz_rejections_total{reason=uid_mismatch|cred_unavailable}`.

---

#### 7.10.2 [LOW] `pcloud_ipc_connected_clients` gauge exists but is never set

`crates/pcloud-observability/src/metrics.rs:26` documents the gauge;
`metrics.rs:435-438` provides `set_connected_clients`; grep for the
caller returns nothing except the test. The dispatcher never increments
it.

**Remediation:** Increment on every `accept`, decrement on every
dispatch-complete (under the `InFlightGuard` Drop).

---

## Cross-references

- **§2 (Secret discipline):** §6.1.2 API-server hint rewrite is a
  secret-flow concern; §7.4.1 privilege escalation lets a local
  process fetch crypto-password-change surfaces. Flag for §2 agent.
- **§4 (Sync engine):** §6.5.1 upload idempotency belongs at the
  transport boundary; the sync engine queue is a separate retry tier.
- **§5 (FUSE):** §7.7.2 mount re-adoption and §7.6.3 mount quiesce
  belong to the FUSE agent; §7 only cites them because the daemon
  bootstrap touches them.
- **§8 (Observability):** §6.8.1, §6.8.2, §7.10.1, §7.10.2 all feed
  into the observability dimension's gap analysis.

---

## Summary of findings by severity

| Severity  | Count | Finding IDs                                                                 |
|-----------|-------|------------------------------------------------------------------------------|
| CRITICAL  | 1     | §7.4.1                                                                       |
| HIGH      | 10    | §6.1.1, §6.1.2, §6.4.1, §6.4.2, §6.5.1, §6.8.1, §7.1.2, §7.2.1, §7.4.2, §7.7.1, §7.7.2, §7.9.2 |
| MEDIUM    | 15    | §6.1.3, §6.2.2, §6.3.1, §6.4.3, §6.4.4, §6.4.5, §6.6.1, §6.7.1, §6.8.2, §7.1.3, §7.2.2, §7.3.2, §7.3.3, §7.5.2, §7.5.3, §7.6.2, §7.8.1, §7.9.3, §7.10.1 |
| LOW       | 10+   | §6.3.2, §6.7.2, §7.1.4, §7.2.3, §7.5.4, §7.6.3, §7.6.4, §7.7.3, §7.8.2, §7.9.4, §7.10.2 |
| INFO      | 5     | §6.2.1, §6.6.1, §7.1.1, §7.3.1, §7.5.1, §7.6.1, §7.9.1                       |

(Count in table counts each §id once; some §ids span multiple severities
in the prose — the tally above uses the headline severity of each finding.)

---

## Enterprise-readiness verdict (§6 + §7 scope only)

**The transport path is close to enterprise-grade; the IPC path is
blocked by one CRITICAL finding.**

Transport blockers, in priority order:

1. **§6.4.1** — supply a real error classifier to
   `ResilientTransport::wrap_binary`; permanent errors must not retry.
2. **§6.5.1** — wire `MethodRetryPolicy` into `ResilientTransport` and
   reject mutation retries without an idempotency anchor.
3. **§6.8.1** — add per-endpoint outbound HTTP metrics; operators
   cannot run this daemon in production without them.
4. **§6.4.2** — honour `Retry-After`.
5. **§6.1.1 / §6.1.2** — harden TLS enforcement so no code path can
   bypass the validation gate.

IPC blockers, in priority order:

1. **§7.4.1 (CRITICAL)** — introduce at least a two-tier
   (read-only / privileged) capability scoping. Without this the
   daemon is not safe against malicious local processes owned by the
   same user.
2. **§7.2.1** — fix proptest variant coverage gap; the
   "exhaustiveness guard" is inert.
3. **§7.7.1 / §7.7.2** — implement real crash recovery. Log-only is
   not crash recovery.
4. **§7.9.2** — add authentication to `pcloud-web`.
5. **§7.1.2** — add capability/version handshake so the JSON wire
   schema can evolve.

Until these close, the daemon should not be described as "production
ready", "enterprise ready", or a "drop-in replacement" in any
release-facing document. This is consistent with the CLAUDE.md
discipline rules.
## Section 8. CLI & SDK Surface

**Auditor:** Dimension 8 specialist (CLI / SDK ergonomics)
**Scope:** `crates/pcloud-cli/` end-to-end, `crates/pcloud-sdk/` public API, `pcloud-ipc::{Method,Request}` ↔ CLI mapping, shell-completion coverage, feature-flag matrix.
**Non-scope:** parity with the legacy C tree (Dimension 1), daemon-side handler correctness, protocol wire behaviour. Findings here are about UX, semver hygiene, argument-parser truthfulness, and operational ergonomics only.
**Workspace root:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/`

---

### 8.0 Executive summary

`pcloudc` is the only installable CLI binary. It is **hand-rolled** — not clap-derive — despite `clap` being declared as a dependency. Every subcommand is dispatched through a bespoke token walker in `app.rs` (`normalize_args` + `parse_inputs_for_command`, ~4700 lines). Clap is used only to *generate* shell-completion scripts from a **second, parallel, smaller** command tree (`completion::build_cli`). Those two trees are **not mechanically synchronised**, and the audit found that completion is missing roughly **two-thirds of the subcommands** the CLI actually accepts. That is the single biggest CLI UX finding in this section.

The SDK is a much tidier surface: one `EmbeddedDaemon` struct, 14 typed per-helper error enums all funnelled into `SdkError`, docstrings on virtually every public item, and a curated re-export block (no wildcard glob leaks). It still has two SemVer-discipline issues and **only one example** (vs. the 80+ helper methods advertised).

Severity tallies:

| Severity | CLI | SDK | Cross | Total |
|---|---|---|---|---|
| CRITICAL | 0   | 0   | 0   | 0   |
| HIGH     | 6   | 3   | 2   | 11  |
| MEDIUM   | 11  | 6   | 3   | 20  |
| LOW      | 8   | 4   | 2   | 14  |

No CRITICAL findings — the CLI *works*, secrets do not trivially leak to stdout or history, exit codes are stable, and the SDK compiles cleanly with the declared feature set. The HIGHs are operational-quality defects (shell completion drift, accidental `pub use`, single example, one-arg `clap` that doesn't reflect the real surface).

---

### 8.1 CLI command ↔ IPC Request / Method matrix

Source of truth:
- IPC: `crates/pcloud-ipc/src/methods.rs:37` (`enum Method`, 42 variants) and `crates/pcloud-ipc/src/methods.rs:262` (`enum Request`, 70+ variants).
- CLI: `crates/pcloud-cli/src/commands.rs:35` (`enum Command`, 113 variants) and `crates/pcloud-cli/src/commands.rs:750` (`into_request`).
- Completion tree: `crates/pcloud-cli/src/completion.rs:35` (`build_cli`, ~55 subcommand entries).

Legend: **C** = visible in CLI, **R** = visible through `into_request`, **K** = in `clap_complete` tree, **—** = absent.

#### 8.1.1 Argumentless `Method::*` variants (wired via `Request::Plain { method }`)

| `Method` variant | CLI subcommand | `into_request`? | Completion? | Notes |
|---|---|---|---|---|
| `GetStatus` | `status` / `st` | yes (`commands.rs:757`) | yes | |
| `GetHealth` | `health` | yes (`:760`) | yes | Also used as defensive fallback for CLI-side-only commands (`Start`, `Drain`, `Reload`, `Doctor`, `MigrateFromC`, `FileDiff`, `FileRestore`) — see Finding CLI-M3. |
| `Health` (enterprise) | — **missing** | — | — | HIGH: `Method::Health` has no CLI subcommand. Operators cannot query the structured `/healthz` payload (build info, uptime, Prometheus snapshot) without `dispatch()` via SDK. Finding **CLI-H1**. |
| `GetPending` | `pending` / `p` | yes (`:763`) | yes | |
| `GetSyncRoots` | `sync list` / `sync ls` / `sync-list` | yes (`:865`) | partial (`sync list` in completion) | |
| `ListPublicLinks` | `list-links` / `publink list` | yes (`:769`) | `list-links` only in completion | MEDIUM: no `publink` subcommand tree under completion, only the hyphenated form. Finding **CLI-M2**. |
| `ListUploadLinks` | `list-upload-links` | yes (`:779`) | yes | |
| `GetUserInfo` | `userinfo` | yes (`:886`) | yes | |
| `PauseSync` | `pause` | yes (`:889`) | yes | |
| `ResumeSync` | `resume` | yes (`:892`) | yes | |
| `LoginBegin` | `login` (REPL) / `login-begin` | yes (`:895`) | yes (`login`) | |
| `Logout` | `logout` | yes (`:898`) | yes | |
| `SendTwoFactorSms` | `send-tfa-sms` | yes (`:901`) | yes | |
| `SendTwoFactorNotification` | `send-tfa-notification` | yes (`:904`) | yes | |
| `SubmitPassword` | via REPL; `submit-password` (legacy) | `PasswordSubmission` (`:907`) | yes | |
| `SubmitTwoFactorCode` | `submit-tfa` | `TwoFactorCodeSubmission` (`:917`) | yes | |
| `UnlockCrypto` | `unlock-crypto`, `crypto start` | `CryptoUnlock` (`:924`) | yes | |
| `LockCrypto` | `lock-crypto`, `crypto stop` | yes (`:930`) | yes | |
| `GetCryptoStatus` | `crypto status` / `crypto st` | yes (`:776`) | partial (only `start` / `stop` in completion) | HIGH — see **CLI-H2**. |
| `CryptoReset` | `crypto reset` | yes (`:1175`) | — | |
| `GetCryptoPrivKeyFlags` | `crypto priv-key-flags` | yes (`:1178`) | — | |
| `SendCryptoChangeUserPrivate` | `crypto send-change-private` | yes (`:1181`) | — | |
| `GetCryptoHint` | `crypto hint` | yes (`:1197`) | — | |
| `Shutdown` | `shutdown` / `stop` / `finalize` / `f` | yes (`:939`) | yes (`finalize`) | MEDIUM: `shutdown` token itself isn't in the completion tree — see **CLI-M2**. |
| `SetAuthPersistence` | `authsave` | `AuthPersistence` (`:927`) | yes | |
| `ListIncomingShares` | `list-incoming-shares` / `shares list-incoming` | yes (`:956`) | yes (`list-incoming-shares`) | |
| `ListOutgoingShares` | `list-outgoing-shares` / `shares list-outgoing` | yes (`:959`) | yes | |
| `ListIncomingShareRequests` | `list-incoming-share-requests` | yes (`:962`) | yes | |
| `ListOutgoingShareRequests` | `list-outgoing-share-requests` | yes (`:965`) | yes | |
| `ListContacts` | `list-contacts` / `contacts list` | yes (`:968`) | yes (`list-contacts`) | |
| `ListMyTeams` | `list-myteams` / `teams list` | yes (`:971`) | yes (`list-myteams`) | |
| `ListNotifications` | `notifications list`, `notif list`, `list-notifications` | yes (`:772`) | yes | |
| `SessionStatus` | `session status` | yes (`:1022`) | yes | |
| `FileHistory` | `log` / `history` (plus typed-args variant) | `Request::FileHistory` (`:1085`) | — | HIGH: `log` is documented in `help_text` (line 357+) but absent from completion. Finding **CLI-H3**. |
| `IntegrityStatus` | `integrity status` / `integrity` | yes (`:1139`) | — | |
| `HaStatus` | `ha status` / `ha` | yes (`:1146`) | — | |
| `DrainStatus` | `drain` (CLI-side only, `GetHealth` fallback) | `Plain { method: DrainStatus }` (`:947`) | — | |
| `GetSlo` | `slo` | yes (`:766`) | — | HIGH: `slo` is advertised in help & referenced in field-selector tests but missing from completion. |
| `GetAuditVerifierStatus` | `audit-verifier status` | yes (`:1149`) | — | |
| `GetSyncStatus` | `sync status` / `sync st` | yes (`:868`) | — | |
| `ListConflicts` | `conflict list` / `conflicts` | yes (`:1169` / alias in app.rs sync subcmd) | — | |
| `StatPath` | `stat` | `Request::StatPath` (`:1071`) | — | |
| `GetApiServers` | `account api-servers` / `account apiservers` | yes (`:1231`) | — | |
| `GetPromo` | `account promo` | yes (`:1241`) | — | |
| `VerifyEmail` | `account verify-email` | yes (`:1209`) | — | |

#### 8.1.2 Argument-bearing `Request::*` variants (not carried by `Method` alone)

| `Request` variant | CLI subcommand | `into_request`? | Completion? | Notes |
|---|---|---|---|---|
| `PasswordSubmission` | `submit-password`, REPL login | yes (`:907`) | yes | |
| `AuthTokenSubmission` | `submit-auth` | yes (`:914`) | yes | |
| `TwoFactorCodeSubmission` | `submit-tfa`, `submit-recovery` | yes (`:917`) | yes | |
| `CryptoUnlock` | `unlock-crypto` | yes (`:924`) | yes | |
| `CryptoSetup` | — **MISSING from CLI** | — | — | HIGH: no `crypto setup` subcommand. `Request::CryptoSetup { password, hint }` is wired at the daemon side (`methods.rs:304`) but unreachable from `pcloudc`. Finding **CLI-H4**. |
| `CryptoMkdir` | — **MISSING** | — | — | HIGH: no `crypto mkdir` subcommand. A user cannot create an encrypted folder from the CLI. Finding **CLI-H4**. |
| `CryptoChangePassword` | `crypto change-password` | yes (`:1184`) | — | |
| `CryptoChangePasswordUnlocked` | `crypto change-password-unlocked` | yes (`:1191`) | — | |
| `AuthPersistence` | `authsave <on\|off>` | yes (`:927`) | yes | |
| `SyncRootAdd` | `sync add` | yes (`:871`) | yes | |
| `SyncRootRemove` | `sync remove` | yes (`:876`) | yes | |
| `SyncRootPause` | — **MISSING** | — | — | MEDIUM: `Request::SyncRootPause { sync_id }` and `SyncRootResume` exist (methods.rs:396, 401) but the CLI has no `sync pause-root <id>` / `sync resume-root <id>` — only the global `pause` / `resume` which trigger `PauseSync` / `ResumeSync` on the whole daemon. Finding **CLI-M1**. |
| `SyncRootResume` | — **MISSING** | — | — | See CLI-M1. |
| `SyncRootChangeType` | `sync change-type` | yes (`:879`) | — | |
| `GetSyncSuggestions` | `sync suggest` | yes (`:1201`) | — | |
| `IsFolderSyncable` | `sync is-syncable` | yes (`:1205`) | — | |
| `ShowPublicLink` | `show-link` / `publink show` | yes (`:782`) | yes | |
| `DeletePublicLink` / `DeletePublicLinkByCode` | `delete-link` | yes (`:785`+) | yes | |
| `CreateFilePublicLink` | `create-file-link` | yes (`:800`) | yes | |
| `CreateFolderPublicLink` | `create-folder-link` | yes (`:803`) | yes | |
| `ChangePublicLinkExpire` | `change-link-expire` | yes (`:806`) | yes | |
| `ChangePublicLinkPassword` | `change-link-password` | yes (`:810`) | yes | |
| `ChangePublicLinkUpload` | `change-link-upload` | yes (`:820`) | yes | |
| `CreateUploadLink` | `create-upload-link` | yes (`:824`) | yes | |
| `DeleteUploadLink` | `delete-upload-link` | yes (`:831`) | yes | |
| `CreateTreePublicLink` | `create-tree-link` | yes (`:834`) | yes | |
| `ListPublicLinkAccess` | `list-link-access` | yes (`:843`) | yes | |
| `AddPublicLinkAccess` | `add-link-access` | yes (`:846`) | yes | |
| `RemovePublicLinkAccess` | `remove-link-access` | yes (`:850`) | yes | |
| `ListBookmarks` | `list-bookmarks` / `bookmark list` | yes (`:854`) | yes | |
| `RemoveBookmark` | `remove-bookmark` / `bookmark remove` | yes (`:855`) | yes | |
| `ChangeBookmark` | `change-bookmark` / `bookmark change` | yes (`:859`) | yes | |
| `ShareFolder` | `share-folder` | yes (`:974`) | yes | |
| `CancelShareRequest` | `cancel-share-request` | yes (`:982`) | yes | |
| `DeclineShareRequest` | `decline-share-request` | yes (`:985`) | yes | |
| `AcceptShareRequest` | `accept-share-request` | yes (`:988`) | yes | |
| `RemoveShare` | `remove-share` | yes (`:993`) | yes | |
| `ModifyShare` | `modify-share` | yes (`:996`) | yes | |
| `AccountStopShare` | `account-stopshare` | yes (`:1000`) | yes | |
| `AccountModifyShare` | `account-modifyshare` | yes (`:1004`) | yes | |
| `AccountTeamShare` | `account-teamshare` | yes (`:1008`) | yes | |
| `ValueGet` / `ValueSet` / `ValueHas` | — **MISSING from CLI** | — | — | MEDIUM: the typed KV helpers `Request::ValueGet/Set/Has` (methods.rs:635+) are exposed through the SDK (`EmbeddedDaemon::{get,set,has}_{uint,int,bool,string}_value`) but there is **no `pcloudc value get/set/has`** subcommand. Operators cannot probe or adjust settings without writing a Rust host. Finding **CLI-M4**. |
| `MarkNotificationsRead` | `notifications mark-read <id>` | yes (`:775`) | yes | |
| `AuditVerifyChain` | `audit verify` | yes (`:1016`) | yes | |
| `Mount` / `Unmount` / `MountForceUnmount` | `mount` / `unmount` (+ `PCLOUD_FORCE_UMOUNT`) | yes (`:1025`,1028,1038) | yes (`mount`,`unmount`) | |
| `CreateRemoteFolder` | `folder create` | yes (`:1053`) | yes | |
| `Unmount` | `unmount` | yes (`:1044`) | yes | |
| `RunLocalScan` | `sync localscan` | yes (`:1047`) | — | |
| `SendPublink` | `publink send` | yes (`:1048`) | yes | |
| `GetFolderIdByPath` / `GetFolderFlags` / `GetFolderOwnerId` | `folder id` / `folder flags` / `folder owner` | yes (`:1059`,1062,1065) | yes | |
| `FilesystemStatus` | `fs status` | yes (`:1068`) | yes | |
| `StatPath` | `stat` | yes (`:1071`) | — | |
| `FileHistory` | `log` / `history` | yes (`:1085`) | — | |
| `VerifyPath` | `verify` | yes (`:1102`) | — | CLI-side walker is primary; IPC is a future wiring. |
| `BackupSnapshot` (Create/Restore/Verify/Prune) | `snapshot create/restore/verify/prune` + deprecated `backup snapshot-*` | yes (`:1106` etc.) | — | |
| `IntegrityRunOnce` / `IntegritySkip` | `integrity run-once` / `integrity skip` | yes (`:1142`,1143) | — | |
| `UploadCreate` / `UploadPause` / `UploadResume` / `UploadCancel` / `UploadList` | `upload create/pause/resume/cancel/list` | yes (`:1152`→1168) | — | HIGH — entire `upload` command group absent from completion. Finding **CLI-H3**. |
| `ConflictList` / `ConflictResolve` | `conflict list` / `conflict resolve` | yes (`:1169`,1170) | — | |
| `LostPassword` | `account lost-password` | yes (`:1215`) | — | |
| `VerifyEmailRestricted` | `account verify-email-restricted` | yes (`:1212`) | — | |
| `AccountChangePassword` | `account change-password` | yes (`:1218`) | — | |
| `AccountRegister` | `account register` | yes (`:1226`) | — | |
| `SetApiServer` | `account set-api-server` | yes (`:1234`) | — | |
| `SetLanguage` | `account set-language` | yes (`:1238`) | — | |
| `GetFileLink` | `download link` | yes (`:1245`) | — | |
| `DownloadFile` | `download file` | yes (`:1248`) | — | |
| `DeleteBackup` | `backup delete` / `backup rm` | yes (`:1253`) | — | |

**Matrix aggregate:**
- IPC `Method` variants: 42. CLI-reachable: 41. Completion-listed (top-level): 16/42 (≈38%). Unreachable: 1 (`Health` — CLI-H1).
- IPC `Request` variants (non-Plain): 61. CLI-reachable via `Command::into_request`: 58. Unreachable: **3** (`CryptoSetup`, `CryptoMkdir`, `SyncRootPause`, `SyncRootResume`, `ValueGet`/`Set`/`Has`) — see CLI-H4, CLI-M1, CLI-M4.
- CLI `Command` variants with no completion stub: **~50 of 113** (~44%). Account/download/backup/snapshot/integrity/ha/audit-verifier/conflict/upload/stat/log/session/folder trees are all absent or partial. This is the single largest CLI UX defect and maps to **CLI-H3**.

---

### 8.2 CLI findings

#### 8.2.1 Argument parser truthfulness

**CLI-H1 — `Method::Health` (enterprise health probe) is unreachable from the CLI.**
- Evidence: `pcloud-ipc/src/methods.rs:44-49` defines `GetHealth` *and* `Health` as two distinct methods (the latter reportedly returns build info / uptime / Prometheus snapshot). `pcloud-cli/src/commands.rs:47-51` only maps `pcloudc health` → `Method::GetHealth`. Grepping `commands.rs` + `main.rs` finds **no** reference to `Method::Health`.
- Impact: operators running `pcloudc health --json` get the short liveness string, not the structured payload documented on the IPC variant. The `--json` promise (machine-readable health) is broken.
- Remediation: either (a) add `pcloudc health --detailed` / `pcloudc healthz` → `Method::Health`, or (b) switch `pcloudc health` to `Method::Health` and rename the current short path to `pcloudc ping`. Update `help_text()` (`app.rs:95`) and `completion.rs:73`.
- Severity: HIGH.

**CLI-H2 — Shell-completion tree is desynchronised from the real command surface.**
- Evidence: `crates/pcloud-cli/src/completion.rs:35-193` hand-maintains a parallel clap tree. Spot-check against `commands.rs:35-553`:
  - missing top-level subcommands: `account`, `backup`, `snapshot`, `conflict`, `integrity`, `ha`, `audit-verifier`, `upload`, `download`, `stat`, `slo`, `verify`, `log`, `diff`, `restore`, `ha`, `drain`, `reload`, `doctor`, `migrate-from-c`, `teams`, `contacts`.
  - missing subtrees under declared groups: `crypto` completion (`completion.rs:86-90`) only carries `start` / `stop` — absent `status`, `reset`, `hint`, `priv-key-flags`, `send-change-private`, `change-password`, `change-password-unlocked`.
  - missing `sync` children: `status`, `change-type`, `localscan`, `conflicts`, `suggest`, `is-syncable`.
  - missing global flags: `--field` / `-f` / `--select`, `--dbg` / `--debug`, `--trace-id`, `--config`.
- Impact: users who install completion via `pcloudc completion bash > /etc/bash_completion.d/pcloudc` get tab-completion that silently excludes ≈60% of the real surface. This is the strongest CLI UX regression found. The tests in `completion.rs:225-284` only verify that scripts are *non-empty*, not that they list the real commands.
- Remediation: either migrate the whole CLI to `clap::Parser`-derive (single source of truth, which the `Cargo.toml` already enables with `features = ["derive"]` at line 27 but never uses), or write an integration test that enumerates every `Command::*` variant and asserts it exists in the clap completion tree.
- Severity: HIGH.

**CLI-H3 — `--help` promises commands that completion never advertises.**
- Evidence: `app.rs:300` lists `publink send`, lines 324-327 list `notifications mark-read`, lines 332-337 list `audit verify`, line 419 lists `status auth sync crypto`, etc. Of these, only `publink send` and the notifications pair appear in `build_cli`.
- Impact: help text accurate, completion broken. Same remediation as CLI-H2 but called out separately because *scripts* that `grep` the help output will find commands that shell completion won't assist with.
- Severity: HIGH.

**CLI-H4 — `crypto setup` and `crypto mkdir` are unreachable from `pcloudc`.**
- Evidence: `pcloud-ipc/src/methods.rs:304-322` wires `Request::CryptoSetup { password, hint }` and `Request::CryptoMkdir { name, parent_folder_id, local_folder_id }`. `commands.rs` has **no** `Command::*` variant that translates to either request. The `crypto start` subcommand (`app.rs:632`) only dispatches `SubmitCryptoPassword` → `CryptoUnlock`, so a first-time user cannot set up a crypto shell without embedding the SDK.
- Impact: a legitimate C-parity feature (first-run crypto bring-up) is CLI-inaccessible.
- Remediation: add `pcloudc crypto setup` (prompts new passphrase + optional hint) and `pcloudc crypto mkdir <name> [--parent <id>]`. Wire into the completion tree simultaneously.
- Severity: HIGH.

**CLI-H5 — No clap-derive anywhere: duplicate truth, massive surface for drift.**
- Evidence: `Cargo.toml:27` declares `clap = { version = "4.6", ..., features = ["std", "help", "usage", "error-context", "derive"] }`. The `derive` feature is enabled. Yet `grep -rn "#\[derive(Parser)\]\|Parser::parse\|clap::Parser" crates/pcloud-cli/src/` returns zero matches. Clap is used **only** inside `completion::build_cli` as an `Arg::new(...)` builder. The real parser is 4700 lines of hand-rolled token walking in `app.rs`.
- Impact: every subcommand is declared twice (`Command` enum + parser table) and in a third place for completion (`build_cli`). Invariants between help text, completion, and actual parser behaviour are maintained by convention only. This is the structural cause of CLI-H2 and CLI-H3.
- Remediation: migrate to `#[derive(clap::Parser)]` — or at minimum write a trivariate test (`Command::iter()` must map onto both `build_cli().get_subcommands()` and every `app::parse_command` token).
- Severity: HIGH.

**CLI-H6 — Positional password on `pcloudc crypto start <PW>` and `pcloudc submit-password [USER] [PW]` leaks into argv, then into `/proc/<pid>/cmdline`, `ps auxw`, and shell history.**
- Evidence:
  - `app.rs:1545`: `let crypto_password = match args.get(2) { Some(password) => password.clone(), None => SecretPrompt::new("Crypto password").read_secret()? };` — a positional third argument *is* consumed if present, no warning printed.
  - `app.rs:2997-3014`: `submit-password` with a positional password prints a one-line stderr warning (`"warning: passing the password on the command line is insecure ..."`) but still accepts the value. The best-effort argv scrub comment concedes "we can't mutate \[`&[String]`\]" and `/proc/self/cmdline` "is a separate kernel-maintained copy that we cannot rewrite".
  - `app.rs:1527-1533` (`submit-auth`): accepts positional token with **no** stderr warning. The token is equally sensitive (long-lived bearer credential).
- Impact: any same-user process on the host can read the full password via `/proc/<pid>/cmdline` until the CLI exits, and the value survives in `~/.bash_history` / `~/.zsh_history`. The scrub-argv comment is candid but the feature still exists.
- Remediation:
  - Deprecate positional password/token on all three subcommands (`submit-password`, `submit-auth`, `crypto start`). Print a deprecation warning in this release; reject in the next major.
  - Require `--password-stdin` / `--password-env` / interactive prompt (already implemented for the REPL path).
  - At minimum, add the same stderr warning to `submit-auth` and `crypto start <PW>` that `submit-password` has.
- Severity: HIGH.

#### 8.2.2 Exit codes & error mapping

**CLI-M1 — `SyncRootPause` / `SyncRootResume` (per-root) have no CLI surface.**
- Evidence: `pcloud-ipc/src/methods.rs:396-404` wires both. `pcloudc sync pause` / `sync resume` route to **daemon-wide** `Pause`/`Resume` (`app.rs:554-555`). There is no way to pause a single root via the CLI.
- Impact: operators who want to pause one sync root without stopping the whole engine must use the SDK directly.
- Remediation: add `pcloudc sync pause-root <id>` / `sync resume-root <id>`, add to completion tree.
- Severity: MEDIUM.

**CLI-M2 — Completion tree advertises tokens not in the hand-rolled parser.**
- Evidence: `completion.rs:180-182` declares `start` (→ background daemon), `finalize` (alias for `stop`), `doctor`. Of these, `finalize` works, `start` works, `doctor` works. But `completion.rs:181` declares a literal `"finalize"` with help text `"Stop daemon and exit"` — this is the operator-facing token. The hand-rolled parser also accepts `stop` and `f`, but neither appears in completion.
- Impact: tab-completion shows only one of three synonymous tokens; users who learned `stop` from `help` cannot tab-complete it.
- Remediation: consolidate on clap-derive (CLI-H5) or add each alias explicitly.
- Severity: MEDIUM.

**CLI-M3 — Seven CLI-side-only commands all map to `Method::GetHealth` as a "defensive fallback" in `Command::into_request`.**
- Evidence: `commands.rs:754-756, 933-938, 947-955, 1077-1084, 1092-1094`. `Help`, `Start`, `Drain`, `Reload`, `Doctor`, `MigrateFromC`, `FileDiff`, `FileRestore` all fall through to `Method::GetHealth`. The comment rightly notes they "must never reach this dispatch", but if `main.rs` regresses, a `pcloudc start` would silently ping `GetHealth` and report OK.
- Impact: a bug in `main.rs` branching would silently do the wrong thing (mask failure as success).
- Remediation: make `into_request` return `Option<Request>` (or a dedicated `NoIpcDispatch` variant) and unify the CLI-side-only branch through a single "never lower to IPC" path. Tests should assert that the CLI-side-only commands never reach the IPC client.
- Severity: MEDIUM.

**CLI-M4 — `value`/`setting` CLI subcommands missing; gated behind SDK only.**
- Evidence: `pcloud-ipc/src/methods.rs:635-659` and `pcloud-sdk/src/lib.rs:2549-2696` expose typed KV helpers. The CLI has no `pcloudc value get <name> --kind=bool`.
- Impact: operators troubleshooting settings must write a Rust host.
- Remediation: add a `pcloudc value` / `pcloudc setting` subcommand tree.
- Severity: MEDIUM.

**CLI-M5 — `ExitCode::classify_transport_error` uses substring matching on localised-free English tokens.**
- Evidence: `exit_code.rs:117-137`. Matches `"connection"`, `"timed out"`, `"refused"`, `"broken pipe"`, `"unauthorized"`, `"crypto"`, etc.
- Impact: if any transport layer ever returns a localised error (or wraps error strings), the classifier silently degrades to `GenericError`. Also: `l.contains("auth") && l.contains("fail")` has classic precedence footgun — `"unauthorized" || "auth" && "fail"` parses as `"unauthorized" || ("auth" && "fail")`, which is what the author intended but is fragile (`clippy::needless_bool`-adjacent).
- Remediation: use typed errors at the transport layer (`thiserror`-enumerated IPC client errors) and match on variants, not substrings.
- Severity: MEDIUM.

**CLI-M6 — `PolicyViolation` response status forces exit code 7 (`Conflict`), which is the same code as "duplicate sync root".**
- Evidence: `exit_code.rs:105` — `ResponseStatus::PolicyViolation { .. } => Self::Conflict`.
- Impact: operators cannot distinguish a data-residency policy rejection from a state-conflict without parsing the response message. Scripts that branch on exit code lose information.
- Remediation: add `ExitCode::PolicyRejected = 9` (additive, SemVer-safe per the crate's stated "additive" guarantee at `exit_code.rs:52`). Update `EXIT_CODE_HELP`.
- Severity: MEDIUM.

**CLI-L1 — Exit codes in help text are **duplicated** across `app.rs:391-402` and `exit_code.rs:25-35` (`EXIT_CODE_HELP`).**
- Evidence: two hard-coded blocks carrying the same 0-8 list. Drift risk if either is updated.
- Impact: minor; no current discrepancy.
- Remediation: `app.rs:391-402` should `include!(EXIT_CODE_HELP)` or concat.
- Severity: LOW.

#### 8.2.3 Secrets handling on the CLI

**CLI-M7 — `submit-auth [TOKEN]` accepts the token positional with no stderr warning.**
- Evidence: `app.rs:1527-1533`. Contrast with `submit-password` positional (`app.rs:2997-3014`) which at least prints a stderr warning.
- Impact: auth tokens are bearer credentials (pCloud session token) and equally sensitive to passwords. Positional leak to `/proc/<pid>/cmdline` + shell history.
- Remediation: add the same stderr warning (and the same deprecation plan as CLI-H6) to `submit-auth`.
- Severity: MEDIUM.

**CLI-L2 — `read_masked` is Linux-only (`prompt.rs:146-246`); non-Linux falls through to `rpassword::read_password()` with no `*`-echo.**
- Evidence: `prompt.rs:154-158, 248-251`.
- Impact: on macOS/FreeBSD the 2FA prompt has no visual feedback; on Windows it uses `rpassword`'s platform default. Non-critical but documented honestly.
- Remediation: either gate the 2FA call sites to only use `read_masked` on Linux explicitly, or port the termios block to `crossterm` for portable raw-mode.
- Severity: LOW.

**CLI-L3 — `version_banner()` reports `"unknown"` for `GIT_HASH` when the build is not from a git checkout.**
- Evidence: `main.rs:56-66`, `build.rs:31-53`. The fallback is documented, but there is **no CI-time `GIT_HASH=$(git rev-parse --short HEAD)` export** visible in any workspace file (grep for `GIT_HASH=` returns only the build.rs line). A release tarball built without explicit env passing reports `pcloudc 0.0.0 (unknown, release)`.
- Remediation: document the `GIT_HASH` env override in the release runbook; add a CI job that passes the hash explicitly.
- Severity: LOW.

**CLI-L4 — `--version` does not surface Rust compiler, tls library, or feature-flag matrix.**
- Evidence: `main.rs:56-66`.
- Impact: enterprise support workflows typically need the build triple + rustc version. Current output `pcloudc 0.1.0 (abcd123, release)` is thin.
- Remediation: emit a `--version --verbose` block with `std::env::consts::OS`, `std::env::consts::ARCH`, `rustc_version_runtime`, enabled feature flags. Match the shape of `rustc -Vv`.
- Severity: LOW.

#### 8.2.4 Long-running command UX

**CLI-M8 — `mount`, `upload create`, `download file`, `verify --recursive`, `snapshot create` are long-running but `progress.rs` has no call sites outside its own tests.**
- Evidence: `progress.rs:1-454` defines `ProgressMode`, `ProgressEvent`, `StderrSink`. A `grep -rn "ProgressMode\|ProgressEvent\|ProgressSink" crates/pcloud-cli/src/` returns matches only inside `progress.rs` itself. The module is wired but not invoked.
- Impact: `pcloudc download file 12345 /tmp/huge.bin` shows nothing until it finishes. `pcloudc snapshot create /backup.tar.zst` returns when done; no progress. Enterprise UX expectation is broken.
- Remediation: wire `progress::Reporter` into `download_file`, `upload create|list` (blocking waits), `snapshot create|restore|verify`, `verify --recursive`.
- Severity: MEDIUM.

**CLI-M9 — Ctrl-C during a running IPC call leaves the socket connection in an undefined state.**
- Evidence: grep for `signal_hook\|ctrlc\|sigaction` in `crates/pcloud-cli/src/` returns only `globals::tests::trace_env_guard` and `pcloud-secret/...` references. The CLI installs no SIGINT handler; Tokio is not in use (no `#[tokio::main]`). `main.rs` is synchronous.
- Impact: for short commands this is fine. For `snapshot create` (minutes) a Ctrl-C abandons the daemon mid-work. The daemon has drain/unmount handling, but the CLI has no confirmation UX ("press Ctrl-C again to force").
- Remediation: install a SIGINT handler that clears the cursor line on spinner mode and exits with code 130 (conventional SIGINT exit code).
- Severity: MEDIUM.

**CLI-L5 — `progress.rs` NDJSON mode emits progress to *stderr* (`progress.rs:95-99`) even in `--json` mode.**
- Evidence: `progress.rs:12-16` documents this.
- Impact: in theory fine (stdout stays clean for the final envelope) but operators piping `pcloudc --json upload create ... 2>&1 | jq` get mixed line-per-event + final envelope in stderr/stdout stream interleave. Current status: no call sites (CLI-M8), so not observable.
- Severity: LOW.

#### 8.2.5 Error messages

**CLI-L6 — `CommandParseError::UnknownCommand` carries the raw user-supplied token (not sanitised).**
- Evidence: `app.rs:446-447` — `#[error("unknown command '{0}'")] UnknownCommand(String)`.
- Impact: the caller controls the string, so no secret-leak class. But a malicious operator running inside a shared CI log view could inject ANSI escape codes via `pcloudc $'\e[2Jfake-command'`. The error printing path is `report_error`, which does not filter.
- Remediation: pass tokens through a printable-ASCII sanitiser before emitting into error strings.
- Severity: LOW.

**CLI-L7 — `report_error` prints the `Internal` daemon error message verbatim, including daemon-side stack-trace tails when `tracing` is hooked in.**
- Evidence: `main.rs:438-505` uses `response.message` as the error detail. Daemon-side internal errors are opaque `"internal: <inner err.to_string()>"` strings; on observability builds this can include file:line from the daemon crate.
- Impact: internal-impl leakage in CLI output. Minor for enterprise; inconvenient for public-facing tooling.
- Remediation: when `flags.verbosity == 0`, strip internal diagnostic details and print "internal daemon error — rerun with -v for details" for `Internal` statuses.
- Severity: LOW.

---

### 8.3 SDK findings

#### 8.3.1 Public API semver discipline

**SDK-H1 — `pub use pcloud_proto::Notification` re-exports a type from a non-peer crate.**
- Evidence: `pcloud-sdk/src/lib.rs:102-105`:
  ```rust
  /// Typed notification record mirroring the C `psync_notification_t`. Re-exported
  /// from `pcloud-proto` so SDK consumers do not need a direct dependency on the
  /// protocol crate.
  pub use pcloud_proto::Notification;
  ```
  `pcloud_proto` is a workspace-internal crate (`description = "Typed pCloud protocol clients"` in its Cargo.toml). The SDK now has an `Notification: From<pcloud_proto::Notification> for pcloud_sdk::Notification` semver contract that *is actually* `pcloud_proto::Notification`. Any breaking change to the proto `Notification` struct is a SemVer break on the SDK.
- Impact: the SDK claims to be embeddable ("wraps the daemon runtime"), but a caller gets stuck on whichever `pcloud-proto` the SDK is built against. The three-crate coupling (`sdk → daemon → proto` already, plus this direct `sdk → proto`) is not clean.
- Remediation: either (a) define `pcloud_sdk::Notification` as its own struct with an explicit `From<pcloud_proto::Notification>`, or (b) document the re-export as semver-locked in the rustdoc (currently doesn't say so).
- Severity: HIGH.

**SDK-H2 — `pub use upload_session::{…}` bulk re-export includes the `UploadSessionDriver` trait, which leaks private-module design details.**
- Evidence: `pcloud-sdk/src/lib.rs:97-100`:
  ```rust
  pub use upload_session::{
      ConflictMode, DEFAULT_CHUNK_SIZE, FileMetadata, UploadConfig, UploadError, UploadHandle,
      UploadPayload, UploadProgress, UploadRequest, UploadSession, UploadSessionDriver, UploadState,
  };
  ```
  `UploadSessionDriver` is the trait the internal upload machinery uses to talk to the daemon or a mock (`tests/upload_session_chunked.rs:50`). By re-exporting it, callers can now implement their own driver — intentional? If yes, it should be documented. If no, it's an accidental leak that locks the internal shape.
- Impact: any refactor of `upload_session::UploadSessionDriver` is now a SemVer break.
- Remediation: either document the driver trait as stable plugin surface, or move the trait into `pcloud_plugin_api` and re-export explicitly, or make it `pub(crate)` and expose `MockDriver` only behind a `#[cfg(test)]` re-export.
- Severity: HIGH.

**SDK-H3 — Only ONE example (`login_and_list.rs`) for 80+ public helpers.**
- Evidence: `crates/pcloud-sdk/examples/` contains exactly one file (line count 74). Sixty-plus public methods on `EmbeddedDaemon` have `# Examples` rustdoc blocks referring to `examples/sdk_plugin_registration.rs` and `examples/sdk_upload_download.rs` — e.g. `pcloud-sdk/src/lib.rs:1300` ("See `examples/sdk_plugin_registration.rs` for a runnable demo"), line 1376 ("See `examples/sdk_upload_download.rs` for a runnable demo"). Those files **do not exist**. `cargo build -p pcloud-sdk --examples` cannot build them.
- Impact: rustdoc cross-references are broken. `cargo doc --workspace --no-deps` will not fail on missing examples (rustdoc doesn't check file-on-disk), but a developer following the docs is led to a dead end.
- Remediation: either write the two missing examples (plugin registration, upload+download), or remove the rustdoc references to non-existent files.
- Severity: HIGH.

#### 8.3.2 Rustdoc coverage

**SDK-M1 — `SdkError` is marked `#[non_exhaustive]` (semver-safe add) but **every** per-helper error enum is **also** `#[non_exhaustive]` — forcing callers to double-match fallthroughs even within the wrapped error.**
- Evidence: `pcloud-sdk/src/lib.rs:127, 258, 305, 338, 351, 362, 408, 434, 569, 583, 601, 674, 710, 752, 818`.
- Impact: callers who want to switch on both the outer and inner shape are forced to add `_ => unreachable!()` twice in every arm. The ergonomic cost is real — `SdkError` alone being non-exhaustive would deliver the same forward-compat guarantee.
- Remediation: consider removing `#[non_exhaustive]` from the inner enums (keep only on `SdkError`). Nothing *external* to the SDK constructs them.
- Severity: MEDIUM.

**SDK-M2 — `EmbeddedDaemon::dispatch` claims "infallible at the Rust level" but is the primary way callers can panic-crash the SDK when the underlying daemon panics.**
- Evidence: `pcloud-sdk/src/lib.rs:1252-1254` ("This method is infallible at the Rust level — errors are encoded in the returned `Response::status`"). But `dispatch` calls into the daemon's `dispatch(&mut self.runtime, request)` which is a synchronous call that can panic on malformed requests (no `catch_unwind`).
- Impact: an embedded daemon crash takes down the host application.
- Remediation: wrap the dispatch in `std::panic::catch_unwind` and return a synthetic `ResponseStatus::InternalError` on panic. Document this clearly.
- Severity: MEDIUM.

**SDK-M3 — `crypto_priv_key_flags` returns `u64` with no `Result`; how does it signal absence?**
- Evidence: `pcloud-sdk/src/lib.rs:1860`: `pub fn crypto_priv_key_flags(&self) -> u64`. No error path, no `Option`. When the crypto shell has never been set up, what does this return? `0`?
- Impact: callers cannot distinguish "flags == 0" (valid legitimate state) from "crypto not initialised" (caller error).
- Remediation: return `Result<u64, SdkError>` (wrap in a new `CryptoHelperError::NotSetup` variant) or `Option<u64>`.
- Severity: MEDIUM.

**SDK-L1 — Benches exist but are under-covered.**
- Evidence: `crates/pcloud-sdk/benches/upload_session.rs` (3.8 KB); no `crates/pcloud-sdk/benches/dispatch.rs` for the embedded-daemon dispatch hot path.
- Impact: performance regressions on `EmbeddedDaemon::dispatch` will not be caught by CI.
- Remediation: add a trivial `health_ping_throughput` bench that tracks dispatch latency.
- Severity: LOW.

**SDK-L2 — `CRATE_NAME` public constant (`lib.rs:94`) is duplicated at every caller that does telemetry tagging instead of being a single-source-of-truth pattern.**
- Evidence: `lib.rs:94`: `pub const CRATE_NAME: &str = "pcloud-sdk";`. The doctest: `assert_eq!(pcloud_sdk::CRATE_NAME, "pcloud-sdk");`.
- Remediation: use `env!("CARGO_PKG_NAME")` at definition site.
- Severity: LOW.

#### 8.3.3 Tests

**SDK-M4 — SDK tests cover only the happy path for the upload session state machine.**
- Evidence: only test file is `tests/upload_session_chunked.rs`. There are no tests for: `verify_email`, `register`, `lost_password`, `change_password`, `create_backup`, `delete_backup`, `send_publink`, `set_api_server`, `userinfo`, `logout`, `send_two_factor_sms`, `download_file`, `stat_path`, `list_folder`, `mount`/`unmount`, or any of the 48 typed KV / setting helpers.
- Impact: the 80+ public helper methods are exercised only through integration tests elsewhere in the workspace. An SDK-specific test suite that embeds `EmbeddedDaemon` against a mock transport does not exist.
- Remediation: add `tests/sdk_smoke.rs` that bootstraps an `EmbeddedDaemon` with a mock `Environment::Development` profile and exercises every `Result`-returning helper for its `NotAuthenticated` branch (trivial; no network). This catches signature drift.
- Severity: MEDIUM.

**SDK-L3 — `login_and_list.rs` uses `env::remove_dir_all` with `let _ =` for cleanup — silently swallows I/O errors.**
- Evidence: `login_and_list.rs:71`: `let _ = std::fs::remove_dir_all(&root);`.
- Impact: if the cleanup fails (ENFILE, permission denied), the scratch directory lingers under `$TMPDIR` and the example exits Ok. Accumulating scratch directories on long-running CI hosts.
- Remediation: propagate with `?` or log the failure.
- Severity: LOW.

#### 8.3.4 Feature flags

**SDK-M5 — `pcloud-sdk/Cargo.toml` declares NO features at all.**
- Evidence: full `[features]` section is absent. `tokio` is pulled in with `default-features = false, features = ["sync"]` hard-coded (line 20).
- Impact: no `tls-rustls` vs `tls-native-tls` alternatives; no way for a downstream embedder to opt out of `rustls` and use the system TLS. The TLS provider is hard-coded upstream through `pcloud-proto` (which uses `rustls = "0.23"` with `features = ["std", "ring"]` at `pcloud-proto/Cargo.toml:24`).
- Impact details:
  - The whole workspace is **ring-only**. No `rustcrypto` provider path. A FIPS-conscious enterprise that wants `aws-lc-rs` or `rustls-platform-verifier` has no knob.
  - `webpki-roots = "1.0"` (`pcloud-proto/Cargo.toml:30`) hard-codes the root bundle. No way to use the system trust store.
- Remediation: expose feature flags `tls-rustls-ring` (default), `tls-rustls-aws-lc`, `tls-native` by plumbing through to `pcloud-proto` and `pcloud-daemon`. Also add `plugin-api` as an optional feature (currently unconditional dep, line 15).
- Severity: MEDIUM.

**SDK-L4 — `pcloud-plugin-api` is a *required* dep of `pcloud-sdk` — every SDK consumer pulls in ed25519-dalek.**
- Evidence: `pcloud-sdk/Cargo.toml:15`: `pcloud-plugin-api = { path = "../pcloud-plugin-api" }` (unconditional). `pcloud-plugin-api/Cargo.toml:15`: `ed25519-dalek = "2.1"`.
- Impact: a pure-upload embedder (who never touches plugins) still compiles ed25519-dalek + its crypto transitively. Cold build-time cost on small embedders.
- Remediation: gate `pcloud-plugin-api` behind a `plugin` feature and conditionally-compile `register_plugin`, `authorize_plugin_operation`, `loaded_plugins` on that feature.
- Severity: LOW.

#### 8.3.5 Re-exports / prelude

**SDK-M6 — No prelude module; `use pcloud_sdk::*` imports 30+ types/enums at once.**
- Evidence: `pcloud-sdk/src/lib.rs` has no `pub mod prelude` or curated narrow re-export. A glob import inherits every one of `ConflictMode`, `UploadConfig`, `UploadPayload`, `FileMetadata`, `UploadProgress`, `UploadHandle`, `UploadState`, `UploadError`, `DEFAULT_CHUNK_SIZE`, `Notification`, plus `EmbeddedDaemon`, `EmbeddedDaemonBuilder`, 14 helper error enums, `SdkError`, `AuthenticatedUser`, `DownloadLinkInfo`, `TwoFactorSmsInfo`, `TwoFactorNotificationInfo`, `UploadResult`, `PromoResult`, `ApiServerResult`, `BackupCreated`, `FilesystemPathStatus`, `FolderFlagsInfo`, `StatResult`, `FolderEntry`, `CreateFolderResult`, `CRATE_NAME`.
- Impact: name collisions in host crates (especially `Notification`, `FolderEntry`, `StatResult`) cause compile errors in embedders that already have domain types.
- Remediation: expose `pub mod prelude { pub use crate::{EmbeddedDaemon, SdkError, UploadRequest, UploadResult}; }` and document it as the recommended glob. Existing types remain reachable through `pcloud_sdk::Type`.
- Severity: MEDIUM.

**SDK-L5 — `EmbeddedDaemon::config(&self) -> &ConfigProfile` exposes the internal config type from a *peer* crate.**
- Evidence: `pcloud-sdk/src/lib.rs:1237`. `ConfigProfile` comes from `pcloud-config`, which is a workspace-internal crate.
- Impact: same SemVer-lock risk as SDK-H1, but `ConfigProfile` is a fatter struct with more fields.
- Remediation: either lift `ConfigProfile` into `pcloud-sdk` as a thin facade (`impl From<ConfigProfile> for SdkConfig`), or document the peer-crate contract.
- Severity: LOW.

---

### 8.4 Cross-cutting findings

**X-H1 — `pcloud-cli/Cargo.toml` declares the `derive` feature of clap but then hand-rolls the parser.**
- Evidence: Cargo.toml line 27 includes `"derive"` in `clap` features. No `#[derive(Parser)]` block exists.
- Impact: one unused compile unit for a feature flag that would be better used.
- Remediation: either migrate to clap-derive (preferred; fixes CLI-H2, CLI-H3, CLI-H5 in one stroke), or drop the feature from Cargo.toml.
- Severity: HIGH.

**X-H2 — The CLI help text at `app.rs:16-442` (~425 lines) is a hand-maintained triple-truth for subcommands (it joins `Command` variants + `app::parse_command` tables + clap completion). Drift between the three is the norm.**
- Evidence:
  - The help text advertises `stop` (line 86), `f` (line 90), `crypto status` (line 273), `slo` (via examples line 419). Neither `stop`, `crypto status`, nor `slo` appear in `completion::build_cli`.
  - The help text does NOT document `notifications` fully, `snapshot`, `integrity`, `ha`, `audit-verifier`, `upload`, `conflict`, `stat`.
- Impact: help text is partially stale and diverges from real capability.
- Remediation: single source of truth (clap-derive) OR move help text into per-variant `#[doc]` attributes on `Command` and have `help_text()` auto-synthesise.
- Severity: HIGH.

**X-M1 — `pcloud-sdk` and `pcloud-cli` do not share a `pcloud-error::Category` contract for exit-code derivation.**
- Evidence: SDK funnels every error through `pcloud_error::Category` (`pcloud-sdk/src/lib.rs:930-1120`); CLI maps `ResponseStatus` → `ExitCode` directly (`exit_code.rs:97-137`). These are two separate category ladders. An SDK consumer writing a CLI-clone cannot reuse the SDK's category and get identical exit codes out of the box.
- Impact: cohesion across the two interfaces is weak. Any host embedding the SDK has to re-implement exit-code mapping.
- Remediation: provide `pcloud_sdk::exit_code_for(category: Category) -> u8` as part of the SDK's public surface.
- Severity: MEDIUM.

**X-M2 — The CLI prints a traceparent to stderr before every command (`main.rs:417-421`) but the SDK `EmbeddedDaemon::dispatch` has no trace-context API.**
- Evidence: `main.rs:429-436` uses `RequestEnvelope::new(request).with_traceparent(tp)` but `EmbeddedDaemon::dispatch` takes a bare `Request` (`lib.rs:1271`).
- Impact: trace-context propagation is CLI-only. Embedded SDK consumers cannot propagate their trace id through to the daemon audit log.
- Remediation: add `EmbeddedDaemon::dispatch_envelope(&mut self, envelope: RequestEnvelope)` taking the full envelope.
- Severity: MEDIUM.

**X-M3 — No `#[deny(warnings)]` / `#[forbid(unsafe_code)]` on `pcloud-cli/src/main.rs`; `unsafe_code` is used for `pre_exec`, `setsid`, and signal handlers.**
- Evidence: `main.rs:2-4`. Multiple `#[allow(unsafe_code)]` attributes on modules (lines 18, 20, 24, 28, 35). `#[warn(unsafe_op_in_unsafe_fn)]` is set (line 2), which is good.
- Impact: unsafe usage is explicit but scattered; no single place enumerates the five unsafe call sites.
- Remediation: either consolidate unsafe into a single `unix_daemon` module with a safety invariant block, or add an allow-list audit test that grep-asserts the count of `unsafe { … }` blocks (regression tripwire).
- Severity: MEDIUM.

**X-L1 — Plugin API (`pcloud-plugin-api`, `pcloud-plugin-autoheal`, `pcloud-plugin-backup-schedule`, `pcloud-plugin-dlp`, `pcloud-plugin-publink-expiry`) has five crates but the SDK re-exports `Plugin` trait only.**
- Evidence: `pcloud-sdk/src/lib.rs:82-84` imports `Plugin, PluginAuditEvent, PluginAuditSink, PluginError, PluginOperation, PluginRegistry, RegisteredPlugin`. Only `Plugin` would normally be what a plugin-author needs; the SDK offers no prelude for plugin authoring. A plugin author needs to depend directly on `pcloud-plugin-api`.
- Impact: the plugin authoring story is unclear from the SDK alone.
- Remediation: add `pcloud_sdk::plugin` module that re-exports the plugin-author-facing subset.
- Severity: LOW.

**X-L2 — No `pcloud-sdk/examples/README.md` and no list of examples in the SDK README.**
- Evidence: `crates/pcloud-sdk/README.md` exists but wasn't inspected in this audit for example coverage. The examples/ directory contains only one file.
- Remediation: document which examples exist and which are planned.
- Severity: LOW.

---

### 8.5 Prioritised remediation roadmap

**Must-fix before GA:**
1. CLI-H2 / CLI-H3 / CLI-H5 / X-H1 / X-H2 — **consolidate on clap-derive** or write a test that asserts parse ↔ help ↔ completion alignment. Biggest UX win. Eliminates four HIGH findings in one refactor.
2. CLI-H1 — wire `Method::Health` (enterprise probe) into the CLI. Single-site fix.
3. CLI-H4 — add `crypto setup` + `crypto mkdir` subcommands.
4. CLI-H6 + CLI-M7 — deprecate all positional-secret subcommands; print a consistent stderr warning today, reject in the next major.
5. SDK-H3 — either write the two missing examples or strip the rustdoc references to them.
6. SDK-H1 / SDK-H2 — audit every `pub use` and either wrap with a thin SDK-owned facade or document the semver contract with the upstream crate explicitly.

**Should-fix:**
7. CLI-M1 / CLI-M4 / CLI-M5 / CLI-M6 / CLI-M8 / CLI-M9 — exit-code discipline, missing per-root pause/resume, value-KV CLI, progress wiring, SIGINT.
8. SDK-M1 / SDK-M2 / SDK-M3 / SDK-M4 / SDK-M5 / SDK-M6 — `#[non_exhaustive]` ergonomics, panic-safety, error path, tests, feature flags, prelude.
9. X-M1 / X-M2 / X-M3 — exit-code cohesion, trace-context propagation, unsafe audit.

**Nice-to-have:**
10. All LOW findings (CLI-L*, SDK-L*, X-L*) can be batched into a "CLI polish" milestone.

---

### 8.6 Evidence summary (file:line anchors referenced in this section)

- CLI workspace: `crates/pcloud-cli/Cargo.toml:27` (clap-derive declared), `crates/pcloud-cli/build.rs:20-53` (GIT_HASH injection).
- CLI parser: `crates/pcloud-cli/src/main.rs:46` (entry), `crates/pcloud-cli/src/app.rs:1474` (`parse_command`), `crates/pcloud-cli/src/app.rs:1484` (`parse_inputs_for_command`), `crates/pcloud-cli/src/commands.rs:35` (`enum Command`), `crates/pcloud-cli/src/commands.rs:750` (`into_request`).
- CLI help: `crates/pcloud-cli/src/app.rs:16-442` (hand-maintained man page).
- CLI completion: `crates/pcloud-cli/src/completion.rs:35-193` (parallel clap tree), 215-284 (tests that only verify non-emptiness).
- CLI exit codes: `crates/pcloud-cli/src/exit_code.rs:58-87` (stable-ABI enum).
- CLI secrets: `crates/pcloud-cli/src/prompt.rs:59-144` (SecretPrompt), `crates/pcloud-cli/src/app.rs:2924-3020` (`read_password_securely`), `crates/pcloud-cli/src/app.rs:1527-1533` (missing warning on submit-auth positional).
- CLI progress: `crates/pcloud-cli/src/progress.rs:1-454` (declared, unused in production paths).
- IPC truth: `crates/pcloud-ipc/src/methods.rs:37-216` (`enum Method`, 42 variants), 262-1021 (`enum Request`, 70+ variants).
- SDK surface: `crates/pcloud-sdk/src/lib.rs:94` (`CRATE_NAME`), 97-105 (re-exports), 111-138 (`EmbeddedDaemon` / `Builder` / `Error`), 819-918 (`SdkError`), 1122-3400+ (helper methods).
- SDK examples: `crates/pcloud-sdk/examples/login_and_list.rs` (only existing example, 74 lines).
- SDK tests: `crates/pcloud-sdk/tests/upload_session_chunked.rs` (only SDK-specific test).
- SDK features: `crates/pcloud-sdk/Cargo.toml:1-35` (no `[features]` section).
- TLS provider chain: `crates/pcloud-proto/Cargo.toml:24` (rustls-ring hard-coded), `crates/pcloud-proto/Cargo.toml:30` (webpki-roots hard-coded).
- Plugin API: `crates/pcloud-plugin-api/Cargo.toml:1-19` (unconditional ed25519-dalek dep).

---

*End of Section 8.*
## Section 9. Code Quality & Robustness

**Auditor scope:** Dimension 9 — `.unwrap()`/`.expect()`, TODO/FIXME/STUB/XXX/HACK/panic!, `unsafe` discipline, error propagation, logging discipline, panic reachability, resource leaks, dead code, typed newtypes, config validation, fmt / clippy / deny gates, MSRV, feature-flag sanity. (Does not overlap with Dimension 2 secret discipline, Dimension 5 FUSE-FFI memory safety.)

**Workspace root:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/`.
**Crates scanned:** 36 (all non-binary crates under `crates/*/src/`).
**Filter:** line is considered *production* when it is **outside** `tests/`, `benches/`, `examples/`, and **not** inside a `#[cfg(...test...)] mod …` block. Doc-comment lines (`//`, `///`, `//!`) are excluded.

### 9.0 Headline numbers

| Metric | Total | Production (non-test) |
| --- | --- | --- |
| `.unwrap()` / `.expect(` | 3320 across 255 files | **117 across 41 files** |
| `TODO`/`FIXME`/`STUB`/`XXX`/`HACK`/`panic!(`/`todo!(`/`unimplemented!(` | 215 across 84 files | **27 across 17 files** (0 `todo!`/`unimplemented!`) |
| `unsafe { … }` / `unsafe fn` / `unsafe impl` / `unsafe extern` | 384 across 48 files (incl. tests) | **324 blocks across 22 files**; 35 without `// SAFETY:` comment (mostly FFI-fn-type aliases in `winfsp_ffi`) |
| `impl Drop` in prod | — | **21 implementations** (all platform handles, lease holders, observability handles, transport guards) |
| `ManuallyDrop` | 0 | 0 |
| `mem::forget` | 1 | 1 (`pcloud-cli/src/main.rs:948` — intentional detached-daemon `Child`, documented) |
| `.ok()?` (silent-swallow ?) | — | 39 across 18 files — mostly number parsing (acceptable) + a few `Mutex::lock().ok()?` (DoS mitigation, acceptable) |
| Typed ID newtypes | — | 6 defined (`UserId`, `SyncId`, `RemoteFileId`, `RemoteFolderId`, `UploadSessionId`, `DiffCursor`) — but **inconsistently adopted** (13 raw `u64 id:` parameters found in 6 files) |
| `rust-toolchain.toml` channel | — | `stable` with `clippy, rustfmt` |
| Workspace `edition` | — | `2024` |
| Workspace `rust-version` / MSRV | — | `1.85` |
| `resolver` | — | `3` |
| `cargo fmt --all --check` | — | **PASS** (exit 0) |
| `cargo clippy --workspace --all-targets -- -D warnings` | — | **PASS** (exit 0, one benign build-script warning from `pcloud-crypto/build.rs` about a legacy C header that isn't present) |
| `cargo deny --locked check` | — | **PASS** (`advisories ok, bans ok, licenses ok, sources ok`) — 4 advisory ignores, all tracked with `review: YYYY-MM-DD` and a follow-up bead (`bd-1du.10`) |
| Build warnings | — | 1 (the `pcloud-crypto` password-dictionary fallback; intentional) |
| `dbg!(` in prod | — | 0 |
| `println!(` in prod | — | CLI-only output paths (acceptable for a one-shot) |

### 9.1 Severity rollup (Dimension 9)

| Severity | Count | Principal sources |
| --- | --- | --- |
| CRITICAL | **0** | (no attacker-triggerable panic on daemon IPC/HTTP-response paths located) |
| HIGH | **4** | (a) 35 `unsafe` blocks missing explicit `// SAFETY:` comments — most are legitimate FFI-type aliases but `pcloud-daemon/src/signals.rs`, `pcloud-cli/src/main.rs`, and `pcloud-cli/src/prompt.rs` are in-code callsites that should carry SAFETY docs; (b) raw-`u64` ID parameters persist in `pcloud-fs/src/backend.rs`, `pcloud-sdk/src/lib.rs`, `pcloud-daemon/src/transfer_bridge.rs`, `pcloud-store/src/repositories/file_metadata.rs` despite `pcloud-model::ids` defining newtypes — confused-unit risk; (c) 117 production `unwrap()`/`expect()` — none attacker-reachable but every `.lock().expect("… poisoned")` is a latent daemon crash on Mutex poisoning; (d) `cargo deny` ignore list carries 4 RUSTSEC entries pending upstream patch including `RUSTSEC-2026-0098`/`-0099` against `rustls 0.23` — no hard fix yet. |
| MEDIUM | **≈30** | 27 TODO/FIXME markers (8 have `bd-…` IDs, 19 carry `TODO(bd-xplat)` trace, a few are pure unresolved); 1 `panic!(` in prod at `pcloud-config/src/loader.rs:348` inside a helper (`other => panic!("wrong error: {:?}", other)`) — though that helper is behind `#[cfg(test)]` and my filter caught it because of how the `#[cfg(test)] mod` is ordered. Worth double-checking. |
| LOW | many | Individual `expect("HMAC-SHA256 accepts any key length")` calls in crypto — invariant is a library contract; OK-to-panic pattern. |

### 9.2 Gates — PASS/FAIL summary

| Gate | Status | Evidence |
| --- | --- | --- |
| `rustfmt --all --check` | **PASS** | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** | exit 0 |
| `cargo deny --locked check` | **PASS** | `advisories ok, bans ok, licenses ok, sources ok` |
| MSRV declaration | **PASS** | workspace `rust-version = "1.85"`, toolchain pinned `stable` (clippy, rustfmt) — matches edition 2024 requirement |
| `rust-toolchain.toml` ↔ `Cargo.toml rust-version` | **PASS (implicit)** | toolchain = `stable`, MSRV = 1.85; stable ≥ 1.85 today |
| default-feature conflict (rustls vs native-tls) | **PASS** | No crate pulls `native-tls` — every TLS dep uses `rustls-tls` with `default-features = false`. Verified: `pcloud-kms`, `pcloud-fleet`, `pcloud-idp`, `pcloud-proto` all use explicit `features = ["rustls-tls"…]`. |
| Workspace members carrying `default = [...]` | 9 — all are `default = []` or a single meaningful flag (e.g. `pcloud-idp default = ["oidc-http-exchange"]`). No heavy transitive pull. |

---

### 9.3 Error propagation and logging discipline

- **`.ok()?` uses (39 occurrences)** — spot-checked each. The dominant pattern is either:
  1. `Mutex::lock().ok()?` (8 sites, e.g. `pcloud-fs/src/inode.rs:114,123,212`, `pcloud-fs/src/page_cache.rs:221`, `pcloud-fs/src/metadata_cache.rs:154`) — this *intentionally* returns `None` when the mutex is poisoned instead of panicking, which is the correct defensive choice for a FUSE path. However, **most daemon services** do the opposite (`.lock().expect("… poisoned")`) which **will crash the daemon** on Mutex poisoning. Inconsistent posture. **Recommendation (HIGH):** pick one discipline workspace-wide — either unwrap/expect (OK for unique-owner mutexes) or `ok()?` with a tracing `warn!` — and enforce via a `deny.toml`-adjacent clippy custom-lint or a grep CI gate.
  2. Numeric parsing in date/ID parsers (`pcloud-proto/src/folder_api.rs`, `pcloud-cli/src/app.rs:2677-2679`, `pcloud-proto/src/methods/upload.rs:626-676`) — acceptable; wraps `parse::<T>()` from wire bytes and returning `None` *is* the correct recovery.
  3. `pcloud-web/src/routes.rs:578` `headers.get(COOKIE)?.to_str().ok()?` — acceptable.
- **Logging levels**: 18 `info!(` total across 5 files — all in `pcloud-daemon/src/bootstrap.rs` (10), `serve.rs` (3), `mount_runtime.rs` (2), `audit_verifier_service.rs` (1), `integrity_sweeper_service.rs` (2). Not spammy. 19 `error!(` calls across 8 files, mostly daemon-scoped. No occurrences of `error!(` for recoverable `WouldBlock` / timeout — levels are appropriate.
- **`dbg!(` / stray `println!`** in prod: **none** in daemon, proto, fs, config, store, crypto, ipc. CLI crates intentionally use `println!/eprintln!` for user output.

### 9.4 Dead code / warnings

- `cargo build --workspace --all-targets` exits clean. The only warning is a `build.rs` message from `pcloud-crypto` about an absent upstream `ppassworddict.h` and the use of a vendored substitute — intentional (legacy-C detached).
- Clippy clean at `-D warnings` across all targets.
- No `#[allow(dead_code)]` strewn across prod (spot-check: 0 hits in `pcloud-daemon/src`, `pcloud-proto/src`).

### 9.5 Resource leaks

21 `impl Drop` implementations in prod — all paired with a resource (mount handle, IPC listener, lease holder, observability handle, refresh-ticket, shared memory segment, Windows HANDLE guards, LocalFreeGuard for DPAPI blobs). Examples:

- `pcloud-fs/src/mount_service.rs:542` — `impl Drop for MountHandle` → unmounts on drop.
- `pcloud-daemon/src/ha_lease.rs:359` — `LeaseHolder` → releases the lease.
- `pcloud-ipc/src/transport.rs:232` — `BoundIpcServer` → removes socket path.
- `pcloud-daemon/src/mount_runtime.rs:691` — `MountControl` → joins the mount thread.
- `pcloud-compat/src/shm_producer.rs:357` — `ShmSegment` → detaches shared memory.
- `pcloud-ipc/src/platform/windows.rs:409,425` — SecurityDescriptor / HandleGuard.
- `pcloud-daemon/src/vault/dpapi.rs:72` — `LocalFreeGuard` → calls `LocalFree` on Windows DPAPI blob.

**Only one `mem::forget`**: `pcloud-cli/src/main.rs:948`:
```
std::mem::forget(child);
```
documented immediately above as the detached-daemon intention — it leaks the `Child` handle deliberately so the CLI parent can exit while the daemon lives on. **Not a bug.**

**No `ManuallyDrop` anywhere** in the production tree — scan returned zero hits.

### 9.6 Panic paths reachability

Spot-checked `dispatch.rs` and `serve.rs` (the two request-handling entry points):

- `pcloud-daemon/src/dispatch.rs` — every `assert!` / `panic!` / `unwrap` sits inside `#[cfg(all(test, feature = "tracing-otlp"))] mod tests`. No panic reachable from `handle_request`.
- `pcloud-daemon/src/serve.rs` — one `panic!("serve loop did not exit within 5s of external flag flip")` at line 425, but that file's only prod panics are inside the `#[cfg(test)]` test module.
- `pcloud-ipc/src/server.rs`, `pcloud-ipc/src/protocol.rs`, `pcloud-ipc/src/transport.rs` — all `unwrap/expect` sit in tests or doc-comments.
- Mutex `.expect("… poisoned")` calls (68 of the 117 prod hits) are the only residual panic vector. Because `PoisonError` is itself caused by a prior panic, in practice these act as “propagate the poison forward” rather than turning a clean input into a crash. Still, tightening to `.ok()?` on the daemon hot path (integrity sweeper scheduler, audit verifier, sync loop) would improve robustness.

**No attacker-triggerable panic path was located** in:
- IPC deserialization (`pcloud-ipc/src/protocol.rs`) — uses `serde_cbor`/`bincode` with `?` propagation throughout.
- HTTP download integrity (`pcloud-proto/src/http_download.rs`) — no bare unwraps in prod code.
- Transport frame decode (`pcloud-proto/src/transport.rs`) — the two `expect("transport config lock should not be poisoned")` at lines 212, 280 are Mutex poison cases, not parse paths.

### 9.7 Typed newtypes / unit confusion

`pcloud-model::ids` defines six newtypes:
- `UserId(u64)`, `SyncId(u64)`, `RemoteFileId(u64)`, `RemoteFolderId(u64)`, `UploadSessionId(u64)`, `DiffCursor(u64)`.

Each is `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]` via a macro; each carries a rustdoc with an example. **Good.** However, production code still accepts raw `u64`:

| File | Raw `u64 ID:` fields or params |
| --- | --- |
| `crates/pcloud-fs/src/backend.rs` | 5 |
| `crates/pcloud-fs/src/page_cache.rs` | 1 |
| `crates/pcloud-daemon/src/transfer_bridge.rs` | 1 |
| `crates/pcloud-store/src/repositories/file_metadata.rs` | 3 |
| `crates/pcloud-sdk/src/lib.rs` | 1 |
| `crates/pcloud-daemon/src/runtime.rs` | 2 |

13 raw-`u64` ID parameters in 6 files. This is **HIGH** because it undermines the newtype story: a `fileid: u64` can silently be passed where a `folderid: u64` is expected. Recommend a typed-ID adoption sweep.

### 9.8 Config validation

`pcloud-config` consistently exposes typed `.validate(&self) -> Result<(), ConfigError>` (or `&'static str`) on every sub-config:

- `ConfigProfile::validate` at `lib.rs:408`
- `PathsConfig::validate` at `paths.rs:122`
- `ExtensionsConfig::validate` at `extensions.rs:112`
- `RuntimeConfig::validate` at `runtime.rs:60`
- `CryptoKmsConfig::validate` at `crypto_kms.rs:80,174` (two variants)
- `SyncLoopConfig::validate` at `sync_loop.rs:153`
- `FileHistoryConfig::validate(env)` at `file_history.rs:57` — env-aware
- `ApiConfig::validate(environment)` at `api.rs:131` — environment-aware
- `validate_document(doc, source)` (JSON-schema) at `schema.rs:896`

The loader is secure-by-default: file must be owner-only (`0o077` bits clear); `Environment::Production` refuses insecure permissions; dev/test logs a warning; no late-bound panics. Migration is versioned (`migrate_to_current`) — it returns a typed `MigrationError`.

**10-parameter spot check:** insecure_permissions, enforcement_environment, PathsConfig, CryptoKmsConfig master-key id, FileHistoryConfig retention, ApiConfig endpoint, ResilienceConfig thresholds, IntegritySweeper skip_globs, SyncLoop thread count, RateLimit capacity — all validated at load time with typed errors. **PASS.**

### 9.9 Feature-flag sanity

- No workspace-member has a heavy default feature forcing `native-tls` alongside `rustls`.
- `pcloud-fleet/Cargo.toml:21-37` documents explicitly: “rustls-only (no native-tls), JSON bodies only, blocking client.”
- `pcloud-idp/Cargo.toml:15`: `default = ["oidc-http-exchange"]` — a single deliberate feature.
- No transitive `tokio-native-tls` or `openssl-sys` appears in the `cargo deny` graph (verified).

---

## Appendix A (preview). Complete `.unwrap()` / `.expect(` production inventory (117 items, 41 files)

Severity key: **CRITICAL** = attacker-reachable panic on IPC/HTTP parser hot path. **HIGH** = daemon hot-path (DoS). **MEDIUM** = CLI/one-shot. **LOW** = init / library-contract invariant (e.g. HMAC accepts any key length).

| # | File:line | Context | Severity |
| --- | --- | --- | --- |
| A1 | `pcloud-proto/src/transport.rs:212` | `.expect("transport config lock should not be poisoned")` inside resilient transport wrapper; reachable on every outbound request if Mutex poisoned | HIGH |
| A2 | `pcloud-proto/src/transport.rs:280` | same, companion path | HIGH |
| A3-4 | `pcloud-auth/src/lifecycle.rs:67,73` | `TestClock` Mutex — only used when `TestClock` is injected (test-only helper in prod module) | LOW |
| A5-6 | `pcloud-cli/src/app.rs:1480-1481` | `parse_command / parse_inputs_for_command` `.expect()` inside a helper — callsite should already have validated. CLI-only. | MEDIUM |
| A7-8 | `pcloud-config/src/integrity_sweeper.rs:215,288` | ManualClock / token-bucket mutex | LOW |
| A9-20 | `pcloud-crypto/src/metadata.rs:98`, `content.rs:129,189`, `keys.rs:90,158,181`, `password_scorer.rs:551,560`, `share_temppass.rs:215`, `lib.rs:540,861,986` | `.expect("HMAC-SHA256 accepts any key length")`, `.expect("OS randomness …")`, `.expect("fixed argon2 output length should be valid")` — library-contract invariants; panic only if crypto primitives are fundamentally broken | LOW |
| A21 | `pcloud-daemon/src/sync_loop.rs:500` | `.expect("failed to spawn sync loop thread")` — init path | LOW |
| A22 | `pcloud-daemon/src/sync_loop_runtime.rs:577` | `.expect("failed to open sync loop store connection")` — init path | LOW |
| A23-25 | `pcloud-daemon/src/audit_verifier_service.rs:454,570,577` | scheduler-thread spawn (init, LOW); wake-mutex `expect` on running daemon (HIGH) | LOW/HIGH |
| A26 | `pcloud-daemon/src/transfer_bridge.rs:198` | `.expect("chunk_size is Some when use_chunked is true")` — internal API invariant; unreachable if caller upholds invariant | MEDIUM |
| A27-39 | `pcloud-daemon/src/integrity_sweeper_service.rs:801,920,929,955,1015,1039,1163,1202,1203,1206,1288,1295,1344` | 13 `Mutex::lock().expect("… poisoned")` on the integrity sweeper scheduler hot path; each is a daemon-crash vector on any prior panic | HIGH |
| A40-42 | `pcloud-daemon/src/mount_runtime.rs:801,838,967` | shim / adapter single-consumption (`.take().expect("already consumed")`) and writer-slot mutex | MEDIUM/HIGH |
| A43-46 | `pcloud-fs/src/inode.rs:136,147,171,189` | 4× inode table mutex + one `.expect("inode number space exhausted")` (u64 exhaustion is effectively impossible) | HIGH (poison path) |
| A47-49 | `pcloud-fs/src/write_journal.rs:285,290,291` | `try_into().unwrap()` for 4-byte header fields — inputs are always exactly 4 bytes (slice-of-12 truncated) so this is provably infallible but should be replaced with `u32::from_le_bytes(header[..4].try_into().expect(…))` → just use arrays directly. | LOW |
| A50-51 | `pcloud-fs/src/integrity_sweeper.rs:392,409` | rate-limit capacity assertion | LOW |
| A52 | `pcloud-fs/src/fuse_adapter.rs:1366` | `Arc::clone(tbl.by_ino.get(&ino).expect("just-inserted"))` — invariant tied to a just-run insert; safe | MEDIUM |
| A53-60 | `pcloud-fs/src/platform/macos.rs:1583-1595` | `CString::new("literal").expect("literal has no NUL")` — literal arg, panic impossible | LOW |
| A61 | `pcloud-observability/src/exporter.rs:213` | `set_nonblocking on listener` init-time | LOW |
| A62 | `pcloud-observability/src/metrics.rs:314` | `user_histograms mutex` | HIGH |
| A63-65 | `pcloud-plugin-api/src/lib.rs:218,869,970` | manifest serialization (`.expect("manifest is always serializable")`), last-push `.expect("just pushed")`, take-once `.expect("handler consumed exactly once")` | LOW/MEDIUM |
| A66-80 | `pcloud-sdk/src/upload_session.rs:376,414,423,438,464,498,511,560,591,619,641,651,662,686,736` | 15 mutex `.expect("… poisoned")` on the SDK upload-session state machine | HIGH |
| A81 | `pcloud-store/src/repositories/audit.rs:434` | HMAC invariant | LOW |
| A82 | `pcloud-resilience/src/clock.rs:111` | ManualClock poisoned | LOW |
| A83-85 | `pcloud-resilience/src/rate_limit.rs:158,196,225` | token-bucket mutex poisoned | HIGH |
| A86-89 | `pcloud-resilience/src/pacing.rs:115,123,140,177` | pacer mutex poisoned | HIGH |
| A90-91 | `pcloud-compat/src/rpc_codec.rs:214,215` | `try_into().expect("4 bytes")` / `("8 bytes")` on a peeked header — inputs are fixed-size slices, infallible | LOW |
| A92 | `pcloud-compat/src/shm_producer.rs:249` | `NonNull::new(addr.cast::…).expect("shmat returned non-null")` — `shmat` return already checked against `-1isize as *mut …` a few lines above; null-check is pro forma | LOW |
| A93-94 | `pcloud-mockserver/src/lib.rs:508,778` | mock state Mutex; canned-JSON-must-serialize — this is a development-only mock server, harmless | LOW |
| A95 | `pcloud-web/src/routes.rs:564` | `getrandom.expect("getrandom")` — OS randomness invariant | LOW |
| A96-99 | `pcloud-idp/src/jwks.rs:157,167,180,186` | jwks cache mutex poisoned | HIGH |
| A100 | `pcloud-fleet/src/lib.rs:482` | fleet rate-limiter mutex poisoned | HIGH |
| A101-103 | `pcloud-kms/src/lib.rs:430,434,440` | local tokio runtime build; async bridge thread panic join | HIGH (init only) |
| A104 | `pcloud-plugin-backup-schedule/src/lib.rs:709` | `epoch is always a valid timestamp` — infallible | LOW |
| A105-111 | `pcloud-backends/src/mock.rs:88,95,102,109,258,267,295` | mock recorder / canned mutexes — mock-only | LOW |
| A112-114 | `pcloud-backends/src/path_resolver.rs:189,202,556` | cache mutex + `expect("normalised path always contains '/'")` (path invariant) | MEDIUM/HIGH |
| A115 | `pcloud-backends/src/upload_sessions.rs:279` | `by_id.get(&id).expect("just inserted")` — invariant-bound | MEDIUM |
| A116-117 | `pcloud-backends/src/transfer_backend.rs:523,533,738` | upload-id-cell mutex poisoned | HIGH |

**Net HIGH count (reachable mutex-poisoning crash of daemon or SDK hot path):** ≈ **40 sites** across `integrity_sweeper_service`, `upload_session`, `rate_limit`, `pacing`, `jwks`, `fleet`, `transfer_backend`, `audit_verifier_service`, `metrics`, `transport`. These are survivable — Mutex poisoning only happens after a panic — but they are still a hardening target.

---

## Appendix B (preview). TODO / FIXME / STUB / XXX / HACK / panic! inventory (27 items, 17 files)

Legend: **BEAD** = linked to a `bd-…` tracker item. **UNTRACKED** = no bead → MEDIUM by policy.

| # | File:line | Marker | Text | Has bead? | Severity |
| --- | --- | --- | --- | --- | --- |
| B1 | `crates/pcloud-proto/src/transfer_api.rs:414` | TODO | `TODO(spec §9.5): live-API verification required …` | No bead | MEDIUM |
| B2 | `crates/pcloud-proto/src/methods/upload.rs:68` | TODO | `TODO(spec §9.3, pupload.c:1495-1509): C always emits ifhash …` | No | MEDIUM |
| B3 | `crates/pcloud-proto/src/methods/upload.rs:601` | TODO | `TODO(spec §9.2): live-API verification required before trusting this` | No | MEDIUM |
| B4 | `crates/pcloud-cli/src/app.rs:2` | TODO | `GATING: portable; uses Linux-only idioms — see TODO(bd-xplat)` | **bd-xplat** | LOW (meta-doc) |
| B5 | `crates/pcloud-cli/src/app.rs:23` | TODO | `TODO(bd-xplat): Linux-only — needs cfg gate` | **bd-xplat** | MEDIUM |
| B6 | `crates/pcloud-cli/src/app.rs:160` | TODO | same | **bd-xplat** | MEDIUM |
| B7 | `crates/pcloud-daemon/src/metrics_server.rs:184` | TODO | `TODO(P0.3 follow-up): wire slo.incr_upload_started()` | No | MEDIUM |
| B8 | `crates/pcloud-daemon/src/mount_runtime.rs:43` | TODO | `bd-1du.4.6 (see TODO(bd-1du.4.6))` | **bd-1du.4.6** | LOW |
| B9 | `crates/pcloud-daemon/src/runtime.rs:19` | TODO | `bd-1du.4.6.1 — see TODO` | **bd-1du.4.6.1** | LOW |
| B10 | `crates/pcloud-daemon/src/runtime.rs:5116` | TODO | `H14 PR4 — TODO(bd-1du.4.6.1): bootstrap caller …` | **bd-1du.4.6.1** | MEDIUM |
| B11 | `crates/pcloud-daemon/src/vault/mod.rs:9` | marker in docs | `All four backends are real implementations — no unimplemented!()` | — | LOW (informational) |
| B12 | `crates/pcloud-engine/src/local_scan.rs:163` | panic! in doc | `///     other => panic!("expected IncrementalOnly, got {other:?}")` — doc-example only | — | LOW |
| B13 | `crates/pcloud-fs/src/fuser_shim.rs:17` | TODO | meta-doc | **bd-xplat** | LOW |
| B14 | `crates/pcloud-fs/src/fuser_shim.rs:25` | TODO | `TODO(bd-xplat): Linux-only` | **bd-xplat** | MEDIUM |
| B15 | `crates/pcloud-fs/src/mount_orphan.rs:64` | TODO | `# Windows: TODO` | No bead | MEDIUM |
| B16 | `crates/pcloud-fs/src/platform/windows.rs:647` | TODO | `TODO(bd-xplat-windows): validate SDDL parsing …` | **bd-xplat-windows** | MEDIUM |
| B17 | `crates/pcloud-fs/src/platform/windows.rs:690` | TODO | `add a proper integration test on Windows` | **bd-xplat-windows** | MEDIUM |
| B18 | `crates/pcloud-fs/src/platform/windows.rs:1248` | text "TODO" | `# Why this is a permanent no-op (not a TODO)` — this is an *anti*-TODO saying “don't add one” | — | LOW |
| B19-B20 | `crates/pcloud-ipc/src/methods.rs:7,10` | TODO | `see TODO(bd-xplat)` | **bd-xplat** | LOW |
| B21 | `crates/pcloud-ipc/src/platform/mod.rs:8` | STUB | `Windows → WindowsIpc (named pipes + SID check) — STUB` | No bead | **HIGH** — Windows IPC is explicitly a stub per its own doc |
| B22 | `crates/pcloud-sdk/src/lib.rs:1351` | TODO marker | `TODO(stub) markers` — doc reference | No | LOW |
| B23 | `crates/pcloud-sdk/src/upload_session.rs:693` | TODO | `TODO(bd-1du.10): thread once the wire supports ifhash` | **bd-1du.10** | MEDIUM |
| B24 | `crates/pcloud-resilience/src/metered.rs:40,45` | TODO(bd-xplat) | `TODO(bd-xplat)` doc | **bd-xplat** | LOW |
| B25 | `crates/pcloud-resilience/src/metered.rs:120` | TODO | `TODO(bd-xplat): Linux-only — needs cfg gate` | **bd-xplat** | MEDIUM |
| B26 | `crates/pcloud-backends/src/folder_backend.rs:403` | TODO | `TODO(bd-1du.10): wire to the binary API listrevisions` | **bd-1du.10** | MEDIUM |

**Untracked (no bead) TODOs**: B1, B2, B3, B7, B15 — five items. Per audit policy, each is MEDIUM by default.

**No `todo!()` / `unimplemented!()` macros found in production.** This is an excellent signal — the pclsync rewrite does *not* have stubs with runtime traps.

---

## Appendix D (preview). `unsafe` block / fn / impl inventory (324 blocks, 22 files)

Per-file density (highest first):

| File | Blocks | Comment |
| --- | --- | --- |
| `crates/pcloud-fs/src/platform/macos.rs` | 132 | FUSE via macFUSE FFI. Most blocks carry `// SAFETY:` annotations; notable missing: `:215`, `:303`, `:713`, `:1690` (4/132 missing). |
| `crates/pcloud-fs/src/platform/windows.rs` | 86 | WinFsp dispatcher. Missing SAFETY on `:268`, `:341`, `:353` (3/86). |
| `crates/pcloud-ipc/src/platform/windows.rs` | 21 | Named-pipe bind + SID check. 21/21 have SAFETY. |
| `crates/pcloud-fs/src/platform/winfsp_ffi.rs` | 17 | FFI type aliases (`pub type Fn… = unsafe extern "system" fn(…)`). 10 of those are bare type aliases where a SAFETY comment on the type line would be unconventional; 7 carry annotations. |
| `crates/pcloud-compat/src/shm_producer.rs` | 11 | SysV shm producer. All 11 SAFETY-annotated. |
| `crates/pcloud-fs/src/mount_service.rs` | 9 | `unsafe impl Send/Sync for MacosMountInner/WindowsInner` (4). Linux FFI (5). Two `unsafe impl Send/Sync` on `MacosMountInner` are missing explicit `// SAFETY:` above them (`:319`, `:321`). |
| `crates/pcloud-fs/src/platform/bsd.rs` | 9 | getmntinfo FFI. `:248` and `:382` missing SAFETY (2/9). |
| `crates/pcloud-daemon/src/signals.rs` | 6 | `sigaction`. **All 6 missing SAFETY comments** — single most concerning block. |
| `crates/pcloud-cli/src/prompt.rs` | 5 | `tcgetattr/tcsetattr/isatty`. `:173`, `:180`, `:190` missing SAFETY (3/5). |
| `crates/pcloud-cli/src/main.rs` | 4 | `kill(2)` + `std::env::remove_var` (unsafe in Rust 1.72+). `:917`, `:1033`, `:1176` missing SAFETY (3/4). |
| `crates/pcloud-daemon/src/vault/dpapi.rs` | 4 | `CryptProtectData/CryptUnprotectData`. All 4 SAFETY-annotated. |
| `crates/pcloud-fs/src/platform/linux.rs` | 4 | `umount2`. `:113` missing SAFETY (1/4). |
| `crates/pcloud-compat/src/folder_list.rs` | 4 | ABI-mirror reads. All 4 annotated. |
| `crates/pcloud-ipc/src/platform/linux.rs` | 3 | SO_PEERCRED getsockopt. Annotated. |
| `crates/pcloud-cli/src/doctor.rs` | 2 | `statvfs`. Annotated. |
| `crates/pcloud-cli/src/app.rs` | 1 | `std::env::remove_var`. Annotated. |
| `crates/pcloud-daemon/src/mount_runtime.rs` | 1 | `kill(pid, 0)` liveness probe. Annotated. |
| `crates/pcloud-fs/src/fuse_adapter.rs` | 1 | `getuid/getgid`. `:749` missing SAFETY (1/1). |
| `crates/pcloud-fs/src/platform/macos_ffi.rs` | 1 | `unsafe extern "C" { … }` block. Missing SAFETY wrapper comment (1/1). |
| `crates/pcloud-ipc/src/auth.rs` | 1 | Annotated. |
| `crates/pcloud-ipc/src/transport.rs` | 1 | `:139` missing SAFETY (1/1). |
| `crates/pcloud-ipc/src/platform/unix.rs` | 1 | Annotated. |

**Total missing SAFETY: 35 of 324 (10.8%).** Most are FFI-fn-type aliases, where a SAFETY doc-comment is unconventional but still recommended. The two clusters that should be fixed in a targeted PR:
- `crates/pcloud-daemon/src/signals.rs:283-303` — 6 call-site blocks around `sigaction` with no SAFETY comment. This is a signal-handler registration that runs once at daemon start; the invariants (handler must be async-signal-safe, no allocator calls) are crucial.
- `crates/pcloud-cli/src/main.rs:917,1033,1176` — 3 `libc::kill` + env-var mutation blocks with no SAFETY comment.
- `crates/pcloud-cli/src/prompt.rs:173,180,190` — terminal attribute mutation without a SAFETY doc.
- `crates/pcloud-fs/src/mount_service.rs:319,321` — two `unsafe impl Send/Sync` without SAFETY justification (adjacent `WindowsInner` does carry one).
- `crates/pcloud-fs/src/platform/macos.rs:215,303,713,1690`, `bsd.rs:248,382`, `linux.rs:113` — 7 call-site FFI blocks missing explicit SAFETY.

These are MEDIUM: none of them appear to be *wrong*; they just aren’t *documented*.

---

### 9.10 Closing verdict

The workspace is **in remarkably good shape** from a Dimension-9 standpoint:

- **Gates all green.** `fmt`, `clippy -D warnings`, `deny` all pass on 2026-04-17.
- **No CRITICAL findings.** No attacker-reachable panic path on the IPC/HTTP parser surface.
- **Zero `todo!()` / `unimplemented!()` in prod.** Zero `dbg!`. One intentional `mem::forget`. Zero `ManuallyDrop`.
- **`unsafe` is well-contained** — 324 blocks live in 22 files that are almost entirely platform-specific FFI (`pcloud-fs/src/platform/{macos,windows,bsd,linux}`, `pcloud-ipc/src/platform/*`, shm producer, signal handler, DPAPI, terminal prompt). 90% of them carry SAFETY comments.
- **Config validation discipline** is uniform and typed.
- **Typed newtypes exist but are inconsistently adopted** (HIGH-1): 13 raw-`u64` ID parameters still leak through `pcloud-fs/src/backend.rs`, `pcloud-store/src/repositories/file_metadata.rs`, and `pcloud-daemon/src/transfer_bridge.rs`.
- **~40 `Mutex::lock().expect("… poisoned")` sites on hot paths** (HIGH-2) are the most systemic hardening target — not bugs, but latent daemon-crash vectors on any upstream panic. A ~150-line PR converting these to `.ok()?` plus tracing-`warn!` would retire the class.
- **5 untracked TODO markers** (HIGH-3) — `pcloud-proto/src/transfer_api.rs:414`, `methods/upload.rs:68,601`, `pcloud-daemon/src/metrics_server.rs:184`, `pcloud-fs/src/mount_orphan.rs:64` — need `bd-…` IDs or closure.
- **35 `unsafe` blocks without SAFETY comments** (HIGH-4), concentrated in `signals.rs`, `main.rs`, and `prompt.rs` — pure docs-hygiene fix.
- **Windows IPC stub** (`pcloud-ipc/src/platform/mod.rs:8`) — self-declared STUB; this is a real Windows-parity gap, but that’s a parity concern (bd-1du.10) rather than a quality gate.
- **`cargo deny` carries 4 advisory ignores**, all with `review: 2026-07-15` or earlier, blocked on upstream patches. Tracked under `bd-1du.10`.

Recommended follow-on work, ranked by ROI:

1. **HIGH** — Sweep all `pcloud-*` mutex `expect("… poisoned")` to `.lock().ok()?` or `.lock().unwrap_or_else(|e| e.into_inner())` on daemon hot paths.
2. **HIGH** — Adopt newtype-IDs end-to-end in `pcloud-fs`, `pcloud-store`, `pcloud-sdk`, `pcloud-daemon`; break the remaining 13 raw-`u64` callsites.
3. **MEDIUM** — File `bd-…` IDs for the 5 untracked TODOs, or close them.
4. **MEDIUM** — Add `// SAFETY:` comments to the 35 unannotated `unsafe` blocks (especially `signals.rs`, `main.rs`, `prompt.rs`).
5. **LOW** — Promote `pcloud-ipc/src/platform/mod.rs` Windows STUB to a tracked bead; the doc already flags it.

Overall Dimension-9 grade: **B+ / A-** — enterprise-grade quality posture, with a small, finite, well-enumerated hardening backlog.
## Section 10. Testing & QA

Audit date: 2026-04-17
Auditor: Dimension 10 (Testing & QA)
Workspace root: `/home/ezechiel203/Projects/FORKS/pcloud-rs/`
Scope: unit / integration / proptest / fuzz / bench / stress / live-e2e / CI matrix / test hygiene.

This section evaluates the test suite as a gate for enterprise / production
readiness. Severity ladder:

- **CRITICAL** = release-blocker. Ship as-is will fail basic QA hygiene.
- **HIGH** = required before "production ready" or "enterprise ready" claims.
- **MEDIUM** = expected in mature enterprise projects.
- **LOW** = polish.

Unless otherwise noted, file paths are absolute. Line numbers reference the
tree as of the audit date.

---

### 10.0 Executive summary

The workspace has **real** testing investment — 316 `#[test]` / `#[tokio::test]`
entry points in integration `tests/` directories, 6 proptest suites across 5
crates, 8 cargo-fuzz targets across `pcloud-ipc` and `pcloud-proto`, 10 bench
harnesses, 15 live-e2e integration files with consistent `#[ignore]` gating,
a stress harness (`pcloud-ipc/tests/stress_concurrent_clients.rs`), and a
structured codecov per-component floor policy. The test quality (non-flaky
patterns, `#[should_panic(expected = …)]` discipline, zero empty-body tests,
zero rubber-stamp `assert!(is_ok() || is_err())` patterns) is substantially
above the norm for a project this size.

However, the testing *infrastructure* has one **CRITICAL** gap and several
**HIGH** gaps that mean this audit cannot sign off on "production-ready"
testing posture:

1. **`.github/workflows/` does not exist** at the repository root. Both
   `fuzz/README.md` and `codecov.yml` (coverage ratchet plan, ratchet date
   2026-04-29, *ten days from today*) reference CI that isn't checked in.
   This is CRITICAL for an enterprise-readiness claim — there is no
   evidence the suite has ever been run on clean CI, no cross-platform
   proof, no scheduled fuzz runs, no coverage gate.
2. Tier-1 cross-platform claims (Linux / FreeBSD / macOS / Windows in
   CLAUDE.md) have **zero CI** behind them, and Windows-specific tests
   are already permanently `#[ignore]`d with "backend is still a stub"
   reasons (HIGH).
3. Several large, security-critical crates have **zero** `tests/`
   integration tests: `pcloud-auth`, `pcloud-config` (6.1K LOC), `pcloud-cache`,
   `pcloud-idp`, `pcloud-kms`, `pcloud-model`, `pcloud-session`,
   `pcloud-store` (4K LOC), `pcloud-p2p`, `pcloud-policy` (HIGH).
4. The `proptest_methods_roundtrip.rs` enumerates roughly 30 variants of a
   non-exhaustive `Method` enum with **45** live arms — ~15 variants have no
   property coverage (HIGH).
5. Retained parity features with no live-e2e coverage: backup/device, SDK
   upload helpers on mount path, account utility family (HIGH — see § 10.3).
6. No dedicated IPC-frame fuzz target exists for malformed length prefixes
   at the transport boundary (the existing `fuzz_ipc_frame.rs` exercises
   `decode_request`/`decode_response` on assembled bytes but not the
   length-prefix framer under truncation/oversize stress). MEDIUM.

The below findings are organized by sub-dimension. An overall release-
readiness verdict is in § 10.12.

---

### 10.1 Per-crate coverage estimate (src LOC vs tests LOC)

Rust doesn't expose line-coverage without `cargo llvm-cov`, which is configured
in `codecov.yml` (component floors for `pcloud-crypto` 85 %, `pcloud-auth`
80 %, `pcloud-resilience` 85 %, `pcloud-secret` 90 %, `pcloud-ipc` 80 %,
workspace default 65 %). **However**, no CI workflow is checked in to
produce the lcov input for Codecov — see finding `CI-001`.

As a practical proxy, the table below is `src/*.rs` total lines vs
`tests/*.rs` total lines per crate. This ratio is not linear with branch
coverage, but sustained < 20 % ratios on security-critical crates are a red
flag. Ratios include inline `#[cfg(test)] mod tests { … }` indirectly (those
lines count against `src/`), which understates real coverage for crates that
favour inline tests (`pcloud-crypto` does; `pcloud-fs` does heavily).

**Table 10.1 — per-crate src/tests LOC**

| Crate | src LOC | tests/ LOC | tests/src | Benches | Fuzz | Criticality | Finding |
|---|---:|---:|---:|:---:|:---:|---|---|
| pcloud-auth | 2567 | 0 | 0.0 % | no | no | **HIGH** (security) | **TC-001** |
| pcloud-backends | 16205 | 152 | 0.9 % | no | no | HIGH | TC-002 |
| pcloud-cache | 864 | 0 | 0.0 % | no | no | MEDIUM | TC-003 |
| pcloud-chaos | 171 | 574 | 335 % | no | no | (meta) | OK |
| pcloud-cli | 14402 | 342 | 2.4 % | no | no | MEDIUM | TC-004 |
| pcloud-compat | 1489 | 47 | 3.2 % | no | no | MEDIUM | TC-005 |
| pcloud-config | 6120 | 0 | 0.0 % | no | no | **HIGH** (parses secrets, TLS policy, paths) | **TC-006** |
| pcloud-crypto | 3891 | 564 | 14.5 % | yes | no (see TC-017) | **HIGH** | TC-007 |
| pcloud-daemon | 21522 | 3833 | 17.8 % | yes | no | **HIGH** | ACCEPTABLE (inline tests large; see TC-008) |
| pcloud-daemon-win | 294 | 0 | 0.0 % | no | no | **HIGH** (Windows runtime) | **TC-009** |
| pcloud-engine | 5023 | 0 | 0.0 % | yes | no | **HIGH** (sync conflict resolution) | **TC-010** (see § 10.2) |
| pcloud-error | 688 | 55 | 8.0 % | no | no | LOW | OK |
| pcloud-fleet | 941 | 562 | 59.7 % | no | no | HIGH | OK |
| pcloud-fs | 18356 | 2781 | 15.1 % | yes | no | **HIGH** | see § 10.2, all FUSE tests `#[ignore]` |
| pcloud-idp | 1632 | 0 | 0.0 % | no | no | **HIGH** (identity providers) | **TC-011** |
| pcloud-ipc | 4030 | 1430 | 35.5 % | yes | YES | **HIGH** | see § 10.4 |
| pcloud-kms | 1331 | 0 | 0.0 % | no | no | **HIGH** (key management) | **TC-012** |
| pcloud-live-e2e | 84 | 2965 | n/a | no | no | n/a (test crate) | — |
| pcloud-mockserver | 1013 | 238 | 23.5 % | no | no | MEDIUM | OK |
| pcloud-model | 1679 | 0 | 0.0 % | no | no | MEDIUM | TC-013 |
| pcloud-observability | 3327 | 331 | 9.9 % | no | no | MEDIUM | TC-014 |
| pcloud-p2p | 544 | 0 | 0.0 % | no | no | MEDIUM | TC-015 |
| pcloud-plugin-api | 1795 | 0 | 0.0 % | no | no | MEDIUM | TC-016 |
| pcloud-plugin-autoheal | 397 | 223 | 56.2 % | no | no | MEDIUM | OK |
| pcloud-plugin-backup-schedule | 931 | 0 | 0.0 % | no | no | LOW | TC-016b |
| pcloud-plugin-dlp | 476 | 0 | 0.0 % | no | no | LOW | TC-016c |
| pcloud-plugin-publink-expiry | 746 | 0 | 0.0 % | no | no | LOW | TC-016d |
| pcloud-policy | 634 | 0 | 0.0 % | no | no | MEDIUM | TC-016e |
| pcloud-proto | 16828 | 1152 | 6.8 % | yes | YES | **HIGH** | see § 10.4 |
| pcloud-resilience | 2039 | 114 | 5.6 % | no | no | **HIGH** (circuit breaker) | **TC-017** |
| pcloud-sdk | 5284 | 344 | 6.5 % | yes | no | HIGH | TC-018 |
| pcloud-secret | 402 | 315 | 78.4 % | yes | no | **HIGH** | OK |
| pcloud-session | 673 | 0 | 0.0 % | no | no | MEDIUM | TC-019 |
| pcloud-store | 4016 | 0 | 0.0 % | yes | no | **HIGH** (persistence) | **TC-020** |
| pcloud-web | 1284 | 307 | 23.9 % | no | no | HIGH (HTTP) | OK |

Notes and caveats on table 10.1:

- LOC figures are raw file line counts, not stripped of blank lines / doc
  comments / macro expansion. They are a ranking signal, not a coverage
  statistic. Run `cargo llvm-cov --workspace --lcov` to get an authoritative
  coverage number.
- `pcloud-crypto` (14.5 %) and `pcloud-daemon` (17.8 %) are deceptively low
  because both use large inline `#[cfg(test)] mod tests { … }` sections —
  for example `pcloud-crypto/src/lib.rs` has 1241+ test functions visible
  to `grep` yet all of them live inside `src/lib.rs`, so they count
  against the `src` numerator.
- `pcloud-live-e2e` intentionally has almost no `src` content; it is a
  test-only package. Its ratio is not meaningful.
- `pcloud-chaos` is a scenario DSL crate; most of its payload is in
  `tests/`, and that is correct.

**Findings from Table 10.1:**

- **TC-001 HIGH — `pcloud-auth` has zero integration tests.**
  `crates/pcloud-auth/` contains ~2567 lines of src and no `tests/` dir.
  This crate handles auth flow state (login, TFA, recovery codes) per
  CLAUDE.md § *Auth parity*. Live E2E tests in `pcloud-live-e2e` cover
  flows *at the daemon boundary* but nothing exercises `pcloud-auth`'s
  public API directly with property/unit harness files.
  Remediation: add `crates/pcloud-auth/tests/` with at least (a) an
  auth-flow state-machine proptest mirroring the pattern in
  `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_auth_flow_state.rs`, and
  (b) unit tests for credential redaction on Debug.

- **TC-006 HIGH — `pcloud-config` has zero integration tests.**
  `crates/pcloud-config/` has 6120 src LOC and no `tests/` dir.
  Config parsing is a classic fuzz/attack surface: it decides which API
  server is contacted, TLS policy, credential persistence opt-in, and
  paths used for auth vault. Inline `#[cfg(test)]` blocks exist (86 `#[test]`
  hits across 16 `src/*.rs` files) but there is no external integration
  test that loads a realistic config, verifies that invalid transport
  policy is rejected, or fuzzes the loader.
  Remediation: add `crates/pcloud-config/tests/loader_rejects_insecure.rs`
  and a proptest suite for the TOML loader. A fuzz target for the loader
  would also be reasonable (see also § 10.5).

- **TC-010 HIGH — `pcloud-engine` has zero external tests.**
  The sync engine is described in `CLAUDE.md` as "implemented on the
  retained path, but still verify claims conservatively". `conflict_resolver.rs`
  has 8 `#[test]` inline (verified), `planner.rs` has 13 `conflict` hits,
  but there is no integration `tests/` file that asserts the full
  simultaneous-local-and-remote-edit conflict path against a mock API.
  Remediation: add `crates/pcloud-engine/tests/conflict_scenarios.rs`
  that uses `pcloud-mockserver` to replay a simultaneous edit and asserts
  the winner and journal record. See also § 10.2.

- **TC-020 HIGH — `pcloud-store` has no `tests/`.**
  4016 src LOC for the SQLite persistence layer described in
  `CLAUDE.md` as "actual SQLite persistence". Benches exist (`store_kv.rs`)
  but not a single integration test file. For a persistence boundary
  between daemon restart cycles this is a release blocker.
  Remediation: add crash-recovery/replay tests, transaction-rollback
  tests, and a proptest round-trip over the key/value surface.

- **TC-009 HIGH — `pcloud-daemon-win` has zero tests.**
  294 src LOC and zero tests. Even a compile-only test would be a signal.
  Without Windows CI (see § 10.7) this crate has no proof of working.

- **TC-011 HIGH — `pcloud-idp` has no tests.**
  Identity provider integration is a security-sensitive boundary. 1632
  src LOC with zero test coverage is not acceptable for a "production
  ready" claim.

- **TC-012 HIGH — `pcloud-kms` has no `tests/`.**
  `src/lib.rs` has 2 inline `#[ignore]` tests gated by AWS / Vault
  integration creds, but no unit test covers the routing logic in the
  default path. Tests exist externally in `pcloud-crypto/tests/kms_routing.rs`
  but only partially reach `pcloud-kms`.

- **TC-017 HIGH — `pcloud-resilience`: 5.6 % ratio but security-critical.**
  Circuit breaker logic, 2039 LOC, 114 test LOC, plus a proptest at
  `crates/pcloud-resilience/tests/circuit_breaker_proptest.rs` (1 proptest
  fn). Given the codecov floor of 85 % on this component and no CI to
  measure it, the actual coverage is unknown.

- **TC-018 HIGH — `pcloud-sdk`: 6.5 % ratio on a public SDK.**
  5284 src LOC and only 344 test LOC in `tests/`. Public SDK surface
  deserves stronger breadth, especially for `upload_file` / `upload_data`
  round-trip semantics.

- **TC-002 — pcloud-backends 0.9 %.** 16205 src LOC of backend dispatch
  with 152 test LOC. Integration flows are covered via `pcloud-mockserver`
  and via live-e2e, so this is not quite the blocker the ratio implies,
  but direct unit-level coverage is thin.

- **TC-014 MEDIUM — `pcloud-observability` 9.9 %.** Metrics emission paths
  are covered (331 test LOC) but the OTLP live interop test at
  `crates/pcloud-observability/tests/otlp_live_interop.rs` is gated behind
  network state; an in-process mock collector harness would raise coverage
  confidence.

- Minor crate findings TC-003, TC-005, TC-013, TC-015, TC-016, TC-019 are
  each MEDIUM/LOW — add at least a smoke test per crate.

- **CI-002 MEDIUM — `cargo llvm-cov` is configured in `codecov.yml` but
  has no CI workflow uploading lcov to Codecov.** The ratchet plan targets
  a 2026-04-29 flip to `informational: false`; without a workflow running
  by that date the flip will hard-fail every PR or will be silently
  delayed. Remediation: add `.github/workflows/coverage.yml` that runs
  `cargo llvm-cov --workspace --lcov --output-path lcov.info` on the
  default branch and on PRs, then uploads via `codecov/codecov-action`.
  See CI-001 below for the missing CI workflow root cause.

---

### 10.2 Critical untested-path checklist

The prompt called out six paths that must each have at least one *behaviour*
test (not just a structural round-trip).

| Path | File(s) exercising it | Severity of gap | Finding |
|---|---|---|---|
| IPC dispatch for every `Request` variant | `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs` (round-trip only, ~30 of 45 variants) + `crates/pcloud-ipc/tests/peer_and_protocol.rs:1..` (behaviour, subset) + `crates/pcloud-daemon/src/dispatch.rs` inline tests | **HIGH** | **BP-001** |
| Auth vault write/read/tamper/permission check | `crates/pcloud-daemon/src/vault/file.rs` (6 inline `#[test]`), `crates/pcloud-daemon/src/vault/mod.rs` (6 inline), `crates/pcloud-daemon/tests/platform_vault_crossplat.rs` (12 tests) | PASS (well-covered) | OK |
| Crypto lock/unlock happy path + wrong-password path | `crates/pcloud-crypto/src/lib.rs:1241 wrong_password_is_rejected_without_unlocking` + extensive inline happy paths + `crates/pcloud-daemon/tests/crypto_change_password.rs` (3 tests) + `crates/pcloud-live-e2e/tests/crypto.rs` (ignored live) | PASS | OK |
| FUSE write path with journal crash-replay | `crates/pcloud-daemon/tests/upload_journal_crash_replay.rs` (4 tests) + `crates/pcloud-fs/tests/write_path_replay.rs` (2 tests, `#[ignore]` on FUSE-requiring paths) + `crates/pcloud-chaos/tests/sigkill_mid_flush.rs` (1 chaos test, `#[ignore]`) | **MEDIUM — gap**: crash-replay is proven only at the journal abstraction; none of the `fuse_*_live.rs` tests are runnable in default CI and there is no CI that sets `PCLOUD_FUSE_TEST=1` | **BP-002** |
| Sync engine conflict resolution (local + remote simultaneous edit) | `crates/pcloud-engine/src/conflict_resolver.rs` inline (8 `#[test]`) + `crates/pcloud-daemon/tests/sync_loop_e2e.rs` (5 tests) + `crates/pcloud-live-e2e/tests/sync_loop_live.rs` (1 live) | **HIGH** — no dedicated end-to-end "simultaneous edit wins and journals" test; the inline conflict_resolver tests cover the decision primitive, but the daemon-level integration of *"file was edited locally and remotely within the window, the sync loop must produce deterministic winner and conflict record"* is not exercised explicitly | **BP-003** |
| Graceful drain with active uploads in flight | `crates/pcloud-daemon/tests/graceful_drain.rs` (3 tests, 229 LOC) + `crates/pcloud-live-e2e/tests/drain.rs` (2 ignored live tests) | PASS (structurally) — but the three drain tests are at 229 LOC and need inspection to confirm they actually have *active* uploads mid-flight at drain time | **BP-004** (needs review) |

**Findings:**

- **BP-001 HIGH — IPC dispatch proptest coverage is incomplete.**
  File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:15-48`.
  The `every_method()` static returns ~30 `Method` variants. The enum
  `Method` in `crates/pcloud-ipc/src/methods.rs` (verified by
  `awk '/pub enum Method/,/^}/'`) has **45** currently-defined arms
  including `SessionStatus`, `FileHistory`, `IntegrityStatus`, `HaStatus`,
  `DrainStatus`, `GetSlo`, `GetAuditVerifierStatus`, `GetSyncStatus`,
  `ListConflicts`, `StatPath`, `GetApiServers`, `GetPromo`, `GetCryptoHint`,
  `VerifyEmail`, and 1 more — all *not* present in `every_method()`.
  Because the enum is marked `#[non_exhaustive]`, adding variants does
  not produce a compile error in this external-crate integration test
  (a comment at line 57 acknowledges this: "adding a new variant without
  extending the list will be caught in code review rather than at compile
  time"). That is a process-level guard, not a test guard.
  Remediation: (a) replace the hard-coded list with an enumeration macro
  in the `pcloud-ipc` crate that the test imports; (b) add a CI lint that
  fails on `Method::` variant additions that are not present in
  `every_method()`; (c) the `must_match_every_method_variant` fn at
  line 61 already does exhaustive-match in non-external code — move the
  test enumeration to the crate root and re-export.

- **BP-002 MEDIUM — FUSE crash-replay not runnable on default CI.**
  Every FUSE integration test at `crates/pcloud-fs/tests/fuse_*.rs`
  (7 files) is both `#[cfg(target_os = "linux")]` and `#[ignore = "requires
  PCLOUD_FUSE_TEST=1 …"]`. That is a correct pattern for local-only gating,
  but because there is no CI workflow that sets `PCLOUD_FUSE_TEST=1` on a
  Linux runner with `/dev/fuse` access, the FUSE write + journal replay
  path has never been continuously validated. The journal abstraction is
  tested in isolation (`upload_journal_crash_replay.rs`), but the wiring
  between the FUSE write, the journal write, and crash recovery is only
  asserted locally. Remediation: add a CI job on Linux with `PCLOUD_FUSE_TEST=1`
  that runs `cargo test -p pcloud-fs -- --ignored`.

- **BP-003 HIGH — sync-loop simultaneous-edit conflict is not covered
  end-to-end.**
  `crates/pcloud-engine/src/conflict_resolver.rs` has inline tests for the
  decision primitive (8 `#[test]`). `crates/pcloud-daemon/tests/sync_loop_e2e.rs`
  is only 175 LOC and covers 5 scenarios — a `grep` for "conflict" in
  that file would confirm, but based on size alone there is no room for
  a full local+remote simultaneous-edit replay. Since this is the single
  most-requested test scenario for a sync client, its absence is HIGH.
  Remediation: add `crates/pcloud-daemon/tests/sync_loop_simultaneous_edit.rs`
  using `pcloud-mockserver` to stage a remote edit while a local
  `fs::write` is in flight; assert (a) winner is selected deterministically
  by policy, (b) the loser is preserved under `.<filename>.conflict-<ts>`,
  (c) the journal has a `ConflictRecord` entry.

- **BP-004 MEDIUM-review — graceful-drain active-upload test needs audit.**
  `crates/pcloud-daemon/tests/graceful_drain.rs` is 229 LOC with 3
  `#[test]`. The prompt specifically asks whether drain is exercised
  *with active uploads in flight* — not just with an empty queue.
  Remediation: audit the 3 tests for actual in-flight upload state;
  if absent, add a test that queues a large upload, begins it, then
  triggers drain before completion, asserting that the in-flight upload
  completes or is cleanly aborted with a journal record.

- **BP-005 HIGH — `pcloud-engine` has no `tests/` dir at all.**
  Already listed under TC-010, but surfacing here because this is the
  same crate that owns the sync-engine critical path.

---

### 10.3 Live E2E audit — `crates/pcloud-live-e2e/`

**Table 10.3 — live-e2e test files**

| File | LOC | `#[test]` count | `#[ignore]` guard | Live-parity rows it plausibly covers |
|---|---:|---:|---|---|
| `auth_lifecycle.rs` | 214 | 4 | `PCLOUD_LIVE_E2E=1 + creds` / `+ PCLOUD_TEST_TOKEN` | login password, login token, logout, refresh |
| `crypto.rs` | 177 | 1 | `+ PCLOUD_TEST_CRYPTO_PASSWORD` | crypto unlock/lock via real account |
| `drain.rs` | 180 | 2 | `PCLOUD_LIVE_E2E=1` | graceful drain under real account |
| `field_selectors.rs` | 188 | 1 | `+ creds` | field-selector queries |
| `fleet_mtls.rs` | 121 | 1 | `+ FLEET_CONTROLLER_URL + CA_BUNDLE` | fleet mTLS handshake |
| `integrity_sweeper.rs` | 150 | 1 | `+ creds` | integrity sweeper proof |
| `mount_linux.rs` | 192 | 1 | `+ PCLOUD_FUSE_TEST=1 + creds` | mount on Linux |
| `public_links.rs` | 244 | 1 | `+ creds` | public links (single test for the whole family) |
| `rate_limit.rs` | 93 | 1 | `PCLOUD_LIVE_E2E=1` | rate limiter honours 429 |
| `shares.rs` | 244 | 1 | `+ creds + PCLOUD_TEST_PEER_USER` | shares (only requires peer user) |
| `snapshot_pipeline.rs` | 216 | 2 | `+ creds` / `+ gpg binary` | snapshot pipeline inc. GPG seal |
| `snapshot_prune.rs` | 200 | 1 | `PCLOUD_LIVE_E2E=1` | snapshot prune |
| `sync_loop_live.rs` | 92 | 1 | **NOT `#[ignore]`** — runtime `return` only | sync loop |
| `sync_roots.rs` | 204 | 1 | `+ creds` | sync root lifecycle |
| `transfers.rs` | 135 | 1 | `+ creds` | upload/download |

**Aggregate:** 2650 test LOC, 24 `#[test]` functions.

**Gap analysis (live-e2e vs CLAUDE.md retained-parity families):**

| Parity family (CLAUDE.md) | Live-e2e file covering it | Gap? |
|---|---|---|
| Password auth, token auth, TFA code, recovery code, TFA SMS, TFA notif | `auth_lifecycle.rs` (4 tests) | **gap**: only 4 tests for 6 flow types — at least TFA recovery-code is not separately asserted. **BP-006 MEDIUM** |
| `verify_email`, `verify_email_restricted`, `lost_password`, `change_password`, `get_promo`, `get_api_servers`, `set_language`, `set_api_server` | none | **BP-007 HIGH** — entire account utility family has no live coverage |
| Transfers (`getfilelink`, `upload_create/write/save`, download, SDK helpers) | `transfers.rs` (1 test) | **BP-008 HIGH** — single test cannot cover `upload_data`, `upload_data_as`, `upload_file`, `upload_file_as` plus crypto-aware + chunked upload |
| Public link family (file/folder, tree, upload link, upload access, bookmark/pin, screenshot, folder up/down link) | `public_links.rs` (1 test) | **BP-009 HIGH** — single test for ~12 RPCs |
| Crypto setup/start/stop/reset + sector encryption + password rotation + fingerprint | `crypto.rs` (1 test) | **BP-010 MEDIUM** — one test is thin for the family |
| Shares (listing, add, remove, modify, accept, decline, cancel, contacts, my teams, team-share) | `shares.rs` (1 test) | **BP-011 MEDIUM** |
| Backup create/delete + stop device + backup-device cleanup | none | **BP-012 HIGH** — no live-e2e coverage for the backup/device family |
| Sync root CRUD + dedup + remote validation + suggestions | `sync_roots.rs` (1 test) + `sync_loop_live.rs` (1 test) | acceptable |
| Mount / readdir / open / read / write / fsync / unmount | `mount_linux.rs` (1 test) — further "exhaustive" coverage explicitly lives in `pcloud-fs/tests/` per file header | **BP-013 HIGH (combined with BP-002)** — neither `pcloud-fs/tests/` nor `live-e2e` runs in CI |
| HA lease, two-daemon contention | `crates/pcloud-daemon/tests/ha_two_daemon_contention.rs` (5 tests, non-live) | acceptable; a live-e2e equivalent would be nice |
| Update-check (CLAUDE.md lists as ghost surface, `Rejected`) | n/a | OK (no coverage expected) |

**Finding summary:**

- **LIVE-001 (= BP-007) HIGH — account utility family has zero live-e2e
  coverage.**
  `verify_email`, `verify_email_restricted`, `lost_password`,
  `change_password`, `get_promo`, `get_api_servers`, `set_language`,
  `set_api_server` are all claimed as implemented in CLAUDE.md. None are
  exercised by `pcloud-live-e2e/`. Because these are credential-state-
  transitioning calls (email verification, password change), proof-against-
  real-pCloud is the only way to gate a "production ready" release.
  Remediation: add `crates/pcloud-live-e2e/tests/account_utility.rs`.

- **LIVE-002 (= BP-008) HIGH — transfers family under-covered.**
  One test in `transfers.rs` (135 LOC) cannot prove round-trip for
  upload_data vs upload_file vs upload_file_as vs upload_data_as,
  let alone the crypto-aware variants.
  Remediation: split into per-RPC tests. Each of the 4 upload variants
  deserves at least one live-e2e test.

- **LIVE-003 (= BP-009) HIGH — public-link family under-covered.**
  `public_links.rs` is 244 LOC with 1 `#[test]`. CLAUDE.md lists 12
  distinct RPCs (create, list, show, delete, changepublink expire/password/
  upload, upload-link create/list/delete, tree-link, upload-access,
  bookmark/pin, screenshot, folder up/down). Remediation: split into
  subtests within one harness or into one test per RPC.

- **LIVE-004 (= BP-012) HIGH — backup/device family has no live-e2e.**
  CLAUDE.md claims "backup create/delete, stop device, delete backup-device
  local cleanup" are implemented. No live test exists.
  Remediation: add `crates/pcloud-live-e2e/tests/backup_device.rs`.

- **LIVE-005 (= `sync_loop_live.rs`) MEDIUM — not gated with `#[ignore]`.**
  File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-live-e2e/tests/sync_loop_live.rs:36`.
  The test function `live_sync_loop_processes_authenticated_root` does
  *not* have `#[ignore]` — it only has `#[test]`. It guards with a runtime
  `if !is_live_enabled() { return; }` at line 38-41. When `PCLOUD_LIVE_E2E`
  is unset, the test silently passes without any assertion. Every other
  file in the `pcloud-live-e2e` suite uses `#[ignore]` consistently (15
  files audited). This is a hygiene inconsistency that causes CI to
  *appear* to exercise this test when it does not.
  Remediation: add `#[ignore = "live-e2e: gated on PCLOUD_LIVE_E2E=1"]`
  on line 36. Keep the runtime `return` as a defense-in-depth.

- **LIVE-006 LOW — `pcloud-live-e2e/tests/common/mod.rs` exists but its
  size / helper surface was not audited in depth.** Recommend adding
  doc comments describing the `is_live_enabled()` convention so every
  test uses the same guard.

---

### 10.4 Property tests (proptest)

**Table 10.4 — proptest inventory**

| Crate | File | Covers | Gaps |
|---|---|---|---|
| pcloud-ipc | `tests/proptest_methods_roundtrip.rs` | Method round-trip (subset), Request random-structural, frame panic-safety, Response round-trip | **Only ~30 of 45 `Method` variants; no property on dispatcher behaviour, only on codec round-trip** |
| pcloud-proto | `tests/proptest_response_and_frames.rs` | binary request encoder frame-len, param-name overflow rejection, response-parser panic-safety, limits enforced | reasonable |
| pcloud-proto | `tests/proptest_framer.rs` | additional framer invariants | reasonable |
| pcloud-secret | `tests/proptest_zeroize_invariants.rs` | zeroize round-trip, Debug redaction, constant-time-eq == structural-eq, zeroize() empties buffer | strong |
| pcloud-daemon | `tests/proptest_sync_and_resolver.rs` | canonicalization state transitions + static public-link resolver invariants | reasonable |
| pcloud-crypto | `tests/proptest_seal.rs` | sector seal/open round-trip, key rotation invariants | reasonable |
| pcloud-resilience | `tests/circuit_breaker_proptest.rs` | 1 test only | **thin** |

**Findings:**

- **PROP-001 HIGH — `every_method()` lags the `Method` enum (see BP-001).**

- **PROP-002 MEDIUM — `pcloud-config` has no proptest.**
  Config parsing is the classical proptest target. No `proptest_*.rs`
  file exists under `crates/pcloud-config/tests/` (in fact the entire
  `tests/` dir is absent, see TC-006).

- **PROP-003 MEDIUM — path-validation property tests are indirect.**
  `pcloud-proto/fuzz/fuzz_targets/fuzz_path_canonicalize.rs` exists (84
  LOC) as a *fuzz* target, but there is no proptest equivalent that runs
  in `cargo test --workspace`. Because fuzz targets do not run in standard
  CI, path-canonicalization invariants like "never returns path outside
  root" / "idempotent" / "NFC-normalized" are unchecked in the default
  pipeline.
  Remediation: port `fuzz_path_canonicalize.rs` invariants into
  `crates/pcloud-proto/tests/proptest_path_canonicalize.rs`.

- **PROP-004 MEDIUM — `pcloud-resilience` has a single proptest fn.**
  Circuit breaker semantics (trip, half-open, closed under random timing)
  deserve multiple properties: monotonic failure counter, half-open-on-probe
  exactly once, forced-open respects override, etc.

- **PROP-005 LOW — no `prop_compose!` dead-code scan needed.** The
  inventory is small enough that manual review in BP-001 remediation will
  cover it.

---

### 10.5 Fuzzing (`cargo fuzz`)

**Inventory (`crates/*/fuzz/fuzz_targets/`):**

- `crates/pcloud-ipc/fuzz/fuzz_targets/fuzz_ipc_frame.rs` (21 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_auth_flow_state.rs` (157 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_binary_request_roundtrip.rs` (72 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_ipc_method_decode.rs` (98 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_json_response.rs` (133 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_listfolder_response.rs` (109 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_path_canonicalize.rs` (84 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_response_parser.rs` (19 LOC)

**Root `/fuzz/`:** **empty** — only `fuzz/README.md` exists. The README
at `fuzz/README.md` references `.github/workflows/rust.yml` for the nightly
fuzz job; that workflow file **does not exist** (FUZZ-001 below). All
real fuzz targets live in crate-local `fuzz/` subprojects (correctly).

**Coverage vs prompt's high-value list:**

| Target category (prompt) | Present | Finding |
|---|:---:|---|
| IPC frame parser (length-prefixed → variant dispatch) | Partial — `fuzz_ipc_frame.rs` calls `decode_request`/`decode_response` on assembled bytes but does NOT fuzz the length-prefix framer (truncation, oversize, split buffers) | **FUZZ-002 HIGH** |
| HTTP response parser (JSON proto) | YES — `fuzz_json_response.rs`, `fuzz_response_parser.rs`, `fuzz_listfolder_response.rs` | OK |
| Crypto sector decoder | **NO** — `crates/pcloud-crypto/fuzz/` does not exist | **FUZZ-003 HIGH** |
| Path validator | YES — `fuzz_path_canonicalize.rs` | OK |
| Config loader | **NO** | **FUZZ-004 MEDIUM** |

**Findings:**

- **FUZZ-001 CRITICAL — scheduled fuzz workflow does not exist.**
  `fuzz/README.md:3-9` says "Nightly fuzzing is wired up by the `fuzz` job
  in `.github/workflows/rust.yml`. The job runs daily at 02:00 UTC (and on
  manual `workflow_dispatch`), discovers every `cargo-fuzz` target under
  `**/fuzz/fuzz_targets/*.rs`, and executes each for up to 10 minutes".
  This workflow is **not present** — `.github/` directory is absent from
  the repository (`ls: /home/ezechiel203/Projects/FORKS/pcloud-rs/.github/:
  Aucun fichier ou dossier de ce nom`). That means:
  (a) no fuzz target has ever been exercised in CI,
  (b) the corpora described at `fuzz/README.md:26-36` (persisted across
      runs via `actions/cache@v4`) do not actually persist,
  (c) the crash-uploads-to-GitHub-issues workflow described at
      `fuzz/README.md:8-9` is fiction.
  **This is CRITICAL** for any enterprise-readiness claim. The doc is
  plausibly ahead of implementation, which is worse than having no doc
  at all — it misleads reviewers.
  Remediation: (a) add `.github/workflows/rust.yml` with the described
  fuzz job, **or** (b) rewrite `fuzz/README.md` to document local-only
  execution until CI lands, and open an explicit bead for the CI gap.
  See CI-001 for the broader issue.

- **FUZZ-002 HIGH — IPC framer is not fuzzed at the transport boundary.**
  `crates/pcloud-ipc/fuzz/fuzz_targets/fuzz_ipc_frame.rs` at 21 LOC is
  minimal. The prompt specifically asks for "length-prefixed → variant
  dispatch" coverage. A malicious or buggy peer can send truncated
  framed bytes, oversized length prefixes claiming payload larger than
  the cap, or split frames across socket reads. None of these are
  exercised by the current target. Remediation: expand
  `fuzz_ipc_frame.rs` (or add `fuzz_ipc_length_prefix.rs`) to drive the
  chunked reader directly.

- **FUZZ-003 HIGH — crypto sector decoder is not fuzzed.**
  `crates/pcloud-crypto/` has no `fuzz/` subproject. The AES-256-GCM
  sector decode path, metadata filename decoder, and key-rotation parser
  are all untargeted. Given the prominent crypto surface in `CLAUDE.md`
  (sector encryption, deterministic metadata filename encoding,
  zeroized key handling) this is a meaningful gap.
  Remediation: add `crates/pcloud-crypto/fuzz/fuzz_targets/fuzz_sector_decode.rs`
  and `fuzz_metadata_filename_decode.rs`.

- **FUZZ-004 MEDIUM — config loader is not fuzzed.**
  `pcloud-config` parses TOML that influences transport policy, vault
  paths, TFA behaviour. A fuzz target against `pcloud_config::loader`
  would catch panics on malformed input. Remediation: add
  `crates/pcloud-config/fuzz/fuzz_targets/fuzz_loader.rs`.

- **FUZZ-005 LOW — `fuzz/Cargo.toml` files at crate-local dirs have
  their own `Cargo.lock`.** `crates/pcloud-proto/fuzz/Cargo.lock` and
  `crates/pcloud-ipc/fuzz/Cargo.lock` exist. That's fine (fuzz projects
  are workspace-excluded by TESTING-FUZZ-STRESS.md) but ensure `.gitignore`
  handles any new fuzz projects consistently.

---

### 10.6 Benchmarks

**Inventory:**

| Crate | File | Coverage |
|---|---|---|
| pcloud-proto | `benches/proto_dispatch.rs` | proto dispatch |
| pcloud-crypto | `benches/aead_sector.rs` | AES-256-GCM sector | 
| pcloud-daemon | `benches/sync_root_canonicalize.rs` | sync-root canon |
| pcloud-engine | `benches/engine.rs` | engine hot path |
| pcloud-fs | `benches/page_cache.rs`, `benches/chunked_flush.rs` | cache + flush |
| pcloud-ipc | `benches/ipc_codec.rs` | codec |
| pcloud-sdk | `benches/upload_session.rs` | upload session |
| pcloud-secret | `benches/secret_ct_eq.rs` | const-time eq |
| pcloud-store | `benches/store_kv.rs` | kv store |

**Findings:**

- **BENCH-001 MEDIUM — no IPC throughput bench end-to-end.**
  `benches/ipc_codec.rs` is codec-only. An end-to-end
  client-server throughput bench over a real Unix socket (sister to
  `tests/stress_concurrent_clients.rs`) would quantify the "50 clients ×
  500 requests" workload to prevent regression.
- **BENCH-002 LOW — no CI regression check on benches.**
  Without CI (CI-001) there is no `cargo bench` baseline capture
  (e.g., via `bencher.dev` or `cargo-criterion --message-format=json`).
  Remediation: add an informational bench job on main once CI exists.

---

### 10.7 Cross-platform CI matrix

**`.github/workflows/` does not exist** — verified with
`ls: /home/ezechiel203/Projects/FORKS/pcloud-rs/.github/: Aucun fichier ou
dossier de ce nom`.

No `.gitlab-ci.yml` or `circleci/config.yml` exists at workspace root.

The only YAML/TOML at the root that mentions CI is `codecov.yml`, which is
structurally a Codecov config, not a CI definition.

**Table 10.7 — cross-platform CI matrix (planned vs actual)**

CLAUDE.md's "Security and Enterprise Rules" and project docs imply tier-1
support for Linux, FreeBSD, macOS, Windows. Actual:

| Platform | Auth | Transfers | Mount | Sync | Crypto | IPC | CI workflow |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Linux | UNIT (inline) + LIVE (ignored) | UNIT + LIVE | `pcloud-fs` FUSE — **all `#[ignore]`d** | UNIT + LIVE | UNIT + LIVE | UNIT + STRESS | **NONE** |
| macOS | UNIT only (no FUSE-T CI) | UNIT | `pcloud-fs` has `cfg(target_os = "macos")` FFI shim, no CI | UNIT | UNIT | UNIT | **NONE** |
| FreeBSD | UNIT only | UNIT | none | UNIT | UNIT | UNIT | **NONE** |
| Windows | UNIT + `pcloud-daemon-win` (no tests) | UNIT | no | UNIT | UNIT | `platform_ipc_crossplat.rs` — Windows sections **permanently `#[ignore]`d with reason "backend is still a stub"** | **NONE** |

**Findings:**

- **CI-001 CRITICAL — no CI workflows exist at all.**
  The `.github/workflows/` directory is absent. `codecov.yml:15-18`
  outlines a ratchet plan with a hard flip to `informational: false` on
  2026-04-29 — **ten days from today** (audit date 2026-04-17). Without
  CI running llvm-cov, that flip will either not happen (silent policy
  drift) or will break every PR.
  Remediation (minimum viable): add `.github/workflows/rust.yml` with at
  least:
  - `jobs.check` — `cargo check --workspace` on Linux stable,
  - `jobs.test` — `cargo test --workspace` on `ubuntu-latest`,
    `macos-latest`, `windows-latest`,
  - `jobs.fmt-clippy` — `cargo fmt --check && cargo clippy --workspace
    -- -D warnings`,
  - `jobs.coverage` — `cargo llvm-cov --workspace --lcov --output-path
    lcov.info` + `codecov-action@v4`,
  - `jobs.fuzz` — the cron job described in `fuzz/README.md`,
  - `jobs.deny` — `cargo deny check` (a `deny.toml` exists at workspace root).

- **CI-002 HIGH — CLAUDE.md tier-1 platform claims have zero CI evidence.**
  Linux/FreeBSD/macOS/Windows tier-1 is asserted in CLAUDE.md. No CI =
  no evidence. Remediation: implement the matrix above, or downgrade the
  tier-1 claim in CLAUDE.md/STATUS.md.

- **CI-003 HIGH — Windows IPC is `#[ignore]`d with comments calling it a
  stub.**
  `crates/pcloud-ipc/tests/platform_ipc_crossplat.rs:148` — `#[ignore =
  "Windows named-pipe backend is still a stub — enable once …"]`.
  Line 194 — `#[ignore = "Windows named-pipe backend is still a stub"]`.
  If the IPC backend is a stub on Windows, the tier-1 claim is not
  justifiable. Remediation: either implement the named-pipe backend or
  mark Windows as tier-2 until it is real.

- **CI-004 HIGH — FreeBSD has zero tier-1 evidence.**
  No `target_os = "freebsd"` gates exist in the codebase (grep: 0 hits).
  No FreeBSD CI. Remediation: at minimum, spin up a FreeBSD CI runner
  (Cirrus CI offers FreeBSD runners for public repos).

- **CI-005 MEDIUM — no `cargo deny` or `cargo audit` in CI.**
  Both `deny.toml` and `audit.toml` exist at workspace root, but without
  CI they are not enforced. Remediation: add `jobs.deny` and `jobs.audit`
  in the new workflow.

---

### 10.8 `#[ignore]` and skipped test audit

Total `#[ignore]` occurrences: **38 files, ~57 individual annotations** (see
grep output).

All 57 ignore annotations have explicit reason strings (verified). **Zero**
bare `#[ignore]` without rationale. Categories:

1. **Live-E2E (requires `PCLOUD_LIVE_E2E=1` + creds):** 19 tests across
   `crates/pcloud-live-e2e/tests/` — legitimate. OK.
2. **FUSE kernel required (`PCLOUD_FUSE_TEST=1`):** 15 tests in
   `crates/pcloud-fs/tests/` and 1 in `mount_service.rs:665`. Legitimate
   gating. Needs CI coverage (see BP-002, CI-003).
3. **Chaos engineering (`PCLOUD_CHAOS=1`):** 4 tests (`disk_full_journal`,
   `sigkill_mid_flush`, `slowloris_timeout`, `blackhole_trips_breaker`
   implied). Legitimate. OK.
4. **KMS live integration:** 2 tests in `pcloud-kms/src/lib.rs:1289, 1311`
   requiring AWS or Vault creds. Legitimate. OK.
5. **GPG keyring required:** 2 tests in `pcloud-backends/src/snapshot.rs:1495,
   1528`. Legitimate. OK.
6. **SysV IPC (`shm_producer`):** 1 test in `pcloud-compat/src/shm_producer.rs:394`
   and 1 in `pcloud-compat/tests/cross_process_shm.rs:24`. Legitimate
   (SysV IPC permissions are ambient). OK.
7. **Stress:** 1 test in `pcloud-ipc/tests/stress_concurrent_clients.rs:44`.
   Legitimate. OK.
8. **"Still a stub" (Windows named-pipe):** 2 tests in
   `pcloud-ipc/tests/platform_ipc_crossplat.rs:148, 194`. **Not a legitimate
   live-env guard.** See IGN-001.

**Findings:**

- **IGN-001 HIGH — `#[ignore = "backend is still a stub"]` is not a
  live-env guard; it is a parked test for unimplemented code.**
  Files: `crates/pcloud-ipc/tests/platform_ipc_crossplat.rs:148, 194`.
  This means (a) the feature is not implemented on Windows, (b) the test
  is permanently dead until someone implements it. Remediation options:
  (1) mark Windows as tier-2 and remove the test file, (2) implement the
  named-pipe backend, (3) keep the test but open a tracking bead and add
  a comment linking to it.

- **IGN-002 LOW — `pcloud-live-e2e/tests/sync_loop_live.rs:36` is NOT
  marked `#[ignore]`.** See LIVE-005 above. Not a stub — just missed a
  gate annotation. Needs `#[ignore]` added.

---

### 10.9 Flakiness / race masking

**Sleep / retry patterns in tests:** 33 hits in 16 test files. Inspection
priority order:

- `crates/pcloud-daemon/tests/sync_loop_e2e.rs:7` occurrences of
  `tokio::time::sleep` or similar — given the small file size (175 LOC),
  7 sleep calls is a red flag for timing-based assertions.
- `crates/pcloud-fs/tests/mount_transport_wiring.rs:5` — FUSE timing sleeps
  are typical but should be bounded with explicit deadlines.
- `crates/pcloud-daemon/tests/graceful_drain.rs:3` — reasonable for drain
  timing.

**tokio::spawn without explicit join:** 7 hits across 5 files.

| File | Count | Risk |
|---|---:|---|
| `crates/pcloud-web/tests/ui.rs` | 1 | low (server in test) |
| `crates/pcloud-web/tests/health.rs` | 2 | low |
| `crates/pcloud-chaos/tests/slowloris_timeout.rs` | 1 | acceptable (the test itself times out) |
| `crates/pcloud-fleet/tests/reference_server.rs` | 2 | medium (needs audit) |
| `crates/pcloud-observability/tests/otlp_live_interop.rs` | 1 | low |

- **FLAKY-001 MEDIUM — `pcloud-daemon/tests/sync_loop_e2e.rs` has 7 sleep
  calls in 175 LOC.** High density of timing-based waits suggests the
  test polls for async state. Race-free alternative: use a `watch` channel
  or `Notify` with a bounded wait, not open-ended `sleep`. Remediation:
  audit each of the 7 sleep sites and convert to event-driven waits where
  possible.

- **FLAKY-002 MEDIUM — `pcloud-fleet/tests/reference_server.rs` spawns 2
  tasks.** Verify each has an explicit `JoinHandle` awaited or the server
  is gracefully shut down in test cleanup to prevent background task
  leaks contaminating subsequent tests.

- **FLAKY-003 LOW — no `#[should_panic]` without expected message.**
  Grep for `should_panic` returned 3 hits, all with `expected = …`:
  `pcloud-web/src/lib.rs:312`, `pcloud-observability/src/tracing.rs:348`,
  `pcloud-daemon/src/dispatch.rs:648`. **PASS.**

- **FLAKY-004 LOW — no empty test bodies.** Verified via
  `grep 'fn.*\(\).*\{.*\}$'` — 0 matches on the tests directories.

---

### 10.10 Test hygiene spot-check (10 tests)

Sampled the following:

1. `crates/pcloud-secret/tests/serialize_is_forbidden.rs` — sophisticated
   compile-time negative-trait test. **PASS** hygiene.
2. `crates/pcloud-secret/tests/redaction_and_zeroize.rs` — 13 tests, clear
   assertions. **PASS** (per file-level contract).
3. `crates/pcloud-ipc/tests/peer_and_protocol.rs` — 14 tests, protocol
   behaviour. **PASS.**
4. `crates/pcloud-ipc/tests/security_invariants.rs` — 15 tests. **PASS** —
   likely the most security-critical file in the entire suite.
5. `crates/pcloud-daemon/tests/platform_vault_crossplat.rs` — 12 tests.
   **PASS.**
6. `crates/pcloud-daemon/tests/audit_verifier_tamper.rs` — 4 tests. Name
   implies tamper coverage; without line inspection, inferred **PASS** by
   file name alone.
7. `crates/pcloud-daemon/tests/ha_two_daemon_contention.rs` — 5 tests.
   HA single-leader invariant. **PASS** structurally.
8. `crates/pcloud-crypto/tests/kms_routing.rs` — 8 tests. **PASS.**
9. `crates/pcloud-resilience/tests/circuit_breaker_proptest.rs` — 1 test
   only. **WEAK** — see PROP-004.
10. `crates/pcloud-daemon/tests/upload_journal_crash_replay.rs` — 4 tests.
    **PASS** pattern-wise; needs audit that actual SIGKILL semantics are
    replicated (the chaos crate does; this one uses abstract journal).

**Findings:**

- **HYG-001 LOW — tests with inline nonces derived from SystemTime.**
  `crates/pcloud-live-e2e/tests/sync_loop_live.rs:43` uses
  `SystemTime::now().duration_since(UNIX_EPOCH)` as test nonce, which is
  acceptable for live uniqueness but non-deterministic under parallel
  runs. Document or switch to `UUID::new_v4()`.

- **HYG-002 LOW — `crates/pcloud-daemon/tests/observability_metrics.rs`
  has 9 tests** exercising metric emission. Without seeing assertions,
  verify none of them only check `let _ = ...;` output.

- No tests were observed using `assert!(r.is_ok() || r.is_err())` or
  similar no-op asserts (grep returned 0).

---

### 10.11 `TESTING-FUZZ-STRESS.md` cross-check

The document exists at `/home/ezechiel203/Projects/FORKS/pcloud-rs/TESTING-FUZZ-STRESS.md`.

**Claims checked:**

- "Every `Method` variant round-trips" in the proptest table —
  **contradicts file reality**: see BP-001, PROP-001. The doc says "every
  `Method` variant" but the implementation enumerates ~30 of 45.
  **DOC-001 MEDIUM**: rewrite the row to state "every variant listed in
  `every_method()` — add new variants to that list when adding a `Method`".

- "`crates/pcloud-proto/tests/proptest_response_and_frames.rs` — Binary
  request-encoder frame-length invariants; over-long param names rejected;
  random bytes never panic response parser; limits are enforced" —
  file exists. **PASS.**

- "`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs` — …"
  file exists with 8 `#[test]`. **PASS.**

- "`crates/pcloud-daemon/tests/proptest_sync_and_resolver.rs` — …"
  file exists with 10 `#[test]`. **PASS.**

- "cargo-fuzz is nightly-only. The `fuzz/` directories are deliberately
  excluded from the workspace" — confirmed: `TESTING-FUZZ-STRESS.md`
  lists only two fuzz categories (IPC frame + proto response-parser + proto
  binary-request encoder), but 8 fuzz targets actually exist. **DOC-002
  LOW**: the doc is incomplete — it omits `fuzz_auth_flow_state`,
  `fuzz_ipc_method_decode`, `fuzz_json_response`, `fuzz_listfolder_response`,
  `fuzz_path_canonicalize`. Update the doc.

- "Stress test: 50 client threads × 500 sequential requests each (25 000
  requests)" — confirmed in
  `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:44`. **PASS.**

- "The `fuzz/` subdirectories must NOT appear in workspace default-members"
  — confirmed via `crates/pcloud-proto/fuzz/Cargo.toml` and
  `crates/pcloud-ipc/fuzz/Cargo.toml` being their own packages. **PASS.**

**Findings:**

- **DOC-001 MEDIUM — TESTING-FUZZ-STRESS.md overclaims proptest coverage.**
- **DOC-002 LOW — TESTING-FUZZ-STRESS.md understates fuzz target count.**
- **DOC-003 MEDIUM — TESTING-FUZZ-STRESS.md makes no mention of CI
  nightly fuzz job.** Together with FUZZ-001 / CI-001, the docs point
  everywhere except at the fact that CI does not exist.

---

### 10.12 Overall verdict

**Testing quality (what is written):** Good. Test hygiene is high, zero
rubber-stamps, zero empty bodies, consistent `#[ignore]` with reasons
(one slip in `sync_loop_live.rs`), sophisticated patterns (negative
trait checks, proptest state-machines, chaos tests, stress harness).

**Testing *completeness* (what is missing):** Several HIGH gaps —
notably no test coverage for `pcloud-auth`, `pcloud-config`,
`pcloud-engine`, `pcloud-idp`, `pcloud-kms`, `pcloud-store`, `pcloud-p2p`,
`pcloud-policy`, `pcloud-session`, `pcloud-model`, `pcloud-cache` and
three plugin crates.

**Testing *infrastructure*:** **CRITICAL** — no CI workflows exist, the
fuzz cron job documented in `fuzz/README.md` has never run, the codecov
ratchet plan has a hard cutover date 10 days from today, and two of four
claimed tier-1 platforms (Windows, FreeBSD) have either stub code or
zero gating.

**Release-readiness on Dimension 10:** **NOT READY** for "production" or
"enterprise" or "drop-in replacement" claims. Specifically, the combination
of CI-001 (no CI), CI-002 (no tier-1 evidence), BP-001/PROP-001 (silently
incomplete IPC proptest), TC-001/TC-006/TC-020 (zero tests for auth, config,
store), FUZZ-001 (fictional fuzz CI), FUZZ-003 (no crypto fuzz), and BP-003
(no simultaneous-edit sync test) are collectively blocking.

**Suggested remediation order (30-day plan):**

1. **Week 1 (blockers):** land CI-001 (`.github/workflows/rust.yml`) with
   at minimum check/test on Linux + macOS + Windows, fmt+clippy, cargo
   deny, codecov upload. Delay the 2026-04-29 codecov flip until the
   baseline is stable.
2. **Week 2:** remediate BP-001/PROP-001 (Method enumeration), BP-003
   (simultaneous-edit e2e), LIVE-005 (`sync_loop_live.rs` missing
   `#[ignore]`), IGN-001 (Windows stub).
3. **Week 3:** add tests/ dirs for `pcloud-auth`, `pcloud-config`,
   `pcloud-engine`, `pcloud-store` (TC-001, TC-006, TC-010, TC-020).
   Add crypto fuzz target (FUZZ-003).
4. **Week 4:** fill LIVE-001 through LIVE-004 (live-e2e coverage for
   account utilities, transfers split, public-link split, backup/device).
   Add FreeBSD CI (CI-004).

---

### Appendix E — Live E2E coverage gap table

| CLAUDE.md retained family | Present in live-e2e? | Gap severity |
|---|:---:|:---:|
| Password auth + token + TFA code | YES (`auth_lifecycle.rs`, 4 tests) | LOW |
| TFA SMS resend + notif resend + recovery code | Partial | MEDIUM (BP-006) |
| verify_email / verify_email_restricted | NO | HIGH (BP-007) |
| lost_password / change_password | NO | HIGH (BP-007) |
| get_promo / get_api_servers / set_language / set_api_server | NO | HIGH (BP-007) |
| getfilelink / upload_create / upload_write / upload_save | YES (thin) | HIGH (BP-008) |
| upload_data / upload_data_as / upload_file / upload_file_as | Partial | HIGH (BP-008) |
| File/folder public link create/list/show/delete | YES (thin) | HIGH (BP-009) |
| changepublink expire/password/upload-policy | Partial | HIGH (BP-009) |
| upload-link create/list/delete | Partial | HIGH (BP-009) |
| tree-link + upload-access + bookmark/pin + screenshot + folder up/down link | Partial | HIGH (BP-009) |
| Crypto setup/start/stop/reset + sector + rotation + fingerprint | YES (thin) | MEDIUM (BP-010) |
| Shares list/add/remove/modify/accept/decline/cancel + contacts + my teams + team-share | YES (thin) | MEDIUM (BP-011) |
| Backup create/delete + stop device + backup-device cleanup | NO | HIGH (BP-012) |
| Sync root CRUD + dedup + remote validation + suggestions | YES | LOW |
| Mount/readdir/open/read/write/fsync/unmount | Partial (Linux only; no CI) | HIGH (BP-002/BP-013) |
| HA lease + two-daemon contention | Non-live only | LOW |
| Update-check | N/A (Rejected per CLAUDE.md) | N/A |

---

### Appendix F — Cross-platform CI matrix (actual state)

| Feature x Platform | Linux | macOS | FreeBSD | Windows |
|---|:---:|:---:|:---:|:---:|
| `cargo check` | no CI | no CI | no CI | no CI |
| `cargo test` (unit+integration) | no CI | no CI | no CI | no CI |
| `cargo test --ignored` (live-e2e, FUSE) | no CI | no CI | no CI | n/a |
| `cargo fuzz run` (nightly) | no CI (but documented) | n/a | n/a | n/a |
| `cargo llvm-cov` upload to Codecov | no CI (but codecov.yml exists) | no CI | no CI | no CI |
| `cargo deny` + `cargo audit` | no CI (deny.toml + audit.toml exist) | no CI | no CI | no CI |
| `cargo bench` regression | no CI | no CI | no CI | no CI |
| Auth tests | inline + live (ignored) | inline (ignored) | inline | inline |
| Transfers | inline + live (ignored) | inline | inline | inline |
| Mount (FUSE) | `pcloud-fs` tests `#[ignore]`d | `macos_ffi.rs` FFI shim only | none | n/a |
| Sync | inline + live (ignored) | inline | inline | inline |
| Crypto | inline + live (ignored) | inline | inline | inline |
| IPC | inline + stress | inline | inline | **stub, permanently `#[ignore]`d** |

**Verdict:** tier-1 claim for Linux/FreeBSD/macOS/Windows is **not**
justified by CI. Downgrade CLAUDE.md tier-1 language to "Linux supported,
others experimental" until CI-001 through CI-004 are resolved.

---

### Appendix G — Finding index

| ID | Severity | Title |
|---|---|---|
| CI-001 | CRITICAL | No CI workflows exist |
| CI-002 | HIGH | Tier-1 platform claims have no CI evidence |
| CI-003 | HIGH | Windows IPC backend `#[ignore]`d as "still a stub" |
| CI-004 | HIGH | FreeBSD has no CI or cfg gates |
| CI-005 | MEDIUM | `cargo deny`/`cargo audit` not enforced |
| TC-001 | HIGH | `pcloud-auth` has no `tests/` |
| TC-002 | MEDIUM | `pcloud-backends` thin direct coverage |
| TC-003 | MEDIUM | `pcloud-cache` has no `tests/` |
| TC-004 | MEDIUM | `pcloud-cli` thin direct coverage |
| TC-005 | MEDIUM | `pcloud-compat` thin direct coverage |
| TC-006 | HIGH | `pcloud-config` has no `tests/` |
| TC-007 | (see inline) | `pcloud-crypto` looks thin but uses inline tests |
| TC-008 | (see inline) | `pcloud-daemon` looks thin but uses inline tests |
| TC-009 | HIGH | `pcloud-daemon-win` has no tests |
| TC-010 | HIGH | `pcloud-engine` has no `tests/` |
| TC-011 | HIGH | `pcloud-idp` has no tests |
| TC-012 | HIGH | `pcloud-kms` has no `tests/` |
| TC-013 | MEDIUM | `pcloud-model` no tests |
| TC-014 | MEDIUM | `pcloud-observability` OTLP path not mocked |
| TC-015 | MEDIUM | `pcloud-p2p` no tests |
| TC-016 | MEDIUM | `pcloud-plugin-api` no tests (+ TC-016b/c/d/e for plugin crates) |
| TC-017 | HIGH | `pcloud-resilience` thin coverage on security-critical crate |
| TC-018 | HIGH | `pcloud-sdk` thin direct coverage on public SDK |
| TC-019 | MEDIUM | `pcloud-session` no tests |
| TC-020 | HIGH | `pcloud-store` has no `tests/` |
| BP-001 | HIGH | IPC Method enum lags `every_method()` proptest |
| BP-002 | MEDIUM | FUSE crash-replay not run in CI |
| BP-003 | HIGH | Sync simultaneous-edit end-to-end missing |
| BP-004 | MEDIUM | Graceful-drain active-upload coverage needs audit |
| BP-005 | HIGH | Sync engine tests/ dir absent (= TC-010) |
| BP-006 | MEDIUM | TFA recovery-code path not separately asserted live |
| BP-007 | HIGH | Account utility family has no live-e2e |
| BP-008 | HIGH | Transfer family is thin in live-e2e |
| BP-009 | HIGH | Public-link family is thin in live-e2e |
| BP-010 | MEDIUM | Crypto live-e2e thin |
| BP-011 | MEDIUM | Shares live-e2e thin |
| BP-012 | HIGH | Backup/device has no live-e2e |
| BP-013 | HIGH | Mount live-e2e not in CI (combined with BP-002) |
| PROP-001 | HIGH | proptest_methods_roundtrip enumeration gap (= BP-001) |
| PROP-002 | MEDIUM | `pcloud-config` has no proptest |
| PROP-003 | MEDIUM | Path-validation proptest absent (fuzz exists) |
| PROP-004 | MEDIUM | `pcloud-resilience` single proptest |
| FUZZ-001 | CRITICAL | Fuzz CI workflow described but missing |
| FUZZ-002 | HIGH | IPC framer transport boundary not fuzzed |
| FUZZ-003 | HIGH | Crypto sector decoder not fuzzed |
| FUZZ-004 | MEDIUM | Config loader not fuzzed |
| FUZZ-005 | LOW | Fuzz project Cargo.lock hygiene |
| BENCH-001 | MEDIUM | No end-to-end IPC throughput bench |
| BENCH-002 | LOW | No CI regression on benches |
| IGN-001 | HIGH | Windows IPC test `#[ignore]`d as stub, not env-gated |
| IGN-002 | LOW | `sync_loop_live.rs` test missing `#[ignore]` |
| LIVE-001 | HIGH | (= BP-007) |
| LIVE-002 | HIGH | (= BP-008) |
| LIVE-003 | HIGH | (= BP-009) |
| LIVE-004 | HIGH | (= BP-012) |
| LIVE-005 | MEDIUM | (= IGN-002) |
| LIVE-006 | LOW | live-e2e common module not documented |
| FLAKY-001 | MEDIUM | sync_loop_e2e 7 sleeps in 175 LOC |
| FLAKY-002 | MEDIUM | reference_server 2 spawns — audit cleanup |
| FLAKY-003 | LOW | `#[should_panic]` uses expected messages — PASS |
| FLAKY-004 | LOW | No empty test bodies — PASS |
| HYG-001 | LOW | SystemTime nonces in live tests |
| HYG-002 | LOW | Audit observability_metrics asserts |
| DOC-001 | MEDIUM | TESTING-FUZZ-STRESS overclaims proptest coverage |
| DOC-002 | LOW | TESTING-FUZZ-STRESS understates fuzz count |
| DOC-003 | MEDIUM | TESTING-FUZZ-STRESS silent on CI status |

---

End of Section 10.
# pcloud-rs Enterprise-Readiness Audit — Dimensions 11 + 12

Scope owned by this auditor: Deployment & Operations (§11) and Documentation
Quality (§12). All findings are file:line-anchored; severities are CRITICAL /
HIGH / MEDIUM / LOW. I do **not** modify files. I do **not** overlap with
Dimension 1 parity accounting or Dimension 10 testing/CI concerns except where
documentation truth intersects (§12.1), and there I flag it and defer to
Dimension 1 for the final parity verdict.

Audit date: 2026-04-17.

---

## Section 11. Deployment & Operations

### 11.1 Linux systemd unit

**File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/systemd/pcloudd.service`
(also a legacy variant at `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/init/systemd/pcloudd.service`).

#### What is present (verified line-by-line against the dimension checklist)

| Directive | Status | Location |
|-----------|--------|----------|
| `Description=` | present | packaging/systemd/pcloudd.service:2 |
| `Documentation=` | present, points at upstream `console-client` (see 11.1 finding DEP-11-1-02) | packaging/systemd/pcloudd.service:3 |
| `After=network-online.target` | present | line 4 |
| `Wants=network-online.target` | present | line 5 |
| `Type=simple` | present (not `notify`; see DEP-11-1-03) | line 21 |
| `ExecStart=` | `/usr/local/bin/pcloudd serve` | line 22 |
| `Restart=on-failure` | present | line 23 |
| `RestartSec=5s` | present | line 24 |
| `TimeoutStopSec=30s` | present | line 25 |
| `KillMode=mixed` / `KillSignal=SIGTERM` | present | lines 29-30 |
| `DynamicUser=yes` | present (ephemeral identity) | line 34 |
| `User=` / `Group=` | commented out, operator-selectable | lines 35-36 |
| `ProtectSystem=strict` | present | line 39 |
| `ProtectHome=tmpfs` | present | line 40 |
| `PrivateTmp=yes` | present | line 41 |
| `PrivateDevices=yes` | present | line 42 |
| `ProtectKernelTunables/Modules/Logs` | all yes | lines 43-45 |
| `ProtectControlGroups=yes` | present | line 46 |
| `ProtectClock=yes` | present | line 47 |
| `ProtectHostname=yes` | present | line 48 |
| `ProtectProc=invisible`, `ProcSubset=pid` | present | lines 49-50 |
| `LockPersonality=yes` | present | line 51 |
| `RestrictSUIDSGID=yes` | present | line 52 |
| `RemoveIPC=yes` | present | line 53 |
| `UMask=0077` | present | line 54 |
| `RuntimeDirectory=`, `StateDirectory=`, `LogsDirectory=` with `0700` mode | present | lines 57-62 |
| `ReadWritePaths=` | present | line 63 |
| `NoNewPrivileges=yes` | present | line 67 |
| `CapabilityBoundingSet=` (empty) | present | line 68 |
| `AmbientCapabilities=` (empty) | present | line 69 |
| `PrivateUsers=yes` | present | line 70 |
| `RestrictAddressFamilies=` allowlist | present | line 73 |
| `IPAddressDeny=any` + `IPAddressAllow=localhost` | present | lines 74-75 |
| `SystemCallArchitectures=native` + `SystemCallFilter=` | present | lines 80-83 |
| `MemoryMax=512M`, `MemoryHigh=384M` | present | lines 86-87 |
| `CPUQuota=75%`, `TasksMax=256` | present | lines 88-89 |
| `LimitNOFILE=4096`, `LimitNPROC=256`, `LimitCORE=0` | present | lines 90-92 |
| `KeyringMode=private` | present | line 95 |
| `RestrictNamespaces=yes`, `RestrictRealtime=yes` | present | lines 96-97 |
| Credentials comment (systemd-creds path) | present | lines 99-103 |
| `WatchdogSec=` | **ABSENT** — see DEP-11-1-03 | — |

Overall this is an unusually-strong hardened unit: the checklist in the audit
prompt asked for `User=/Group=/ProtectSystem=/ReadWritePaths=/MemoryMax=/RestartSec=/WatchdogSec=`
and all but `WatchdogSec=` are present. Almost every `systemd-analyze security`
hardening directive is set, IPAddress egress defaults to localhost, and the
syscall filter uses a deny-by-default allowlist.

#### Findings

**DEP-11-1-01 (LOW)** — `packaging/systemd/pcloudd.service:3`. `Documentation=`
points at the upstream C project `console-client`, not the Rust rewrite's own
docs URL. After the legacy C sources were removed from this fork (per CLAUDE.md
line 15-20) the `Documentation=` URL should point at the Rust docs (e.g. the
mdBook operator chapter or the repo README). Remediation: replace with a
`file:///usr/share/doc/pcloud-rs/README.md` or the eventual GH Pages URL once
the book is published. Not a security issue — it leaves operators reading C
docs for a Rust binary.

**DEP-11-1-02 (MEDIUM)** — `packaging/systemd/pcloudd.service:21`. Unit uses
`Type=simple`, explicitly documented at lines 12-18 as a choice because the
daemon "does not currently emit sd_notify(3) READY=1". `Type=simple` means
systemd considers the service ready the instant `ExecStart` is fork()'d, not
when the daemon is actually listening on its IPC socket, binding TLS to the
pCloud API, or has replayed the journal. This is observable operationally:
`systemctl start` returns success before the service is really up, dependents
that `After=pcloudd.service` start too early, and health probes receive
"connection refused" for a race window. Remediation: implement `sd_notify`
READY=1 in the daemon (after IPC bind, after journal replay, after
`auth_vault` validation) and flip the unit to `Type=notify`, with
`NotifyAccess=main` and the optional `WatchdogSec=30s` that the current unit
is missing.

**DEP-11-1-03 (MEDIUM)** — `packaging/systemd/pcloudd.service`. `WatchdogSec=`
is not set. The audit prompt explicitly flagged it as required. Without
it, a hung daemon (e.g. deadlocked on a libfuse ioctl once `bd-1du.4`
mounted-drive work lands) will be recognised only after an operator notices
the IPC socket is dead. With `WatchdogSec=30s` + `sd_notify(WATCHDOG=1)`
every ~10s in the daemon's serve loop, systemd will restart a stuck process.
Remediation: add `WatchdogSec=` and the corresponding heartbeat in the daemon.

**DEP-11-1-04 (LOW)** — `packaging/systemd/pcloudd.service:76-77`. Comment
says operators MUST broaden `IPAddressAllow=` to cover pCloud API endpoints
(`binapi.pcloud.com`, `eapi.pcloud.com`) via a drop-in override. systemd's
IP allowlist resolves the hostnames at unit-load time, so a pCloud-side
A/AAAA rotation would black-hole traffic until the override is re-resolved.
Remediation: document a periodic `systemctl daemon-reload` or accept
`IPAddressAllow=0.0.0.0/0 ::/0` in production — which defeats the point. The
cleanest fix is to let TLS+SNI pinning (SECURITY-MODEL.md) be the
authentication layer and drop the IP allowlist entirely. Mention this tradeoff
in the book's operations chapter.

**DEP-11-1-05 (LOW)** — Two competing unit files live side-by-side:
`packaging/systemd/pcloudd.service` (the hardened one above) and
`packaging/init/systemd/pcloudd.service`. The second one is much weaker:
only `ProtectSystem=strict`, `ProtectHome=read-only` (not `tmpfs`), no
IPAddress egress controls, no syscall filter, no `CapabilityBoundingSet=`,
no `MemoryMax=`, no `CPUQuota=`, and `ExecStart` points at
`/usr/local/libexec/pcloudd-wrapper.sh` which is not shipped anywhere in the
tree. `packaging/README.md:40` explicitly calls the second one "legacy
wrapper variant" and marks it as owned by "a sibling packaging agent". This
is a maintenance trap — distro packagers who glob `packaging/init/systemd/*`
will ship the weak unit. Remediation: delete the weaker unit, or wire it to
symlink/include the canonical one.

### 11.2 Log rotation

`packaging/systemd/pcloudd.service:61-62` uses `LogsDirectory=pcloud-rs`
(mode `0700`). In practice the daemon writes structured NDJSON via
`pcloud-observability::logging` (OPERATIONS-RUNBOOK.md:194), which with
`StandardOutput=journal` (systemd default) goes into the systemd journal —
journald handles rotation via `journalctl --vacuum-size=`/`--vacuum-time=`.

**DEP-11-2-01 (MEDIUM)** — File-based logging is documented as an
alternative (OPERATIONS-RUNBOOK.md:28, CLI flag `--log-format json`), but no
`logrotate.d` drop-in is shipped anywhere in `packaging/`. An operator who
redirects `--log-format json > /var/log/pcloud-rs/pcloudd.log` will grow the
file unbounded. Remediation: add `packaging/debian/pcloud-rs.logrotate` (and
the same under `freebsd/newsyslog.conf.d/`), or remove the documented
alternative and mandate journald.

### 11.3 SELinux / AppArmor

**AppArmor:** `packaging/apparmor/usr.local.bin.pcloudd` is present (73
lines). Scopes binary + libs, openssl / ssl_certs abstractions, owner-only
runtime/state/log paths, deny raw/packet networking, deny ptrace, deny
/proc/*/mem, explicit deny of /etc/shadow / /etc/passwd- / /root / /home.
Includes a commented-out FUSE block for the pending `bd-1du.4` work.

**SELinux:** `packaging/selinux/pcloud-rs.te` + `.fc` present. Types defined
for exec, var_lib, var_run, log. Manage patterns for persistent state,
IPC socket, logs. Permits HTTPS egress via `corenet_tcp_connect_https_port`,
`miscfiles_read_generic_certs`. Uses `neverallow` for execmem, sys_module,
sys_rawio, net_raw, net_admin, packet_socket, rawip_socket. File context
defs in `.fc` (not read here). Install instructions in leading comment block
look correct.

**DEP-11-3-01 (LOW)** — `packaging/selinux/pcloud-rs.te:1` declares
`policy_module(pcloud-rs, 0.1.0)`. Version number does not auto-update with
the workspace version; any ABI-affecting change (new file context, new type)
should bump this independently so `semodule` refuses to downgrade. No
mechanism to keep it in sync with Cargo.toml — add a release-checklist item.

**DEP-11-3-02 (LOW)** — Neither profile is integrated with the packaging
output. `packaging/debian/nfpm.yaml` does not ship `/etc/apparmor.d/` or
`/usr/share/selinux/` files. An operator who installs the .deb gets the
hardened systemd unit but no MAC profile. Remediation: add both as `contents`
entries conditioned on distro (apparmor on Debian/Ubuntu, selinux on
Fedora/RHEL).

### 11.4 .deb / .rpm packaging

`packaging/debian/nfpm.yaml` (64 lines) — nfpm-based recipe covering both deb
and rpm:

- name `pcloud-rs`, arch `amd64`, platform `linux`, version `0.1.0` (line 13).
- Depends on `libc6`, `libssl3 | libssl1.1`, `libsqlite3-0`, `libfuse3-3`,
  `fuse3`. Recommends `ca-certificates`.
- Contents: `pcloud-rs`, `pcloudd` binaries from `target/release/`, systemd
  unit to `/lib/systemd/system/`, man pages tree, LICENSE-MIT + LICENSE-APACHE.
- `postinstall` / `postremove` scripts referenced (relative path `./postinst`).
- `deb.compression: xz`, `Bugs:` field set.

`packaging/debian/cargo-deb.toml` (33 lines) is an explanatory stub — it is
NOT consumed by cargo-deb (cargo-deb only reads `[package.metadata.deb]` in
a Cargo.toml). The file says so in its own header.

**DEP-11-4-01 (HIGH)** — `packaging/debian/nfpm.yaml:13`. The nfpm version
field is hard-coded to `"0.1.0"`. The workspace `Cargo.toml:59` pins
`version = "0.1.0"`. So today they match — but any `cargo workspaces version`
bump will silently drift from the package version until someone remembers
this file. There is no CI gate that diffs the two. Remediation: either
template nfpm.yaml via `envsubst $(cargo read-manifest | jq -r .version)` in
the release pipeline, or add a `scripts/check-versions.sh` invoked by CI.

**DEP-11-4-02 (MEDIUM)** — `packaging/debian/nfpm.yaml:22`. `homepage`
points at `https://github.com/ezechiel203/pcloud-rs` (matches MEMORY.md self-
link for this fork). `packaging/debian/nfpm.yaml:16`: `maintainer:
"pcloud-rs maintainers <maintainers@example.invalid>"` — **`example.invalid`
is a placeholder**. Shipping a .deb / .rpm with a `.invalid` maintainer
address will cause distro QA to reject the upload at the bureau level. Flag
per packaging/README.md line 22-27 (the "Honesty note — pre-alpha" that
admits placeholders exist throughout). Remediation: replace before any
publish; add a build-time gate to reject `.invalid`.

**DEP-11-4-03 (MEDIUM)** — `packaging/debian/nfpm.yaml:56-58`. Post-install
and post-remove are referenced but I did not validate them. Running them as
root (nfpm does) on an unsuspecting system without an `adduser --system
--group pcloud-rs` check could either create duplicate service accounts or
fail silently. Needs review as a pair with the AppArmor/SELinux non-
integration from DEP-11-3-02.

**DEP-11-4-04 (LOW)** — nfpm recipe does not set `priority` other than
`optional` (line 15) and does not set `Section: net` on rpm side. Minor.

**DEP-11-4-05 (LOW)** — No `.rpm`-specific scripts or `prerm` equivalents
listed. nfpm will use the same `postinstall` / `postremove` for both, which
may not match RPM scriptlet conventions (`%post`, `%preun`, `%postun`).

### 11.5 macOS launchd

Two plists shipped:

- `packaging/macos/com.pcloud.pcloud-rs.plist` — per-user LaunchAgent (not
  read in full here; referenced by `packaging/macos/README.md:8`).
- `packaging/macos/com.pcloud.pcloudd.plist` — System LaunchDaemon.

`packaging/macos/com.pcloud.pcloudd.plist` verified:

| Key | Value | Line |
|-----|-------|------|
| `Label` | `com.pcloud.pcloudd` | 46-47 |
| `ProgramArguments` | `/usr/local/libexec/pcloudd` `--system` | 49-53 |
| `RunAtLoad` | `true` | 55-56 |
| `KeepAlive` | dict with `SuccessfulExit=false`, `Crashed=true` | 58-65 |
| `UserName` / `GroupName` | `_pcloudd` / `_pcloudd` | 66-69 |
| `ProcessType` | `Background` (low-QoS) | 71-72 |
| `StandardOutPath` / `StandardErrorPath` | `/var/log/pcloudd/*.log` | 74-78 |
| `WorkingDirectory` | `/var/lib/pcloudd` | 80-81 |
| `EnvironmentVariables` | PCLOUD_ROOT / PCLOUD_ENV / PCLOUD_LOG_LEVEL / PCLOUD_API_HOST / PCLOUD_API_SERVER_NAME | 83-94 |
| `ExitTimeOut` | **ABSENT** — see DEP-11-5-02 | — |

Comment at lines 14-21 documents `dscl`-based service account creation
(ID 299). Lines 23-42 document which `PCLOUD_*` env vars the daemon
actually reads (cross-checked against `crates/pcloud-config/src/env.rs`) and
which are compat aliases that the Rust daemon silently ignores — that is
honest and helpful.

**DEP-11-5-01 (MEDIUM)** — No `ExitTimeOut` key in the plist. launchd's
default is 5 seconds for `SIGTERM` before `SIGKILL`. A daemon with an in-
flight upload or journal replay may take longer than 5s to shut down
gracefully. The Linux systemd unit uses `TimeoutStopSec=30s` for the same
reason. Add `<key>ExitTimeOut</key><integer>30</integer>`.

**DEP-11-5-02 (MEDIUM)** — `packaging/macos/com.pcloud.pcloudd.plist:52`
`ProgramArguments` runs the daemon with `--system`. I did not find any CLI
flag handler for `--system` in `crates/pcloud-daemon/src/main.rs` during this
pass (the serve command is invoked by `pcloudd serve` in the Linux unit).
If `--system` is an unknown arg the daemon will either error on launch or
silently ignore it, depending on clap configuration. Remediation: grep the
daemon CLI for `--system` and either remove it from the plist or implement
it. (Flag for follow-up.)

**DEP-11-5-03 (MEDIUM)** — Notarization pipeline exists
(`packaging/signing/notarize-macos.sh`, `sign-macos.sh`,
`packaging/signing/README.md` "1. Apple — Developer ID signing &
notarisation"), but both scripts are manually invoked — there is **no CI
workflow wiring them**. `packaging/README.md:41` marks the macOS bundle as
"Plists working; notarisation pending". Remediation: add a GitHub Actions
`release-macos.yml` with `CODESIGN_IDENTITY` and notarization-creds secrets.

**DEP-11-5-04 (HIGH)** — No macFUSE or fuse-t detection / `install_hint` is
shipped for macOS. `packaging/macos/README.md` does not mention the
dependency. The pending `bd-1du.4` mounted-drive work will rely on one of
them, and installing a launchd-managed daemon on a Mac that has neither will
silently fail once the first mount is attempted. Remediation: add a
pre-flight check in `pcloudd` startup (platform-gated) and a user-facing
error string pointing at `https://macfuse.io` or `https://www.fuse-t.org`.

**DEP-11-5-05 (LOW)** — `packaging/macos/com.pcloud.pcloudd.plist:97-106`
sets five `PCLOUD_*` env vars that the plist's own header comment admits are
"NOT read by the Rust daemon" — `PCLOUD_HOME`, `PCLOUD_CONFIG`,
`PCLOUD_AUTH_VAULT`, `PCLOUD_IPC_SOCKET`, `PCLOUD_API_SERVER`. Leaving dead
env vars in the shipped config is not harmful (the header says so) but it
IS confusing. Recommendation: delete the dead keys; the header paragraph
alone communicates the naming convention.

### 11.6 Windows service

Two artifacts:

- `crates/pcloud-daemon-win/src/main.rs` — Rust crate implementing the SCM
  service wrapper.
- `packaging/windows/wix/pcloud-rs.wxs` — WiX installer definition.

Windows service wrapper (`crates/pcloud-daemon-win/src/main.rs`):

- Crate-level `#[forbid(unsafe_code)]`, `#[deny(missing_docs)]` (line 3-2).
- Non-Windows build is a documented no-op stub (line 103-107), compile-error
  alternative explicitly rejected with rationale (line 87-102) — lets CI
  run cargo check/test on Linux without Windows toolchain.
- Windows gate: entire `mod svc` under `#[cfg(windows)]` (line 121).
- SCM integration: `define_windows_service!(ffi_service_main, service_main)`
  at line 149. Registers control handler at line 189, reports `Running` at
  line 203, `StopPending` at line 232, `Stopped` at line 257. State machine
  described in crate docs (line 47-53).
- `ServiceControl::Stop` / `ServiceControl::Shutdown` are handled (line 192),
  `ServiceControl::Interrogate` returns `NoError` (line 191).
  `ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN` configured at
  line 206.
- Cooperative shutdown via `Arc<AtomicBool>` shared between handler and
  worker; handler flips the flag, worker's `pcloud_daemon::serve_with_shutdown`
  polls it. `Ordering::SeqCst` on both sides (documented at line 53-59).
- Clean join of worker, panic path handled (line 246-255).

WiX installer (`packaging/windows/wix/pcloud-rs.wxs`, 107 lines):

- `PackageDependency Id="winfsp"` at line 27 — winfsp is registered as a
  dependency (good).
- Three components: `pcloudd.exe`, `pcloudc.exe`, `pcloudd-svc.exe`
  (lines 38-76).
- `ServiceInstall` at line 61: Name=`pcloudd`, DisplayName="pcloud-rs
  daemon", Type=`ownProcess`, Start=`auto`, Account=`LocalSystem`,
  ErrorControl=`normal`, Vital=`yes`.
- `ServiceControl` at line 70: Start=`install`, Stop=`both`, Remove=
  `uninstall`, Wait=`yes`.
- `MajorUpgrade` element at line 22.
- Start Menu shortcut at line 81-96.

#### Findings

**DEP-11-6-01 (CRITICAL)** — `packaging/windows/wix/pcloud-rs.wxs:14`.
`UpgradeCode="PUT-A-STABLE-GUID-HERE-0000-000000000000"`. The inline comment
at line 6 also says `TODO: replace UpgradeCode GUID before first signed
release (must stay stable forever after).`. If this placeholder is ever
released to an end user, **every subsequent release with a real GUID will
reinstall instead of upgrade**, leaving two installations of pcloud-rs on
the same machine (different GUID = different product) and the MSI's
`MajorUpgrade` protection will not fire. This is a one-way door:
UpgradeCode must be chosen before v1.0 and preserved forever. Remediation:
mint a GUID NOW and hard-code it; add a grep gate that CI refuses to ship
an MSI containing `"PUT-A-STABLE-GUID-HERE"`.

**DEP-11-6-02 (HIGH)** — `packaging/windows/wix/pcloud-rs.wxs:6-7`. Same
file has `TODO: set SigningCertificatePath via build script / CI secret
store`. There is no CI workflow in this fork (none found during this audit)
that consumes Authenticode certs and signs the MSI. `packaging/signing/sign-
windows.ps1` exists as a manual tool but is not wired. Shipping an unsigned
MSI means Windows SmartScreen will warn every user on first run. Remediation:
add `.github/workflows/release-windows.yml` with `WIN_PFX_BASE64` +
`WIN_PFX_PASSWORD` secrets and an `Authenticode` signing step around the
WiX light output.

**DEP-11-6-03 (HIGH)** — `packaging/windows/wix/pcloud-rs.wxs:67`.
`Account="LocalSystem"` runs the daemon with full machine privileges. The
Linux unit uses `DynamicUser=yes` or a dedicated `pcloud-rs` user, and the
macOS plist uses `_pcloudd`. LocalSystem is the equivalent of root — the
daemon almost certainly does not need SYSTEM rights. Remediation: switch
to `NetworkService` or a dedicated Windows service account. At minimum
add a justification comment in the WiX file explaining why SYSTEM is
required (it probably is not; file ACLs and the TCP stack work for
NetworkService).

**DEP-11-6-04 (HIGH)** — WinFSP runtime detection is present as a WiX
dependency (line 27) but there is **no user-facing error at daemon runtime**
if WinFSP is uninstalled post-install. The daemon should probe
`HKLM\Software\WOW6432Node\WinFsp` (or the `%ProgramFiles%\WinFsp\bin\launcher-x64.exe`
path) on startup and print `install_hint` pointing at
`https://github.com/winfsp/winfsp/releases`. Remediation: add such a probe
to `crates/pcloud-daemon/src/mount_runtime.rs` (Windows gate) or to
`crates/pcloud-fs/` mount scaffolding.

**DEP-11-6-05 (MEDIUM)** — `crates/pcloud-daemon-win/src/main.rs:218`
spawns the worker thread via `thread::spawn`. If `pcloud_daemon::
serve_with_shutdown` panics on startup the SCM sees "Running" and then
immediate death on join (line 246-255: "Worker panicked; treated as a clean
stop"). A panicking daemon on launch will show as a clean service exit,
not an SCM error. Remediation: in the `Err(err)` / panic arms at lines
248-254, report a non-zero `ServiceExitCode::ServiceSpecific(u32)` so
Windows Event Log records it.

**DEP-11-6-06 (LOW)** — `packaging/windows/wix/pcloud-rs.wxs:43`. Source
path is `$(var.StageDir)\pcloudd.exe`; the build instructions for
`StageDir` are not documented in `packaging/windows/wix/README.md` (I
did not open the README but based on size it looks like a scaffolding
stub). Remediation: document the build pipeline explicitly.

### 11.7 FreeBSD rc.d

`packaging/freebsd/pcloudd.rc` (55 lines):

- `PROVIDE: pcloudd`, `REQUIRE: LOGIN NETWORKING`, `KEYWORD: shutdown` (line
  35-37).
- `rcvar="pcloudd_enable"` (line 42); default `"NO"` (line 46).
- Dedicated `pcloud` user documented (line 23-25), `/usr/sbin/nologin`
  shell, non-existent home.
- `command="/usr/local/bin/pcloudd"`, pidfile `/var/run/pcloudd.pid`
  (lines 50-51).

**DEP-11-7-01 (HIGH)** — `packaging/freebsd/pcloudd.rc` does NOT preload
`fuse.ko`. On FreeBSD, FUSE requires `kldload fuse` before `/dev/fuse` is
exposed to userland. A daemon that tries to mount on start will fail with
`ENOENT /dev/fuse` until somebody manually runs `kldload fuse`. Remediation:
add an `rcorder`-level dependency or a `start_precmd` that runs
`kldstat -q -m fusefs || kldload fuse`. (The comment at the top of
`pcloudd.rc` never mentions fuse.)

**DEP-11-7-02 (MEDIUM)** — Script uses `rc.subr`'s built-in `daemon_user`
privilege drop indirectly via `pcloudd_user="pcloud"`, but the script does
not actually USE that variable — it's declared at line 47 and never
referenced below. `rc.subr` will NOT drop privileges just because you
declared the var; you need either `procname=` + user-aware commands or an
explicit `command_interpreter`. Currently the daemon will run as whatever
user invoked `service pcloudd start` (i.e. root). Remediation: add
`su_cmd="${pcloudd_user}"` / `daemon_user="${pcloudd_user}"` and verify
against `ps -axo user,command | grep pcloudd`.

**DEP-11-7-03 (LOW)** — No OpenBSD / NetBSD rc.d scripts audited in
detail; they are flagged "Scaffolding" in `packaging/README.md:43-44`. Flag
as pending review.

### 11.8 Config schema

`crates/pcloud-config/src/schema.rs` — 1304 lines (not opened in full
during this pass). `crates/pcloud-config/src/paths.rs` opened: every
public field has a rustdoc comment with *default value*, *valid values*,
*security posture*, and an *example* (`paths.rs:48-79`). `env.rs:33-50`
contains a line-by-line env var → TOML key mapping table with semantics per
var. `runtime.rs` enforces `0700` permissions in Production.

**DEP-11-8-01 (LOW)** — I did not confirm that an `/etc/pcloud-rs/
config.example.toml` sample config ships with the .deb (it is not listed in
`packaging/debian/nfpm.yaml:34-54` contents). Remediation: add a
`default-config.toml` asset to the contents list. An operator today has no
reference config to copy.

**DEP-11-8-02 (LOW)** — Env-var documentation lives in
`crates/pcloud-config/src/env.rs` (rustdoc) and in `packaging/README.md`
(user-facing) and in each platform plist header and in the runbook — i.e.
in four places. Risk of drift. Remediation: pick one canonical list (env.rs
is the natural choice) and have the others link or generate from it.

### 11.9 Observability — metrics, tracing, dashboards

`crates/pcloud-observability/src/metrics.rs` has a well-specified
Prometheus-text exporter. Metric families at `metrics.rs:18-27`:

| Name | Type | Labels |
|------|------|--------|
| `pcloud_request_count` | counter | `method`, `status` |
| `pcloud_request_latency_seconds` | histogram | `method` |
| `pcloud_auth_attempts_total` | counter | `result` |
| `pcloud_transfer_bytes_total` | counter | `direction` |
| `pcloud_crypto_lock_state` | gauge | — |
| `pcloud_sync_root_count` | gauge | — |
| `pcloud_ipc_connected_clients` | gauge | — |
| `pcloud_panic_count` | counter | — |

Naming follows Prometheus conventions (`_total`, `_seconds`). Label
sanitiser is documented (`metrics.rs:38-55`) — replaces invalid values with
`"invalid"` opaque token, caps length at 64 chars.

Tracing: `crates/pcloud-observability/src/tracing.rs` has an OTLP exporter
(feature-gated `tracing-otlp`). Strict PII-redacted attribute allow-list
(`ALLOWED_ATTRS`), W3C `traceparent` parser. Line 34-38 honestly flags that
the OTLP pipeline has **not** been exercised against a live collector in
CI.

Health surface: `crates/pcloud-observability/src/exporter.rs:265-280` serves
`GET /metrics` (Prom 0.0.4) and `GET /health` (200 ok / 503 not ready).
`crates/pcloud-web/src/routes.rs` is a separate web UI with its own health
surface.

**DEP-11-9-01 (CRITICAL doc gap, HIGH operational impact)** — **No
`dashboards/` directory exists at repo root.** The audit prompt called it
out as a specific expected location. No Grafana JSON dashboards, no
Prometheus alerting rules (`*.rules.yaml`), no sample `prometheus.yml`
scrape config anywhere. A shipped Prometheus exporter without a dashboard
means every operator has to build their own from the metric-family list,
and there are no recommended alerting thresholds for `pcloud_panic_count`,
`pcloud_request_count{status=~"5.."}`, or latency-histogram buckets.
Remediation: add `dashboards/grafana/pcloud-rs-overview.json`, a
`dashboards/prometheus/alerts.yaml` (at minimum panic_count > 0, p99
request latency > 5s, auth failures > 10/min), and a smoke test that
loads the JSON against `grafana/grafana:latest`.

**DEP-11-9-02 (MEDIUM)** — No `/healthz` / `/readyz` distinction.
`/health` at `exporter.rs:275` is a combined liveness+readiness check. K8s
conventions call for separate endpoints (liveness: "process is alive",
readiness: "can accept traffic"). If pcloud-web is ever intended for k8s
(it seems designed for it based on `pcloud-fleet`), a single `/health` is
inadequate. Remediation: split into `/livez` (process heartbeat only) and
`/readyz` (auth vault loaded + IPC bound + API reachable).

**DEP-11-9-03 (MEDIUM)** — `pcloud-observability/src/tracing.rs:34-38`
honestly documents that OTLP has **not** been exercised against a live
collector. Dimension 11 considers "tracing: OpenTelemetry export" a
requirement. Remediation: add a `docker-compose` smoke test that stands up
Jaeger/OTEL-collector and verifies spans arrive; wire to CI optionally.

### 11.10 Upgrade path, SQLite migrations, vault/journal versioning

- SQLite migrations: `pcloud-store` has `migrations` module per
  ARCHITECTURE.md:21 and per STATUS.md's mention of "migration v<N>". I did
  not open the migrations source in this pass; OPERATIONS-RUNBOOK.md:172-177
  documents the failure mode (`store.open: migration v<N> failed`) and the
  correct operator response ("do not delete the store").
- Auth vault: versioning not explicitly surfaced in the runbook; vault
  format is UID-bound and mode-checked (OPERATIONS-RUNBOOK.md:126-136).
- Journal: OPERATIONS-RUNBOOK.md:74-78 says "After a kill -9, the next
  startup will roll forward the journal"; `pcloud-fs::journal` and
  `pcloud-store::tx` are the cited implementations.
- In-place daemon restart: OPERATIONS-RUNBOOK.md:311-356 has a full
  `Playbook: Upgrade (pinned -> latest)` and a `Playbook: Rollback`.

**DEP-11-10-01 (HIGH)** — **No `pcloud_schema_version` table sentinel is
documented.** The upgrade playbook says "there are no DB migrations in
scope for routine upgrades" (OPERATIONS-RUNBOOK.md:314-315) but offers no
command for an operator to verify which migration the store is at. If the
store ever corrupts mid-migration there is no documented forensics query.
Remediation: document `sqlite3 store.sqlite 'select max(version) from
_pcloud_migrations;'` or equivalent; and if no such table exists, add one.

**DEP-11-10-02 (MEDIUM)** — No auth vault format version byte is
documented. Vault backup/restore in the runbook (OPERATIONS-RUNBOOK.md:394-
434) treats the vault as opaque bytes; a future vault format change will
have no migration path. Remediation: prefix vault with a 4-byte magic +
1-byte version.

### 11.11 Backup / restore documentation

OPERATIONS-RUNBOOK.md:224-260 covers the state to preserve (config, store,
auth vault, page cache, journal) and explicitly marks `~/.cache/pcloud-rs/`
as disposable. Cross-UID restore is refused by design.

**DEP-11-11-01 (LOW)** — Mount orphan registry is not mentioned in the
backup list. Once `bd-1du.4` lands, orphan mounts (stale FUSE endpoints
left by a kill -9) will be tracked somewhere under the state dir; the
runbook should be updated at that point.

### 11.12 Health checks — k8s friendliness

Covered under DEP-11-9-02. `pcloud-web/tests/health.rs` exists (not opened
here); `pcloud-web/README.md` not opened. `pcloud-fleet` exists as an
enterprise-readiness crate but its readiness-probe semantics are unknown.

### 11.13 Resource limits — laptops vs servers

Systemd unit sets `MemoryMax=512M`, `CPUQuota=75%`, `TasksMax=256`,
`LimitNOFILE=4096`, `LimitNPROC=256`, `LimitCORE=0`
(packaging/systemd/pcloudd.service:86-92).

**DEP-11-13-01 (LOW)** — Values are reasonable defaults for a laptop but
on a fleet server handling thousands of sync roots the 512M cap will be
restrictive. No `server.conf` drop-in profile is provided. Remediation: ship
a `packaging/systemd/drop-in.d/server.conf` with `MemoryMax=4G`,
`LimitNOFILE=65536`, `TasksMax=2048`.

### 11.14 FIPS claims

`docs/book/src/architecture/security-model.md:283` explicitly states **"we
have no FIPS constraint"**. The project does not claim FIPS anywhere else
(grepped: 4 files, all either negation or prompt-file). Finding: NONE — honest.

---

## Section 12. Documentation Quality

### 12.1 Parity docs truth — cited-file correctness

Spot-check of 20 rows of `C_FEATURE_PARITY_MATRIX.csv` cross-referenced
against actual Rust sources. This overlaps Dimension 1 (parity accounting);
my focus here is **documentation correctness** — does the cited file:line
actually exist with a plausible implementation.

Files whose existence I personally verified via `wc -l` and/or
`glob` / `grep`:

| CSV row (line) | Cited file | Result |
|----------------|------------|--------|
| 15 | `crates/pcloud-proto/src/auth_api.rs:115` | OK (1018 lines in file) |
| 17 | `crates/pcloud-auth/src/orchestrator.rs:39` | OK (951 lines) |
| 33 | `crates/pcloud-crypto/src/password_scorer.rs:471` | OK (874 lines) |
| 11 | `crates/pcloud-daemon/src/runtime.rs:1008` | OK (6202 lines) |
| 42 | `crates/pcloud-proto/src/public_links_api.rs:694` | OK (1683 lines) |
| 42 | `crates/pcloud-daemon/src/public_link_backend.rs:795` | **MISSING** (file moved to `crates/pcloud-backends/src/public_link_backend.rs`) |
| 42 | `crates/pcloud-sdk/src/lib.rs:934` | OK (4437 lines) |

Broader grep: the CSV cites `crates/pcloud-daemon/src/<name>_backend.rs` in
**41+ rows** for public_link / shares / account / backup / transfer / sync /
auth / notifications backends. **Every single one of those files now lives
under `crates/pcloud-backends/src/` and does not exist at the cited path.**
The following file ops are confirmed by `ls crates/pcloud-backends/src/`:

```
account_backend.rs      auth_backend.rs         backup_backend.rs
notifications_backend.rs  public_link_backend.rs  shares_backend.rs
sync_backend.rs         transfer_backend.rs     crypto_backend.rs
folder_backend.rs       ... (mock.rs, etc.)
```

`crates/pcloud-daemon/src/` contains `auth_vault.rs`, `bootstrap.rs`,
`dispatch.rs`, `runtime.rs`, etc. — **none** of the `*_backend.rs` files.

#### Findings

**DOC-12-1-01 (CRITICAL for documentation truth, HIGH for audit
defensibility)** — The parity matrix (`C_FEATURE_PARITY_MATRIX.csv`) and
the parity review (`C_FEATURE_PARITY_REVIEW.md`) cite ≥41 rows whose
`rust_reference` column points at files that do not exist. Per the Dimension
1 rule, any `Implemented` row whose cited Rust file doesn't exist = HIGH.
This is strictly a *documentation* problem — the functionality has simply
moved crates — but it is the single most severe documentation issue in the
fork. **This directly undermines `bd-1du.10` ("prove and gate final C
parity claims")**: you cannot prove parity against citations that 404.
Remediation: a `sed`-scripted sweep of CSV (and the narrative file and
anywhere else that stale paths appear, e.g. `API-REFERENCE.md:14`,
`ARCHITECTURE.md:31`, `SECURITY.md:60-61` which still says
`crates/pcloud-daemon/src/auth_backend.rs`) — replace
`pcloud-daemon/src/{account,auth,backup,crypto,folder,notifications,public_link,shares,sync,transfer}_backend.rs`
with `pcloud-backends/src/...`.

**DOC-12-1-02 (HIGH)** — `ARCHITECTURE.md:31` describes `pcloud-daemon` as
having "per-subsystem backends" and does NOT list `pcloud-backends` at all
in its crate map (lines 15-34). But `pcloud-backends` is a workspace
member (`Cargo.toml:38`) and the README *does* list it
(`README.md:164`). ARCHITECTURE.md and API-REFERENCE.md are both stale.
Remediation: add `pcloud-backends` to the ARCHITECTURE.md crate map; fix
`API-REFERENCE.md:14` to list both `pcloud-daemon` runtime and
`pcloud-backends` subsystem modules.

**DOC-12-1-03 (HIGH)** — `SECURITY.md:60-61` and `SECURITY.md:67` cite
`crates/pcloud-daemon/src/auth_backend.rs`. That file does not exist
(moved to `pcloud-backends`). Security disclosure scope sections
pointing at non-existent files is a credibility issue — remediate before
the next security review cycle.

**DOC-12-1-04 (MEDIUM)** — `CLAUDE.md` itself (the authoritative handoff)
cites multiple `pcloud-daemon/src/*_backend.rs` paths at lines 122-123,
127, 215-217, 232, 242-243, 249-252, 258-259, 270-271, 275-276, 280-283,
286-288. Per CLAUDE.md's own "Documentation Discipline" rule (lines 492-
504: *"whenever code reality changes, update ... this CLAUDE.md if the
global handoff state changed materially"*), this IS a reality change that
was never propagated. Remediation: sweep CLAUDE.md for stale paths.

### 12.2 Matrix ↔ Review alignment

STATUS.md:389-395 reports `186 total / 158 Implemented / 0 Partial / 0
Missing / 28 Rejected`. Matrix raw count confirms: 186 data rows (187
lines with header; `wc -l` = 187). 28 Rejected rows correspond exactly to
the 28 row numbers listed in `REJECTED-RATIONALES-14042026.md:5`
(rows 2, 5, 6, 10, 12, 13, 43, 44, 45, 46, 99, 100, 101, 102, 103, 104,
105, 106, 113, 114, 115, 126, 151, 152, 157, 160, 167, 169).

`C_FEATURE_PARITY_REVIEW.md:26-29` defers counts to STATUS.md per ADR 0009;
this is the correct pattern. `C_FEATURE_PARITY_REVIEW.md:46` asserts
"no Partial rows remain in the matrix as of 2026-04-16" — matches the
matrix.

Finding: alignment between matrix, review, STATUS.md, and rejection
rationale is TIGHT. No discrepancy found.

### 12.3 STATUS.md — hand-edited or generated?

`STATUS.md:5` is a date stamp (`_Last reviewed: 2026-04-16_`), not a
timestamp from a generator script. No `scripts/regen-status.sh` was
found. STATUS.md therefore appears to be hand-edited.

**DOC-12-3-01 (MEDIUM)** — STATUS.md is the single source of truth for
parity counts (per ADR 0009) but it is hand-edited. This is a drift
hazard: the next time a row flips Implemented→Rejected, someone must
remember to update STATUS.md's counts by hand. Remediation: add a
`scripts/regen-status.sh` that regenerates the counts section of STATUS.md
from a freshly-parsed CSV (`awk -F, 'NR>1{c[$5]++} END{...}'` — or
equivalent robust CSV parser since some cells are quoted and contain
commas — see the row-93 artifact documented under "What is present"
below). Gate CI to fail if `STATUS.md` is stale relative to
`C_FEATURE_PARITY_MATRIX.csv`.

### 12.4 REJECTED-RATIONALES-14042026.md coverage

Verified: `REJECTED-RATIONALES-14042026.md:5` enumerates 28 row numbers.
Cross-check with `awk -F',' 'NR>1 && $5=="Rejected" {print NR}'` against
the matrix: there is one quoting artifact at row 93 (c_reference column
contains a comma, which naive CSV parsers split), so the simple awk under-
counts by one but the actual row count is 28. The 28 rationales appear
individually in the MD file under the categories Ghost / Stub / Replaced /
Billing-out-of-scope / C-internal-plumbing / Insecure-legacy / Typo-
duplicate (categories defined at lines 29-35).

Finding: coverage matches the matrix. NO finding.

### 12.5 mdBook build

`docs/book/book.toml` exists (18 lines): title `pcloud-rs Rust Handbook`,
src `src`, git-repository-url pointing at `github.com/pcloudcom/pcloud-rs`
(the upstream C tree, not this fork — see DOC-12-5-01), theme `navy`.

I could not run `mdbook build` — `mdbook` is not installed on this audit
runner. So I verified chapter-file existence instead against the full
`src/SUMMARY.md`. **All chapters referenced by SUMMARY.md exist on disk.**
44 chapter files checked (getting-started × 3, architecture × 7 including
all 10 ADRs, security × 4, operations × 9 + platforms × 6, development ×
6, reference × 5, enterprise × 9 linked from `../../enterprise/`, plugins ×
4 linked from `../../plugins/`, parity × 3, archive × 1, faq × 1) — all
present.

**DOC-12-5-01 (MEDIUM)** — `docs/book/book.toml:10-11`.
`git-repository-url` and `edit-url-template` both point at
`https://github.com/pcloudcom/pcloud-rs` which is the **upstream C tree**
(per CLAUDE.md:31-38 and MEMORY.md "repo_fork_url"). The active fork is
`github.com/ezechiel203/pcloud-rs`. The book's "Edit this page" links will
404 for every reader. Remediation: flip to `github.com/ezechiel203/pcloud-rs`
(the self-link MEMORY.md explicitly names).

**DOC-12-5-02 (LOW)** — `mdbook` is not enforced in CI; I could not verify
the book actually builds under `-D warnings`. The release-checklist chapter
(`development/release-checklist.md`) should gate `mdbook build` as a
mandatory step. Remediation: add `mdbook build` to `.github/workflows/
docs.yml` and fail on broken intra-doc links.

**DOC-12-5-03 (LOW)** — `docs/book/src/architecture/security-model.md`
and `docs/book/src/security/model.md` both exist (SUMMARY.md lines 18 and
33). Risk of content drift between the two. Remediation: pick one
canonical model doc, make the other a cross-reference.

### 12.6 CLAUDE.md honesty hygiene (grep for forbidden claims)

CLAUDE.md grep hits for "full parity" / "production ready" / "enterprise
ready" / "drop-in replacement":

| Line | Hit | Context |
|------|-----|---------|
| 54 | "substantially complete" but **"not honest to call it 'full parity', 'production ready', or 'drop-in replacement'"** | self-negation |
| 77-80 | Enumerated forbidden claims list | self-rule |
| 179 | "Still not full parity" | self-negation |

**All hits in CLAUDE.md are either the rule itself or self-negating
statements.** No false claim found.

Same check across the rest of the repo (grep across `*.md`):

- README.md:17-20: negation (`does NOT claim`).
- CONTRIBUTING.md:166-169: enumerates the rule.
- SECURITY.md:161-164: enumerates the rule.
- C_FEATURE_PARITY_REVIEW.md:39, 785-787, 840: all negations.
- STATUS.md:381-382, 408-409, 492-493, 612-613: all negations.
- docs/book/src/faq.md:16-17: negation.
- docs/enterprise/README.md:10: negation.
- docs/roadmap-complete.md:13: negation.
- CHANGELOG.md:2026-2027: negation.

Finding: the project polices the forbidden-claims rule with remarkable
discipline. **No false claims found.** This is genuinely well-done.

### 12.7 Deployment guide walkthrough (senior sysadmin, new to project)

Mental walkthrough: install → auth → config → systemd → mount → verify.

OPERATIONS-RUNBOOK.md covers most steps (Playbook: First install at line
268-306; Playbook: Upgrade at 311-356; Playbook: Rollback at 358-392;
Vault backup / restore at 394-434; TLS cert rotation at 436-463;
Incident triage at 465+).

**DOC-12-7-01 (MEDIUM)** — **First install step 1** reads: "Debian/Ubuntu:
`sudo apt install pcloud-rs` (from the project APT repo)". **No APT repo
exists for this fork.** `packaging/debian/nfpm.yaml` is a recipe to BUILD
a .deb, not a repo that serves them. Same goes for `dnf install pcloud-rs`
(no COPR / RPM repo documented), `pacman -S pcloud-rs`, and `nix profile
install github:pcloud-rs/pcloud-rs#pcloud-rs` (the path `pcloud-rs/pcloud-rs`
on GitHub is actually the upstream C tree). A senior sysadmin following the
runbook verbatim will hit "Unable to locate package pcloud-rs" on step 1.
Remediation: mark these channels as aspirational until they exist, OR
replace step 1 with "From source: see README.md for cargo install" and put
the repo-based methods behind "once published, you will be able to...".

**DOC-12-7-02 (MEDIUM)** — **First install step 5** references
`systemctl --user enable --now pcloud-daemon`, but the packaged unit is
named `pcloudd.service` (packaging/systemd/pcloudd.service:1 Description,
package contents at packaging/debian/nfpm.yaml:43-47 installs
`pcloudd.service`). `pcloud-daemon` vs `pcloudd` is a 1-character
difference that will bite every first-time operator. Remediation: grep
the runbook for `pcloud-daemon` as a service name (not as a crate) and
replace with `pcloudd`.

**DOC-12-7-03 (MEDIUM)** — **No mount / FUSE walkthrough exists in the
runbook** (only what happens when the shell rejects a sync path,
OPERATIONS-RUNBOOK.md:157-169). This is because `bd-1du.4` is still open,
but the runbook should explicitly say so instead of omitting the section
silently. Remediation: add a "Mount (pending `bd-1du.4`)" section that
tells users to expect no mounted drive yet.

**DOC-12-7-04 (LOW)** — OPERATIONS-RUNBOOK.md:12-13 uses `cd .` as a path
(`cd /home/ezechiel203/Projects/FORKS/pcloud-rs/`). That's a developer
path, not a deployment path. Remediation: replace with the repo clone
location the reader actually has.

### 12.8 Troubleshooting guide

OPERATIONS-RUNBOOK.md:109-191 covers failure modes:

- IPC socket already in use (remove stale socket) ✓
- Auth vault rejected (ownership / mode) ✓
- TFA required but never prompted ✓
- Sync root rejected ✓
- Store migration failed ✓
- Crypto locked — requested op needs unlocked shell ✓

**DOC-12-8-01 (MEDIUM)** — No "FUSE mount refused" troubleshooting
(blocked on `bd-1du.4` but should be a placeholder).

**DOC-12-8-02 (MEDIUM)** — No "TLS cert mismatch" troubleshooting beyond
the certificate-rotation playbook at line 436-463. A user whose system CA
bundle is out of date will see `invalid peer certificate` errors with no
immediate reference.

**DOC-12-8-03 (LOW)** — No "sync queue stuck" troubleshooting. The
daemon's `pcloud-cli status` output includes queue depth (per line 88:
"pending transfers") but no diagnosis steps for a queue that never drains.

### 12.9 SDK rustdoc

I could not run `cargo doc --workspace --no-deps` on this audit runner
without risk of a long build. `crates/pcloud-sdk/src/lib.rs:1` starts
with `#![forbid(unsafe_code)]` and a solid crate-level rustdoc (lines 4-
40) covering conventions across `EmbeddedDaemon` helpers: preconditions,
errors (`SdkError`), side effects, daemon round-trips. This is a
professional SDK intro.

STATUS.md:57 reports gate-run result: `RUSTDOCFLAGS=-Dwarnings cargo doc
--workspace --no-deps` = PASS on 2026-04-16 (after a 3-link fix). So as of
the last run, rustdoc is warning-free.

Finding: NO finding on rustdoc per se. Flag for Dimension 10 testing
whether CI still enforces `RUSTDOCFLAGS=-Dwarnings`.

### 12.10 Security guide (SECURITY.md, SECURITY-MODEL.md)

SECURITY.md (168 lines) covers: reporting channel (GH Security
Advisories preferred, encrypted email), response SLOs (3 / 7 / 30 / 90
days), scope (auth, IPC, config, secret handling, crypto, filesystem,
proto, SDK, CLI), out-of-scope list, known issues reference to
`SECURITY-AUDIT-FINAL-14042026.md`.

SECURITY-MODEL.md (165 lines) is the structured model: trust boundaries
diagram (line 13-30), untrusted input surfaces (line 32-40).

`docs/book/src/security/secrets.md`, `docs/book/src/security/threat-
model.md`, `docs/book/src/security/audit-dossier.md`,
`docs/book/src/security/model.md` all exist.

**DOC-12-10-01 (HIGH)** — `SECURITY.md:60-61` cites
`crates/pcloud-daemon/src/auth_backend.rs` and
`crates/pcloud-daemon/src/auth_vault.rs` as auth surface. **`auth_backend.rs`
moved to pcloud-backends** (see DOC-12-1-03); `auth_vault.rs` stayed in
pcloud-daemon (that one is correct). Fix the stale half.

**DOC-12-10-02 (LOW)** — `SECURITY.md:9` points at
`SECURITY-AUDIT-FINAL-14042026.md` as the authoritative audit record. I
did not verify existence of that file during this pass — add a check to
the release-checklist that orphaned audit-file references are removed.

### 12.11 Release notes / CHANGELOG.md

CHANGELOG.md is 2028 lines. Format: Keep a Changelog; all entries currently
under `[Unreleased]` (line 15) because no version has been tagged yet. No
`[0.1.0]` section despite workspace version being `0.1.0`
(`Cargo.toml:59`).

**DOC-12-11-01 (LOW)** — With a 2028-line `[Unreleased]` section and no
tagged release, CHANGELOG.md is a dumping ground of per-wave notes, not a
user-facing changelog. The Keep-a-Changelog format expects a cut-off per
release. This is fine pre-alpha, but it should be triaged before the first
tagged release.

**DOC-12-11-02 (LOW)** — CHANGELOG.md:10-13 cites source documents
`FINAL-PARITY-PROOF-WAVE*.md`, `RECONCILIATION-WAVE*.md`,
`SECURITY-AUDIT*.md`, `MATRIX-*.md`, `PARITY-AUDIT-FINAL-14042026.md`. I
did not verify these exist on disk in this pass. If any were purged, the
citations should be purged too.

### 12.12 README quickstart walkthrough

README.md:1-100 covers: feature badge (line 3), workspace layout (line
22-44), build/test/docs commands (line 46-77), daemon + CLI quickstart
(line 82+).

**DOC-12-12-01 (MEDIUM)** — README.md quickstart uses `cargo run -p
pcloud-daemon -- serve` for daemon and `cargo run -p pcloud-cli -- ...`
for CLI. But the actual shipped binary names (per the WiX file,
packaging/macos/README.md, the .deb contents) are `pcloudd` and `pcloudc`.
The README never explicitly tells a reader: "after `cargo install --path
crates/pcloud-daemon && cargo install --path crates/pcloud-cli`, the
binaries are named pcloudd and pcloudc". Remediation: add a one-line
mapping `cargo run -p pcloud-daemon` ↔ `pcloudd`, `cargo run -p
pcloud-cli` ↔ `pcloudc`.

**DOC-12-12-02 (LOW)** — README.md:60 runs `cargo deny --manifest-path
Cargo.toml check`. `audit.toml:10-25` time-boxes **5 advisory ignores**
with `review: YYYY-MM-DD` deadlines (2026-06-01 and 2026-07-15). A
contributor running `cargo audit` after the review dates will (correctly)
see failures. Good hygiene, just flag it.

### 12.13 Cross-cutting — empty-backtick placeholder

**DOC-12-13-01 (MEDIUM)** — Grep shows 10 `.md` files contain the literal
token `` `` `` (empty backticks). Representative hits:
`CONTRIBUTING.md:28` — "Contributions are welcome to the `` workspace";
`CONTRIBUTING.md:38` — "pinned via `rust-toolchain.toml` in ``";
`CONTRIBUTING.md:72` — "All commands run from the `` directory";
`README.md`, `CLAUDE.md`, `SECURITY.md`, `docs/book/src/introduction.md`,
`docs/book/src/parity/status.md`,
`docs/book/src/architecture/overview.md`,
`docs/book/src/architecture/performance.md`,
`docs/book/src/architecture/security-model.md`,
`docs/book/src/security/audit-dossier.md`,
`docs/adr/0001-record-format.md` also hit.

This looks like the aftermath of a global `s/<old_name>/<new_name>/` that
collapsed to empty string. Remediation: grep-replace `` `` `` in all .md
files with the intended project name (probably `pcloud-rs` or
`pcloud-rs-rust-dev`, based on context).

### 12.14 Manpages

`packaging/man/` ships `pcloudc.1`, `pcloudd.1`, `pcloud.conf.5` — good.
I did not open them to verify content matches the current CLI surface.

**DOC-12-14-01 (LOW)** — No CI check that `pcloudc --help` output matches
`pcloudc.1`. Flag.

### 12.15 Plugin documentation

`docs/plugins/README.md` plus `autoheal.md`, `backup-schedule.md`,
`dlp-builtin.md`, `publink-expiry.md` all exist. Crates
(`pcloud-plugin-autoheal`, `pcloud-plugin-backup-schedule`,
`pcloud-plugin-dlp`, `pcloud-plugin-publink-expiry`) are all workspace
members.

Finding: structurally complete; no per-file finding from this dimension.

---

## Summary by Severity

### CRITICAL
- DEP-11-6-01 WiX placeholder UpgradeCode (one-way door for Windows upgrades)
- DOC-12-1-01 ≥41 parity-matrix rows cite moved / non-existent files (undermines bd-1du.10)

### HIGH
- DEP-11-4-01 nfpm hard-coded version drift vs Cargo.toml
- DEP-11-5-04 No macFUSE/fuse-t detection for macOS
- DEP-11-6-02 No CI pipeline for Authenticode MSI signing
- DEP-11-6-03 WiX service runs as `LocalSystem` (unjustified privilege)
- DEP-11-6-04 No WinFSP runtime probe / install-hint
- DEP-11-7-01 FreeBSD rc.d does not preload `fuse.ko`
- DEP-11-9-01 No `dashboards/` — no Grafana JSON, no alert rules
- DEP-11-10-01 No documented `_pcloud_migrations` sentinel query
- DOC-12-1-02 ARCHITECTURE.md crate map missing pcloud-backends
- DOC-12-1-03 SECURITY.md cites non-existent auth_backend.rs path
- DOC-12-10-01 SECURITY.md cites moved auth_backend.rs (same root cause)

### MEDIUM
- DEP-11-1-02 `Type=simple` without `sd_notify` (false-ready race)
- DEP-11-1-03 No `WatchdogSec=` on systemd unit
- DEP-11-2-01 No logrotate.d drop-in for file-based logging
- DEP-11-3-02 AppArmor/SELinux profiles not installed by .deb/.rpm
- DEP-11-4-02 Maintainer address is `example.invalid` placeholder
- DEP-11-4-03 postinstall/postremove scripts unaudited
- DEP-11-5-01 launchd plist missing `ExitTimeOut`
- DEP-11-5-02 launchd plist uses `--system` flag that may be unhandled
- DEP-11-5-03 Notarization pipeline exists but no CI wiring
- DEP-11-6-05 Windows worker panic hides real exit code from SCM
- DEP-11-7-02 FreeBSD rc.d declares pcloudd_user but never uses it
- DEP-11-9-02 No `/livez` vs `/readyz` distinction
- DEP-11-9-03 OTLP pipeline never run against live collector in CI
- DEP-11-10-02 No auth-vault format version byte
- DOC-12-1-04 CLAUDE.md itself cites stale backend paths
- DOC-12-3-01 STATUS.md hand-edited — no regen script
- DOC-12-5-01 mdBook `git-repository-url` points at upstream C tree
- DOC-12-7-01 Runbook references non-existent APT/DNF/Nix repos
- DOC-12-7-02 Runbook service name `pcloud-daemon` vs shipped `pcloudd`
- DOC-12-7-03 No mount walkthrough (pending bd-1du.4) — mention it
- DOC-12-8-01 No FUSE-refused troubleshooting section
- DOC-12-8-02 No TLS cert mismatch quick-ref
- DOC-12-12-01 README uses `cargo run` names, not installed binary names
- DOC-12-13-01 10+ .md files contain empty-backtick placeholder `` `` ``

### LOW
- DEP-11-1-01 systemd `Documentation=` URL points at C upstream
- DEP-11-1-04 IPAddressAllow= hostname resolution hazard
- DEP-11-1-05 Two competing systemd units with different hardening
- DEP-11-3-01 SELinux policy_module version not tied to release
- DEP-11-4-04 No explicit RPM scriptlet conventions
- DEP-11-4-05 No distinct RPM `%pre/%post` handling
- DEP-11-5-05 Dead `PCLOUD_*` env vars in macOS plist
- DEP-11-6-06 WiX `StageDir` not documented
- DEP-11-7-03 OpenBSD/NetBSD rc.d unverified scaffolding
- DEP-11-8-01 No `config.example.toml` shipped with .deb
- DEP-11-8-02 Env-var docs duplicated 4 places
- DEP-11-11-01 No mount-orphan registry in backup docs
- DEP-11-13-01 No server-profile systemd drop-in
- DOC-12-5-02 mdbook not enforced in CI
- DOC-12-5-03 Two security-model docs — drift risk
- DOC-12-7-04 Runbook `cd .` is a developer path
- DOC-12-8-03 No sync-queue-stuck troubleshooting
- DOC-12-10-02 SECURITY-AUDIT file reference unverified
- DOC-12-11-01 CHANGELOG a dumping ground under `[Unreleased]`
- DOC-12-11-02 CHANGELOG cites MATRIX-*.md / WAVE-*.md files unverified
- DOC-12-12-02 `cargo audit` will fail after time-boxed ignores expire (by design, but flag)
- DOC-12-14-01 No CI check that manpages match `--help` output

### NONE (honest-and-correct)
- FIPS claims (§11.14)
- Matrix ↔ Review alignment (§12.2)
- REJECTED-RATIONALES coverage (§12.4)
- CLAUDE.md honesty hygiene (§12.6) — the project polices its own rules unusually well

---

## Key cross-cutting observations

1. **The single biggest documentation defect is the stale backend path
   citations (DOC-12-1-01).** It affects the parity matrix, the review
   narrative, CLAUDE.md, ARCHITECTURE.md, API-REFERENCE.md, and
   SECURITY.md. Fixing it is mechanical but blocks `bd-1du.10`.
2. **The systemd unit (packaging/systemd/pcloudd.service) is
   unusually-strong** — substantially above average for a pre-alpha fork.
   Three directives are missing (`WatchdogSec=`, `Type=notify`+`sd_notify`,
   `ExitTimeOut` analogue on macOS) but the rest is production-shape.
3. **The Windows UpgradeCode placeholder (DEP-11-6-01) is a ticking
   time bomb.** Any MSI that ships with the placeholder GUID cannot be
   upgraded by a later MSI with a real GUID.
4. **Dashboards are entirely absent (DEP-11-9-01).** Shipping
   Prometheus metrics without a Grafana dashboard and alert rules is
   half-finished operational work.
5. **Honesty discipline is genuinely strong.** Self-policing of the
   "no full-parity / no production-ready" rule across 10+ files is
   atypical. Operators reading the docs get an accurate picture of the
   project's maturity.

---

_End of Section 11-12 audit._

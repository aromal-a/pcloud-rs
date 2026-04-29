# pcloud-rs — Dimension 10: Testing & QA Audit

Date: 2026-04-29
Auditor: Claude (Opus 4.7, 1M)
Scope: `crates/**/{src,tests,benches,fuzz}`, `.github/workflows/*.yml`, `crates/pcloud-live-e2e/`.
Mode: read-only.

## Summary

pcloud-rs has substantial testing scaffolding (~1,800 in-source `#[test]` markers, 21 live-e2e flows, 11 fuzz targets across 4 crates, 12 bench targets, 11 proptest files, 1 chaos suite, IPC stress test). Coverage strength is concentrated in `pcloud-fs`, `pcloud-daemon`, `pcloud-crypto`, `pcloud-ipc`, `pcloud-cli`, and `pcloud-proto`. Live verification is genuine — it dispatches against a real account and asserts on response state — but the live-e2e job is `continue-on-error: true` (`.github/workflows/ci.yml:274`), so green CI does NOT prove live regressions absent. Coverage CI is also `continue-on-error: true` (`ci.yml:328`) with no enforced threshold; macOS live-FUSE and chaos jobs are explicitly **deferred** (`ci.yml:353-397`).

Most material gaps for enterprise readiness:

1. **Cross-platform CI is below the Tier-1 bar** the docs claim. Linux Tier-1 is genuine; macOS runs only mock-backend tests (no fuse-t in CI); Windows is `--exclude pcloud-fs` and there is no integration-test job; FreeBSD is informational `continue-on-error: true`. CLAUDE.md documents Windows as Tier-2 with named-pipe accept-loop unwired and no `cargo test --tests` ever run on Windows — yet `STATUS.md`/audit narrative speak of cross-platform parity as substantially landed.
2. **Live-E2E missing flows for retained parity rows.** No live test for TFA code submission, recovery code, TFA SMS / device resend, change_password, register, verify_email, get_promo, account team-share crypto wrap, or `upload_writefromfile` (the very row 93 still Partial).
3. **Live-E2E `change_crypto_pass` test is a `todo!()` body** (`crates/pcloud-live-e2e/tests/change_crypto_pass.rs:47`).
4. **Several crates have zero tests** — `pcloud-daemon-win`, `pcloud-cache` integration, `pcloud-mockserver`, `pcloud-error`, `pcloud-fleet` (only live tests), `pcloud-plugin-*` excluding api+dlp.
5. **Fuzz coverage holes**: no fuzz target for HTTP/JSON proto-response decoding chain end-to-end or for crypto `seal/open` round-trip with adversarial AAD.
6. **Benchmark gaps**: no IPC throughput bench (only codec), no cache-with-eviction bench, no concurrency benchmark for the sync engine queue.

No CRITICAL findings (the test infrastructure is real and reaches production code paths). HIGH findings concentrate in CI honesty and live-coverage gaps for retained rows. CLAUDE.md's "production ready" prohibition is honored in CI (live tests are explicitly non-gating until 4 green weekly windows — `ci.yml:274`).

---

## Findings by severity

CRITICAL: 0  HIGH: 7  MEDIUM: 9  LOW: 6

---

## HIGH

### H-1. Live-E2E job is `continue-on-error: true` and only runs weekly

**Severity:** HIGH
**File:** `.github/workflows/ci.yml:269-298`
**Evidence:** `if: github.event_name == 'workflow_dispatch' || github.event_name == 'schedule'` + `continue-on-error: true  # Provisional: remove once suite is stable`. PRs and pushes never run live-e2e. The note says "Remove it once the live-e2e suite has run cleanly for at least four consecutive weekly windows" but no automation enforces tracking that.
**Risk:** A real-account regression in auth, transfers, public links, sync, crypto, or shares can land on `development`/`main` and the only CI signal is suppressed (job goes green even on test failure). Combined with the gating to weekly cadence, regressions can age 6 days before the next window catches them — and even then the PR that introduced them is already merged.
**Remediation:** Add a release-blocking pre-tag check that the most recent 4 scheduled live-e2e runs all passed; flip `continue-on-error: false` per the inline comment's stated criterion. Track the streak in `STATUS.md` so it cannot be gamed.

### H-2. Live-E2E missing for retained parity rows

**Severity:** HIGH
**Files:** `crates/pcloud-live-e2e/tests/` (file inventory).
**Evidence:** Live flows present: `auth_lifecycle`, `backup_lifecycle`, `change_crypto_pass` (stub), `crypto`, `drain`, `field_selectors`, `fleet_mtls`, `integrity_sweeper`, `mount_linux`, `public_links`, `rate_limit`, `shares`, `shares_a_to_b`, `shares_active_a_to_b`, `snapshot_pipeline`, `snapshot_prune`, `sync_loop_live`, `sync_roots`, `transfers`, `tree_link_from_paths`, `windows_liveness`. **Missing live coverage** for retained rows that CLAUDE.md lists as "Implemented and live-verified": TFA code submission (`Method::TfaSubmit`), TFA recovery code (`Method::TfaRecoverySubmit`), TFA SMS resend, TFA device resend, `verify_email` / `verify_email_restricted` / `lost_password` / `change_password` / `register` / `get_promo` / `set_language` / `set_api_server`, `upload_writefromfile` (parity row 93 — still Partial), `psync_send_publink` (row 42), `psync_crypto_account_teamshare` (row 142), `psync_crypto_share_folder` (row 124).
**Risk:** Auth/TFA flows cited in CLAUDE.md as "live-verified" have no automated regression detection. Crypto share/team-share temppass flow (rows 124/142) is Partial per `STATUS.md` and has no live wire-level proof in this suite.
**Remediation:** Add `tfa_lifecycle.rs`, `account_utility.rs`, `upload_writefromfile.rs`, `team_share_temppass.rs` modules. For TFA, use a CI account that exposes a deterministic recovery code (or mock the SMS channel). Track each missing flow under `bd-1du.10`.

### H-3. `change_crypto_pass` live test is a `todo!()` body

**Severity:** HIGH
**File:** `crates/pcloud-live-e2e/tests/change_crypto_pass.rs:47`
**Evidence:** `todo!("email-OTP channel not automatable — see bd-1du.10 and pcloud-rs-s1p.57")` — the `#[test]` is gated `#[ignore]` and panics if executed. This means the parity claim "implemented" for `change_crypto_pass` family (CLAUDE.md "Crypto parity progress") has zero live evidence.
**Risk:** Crypto password rotation is the most security-critical operation in the system after key generation. A regression in `SendCryptoChangeUserPrivate` → confirmation → `CryptoChangePassword` could brick user vaults silently. CLAUDE.md status of "implemented" is unsupported by live test evidence.
**Remediation:** Stand up a CI-scoped IMAP/SMTP capture (or pre-shared OTP fixture as the code comments suggest), promote the test to executable, and gate parity row closure on it passing. Track under `bd-1du.10`.

### H-4. Coverage CI is non-gating with no threshold

**Severity:** HIGH
**File:** `.github/workflows/ci.yml:300-351`
**Evidence:** `coverage` job runs `cargo llvm-cov` weekly, uploads `lcov.info` as an artifact, but `continue-on-error: true` and no `--fail-under-lines=N`. Comment block (lines 305-323) explicitly defers the policy decision.
**Risk:** Coverage cannot regress without notice. Adding new IPC dispatch arms, new mount-policy branches, or new crypto recovery paths and never testing them is invisible to CI.
**Remediation:** Set a workspace floor (suggest 65% lines; pcloud-fs/pcloud-crypto/pcloud-ipc 80% as critical-path crates); fail the job on regression of >2 percentage points. Per-crate floors enforced via `cargo llvm-cov --workspace --json` parsed in a follow-up step.

### H-5. Windows CI excludes `pcloud-fs` and never runs `--tests`

**Severity:** HIGH
**File:** `.github/workflows/ci.yml:63-71`
**Evidence:** `cargo test --workspace --exclude pcloud-fs` only. CLAUDE.md ("Windows posture") documents that `cargo test --workspace --tests` has NOT been run on Windows — only `--lib`. CI matches that (`cargo test` without `--tests` runs unit + integration but `--exclude pcloud-fs` strips the platform-dependent surface, and there is no live WinFSP mount step).
**Risk:** `WindowsIpc` named-pipe accept loop (per CLAUDE.md, in-flight) and the `winfsp_ffi` shim have no integration coverage in CI. A regression there is invisible until release-time hardware verification.
**Remediation:** Add a Windows job that runs `cargo test -p pcloud-ipc --tests`, `cargo test -p pcloud-fs --lib --tests` (mock backend only), and a smoke test of `pcloudd-svc` start/stop. Add a self-hosted Windows runner with WinFSP for the live mount path before claiming Tier-1.

### H-6. macOS CI runs only mock-backend tests; no live fuse-t

**Severity:** HIGH
**File:** `.github/workflows/ci.yml:42-61` and `ci.yml:353-374` (deferred mac-FUSE job).
**Evidence:** macOS CI runs `cargo test --workspace --exclude pcloud-fs` plus `pcloud-fs --lib` + 3 mock-backend integration tests. The deferred mac-FUSE job comment block (lines 353-374) acknowledges no fuse-t in GH-hosted runners and lists "infrastructure / budget decision" as the closure path.
**Risk:** macOS FFI shim (`crates/pcloud-fs/src/platform/macos.rs`, `macos_ffi.rs`) — 16 callbacks per CLAUDE.md — has no automated live verification. The mount-handle RAII, signal-handler reaping (`platform/macos.rs::install_signal_handler_once`), and lifecycle invariants are tested only on Linux. CLAUDE.md correctly says "live-verified on real Darwin hardware is still pending hardware sign-off" — but there is no scheduled hardware run either.
**Remediation:** Provision a self-hosted macOS runner with fuse-t pre-installed, gate it weekly. Until then, do not let docs imply parity with Linux; STATUS.md/CLAUDE.md should call macOS Tier-2 explicitly in user-facing surfaces.

### H-7. FreeBSD CI is `continue-on-error: true` and excludes `pcloud-fs`

**Severity:** HIGH
**File:** `.github/workflows/ci.yml:79-91`
**Evidence:** `vmactions/freebsd-vm@v1` runs `cargo check --workspace` + `cargo test --workspace --exclude pcloud-fs`. `pcloud-fs/src/platform/bsd.rs` is in scope per the parity matrix and audit-06 ncx.29 comment block, but BSD never runs FUSE tests.
**Risk:** BSD signal-driven mount cleanup (CLAUDE.md "Signal-driven mount cleanup posture") is documented as Tier-3 — but the current CI cannot detect even basic compile regressions in `bsd.rs::bsd_reaper_main` because the job is non-gating.
**Remediation:** Either remove `pcloud-fs` from BSD exclusion and run with `fusefs.ko` preloaded inside `vmactions/freebsd-vm`, or hold BSD documentation strictly to Tier-3 in user-facing docs and remove FUSE-bsd citations from parity claims.

---

## MEDIUM

### M-1. `pcloud-cache` has no integration tests directory

**Severity:** MEDIUM
**File:** `crates/pcloud-cache/`
**Evidence:** No `tests/` dir. 10 inline `#[test]` cases in `staging.rs`, `page_cache.rs`, `lib.rs`. The page_cache benches exist (`crates/pcloud-fs/benches/page_cache.rs`) but the cache crate itself ships no integration tests for eviction-under-pressure, concurrent reader/writer, or replay-after-crash.
**Risk:** Cache layer mistakes manifest as data-corruption or perf cliffs that unit tests in the same module will not catch.
**Remediation:** Add `tests/concurrent_eviction.rs`, `tests/replay_after_crash.rs`, drive via `pcloud-chaos`.

### M-2. Several crates have zero tests

**Severity:** MEDIUM
**Crates:** `pcloud-daemon-win` (0 src + 0 tests), `pcloud-error` (0), `pcloud-mockserver` (0 in src; integration tests exist in `tests/mock_flows.rs` only), `pcloud-fleet` (0 in src; only `tests/live_mtls.rs` and `tests/reference_server.rs`), `pcloud-plugin-publink-expiry` (0), `pcloud-plugin-backup-schedule` (0), `pcloud-plugin-autoheal` (0 in src — has `tests/`), `pcloud-policy` actually has 7 inline (initial enumerator missed because grep filtered to files with at least one match — see corrected count via `grep -c` on `policy/src/lib.rs`).
**Risk:** Plugin crates with policy-effect on live operations (publink-expiry, backup-schedule) are uncovered.
**Remediation:** Add at least a `tests/contract.rs` per plugin verifying it binds to `pcloud-plugin-api` correctly and rejects malformed configs.

### M-3. No fuzz target for HTTP / JSON response decoding pipeline end-to-end

**Severity:** MEDIUM
**Files:** `crates/pcloud-proto/fuzz/fuzz_targets/` has `fuzz_response_parser.rs`, `fuzz_json_response.rs`, `fuzz_listfolder_response.rs` — these target the parser layer but not the *decode-then-dispatch* chain. No fuzz target for the HTTP body framing → decompress → JSON → typed-response chain.
**Risk:** A maliciously crafted server response (or MITM payload, even with TLS-pinned cert if cert validation has a gap) could exploit the pipeline; per-stage fuzzers do not catch interaction bugs.
**Remediation:** Add `fuzz_http_response_pipeline.rs` that drives a fake HTTP body through the full stack including content-encoding (gzip bomb defense per Section 2 of the master prompt).

### M-4. No fuzz target for crypto `seal`/`open` round-trip with adversarial AAD/tweak

**Severity:** MEDIUM
**Files:** `crates/pcloud-crypto/fuzz/fuzz_targets/` has `fuzz_open_sector.rs` (decryption-only) and `fuzz_pclsync_filename_decode.rs`. No symmetric `fuzz_seal_open_roundtrip.rs` that proves adversarial inputs cannot trigger key-reuse, nonce reuse, or auth-tag-equivocation bugs.
**Risk:** AEAD bugs typically manifest as "encryption succeeds but the corresponding open fails on a malicious adversarial setup" — single-direction fuzzers miss.
**Remediation:** Add a symmetric round-trip fuzzer that randomizes plaintext, AAD, and sector index; assert open(seal(x)) == x and open(seal(x)) under a tampered tag/AAD == AuthError.

### M-5. No bench target for IPC throughput (only codec)

**Severity:** MEDIUM
**File:** `crates/pcloud-ipc/benches/ipc_codec.rs` only.
**Evidence:** Codec micro-bench but no end-to-end client→server→response throughput, no concurrent-clients throughput. Stress test exists at `tests/stress_concurrent_clients.rs` but is functional, not throughput-tracking.
**Risk:** IPC perf regressions (e.g., serialization-loop slowdowns under concurrency) are invisible.
**Remediation:** Add `benches/ipc_throughput.rs` (criterion) measuring single-client and 10-client req/sec.

### M-6. No bench for cache eviction or sync-engine queue under contention

**Severity:** MEDIUM
**Files:** `crates/pcloud-cache/`, `crates/pcloud-engine/benches/engine.rs`.
**Evidence:** `engine.rs` exists but has 1 bench function; no contention or queue-depth scaling test. Cache crate has no benches dir.
**Risk:** Sync engine scalability cannot be proven without a contention bench. Cache eviction strategy regressions (e.g., LRU → MRU) only show up at scale.
**Remediation:** Add `pcloud-engine/benches/queue_contention.rs` (N producers / 1 consumer, then 1/N) and `pcloud-cache/benches/eviction_under_pressure.rs`.

### M-7. Tests rely on `thread::sleep` without polling — flake risk

**Severity:** MEDIUM
**Files:** `crates/pcloud-daemon/tests/sync_loop_e2e.rs:65,84,105,129,134,140,158`; `crates/pcloud-daemon/tests/graceful_drain.rs:137,191,240`; `crates/pcloud-fs/tests/mount_transport_wiring.rs:114,190,233,238,259`; `crates/pcloud-fs/tests/fuse_dyn_shim_write.rs:144`; `crates/pcloud-fs/tests/fuse_kernel_e2e.rs:269`.
**Evidence:** `std::thread::sleep(Duration::from_millis(50))` / 100 / 200 / 500 used as synchronization in several places.
**Risk:** Slow CI runners (especially the FreeBSD VM, Windows-latest, the macOS budget tier) flake; flakes get masked by retries, masking real regressions.
**Remediation:** Replace blind sleeps with bounded poll loops (`loop { check(); if ok { break } }` capped at e.g. 10s) or with explicit channel/handshake signals.

### M-8. `panic!` in live-test helpers can mask infrastructure issues

**Severity:** MEDIUM
**Files:** `crates/pcloud-live-e2e/tests/sync_loop_live.rs:` "live sync loop did not complete a cycle within 60s"; `tests/backup_lifecycle.rs:` "could not create scratch folder for CreateBackup root_folder_id" (×2); `tests/shares_a_to_b.rs:` "A CreateRemoteFolder response had no folder_id".
**Evidence:** These `panic!` calls fire on legitimate setup failures (network blip, transient API 5xx) — inside `#[ignore]`-gated tests they will fail the live-e2e CI run silently because the job is `continue-on-error: true` (H-1) but locally they obscure whether the test is broken or the precondition is broken.
**Risk:** A real bug (sync loop *broken*, not slow) reads identically to a flaky precondition (account quota hit); the test cannot tell you which.
**Remediation:** Convert top-of-test infrastructure failures to `eprintln!("[live-e2e] skipping: ...")` + `return;` (matches the pattern already used at `auth_lifecycle.rs:36-39`). Reserve `panic!` for assertion of the actual feature under test.

### M-9. Cache files / kms / session / p2p / idp have inline tests but no integration tests

**Severity:** MEDIUM
**Crates:** `pcloud-kms` (12 inline, 0 integration), `pcloud-policy` (7 inline, 0 integration), `pcloud-idp` (17 inline across 5 files, 0 integration), `pcloud-session` (9 inline, 0 integration), `pcloud-p2p` (16 inline, 0 integration), `pcloud-plugin-api` (23 inline, 0 integration).
**Evidence:** `find` output shows zero `tests/` dir or empty `tests/` dir for each.
**Risk:** Cross-module behaviour (e.g., session → KMS → IDP token-refresh handshake) untested.
**Remediation:** Add at minimum one integration test per crate exercising the cross-crate contract.

---

## LOW

### L-1. `cargo deny` is integrated into CI but `mdbook` build does not check broken links

`ci.yml:37-40` and `ci.yml:157-168`. Add `mdbook-linkcheck` invocation.

### L-2. Reproducible-build check is digest-only, not bit-by-bit diff

`ci.yml:108-153`. Sufficient for now; consider adding `diffoscope` artifact upload on mismatch.

### L-3. Coverage `--ignore-filename-regex '(live_e2e|fuzz|benches)'` excludes test scaffolding but not generated code

`ci.yml:341`. Audit whether any auto-generated proto/IDL code is double-counted.

### L-4. `cargo bench` is never run in CI

No workflow runs benches. Perf regressions invisible. Suggest a weekly `criterion` run with comparison to baseline.

### L-5. No Miri / sanitizer job in CI

Given workspace contains `unsafe` blocks (FFI in `pcloud-fs/src/platform/macos_ffi.rs`, `winfsp_ffi.rs`), an `xtask miri` job over the safe-Rust crates would be cheap insurance.

### L-6. `pcloud-secret` proptest target is excellent — replicate the pattern elsewhere

`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs` proves zeroize-on-drop holds across N random sizes. Apply the same harness to `pcloud-crypto::SecretKey` types.

---

## Per-crate test inventory

| Crate | Inline `#[test]` | Tests/ | Integration | Live-E2E | Bench | Proptest | Fuzz |
|---|---:|---:|:-:|:-:|:-:|:-:|:-:|
| pcloud-auth | 26 | 0 | Y | N | N | N | N |
| pcloud-backends | 174 | 13 | Y | N | N | N | N |
| pcloud-cache | 10 | 0 | N | N | N | N | N |
| pcloud-chaos | 0 | 4 | Y | (chaos) | N | N | N |
| pcloud-cli | 228 | 19 | Y | N | N | N | N |
| pcloud-compat | 23 | 0 | Y | N | N | N | N |
| pcloud-config | 99 | 0 | Y | N | N | N | N |
| pcloud-crypto | 174 | 43 | Y | (kat-live) | Y | Y | Y (2) |
| pcloud-daemon | 227 | 104 | Y | (live_auth, macos_pcloud_live) | Y (3) | Y | Y (1) |
| pcloud-daemon-win | 0 | 0 | N | N | N | N | N |
| pcloud-engine | 110 | 0* | Y (engine_basics) | N | Y | N | N |
| pcloud-error | 0 | 0 | Y | N | N | N | N |
| pcloud-fleet | 10 (inline lib) | 0 | Y | (live_mtls) | N | N | N |
| pcloud-fs | 274 | 115 | Y (many) | (kernel_e2e, write_path_live, winfsp_mount_live, macos_mount_live) | Y (3) | N | N |
| pcloud-idp | 17 | 0 | N | N | N | N | N |
| pcloud-ipc | 27 | 48 | Y (envelope, peer_and_protocol, security_invariants, request_size_cap, stress) | N | Y | Y | Y (1) |
| pcloud-kms | 12 (inline lib) | 0 | N | N | N | N | N |
| pcloud-live-e2e | 0 | 27 | Y | YES (21 flows) | N | N | N |
| pcloud-mockserver | 0 | 0 (lib); tests/mock_flows | Y | N | N | N | N |
| pcloud-model | 22 | 0 | N | N | N | N | N |
| pcloud-observability | 43 | 0 | Y | N | N | N | N |
| pcloud-p2p | 16 (lib) | 0 | N | N | N | N | N |
| pcloud-plugin-api | 23 | 0 | N | N | N | N | N |
| pcloud-plugin-autoheal | 0 | 0 | Y | N | N | N | N |
| pcloud-plugin-backup-schedule | 0 | 0 | N | N | N | N | N |
| pcloud-plugin-dlp | 9 | 0 | N | N | N | N | N |
| pcloud-plugin-publink-expiry | 0 | 0 | N | N | N | N | N |
| pcloud-policy | 7 | 0 | N | N | N | N | N |
| pcloud-proto | 211 | 35 | Y | N | Y | Y | Y (7) |
| pcloud-resilience | 70 | 0 | Y (engine_basics, circuit_breaker_proptest) | N | N | Y | N |
| pcloud-sdk | 49 | 0 | Y | N | Y | N | N |
| pcloud-secret | 0 | 22 | Y (redaction_and_zeroize, proptest_zeroize_invariants) | N | Y | Y | N |
| pcloud-session | 9 | 0 | N | N | N | N | N |
| pcloud-store | 34 | 0 | Y | N | Y | N | N |
| pcloud-web | 11 | 8 | Y | N | N | N | N |

*Note: enumeration via `grep -cE` may slightly differ from initial coarse grep; counts above use the more accurate per-file count.*

---

## CI matrix: platform × subsystem

| Platform | build | lib tests | integration tests | live e2e | bench | mount (FUSE/WinFSP) |
|---|:-:|:-:|:-:|:-:|:-:|:-:|
| Linux ubuntu-latest | Y | Y | Y | Y (weekly, non-gating) | N (no bench job) | Y (kernel_e2e in unit suite) |
| macOS macos-latest | Y | Y (workspace minus pcloud-fs) | Y (pcloud-fs --lib + 3 mock files) | N | N | N (deferred — no fuse-t in runner) |
| Windows windows-latest | Y | Y (workspace minus pcloud-fs) | partial (no `--tests` flag wide WinFSP) | N | N | N |
| FreeBSD vmactions | Y | Y (--exclude pcloud-fs, continue-on-error) | partial | N | N | N |

Tier-1 claim is honored only on Linux. CLAUDE.md correctly notes Windows is Tier-2 and BSD is Tier-3 — but `STATUS.md` style narrative should mirror that explicitly in user-facing surface (see H-5/H-6/H-7).

---

## Live-E2E gap table

| Parity row / capability | Live test? | Gap severity |
|---|:-:|:-:|
| Auth password / token / userinfo / logout | YES — `auth_lifecycle.rs` | — |
| Auth session-status payload | YES — `auth_lifecycle.rs::live_session_status_payload_is_non_empty` | — |
| Auth vault permission opt-in | YES — `auth_lifecycle.rs::live_vault_permissions_after_persistence_opt_in` | — |
| TFA code submission (`Method::TfaSubmit`) | NO | HIGH |
| TFA recovery code | NO | HIGH |
| TFA SMS resend / device-notify resend | NO | HIGH |
| `verify_email`, `verify_email_restricted`, `lost_password`, `change_password`, `register`, `get_promo`, `set_language`, `set_api_server` | NO | HIGH (claim "Implemented and live-verified" in CLAUDE.md is unsupported) |
| Crypto setup/start/stop/lock/unlock/mkdir | YES — `crypto.rs::live_crypto_setup_unlock_status_mkdir_lock` | — |
| `change_crypto_pass` | NO (body is `todo!()`) | HIGH (H-3) |
| Crypto share/team-share temppass (rows 124, 142) | NO | HIGH |
| Public link CRUD + changepublink | YES — `public_links.rs::live_public_link_lifecycle` | — |
| Tree-link from paths | YES — `tree_link_from_paths.rs` | — |
| `psync_send_publink` (row 42) | NO | MEDIUM |
| Shares folder invite/accept/decline/cancel | YES — `shares*.rs` | — |
| Share active visibility A→B | YES — `shares_active_a_to_b.rs` | — |
| Backup create / delete / stop device | YES — `backup_lifecycle.rs` | — |
| Sync root add/list/remove all flavors | YES — `sync_roots.rs` | — |
| Sync loop processing | YES — `sync_loop_live.rs` | — |
| Transfers upload/download round-trip | YES — `transfers.rs::live_upload_download_roundtrip` | — |
| `upload_writefromfile` server-side copy (row 93, Partial) | NO | HIGH |
| Mount Linux readdir/cat/unmount | YES — `mount_linux.rs` | — |
| Mount macOS lifecycle | NO (deferred per `ci.yml:353`) | HIGH (H-6) |
| Mount Windows lifecycle | NO | HIGH (H-5) |
| Drain (graceful shutdown) | YES — `drain.rs` | — |
| Rate limiter | YES — `rate_limit.rs` | — |
| Integrity sweeper | YES — `integrity_sweeper.rs` | — |
| Snapshot pipeline + GFS prune | YES — `snapshot_pipeline.rs`, `snapshot_prune.rs` | — |
| Fleet mTLS heartbeat | YES — `fleet_mtls.rs` | — |
| Field selectors | YES — `field_selectors.rs` | — |
| Windows liveness probe | YES — `windows_liveness.rs` | — |

---

## Remediation priority

1. (H-1, H-3, H-4) Make CI honest: gate `live-e2e` and `coverage` once stable; replace `change_crypto_pass` `todo!()` with a real test (or move to a separate `bd-1du.10` blocker with explicit "missing — automation channel TBD").
2. (H-2) Author the missing live-e2e modules (TFA, account utility, `upload_writefromfile`, team-share temppass) — unblocks the CLAUDE.md "live-verified" claim for retained rows.
3. (H-5, H-6, H-7) Stand up macOS self-hosted runner with fuse-t and Windows runner with WinFSP. Drop `--exclude pcloud-fs` once mount tests can run; add Windows `--tests` invocation to catch named-pipe accept-loop regressions when that work lands.
4. (M-3, M-4) Add the two missing high-value fuzz targets.
5. (M-5, M-6) Add IPC throughput and engine queue contention benches; consider scheduling them weekly with criterion comparison.
6. (M-7, M-8) Replace blind `thread::sleep` with bounded polling; convert live-test infra `panic!`s to skip-with-eprintln.
7. (L-4, L-5) Add weekly bench-comparison job and Miri job over safe-Rust crates.

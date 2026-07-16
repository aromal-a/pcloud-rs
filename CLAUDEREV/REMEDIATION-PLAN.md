# CLAUDEREV Remediation Plan

Date: 2026-04-30
Source: deferred findings across CLAUDEREV iter-1..iter-5 (`CLAUDEREV/iter-5-summary.md` — "What remains open").
Driver: user instruction "/loop 3m over the plan … until all issues are fixed, all remediations are in and all refactors are complete and thoroughly tested and documented".

This plan is **the single source of truth for the loop**. Each cron fire picks the next unfinished item, executes the fix in-tree, verifies via the standard tooling, and updates `CLAUDEREV/REMEDIATION-PROGRESS.md`. Items are ordered by `(severity, blast-radius-of-fix, dependency-chain)`.

A separate Out-of-Scope section captures items that genuinely cannot be closed from a single Linux host (Windows compile loop, hardware-attached mount verification, signed package distribution, canonical KAT capture from a real C client, human reviewer sign-off). The loop will skip those rather than thrash.

---

## Phase 1 — Critical & quick wins

### P1.1 — `FUSE-C-1` Windows reaper unwired (CRITICAL)
- Files: `crates/pcloud-fs/src/platform/windows.rs:1931-2138` (reaper registry already exists), `mount_with_winfsp_dyn` (the only production WinFSP entry point).
- Fix: call `reaper::register_mount` from `mount_with_winfsp_dyn` before returning the handle; add `unregister_mount` in the handle's `Drop`. Pattern is fully proven on Linux.
- Verification: `cargo check -p pcloud-fs --target x86_64-pc-windows-msvc` (cross-target) — host limitation may force a `cfg(windows)` syntactic check only.
- Acceptance: code compiles under `--target x86_64-pc-windows-msvc`; reaper is reachable from production WinFSP path.

### P1.2 — `dim 12 DELTA-MEDIUM-3-1` deployment-guide.md orphan (MEDIUM)
- Files: `docs/book/src/operations/deployment-guide.md` (557-line orphan), `docs/book/src/SUMMARY.md`.
- Fix: choose between (a) link the orphan into SUMMARY.md replacing the older `deployment.md`, or (b) merge content + delete the orphan. The user's intentional SUMMARY edit shows they kept the existing `deployment.md` line — pick option (b): merge any content from `deployment-guide.md` into `deployment.md` and delete the orphan.
- Verification: `find docs/book/src -name "deployment-guide.md"` returns empty; `mdbook build` (if installed) clean.
- Acceptance: no orphan files under `docs/book/src/`; SUMMARY-referenced files all exist.

### P1.3 — Lowest-hanging rustdoc warnings (MEDIUM, 8/49)
Target the 8 single-warning sites that are in already-touched files (`pcloud-fs::write_path.rs`, `pcloud-fs::metadata_cache.rs`, `pcloud-engine::lib.rs:777,810,828`, `pcloud-engine::divergence_sweeper.rs`, `pcloud-ipc::methods.rs:1023`).
- Fix: convert private-item intra-doc links to plain code spans; fix `crate::platform::windows::*` cross-cfg links by gating with `#[cfg(windows)]` doc-only attributes.
- Verification: `cargo doc --workspace --no-deps 2>&1 | grep "generated [0-9]+ warning"`. Target: total ≤ 41.
- Acceptance: total warning count reduces by ≥8.

### P1.4 — 27 `unsafe` blocks lacking `// SAFETY:` (MEDIUM)
- Files: across `crates/pcloud-fs/src/platform/{macos,windows,bsd,linux,winfsp_ffi}.rs`, `crates/pcloud-cli/src/{prompt,main}.rs`, `crates/pcloud-compat/src/shm_producer.rs`.
- Fix: add a `// SAFETY:` comment with the actual invariant for each block. Where the rationale lives at module level, add a one-liner pointing back to it.
- Verification: tooling spot-check (`grep -B 4 "unsafe " <file>` shows `// SAFETY:` in the 4-line preceding window).
- Acceptance: 0 unsafe blocks lacking a SAFETY comment workspace-wide (or each remaining one has a documented exemption).

---

## Phase 2 — Security hardening

### P2.1 — `SEC-H-1..H-3` four SecretString migrations (HIGH ×3)
- Files: `crates/pcloud-proto/src/auth_api.rs`, `crates/pcloud-proto/src/account_api.rs`, `crates/pcloud-ipc/src/methods.rs`, `crates/pcloud-web/src/config.rs` (the 4 sites flagged by iter-1 dim 2).
- Fix: change bearer-credential `String` fields to `pcloud_secret::SecretString`; update serde derives if needed; ensure `Debug` redacts.
- Verification: `cargo check --workspace --all-targets`; `cargo test -p pcloud-ipc proptest_methods_roundtrip` (the IPC wire-shape regression suite); manual `Debug` output check.
- Acceptance: 0 raw `String` credential fields on long-lived structs; existing tests pass; `Debug` output is redacted.

### P2.2 — `SEC-H-4` TLS revocation default-on (HIGH)
- Bead: `pcloud-rs-t9o` (existing live tracker entry).
- Files: `crates/pcloud-proto/src/builder.rs` or wherever `reqwest::Client` is built; `crates/pcloud-config/src/api.rs` for the new opt-out flag.
- Fix: wire CRL/OCSP via rustls's `webpki` revocation API (or document why a different default was chosen); add `--insecure-no-revocation` opt-out CLI flag.
- Verification: ad-hoc test that builder rejects a known-revoked test cert.
- Acceptance: production builds revoke-check by default; opt-out is explicit.

### P2.3 — `IPC-H-7.1` privileged-Request capability tier (HIGH)
- Files: `crates/pcloud-daemon/src/dispatch.rs`; `crates/pcloud-ipc/src/methods.rs`.
- Fix: replace audit-only `is_privileged_request` with a typed `RequiresPrivilege` flag on each `Request` variant; deny by default; require an explicit elevated-peer proof. Audit log retained.
- Verification: existing IPC integration tests + new test asserting non-privileged peer can't `Shutdown` / `CryptoReset` / `AccountChangePassword`.
- Acceptance: per-Request capability table is authoritative; audit log entries no longer carry the only-enforcement role.

---

## Phase 3 — Parity closure (IPC variants + crypto wiring)

### P3.1 — `iter-2 H-4` three public-link IPC variants (HIGH ×3)
- Rows: 147 (`psync_folder_public_link_full`), 148 (`psync_folder_updownlink_link`), 168 (`psync_screenshot_public_link`).
- Files: `crates/pcloud-ipc/src/methods.rs` (add `Request::CreateFolderPublicLinkWithOptions`, `Request::CreateFolderUpDownLink`, `Request::CreateScreenshotPublicLink`); `crates/pcloud-daemon/src/dispatch.rs` (add 3 dispatch arms wiring through `PublicLinkRuntime::*`); `crates/pcloud-cli/src/commands.rs` (add 3 CLI subcommands or argument shapes); plus serde-bincode roundtrip update for the new variants.
- Verification: `cargo test -p pcloud-ipc proptest_methods_roundtrip`; manual CLI smoke against a mock daemon.
- Acceptance: rows 147/148/168 flip from Partial → Implemented in `C_FEATURE_PARITY_MATRIX.csv` with code citations; STATUS.md tally updated to `152/4/0/30`.

### P3.2 — `iter-2 H-5` `Request::CryptoShareFolder` IPC variant (HIGH)
- Row: 138 (`shares,psync_crypto_share_folder` duplicate row).
- Files: `crates/pcloud-ipc/src/methods.rs`; `crates/pcloud-daemon/src/dispatch.rs`; `crates/pcloud-cli/src/commands.rs` if applicable; routes through `SharesRuntime::crypto_share_folder` / `crypto_share_folder_rsa`.
- Verification: same as P3.1.
- Acceptance: row 138 flips to Implemented (or merges with 124).

### P3.3 — `CRYPTO-H-2` `derive_temppass_wire` RSA-OAEP wiring (HIGH)
- Files: `crates/pcloud-crypto/src/share_temppass.rs:343-345` (currently returns `RsaBackendRequired`); `crates/pcloud-crypto/src/share_rsa.rs::wrap_share_invitation_b64` (already public + wired through shares-backend).
- Fix: replace the `RsaBackendRequired` early-return with a call to `share_rsa::wrap_share_invitation_b64` for the `CryptoBackend::PclsyncCompat` path.
- Verification: existing `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs` should now exercise this path; add a unit test for the `derive_temppass_wire` PclsyncCompat branch.
- Acceptance: rows 124/142 — once the live two-account E2E is captured (out-of-scope for this loop) — flip to Implemented; for now, the symbolic blocker is removed.

### P3.4 — `CRYPTO-H-3` AES-ECB Merkle parent-tag step (HIGH)
- Files: `crates/pcloud-crypto/src/pclsync_auth_tree.rs` (header admits the AES-256-ECB step is absent).
- Fix: insert the AES-256-ECB step on parent-tag computation per `docs/crypto-reference-pclsync.md`.
- Verification: capture-or-construct a multi-sector test vector against the C client (offline tool); add KAT test.
- Acceptance: multi-sector files written by C clients verify byte-identical at the master tag under Rust.

---

## Phase 4 — Resilience & sync

### P4.1 — `TRANSPORT-H-1` route production HTTP through `ResilientTransport` (HIGH)
- Files: `crates/pcloud-daemon/src/runtime.rs` (transport composition); `crates/pcloud-proto/src/builder.rs`; `crates/pcloud-resilience/src/transport.rs`.
- Fix: wrap the production `BinaryApiTransport` in `ResilientTransport` so circuit-breaker / retry-budget / token-bucket are reachable.
- Verification: existing resilience tests + a new integration test that observes the circuit-breaker opening on a forced-503 mock.
- Acceptance: every API call site goes through `ResilientTransport`; no raw `reqwest::Client::get` in production paths.

### P4.2 — `SYNC-H-04-1` fs_watcher overflow telemetry + recovery scan (HIGH)
- Files: `crates/pcloud-engine/src/fs_watcher.rs`.
- Fix: count dropped events; emit a metric; on N drops trigger a full-tree rescan.
- Verification: unit test that injects an overflow.

### P4.3 — `SYNC-H-04-2` replace hand-rolled debouncer (HIGH)
- Fix: swap to `notify-debouncer-full`.
- Verification: existing engine tests + a churn-stress test.

### P4.4 — `SYNC-H-04-3` macOS / Windows battery awareness (HIGH)
- Fix: feature-flag `battery` crate or platform-specific FFI; populate the existing `power_state` reader trait on those platforms.
- Verification: unit tests with mocked power state.

### P4.5 — `SYNC-H-04-4` case-insensitive collision detection (HIGH)
- Fix: activate the existing unused `probe_case_insensitive_fs` helper; reject conflicting filenames at sync time on macOS/Windows.
- Verification: unit test with HFS+/NTFS-shaped collision.

### P4.6 — `SYNC-H-04-5` `SQLITE_BUSY` retry (HIGH)
- Fix: wrap the short-lived store facade in a busy-retry loop with exponential backoff.
- Verification: integration test with concurrent writers.

### P4.7 — `SYNC-H-04-6` integrity_sweeper unwrap audit (HIGH)
- Files: `crates/pcloud-daemon/src/integrity_sweeper_service.rs` (~50 unwrap sites flagged by iter-1).
- Fix: replace each `.unwrap()` / `.expect()` with proper error propagation.
- Verification: clippy `unwrap_used = "deny"` clean for that file; unit tests cover the new error paths.

---

## Phase 5 — Testing & CI

### P5.1 — `TEST-H-1` remove `continue-on-error` from live-e2e CI (HIGH)
- File: `.github/workflows/*.yml` (live-e2e job).
- Fix: drop the `continue-on-error: true`. If account flakiness is the gate, document the precise mitigation (rate-limit budget, retry policy, soak account provisioning).
- Verification: CI run on a PR observes the gate fires.

### P5.2 — `TEST-H-2` live coverage for retained-but-unreached parity rows (HIGH)
- Suites to add: TFA (rows 19-22), account utility (verify_email, lost_password, etc.), `upload_writefromfile` (row 93 — needs P3 work first), team-share temppass (row 142).
- Files: `crates/pcloud-live-e2e/tests/`.
- Acceptance: each retained Implemented row has at least one live-gated test path.

### P5.3 — `TEST-H-3` replace `change_crypto_pass` `todo!()` (HIGH)
- File: `crates/pcloud-live-e2e/tests/crypto_change_password.rs` (or equivalent).
- Fix: implement the test body; it has been a stub since iter-1.
- Acceptance: test runs (live-gated) and passes.

### P5.4 — `TEST-H-4` coverage CI threshold (HIGH)
- Fix: gate `cargo llvm-cov` to fail below a documented threshold (e.g., 60% line coverage workspace-wide, 75% on critical crates).
- Verification: PR can't merge below threshold.

### P5.5 — `TEST-H-5/6/7` cross-platform CI inclusion of `pcloud-fs` (HIGH ×3)
- Files: `.github/workflows/macos.yml`, `.github/workflows/windows.yml`, `.github/workflows/freebsd.yml`.
- Fix: stop excluding `pcloud-fs` from those jobs; or honestly downgrade Tier-1 → Tier-2 in CLAUDE.md and the parity dossier until the build-out lands.
- Acceptance: either CI runs `pcloud-fs` on those platforms, or the docs accurately reflect the actual coverage.

---

## Phase 6 — Deploy / Ops

### P6.1 — `DEPLOY-H-11.2` `.deb` / `.rpm` package signing in CI (HIGH)
- Files: `.github/workflows/release-packaging.yml`.
- Fix: add gpg-sign step + checksum publication. Sign with the project release key.
- Verification: artifact upload in a tagged release shows `.sig` files.

### P6.2 — `DEPLOY-H-11.4` `CryptoPolicy::fips_mode` runtime gate (HIGH)
- Decision: either implement or remove the FIPS claim from any forward-looking docs.
- Fix path A (implement): add a `fips_mode` flag to `CryptoPolicy`; route AES/HMAC through a FIPS-validated provider when set; reject Argon2id (not FIPS).
- Fix path B (remove claim): scrub any "FIPS" mentions from `docs/`, `enterprise/`, marketing surfaces.
- Acceptance: claim and code agree.

### P6.3 — `DEPLOY-H-11.1` Windows MSI service ← deferred until Windows host
- Cross-host work; will be skipped by the loop.

---

## Phase 7 — pcloud-cache duplication

### P7.1 — `dim 5 NEW-1` pcloud-cache vs pcloud-fs page-cache (MEDIUM)
- Files: `crates/pcloud-cache/src/page_cache.rs` (newer parking_lot/LinkedHashMap rev), `crates/pcloud-fs/src/page_cache.rs` (older single-mutex + intrusive list).
- Fix: pick `pcloud-cache::PageCache` as canonical; route all `pcloud-fs` callers through it; delete `pcloud-fs/src/page_cache.rs`.
- Verification: `cargo check --workspace`; `cargo test -p pcloud-fs`; symbol `PageCache` resolves to a single canonical implementation.
- Acceptance: one canonical page-cache primitive workspace-wide.

---

## Out-of-scope for this loop

These items genuinely cannot be closed from a single Linux host without external action:

| Finding | Why out of scope |
|---|---|
| Hardware-attached macOS / Windows live mount verification | Requires real Darwin / Windows hardware |
| `CRYPTO-H-1` cross-interop KAT for Enhanced backend | Requires capturing canonical ciphertext from a real pCloud C client |
| Signed `.deb` / `.rpm` distribution channel | Requires release-key infrastructure decision (out of code scope) |
| Apple Developer notarisation | Requires Apple Developer account |
| Windows Authenticode EV signing | Requires EV hardware token |
| Human reviewer sign-off for `bd-1du.10` closure | Out of AI scope |

The loop will explicitly skip these rather than thrash. Each will be marked `[OUT-OF-SCOPE]` in `CLAUDEREV/REMEDIATION-PROGRESS.md` after one inspection turn.

---

## Operating model

Each cron fire:
1. Reads `CLAUDEREV/REMEDIATION-PROGRESS.md` to find the next unfinished item (in-order, top-down through this plan).
2. Picks one in-scope item; if it would take more than ~30 minutes of agent work or requires a multi-crate refactor, decompose first.
3. Executes the fix.
4. Verifies via the acceptance criteria.
5. Updates `CLAUDEREV/REMEDIATION-PROGRESS.md` with: item ID, files touched, verification commands run, observed output.
6. If everything in this plan is done (Phases 1–7 complete and out-of-scope items acknowledged), call `CronDelete` to stop the loop and write `CLAUDEREV/REMEDIATION-COMPLETE.md`.

Verification baseline (must hold across every fire):
- `cargo check --workspace --all-targets` exit 0
- `cargo fmt --all --check` exit 0
- `cargo deny check` clean
- `cargo doc --workspace --no-deps` warning count monotonically non-increasing

If a fire would break the baseline, **the fire reverts its own changes** and logs the regression for analysis on the next fire.

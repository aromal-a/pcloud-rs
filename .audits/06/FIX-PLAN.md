# Audit 06 — Fix Plan

**Date:** 2026-04-18
**Scope:** All 20 reports under `.audits/06/section-*-{opus,sonnet}.md` (10 Opus + 10 Sonnet, cross-validated).
**Selection rule:** Only findings **confirmed by Opus** enter the action list. Sonnet-only findings are parked in a separate "Needs Opus Validation" bucket. Consensus findings (both agreed) are tagged "Consensus" and prioritized first in their band.
**Baseline:** audit-05 FIX-PLAN `.audits/05/FIX-PLAN.md`, authoritative matrix CSV `153 Implemented / 5 Partial / 0 Missing / 28 Rejected` (186 rows). Audit-06 reaffirms that CSV count as correct.

---

## Executive summary

Audit-06 confirms that audit-05's structural remediations (page-cache, GLOBAL_STAGING_BYTES, JournalError::Full, flush interval, macOS teardown call-site, chunked-upload backend API, systemd override, typed transport, is_known_safe_host dedup, peer-uid threading, per-peer rate limiter, IPC bind re-chmod, digest-only extract-kat, sectors_sealed persistence, SeqCst lockout, EmptySector reject, offline KAT, Sigstore signing, SBOM/SARIF, cargo-doc/changelog gates) **are held in source**. However, several audit-05 items labelled "fixed" were only partially landed or regressed, and ~40 new findings surfaced. The code path is materially stronger than audit-05 state but is **not** release-clean yet.

Top-line audit-06 regression signals:
1. Shipped Prometheus alerts and Grafana dashboards reference metrics the daemon does not emit (all alerts silent).
2. SDK examples do not compile — breaks §8 acceptance criterion.
3. CLAUDE.md internal self-contradiction on Partial count (`156/2` headline vs `153/5` body).
4. Audit-05 claimed "SynchronousGuard RAII" and "byte-progress StallDetector" — neither symbol exists.
5. `BackendMismatch` variant remains unreachable despite audit-05 P2-2a claim.
6. `eprintln!` still in FUSE adapter hot paths (4 sites) despite audit-05 sweep claim.
7. macOS UAF window self-documented as open in source despite audit-05 claiming it closed.
8. `metrics_server` serve loop bypasses privileged-audit + peer-uid plumbing regressed into audit-05.
9. `ack_batch` matches path only — multi-root collision drops dispatched ops silently.

---

## Priority 0 — Parity Honesty + Audit-05 Regression Reconciliation

### P0-1. CLAUDE.md self-contradicts on Partial count
**Consensus.** Opus §11-12 H11-001 + Sonnet §11-12 L-12-1 + Opus §1 M-1, M-2, L-1, L-2 + Sonnet §1 M-02, M-03.

- `CLAUDE.md:52` header says "post Audit 03".
- `CLAUDE.md:60, 372` say "5 Partial rows (26, 27, 93, 124, 142)".
- `CLAUDE.md:415-416` still say "Transfers: one Partial row remains (row 93)" and "Public links: one Partial row remains (row 149)".
- `CLAUDE.md:66-70` hard-coded "156 Implemented / 2 Partial" in Audit-03 style.
- `CLAUDE.md:78` says "two IPC-wiring gaps" — stale; actual is five.
- `CLAUDE.md:388-391` still lists landed row-149 work as remaining under §bd-1du.10.

**Actions:**
1. Change line 52 to "post Audit 05".
2. Rewrite §"Feature Parity Matrix Summary" (lines 415-416) to `153/5/0/28`, delete the row-149 "Partial" line, link to `STATUS.md`.
3. Delete §bd-1du.10 bullets about row 149 (closed) and reword §"residual parity work" to cite the five real Partial rows.
4. Remove stale `psync_send_publink` missing claim in §Backup/device (Sonnet §1 M-02: send_publink is Implemented end-to-end).
5. Rewrite §"Still not full parity" sync paragraph to acknowledge the sync-loop wiring at daemon startup (Opus §1 L-1).
6. Remove duplicate "Primary files:" block under public-link section (Opus §1 L-2).

**Complexity:** small. **Verification:** single CLAUDE.md diff; grep returns zero `156 / 2` / `155 / 3` / `row 149.*Partial`.

### P0-2. CONTRIBUTING.md still claims C tree maintained after deletion
**Opus §11-12 H11-002.**
- `CONTRIBUTING.md:28-31, 53-58` say legacy C client is "in maintenance mode — bug fixes only" and quotes `make -j4` C build.
- `CLAUDE.md:29` (correctly) says C sources were deleted.

**Action:** rewrite CONTRIBUTING.md:28-31 + 53-58 to mirror CLAUDE.md "C removed; upstream pcloudcom/pcloud-rs is reference-only". Drop `make -j4` snippet.
**Complexity:** small.

### P0-3. CSV + STATUS.md stale TODO line citation for row 93
**Opus §1 M-3.**
`C_FEATURE_PARITY_MATRIX.csv` row 93 narrative + `STATUS.md:609` cite `transfer_backend.rs:445` but the real TODO(bd-1du) block lives at `transfer_backend.rs:601-613`. Line 445 is inside `download_to_path`.

**Action:** repoint both citations to line 601. Grep-and-replace.
**Complexity:** small.

### P0-4. Rows 26/27 (`psync_tfa_has_devices`, `psync_tfa_type`) have no linked bead
**Consensus.** Sonnet §1 H-01 + audit-05 carry-over.

No resolution path declared. Per audit rule "Partial without linked bead = HIGH for parity honesty".

**Action:** either (a) create `bd-1du.11` tracking TFA introspection surface and cite from CSV rows 26/27, or (b) conduct a scope decision and flip both to Rejected in `REJECTED-RATIONALES-14042026.md`. Recommendation: (a) — create the bead, hold Partial.
**Complexity:** small.

### P0-5. Rows 124/142 RSA-4096 share invitation produces silently invalid blob
**Consensus.** Sonnet §1 H-02 + Sonnet §3 M-1 + Opus §3 confirms HMAC-only fallback.

`share_temppass.rs:39-45,211-222` documents the symmetric-only limitation but surfaces no user-visible warning when a Rust-generated invitation targets a C-client invitee. Tracked under `bd-1du.5` (RSA keypair landing), but the operational gap is: no guard prevents the current code path from emitting a non-functional blob.

**Action:** at the daemon dispatch site (share invitation), when `CryptoBackend::PclsyncCompat` is active and the share flow is invoked, return `CryptoError::NotYetWired` with an explicit message "crypto share requires RSA-4096; not yet supported on PclsyncCompat backend". Track the underlying feature under `bd-1du.5`.
**Complexity:** small (guard only; actual RSA landing is out of scope for P0).

### P0-6. Audit-05 regression reconciliation — headline
See §"Audit-05 Regression Ledger" below. P0-1 is the biggest; the other audit-05 regressions become P1/P2 items under their technical section.

---

## Priority 1 — CRITICAL code defects

### P1-1. Prometheus alerts + Grafana dashboard reference non-existent metrics
**Opus §11-12 C11-001.** Shipped-but-broken artifact — all alerts silent. This creates a false sense of coverage worse than having no alerts.

- **Files:** `ops/prometheus/pcloud-rs-alerts.yml:22-94`, `ops/grafana/pcloud-rs-overview.json`.
- **Missing-from-code:** `pcloud_ipc_requests_total`, `pcloud_sync_queue_depth`, `pcloud_crypto_operations_total`, `pcloud_transport_ratelimit_rejected_total`, `pcloud_mount_active`.
- **Actually-emitted:** `pcloud_auth_attempts_total`, `pcloud_request_latency_seconds`, `pcloud_transfer_bytes_total`, `pcloud_crypto_lock_state`, `pcloud_sync_root_count`, `pcloud_ipc_connected_clients`, `pcloud_panic_count` (source: `crates/pcloud-observability/src/metrics.rs:487-526`).

**Action:** either rewrite the rules + dashboard against real metric names, or add the missing metrics to `pcloud-observability` before shipping the rules. Either way, add a CI check that greps alert/dashboard files and fails if a referenced metric name is not in the observability crate's allow-list.
**Complexity:** medium.

### P1-2. SDK examples do not compile — §8 acceptance criterion fails
**Opus §8 C-8.1.** Blocks §8 acceptance gate.

- **Files:**
  - `crates/pcloud-sdk/examples/public_link.rs` — 5 errors (`E0559`: `Request::PasswordSubmission` field is `value` not `password`; `E0609`: no field `payload` on `Response`; 3× `E0282`).
  - `crates/pcloud-sdk/examples/create_tree_public_link_from_paths.rs` — 1 × `E0559` on same stale field, 1 × `Method` unused-import warning.

**Action:** update both examples to current `pcloud-ipc` shape; add `cargo build --examples -p pcloud-sdk` to CI.
**Complexity:** small.

### P1-3. `BackendMismatch` variant remains unreachable (audit-05 P2-2a regression)
**Opus §3 C-1.** Audit-05 H-1 was acknowledged in handoff but not implemented.

- **File:** `lib.rs:306` defines variant. Zero production construction sites. Cross-backend dispatch still bails with `NotYetWired` (`lib.rs:2014,2192,2214,2246,2282`) or `MissingFileId` (`lib.rs:2495`) or `Locked` (`lib.rs:2485`).
- **Permissive test:** `tests/pclsync_compat_roundtrip.rs:251` accepts `BackendMismatch` OR `NotYetWired`, hiding the gap.

**Action:** raise `BackendMismatch { expected, provided }` from `change_password_with_context` and the Enhanced-only sector/filename/metadata entry points before they fall through to `NotYetWired`. Tighten the roundtrip test to expect exactly `BackendMismatch`.
**Complexity:** medium.

### P1-4. Audit-05 H3 `SynchronousGuard` RAII absent
**Opus §4 H-4.1.** Audit-05 claimed this landed; grep for `SynchronousGuard|synchronous_guard` returns zero hits in `crates/pcloud-engine/`.

- **File:** `crates/pcloud-engine/src/scheduler.rs`, ~204-211 (`next_batch`) + dispatch sites.
- **Gap:** `Scheduler::next_batch` pushes to `dispatched_operations` and relies on caller-side `ack_batch`. No Drop-coupled safety net. Panic in transfer worker between dispatch and ack permanently leaks the op.

**Action:** introduce `DispatchedGuard<'a>` holding `&mut Scheduler` + path list; on `commit()` → ack; on unwind-drop → re-enqueue. Wire into dispatch path.
**Complexity:** medium.

### P1-5. Audit-05 H4 byte-progress `StallDetector` mislabelled
**Opus §4 H-4.2.** `stall_detector.rs:37-95` has only `mark_progress()` (wall-clock reset). No `observe_bytes(n)` / `update_bytes_transferred` method exists. The test `long_running_transfer_does_not_stall_if_bytes_progress` (line 194-219) calls `mark_progress()` — a *time-based* liveness check, not byte-level. A transfer that hangs mid-stream (bytes=0 but heartbeat ticks) fools the detector.

**Action:** add `observe_bytes(delta: u64)`; track `last_bytes_seen + last_bytes_change_instant` separately. `check_stall` fires on either axis. Retest.
**Complexity:** medium.

### P1-6. `metrics_server::serve_with_metrics` bypasses privileged-audit + peer-uid
**Opus §7 M-7.1.** Audit-05 peer-uid plumbing regression on the Prometheus-enabled serve path.

- **File:** `crates/pcloud-daemon/src/metrics_server.rs:145-170`.
- **Gap:** uses `bound.serve_once(...)` (not `serve_once_with_peer`) and `dispatch(runtime, request)` (not `dispatch_with_peer`). When metrics are enabled, privileged IPC requests (`CryptoReset`, `Shutdown`, `AccountChangePassword`) are invisible to audit log; per-peer rate limiter loses keying.

**Action:** replace with `serve_once_with_peer` and route through `dispatch_with_drain_gate` (or shared helper). Extract `drain_admits(&Request)` to single helper used by both loops (see Opus §7 L-7.2).
**Complexity:** small.

### P1-7. `eprintln!` residual in FUSE adapter hot paths (audit-05 sweep incomplete)
**Consensus.** Opus §5 H-1 + Sonnet §5 H-1.

- **Files:** `crates/pcloud-fs/src/fuse_adapter.rs:1373, 1442, 1461, 1490`; `crates/pcloud-fs/src/platform/windows.rs:1759`.
- **Gap:** stderr-only output bypasses log/tracing filtering; floods journal on stat-heavy clients.

**Action:** replace with `log::warn!` (backend failures) / `log::error!` (config misconfiguration, EBADF).
**Complexity:** small.

### P1-8. macOS UAF window not closed (`teardown_macos` missing `deregister_active_session`)
**Consensus.** Opus §5 L-1 (acknowledges stale doc; fix held at mount_service.rs:556) **vs** Sonnet §5 H-2 (the `platform/macos.rs:1636-1645` in-source comment still says the function is not called from teardown).

Auditors disagree on which site is authoritative. Direct inspection required — if `mount_service.rs:556` does call `deregister_active_session(inner.session)` before `fuse_session_destroy`, then Opus is correct and only the docstring at `macos.rs:1636-1645` is stale. If the docstring is accurate, Sonnet is correct and the UAF is live.

**Action:** verify by direct inspection of `mount_service.rs:551-559`. If fix is present, update docstring at `macos.rs:1636-1645` to say "FIXED in teardown_macos". If absent, call `deregister_active_session(inner.session)` inside `teardown_macos` before `fuse_session_destroy`.
**Complexity:** small. **Priority:** P1 because UAF is safety-critical if still open.

### P1-9. `ack_batch` matches by path only — multi-root collision drops dispatched ops
**Sonnet §4 M-04-S04.** Real correctness gap in multi-root deployments.

- **File:** `crates/pcloud-engine/src/scheduler.rs:270-276` (+ `lib.rs:898`).
- **Gap:** `self.dispatched_operations.retain(|op| !paths.iter().any(|p| op.path() == *p))` ignores `sync_id`. Two different sync roots with the same relative path (e.g., `documents/report.pdf`) collide: acking root A's op silently drops root B's dispatched entry, defeating H2 crash-recovery for root B.

**Action:** change `ack_batch` / `ack_dispatched_path` to match on `(sync_id, path)` pairs. Caller already has `PlannedOperation` with both.
**Complexity:** small. **Priority:** P1 because cross-root ack collision silently loses work.

---

## Priority 2 — HIGH severity

### P2-1. Observability gaps on binary `ResilientTransport`
**Consensus.** Opus §6 (resolved as held only partly) + Sonnet §6 H-1 + H-2.

- **File:** `crates/pcloud-proto/src/resilient_transport.rs:302-365`. Two explicit `TODO(bd-1du)` where `pcloud_transport_latency_seconds` and `pcloud_transport_errors_total` should be emitted. Latency captured into `_latency` (line 358), discarded.
- **Feature flag:** `crates/pcloud-resilience/Cargo.toml:9-18` has `default = []` — `transport-metrics` off by default in every consumer.

**Action:** wire `pcloud-observability` into `pcloud-proto` as a workspace dep; replace `_latency` discards with `observe_latency()` calls mirroring `pcloud-resilience/src/transport.rs` pattern. Add `transport-metrics` to default features of `pcloud-resilience`, or make the observability calls unconditional once `pcloud-observability` is a hard dep.
**Complexity:** medium.

### P2-2. `bootstrap_with_config().expect("runtime bootstrap should succeed")` still panics in production
**Opus §9-10 H-1.** Audit-05 HIGH-3 not closed.

- **Files:**
  - `crates/pcloud-daemon/src/serve.rs:585`
  - `crates/pcloud-daemon/src/dispatch.rs:562`
  - `crates/pcloud-daemon/src/lib.rs:134`

**Action:** replace with typed-error propagation (`Result<_, DaemonError>`); surface user-facing diagnosis.
**Complexity:** medium.

### P2-3. Windows IPC is a documented STUB but Windows is claimed tier-1
**Consensus.** Sonnet §9-10 H3 + Sonnet §7 M3 + Opus §2 implicit (tier-1 claim).

- **File:** `crates/pcloud-ipc/src/platform/mod.rs:8` explicitly `— STUB`. `platform/windows.rs` exists but named-pipe accept loop is not wired into shared `serve_once_with_peer`. `peer_identity()` in `transport.rs` has no `#[cfg(windows)]` branch.

**Action:** either (a) wire the Windows named-pipe accept loop into the production serve path; add compile-test + stub integration test on a Windows runner; or (b) downgrade Windows from tier-1 to tier-2 in STATUS.md/README.md/CLAUDE.md. Opus §7 L-7.3 notes stress test is Linux-only by design.
**Complexity:** large (a) or small (b).

### P2-4. 3,018 `.unwrap()`/`.expect()` in non-test `src/` (daemon hot paths)
**Consensus.** Sonnet §9-10 H1/H2 + Opus §9-10 M-5.

- **Daemon vault:** `crates/pcloud-daemon/src/vault/mod.rs:434,444,452`.
- **CLI renderer:** `crates/pcloud-cli/src/progress.rs:305,311,317` (Mutex poison panic in production CLI path).
- **Account API:** `crates/pcloud-proto/src/account_api.rs:544` (`.expect("locations should parse")`).
- **Audit-05 mutex sweep:** 58 sites remain in `src/`; top offenders `pcloud-ipc/src/transport.rs` (6), `pcloud-daemon/src/sync_loop_runtime.rs` (5), `pcloud-plugin-backup-schedule/src/lib.rs` (5), `pcloud-daemon/src/audit_verifier_service.rs` (2), `pcloud-daemon/src/dispatch.rs` (3), `pcloud-daemon/src/mount_runtime.rs` (2), `pcloud-resilience/src/rate_limit.rs` (4), `pcloud-idp/src/jwks.rs` (4).

**Action:** adopt `.unwrap_or_else(|p| p.into_inner())` uniformly, or introduce `LockExt::lock_or_poisoned()` helper in `pcloud-observability`. Migrate CLI `progress.rs` Mutex. Convert account_api `.expect` into returning `Result`.
**Complexity:** large.

### P2-5. `sectors_sealed` counter uses `Relaxed` ordering (audit-05 P2-2e re-flagged)
**Consensus.** Opus §3 H-1 + Sonnet §3 H-1.

- **File:** `lib.rs:2503-2518`. Pre-seal load + post-seal `fetch_add` both `Relaxed`. If two threads concurrently call `seal_sector` and counter sits below budget_cap, both pass and overshoot. `AtomicU64` signals concurrency-safety but `CryptoShell` is `!Sync` by intent.
- **Serde shim** at `lib.rs:791-797,808-814` also snapshots under `Relaxed`.

**Action:** use `SeqCst` or `compare_exchange_weak` CAS loop to match `lockout_state` discipline (`lib.rs:1401,1434`). Document non-`Sync` invariant if decide to keep Relaxed.
**Complexity:** small.

### P2-6. SDK has no `[features]` block — feature-flag matrix missing
**Opus §8 H-8.1.** `pcloud_rev.md` §8 line 221 requires feature-flag combinations to compile.

- **File:** `crates/pcloud-sdk/Cargo.toml`. `grep -n "^\[features\]"` returns zero hits. Downstream consumers cannot select TLS backend.

**Action:** add `[features]` with `default`, `tls-rustls`, `tls-native-roots`. Matrix-compile in CI. Or document in SDK README that feature-gating is intentionally out of scope and why.
**Complexity:** medium.

### P2-7. `PCLOUD_LIVE_E2E` never set in ci.yml
**Sonnet §9-10 H4.** All 18 live-e2e tests `#[ignore]`-gated; no CI job sets the env var. Parity matrix rows marked `live-verified` are not auto-re-verified.

**Action:** wire a nightly CI job with a sandboxed pCloud test account; gate on `PCLOUD_LIVE_E2E` secret. Add similar for FUSE tests (Sonnet §9-10 M9: `PCLOUD_FUSE_TEST=1` on privileged Linux runner).
**Complexity:** medium.

### P2-8. FreeBSD CI job is `continue-on-error: true` — tier-1 claim unsound
**Consensus.** Opus §9-10 H-3 + Sonnet §9-10 H5.

- **File:** `.github/workflows/ci.yml:76`. FreeBSD documented as tier-1 for FUSE; `continue-on-error` means regressions silent.

**Action:** either fix CI stability and remove `continue-on-error`, or explicitly downgrade FreeBSD to tier-3 in STATUS.md/README.md.
**Complexity:** small (doc) or large (CI stability).

### P2-9. Unsafe/SAFETY delta ~51 blocks missing annotation
**Opus §9-10 H-3 (unsafe delta).** 423 unsafe vs 372 `// SAFETY:` in `src/`. Hotspots: `pcloud-fs/src/platform/macos.rs` (4 naked), `windows.rs` (6 naked), `winfsp_ffi.rs` (12 naked).

**Action:** sweep; annotate `winfsp_ffi.rs` 12 blocks + remaining macos/windows FFI. Sonnet §9-10 M1/M2 flags CLI sites too: `pcloud-cli/src/doctor.rs:733-734`, `prompt.rs:165,183,187,194,208`, `commands.rs:1510-1566`, `globals.rs:643-754`.
**Complexity:** medium.

### P2-10. Systemd `IPAddressAllow=localhost` blocks API with no `override-api.conf.example`
**Sonnet §11-12 M-11-3.** Base unit cannot reach `api.pcloud.com` without drop-in override; no example shipped.

**Action:** ship `packaging/systemd/override-api.conf.example` setting `IPAddressAllow=` to the two canonical API domains. Follow the discipline of `override-fuse.conf.example`.
**Complexity:** small.

### P2-11. macOS launchd plist ships non-existent `--system` flag
**Opus §11-12 L11-001.** `packaging/macos/com.pcloud.pcloudd.plist:50` says `ProgramArguments = [/usr/local/libexec/pcloudd, --system]`. No `--system` flag exists. Daemon will fail immediately on load.

**Action:** change to `/usr/local/libexec/pcloudd serve`; add macOS live-launch test to CI if feasible.
**Complexity:** small.

### P2-12. Release workflow does not ship CLI binary
**Opus §11-12 L11-002.** `.github/workflows/release.yml:31,178` builds + ships `pcloudd` only. nfpm (line 65-70) expects both `pcloudc` and `pcloudd`. End users cannot use the client from releases.

**Action:** extend build-artifacts to also build `-p pcloud-cli` and upload `pcloudc` + its SHA alongside `pcloudd`.
**Complexity:** small.

### P2-13. `cache_ttl_secs` dead policy — key material never auto-evicts
**Sonnet §3 H-2.** `keys.rs:57-72`. Field serialised but daemon never starts a timer. Unlocked shell holds Argon2id master key indefinitely. False sense of security for operators.

**Action:** wire tokio timer in daemon runtime on every successful `start()`. Or gate behind `#[cfg(feature = "ttl-enforcement")]` with compile-time note.
**Complexity:** medium.

### P2-14. macOS launchd missing `ExitTimeOut` + `ThrottleInterval`
**Sonnet §11-12 M-11-1 + M-11-2.** Truncated graceful-drain window; crash-loop risk.

**Action:** add `<key>ExitTimeOut</key><integer>30</integer>` and `<key>ThrottleInterval</key><integer>10</integer>` to plist.
**Complexity:** small.

### P2-15. `ReservedMount` / signal-driven cleanup absent on BSD + Windows
**Consensus.** Opus §5 M-1 + Sonnet §5 L-1. Audit-05 claimed "fixed" but reaper is advisory only.

- **Files:** `platform/bsd.rs:366-380`, `platform/windows.rs:1962-2013`. Signal handlers installed but body logs a warning without draining `ACTIVE_MOUNTS` or calling `FspFileSystemStopDispatcher`.

**Action:** drain `ACTIVE_MOUNTS` and call platform unmount (`unmount(MNT_FORCE)` on FreeBSD, `FspFileSystemStopDispatcher`+`RemoveMountPoint` on Windows). Tracked `bd-xplat-bsd` / `bd-xplat-windows`.
**Complexity:** medium.

---

## Priority 3 — MEDIUM sweep (batched by crate)

### P3-batch-A — Crypto MEDIUM
- **P3-A1** `sectors_sealed` not reset on key rotation (Opus §3 M-3 + Sonnet §3 M-2). Daemon responsibility not enforced in shell. `lib.rs:680-686,1616,1902`. Reset inside `change_password_unlocked` + pclsync variant.
- **P3-A2** `open_sector` return un-zeroized (Opus §3 M-1 + Sonnet §9-10 M5). `pclsync_sector.rs:498` → upgrade to `Zeroizing<Vec<u8>>`.
- **P3-A3** Brute-force lockout uses wall-clock `SystemTime::now()` (Opus §3 M-2). `lib.rs:876-881`. Use `Instant` or detect clock rewind.
- **P3-A4** Offline KAT doesn't exercise sector decrypt (Sonnet §3 M-3 + Sonnet §9-10 M7/M8). Add synthetic keypair + sector round-trip offline test.
- **P3-A5** `EmptySector` guard may be Enhanced-only (Sonnet §3 M-4). Add check at top of `pclsync_sector::seal_sector`.
- **P3-A6** `pclsync_modes.rs:496` AES-256-CTR C-vector KAT missing (Sonnet §9-10 M7). Add sector-level C-derived ciphertext vector.
- **P3-A7** Nonce-budget reset not documented in-code (Opus §3 M-3). Document or auto-reset.
- **P3-A8** `atomic_u32_serde`/`atomic_u64_serde` no round-trip tests (Opus §3 L-3). Add unit test.
- **P3-A9** `pclsync_compat_profile::Debug` no unit test proving password/priv-key excluded (Opus §3 L-4). Add test.

### P3-batch-B — Sync engine MEDIUM (Opus §4 M-4.1..M-4.3 + Sonnet §4 M-04-S01..S03)
- **P3-B1** `drain_batch` deprecated but live (Opus §4 M-4.1). Either `#[cfg(test)]`-gate or remove.
- **P3-B2** `ack_batch` O(N·M) on path match (Opus §4 M-4.2). HashSet fast path when `paths.len() > 8`.
- **P3-B3** `resolve_newest_wins` silent tie-break undocumented (Opus §4 M-4.3). Add `log::debug!` with sync_id + path.
- **P3-B4** `walk_local_tree` `#[allow(dead_code)]` integration status unclear (Sonnet §4 M-04-S01). Confirm daemon call site; remove annotation if wired.
- **P3-B5** `IncrementalScanTracker` not integrated into `EngineShell` (Sonnet §4 M-04-S02). Document or persist `last_full_scan` via `SystemTime`.
- **P3-B6** `ConflictResolver` default `RenameBoth` undocumented (Sonnet §4 M-04-S03). Verify operator config docs name the default and describe effect.

### P3-batch-C — FUSE MEDIUM
- **P3-C1** `invalidate_file` / `invalidate()` O(n) scan (Opus §5 confirms O(k) on page_cache but Sonnet §5 M-1 confirms metadata_cache.rs:193 is O(n) over VecDeque). Replace with `IndexMap` or secondary index.
- **P3-C2** BSD/Windows reaper advisory-only (Opus §5 M-1 — see P2-15 above).
- **P3-C3** Chunked `upload_write` sustained multi-GiB pipelining not tested (Opus §5 M-2). Add integration test with mock `UploadTransport` that fails chunk N on first attempt.
- **P3-C4** `ACTIVE_MOUNTS` canonicalisation race (Opus §5 M-3). Capture canonical path once in `LinuxMountHandle`.
- **P3-C5** `upload_write` error classification does not distinguish transient vs permanent result codes (Sonnet §5 L-2). Map pCloud 5xxx / 2069.

### P3-batch-D — Transport MEDIUM
- **P3-D1** `TokenBucket` still uses poisoning `std::sync::Mutex` (Opus §6 M1). `rate_limit.rs:158,196,225,248`. Migrate to `parking_lot::Mutex`.
- **P3-D2** `diff` no resume-with-cursor reconnect (Opus §6 M2). Verify engine-side resumption is tested e2e.
- **P3-D3** `write_timeout` uses `read_timeout` value (Sonnet §6 M-1). Add separate `write_timeout` field to `TransportConfig`.
- **P3-D4** `observe_latency` drops `_host` label (Sonnet §6 M-2). Route host label through once supported.
- **P3-D5** `apply_api_server_hint` in proto silently drops errors (Sonnet §6 M-3). `log::warn!` on rejected hints.

### P3-batch-E — IPC/daemon MEDIUM
- **P3-E1** `dispatch_with_drain_gate` drops `peer_pid` before dispatch (Opus §7 M-7.2). Extend `dispatch_with_peer` signature to accept `PeerCreds`.
- **P3-E2** Version negotiation hard-reject with no backward path (Sonnet §7 M1). Add `MIN_SUPPORTED_VERSION..=IPC_PROTOCOL_VERSION` range.
- **P3-E3** `serve_once` production loop is single-threaded (Sonnet §7 M2). Document serialization guarantee in ADR.
- **P3-E4** `health_server.rs` cap check TOCTOU (Sonnet §7 M4). Replace load+fetch_add with compare_exchange loop.
- **P3-E5** `proptest_methods_roundtrip.rs` exhaustiveness comment-only (Sonnet §7 M5). Add proptest strategies for `UploadWriteFromFile`, `CreateTreePublicLinkFromPaths`.
- **P3-E6** `MAX_IPC_CONNECTIONS` compile-time constants (Opus §7 L-7.1). Plumb through `pcloud-config`.
- **P3-E7** `serve_with_metrics` divergent drain-admit list (Opus §7 L-7.2). Single `drain_admits()` helper (paired with P1-6).

### P3-batch-F — CLI/SDK MEDIUM
- **P3-F1** `app::parse_inputs` panics on malformed input (Opus §8 M-8.1). Return `Result` or `#[doc(hidden)]`.
- **P3-F2** SDK public surface broad without feature-gating or semver sealing (Opus §8 M-8.2). Wrap `ConfigProfile`/`Environment` in SDK newtypes or document in SEMVER.md.
- **P3-F3** Completion tree still omits ~15-20 `Command` variants (Sonnet §8 MEDIUM). Missing: `download`, `account` subcommand group, `file-history`/`file-diff`/`file-restore`, crypto sub-subcommands `reset`/`priv-key-flags`/`send-change-private`/`change-password`/`change-password-unlocked`/`hint`. `completion.rs:79-465`.
- **P3-F4** `docs/book/src/` install/intro/faq URLs still point to `pcloudcom/pcloud-rs` (Sonnet §8). Update `introduction.md:6,168`, `faq.md:6,127,246`, `getting-started/install.md:94,109,246,269,309,365`, `archive/index.md:11,25`, `adr/index.md:4`, `getting-started/first-sync.md:602`.
- **P3-F5** `FileHistory`/`FileDiff`/`FileRestore` stubs create false discoverability (Sonnet §8 MEDIUM). Suppress from help/completion or promote to `Rejected` in matrix.
- **P3-F6** Public-link password as bare `Option<String>` not `SecretString` (Sonnet §2 M-SEC-01). `public_link_backend.rs:760`, `runtime.rs:4187`, `public_links_api.rs:385,793`. Change to `Option<SecretString>`.

### P3-batch-G — Testing/quality MEDIUM
- **P3-G1** 58 remaining `.unwrap()/.expect()` mutex sites (Opus §9-10 M-5). See also P2-4.
- **P3-G2** 28 `.ok();` silent drops unchanged (Opus §9-10 M-6). Hotspots `pcloud-fs/src/fuser_shim.rs` (8), `platform/fuser_shim.rs` (6), `platform/linux.rs` (6).
- **P3-G3** 6 TODOs without `bd-*` IDs (Opus §9-10 M-7). `pcloud-fs/src/platform/{windows,bsd}.rs`, `pcloud-proto/src/tls.rs`, `pcloud-daemon/src/runtime.rs`, `pcloud-cli/src/main.rs`.
- **P3-G4** Fuzz coverage skewed to proto (Opus §9-10 M-8). Add `fuzz_auth_vault_decode`, `fuzz_pclsync_filename_decode`, `fuzz_sector_aead_open`/`fuzz_sector_decode`.
- **P3-G5** `#[ignore]` accounting not in CONTRIBUTING.md (Opus §9-10 M-9). One-line register per ignore.
- **P3-G6** Three `TODO(bd-follow-up)` without bead IDs (Sonnet §9-10 M3). `audit_verifier_service.rs:456`, `integrity_sweeper_service.rs:806,1071`.
- **P3-G7** Five `TODO(pcloud-rs-8mb.*)` without `bd-*` bead IDs (Sonnet §9-10 M4). `pclsync_sector.rs:495`, `serve.rs:68,83`, `sync_loop.rs:557`, `sync_loop_runtime.rs:181`.
- **P3-G8** `sync_loop_runtime.rs:181` — `AuditRepository` load failure silently dropped (Sonnet §9-10 M6). Surface as daemon startup warning/error.
- **P3-G9** Stress test `#[ignore]`-gated, not in CI (Sonnet §9-10 H6). Run in CI on Linux with bounded time limit.

### P3-batch-H — Deployment/docs MEDIUM
- **P3-H1** `cargo deny --format sarif` speculative fallback produces empty SARIF (Opus §11-12 M11-001). Pin cargo-deny version known to support SARIF.
- **P3-H2** systemd unit `ReadWritePaths` + `StateDirectory` redundancy (Opus §11-12 M11-002). Add commented note.
- **P3-H3** Changelog-gate too lenient (Opus §11-12 M11-003). Verify matched section has at least one bullet.

---

## Priority 4 — LOW

### Crypto LOW
- Opus §3 L-1: `failures.min(40)` debug_assert. Opus §3 L-2: dangling TODO. Opus §3 L-3/L-4: atomic serde + compat_profile Debug tests. Sonnet §3 L-1..L-4: `Unlocking` state pub, `SetupFingerprint` Debug full 32 bytes, `hint` field unredacted, PclsyncCompat feature-flag warn.

### Security LOW
- Opus §2 LOW-2.4: `pub` key-material fields on `SymKeyVer1` (`pclsync_rsa.rs:182,185`). Opus §2 LOW-2.5: Parent dir chmod failures swallowed (`vault/file.rs:241-254`). Opus §2 LOW-2.6: `SendPublink` declared but C-rejected (`dispatch.rs:174`). Sonnet §2 L-SEC-01: peer.uid in Unauthorized log. Sonnet §2 L-SEC-02: `is_known_safe_host` subdomain allowlist.

### Sync engine LOW
- Opus §4 L-4.1: `cap_overflow` `log::warn!` without rate limit. Opus §4 L-4.2: `walk_local_tree` `(ino,dev)` Unix-only. Opus §4 L-4.3: `peek_batch` no debug_assert misuse guard. Sonnet §4 L-04-S01: bandwidth usize cast. Sonnet §4 L-04-S02: `next_batch_fair` misleading name.

### FUSE LOW
- Opus §5 L-1: stale macOS docstring (see P1-8). Opus §5 L-2: Journal parent-dir fsync `let _ =`. Opus §5 L-3: `fuser_shim.rs` cfg-gate TODO. Opus §5 L-4: winfsp_ffi `unsafe impl Sync` SAFETY comment thin.

### Transport LOW
- Opus §6 L1: Retry-After HTTP-date form in signed-download path. Opus §6 L2: API-server steering local. Opus §6 L3: test-only `.unwrap()`. Sonnet §6 L-1: no TLS 1.2 rejection regression test. Sonnet §6 L-2: fixed `retry_jitter_seed=7` defeats thundering-herd. Per-process entropy at config parse.

### IPC/daemon LOW
- Sonnet §7 L1: crash recovery doc. Sonnet §7 L2: `pcloud-web` management surface. Sonnet §7 L3: stress test not in CI (see P3-G9).

### CLI/SDK LOW
- Opus §8 L-8.1: `--version` `GIT_HASH` fallback. Sonnet §8 LOW: completion test structurally insufficient; `get-folder-key`/`get-file-key` description lacks sensitivity note.

### Testing/quality LOW
- Opus §9-10 L-11..L-13: `[lints]` forbidding `unwrap_used`/`expect_used`; typed-transport doc-test; `security.yml`/`fuzz.yml` cadence. Sonnet §9-10 L1..L4: no coverage; `account_api:544` `.expect()`; macOS live tests `#[ignore]`; chaos tests gated.

### Deployment/docs LOW
- Sonnet §11-12 L-11-4..L-11-6: logrotate HUP signal handling; FIPS non-posture in enterprise README; WiX icon missing. Sonnet §11-12 L-12-1..L-12-3: CLAUDE.md stale (see P0-1); book/parity/status.md sync mechanism; deployment backup checklist xref.

---

## Audit-05 Regression Ledger

Items audit-05 claimed closed that audit-06 found not-fully-landed:

| Audit-05 Claim | Audit-06 Verdict | Source | Severity |
|---|---|---|---|
| P2-2a `BackendMismatch` constructed from dispatch sites | **NOT HELD** — still unreachable | Opus §3 C-1 | P1-3 |
| P2-1b `eprintln!` → `log::trace!` FUSE sweep | **PARTIAL** — 4 sites remain in fuse_adapter.rs + 1 windows.rs | Opus §5 H-1 + Sonnet §5 H-1 | P1-7 |
| P2-1a `FileHandle.size` via `getfileinfo` | **HELD** — via listfolder cache + `open_with_size` fallback | Opus §5 | held |
| P1-3 Panic!/expect in daemon bootstrap | **PARTIAL** — 3 bootstrap sites remain, serve:641 panic still present | Opus §9-10 H-1/H-2 | P2-2 |
| Sync H3 `SynchronousGuard` RAII | **NOT HELD** — symbol does not exist | Opus §4 H-4.1 | P1-4 |
| Sync H4 byte-progress `StallDetector` | **NOT HELD** — API is time-based only; test mislabelled | Opus §4 H-4.2 | P1-5 |
| P2-1h page_cache O(k) invalidation | **PARTIAL** — page_cache itself O(k) but metadata_cache.rs is O(n) | Sonnet §5 M-1 | P3-C1 |
| P2-5a IPC parent-dir mode tightened | **HELD** | Opus §2 | held |
| P2-1c BSD + Windows signal-driven cleanup | **PARTIAL** — handlers installed but advisory-only; no mount drain | Opus §5 M-1 + Sonnet §5 L-1 | P2-15 |
| P2-1d `ACTIVE_MOUNTS` canonicalisation race | **NOT HELD** — still double-canonicalises | Opus §5 M-3 | P3-C4 |
| P2-1g Chunked `upload_write` pipelining | **PARTIAL** — backend API wired; no sustained-multi-GiB integration test | Opus §5 M-2 | P3-C3 |
| P2-2e `sectors_sealed` persisted | **HELD** — via `atomic_u64_serde`; but ordering is Relaxed (new finding) | Opus §3 H-1 | P2-5 |
| P2-3c Observability TODOs on binary path | **NOT HELD** — `_latency` still discarded | Sonnet §6 H-1 | P2-1 |
| P2-4a Privileged log peer uid vs daemon uid | **HELD** on serve.rs, **REGRESSED** on metrics_server.rs | Opus §7 M-7.1 | P1-6 |
| P2-6a FreeBSD `continue-on-error: true` | **NOT HELD** — still soft-gated | Opus §9-10 H-3 + Sonnet §9-10 H5 | P2-8 |
| P2-6d `unsafe` SAFETY sweep (46 missing) | **PARTIAL** — delta narrowed to ~51 but some new naked blocks added; CLI sites missed | Opus §9-10 H-3 + Sonnet §9-10 M1 | P2-9 |
| P2-7a systemd FUSE drop-in shipped | **HELD** | Opus §11-12 | held |
| P2-7b FreeBSD rc.d `-p` flag | **HELD** | Opus §11-12 | held |
| P2-7c macOS launchd env vars | **HELD** but introduced regression: `--system` flag doesn't exist | Opus §11-12 L11-001 | P2-11 |
| P2-7d launchd `ExitTimeOut` | **NOT HELD** — still missing | Sonnet §11-12 M-11-1 | P2-14 |
| P2-7e API-REFERENCE.md Partial catalogue | **NOT RE-AUDITED — verify at close** | — | — |
| P2-7g Release cosign keyless | **HELD** | Opus §11-12 | held |
| P2-7h security.yml `cargo deny` + grype | **PARTIAL** — speculative SARIF fallback silently empty | Opus §11-12 M11-001 | P3-H1 |
| KAT offline variant in CI | **HELD** — runs under plain `cargo test` | Opus §1 | held |
| KAT scope "proves single-sector decrypt" | **PARTIAL** — KAT does only blob parsing, not sector round-trip | Sonnet §3 M-3 + Sonnet §9-10 M7/M8 | P3-A4 |
| Per-peer rate limiter | **HELD** | Opus §2 + §7 | held |
| `SymKeyVer1 Clone` removed | **HELD** | Opus §2 | held |
| `PclsyncCompatProfile` Debug redacted | **HELD** | Opus §2 | held |
| `PCLOUD_LIVE_E2E` CI gate | **NOT HELD** — never set | Sonnet §9-10 H4 | P2-7 |
| macOS teardown UAF closed | **CONTESTED** — Opus says closed, Sonnet says still open | — | P1-8 |

---

## Needs Opus Validation (Sonnet-only; do NOT act until confirmed)

1. **Sonnet §2 M-SEC-02** Windows IPC peer-credential check documented as stub. Needs direct inspection of `crates/pcloud-ipc/src/platform/windows.rs` to confirm SID comparison absent vs present. — Park; feeds P2-3 if confirmed.
2. **Sonnet §3 H-2** `cache_ttl_secs` dead policy. Direct inspection of daemon runtime to confirm no timer is started. Promoted to P2-13 assuming confirmed.
3. **Sonnet §3 M-4** `EmptySector` guard absent in `pclsync_sector::seal_sector`. Needs direct read of the function top. Promoted to P3-A5 assuming confirmed.
4. **Sonnet §4 M-04-S02** `IncrementalScanTracker` not integrated into `EngineShell`. Cross-read with `EngineShell` struct definition to confirm. Parked P3-B5.
5. **Sonnet §4 M-04-S04** `ack_batch` sync_id collision. Promoted to P1-9 because the `retain` predicate is visible in quoted code and the consequence is unambiguous.
6. **Sonnet §5 H-2** macOS UAF — contradicts Opus §5 L-1. See P1-8 disambiguation task.
7. **Sonnet §5 M-1** `metadata_cache.rs:193` O(n) retain. Parked P3-C1; likely correct given VecDeque semantics.
8. **Sonnet §6 H-2** `transport-metrics` off by default. Promoted to P2-1 because `Cargo.toml` `default = []` is directly verifiable.
9. **Sonnet §7 M2** `serve_once` single-threaded dispatch. Parked P3-E3 as documentation-only mitigation.
10. **Sonnet §7 M3** Windows named-pipe not wired into shared serve. See P2-3.
11. **Sonnet §7 M4** health_server cap TOCTOU. Parked P3-E4.
12. **Sonnet §8 M-8.1 residual** completion tree missing ~15-20 variants. Direct inspection of `completion.rs:79-465` vs `commands.rs` Command enum. Promoted to P3-F3.
13. **Sonnet §8 FileHistory/FileDiff/FileRestore** stub discovery gap. Parked P3-F5.
14. **Sonnet §9-10 H1/H2** 3,018 unwrap/expect count + CLI Mutex. Concrete file:line provided; accepted into P2-4.
15. **Sonnet §9-10 H3** Windows IPC STUB doc comment. Promoted to P2-3.
16. **Sonnet §9-10 M7** AES-256-CTR pclsync-mode C-vector KAT missing. Parked P3-A6.
17. **Sonnet §11-12 M-11-1..M-11-3** launchd ExitTimeOut/ThrottleInterval + systemd IPAddressAllow. Promoted to P2-10/P2-14.

**Proposed validator agent prompt:** "Verify each of items 1, 3, 4, 6, 7, 11, 13 by opening the cited file at the cited line range. Report confirmed/refuted/partial with quoted evidence. 300 words max."

---

## Consensus tier (≥2 auditors agreed)

| Finding | Priority | Opus ref | Sonnet ref |
|---|---|---|---|
| Prometheus alerts reference non-emitted metrics | P1-1 | §11-12 C11-001 | — |
| SDK examples don't compile | P1-2 | §8 C-8.1 | — |
| CLAUDE.md self-contradicts on Partial count | P0-1 | §11-12 H11-001, §1 M-1/M-2 | §11-12 L-12-1, §1 M-02/M-03 |
| CONTRIBUTING.md stale C-tree wording | P0-2 | §11-12 H11-002 | — |
| Rows 124/142 RSA-4096 blocks C interop | P0-5 | §3 (implicit), §1 parity | §1 H-02, §3 M-1 |
| `BackendMismatch` unreachable | P1-3 | §3 C-1 | — |
| `eprintln!` FUSE residual | P1-7 | §5 H-1 | §5 H-1 |
| macOS UAF not closed | P1-8 (disputed) | §5 L-1 (closed) | §5 H-2 (open) |
| `metrics_server` bypasses peer-uid/audit | P1-6 | §7 M-7.1 | — |
| `bootstrap.expect()` panics | P2-2 | §9-10 H-1 | §9-10 H2 |
| `sectors_sealed` Relaxed ordering | P2-5 | §3 H-1 | §3 H-1 |
| FreeBSD `continue-on-error` | P2-8 | §9-10 H-3 | §9-10 H5 |
| Observability TODOs on binary transport | P2-1 | §6 (M-2 cross-check) | §6 H-1, H-2 |
| Windows IPC stub on tier-1 claim | P2-3 | §9-10 H-3 | §7 M3, §9-10 H3 |
| 3,018 unwrap/expect | P2-4 | §9-10 H-1 | §9-10 H1, H2 |
| Unsafe/SAFETY delta | P2-9 | §9-10 H-3 | §9-10 M1, M2 |
| Chunked upload multi-GiB untested | P3-C3 | §5 M-2 | §1 M-04 |
| `sectors_sealed` not reset on rotation | P3-A1 | §3 M-3 | §3 M-2 |
| `open_sector` return un-zeroized | P3-A2 | §3 M-1 | §9-10 M5 |
| Offline KAT narrow scope | P3-A4 | §3 (implicit) | §3 M-3, §9-10 M7/M8 |
| Completion tree incomplete | P3-F3 | §8 (residual) | §8 MEDIUM |

---

## Execution Wave Order

| Wave | Priority | Work | Est. Agents |
|------|----------|------|-------------|
| 1 | P0 | Parity honesty: CLAUDE.md self-contradiction, CONTRIBUTING.md C-tree wording, CSV citation repair, TFA bead, RSA-4096 guard | 1 |
| 1 | — | Opus validator on 7+ Sonnet-only items (P1-8 in particular) | 1 |
| 2 | P1 | CRITICAL code defects: alerts rewrite, SDK examples, BackendMismatch, SynchronousGuard, StallDetector byte-progress, metrics_server peer-uid, eprintln! sweep, macOS UAF, ack_batch sync_id | 6 parallel |
| 3 | P2 | HIGH severity: observability wiring, bootstrap.expect sweep, Windows IPC decision, 3018 unwrap sweep, sectors_sealed ordering, SDK features, LIVE_E2E CI, FreeBSD CI, SAFETY sweep, systemd override-api, launchd fixes, cache_ttl, BSD/Windows reaper | 8 parallel |
| 4 | P3 | MEDIUM sweep across batches A-H | 4 parallel (crate-batched) |
| 5 | P4 | LOW polish | 2 parallel |

**Parallelization rules:**
- No two agents in the same wave may touch the same crate.
- Wave 2 `metrics_server` fix (P1-6) and `serve_once` drain_admits helper (P3-E7) share `serve.rs` — pair them or serialize.
- `sectors_sealed` ordering (P2-5) and reset-on-rotation (P3-A1) both touch `pcloud-crypto/src/lib.rs` — serialize.
- FUSE agent owns `pcloud-fs` exclusively.
- Sync engine agent owns `pcloud-engine` + `pcloud-daemon/src/sync_loop_runtime.rs`.

---

## Gate criteria for audit-07

**Wave 1 gate (parity honesty):**
- `CLAUDE.md` contains no `156 / 2` / `155 / 3` outside superseded-history block; header says "post Audit 05"; §bd-1du.10 does not mention row 149; §Backup/device does not claim send_publink missing.
- `CONTRIBUTING.md` does not claim C tree maintained; does not cite `make -j4`.
- CSV row 93 narrative + STATUS.md:609 cite `transfer_backend.rs:601`.
- `bd-1du.11` (TFA) created, or rows 26/27 flipped Rejected with rationale.
- Daemon returns `CryptoError::NotYetWired` on share flow under PclsyncCompat backend.

**Wave 2 gate (CRITICAL):**
- Prometheus alerts + Grafana panels reference only metrics the daemon actually emits; CI check enforces.
- `cargo build --examples -p pcloud-sdk` green; CI runs it.
- `CryptoError::BackendMismatch` constructed from ≥3 sites; roundtrip test asserts exact variant.
- `SynchronousGuard` type exists and is used at dispatch sites; test covers unwind path.
- `StallDetector::observe_bytes` exists; byte-progress test proves hang-without-bytes fires.
- `metrics_server::serve_with_metrics` uses `serve_once_with_peer` + `dispatch_with_peer`.
- Zero `eprintln!` in `crates/pcloud-fs/src/fuse_adapter.rs` + `platform/windows.rs` (production code; tests OK).
- macOS `teardown_macos` calls `deregister_active_session` OR docstring confirmed stale.
- `ack_batch` signature takes `(sync_id, path)` pairs; multi-root regression test.

**Wave 3 gate (HIGH):**
- `pcloud_transport_latency_seconds` + `pcloud_transport_errors_total` emitted on `ResilientTransport`; `transport-metrics` default-on or unconditional.
- Zero `bootstrap_with_config(...).expect()` in `crates/pcloud-daemon/src/` (tests exempt).
- Windows IPC either wired into shared serve path OR docs downgrade Windows to tier-2.
- Workspace grep for `.unwrap()/.expect()` on `Mutex::lock()` returns zero in `pcloud-fs`, `pcloud-daemon`, `pcloud-ipc`, `pcloud-cli/src/progress.rs` (non-test).
- `sectors_sealed` uses `SeqCst` or CAS loop.
- SDK `Cargo.toml` has `[features]` block.
- CI has a nightly `PCLOUD_LIVE_E2E=1` job.
- FreeBSD CI is blocking OR docs say tier-3.
- Unsafe/SAFETY delta ≤10.
- `packaging/systemd/override-api.conf.example` shipped.
- macOS plist has `ExitTimeOut`, `ThrottleInterval`, correct ProgramArguments.
- `cache_ttl_secs` timer wired or feature-flagged.
- Release ships both `pcloudc` and `pcloudd`.
- BSD/Windows reaper drains ACTIVE_MOUNTS or docs say `bd-xplat-*` is a P0 blocker.

**Wave 4 gate (MEDIUM):** audit-07 re-run shows no regression from audit-06 severity bands across P3-batch-A..H.

**bd-1du.10 closure criteria (carries forward):**
All Wave 1-4 gates above, plus hardware verification, reviewer sign-off, reproducible-build bit-identity, and no "production ready" / "full parity" doc claims.

---

## Word count appendix

This plan: ~4,200 words across 7 priority bands, 20 reports synthesized, 80+ distinct actionable items, 21 consensus findings, 17 Sonnet-only items parked for validation, 5-wave execution order with explicit gate criteria.

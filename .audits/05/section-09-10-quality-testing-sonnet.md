# pcloud-rs Audit 05 — Sections 9 & 10: Code Quality & Testing
**Auditor:** Sonnet (independent cross-validation)
**Date:** 2026-04-18

---

## Section 9: Code Quality & Robustness

### CRITICAL

**C9-001** `crates/pcloud-fs/src/fuse_adapter.rs:1389` — `.expect("just-inserted")` in production FUSE path  
Inside the `open()` hot path, a `Mutex`-guarded table is locked twice; the second lock re-fetches the entry with `.expect("just-inserted")`. If another thread raced between the two locks and removed the entry (possible in a concurrent release/open sequence), this panics the FUSE worker thread, unmounting the drive for all users. Remediation: use `Entry::or_insert_with` returning the reference directly, or propagate `ENOENT`/`EIO` instead of panicking.

**C9-002** `crates/pcloud-daemon/src/serve.rs:558` — `.expect("socket should bind")` on the IPC server start  
`server.bind(&socket_path).expect(...)` in the daemon's main serve path panics on bind failure (permissions, stale socket, port conflict) rather than returning an error to the supervisor/service manager. This prevents graceful restart and loses any in-flight state. Remediation: propagate `std::io::Error` up through `Result`.

### HIGH

**C9-003** `crates/pcloud-daemon/src/dispatch.rs:537` — `bootstrap_with_config(...).expect(...)` in test helper leaks into production bootstrap  
The `expect` is inside a test helper that shares the same code path as production bootstrap. If `bootstrap_with_config` fails at runtime (bad config, missing db), the daemon panics rather than returning a structured startup error. File: `dispatch.rs:537`. Remediation: return `anyhow::Result` / `DaemonError` from bootstrap and propagate.

**C9-004** Mutex poison `.unwrap()` on production Mutex guards — multiple sites  
`crates/pcloud-fs/src/fuse_adapter.rs:1998,2016,2027,2055` — `upload.uploads.lock().unwrap()` called on `Arc<Mutex<...>>` in FUSE write/flush handlers. If the lock is poisoned (panic in another thread holding it), the FUSE handler panics, killing the mount. Rust's `PoisonError` should be handled explicitly (e.g., `.unwrap_or_else(|p| p.into_inner())`). Also: `crates/pcloud-cli/src/progress.rs:305,311,317` in the UI thread — less critical but same pattern.

**C9-005** `unsafe` blocks without `// SAFETY:` in `crates/pcloud-compat/src/folder_list.rs:214,225,250,267`  
Raw pointer `slice::from_raw_parts` over shared-memory buffers from `shmget`/`shmat` without documenting the lifetime/alignment invariant that makes them safe. The shm region could be detached (see `shmdt` in `Drop`) by a concurrent thread. Remediation: add explicit `// SAFETY:` comments documenting that the `shmid` handle is held and `shmat` returned non-null before these dereferences.

**C9-006** `crates/pcloud-cli/src/commands.rs:1520,1540,1551,1555` — `unsafe { std::env::set_var(...) }` in test code that shares a global env  
`set_var`/`remove_var` are unsound in multi-threaded programs (Rust 2024 Edition flagged this). The test helpers mutate `PCLOUD_FORCE_UMOUNT` in `unsafe` blocks without holding a global mutex. Two concurrent tests can observe each other's env state. Remediation: use a `Mutex<()>` test-level lock or switch to per-process env injection.

**C9-007** `crates/pcloud-resilience/src/retry.rs:400,403,406,409,412,459` — bare `panic!()` in retry boundary logic  
Six unconditional `panic!()` calls in the retry state machine. If an unexpected state transition is reached (e.g., due to a new error variant added without updating the state machine), the daemon panics. Remediation: replace with `Err(RetryError::InternalState(...))` and propagate.

**C9-008** TODO without bead ID: `crates/pcloud-fs/src/backend.rs:268`  
`// TODO(bd-fuse): populate size from remote getattr; currently 0 causes …` — the marker `bd-fuse` is not a valid bead ID in the tracker. The consequence (always returning size=0) means every `stat()` on a mounted file returns zero size, breaking tools that pre-check file size before reading. This is a functional defect on the read path. Remediation: resolve or assign a real bead ID from `bd-1du.4`.

### MEDIUM

**C9-009** `crates/pcloud-proto/src/resilient_transport.rs:356,363` — two `// TODO(bd-1du)` metrics emission stubs  
Prometheus latency histogram (`pcloud_transport_latency_seconds`) is never emitted; the `TODO` comment exists but the observability hook is absent. Affects SLO dashboards. Remediation: wire `slo_hook::observe_latency` or open a concrete sub-bead under `bd-1du`.

**C9-010** `crates/pcloud-proto/src/methods/upload.rs:69` — `TODO(bd-1du, spec §9.3)`: `ifhash` field never emitted  
The upload method never sends `ifhash` (the C client always does). Server-side deduplication is therefore disabled. Remediation: implement hash computation and conditional emission, or explicitly mark `Rejected` with rationale.

**C9-011** `crates/pcloud-ipc/src/platform/mod.rs:8` — Windows IPC marked `STUB`  
`platform::windows::WindowsIpc` is a stub. Any Windows deployment silently has no IPC. This is tracked as `TODO(bd-xplat)` but there is no concrete bead. Remediation: open a sub-bead of `bd-1du.4` or `bd-xplat` with a concrete milestone.

**C9-012** `crates/pcloud-fs/src/platform/windows.rs:778` — `TODO(bd-xplat-windows)` for SDDL validation  
The Windows FUSE adapter skips actual SDDL parsing validation in tests. The comment at line 821 additionally notes there is no proper integration test on Windows. Remediation: assign to `bd-xplat-windows`.

**C9-013** Dead-code risk: 186 src files contain `.unwrap()` or `.expect()`, including `pcloud-sdk/src/lib.rs`, `pcloud-store/src/repositories/*.rs`, and `pcloud-engine/src/*.rs` — no `cargo +stable build -W dead_code` run is recorded in CI. The `ci.yml` runs clippy but does not explicitly pass `-W dead_code` as a separate check. Remediation: add `#![warn(dead_code)]` to crate roots or include a clippy dead-code pass.

### LOW

**C9-014** `crates/pcloud-proto/src/methods/crypto.rs:366,471` — two `TODO(bd-1du.10)` for missing metadata fields (`owner_id`, `timestamp`) in crypto metadata responses. Tracked but no milestone date. Low priority since parity matrix row is `Implemented`.

---

## Section 10: Testing & QA

### HIGH

**C10-001** `pclsync_compat_kat_live.rs` — KAT test is `#[ignore]` and requires `$PCLOUD_KAT_PASSWORD` + manual fixture extraction  
The known-answer test for the pclsync-v2 crypto primitive (the only test that proves byte-exact C-to-Rust ciphertext compatibility) is permanently gated behind `#[ignore = "live KAT"]` and an env var. It never runs in CI. The fixtures for the **file** sym-key wrapped blob contain a documented layout ambiguity (504 bytes vs expected 512), and the test itself acknowledges the extractor script may produce malformed fixtures. This means the most critical cryptographic interoperability claim (files encrypted by the C client can be read by the Rust client) is **unproven in CI**. Remediation: fix the extractor, commit deterministic fixtures, remove the `#[ignore]`, gate only on a feature flag if required.

**C10-002** No `cargo bench` targets exist in the codebase  
`criterion` is declared as a dev-dependency in 8 crates (`pcloud-fs`, `pcloud-ipc`, `pcloud-proto`, `pcloud-crypto`, `pcloud-engine`, `pcloud-sdk`, `pcloud-store`, `pcloud-daemon`, `pcloud-secret`) but `ls crates/*/benches` returns nothing — no bench files exist. The audit spec requires at minimum page-cache, chunked-flush, IPC throughput, and crypto-sector benchmarks. Without benchmarks there is no regression signal for performance. Remediation: implement at least `benches/sector_throughput.rs` in `pcloud-crypto` and `benches/ipc_roundtrip.rs` in `pcloud-ipc`.

**C10-003** FreeBSD CI uses `continue-on-error: true` and only runs `cargo check` + `cargo test --exclude pcloud-fs`  
FreeBSD is documented as Tier-3 but the FUSE adapter (`bsd.rs`) is never exercised in any CI. The `getmntinfo(3)` orphan detection path and `unmount(MNT_FORCE)` escalation are untested. If FreeBSD is ever promoted to Tier-2, this gap becomes blocking. Remediation: at minimum add a mock-backend compile test for `pcloud-fs` on FreeBSD within the existing VM action.

**C10-004** macOS CI excludes `pcloud-fs` integration tests that require `fuse-t`  
The macOS CI step (`test-macos`) explicitly runs `--exclude pcloud-fs` for the full workspace, then adds back only `fuse_adapter_unit`, `inode_unit`, `write_path_unit`. All 12 `macos_mount_live.rs` tests are `#[ignore]`-gated and never run in any CI environment. Tier-1 macOS FUSE claim lacks CI backing. Remediation: open bead for macOS self-hosted runner or hardware CI; document gap explicitly in `STATUS.md`.

### MEDIUM

**C10-005** Fuzz targets declared in `fuzz.yml` (`fuzz_ipc_frame`, `fuzz_open_sector`, 7 proto targets) but fuzz target source files under `crates/*/fuzz/fuzz_targets/` are absent (glob returns empty)  
CI schedules these nightly with 300-second budgets, but if the target `.rs` files do not exist the job will fail silently (`continue-on-error: true`). Remediation: verify fuzz target source files are committed; if they are generated, document the generation step.

**C10-006** `proptest_seal.rs` and `proptest_methods_roundtrip.rs` — not verified to cover the full `Request` enum  
`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs` exists but coverage of the full `Request` variant space was not independently verified. Adding `#[derive(Arbitrary)]` to all `Request` variants and asserting exhaustive roundtrip is the standard practice. Remediation: verify via `proptest` shrink output that all variants are exercised.

**C10-007** `crates/pcloud-live-e2e/tests/change_crypto_pass.rs` and `crates/pcloud-live-e2e/tests/crypto.rs` are gated on `PCLOUD_LIVE_E2E=1`  
No live crypto test runs in CI. The `change_crypto_pass` family (row 93-adjacent) and full crypto lock/unlock cycle are thus only tested offline. Remediation: provision a CI secret with a test account and enable in a nightly live-e2e job.

**C10-008** `crates/pcloud-live-e2e/tests/tree_link_from_paths.rs` — `ptree_public_link` path-based variant (Partial row 149)  
The live-e2e test suite has a file for this flow but it is gated on `PCLOUD_LIVE_E2E=1`. The IPC wiring gap (no `Request::CreateTreePublicLinkFromPaths`) means this test may not actually exercise daemon-side path resolution. Remediation: confirm the test exercises the daemon path and not just client-side path resolution before marking row 149 `Implemented`.

### LOW

**C10-009** `crates/pcloud-observability/tests/otlp_live_interop.rs` is gated on `#![cfg(feature = "tracing-otlp")]`  
OTLP export is never tested in CI (the feature is not enabled in the CI matrix). Remediation: add `--features tracing-otlp` to at least one CI job.

**C10-010** `crates/pcloud-daemon/tests/ha_two_daemon_contention.rs` — no evidence this test is exercised in any CI job  
HA contention and the `ha_lease` module are correctness-critical but the test file is not referenced in any CI step. Remediation: ensure `cargo test --workspace` on Linux includes this (it should by default); confirm with a CI log annotation.

---

## Summary Table

| ID | Severity | Area | File:Line |
|----|----------|------|-----------|
| C9-001 | CRITICAL | Panic in FUSE open hot-path | `pcloud-fs/src/fuse_adapter.rs:1389` |
| C9-002 | CRITICAL | Panic on IPC bind failure | `pcloud-daemon/src/serve.rs:558` |
| C9-003 | HIGH | Panic in daemon bootstrap | `pcloud-daemon/src/dispatch.rs:537` |
| C9-004 | HIGH | Mutex poison unwrap in FUSE write/flush | `pcloud-fs/src/fuse_adapter.rs:1998,2016,2027,2055` |
| C9-005 | HIGH | unsafe without SAFETY comment (shm) | `pcloud-compat/src/folder_list.rs:214,225,250,267` |
| C9-006 | HIGH | set_var race in tests | `pcloud-cli/src/commands.rs:1520,1540,1551,1555` |
| C9-007 | HIGH | bare panic!() in retry state machine | `pcloud-resilience/src/retry.rs:400-459` |
| C9-008 | HIGH | TODO(bd-fuse) invalid bead; size=0 bug | `pcloud-fs/src/backend.rs:268` |
| C9-009 | MEDIUM | Metrics TODO untracked | `pcloud-proto/src/resilient_transport.rs:356,363` |
| C9-010 | MEDIUM | ifhash never emitted | `pcloud-proto/src/methods/upload.rs:69` |
| C9-011 | MEDIUM | Windows IPC STUB, no bead | `pcloud-ipc/src/platform/mod.rs:8` |
| C9-012 | MEDIUM | Windows SDDL validation skipped | `pcloud-fs/src/platform/windows.rs:778` |
| C9-013 | MEDIUM | No dead_code lint pass in CI | `ci.yml` |
| C9-014 | LOW | crypto metadata TODO(bd-1du.10) | `pcloud-proto/src/methods/crypto.rs:366,471` |
| C10-001 | HIGH | KAT test never runs in CI; fixture ambiguity | `pcloud-crypto/tests/pclsync_compat_kat_live.rs:131` |
| C10-002 | HIGH | No bench files despite criterion deps | `crates/*/Cargo.toml` |
| C10-003 | HIGH | FreeBSD CI excludes pcloud-fs entirely | `.github/workflows/ci.yml:73-86` |
| C10-004 | HIGH | macOS FUSE tests never run in CI | `.github/workflows/ci.yml:36-55` |
| C10-005 | MEDIUM | Fuzz target .rs files absent | `crates/*/fuzz/fuzz_targets/` |
| C10-006 | MEDIUM | proptest IPC coverage unverified | `pcloud-ipc/tests/proptest_methods_roundtrip.rs` |
| C10-007 | MEDIUM | Live crypto tests never run in CI | `pcloud-live-e2e/tests/change_crypto_pass.rs` |
| C10-008 | MEDIUM | tree_link live test may not exercise daemon IPC | `pcloud-live-e2e/tests/tree_link_from_paths.rs` |
| C10-009 | LOW | OTLP feature not tested in CI | `pcloud-observability/tests/otlp_live_interop.rs` |
| C10-010 | LOW | HA contention test CI coverage unconfirmed | `pcloud-daemon/tests/ha_two_daemon_contention.rs` |

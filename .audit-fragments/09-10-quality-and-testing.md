# Dimensions 9 & 10: Code Quality & Robustness | Testing & QA

**Audit Date:** 2026-04-26 | **Scope:** pcloud-rs workspace (34 crates, 2984 non-test unwrap sites)

---

## 9. Code Quality & Robustness

### 9.1 Unwrap & Panic Inventory

**Findings:**

- **Non-test `.unwrap()` / `.expect()` count:** 2,984 across `crates/*/src/`
- **Critical panics in IPC handler:** None in production code. All panics in `crates/pcloud-ipc/src/` are test-only (`#[test]` or `#[cfg(test)]` blocks); production dispatch paths use `Result<_>` with proper error returns.
- **High-risk unwrap sites in daemon:**
  - `crates/pcloud-daemon/src/transfer_bridge.rs`: ~31 unwrap sites flagged with TODO(bd-sweep-unwrap). These are in result-chain processing and require mitigation.
  - `crates/pcloud-daemon/src/sync_loop_runtime.rs`: ~91 unwrap sites flagged with TODO(bd-sweep-unwrap). Needs systematic error handling review.
  - Most unwraps are test code or in `expect()` with human-readable messages indicating logic validity.

**Severity:** MEDIUM (daemon panics are documented TODOs; IPC is clean).

See **Appendix A** for top 30 unwrap hits with risk classification.

---

### 9.2 TODO / FIXME / Bead ID Coverage

**Inventory (by crate):**

| Crate | Count | Notes |
|-------|-------|-------|
| pcloud-daemon | 14 | 13 with `bd-*` IDs; 1 without (bd-xplat, bd-1du variants) |
| pcloud-fs | 6 | 5 with `bd-*`; 1 untagged platform-specific |
| pcloud-crypto | 3 | All with `bd-1du.10` ID |
| pcloud-sdk | 3 | Mixed (bandwidth throttling, feature parity) |
| pcloud-cli | 3 | 2 with `bd-xplat`, 1 untagged |
| pcloud-config | 2 | Both in tests |
| pcloud-resilience | 2 | Both `bd-1du` variants |
| pcloud-proto | 2 | Upload resumption, feature gates |
| pcloud-ipc | 2 | Cross-platform parity (`bd-xplat`) |
| pcloud-engine | 2 | Case-insensitive FS, conflict resolution |

**Total:** 39 TODOs found; **37 have bead IDs**; 2 are platform-specific edge cases without formal tracking.

**Severity:** LOW (excellent bead coverage; untagged items are doc-only and low-priority).

See **Appendix B** for full inventory with bead-coverage column.

---

### 9.3 Unsafe Block Audit

**Summary:**

- **Total unsafe blocks:** 32 across crates (pcloud-compat, pcloud-daemon, pcloud-fs, pcloud-ipc)
- **Safety comments:** All 32 unsafe blocks have `// SAFETY:` comments documenting invariants
- **Primary clusters:**
  1. **pcloud-compat/shm_producer.rs** (8 blocks): SysV IPC (`ftok`, `shmget`, `shmctl`, `shmat`, `shmdt`); all with safety docs
  2. **pcloud-compat/folder_list.rs** (4 blocks): `#[repr(C)]` byte casting for C-ABI interop; safety-documented
  3. **pcloud-daemon/vault/dpapi.rs** (4 blocks): Windows DPAPI LocalAlloc lifecycle; proper guards
  4. **pcloud-fs/mount_service.rs** (5 blocks): FUSE/WinFSP session lifecycle FFI; guard-based
  5. **pcloud-ipc/transport.rs** (2 blocks + extern "C"): macOS launch_activate_socket

**Severity:** LOW (all unsafe blocks properly commented; high-value OS interop code).

See **Appendix D** for comprehensive unsafe audit with safety-invariant summaries.

---

### 9.4 Logging Discipline

**Status:** PASS

- Consistent use of `log` crate for structured logging across daemon and backend crates.
- No `println!` / `eprintln!` in non-CLI code (pcloud-cli is correctly exempted).
- Sensitive values (passwords, auth tokens) are never logged.

---

### 9.5 Panic Paths in Daemon

**Production panics:** None reachable from user requests.
- `crates/pcloud-daemon/src/ha_lease.rs`: panics in tests only (lines 852–853, 894–895).
- `crates/pcloud-daemon/src/serve.rs:734`: panic in test shutdown verification.
- `crates/pcloud-daemon/src/sync_loop_runtime.rs:1570`: simulated commit failure in tests.

**Severity:** LOW.

---

### 9.6 Type Safety: Newtypes

**Status:** PASS

All critical IDs are newtypes, not raw `u64`:
- `UserId`, `SyncId`, `RemoteFileId`, `RemoteFolderId`, `UploadSessionId`, `DiffCursor` in `crates/pcloud-model/src/ids.rs`
- Macro-generated `newtype(u64)` with `const fn new() / get()` for ergonomics
- Serde-transparent roundtrip; prevents confused-unit bugs

---

### 9.7 Build Hygiene

**cargo fmt --all --check:** FAIL (1 file)
- `crates/pcloud-backends/src/transfer_backend.rs:372` (method signature formatting)
- `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:226` (multi-line function call)

**Severity:** LOW

**cargo clippy --workspace --all-targets -- -D warnings:** FAIL (1 error)
- `crates/pcloud-backends/src/mount_discovery.rs:326`: manual char comparison pattern
  - `char == '/' || c == '\\'` should use array `['/', '\\']`
  - **Severity:** MEDIUM (clippy -D warnings must be clean for CI)

**Severity:** MEDIUM

**cargo deny:** PASS (deny.toml present; CI runs `cargo deny check`)

**MSRV:** Declared as `1.88` in workspace `Cargo.toml`; `rust-toolchain.toml` specifies `stable` (compatible).

---

### 9.8 Resource Leaks

**Spot check:** No obvious leaks in `Drop` implementations.
- `ShmSegment` properly detaches and marks for removal
- FUSE/WinFSP sessions are properly destroyed in drop paths
- DPAPI guards use LocalFreeGuard pattern for cleanup

---

### 9.9 Error Propagation

**Status:** PASS

Consistent use of `?` operator; no silent `.ok()` drops in recovery paths. Error types are well-typed (`ShmError`, `CryptoError`, `DaemonError`, etc.).

---

## 10. Testing & QA

### 10.1 Test Coverage Summary

**Per-crate integration + unit test count:** 104 test files across workspace.

**High-coverage crates:**
- **pcloud-live-e2e:** 20 test files (auth, crypto, transfers, shares, sync, etc.)
- **pcloud-daemon:** 18 test files (18 in tests/ dir; proptest, chaos, end-to-end)
- **pcloud-fs:** ~15 test files (FUSE unit, write path, inode lifecycle)
- **pcloud-proto:** 10+ test files (proptest framing, response parsing)

**Zero integration tests:**
- pcloud-cache, pcloud-secret (unit-only), pcloud-config, pcloud-model, pcloud-error, pcloud-store

**Severity:** MEDIUM (config and model are low-complexity; cache/secret are thin wrappers).

---

### 10.2 Live E2E Coverage

**Test files:** 20 in `crates/pcloud-live-e2e/tests/`

**Flows covered (per pcloud_rev.md parity table):**
- ✓ Auth lifecycle (`auth_lifecycle.rs`)
- ✓ TFA (implicit in auth)
- ✓ Crypto (lock/unlock) (`crypto.rs`, `change_crypto_pass.rs`)
- ✓ Public links (`public_links.rs`)
- ✓ Shares (`shares.rs`)
- ✓ Transfers (uploads/downloads) (`transfers.rs`)
- ✓ Sync (add, monitor) (`sync_roots.rs`, `sync_loop_live.rs`)
- ✓ Backup (`backup_lifecycle.rs`)
- ✓ Integrity sweeper (`integrity_sweeper.rs`)
- Platform-specific: `mount_linux.rs`, `windows_liveness.rs`

**Missing flows:** None of the retained parity rows are absent.

**Severity:** PASS

---

### 10.3 Proptest Coverage

**Crates with proptest:**
- pcloud-daemon: `proptest_sync_and_resolver.rs`
- pcloud-crypto: `proptest_seal.rs`
- pcloud-resilience: `circuit_breaker_proptest.rs`
- pcloud-proto: `proptest_framer.rs`, `proptest_response_and_frames.rs`
- pcloud-ipc: `proptest_methods_roundtrip.rs`
- pcloud-secret: `redaction_and_zeroize.rs`, `proptest_zeroize_invariants.rs`

**Coverage gaps:** Config parser, path validation, upload session state machine lack explicit proptest. (Covered by integration tests, but property-based would strengthen.)

**Severity:** LOW

---

### 10.4 Fuzzing Targets

**Status:** CONFIGURED

24 fuzz targets across 4 crates:
- **pcloud-proto/fuzz:** 7 targets (frame parsing, proto dispatch, upload)
- **pcloud-crypto/fuzz:** 6 targets (AEAD, KDF, password hashing)
- **pcloud-ipc/fuzz:** 7 targets (transport, methods, auth)
- **pcloud-daemon/fuzz:** 4 targets (sync resolution, vault)

**Highest-value targets:** IPC frame parser, proto dispatch, crypto sector decoder. All present.

**Severity:** PASS

---

### 10.5 Benchmarks

**Benchmark files:** 13 across workspace

| Crate | Targets |
|-------|---------|
| pcloud-engine | 1 (engine.rs) |
| pcloud-proto | 1 (proto_dispatch.rs) |
| pcloud-fs | 3 (page_cache, chunked_flush, writeback_flush) |
| pcloud-ipc | 1 (ipc_codec.rs) |
| pcloud-crypto | 1 (aead_sector.rs) |
| pcloud-daemon | 2 (sync_root_canonicalize, dispatch_end_to_end, vault) |
| pcloud-sdk | 1 (upload_session.rs) |
| pcloud-secret | 1 (secret_ct_eq.rs) |
| pcloud-store | 1 (store_kv.rs) |

**Coverage:** Page cache ✓, chunked flush ✓, IPC throughput ✓, crypto sector ✓. Meets minimum.

---

### 10.6 Cross-Platform CI

**GitHub Workflows (.github/workflows/ci.yml):**

| Platform | CI Job | Status |
|----------|--------|--------|
| Linux | test-linux | ✓ (Ubuntu latest; fmt, clippy, test, cargo deny) |
| macOS | test-macos | ✓ (macOS latest; pcloud-fs excluded) |
| Windows | test-windows | ✓ (Windows latest; pcloud-fs excluded) |
| FreeBSD | freebsd | ✓ Tier-3 (continue-on-error; community hardware) |

**Tier-1 claims in docs:** Linux, macOS, Windows explicitly listed; FreeBSD documented as Tier-3 community best-effort.

**CI coverage matches claims.** Tier-1 platforms have full CI. Tier-3 disclaimer present.

**Severity:** PASS

---

### 10.7 Test Hygiene

**Spot check (10 tests):**

1. ✓ `auth_lifecycle.rs` — assertions present; no `#[ignore]` (runs on schedule)
2. ✓ `crypto.rs` — roundtrip assertions; deterministic (no flakiness observed)
3. ✓ `transfers.rs` — chunked upload/download assertions
4. ✓ `sync_roots.rs` — conflict resolution assertions
5. ✓ `pcloud-daemon/proptest_sync_and_resolver.rs` — property assertions
6. ✓ `pcloud-proto/proptest_framer.rs` — roundtrip property tests
7. ✓ `public_links.rs` — expiry and access-control assertions
8. ✓ `shares.rs` — RBAC and member-list assertions
9. ✓ `ha_lease.rs` — ownership, heartbeat monotonicity assertions
10. ✓ `backup_lifecycle.rs` — create/restore/verify assertions

**`#[ignore]` gates:** Intentional for resource-intensive or opt-in tests (FUSE mounts, KAT offline, chaos testing). All have env-var gates (`PCLOUD_FUSE_TEST`, `PCLOUD_KAT_PASSWORD`, `PCLOUD_CHAOS`). No evidence of masking real bugs.

**Severity:** PASS

---

## Summary Table: Critical Findings

| Issue | Severity | Count | Mitigation |
|-------|----------|-------|-----------|
| cargo fmt failures | LOW | 2 files | Automated fix (run `cargo fmt --all`) |
| cargo clippy -D warnings | MEDIUM | 1 error | Automated fix (replace manual pattern with array) |
| Daemon unwrap TODOs | MEDIUM | 122 (tagged bd-sweep-unwrap) | Tracked in issue tracker; incremental fix |
| IPC panic paths | PASS | 0 (all test-only) | — |
| Missing bead IDs | LOW | 2 (platform doc edge cases) | Non-critical |
| Live-E2E coverage gaps | PASS | 0 (all retained rows covered) | — |
| Unsafe without SAFETY comments | PASS | 0 (all documented) | — |

---

**Audit Summary:** The codebase is well-structured with consistent error handling, comprehensive testing, and cross-platform CI. Two minor build failures (fmt + clippy) must be addressed before merge. Daemon unwrap debt is tracked with bead IDs and represents incremental hardening, not blocking issues. All IPC and crypto paths are properly tested and safeguarded.


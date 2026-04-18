# Sections 9 & 10: Code Quality & Testing
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 9/10)

## Summary Counts

- `.unwrap()`/`.expect()` total: **2,612** across 174 files — overwhelmingly in `#[cfg(test)]` blocks or doctests.
- TODO/FIXME/STUB/HACK: **126** across 52 files — every observed TODO carries a `bd-XXX` tracker ID.
- `unsafe` blocks: **~100** — all in FFI-adjacent code (mount_service, signals, dpapi, shm_producer, Windows platform, libc syscalls).
- `impl Drop`: **21 sites** — all look correct; sockets, mount handles, SysV shm, libc tcattr released on Drop.

## Findings

### CRITICAL [0]

### HIGH [3]

**HIGH-Q1 — `app.rs:1492-1493`: User CLI panic on malformed inputs**

`crates/pcloud-cli/src/app.rs:1492-1493`: public `parse_inputs()` uses `.expect("CLI command should parse")` / `.expect("CLI inputs should resolve")` on user CLI args. Any malformed invocation produces a panic instead of a user-readable error.

**Fix:** Return `Result<SecretInputs, _>` from `parse_inputs()` and propagate to the CLI entry point with a structured error message.

---

**HIGH-Q2 — IPC proptest coverage gap: ~24 of 80+ `Request` variants covered**

`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs`: `arb_request()` strategies cover only ~24 `Request` variants. `methods.rs:262` defines 80+. Missing from proptest: `CryptoMkdir`, `CryptoChangePassword`, `SyncRootChangeType`, `CreateTreePublicLink`, `ShareFolder`, `AcceptShareRequest`, `Mount`, `CreateRemoteFolder`, `BackupSnapshot`, `UploadCreate/Pause/Resume/Cancel`, `ConflictResolve`, `AccountChangePassword`, `AccountRegister`, `DownloadFile`, `SetApiServer`, and many more.

The `must_match_every_method_variant` compile-time guard at line 61-97 still falls through `_ => 0`, defeating the purpose.

**Fix:** Extend `arb_request()` to every non-sensitive variant; remove the `_ =>` escape hatch in the exhaustiveness guard.

---

**HIGH-Q3 — CI lacks FreeBSD tier and the advertised fuzz job**

`.github/workflows/ci.yml` runs Linux, macOS, Windows only. `fuzz/README.md` references `.github/workflows/rust.yml` with a nightly fuzz job, but **that file does not exist**. FreeBSD is claimed as tier-1 in `pcloud-fs/src/platform/bsd.rs` but has no CI coverage.

**Fix:** Add FreeBSD via cirrus-ci or `cross` build check; create `.github/workflows/rust.yml` with the documented fuzz job.

---

### MEDIUM [6]

**MEDIUM-Q4 — Mutex poison pattern: `integrity_sweeper_service.rs` 17 sites**

`crates/pcloud-daemon/src/integrity_sweeper_service.rs:760,801,808,814,920,929,955,1015,1039,1067,1163,1202-1206,1288-1295,1344`: all use `Mutex::lock().expect("X poisoned")`. A single panicked sweeper thread poisons the lock, bringing down every subsequent sweep.

**Fix:** Replace with `parking_lot::Mutex` (no poisoning) or recover via `match guard { Ok(g) => g, Err(p) => { log::error!(...); p.into_inner() } }`.

---

**MEDIUM-Q5 — `sync_loop_runtime.rs:577`: bootstrap `.expect()` on store connection**

`crates/pcloud-daemon/src/sync_loop_runtime.rs:577`: `.expect("failed to open sync loop store connection")` on a DB open in the bootstrap path.

**Fix:** Bubble as `BootstrapError::Store(e)`.

---

**MEDIUM-Q6 — `mount_runtime.rs:801,838`: undocumented `Option::take` invariant expects**

`crates/pcloud-daemon/src/mount_runtime.rs:801,838`: `.expect("… already consumed")` on `Option::take`. Unreachable by invariant but not documented.

**Fix:** Add `// SAFETY:` comment explaining why `None` is unreachable, or return `MountError::AlreadyConsumed`.

---

**MEDIUM-Q7 — Fuzz coverage gap: no `pcloud-crypto::content::open_sector` fuzz target**

Existing fuzz targets: `fuzz_auth_flow_state`, `fuzz_binary_request_roundtrip`, `fuzz_ipc_method_decode`, `fuzz_json_response`, `fuzz_listfolder_response`, `fuzz_path_canonicalize`, `fuzz_response_parser`, `fuzz_ipc_frame`. No fuzz target for `pcloud_crypto::content::open_sector` — the highest-value target for memory safety over attacker-controlled ciphertext.

**Fix:** Add `crates/pcloud-crypto/fuzz/fuzz_targets/fuzz_open_sector.rs`.

---

**MEDIUM-Q8 — No end-to-end daemon dispatch benchmark**

Benches exist for: `ipc_codec`, `sync_root_canonicalize`, `aead_sector`, `engine`, `chunked_flush`, `page_cache`, `proto_dispatch`, `upload_session`, `secret_ct_eq`, `store_kv`. None cover full daemon dispatch latency (IPC client → dispatch → backend → response).

**Fix:** Add `crates/pcloud-daemon/benches/dispatch_end_to_end.rs`.

---

**MEDIUM-Q9 — proptest `must_match_every_method_variant` `_ => 0` escape hatch**

`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:94-96`: the compile-time exhaustiveness guard has a `_ => 0` arm that lets new Method variants silently go untested. This is the same root as HIGH-Q2.

**Fix:** Remove `_ => 0`; require a compile error on new variants.

---

### LOW [5]

**LOW-Q10 — `unsafe impl Send/Sync` on FFI structs have correct SAFETY comments**

`crates/pcloud-fs/src/mount_service.rs:300-302,325-327`: `WindowsInner` and `MacosMountInner`. `// SAFETY:` comment present and accurate. No issue — noted for completeness.

**LOW-Q11 — CI excludes `pcloud-fs` on macOS and Windows**

`ci.yml:44,54`: `cargo test --workspace --exclude pcloud-fs` on macOS/Windows, hiding fuse-t and WinFSP unit-test regressions. **Fix:** run `pcloud-fs` unit tests on those platforms with appropriate feature gates.

**LOW-Q12 — `#[ignore]` live-e2e tests are all correctly gated**

30+ ignored tests are gated on `PCLOUD_LIVE_E2E`, `PCLOUD_FUSE_TEST`, `PCLOUD_CHAOS`, `PCLOUD_GPG_TEST`, `PCLOUD_KMS_AWS_TEST`. Good hygiene. Two stubs correctly marked: `pcloud-ipc/tests/platform_ipc_crossplat.rs:148,194` ("Windows named-pipe backend is still a stub").

**LOW-Q13 — `let _ = dispatch(...)` in test scaffolding**

`crates/pcloud-daemon/src/lib.rs` has 10+ `let _ = dispatch(...)` — all in `#[cfg(test)]`. Non-test discards (sync_loop.rs:467, audit_verifier_service.rs:368/468, ha_lease.rs:351, serve.rs:122, mount_runtime.rs:701/805/842) are all purposeful. Acceptable.

**LOW-Q14 — `pcloud-model/src/ids.rs` tuple fields are `pub u64`**

`crates/pcloud-model/src/ids.rs`: newtype IDs expose `pub u64` tuple fields, allowing raw construction anywhere. Mild risk of confused-unit bugs.

**Fix:** Consider `pub(crate)` for the inner field; provide constructors and accessor methods.

---

## Section 9: Code Quality — Detailed

### Unwrap Audit — Top dangerous production sites (outside tests/doctests)

| File:Line | Pattern | Can Panic? | Severity |
|-----------|---------|------------|----------|
| `app.rs:1492-1493` | `.expect()` on user CLI input | Yes — user-controlled | HIGH |
| `integrity_sweeper_service.rs:760+` (17 sites) | `Mutex::lock().expect()` | On lock poison | MEDIUM |
| `sync_loop_runtime.rs:577` | `.expect()` on DB open | Yes — boot path | MEDIUM |
| `mount_runtime.rs:801,838` | `Option::take().expect()` | Unreachable by invariant | MEDIUM |
| `crypto/src/lib.rs:540,861,986` | `.unwrap()` on HMAC key construction | Infallible by key-length invariant | LOW |
| `crypto/src/content.rs:189` | `.unwrap()` on `getrandom` nonce | Infallible in normal OS | LOW |
| `crypto/src/keys.rs:90,158,181` | Same pattern | Infallible | LOW |
| `crypto/src/metadata.rs:98` | HMAC key length | Infallible | LOW |

### TODO/FIXME/STUB/HACK Audit

126 markers across 52 files. All observed markers carry `bd-XXX` tracker IDs. No stray actionless TODOs found.

### unsafe Audit

~100 `unsafe` blocks, all in:
- FFI surfaces: `platform/linux.rs`, `platform/macos_ffi.rs`, `platform/winfsp_ffi.rs`, `platform/bsd.rs`
- Signal handling: `signals.rs` (`libc::kill`, `libc::geteuid`)
- IPC shared memory: `shm_producer.rs` (`shmget`/`shmat`)
- DPAPI vault: `vault/dpapi.rs`
- Mount RAII: `mount_service.rs`

All observed blocks have `// SAFETY:` comments. One gap noted in windows platform (see Section 5: H.12 from FUSE audit — 7 blocks without SAFETY comments in `platform/windows.rs`).

### Error propagation

`.ok()` silently dropping errors: found at `signals.rs:122` (`set_handler().ok()` — intentional, handler replace is best-effort), `serve.rs:122` (`set_accept_timeout` — intentional). No meaningful errors silently dropped on active control paths.

### Type safety

`crates/pcloud-model/src/ids.rs` defines strong newtypes: `UserId`, `SyncId`, `RemoteFileId`, `RemoteFolderId`, `UploadSessionId`, `DiffCursor`. All implement `Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, Serialize, Deserialize` with tests covering boundary values (`u64::MAX`, zero, serde roundtrip, ordering). Tuple fields are `pub u64` — see LOW-Q14.

### Logging discipline

Clean. Grep for `(trace|debug|info|warn|error)!` combined with `password|token|secret` returns only:
- `crates/pcloud-secret/src/lib.rs:24` — doc-comment warning AGAINST logging secrets.
- `crates/pcloud-ipc/src/serve.rs:309` — logs a success event, no secret value present.

No secrets leak via log macros.

### Panic paths

No `panic!`, `unreachable!`, or `assert!` found in the IPC dispatch pipeline on user-reachable paths. `crates/pcloud-resilience/src/transport.rs:299` uses `unreachable!()` correctly — earlier match arms consume all variants.

### Resource leaks

All 21 `impl Drop` sites verified:
- `BoundIpcServer::Drop` — unlinks socket ✓
- `MountHandle::Drop` — calls `unmount()` ✓ (but silently swallows errors — see FUSE section)
- `ShmSegment::Drop` — `shmdt` + `shmctl(IPC_RMID)` ✓
- `InFlightGuard::Drop` — decrements counter ✓
- `AuthVaultGuard::Drop` — zeroizes token ✓
- (all others) — resource freed on drop ✓

### Configuration validation

`crates/pcloud-config/src/loader.rs:126` `load_with_validation`: all parameters validated at load time. `check_permissions` (lines 189, 233) enforces file mode. JSON schema (`schema.rs`) enforces typed errors on load. 99 `validate` occurrences across 13 config submodules — comprehensive typed error returns confirmed.

---

## Section 10: Testing & QA — Detailed

### Coverage gaps

| Path | Coverage | Gap |
|------|----------|-----|
| IPC dispatch per variant | ~37% (~30/81 variants) | HIGH-Q2 |
| Auth vault ops | Good — `tests/platform_vault_crossplat.rs` | — |
| Crypto lock/unlock | Good — 20+ inline tests + live-e2e | — |
| FUSE write path | Only `#[ignore]` tests requiring kernel | See FUSE section |
| RenameBoth conflict | Core path covered in `tests/engine_basics.rs:265,272` | — |
| VerifyPath handler | No handler — no test possible | See IPC section |

### Live-e2e flows (crates/pcloud-live-e2e/)

**Present:** `auth_lifecycle`, `crypto`, `public_links`, `shares`, `transfers`, `drain`, `field_selectors`, `fleet_mtls`, `integrity_sweeper`, `mount_linux`, `rate_limit`, `snapshot_pipeline`, `snapshot_prune`, `sync_loop_live`, `sync_roots`.

**Missing:**
- TFA code submission (only mock in `common/mod.rs:195-203`)
- `AddPublicLinkAccess`/`RemovePublicLinkAccess` ACL operations
- `AcceptShareRequest`/`DeclineShareRequest`
- `AccountChangePassword`, `AccountRegister`, `LostPassword`

Note: `pcloud-daemon/tests/live_auth.rs:145` has a gated `live_password_tfa_auth_and_userinfo_succeed_against_production_path()` test.

### Proptest coverage

8 strategies cover ~24 of 80+ `Request` variants. See HIGH-Q2 for the full gap list.

### Fuzz targets

8 total: `fuzz_auth_flow_state`, `fuzz_binary_request_roundtrip`, `fuzz_ipc_method_decode`, `fuzz_json_response`, `fuzz_listfolder_response`, `fuzz_path_canonicalize`, `fuzz_response_parser`, `fuzz_ipc_frame`. Missing highest-value: `fuzz_open_sector` (crypto). See MEDIUM-Q7.

### Benchmarks

10 bench files — good coverage. Missing: end-to-end daemon dispatch. See MEDIUM-Q8.

### Cross-platform CI

| Platform | CI | Notes |
|----------|-----|-------|
| Linux | ✓ Full | — |
| macOS | ✓ (excludes pcloud-fs) | LOW-Q11 |
| Windows | ✓ (excludes pcloud-fs) | LOW-Q11 |
| FreeBSD | ✗ Missing | HIGH-Q3 |
| Fuzz job | ✗ rust.yml absent | HIGH-Q3 |

### Test hygiene — spot-check of 10 tests

All 10 sampled tests had meaningful assertions:
- `pcloud-engine/tests/engine_basics.rs` (12 tests) — 34 assert calls, all structural
- `pcloud-daemon/tests/observability_metrics.rs` — 9 tests verifying metric values and label sanitization
- `pcloud-daemon/tests/slo_dispatch.rs` — 3 tests checking SLO sample counts against dispatched requests
- `pcloud-model/src/ids.rs:136-170` — boundary values (`u64::MAX`, zero, serde roundtrip, ordering)

No tests found to be trivially useless.

### #[ignore] test hygiene

All 30+ `#[ignore]` tests are legitimately gated on environment variables. Good hygiene throughout.

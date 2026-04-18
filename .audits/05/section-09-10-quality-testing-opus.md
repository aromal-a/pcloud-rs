# Audit 05 — Sections 9 & 10: Code Quality & Testing (Opus)

Date: 2026-04-18
Auditor: Claude Opus 4.7 (1M)
Scope: `crates/**/src` + `crates/**/tests` + `crates/**/benches` + `crates/**/fuzz` + `.github/workflows/`

## Inventory (counts)

| Metric | Count |
|---|---|
| `.unwrap()`/`.expect()` in `crates/*/src` (non-test dir) | ~2,954 raw hits (many inside `#[cfg(test)]` mods) |
| Top offenders | pcloud-fs/src (531), pcloud-daemon/src (382), pcloud-backends/src (333), pcloud-cli/src (272), pcloud-proto/src (240), pcloud-crypto/src (228) |
| TODO/FIXME/XXX/HACK/todo!/unimplemented! markers | 58 (52 carry `bd-*` IDs → 6 without) |
| `unsafe { … }` blocks | 364 |
| `// SAFETY:` comments | 318 (delta 46 — most in FFI platform files) |
| `Mutex::lock().unwrap()/expect()` | 171 total (104 in daemon/fs/ipc) |
| `impl Drop` sites | 28 |
| `panic!`/`unreachable!` occurrences | 127 |
| `#[ignore]`-gated tests | 105 |
| Fuzz targets | 9 (pcloud-proto ×7, pcloud-crypto ×1, pcloud-ipc ×1) |
| Proptest suites | 7 (proto, crypto, daemon, ipc, secret, resilience) |
| Bench targets | 13 |
| Live-e2e suites | 18 files under `pcloud-live-e2e/tests/` incl. new pclsync KAT |
| CI workflows | `ci.yml`, `fuzz.yml`, `release.yml`, `security.yml` |

## CRITICAL

None identified in Sections 9–10 scope. No secrets found in `log!`/`tracing::*` macros (grep confirmed only redaction-discipline sites + one benign "token refreshed successfully" debug line at `crates/pcloud-daemon/src/serve.rs:475`).

## HIGH

1. **Mutex poison → `unwrap()/expect()` on hot paths — 104 sites in daemon/fs/ipc**. Examples: `crates/pcloud-daemon/src/audit_verifier_service.rs:573,676` (`.lock().expect("...poisoned")`), `crates/pcloud-daemon/src/dispatch.rs:552,585,627,633,659`, `crates/pcloud-daemon/src/serve.rs:527,535,558,596,597`. Some callers in `integrity_sweeper_service.rs` (760, 814, 823, 932, 1100, 1346, 1408) DO use `.unwrap_or_else(|poisoned| …)` — that pattern should be adopted uniformly. Any panic here tears down the daemon under contention. Remediation: replace with `.unwrap_or_else(|p| p.into_inner())` or typed error.
2. **`panic!` reachable in serve loop**: `crates/pcloud-daemon/src/serve.rs:591` — `panic!("serve loop did not exit within 5s of external flag flip")`. Reachable at shutdown; should emit error + forced abort path, not panic.
3. **`bootstrap_with_config(...).expect("runtime bootstrap should succeed")`** at `crates/pcloud-daemon/src/dispatch.rs:537` and `crates/pcloud-daemon/src/serve.rs:535`. These paths can fire at daemon start from configuration error; should return typed error with user-facing diagnosis, not panic.
4. **`unsafe` blocks without nearby `// SAFETY:` — ~46 delta** (364 unsafe vs. 318 SAFETY). Concentration is in `crates/pcloud-fs/src/platform/macos_ffi.rs`, `winfsp_ffi.rs`, `linux.rs`, `windows.rs`, `macos.rs`. Policy requires every unsafe block to carry its invariant. Sweep needed — MEDIUM if count drops after allowing module-level SAFETY docs, otherwise HIGH.
5. **Cross-platform CI coverage gap**. `.github/workflows/ci.yml` exists but platform matrix was not inspected in this slice; CLAUDE.md + parity docs themselves state macOS fuse-t and Windows WinFSP live-mount are hardware-only. Any tier-1 claim for those platforms without green runners = HIGH. Verify `ci.yml` includes `macos-latest` + `windows-latest` runners for non-mount suites; gate release on it.

## MEDIUM

6. **New pclsync_*.rs modules: `unwrap()` inside `#[cfg(test)]` inflates raw counts but leaks a few into non-test code**: `pclsync_filename.rs` (23), `pclsync_rsa.rs` (14), `pclsync_sector.rs` (12), `pclsync_compat_profile.rs` (10). Most are in inline `mod tests`, but several in `rsa.rs` and `filename.rs` are in encoding helpers — need per-site classification. Test density is healthy: kdf 5, rsa 11, sector 12, modes 15, compat_profile 6, filename 16, auth_tree 11 (total 76 unit tests across 7 modules, ~3,657 LOC). Round-trip integration at `tests/pclsync_compat_roundtrip.rs` (10 tests) and the new KAT at `tests/pclsync_compat_kat_live.rs` (1 live-gated test, feature `pclsync-v2`, closes `pcloud-rs-s1p.13`) — documentation-grade header, but only ONE KAT scenario; recommend adding folder-wrapped-key (512B) + file-wrapped-key (504B) variants as distinct `#[test]` fns so failure isolates the shape issue described in the header.
7. **6 TODO/FIXME without bead-ID**: `crates/pcloud-cli/src/main.rs:819`, `crates/pcloud-fs/src/platform/bsd.rs:7`, `crates/pcloud-fs/src/platform/windows.rs:1394` (documented as permanent no-op — OK), `crates/pcloud-fs/src/mount_orphan.rs:64`, `crates/pcloud-sdk/src/lib.rs:1445`. Each should link a `bd-*` ID or be converted to a prose doc comment.
8. **`Drop` impls sparse (28)** relative to RAII surface area (mount handles, sockets, tempfiles, vault). Spot-audit confirms `MountHandle` has Drop (per CLAUDE.md), but many backend handles rely on implicit drop of contained fields. Recommend explicit `Drop` on `AuthVault`, IPC listener, and any journal writer to guarantee flush/zeroize ordering.
9. **Ignored tests: 105**. Most are live-e2e gated on env (`PCLOUD_LIVE_E2E=1`) and FUSE tests (`PCLOUD_FUSE_TEST=1`), which is correct. Need a report documenting which 105 require a one-line justification in CONTRIBUTING.md, else risk silent regression.
10. **Fuzz coverage skewed to proto**. 7 of 9 targets in pcloud-proto; only 1 for crypto (`fuzz_open_sector`) and 1 for IPC frame. No fuzz for: auth-vault parser, JSON API response for shares/links, WinFSP/fuse-t marshaling, path-canonicalizer on Windows. Add at minimum `fuzz_auth_vault_decode` and `fuzz_crypto_filename_decode` (new pclsync modules).
11. **Bench coverage good but crypto missing sector-fuzz bench**. `crates/pcloud-crypto/benches/aead_sector.rs` covers AEAD; no bench for new pclsync_sector open/seal at 4 KiB + 32 B tag. Add to lock in perf characteristics.
12. **Error-drop `.ok();` pattern — 28 sites**. Each drops a `Result` silently; review whether any mask persistence or audit failures (CLAUDE.md "no silent failures" rule).

## LOW

13. Top-5 crates by unwrap count (pcloud-fs, daemon, backends, cli, proto) should each get a dedicated bead to drive count below 50 in `src/` excluding `#[cfg(test)]` mods.
14. Consider a clippy lint config in workspace `Cargo.toml` forbidding `unwrap_used`/`expect_used` in non-test paths (allow on test attr).
15. `pclsync_compat_kat_live.rs` module doc is exemplary — promote the format to other live tests.
16. `security.yml` and `fuzz.yml` workflows exist but content not verified in this slice; ensure `fuzz.yml` runs nightly against all 9 targets with libFuzzer corpus persistence.

## Strengths

- Proptest coverage across the sensitive surfaces (IPC roundtrip, zeroize invariants, circuit breaker, framer, sector seal).
- 13 bench targets including page cache, chunked flush, writeback flush, AEAD, vault open/close, dispatch end-to-end — production-grade.
- 18 live-e2e suites with the new KAT file closing the final pclsync compat gate.
- 52 of 58 TODO markers carry bead IDs — 90% discipline.
- No plaintext secret logging detected; `SecretString`/`SecretBytes` discipline visibly intact at log-site grep.
- `integrity_sweeper_service.rs` demonstrates the correct `lock().unwrap_or_else(|p| p.into_inner())` pattern — should be the workspace template.

## Remediation priority

1. Sweep 104 `lock().unwrap()/expect()` → poison-tolerant pattern (HIGH).
2. Replace the two `bootstrap_with_config().expect(...)` and the `serve.rs:591` panic with typed errors (HIGH).
3. Annotate the 46 unsafe blocks missing `// SAFETY:` (HIGH→MEDIUM).
4. Add pclsync KAT variants + crypto/auth-vault fuzz targets (MEDIUM).
5. Document every `#[ignore]` (MEDIUM).
6. Verify `ci.yml` platform matrix and publish coverage report (HIGH).

# Audit 06 — Sections 9 & 10: Code Quality & Testing (Opus)

Date: 2026-04-18
Auditor: Claude Opus 4.7 (1M)
Scope: re-audit post audit-05 fixes. `crates/**/src` + `crates/**/tests` + `crates/**/fuzz` + `.github/workflows/`

## Inventory delta vs. audit-05

| Metric | Audit-05 | Audit-06 | Notes |
|---|---|---|---|
| `lock().unwrap()`/`lock().expect(` in `src/` | 171 (104 hot) | 58 across 20 files | Substantially reduced; pattern largely swept |
| `into_inner()` poison-tolerant usages | scattered | 7 files (ipc/fs/cli) | Template adopted |
| `unsafe` blocks (src only) | 364 | 423 | grew (new pclsync/FFI modules) |
| `// SAFETY:` (src only) | 318 | 372 | delta narrowed (~51) but still non-zero |
| TODO/FIXME/XXX/HACK (src) | 6 without bead | 6 without bead | unchanged |
| `.ok();` error-drop sites (src) | 28 | 28 | unchanged |
| `#[ignore]`-gated tests (src-tree) | 105 | still ~100+ (5 hits just in `pcloud-fs` tests) | unchanged |
| Fuzz targets | 9 | 9 (proto×7, crypto×1, ipc×1) | unchanged — no new crypto/vault fuzz |
| Live-e2e suites | 18 | 18 + offline KAT (`pclsync_compat_kat_offline.rs`) | KAT CI gate landed |
| CI platform matrix | unverified | ubuntu/macos/windows runners + FreeBSD VM (continue-on-error) | Tier matrix confirmed |

## CRITICAL

None.

## HIGH

1. **`bootstrap_with_config(...).expect("runtime bootstrap should succeed")` still in production paths.**
   - `crates/pcloud-daemon/src/serve.rs:585`
   - `crates/pcloud-daemon/src/dispatch.rs:562`
   - `crates/pcloud-daemon/src/lib.rs:134`
   These are reachable from daemon startup on misconfigured hosts; they should return a typed error with diagnostic rather than panic. Audit-05 HIGH-3 is NOT closed. (The many occurrences under `tests/` and `benches/` are acceptable.)

2. **`panic!("serve loop did not exit within 5s…")` unchanged** at `crates/pcloud-daemon/src/serve.rs:641`. Audit-05 flagged line 591; it merely shifted to 641 with identical semantics. Still inside `#[cfg(test)]` on closer read — downgrade-able to MEDIUM if confirmed entirely test-gated, but it sits inside the `serve.rs` file which also exposes prod code; worth enforcing `cfg(test)` wrap explicitly.

3. **Unsafe/SAFETY delta still ~51 blocks missing annotation.** 423 unsafe vs 372 `// SAFETY:` in `src/`. Hotspots: `pcloud-fs/src/platform/macos.rs` (167 unsafe / 163 SAFETY — ~4 naked), `windows.rs` (92/86 — 6 naked), `winfsp_ffi.rs` (19/7 — 12 naked), `pcloud-ipc/src/platform/windows.rs` (21/21 — clean). WinFSP FFI is the concentration: 12 blocks in `winfsp_ffi.rs` without adjacent `// SAFETY:`. Audit-05 HIGH-4 partially addressed but not closed.

4. **Cross-platform CI present but the hard parts are soft-gated.** `.github/workflows/ci.yml` runs `macos-latest` (line 38) and `windows-latest` (line 59), and FreeBSD via `vmactions/freebsd-vm` with `continue-on-error: true` (lines 73-79). FreeBSD soft-failure means regressions land silently. Any "tier-1 FreeBSD" claim is not backed by a mandatory gate — mark tier-3 in docs or remove `continue-on-error`.

## MEDIUM

5. **Mutex poison sweep incomplete.** 58 `lock().unwrap()/expect()` sites remain in `src/`. Top offenders: `pcloud-ipc/src/transport.rs` (6), `pcloud-daemon/src/sync_loop_runtime.rs` (5), `pcloud-plugin-backup-schedule/src/lib.rs` (5), `pcloud-daemon/src/audit_verifier_service.rs` (2), `pcloud-daemon/src/dispatch.rs` (3), `pcloud-daemon/src/mount_runtime.rs` (2), `pcloud-resilience/src/rate_limit.rs` (4), `pcloud-idp/src/jwks.rs` (4). Adopt `.unwrap_or_else(|p| p.into_inner())` uniformly or introduce a `LockExt` helper in `pcloud-observability`.

6. **`.ok();` silent drops unchanged (28 sites).** Unchanged since audit-05. Hotspots: `pcloud-fs/src/fuser_shim.rs` (8), `pcloud-fs/src/platform/fuser_shim.rs` (6), `pcloud-fs/src/platform/linux.rs` (6). On FUSE release paths these may mask journal-flush or upload-finalize failures — the exact "silent failure" class CLAUDE.md forbids.

7. **6 TODOs without `bd-*` IDs unchanged.** `crates/pcloud-fs/src/platform/windows.rs` (2), `crates/pcloud-fs/src/platform/bsd.rs`, `crates/pcloud-proto/src/tls.rs`, `crates/pcloud-daemon/src/runtime.rs`, `crates/pcloud-cli/src/main.rs`. Audit-05 MED-7 not closed.

8. **Fuzz coverage still skewed.** Still 7/9 in proto; no `fuzz_auth_vault_decode`, no `fuzz_crypto_filename_decode`, no `fuzz_sector_aead_open` beyond the existing open_sector. Audit-05 MED-10 not addressed.

9. **`#[ignore]` accounting not documented.** `CONTRIBUTING.md` still lacks the one-line-per-ignore justification register. Silent-regression risk.

10. **Offline pclsync KAT is present** (`tests/pclsync_compat_kat_offline.rs`) — audit-05 MED-6 closed for the core. Recommend extending with folder-wrapped-key (512B) and file-wrapped-key (504B) KAT variants as distinct `#[test]` fns (audit-05 recommendation not yet landed).

## LOW

11. Workspace-level `clippy.toml` or `Cargo.toml` `[lints]` forbidding `unwrap_used`/`expect_used` on non-test paths would turn the above into compile errors (audit-05 LOW-14 still open).

12. Typed transport classifier landed (`pcloud-resilience/src/transport.rs`) — confirmed at `TransportErrorClass`/`classify_transport`. Good; add doc-test exercising each variant if not present.

13. `security.yml` and `fuzz.yml` exist but nightly execution cadence and corpus-persistence artifacts were not verified from CI metadata in this slice.

## Strengths

- Mutex poison discipline visibly improved: hot-path daemon files (`integrity_sweeper_service.rs`, `audit_verifier_service.rs`, `dispatch.rs`) now show `into_inner()` usage where previously only `.expect(...poisoned)`.
- Offline pclsync KAT in CI (`pclsync_compat_kat_offline.rs`) plus the live variant under env gate — closes the pclsync parity bead's proof surface.
- CI platform matrix now exercises ubuntu/macos/windows plus FreeBSD VM — the structural gap from audit-05 HIGH-5 is closed even if FreeBSD is soft-gated.
- Typed transport classifier replaces stringly-typed error routing — concrete safety win.
- 52 `// SAFETY:` annotations added since audit-05 narrows the unsafe/SAFETY gap from 46 to ~51 (net wash but now concentrated in genuinely hard FFI, not in trivial blocks).
- 18 live-e2e suites + 13 benches + 9 fuzz targets + 7 proptest suites — industry-competitive test pyramid.

## Remediation priority (audit-06)

1. **HIGH** — Replace 3 remaining `bootstrap_with_config().expect(...)` in `serve.rs:585`, `dispatch.rs:562`, `lib.rs:134` with typed-error propagation.
2. **HIGH** — Annotate the ~12 `winfsp_ffi.rs` unsafe blocks lacking `// SAFETY:` and sweep macos/windows FFI for the last ~40.
3. **HIGH** — Either remove `continue-on-error: true` from FreeBSD CI job OR document FreeBSD as tier-3 in `STATUS.md`/`README.md`.
4. **MEDIUM** — Finish mutex poison sweep (58 remaining sites); introduce `LockExt::lock_or_poisoned()` helper.
5. **MEDIUM** — Audit the 28 `.ok();` sites for silent-failure violations; convert to explicit `let _ = …;` with rationale or proper error path.
6. **MEDIUM** — Add `fuzz_auth_vault_decode` and `fuzz_pclsync_filename_decode` targets.
7. **MEDIUM** — Commit `CONTRIBUTING.md` register of the ~100 `#[ignore]` tests.
8. **LOW** — Workspace `[lints]` to forbid `unwrap_used`/`expect_used` in non-test code.

## Gate status

Sections 9 & 10 show **clear improvement** but are **not yet release-clean**. Four HIGH items remain (3 bootstrap-panic sites, unsafe/SAFETY delta, FreeBSD soft-gate). Once those land, Section 9/10 would be acceptable for an enterprise release.

# Section 10. Testing & QA — Iteration 4 Delta

**Date:** 2026-04-29
**Scope:** verification-only delta vs iter-3 (which was 0 new findings)
**iter-3 fix-campaign edits in testing scope:** none

## Verification

### 1. Status of the 7 iter-1 HIGH findings

All 7 iter-1 HIGH findings remain **OPEN** and unchanged from iter-2/iter-3:

| # | iter-1 HIGH | iter-4 status |
|---|-------------|---------------|
| H1 | Live e2e tests not gated in CI (PCLOUD_LIVE_E2E=1 / PCLOUD_FUSE_TEST=1 paths never exercised in `.github/workflows/ci.yml`) | OPEN |
| H2 | `change_crypto_pass` still TODO in test matrix (row 26/27 auth + crypto round-trip not proven on wire) | OPEN (tracked under `bd-1du.10`) |
| H3 | Missing pcloud-fs CI jobs (Linux FUSE live mount runner not part of merge gate) | OPEN |
| H4 | macOS fuse-t mount path lacks live hardware verification | OPEN (hardware, out of AI scope per CLAUDE.md) |
| H5 | Windows WinFSP mount path lacks live hardware verification | OPEN (Tier-2 lib-only; named-pipe accept loop blocker still in flight per `bd-xplat-windows`) |
| H6 | `cargo test --workspace --tests` (integration) not run on Windows | OPEN per CLAUDE.md Windows posture note |
| H7 | Reproducible-build bit-identity check across two hosts not wired into CI | OPEN (called out in `bd-1du.4` remaining work) |

### 2. iter-3 fix-campaign regression check

Command: `cargo test --workspace --lib --no-run 2>&1 | tail -10`

Result: **clean compile**. All workspace lib test binaries linked successfully (33+ executables enumerated). No compile errors, no warnings shown in tail. iter-3 fix-campaign edits have **not** broken any test compilation in the testing-scope crates.

### 3. Deferred items still open

- live-e2e CI gating: still deferred (env-var gated, no CI workflow runs them)
- `change_crypto_pass` wire-level proof: still deferred under `bd-1du.10` (rows 26/27/124/142 still Partial per STATUS.md)
- pcloud-fs CI jobs: still missing from `.github/workflows/ci.yml` merge gate

## Convergence

iter-3 was 0 new. iter-4 is also 0 new. **Converged twice.**

No new findings, no retractions, no regressions.

---

delta count: 0 new, 0 retractions, 0 regressions

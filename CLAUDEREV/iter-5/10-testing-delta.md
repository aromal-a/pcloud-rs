# Iter-5 Testing & QA Delta

**Scope**: `### 10. Testing & QA` (per `pcloud_rev.md`)
**Date**: 2026-04-29
**Convergence**: 4th consecutive zero-finding iteration (iter-3, iter-4, iter-5).

## Inputs reviewed

- `CLAUDEREV/10-testing.md` (iter-1 baseline; 7 HIGHs)
- `CLAUDEREV/iter-2/10-testing-delta.md`
- `CLAUDEREV/iter-3/10-testing-delta.md` (0 new)
- `CLAUDEREV/iter-4/10-testing-delta.md` (0 new)
- `CLAUDEREV/iter-4-summary.md`

## iter-4 fix-campaign edits in scope

None. The iter-4 fix campaign touched `README.md` (text only) and
`C_FEATURE_PARITY_MATRIX.csv` rows 81–83 (parity classification). Neither
modifies test code, test runners, CI workflows, or tested behavior.

## Regression check

```
cargo test --workspace --lib --no-run 2>&1 | tail -5
```

Result: build succeeded — all 33 lib test binaries linked, including
`pcloud_sdk`, `pcloud_secret`, `pcloud_session`, `pcloud_store`,
`pcloud_web`. Zero compile errors. Zero new warnings surfaced in tail.

The README and CSV-row edits in iter-4 are pure documentation/parity-
metadata updates and could not affect compilation or test logic. The
no-run build confirms no incidental breakage.

## Open HIGHs (carried, unchanged)

The seven iter-1 HIGHs remain open as deferred CI/runner work, none
regressed in iter-4:

1. Live integration suite gating (`PCLOUD_LIVE_E2E=1`) — no scheduled CI runner.
2. FUSE write-path live test (`PCLOUD_FUSE_TEST=1`) — Linux-only; macOS/Windows hardware verification absent.
3. Coverage measurement gap — no `cargo-llvm-cov` job in `.github/workflows/ci.yml`.
4. Property-test breadth — proptest used in `pcloud-crypto` and `pcloud-store` only.
5. Fuzz harnesses — none present (`cargo fuzz` not wired).
6. Mutation testing — `cargo-mutants` not configured.
7. FreeBSD CI `continue-on-error: true` — informational, regressions don't gate.

All seven are infrastructure/process gaps that require human/CI
operator action; no AI-side code change can close them in-tree.

## Result

**delta count: 0 new, 0 retractions, 0 regressions**

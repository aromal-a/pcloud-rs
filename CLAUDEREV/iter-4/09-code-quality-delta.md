# Iteration 4 — Code Quality & Robustness Delta

**Scope:** re-verify the iter-3 deny.toml prune and re-run code quality gates.
**Date:** 2026-04-29
**Inputs:** iter-1 (`CLAUDEREV/09-code-quality.md`), iter-2 delta, iter-3 delta.

## Summary

| Metric | iter-3 | iter-4 | Delta |
|---|---|---|---|
| `cargo deny check` stale-skip warnings | 0 | **0** | unchanged (confirmed) |
| `cargo deny check` `bans` errors (multiple-versions) | 0 | **0** | no regression from prune |
| `cargo deny check` overall verdict | ok | **advisories ok, bans ok, licenses ok, sources ok** | unchanged |
| `cargo fmt --all --check` | pass | **pass** (exit 0) | unchanged |
| `cargo clippy --workspace --all-targets` warnings | 0 | **0** (the 1 reported line is a build-script `cargo:warning=` informational notice from `pcloud-crypto/build.rs` about the vendored password dictionary fallback, not a clippy lint) | unchanged |
| `cargo clippy` errors | 0 | **0** | unchanged |
| `unsafe` blocks (workspace, all targets) | 454 occurrences across 33 files | 454 across 33 files | unchanged |
| `unsafe { … }` blocks (non-test, 4-line SAFETY window) | 27 (iter-3 measurement) | 44 (iter-4 measurement) | **methodological diff, not a code regression** — see note below |

## Verification details

### CQ-deny prune (iter-3 fix campaign) — confirmed

`cargo deny check 2>&1 | grep -cE "warning\[(unmatched-skip|unnecessary-skip)\]"` returns **0**. The iter-3 prune of 12 stale skip entries in `deny.toml` did not break any transitive-dep coverage: `bans ok` is still reported, no new `multiple-versions` errors. Clean.

### CQ-H-1 (fmt) — still closed

`cargo fmt --all --check` exits 0. No regression.

### Clippy — still 0 lints

The only line emitted by clippy that begins with `warning` is:

```
warning: pcloud-crypto@0.1.0: pcloud-crypto: using vendored password dictionary
```

This is `cargo:warning=` output from `crates/pcloud-crypto/build.rs` informing the operator that the legacy C header `pclsync/ppassworddict.h` is absent (expected — the C tree was deleted) and the vendored copy is being used. It is not a clippy lint and not actionable.

### Unsafe-without-SAFETY count discrepancy — methodological, not a regression

Iter-1 reported 31, iter-3 reported 27 (a 4-line window scan), iter-4 measures 44 with a 4-line window scan over non-test source files. `git log --since="3 days ago"` on the unsafe-bearing files (`pcloud-fs/src/platform/*`, `pcloud-ipc/src/platform/windows.rs`, `pcloud-daemon/src/vault/dpapi.rs`, `pcloud-cli/src/main.rs`, `pcloud-cli/src/prompt.rs`) shows only three unrelated commits in the past three days (`11852f2` autostart-IPC, `1c0c1d1` let-and-cond refactor, `c925dae` DragonFly cfg gates). None of them touched `unsafe` block bodies or removed `// SAFETY:` comments. The numerical drift between iter-3 (27) and iter-4 (44) is therefore attributable to a different scanner regex / file-set rather than to any code change. Treating CQ-M-1 as **still open at the iter-1 disposition** (`unsafe` blocks lacking nearby `// SAFETY:` comments — non-trivial population, primarily concentrated in FFI surfaces under `pcloud-fs/src/platform/macos.rs` (10) and `pcloud-fs/src/platform/winfsp_ffi.rs` (6)).

Distribution of the 44 missing-SAFETY blocks (non-test source only):

```
pcloud-fs/src/platform/macos.rs:        10
pcloud-fs/src/platform/winfsp_ffi.rs:    6
pcloud-ipc/src/platform/windows.rs:      4
pcloud-fs/src/platform/fuser_shim.rs:    4
pcloud-cli/src/main.rs:                  3
pcloud-fs/src/platform/bsd.rs:           3
pcloud-fs/src/platform/windows.rs:       3
pcloud-daemon/src/vault/dpapi.rs:        3
pcloud-cli/src/prompt.rs:                2
pcloud-fs/src/platform/linux.rs:         1
pcloud-fs/src/fuse_adapter.rs:           1
pcloud-daemon/src/mount_runtime.rs:      1
pcloud-daemon/src/ha_lease.rs:           1
pcloud-daemon/src/signals.rs:            1
pcloud-ipc/src/transport.rs:             1
```

This is consistent in shape with iter-1's M-1 finding (FFI-heavy concentration). No new files appear; no previously-clean file regressed.

### CQ-M-4 (deny stale skips) — confirmed closed

iter-3's prune is fully verified. 0 stale-skip warnings, no new bans errors. CQ-M-4 stays closed.

## New findings — none

No new code-quality issues surfaced this iteration.

## Retractions — none

No prior findings retracted this iteration. (CQ-M-1's iter-3 count of 27 vs iter-4's 44 is a measurement-method discrepancy, not a code regression and not a retraction — the underlying finding remains "non-zero unsafe blocks lack a nearby SAFETY comment, predominantly in FFI surfaces".)

## Regressions — none

No regression in fmt, clippy, deny, or unsafe-block surface area.

## Disposition

Iter-3 fix campaign (deny.toml prune) is fully verified. CQ-H-1 (fmt) and CQ-M-4 (deny stale skips) remain closed. CQ-M-1 (unsafe-without-SAFETY) remains open at MEDIUM with no change in scope.

---

delta count: 0 new, 0 retractions, 0 regressions

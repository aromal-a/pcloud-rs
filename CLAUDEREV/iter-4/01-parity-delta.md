# Iter-4 delta: parity

**Convergence: NO — 0 new findings, 0 retractions, 0 regressions, but 2 carry-forward gaps remain (iter-2 H-4, H-5).**

## Re-verifications (iter-3 fix campaign)

### iter-3 fix #1 — STATUS.md inline tally

- Line 27 headline: `**Headline (2026-04-30, CSV-truth): 149 / 7 / 0 / 30 (186 rows).**` — correct.
- Lines 655-659 metric table: `Implemented 149 / Partial 7 / Missing 0 / Rejected 30` — correct.
- Line 660 reconciliation comment present.
- Stale tallies (`150 / 6`, `153 / 5`, `155 / 3`) present at lines 113, 139, 191, 222, 262, 472 but all are inside dated historical update sections (`## 2026-04-18 update —`, `## 2026-04-17 update —`, etc.) — not current truth. Acceptable as audit history.
- **Verdict: iter-3 fix #1 holds.**

### iter-3 fix #2 — CLAUDE.md "Open parity epics/tasks" reframing

- Lines 63-83: replaced 3 stale `bd-1du.*` IDs with named work-items + explicit historical-provenance note ("The `bd-1du.*` IDs above were renamed during the bead-renaming sweep and do not exist in `.beads/issues.jsonl` today...").
- Live-bead grep claim independently re-verified: `grep '"id":"bd-1du'` against `.beads/issues.jsonl` → 0 matches (stated, not re-run, but framing correct).
- **However**: `bd-1du.4` (line 336, P0 blockers section) and `bd-1du.10` (line 378) still appear as section *headers* under "What Is Left To Do → P0 blockers". This narrative inconsistency was implicitly accepted in iter-3's fix scope (top-of-file reframing only). Not a fresh regression — pre-existing.
- **Verdict: iter-3 fix #2 holds for the top-of-file framing it targeted.** Body section headers using `bd-1du.4` / `bd-1du.10` as P0 labels is a known inconsistency, not a new finding.

### iter-3 fix #3 — CSV row 79/80 path correction

- Row 80 (`psync_is_name_to_ignore`): cites `crates/pcloud-backends/src/ignore_patterns.rs:192 (is_name_ignored) and :220 (is_local_path_ignored)` — both functions resolve at exactly those lines. Verified.
- (Note: CSV has 0-indexed `head -n` confusion — actual matrix data row 79/80 = file lines 80/81 since header is line 1. Fix landed on the correct semantic rows.)
- **Verdict: iter-3 fix #3 holds.**

## Carry-forward gaps (still unfixed)

- **H-4 (iter-2)**: Three public-link IPC variants (`CreateFolderPublicLinkWithOptions`, `CreateFolderUpDownLink`, `CreateScreenshotPublicLink`) — re-verified absent from `crates/pcloud-ipc/src/` and `crates/pcloud-daemon/src/dispatch.rs`. CSV rows 147/148/168 still `Partial`. **Unfixed.**
- **H-5 (iter-2)**: `Request::CryptoShareFolder` IPC variant — re-verified absent. CSV row 138 still `Partial`. **Unfixed.**

These are deferred remediation, not regressions; iter-3 fix campaign explicitly scoped them as out-of-band.

## Spot-check (5 fresh rows iter-1/2/3 didn't touch)

| Row | Feature | Status | Citation resolves? | Reachable? |
|-----|---------|--------|--------------------|-----------|
| 30  | `psync_verify_email_restricted` | Implemented | yes (`pcloud-sdk/src/lib.rs`, 5 hits) | yes |
| 55  | `psync_reset_setting` | Implemented | yes (`pcloud-store/.../settings.rs`, 5 hits) | yes (SDK + repo) |
| 100 | `psync_add_device_monitor_callback` | Rejected | n/a (correctly empty) | n/a |
| 130 | `psync_share_folder` | Implemented | yes (`pcloud-proto/src/shares_api.rs`, 13 hits) | yes (SharesRuntime + dispatch + CLI per notes) |
| 165 | `psync_link_remove_access` | Implemented | yes (`public_link_backend.rs:352` + proto + IPC `Request::RemovePublicLinkAccess` at `methods.rs:531` + runtime `5474`) | yes |

All 5 fresh rows pass.

## Summary

- iter-3's three targeted fixes all landed correctly and remain in effect.
- No new parity-truth findings.
- iter-2 H-4 / H-5 (public-link + crypto-share IPC routes) remain unfixed but were never in iter-3's scope.
- 5/5 fresh spot-checks pass.
- Parity dimension is **converging** but not yet **converged** while H-4/H-5 are open.

**Convergence: NO** — but only because of carry-forward gaps, not new findings.

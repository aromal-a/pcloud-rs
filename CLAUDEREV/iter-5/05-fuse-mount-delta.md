# Dimension 5 — FUSE / Mount: Iteration 5 Delta

**Audit date**: 2026-04-29
**Iter 1 file**: `CLAUDEREV/05-fuse-mount.md` (1 CRIT / 5 HIGH / 8 MED / 7 LOW)
**Iter 2 file**: `CLAUDEREV/iter-2/05-fuse-mount-delta.md` (+1 advisory)
**Iter 3 file**: `CLAUDEREV/iter-3/05-fuse-mount-delta.md` (+1 NEW MED — pcloud-cache duplication)
**Iter 4 file**: `CLAUDEREV/iter-4/05-fuse-mount-delta.md` (0 new, 1 retraction)
**Iter 4 fix-campaign in FUSE scope**: none — confirmed by iter-4 summary table
(dim 5 row: 0 new / 1 retract / 0 regress).

Recent commits since iter-4 are documentation/review landing
(`1aab575`, `6a5641d`) and cross-stream / xplat compile fixes
(`858ce5e`, IPC adds `dc4cfa5..4b343cd`, CLI `11852f2`/`d7f09ae`,
DragonFly bring-up `7fe5f2b..8e45164`). **None touch FUSE / mount
production paths in dim-5 scope.**

---

## Re-verification of deferred findings

### CRIT-1 / FUSE-C-1 (Windows mount path never registers with reaper) — STANDS

Workspace grep `install_windows_signal_reaper|install_bsd_signal_reaper`
returns:
- `crates/pcloud-fs/src/platform/bsd.rs:340` (doc comment)
- `crates/pcloud-fs/src/platform/bsd.rs:468` (definition)
- `crates/pcloud-fs/src/platform/windows.rs:2045` (definition)

**Zero production callers.** No `mount_with_winfsp_dyn` / `register_mount`
invocation site has been added since iter-4. Tier-1 Windows commit
`cbd7203` and the iter-4 IPC/CLI commits did not wire the mount-reaper.

### HIGH (BSD reaper unwired) — STANDS

`bsd.rs:468` `install_bsd_signal_reaper` definition + iter-3-noted
test caller only. No production reference. Same as iter-3 / iter-4.

### HIGH (macOS docstring honesty gap) — STANDS

`platform/macos.rs:5` reads `**Running on a real Mac.**` followed by
"Real-hardware bring-up in progress under bd-1du.4.6." — the
opening assertion still overstates the in-progress state. Unchanged
from iter-4.

### HIGH (Linux 7s Drop settle window) — STANDS

`platform/linux.rs:841` defines
`SESSION_DROP_SETTLE_WINDOW: Duration = Duration::from_secs(2);` and
`:881` mentions a 5 s timeout — 2 s + 5 s worst-case = 7 s,
unchanged from iter-4.

### MED (pcloud-cache vs pcloud-fs page-cache duplication, iter-3 NEW-1) — STANDS

No dedup landed; both crates remain wired into the daemon.

---

## New scan: directory listing + commit pressure

`crates/pcloud-fs/src/` listing is byte-identical to iter-4
(22 files + `platform/` subdir). **No new modules.**

Recent commit pressure (last 20 commits): all xplat-compat or
review-landing — no FUSE / mount semantic changes. The
`bd-1du.4` and `bd-xplat-*` beads remain the right trackers.

---

## Convergence signal

- 4 deferred findings (CRIT-1, 3 HIGH) re-verified standing at identical
  file:line positions.
- iter-3 NEW-1 (cache duplication) re-confirmed standing.
- No new fix-campaign edits in dim-5 scope since iter-4.
- No new modules. No new unsafe surface. No new findings surfaced
  by directory diff or commit-log scan.

**Dim-5 is fully convergent for the second consecutive iteration**
(iter-4: 0 new; iter-5: 0 new). No further audit value at the
line-by-line layer.

---

## delta count: 0 new, 0 retractions, 0 regressions

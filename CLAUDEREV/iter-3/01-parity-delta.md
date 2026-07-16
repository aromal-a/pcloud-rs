# Iter-3 delta: parity

**Convergence: NO — 2 new findings (1 regression from iter-2 fix, 1 internal-contradiction surfaced by iter-2 fix), 0 retractions.**

## Re-checks performed

- Recounted CSV: 149 Implemented / 7 Partial / 0 Missing / 30 Rejected (186 rows). Truth unchanged.
- Re-verified the 7 Partial rows (93, 124, 138, 142, 147, 148, 168) have current notes and unimplemented IPC routes / RSA wrap.
- Spot-checked 5 fresh Implemented rows iter-1 + iter-2 did not touch (data-row indices 30, 60, 95, 130, 175). All cited files exist.
- Re-verified Rejected-rationale 1:1 (30 H3 sections in `REJECTED-RATIONALES-14042026.md` for 30 Rejected CSV rows). Holds.
- Iter-2 H-4 (3 public-link IPC routes missing): `grep CreateFolderPublicLinkWithOptions|CreateFolderUpDownLink|CreateScreenshotPublicLink` against `crates/pcloud-ipc/src/` and `dispatch.rs` → empty. **Still un-fixed; H-4 stands.**
- Iter-2 H-5 (`CryptoShareFolder` no IPC route): same grep → empty. **Still un-fixed; H-5 stands.**
- Iter-2 M-3 (CSV rows 79/80 cite moved file): rows still cite `crates/pcloud-daemon/src/ignore_patterns.rs` which does not exist; real file is at `crates/pcloud-backends/src/ignore_patterns.rs`. **Still un-fixed; M-3 stands.**
- Iter-1 H-1 / H-2 (bd-1du.* ghost IDs): `grep '"bd-1du' .beads/issues.jsonl` → 0 matches; CLAUDE.md still references `bd-1du` 21 times, STATUS.md 51 times.

## New findings

### HIGH

#### H-6 (NEW, regression from iter-2 fix DOC-HIGH-1) — STATUS.md self-contradicts on the headline parity tally

**Severity**: HIGH (the iter-2 fix to align STATUS.md to CSV truth was incomplete and now makes the same file disagree with itself)
**Files**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/STATUS.md:27` — `**Headline (2026-04-30, CSV-truth): 149 / 7 / 0 / 30 (186 rows).**`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/STATUS.md:649-659` — section "## Current Parity Matrix Tally" still says `Implemented 150 / Partial 6 / Missing 0 / Rejected 30`.
**Evidence**:
```
L27:  **Headline (2026-04-30, CSV-truth): 149 / 7 / 0 / 30 (186 rows).**
L656: | Implemented  | 150   |
L657: | Partial      | 6     |
```
The iter-2 fix campaign (`CLAUDEREV/iter-2-fixes.md` row "DOC-HIGH-1 / iter-2 dim 1 H-3") claimed STATUS.md was "Aligned to CSV truth `149 / 7 / 0 / 30`; added all 7 Partial rows incl. row 93; replaced contradictory tables". The headline was updated but the secondary tally table at L649-659 was missed, so the same file now disagrees with itself by ±1 (Implemented) / ∓1 (Partial).
**Impact**: Any reviewer who reads the "Current Parity Matrix Tally" section gets 150/6, not 149/7. The same drift class iter-1 / iter-2 caught between docs is now *inside* STATUS.md itself.
**Remediation**: Update `STATUS.md:656,657` to `Implemented 149 / Partial 7`. Also re-scan STATUS.md for any other stale `150 / 6` or `153 / 5` headlines outside the historical/audit-log section.

### MEDIUM

#### M-4 (NEW, internal contradiction surfaced by iter-2 fix DELTA-HIGH-1) — `CLAUDE.md` declares `bd-1du.*` IDs "historical" then lists three of them as currently-open parity beads

**Severity**: MEDIUM (docs contradiction; user impact is low because `STATUS.md` already says "no open beads", but the same file disagrees with itself)
**Files**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/CLAUDE.md`
**Evidence**:
- `CLAUDE.md:53-54` (added by iter-2 fix DELTA-HIGH-1): "The `bd-1du.*` IDs referenced in older sections of this file are historical;"
- `CLAUDE.md:65-67` (top of file, normative section "Open parity epics/tasks"): lists `bd-1du`, `bd-1du.4`, `bd-1du.10` as the open epics.
- `.beads/issues.jsonl`: 0 records matching `"id":"bd-1du`. So the L65-67 "open beads" list contains IDs that do not exist in the tracker, yet L53-54 of the same file says those IDs are historical.
**Impact**: A reader following the `## Current Truth` section at L65-67 gets a list of IDs to chase that the tracker has no record of, while the same file 12 lines earlier tells them to ignore those IDs.
**Remediation**: Either (a) replace L65-67 with a description of the open work that does **not** reference `bd-1du.*` IDs, or (b) recreate the three `bd-1du` records in the tracker so the IDs become live again. Recommended: (a), to align with the iter-2 fix intent.

## Retractions

None.

## Carry-forward (still valid from iter-1 + iter-2)

- iter-1 H-1, H-2 (bd-1du.* ghost IDs across CLAUDE.md / STATUS.md not in tracker) — still hold; iter-2's "historical" note partially mitigates but creates M-4.
- iter-2 H-4 (3 public-link IPC variants missing) — still un-fixed (deferred per `iter-2-fixes.md`).
- iter-2 H-5 (`Request::CryptoShareFolder` missing) — still un-fixed.
- iter-2 M-3 (CSV rows 79/80 stale `pcloud-daemon/src/ignore_patterns.rs` path) — still un-fixed.
- iter-2 M-1 (rows 124/142 status-semantics) — unchanged.
- iter-1 R-1/R-2 retractions — already absorbed.

## Non-findings (verified clean this iteration)

- Rejected-rationale 1:1 (30/30) holds after iter-2 fixes.
- Headline CSV count is still 149/7/0/30 (no new flips).
- Five fresh spot-checks (rows 30, 60, 95, 130, 175) all reachable.
- API-REFERENCE.md row 93 fix (iter-2 DOC-HIGH-2) did not introduce a contradiction — the row 93 entry is internally consistent with the CSV and with `TransferRuntime::upload_write_from_file` returning a stub error.
- iter-2 fix DEPLOY-H-11.3 (systemd `IPAddressDeny` block removal) — out of scope for parity dimension; not a parity regression.

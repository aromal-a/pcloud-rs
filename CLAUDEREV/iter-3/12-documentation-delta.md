# Iteration 3 — Documentation Quality Delta (regression-check focus)

Scope: re-verify iter-2 fixes for regressions and dangling references.
Date: 2026-04-29.

## Verification matrix vs iter-2-fixes.md

| Iter-2 fix | Re-verify outcome |
|---|---|
| DOC-HIGH-1 (STATUS.md count alignment to 149/7/0/30) | **REGRESSION** (see DELTA-HIGH-3-1) |
| DOC-HIGH-2 (API-REFERENCE row 93) | OK — row 93 entry coherent, marked Partial |
| DOC-HIGH-3 (install.md `pcloudd` rename + MSRV + man pages) | OK — `grep "pcloud-daemon" docs/book/src/getting-started/install.md` returns 0 hits |
| DOC-HIGH-4 (8 ADR stub files + SUMMARY + index) | OK — all 8 `{{#include ../../../adr/00NN-...md}}` paths resolve to real files under `docs/adr/` |
| DOC-MEDIUM-3 (README crate count) | (not re-checked this pass; iter-2 said landed) |
| DELTA-HIGH-1 (CLAUDE.md RUST-PLANS removal) | OK — only one mention left, framed as historical (line 45) |
| DELTA-HIGH-2 (SECURITY.md auth_backend path) | OK — `crates/pcloud-backends/src/auth_backend.rs` cited at line 60; all other SECURITY.md path citations also resolve to real files |

## NEW FINDINGS

### DELTA-HIGH-3-1 — STATUS.md inline tally table contradicts headline (regression of DOC-HIGH-1)

**Severity**: HIGH (regression). The iter-2 fix for DOC-HIGH-1 updated the
top headline of STATUS.md to `149 / 7 / 0 / 30` but **did not update the
inline summary table** at lines 656-657, which still reads:

```
| Implemented  | 150   |
| Partial      | 6     |
```

This re-introduces the same self-contradiction iter-2 was supposed to
close. The headline (line 27), the "Remaining Partial Rows" section
(line 699), and the audit-07 distribution row (line 633) all say
`149 / 7`; but the secondary tally table (around line 656) and several
historical headline references (`152 / 6 / 0 / 28` at lines 814, 832,
879; `153 / 5 / 0 / 28` at lines 191, 222, 262; `155 / 3 / 0 / 28` at
line 472) still carry stale numbers. The iter-2 fix was scoped narrowly
to the top-of-file headline and missed every other in-file occurrence.

**Fix scope**: either (a) update line 656-657 to `149 / 7`, or
(b) annotate the historical-table sections with a header making clear
they are pre-iter-2 audit snapshots, not the current tally.
Recommend (a) for the lines 656-657 table (it has no "historical"
framing) and (b) for the audit-04/05/06 narrative blocks below.

### DELTA-HIGH-3-2 — `cargo doc --workspace --no-deps` warning count rose from 49 to 59 (+10)

**Severity**: HIGH. Iter-2 reported the warning count dropped from
54 to 49 after a targeted intra-doc-link sweep. Fresh re-run on a clean
target dir reports **59 warnings**:

- `pcloud-crypto` (lib doc): 11 warnings
- `pcloud-backends` (lib doc): 1 warning
- `pcloud-daemon` (lib doc): 4 warnings
- ...plus more from other crates not surfaced in the trailing tail.

Specific new dangling links observed in the tail include:

- `unresolved link to `ReadRangePayload``
- `public documentation for `new` links to private item `DEFAULT_RETRY_BUDGET_CAPACITY``

The +10 net regression suggests new code landed between iter-2 and
iter-3 with rustdoc warnings that were not gated. Recommend running
`cargo doc --workspace --no-deps -- -D warnings` in CI to lock the
floor.

### DELTA-MEDIUM-3-1 — `docs/book/src/operations/deployment-guide.md` still orphan

**Severity**: MEDIUM (carry-over, expected). Iter-2 DELTA-MEDIUM-2
flagged this and explicitly deferred. Re-verified: the file exists at
`docs/book/src/operations/deployment-guide.md` but has no entry in
`docs/book/src/SUMMARY.md` and no inbound link from `book.toml` or
`README.md`. mdBook will skip it on build. Tracking only — no
escalation.

## Tally

- New findings: **2 HIGH** (DELTA-HIGH-3-1, DELTA-HIGH-3-2)
- Carry-over (deferred): 1 MEDIUM
- Retractions: 0
- Regressions: **1** (DELTA-HIGH-3-1 reopens DOC-HIGH-1 territory)

## Note for parent

The iter-2 STATUS.md fix was incomplete: it patched only the
top-of-file headline and missed every other inline tally. The iter-2
rustdoc fix has been overrun by new code. Both qualify as regressions
against iter-2's claimed closure of DOC-HIGH-1 / 49-warning baseline.

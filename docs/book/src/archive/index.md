# Historical Reviews

Point-in-time audits, review reports, and superseded plans are archived
under `.archive/reviews/` in the repository. Nothing in the archive is
load-bearing for the current build or release — treat it as context for
why a given design decision was made, **not** as operational guidance.

For operational guidance use the Operations chapter and the top-level
runbook. For decisions use the [ADR index](../adr/index.md). For parity
claims use [C-to-Rust Status](../parity/status.md) and the
[`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)
source of truth.

## Two Archive Trees

There are two archive trees and they serve different purposes.

### `.archive/reviews/` — frozen wave snapshots

Each file is dated `14042026` and captures a single wave of review,
reconciliation, or audit output. These snapshots are deliberately
**not reconciled** against later code — they are historical records
of what the tree looked like when the wave ran. The rolling index is
maintained at
[`.archive/reviews/INDEX.md`](https://github.com/ezechiel203/pcloud-rs/tree/main/.archive/reviews)
(directory listing if no `INDEX.md` is present).

Representative snapshots you will find there:

- `C-BUILD-AND-DOCS-TRUTH-14042026.md` — C build reality vs docs.
- `PARITY-AUDIT-FINAL-14042026.md` — final parity audit snapshot.
- `SECURITY-AUDIT-FINAL-14042026.md` — final security audit snapshot.
- `FINAL-PARITY-PROOF-WAVE{4,6,7,8,9}-14042026.md` — proof waves.
- `RECONCILIATION-WAVE{3..9}-14042026.md` — matrix reconciliations.
- `PERF-BASELINE-14042026{,-post}.md` — pre/post perf baselines.
- `SECURITY-SWEEP-WAVE6-14042026.md` — mid-wave security sweep.
- `UPLOAD-SPEC-14042026.md` / `UPLOAD-WIRING-GAP-14042026.md` —
  upload state-machine design and wiring gap notes.
- `CLI-PARITY-AUDIT-14042026.md` — CLI surface parity snapshot.
- `REVIEW_FULL_01.md` — first full-tree review.

These files are preserved as-is because they were the evidence cited
when beads were closed. Editing them would destroy that audit trail.

### `.reviews/` — reviewer bundle outputs

The `.reviews/` tree holds the 20-reviewer and R1–R10 comparative
review bundles that fed the Wave-02 hardening pass. Each file is
one reviewer's output, with a numeric prefix indicating their seat:

- `01-code-quality.md` … `10-architecture.md` — reviewers 01–10
  on Rust-internal quality axes.
- `11-enterprise-production.md` … `20-enhancements-brainstorm.md` —
  reviewers 11–20 on enterprise/deep-dive axes (security, stability,
  CLI UX, documentation, perf, testing, architecture, parity honesty,
  enhancements).
- `R1-c-capability-audit.md` … `R10-crossplat-release.md` — ten
  comparative reviewers focused on C-vs-Rust deltas (capability
  audit, performance, stability, security, interop/migration,
  cross-platform release).
- `README.md` — overview of the R5 performance bundle and its
  top-ROI recommendations.

These are rolling reviewer outputs, not frozen snapshots. Where a
reviewer's finding has been actioned, the fix is referenced in the
relevant phase report (`PLAN_A_PLUS_P{0..6}_REPORT.md`) or ADR.
Where a finding is the rationale for an ADR — for example Reviewer
19's FUSE footnote for ADR 0010 — the ADR cites the reviewer file
by number.

## Cross-Reference Map

The table below is a conservative index from archive artifact to the
live documentation it feeds. Where a `.md` artifact is not yet paired
with a live doc, the destination is marked `—`.

| Archive artifact | Current doc | Notes |
|---|---|---|
| `.archive/reviews/PARITY-AUDIT-FINAL-14042026.md` | [Parity Status](../parity/status.md) | Historical source for an earlier parity tally; see [`STATUS.md`](../../../../STATUS.md) for current counts. |
| `.archive/reviews/SECURITY-AUDIT-FINAL-14042026.md` | [Security Model](../security/model.md) | Feeds the secrets/IPC rules. |
| `.archive/reviews/UPLOAD-SPEC-14042026.md` | ADR [0008](../adr/0008.md) | Buffer-size ADR citation. |
| `.archive/reviews/UPLOAD-WIRING-GAP-14042026.md` | ADR [0010](../adr/0010.md) | FUSE/write-path gap. |
| `.reviews/19-parity-honesty.md` | [Parity Status](../parity/status.md) | Three risk-of-misread footnotes. |
| `.reviews/R5-performance-comparison.md` | Performance (see [FAQ](../faq.md)) | Top-ROI perf experiments. |
| `.reviews/07-performance.md` / `16-performance-deep.md` | Performance (see [FAQ](../faq.md)) | Benchmark coverage gaps. |
| `.reviews/12-security-deep-dive.md` / `R7-security-comparison.md` | [Threat Model](../security/threat-model.md) | Residual risk register. |
| `.reviews/18-architecture-deep.md` / `R3-comparison-delta.md` | [Architecture Overview](../architecture/overview.md) | Crate-split rationale. |

The mapping is intentionally incomplete: if a reviewer finding did
not produce a live doc change it stays orphan in `.reviews/` with no
link in this chapter. Orphan findings are either deferred (tracked
in `bd`) or rejected with rationale in the archive itself.

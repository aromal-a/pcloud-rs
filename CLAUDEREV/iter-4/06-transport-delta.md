# Iteration 4 — Transport (HTTP API) & Network Resilience — Delta

**Scope**: Section 6 of `pcloud_rev.md` — Transport / Network Resilience.
**Mode**: Read-only delta vs iter-1 (`CLAUDEREV/06-transport.md`), iter-2
(`CLAUDEREV/iter-2/06-transport-delta.md` — 0 findings), and iter-3
(`CLAUDEREV/iter-3/06-transport-delta.md` — 0 findings).
**Date**: 2026-04-29.

## Verification of iter-3 fix-campaign edit

The iter-3 fix campaign touched exactly one transport-relevant location:

- `crates/pcloud-resilience/src/transport.rs:553` — doc-comment intra-doc-link
  change to `TYPED_ERR_PREFIX`.

Verified by reading lines 540–564 of the current source:

- Line 542 still defines `pub(crate) const TYPED_ERR_PREFIX: &str =
  "pcloud-resilience:typed:";` — visibility unchanged (`pub(crate)`),
  type unchanged (`&str`), value unchanged (`"pcloud-resilience:typed:"`).
- Line 553 contains the textual reference `` `TYPED_ERR_PREFIX` ``
  (backtick-quoted code span) instead of an intra-doc link. The
  parenthetical "(see `TYPED_ERR_PREFIX` — private; intra-doc link
  disabled per CLAUDEREV iter-3 fix)" is a pure rustdoc comment.
- No function signature, no exported item, no runtime path is altered.
  `rustdoc` will emit a code span instead of attempting (and warning on)
  an intra-doc link to a `pub(crate)` private symbol.

**Conclusion**: The iter-3 edit is a docs-only cosmetic change with zero
behavioral, ABI, or API surface impact. No new finding, no retraction,
no regression.

## Standing items (status carry-forward, no change)

- **TRANSPORT-H-1** — production backends bypass `ResilientTransport`
  and call `reqwest` directly. Still open. Not a regression: state
  unchanged from iter-1 / iter-2 / iter-3. Tracked under
  `pcloud-rs-8mb` / TRANSPORT epic.
- **TLS revocation default-off** — tracked under `pcloud-rs-t9o`. No
  change in posture this iteration.

## Convergence

Three consecutive read-only iterations (iter-2, iter-3, iter-4) report
0 new findings, 0 retractions, 0 regressions on Section 6. The only
delta over the window is a single-line rustdoc cosmetic edit which has
been independently verified to be runtime-inert. Section 6 is
**converged** for the audit purposes. Open standing items remain
tracked in their respective beads and are not new findings.

delta count: 0 new, 0 retractions, 0 regressions

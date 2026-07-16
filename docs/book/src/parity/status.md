# C-to-Rust Parity Status

The authoritative tally lives in
[`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)
at the repository root. This page reproduces the current
counts for convenience and cross-references the closure checklist and
reviewer footnotes. If the numbers here ever drift from `STATUS.md`,
**`STATUS.md` wins** — update it first and reconcile back here.

## Sync Mechanism (audit-06 LOW deployment / pcloud-rs-ncx.87-d)

This page does NOT hard-code counts — every concrete tally link
points back to `STATUS.md`. The sync protocol is manual but
deliberate:

1. Reviewers update `STATUS.md` only, as part of closing any parity
   bead or flipping a matrix row.
2. This page (`docs/book/src/parity/status.md`) contains only
   qualitative narrative plus hyperlinks to `STATUS.md`; there is
   nothing numeric to re-sync.
3. If a future reviewer adds a count here, the CI
   `parity-docs-consistency` check (planned follow-up) must flag the
   divergence. Until that check lands, the rule is enforced by
   review.

The matrix CSV (`C_FEATURE_PARITY_MATRIX.csv`) is a separate
authoritative artefact; `STATUS.md` derives its Implemented/Partial/
Rejected counts directly from the CSV. Do not edit counts in the CSV
without re-running the derivation and updating `STATUS.md` in the
same commit.

## Current Matrix Tally

Row source:
[`C_FEATURE_PARITY_MATRIX.csv`](https://github.com/ezechiel203/pcloud-rs/blob/main/C_FEATURE_PARITY_MATRIX.csv).
Narrative review:
[`C_FEATURE_PARITY_REVIEW.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/C_FEATURE_PARITY_REVIEW.md).

For the current Implemented / Partial / Missing / Rejected counts, see
[`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)
— the single source of truth. Do not hard-code counts in this chapter.

Rejected-row per-item justification lives in
[`REJECTED-RATIONALES-14042026.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/REJECTED-RATIONALES-14042026.md).

## Parity Tracking

The historical `bd-1du.*` and `gptrev-01` labels in older review notes are
provenance, not current release evidence. `STATUS.md` plus
`C_FEATURE_PARITY_MATRIX.csv` are the active tracking source. See ADR
[0009 — Parity Matrix Truth Source](../adr/0009.md) for why `STATUS.md` is
authoritative.

The current matrix has no Partial or Missing rows. That does **not** establish
production, enterprise, or drop-in replacement readiness: live pCloud,
native-mount, signed-package, upgrade, and appliance gates are separate and
remain binding.

## Closed Partial Rows

The seven Partial rows recorded on 2026-04-30 were closed by the May 1 parity
work. In particular, SDK row 94 now drives the production chunked driver and
threads `ConflictMode` to `upload_save`; crypto/team shares and the public-link
variants have user-facing IPC routes. For current evidence and exact counts,
see [`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md).

The footnotes below are retained as reviewer context but have been updated to
avoid stale row-status claims. Linux FUSE row-level parity is now `Implemented`;
macOS/Windows live-host mount proof and the final reviewer sign-off remain
release-gating evidence, not extra `Missing` rows.

Closure of the remaining gate items is tracked in the
[`bd-1du.10` closure checklist](https://github.com/ezechiel203/pcloud-rs/blob/main/docs/parity/bd-1du-10-closure-checklist.md)
alongside this chapter.

## How to Read the Matrix

`Implemented` in the matrix means "a C equivalent exists and is
exercised on an identified retained Rust code path." It does **not**
automatically mean "fully integrated into the daemon runtime end-to-end
on a live mount." FUSE write-path rows are the most load-bearing
example of this distinction — see footnote [^fuse-wiring] and ADR
[0010 — FUSE Write-Path Daemon Wiring Pending](../adr/0010.md).

## Risk-of-Misread Footnotes (Reviewer 19)

Reviewer 19's parity-honesty audit (grade B+) flagged three rows that
are classified correctly but easy to misread. Their footnotes are
reproduced below verbatim so readers do not need to cross-open the
review.[^fuse-wiring][^upload-session-stubs][^sdk-shell-scope]

[^fuse-wiring]: **FUSE trait methods vs daemon wiring.** This Reviewer-19
    warning is superseded for the Linux row-level verdict: row 85 is
    `Implemented` after the Linux mounted-drive path was wired and live-verified.
    The remaining caution is platform/release evidence: macOS `fuse-t` and
    Windows WinFSP still need real hardware live-host proof before release-grade
    claims.

[^upload-session-stubs]: **`UploadSession` pause/resume/cancel.**
    This warning is superseded. `EmbeddedDaemon::start_upload` drives the
    production `RuntimeUploadDriver`, and `ConflictMode` reaches the save
    frame. Live release qualification remains a separate gate.

[^sdk-shell-scope]: **`embedded library shell` scope.** Row 187
    (`sdk,embedded library shell`) is `Implemented` for the SDK control-plane
    scope listed in the matrix. Remaining work is release/platform proof, not
    a change to row 187's implemented verdict.

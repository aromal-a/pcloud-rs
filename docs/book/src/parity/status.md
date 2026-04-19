# C-to-Rust Parity Status

The authoritative tally lives in
[`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)
at the root of the `` tree. This page reproduces the current
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

## Open Parity Beads

- `bd-1du` — Close verified C-to-Rust feature parity gaps (epic)
- `bd-1du.4` — Replace filesystem shell with real mounted-drive parity
- `bd-1du.10` — Prove and gate final C parity claims

`bd-1du.10` is still open. Do **not** claim full parity, production
readiness, enterprise readiness, or drop-in replacement status while it
remains open. See ADR
[0009 — Parity Matrix Truth Source](../adr/0009.md) for why `STATUS.md`
is authoritative.

## Remaining Partial Rows

For the authoritative list of current `Partial` rows see
[`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md).
As of Audit 03 (2026-04-18), two genuine Partial rows remain:

- **Row 93** (`transfers,upload_writefromfile`) — proto encoder exists;
  `Request::UploadWriteFromFile` IPC variant and CLI caller are not yet
  wired. TODO at `crates/pcloud-backends/src/transfer_backend.rs:601-613`.
- **Row 149** (`links,ptree_public_link`) — id-based IPC is wired
  end-to-end; path-based CLI variant resolves paths client-side rather
  than via a dedicated daemon-side IPC variant.

The footnotes below (Reviewer 19 risk-of-misread notes) are retained
because they explain nuances of the flipped rows that auditors should
still understand — in particular that daemon-side FUSE mount-lifecycle
wiring and live-host proof are still owed under `bd-1du.4`, and the
final parity-proof gate under `bd-1du.10` (Reviewer-19 regrade +
closing-commit SHA) is still open.

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
review.

[^fuse-wiring]: **FUSE trait methods vs daemon wiring.** The
    `FuseAdapter` write path landed (`PcloudFsShim::create/write/flush/fsync/unlink/rename`
    in `crates/pcloud-fs/src/fuse_adapter.rs:845+`), but the
    **daemon mount lifecycle wiring is still pending**:
    `crates/pcloud-daemon/src/mount_runtime.rs:324` shows only
    placeholder dispatch — no real `WritePathService` instance is
    created on daemon mount lifecycle, and the
    `tests/fuse_mount_integration.rs` integration exercises
    `WritePathService` directly, not a live kernel FUSE mount. A
    live mounted-drive host run has not yet been executed. Tracked
    under `bd-1du.4.6`.

[^upload-session-stubs]: **`UploadSession` pause/resume/cancel.**
    These publish state transitions but are **cooperative stubs**
    over the single-shot daemon path — see `TODO(stub)` markers in
    `crates/pcloud-sdk/src/upload_session.rs`. The SDK exports them
    as `pub fn` which looks production-ready; wire semantics will
    only unlock once the chunked upload state machine (matrix
    row 93) lands in the daemon.

[^sdk-shell-scope]: **`embedded library shell` scope.** Row 187
    (`sdk,embedded library shell`) reflects **SDK control-plane
    scope** (auth/TFA/transfers/account/backup/folder helpers).
    The row lists 20+ helpers under one label; remaining breadth
    is FS-level library helpers tied to `bd-1du.4`. The row will
    stay Partial until mounted-drive parity lands.

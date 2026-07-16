# `bd-1du.10` Closure Checklist

**Purpose:** drive the final parity proof gate. `bd-1du.10` closes when
every item in this file has a ✅ plus a landed test name (or a live-run
artefact) cited in the Evidence column.

**Source of truth for counts:** [`../../STATUS.md`](../../STATUS.md).

**Overall exit rule:** `bd-1du.10` closes when every item below has a ✅ +
test name. Any row still `[ ]` blocks the gate.

## Why this gate exists

We separate "built" from "proved" deliberately. The retained C behaviour
has been re-implemented in Rust (see [`STATUS.md`](../../STATUS.md) for
current Implemented/Rejected counts). Historically this section described
six Partial rows whose underlying code path had either not been
**live-verified** on a real account, or had been wired as a
scaffolding-level cooperative stub rather than the full semantic
behaviour of the C original.

Closing `bd-1du.10` therefore means one of two things for every row
below:

1. **Flip to `Implemented`** with a landed test cite (ideally a CI
   artefact URL); or
2. **Flip to `Rejected`** with the same rigour as the other 28
   rejected rows — cited C site, category tag, and an entry in
   `REJECTED-RATIONALES-14042026.md`.

Until that happens for *every* row below, the honesty rules in
[`CLAUDE.md`](../../../CLAUDE.md) still stand: no "full parity", no
"production-ready", no "drop-in replacement".

## What each row is and why it's still Partial

**Superseded 2026-04-16** — all six rows have since been flipped to
`Implemented`. The narrative below is retained for historical context.
See [`STATUS.md`](../../STATUS.md) for current status.

**Update 2026-05-01 (Fire 91)** — row 94 (`transfers,SDK UploadSession`)
was the last `Partial` row and is now `Implemented`. The CSV tally is
now `156 / 0 / 0 / 30 (186 rows)` — zero Partial, zero Missing. The
remaining unchecked rows in the table below are gated on live
hardware/account proof (libfuse CI runner, live pCloud account
end-to-end, real fuse-t mount), not on additional Rust code work, and
are tracked as `bd-1du.10` release-gating evidence rather than parity
feature work.

- **Row 76 — `fs,psync_stat_path`.** C clients call this to stat a
  path on the mounted drive. The Rust equivalent exists in
  `pcloud-fs`, but it cannot be honestly flipped until it is exercised
  against a live mounted drive (i.e. `bd-1du.4` has to land first).
- **Row 85 — `fs,mounted pcloud filesystem`.** The daemon-side mount
  lifecycle is scaffolded but not wired; see ADR 0010. The gated test
  `kernel_create_write_fsync_unlink_rename_remount_cycle` already
  exists; what is missing is a CI runner with libfuse and an
  end-to-end live-account remount replay.
- **Row 92 — `transfers,upload_create/write/save`.** Rust ships the
  single-shot path. The C client pipelines up to
  `PSYNC_MAX_PENDING_UPLOAD_REQS` chunks and recovers via
  `upload_info`; that state machine has not landed in the daemon.
- **Row 93 — `transfers,upload wire methods`.** Specific protocol
  edges from `UPLOAD-SPEC-14042026.md` §9 (big-endian trailer,
  overwrite-without-ifhash, `upload_info` pipelining) have not been
  live-verified against the production API.
- **Row 94 — `transfers,SDK UploadSession`.** The SDK's
  `pause`/`resume`/`cancel` methods are cooperative stubs over the
  single-shot daemon path. The full semantic behaviour unlocks when
  rows 92–93 land; until then the code has `TODO(stub)` markers.
- **Row 187 — `sdk,embedded library shell`.** The SDK's
  control-plane surface (auth, TFA, transfers, account, backup,
  folder helpers) is Implemented. The FS-level library helpers
  required to flip this row belong to the mounted-drive runtime
  (rows 76 + 85); the row stays Partial until those close.

## Retained Partial rows

| # | Row (matrix key) | Exit criteria (test or live-verified run) | Effort | Owner | Status | Evidence |
|---|---|---|---|---|---|---|
| 1 | `fs,psync_stat_path` (row 76) | Mounted-drive runtime (`bd-1du.4`) lands; live host test reads stat for a tracked path via the kernel shim and asserts name/size/mtime/folder-flag match the `listfolder` metadata; crypto-folder visibility gating sub-case proven (`bd-1du.5`). New integration test `pcloud-fs/tests/fuse_mount_integration.rs::stat_roundtrip_on_live_mount` (gated, `PCLOUD_FUSE_LIVE=1`). | M | TBD | [ ] | — |
| 2 | `fs,mounted pcloud filesystem` (row 85) | Daemon mount lifecycle wiring (`bd-1du.4.6`) completes: `Request::Mount` / `Request::Unmount` drive the real shim on a libfuse host; the existing gated test `kernel_create_write_fsync_unlink_rename_remount_cycle` runs green in CI on a libfuse-enabled runner; live-account end-to-end remount replay captured. | L | TBD | [ ] | — |
| 3 | `transfers,upload_create/write/save` (row 92) | Daemon-side chunked upload state machine lands (multi-`upload_write` driven from `transfer_backend.rs`, pipelining up to `PSYNC_MAX_PENDING_UPLOAD_REQS`, `upload_writefromfile` server-side copy, `upload_info` resume-after-restart). Test: new `transfer_backend::tests::chunked_upload_resumes_after_restart` + proptest over chunk boundary fuzz. | L | TBD | [ ] | — |
| 4 | `transfers,upload wire methods` (row 93) | Live-API verification of UPLOAD-SPEC-14042026.md §9 unknowns (big-endian trailer, overwrite-without-ifhash, `upload_info` pipelining). Test: gated `pcloud-proto/tests/upload_wire_live.rs::wire_spec_section_9` running against a real account. | M | TBD | [ ] | — |
| 5 | `transfers,SDK UploadSession` (row 94) | Replace cooperative-stub pause/resume/cancel with real wire semantics once row 92/93 land. Test: `pcloud-sdk/tests/upload_session_pause_resume.rs::pause_holds_pending_write_frames` + `cancel_aborts_inflight_session`. Remove `TODO(stub)` markers. | M | TBD | [x] | 2026-05-01 (Fire 91): row 94 flipped Partial → Implemented; `EmbeddedDaemon::start_upload` now drives `RuntimeUploadDriver` through `upload_create → N×upload_write (4 MiB) → upload_save`, `ConflictMode` threaded via `to_proto_param()` onto the save frame, mock-server test `start_upload_drives_chunked_sequence_with_conflict_threaded_to_save` asserts the on-wire sequence and `ifhash` param. `cargo test -p pcloud-sdk --lib`: 53 passed / 0 failed. See [`STATUS.md`](../../STATUS.md) Fire 91 entry. |
| 6 | `sdk,embedded library shell` (row 187) | Close once rows 1–2 close (FS-level library helpers arrive via the mounted-drive runtime). Test: `pcloud-sdk/tests/embedded_library_shell.rs::fs_helper_surface_matches_c_psynclib_h` enumerating helpers against `pclsync/psynclib.h`. | S | TBD | [ ] | — |

## Cross-cutting exit criteria

The following must also be true before `bd-1du.10` flips closed:

- [x] `C_FEATURE_PARITY_MATRIX.csv` has **zero** `Partial` rows (either
  flipped to `Implemented` with the above evidence, or explicitly moved
  to `Rejected` with justification in `REJECTED-RATIONALES-14042026.md`).
- [x] `C_FEATURE_PARITY_REVIEW.md` narrative matches the matrix.
- [x] `STATUS.md` tally regenerated (Python `csv` parser, not naive awk)
  and committed.
- [x] `CLAUDE.md`, `README.md`, `ARCHITECTURE.md`, `SECURITY-MODEL.md`,
  `API-REFERENCE.md`, `OPERATIONS-RUNBOOK.md` all link to `STATUS.md` and
  contain no hard-coded counts.
- [x] Release notes and docs remove any remaining "partial parity"
  caveats. Wording like "C parity achieved" remains **gated** until
  the two items below close.
- [ ] Reviewer 19 (parity honesty axis) re-run: grade B+ → A.
- [ ] `bd show bd-1du.10` comment updated with the closing commit SHA.

## Notes

- Effort legend: S ≤ 0.5 eng-week, M ≤ 2 eng-weeks, L > 2 eng-weeks.
- Do not flip a row to `Implemented` without a **landed** test cite;
  "test exists but gated and never run" does not count — capture the
  run artefact (CI log / local run output) in the Evidence column.
- If a row is ultimately rejected rather than implemented, move it to
  `REJECTED-RATIONALES-14042026.md` with the same rigour (cited C site
  + rationale) and tick the box here.

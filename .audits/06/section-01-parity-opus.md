# Audit 06 §1 — C-to-Rust Feature Parity & API Coverage (Opus)

Date: 2026-04-18 (post audit-05)
Auditor: Opus 4.7 (1M ctx)
Scope: Section 1 only — parity matrix vs source tree; STATUS/CLAUDE/CSV
consistency; verification that the 5 Partial rows remain genuinely Partial;
rot hunt since audit-05.

## Summary

Post audit-05 the parity tally is honest. All 5 Partial rows are real and
backed by grep-verified evidence. STATUS.md, CSV, and the dominant
CLAUDE.md narrative agree on **153 / 5 / 0 / 28 (186)**. Two small
residual drifts remain (stale section header + stale TODO line pointer).
No new rot was introduced in audit-05's row-93 rewire.

## Verification of the 5 Partial rows

All 5 rows remain genuinely Partial — workspace grep and file reads confirm
the gaps described in STATUS.md §"Remaining Partial Rows" (lines 586-618).

- **Row 23 (file row 26 in matrix) `psync_tfa_has_devices`** — grep for
  `has_devices|hasdevices` in `crates/` returns zero hits. Genuine.
- **Row 24 (file row 27) `psync_tfa_type`** — grep for
  `tfa_type|TfaType|tfatype` in `crates/` returns zero hits. Genuine.
- **Row 93 `upload_writefromfile`** — encoder exists at
  `crates/pcloud-proto/src/methods/upload.rs:266` (`UploadWriteFromFileRequest`);
  IPC variant rewired to C primitive shape
  (`crates/pcloud-ipc/src/methods.rs:1058` — fields `upload_session_id,
  source_fileid, source_hash, offset, count`); proptest at
  `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:628`; daemon
  handler at `crates/pcloud-daemon/src/runtime.rs:2705-2716` returns the
  documented "not yet wired" stub; rate-limit bucket mapped to
  `Expensive` at `crates/pcloud-daemon/src/rate_limit.rs`. TransferRuntime
  method absent as claimed. Genuine Partial.
- **Row 124 `psync_crypto_share_folder`** — `share_temppass.rs:53` uses
  `hmac::{Hmac, Mac}` / HMAC-SHA256 (line 33-40 docstring explicit);
  `pclsync_rsa.rs` exists for other flows but is not wired into
  `ShareTempPass::sign`. Genuine Partial per `bd-1du.5`.
- **Row 142 `psync_crypto_account_teamshare`** — same root cause as row
  124. Genuine Partial.

## MEDIUM findings

### M-1 — CLAUDE.md header labelled "post Audit 03" despite audit-05 content

`CLAUDE.md:52` still reads `## Current Truth (2026-04-18, post Audit 03)`
while the body (lines 66-68, 60) has been corrected to audit-05 counts.
Header/body mismatch creates confusion for any agent that greps for the
latest audit state.

Remediation: change line 52 to `## Current Truth (2026-04-18, post
Audit 05)`.

### M-2 — CLAUDE.md residual "two IPC-wiring gaps" claim

`CLAUDE.md:78` says _"the remaining parity work is narrow: two IPC-wiring
gaps plus cross-platform mount hardware verification"_. Audit-05
increased Partial from 2 to 5 (rows 26, 27, 124, 142 added to row 93).
This sentence is stale — only row 93 is a pure IPC-wiring gap now.

Remediation: reword to match STATUS.md — "five Partial rows (two TFA
query surfaces, one upload IPC gap, two share_temppass RSA gaps) plus
cross-platform mount hardware verification".

### M-3 — CSV row 93 and STATUS.md both cite stale TODO line number

The actual `TODO(bd-1du)` block for `upload_writefromfile` lives at
`crates/pcloud-backends/src/transfer_backend.rs:601-613`. Both
`C_FEATURE_PARITY_MATRIX.csv` row 93 narrative and `STATUS.md:609` say
`transfer_backend.rs:445`. Line 445 is inside `download_to_path`, not
the upload path.

Remediation: retarget both citations to line 601. Cheap grep-and-repair.

## LOW findings

### L-1 — CLAUDE.md §"Still not full parity" sync paragraph is outdated

`CLAUDE.md:180-188` still talks about "runtime engine is still simplified
versus the C daemon". Since audit-04/05 the sync loop is spawned at
daemon startup (`sync_loop_runtime.rs`, 690 LOC) and STATUS.md §"Sync
Loop Wiring (2026-04-16)" documents this. The paragraph understates
reality. Not a parity claim, just stale narrative.

### L-2 — CLAUDE.md duplicate "Primary files" block under public-link section

`CLAUDE.md:261-265` duplicates the public-link primary-files list (a
second `Primary files:` block appears after the account-utility section).
Cosmetic; not a parity-truth issue.

## Negative findings (clean)

- **No fabricated "full parity" / "production ready" / "drop-in
  replacement" claims** in CLAUDE.md, README.md, or STATUS.md. The
  honesty gate set in CLAUDE.md:80-87 holds.
- **CSV tally** verified `153 Implemented / 5 Partial / 0 Missing / 28
  Rejected / 186 total` once the multi-line quoted rationales are parsed
  correctly. STATUS.md "At a glance" (line 524-528) and "Current Parity
  Matrix Tally" (line 542-549) agree.
- **All 28 Rejected rows** still have 1:1 rationales in
  `REJECTED-RATIONALES-14042026.md` (audit-03 finding preserved; no new
  Rejected rows added in audit-04/05).
- **Audit-05 row 93 rewire is clean.** The new C-primitive-shaped
  `Request::UploadWriteFromFile` variant (`upload_session_id,
  source_fileid, source_hash, offset, count`) matches the C call
  signature; the daemon handler correctly returns a stub error rather
  than the prior OOM-prone local-file shim; the CLI surface was removed
  as promised. No new rot.
- **Offline KAT** (`pclsync_compat_kat_offline.rs`) is runnable under
  plain `cargo test` as claimed.
- **No CRITICAL or HIGH findings** in Section 1 at audit-06 time.

## Recommendations (priority order)

1. M-1, M-2, M-3 are all one-line doc edits. Land in a single commit so
   the next audit does not re-flag them.
2. L-1, L-2 are polish; fold into the next doc sweep.
3. `bd-1du.10` gate closure still blocks on the 5 Partial rows plus
   hardware verification plus human reviewer sign-off. Section 1 has no
   AI-scoped work that accelerates this; it is code-wiring (row 93) and
   new-feature scope (rows 26, 27, 124, 142).

---

End §1 audit-06.

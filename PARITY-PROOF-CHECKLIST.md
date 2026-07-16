# Parity Proof Checklist (bd-1du.10)

Single source of truth for closing the final C-to-Rust parity gate.

Counts link back to [`STATUS.md`](./STATUS.md); rationales for each
Rejected row live in [`REJECTED-RATIONALES-14042026.md`](./REJECTED-RATIONALES-14042026.md);
narrative is in [`C_FEATURE_PARITY_REVIEW.md`](./C_FEATURE_PARITY_REVIEW.md).

## Gate Criteria

- [ ] All retained matrix rows are `Implemented` with tested, reachable code
- [x] All `Rejected` rows have rationale in `REJECTED-RATIONALES-14042026.md`
      (30 rows / 30 rationales, 1:1, verified 2026-04-30)
- [x] No false "production ready" claims in docs
      (honesty constraint in `CLAUDE.md` preserved; no "production ready"
      / "full parity" / "drop-in replacement" claims found in
      `STATUS.md`, `CLAUDE.md`, `C_FEATURE_PARITY_REVIEW.md`,
      `README.md`, `ARCHITECTURE.md`, or `API-REFERENCE.md` as of
      2026-04-18)
- [x] Live verification completed for: auth, TFA, crypto, public links,
      shares, transfers, sync-add
      (see `crates/pcloud-daemon/tests/live_auth.rs` and live-gated
      test families J05 — shares, crypto, snapshot-prune, mount, rate-
      limit, drain)
- [x] FUSE: Linux read/write round-trip verified on real kernel mount
      (`crates/pcloud-fs/tests/fuse_write_path_live.rs::write_unmount_remount_readback_byte_identical`,
      gated on `PCLOUD_LIVE_E2E=1` / `PCLOUD_FUSE_TEST=1`)
- [ ] FUSE: macOS fuse-t verified (hardware — out of AI scope)
- [ ] FUSE: Windows WinFSP verified (hardware — out of AI scope)
- [x] STATUS.md counts match matrix
      (aligned to 149 / 7 / 0 / 30, 186 rows on 2026-04-30)
- [ ] Human reviewer sign-off

## Remaining Partial Rows (must be Implemented or explicitly Rejected before close)

- [ ] **Row 94** — `transfers,SDK UploadSession`. Wire public
      `EmbeddedDaemon::start_upload` to a production daemon-backed chunked
      driver, thread `ConflictMode` into upload-save/ifhash semantics, and add
      live pCloud E2E proof.
- [ ] **Rows 124, 138, 142** — crypto-share/team-share. Add IPC/daemon/CLI/SDK
      reachability for crypto share/team-share and complete live two-account
      RSA proof.
- [ ] **Rows 147, 148, 168** — public-link specialty helpers. Add
      IPC/daemon/CLI/SDK reachability for folder link with options, folder
      up/down-link send, and screenshot public links.

## Hardware-Verification Items (out of AI scope)

- [ ] macOS mount lifecycle against a real `fuse-t` install
- [ ] Windows mount lifecycle against a real WinFSP install and SCM
- [ ] BSD rc.d supervision end-to-end
- [ ] Notarisation / Authenticode signing against real signing identities
- [ ] Reproducible-build bit-identity check across two hosts

## Follow-Up (tracked under `bd-1du.4.6`, non-gating for the parity matrix)

- [ ] Chunked `upload_write` pipelining for sustained multi-GiB writes
      (`TODO(bd-1du.4.6)` in `crates/pcloud-fs/src/write_path.rs`;
      observability hook `slo_hook::observe_flush` is already wired)
- [ ] Dyn-trait `BoxedFuserShim` / generic `FuserShim<A>` write path
      object-safety work (daemon currently composes `PcloudFsShim`
      directly for writable mounts)

## Audit History

- **GPTREV Worker 4 — 2026-04-30.** API/CLI/SDK parity reconciliation.
  Row 93 closed with full upload/source offset reachability; row 94
  downgraded to Partial. Current tally: 149 Implemented / 7 Partial /
  0 Missing / 30 Rejected (186 rows).
- **Audit 03 — 2026-04-18.** Line-level matrix reconciliation under
  `bd-1du.10`. Findings: 156 Implemented / 2 Partial / 0 Missing / 28
  Rejected (186 rows). STATUS.md headline corrected from 158 / 0 / 0 /
  28 to match the CSV. Three stale citations (rows 69, 70, 75) repaired
  — files moved from `pcloud-daemon/src/` to `pcloud-backends/src/`
  in a prior refactor. All 28 Rejected rows verified against
  `REJECTED-RATIONALES-14042026.md`. 20-row spot-check of
  `Implemented` rows was clean.
- **Audit 02 — 2026-04-16.** O-wave gate run; all nine gates PASS;
  2029 passed / 0 failed / 46 ignored. No row flipped by the audit;
  rows 76, 85, 92, 93 (later revisited), 94, 187 were flipped in
  subject-matter waves that same day.
- **Audit 01 — 2026-04-14.** FINAL-PARITY-PROOF-WAVE7. Matrix
  established at 152 / 6 / 0 / 28 with rejections reviewed and
  rationale file committed.

## Current Status (2026-04-18)

| Dimension | Value |
|---|---|
| Release posture | **Pre-alpha** — gate `bd-1du.10` is open |
| Matrix counts | **149 Implemented / 7 Partial / 0 Missing / 30 Rejected** across 186 rows |
| Rejected rationales | 30 / 30 present (1:1 verified) |
| Linux FUSE live proof | **Landed** (write-unmount-remount-readback byte-identical) |
| macOS / Windows FUSE live proof | **Not yet** (hardware — out of AI scope) |
| Nine-gate CI | **All green** on last run (see `STATUS.md`) |
| Open parity beads | `bd-1du`, `bd-1du.4`, `bd-1du.10` |
| Human reviewer sign-off | **Not yet obtained** |

Do **not** claim full parity, production readiness, enterprise
readiness, or drop-in replacement status while `bd-1du.10` remains
open.

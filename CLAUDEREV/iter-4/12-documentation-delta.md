# Iteration 4 — Documentation Quality Delta (regression-check focus)

Scope: re-verify iter-3 fix-campaign edits in the documentation scope.
Date: 2026-04-29.

## Verification matrix vs iter-3 fix-campaign

| Iter-3 fix | Re-verify outcome |
|---|---|
| STATUS.md L656-657 inline tally `150/6` → `149/7` | OK — table now reads `Implemented 149 / Partial 7 / Missing 0 / Rejected 30` with reconcile-note comment at line 660. Headline (L27), audit-07 row (L633-636), and the "Remaining Partial Rows" section all match. |
| CLAUDE.md "Open parity epics/tasks" section rewritten | OK — section now framed as "Open parity work (no live beads — see note below)" with three named work-items. The historical-provenance note (L80-83) explicitly says the `bd-1du.*` IDs were renamed and verifies absence via `grep '"id":"bd-1du' .beads/issues.jsonl`. No internal contradiction. Note: line 107 still contains `unless bd-1du.10 is actually satisfied …` as a frozen quote-style imperative — coherent with the historical framing above but a careful reader could read it as live-bead. Minor tone issue, not a regression. |
| `wrap_share_invitation_b64` comments at shares.rs:107 / shares.rs:343 / shares_api.rs:477 | OK — all three sites carry the same, accurate, parenthetical replacement: cross-crate intra-doc-link disabled, symbol is `pub`, gate is on the `derive_temppass_wire` path not this symbol. Matches code reality (`grep` confirms `wrap_share_invitation_b64` is `pub` in `pcloud-crypto::share_rsa` and is wired through `crypto_share_folder_rsa` / `crypto_account_team_share_rsa`). |
| transport.rs:553 `[TYPED_ERR_PREFIX]` link → plain code span | OK — line 553 reads `(see TYPED_ERR_PREFIX — private; intra-doc link disabled per …)`; no broken link. The constant `TYPED_ERR_PREFIX` at line 542 is `pub(crate)` — confirms the "private" framing. |
| CSV rows 79/80 — path updated | OK for row 80 (`is_name_to_ignore`): cites `crates/pcloud-backends/src/ignore_patterns.rs:192,220`; the file's first line confirms it is the right module. Row 79 not re-spot-checked (out of patch scope). |
| `cargo doc --workspace --no-deps` warnings | **Back to 49 (matches iter-2 baseline).** Per-crate breakdown: pcloud-engine 19 + pcloud-crypto 11 + pcloud-ipc 5 + pcloud-proto 5 + pcloud-daemon 4 + pcloud-fs 4 + pcloud-backends 1 = 49. The 59-warning regression flagged in iter-3 has reverted (likely transient / the new dangling links were fixed). |
| `docs/book/src/operations/deployment-guide.md` orphan | **Still orphan** — no `deployment-guide` reference anywhere under `docs/book/src/`. SUMMARY.md only lists `./operations/deployment.md` (different file). Carry-over MEDIUM, no escalation. |

## NEW FINDINGS

### DELTA-MEDIUM-4-1 — CSV rows 81/82/83 cite `crates/pcloud-daemon/src/folder_backend.rs` which does not exist

**Severity**: MEDIUM. The iter-3 fix-campaign repaired CSV rows 79/80
but rows 81/82/83 (`psync_check_and_create_folder`,
`psync_create_remote_folder`, `psync_create_remote_folder_by_path`)
still cite `crates/pcloud-daemon/src/folder_backend.rs` for
`FolderRuntime::check_and_create_folder` / `::create_remote_folder` /
`::create_remote_folder_by_path`.

That file was moved to `crates/pcloud-backends/src/folder_backend.rs`
during the daemon→backends refactor (verified:
`ls crates/pcloud-daemon/src/folder_backend.rs` → ENOENT;
`ls crates/pcloud-backends/src/folder_backend.rs` → exists). Three CSV
rows currently dangle. Same root cause as iter-3 LOW-4 (`backup` /
`shares` rows): a refactor moved `*_backend.rs` files from
`pcloud-daemon/src/` to `pcloud-backends/src/` and the parity matrix
was not swept exhaustively.

**Fix scope**: `sed -i 's|crates/pcloud-daemon/src/folder_backend.rs|crates/pcloud-backends/src/folder_backend.rs|g' C_FEATURE_PARITY_MATRIX.csv`
on rows 81/82/83. Same pattern likely applies to other backends —
`grep -c pcloud-daemon/src/folder_backend C_FEATURE_PARITY_MATRIX.csv`
returns 3 hits, and a wider `pcloud-daemon/src/[a-z]*_backend.rs` sweep
should be run before closing `bd-1du.10`. Not escalated to HIGH because
all three rows correctly classify as `Implemented` and the wire-level
behavior described in the notes column matches the actual code at the
new path; only the citation is stale.

## Tally

- New findings: **1 MEDIUM** (DELTA-MEDIUM-4-1)
- Carry-over (deferred): 1 MEDIUM (deployment-guide orphan)
- Retractions: **2** — DELTA-HIGH-3-1 (STATUS.md tally) and DELTA-HIGH-3-2 (rustdoc 49→59) both cleared by the iter-3 fix-campaign
- Regressions: **0**

## Note for parent

The iter-3 fix-campaign in scope was clean: STATUS.md tally now
self-consistent, CLAUDE.md historical-provenance note coherent, the
three `wrap_share_invitation_b64` comments match code reality, and the
`cargo doc --workspace --no-deps` total is back at 49 warnings (the
iter-3 +10 spike has reverted). One new MEDIUM surfaced: the CSV
refactor-citation sweep was incomplete — three more rows
(81/82/83, `folder_backend`) still point at the old `pcloud-daemon`
path. Recommend a single-pass `sed` on the remaining `*_backend.rs`
citations rather than treating them row-by-row.

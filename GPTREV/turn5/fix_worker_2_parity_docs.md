# Turn 5 Fix Worker 2 — Parity Docs/Test

Input: `GPTREV/turn5/01_parity_api_cli_sdk.md`

## Outcome

- Repaired the malformed `STATUS.md` headline Markdown.
- Kept the current parity count consistent with CSV truth:
  **149 Implemented / 7 Partial / 0 Missing / 30 Rejected (186 rows)**.
- Removed or superseded current-tense claims that overstated retained-row
  completion.
- Replaced stale `bd-1du.*` / `gptrev-01` live-tracker claims with historical
  provenance language where no live bead exists.
- Documented all seven Partial rows with concrete blockers:
  row 94, rows 124/138/142, and rows 147/148/168.
- Updated the row 149 live-gated test to dispatch
  `Request::CreateTreePublicLinkFromPathTargets` with root, folder, and file
  path targets.

## Changed Paths

- `C_FEATURE_PARITY_MATRIX.csv`
- `STATUS.md`
- `C_FEATURE_PARITY_REVIEW.md`
- `CLAUDE.md`
- `docs/book/src/parity/status.md`
- `docs/book/src/faq.md`
- `docs/book/src/security/audit-dossier.md`
- `crates/pcloud-live-e2e/tests/tree_link_from_paths.rs`
- `GPTREV/turn5/fix_worker_2_parity_docs.md`

## Verification

- `python3` CSV tally check:
  `186 {'Rejected': 30, 'Implemented': 149, 'Partial': 7}`;
  Partial rows `[94, 124, 138, 142, 147, 148, 168]`.
- Targeted stale-claim grep over owned parity docs: no remaining current
  retained-row completion overclaim, stale five/six Partial-row, row 187
  Partial-row, or live historical-GPTREV tracker claims. The only row 93
  Partial hit is the explicit `Partial -> Implemented` delta in `STATUS.md`.
- `cargo fmt --package pcloud-live-e2e --check`: passed.
- `cargo test -p pcloud-live-e2e --test tree_link_from_paths --no-run`: passed.
- `cargo test -p pcloud-live-e2e --test tree_link_from_paths`: passed; live test
  remained ignored without credentials.
- `git diff --check` over owned files: passed.

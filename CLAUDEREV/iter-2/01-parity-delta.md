# Iter-2 delta: parity

**Convergence: NO — 4 new findings, 2 retractions/corrections to iter-1.**

## Re-checks performed

- All 12 less-trodden / non-C-parity-surface crates (`pcloud-fleet`, `pcloud-idp`, `pcloud-kms`, `pcloud-policy`, `pcloud-session`, `pcloud-p2p`, `pcloud-chaos`, `pcloud-mockserver`, `pcloud-plugin-{autoheal,backup-schedule,dlp,publink-expiry}`) — confirmed they are enterprise/scaffolding add-ons with no C-parity counterpart, correctly excluded from the matrix. No drift.
- File-existence sweep over every `rust_reference` cell in the matrix (after splitting on `;` and `+` separators).
- Re-counted matrix statuses (`csv.DictReader`).
- Re-verified iter-1 H-1 / H-2 (bd-1du.* IDs) — still hold, zero open beads, zero `bd-1du` records in `.beads/issues.jsonl`.
- Re-verified rejected-rationale 1:1 (30/30) — holds.
- Reviewed the four "Reachability gap (gptrev-01)" Partial rows for actual code reachability.

## New findings

### HIGH

#### H-3 (NEW) — Iter-1 mis-reported the headline parity count: actual is 149/7/0/30, not 154/2/0/30

**Severity**: HIGH (foundational claim of iter-1 is numerically wrong)
**Files**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv`
**Evidence**: `python3 -c "from collections import Counter; import csv; print(Counter(r['status'] for r in csv.DictReader(open('C_FEATURE_PARITY_MATRIX.csv'))))"` → `Counter({'Implemented': 149, 'Rejected': 30, 'Partial': 7})`. Total = 186, matches `STATUS.md` row count, but the Implemented/Partial split iter-1 reported (154/2) is wrong by **5 Partial rows**. `STATUS.md`/`CLAUDE.md` both correctly say "currently 5 Partial rows" (audit-05 baseline, two of which are 124/142). The five new ones surfaced *after* `STATUS.md`'s last edit, on the same day iter-1 ran (2026-04-29), tagged `gptrev-01` in the CSV `notes` column.
**Impact**: Iter-1 spot-checks claimed rows 147, 148, 168 are "Implemented — reachable". They are explicitly `Partial` in the CSV with a blocker noted. Iter-1 also said the matrix is "internally consistent" — it is, but iter-1 didn't read the current state.
**Remediation**: Retract iter-1's "154/2/0/30" claim; the truth is 149/7/0/30.

#### H-4 (NEW) — Three public-link wire methods exist in proto+backend but are unreachable from CLI/SDK (no IPC route)

**Severity**: HIGH (parity-claim contradicts reachability)
**Files**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/public_links_api.rs` (`create_folder_public_link_with_options`, `create_folder_updownlink`, `create_screenshot_public_link`)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/public_link_backend.rs` (same three methods reachable in-process)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs` — **NO** `Request::CreateFolderPublicLinkWithOptions`, `Request::CreateFolderUpDownLink`, `Request::CreateScreenshotPublicLink` variants.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/dispatch.rs` — no handlers.
- CSV rows 147 (`psync_folder_public_link_full`), 148 (`psync_folder_updownlink_link`), 168 (`psync_screenshot_public_link`) — all `Partial` per matrix.
**Evidence**: `grep -rE 'CreateFolderPublicLinkWithOptions|CreateFolderUpDownLink|CreateScreenshotPublicLink' crates/pcloud-ipc/src/ crates/pcloud-daemon/src/dispatch.rs` → empty. Backend code exists but cannot be invoked across IPC; only an in-process embedded SDK consumer (or a test harness) can hit them.
**Impact**: External users of the daemon cannot reach these public-link surfaces. Real parity gap, correctly flagged in CSV but missed by iter-1.
**Remediation**: Add the three IPC `Request` variants + dispatcher branches + CLI subcommands. Tracker: open the corresponding `gptrev-01 H-01` follow-up bead.

#### H-5 (NEW) — `psync_crypto_share_folder` (CSV row 138, distinct from row 124) has neither IPC route nor CLI exposure

**Severity**: HIGH (duplicate-row signal, but the second row is genuinely a separate gap)
**Files**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv:138` — Partial, notes: "duplicate row for psync_crypto_share_folder (see row 124 for RSA-4096 path). Both `SharesRuntime::crypto_share_folder` and `::crypto_share_folder_rsa` exist in backend but neither is reachable from IPC — `ShareFolder` IPC only routes to non-crypto `share_folder`. Blocker: add `Request::CryptoShareFolder` IPC variant."
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/shares_backend.rs` — both methods exist
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs` — no `Request::CryptoShareFolder`
**Evidence**: `grep -rE 'CryptoShareFolder' crates/pcloud-ipc/src/ crates/pcloud-daemon/src/dispatch.rs` → empty.
**Impact**: Crypto share-folder backend is dead code from an external-IPC perspective. Iter-1's "Crypto: 30+ rows… all reachable" claim does not hold for this surface.
**Remediation**: Add `Request::CryptoShareFolder` IPC variant, dispatcher, CLI subcommand. Tracker: `gptrev-01 H-02`. (Note: row 138 also raises a CSV-hygiene question — is this a genuine duplicate of row 124 or a separate feature? Notes treat it as a duplicate; consider merging.)

### MEDIUM

#### M-3 (NEW) — Two CSV rows cite a moved file path (`pcloud-daemon/src/ignore_patterns.rs` → `pcloud-backends/src/ignore_patterns.rs`); line numbers also stale

**Severity**: MEDIUM (parity-evidence drift)
**Files**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv:79,80` — both cite `crates/pcloud-daemon/src/ignore_patterns.rs:177` / `:199`
- Real location: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/ignore_patterns.rs:192,220` (`is_name_ignored` / `is_local_path_ignored`)
**Evidence**: `Glob **/ignore_patterns.rs` returns only the `pcloud-backends` path. Functions confirmed at lines 192 and 220.
**Impact**: A reviewer following the citation gets `file not found`. Same class of drift as audit-05 caught for rows 69/70/75 but not yet repaired for 79/80.
**Remediation**: Update CSV rows 79 and 80 to `crates/pcloud-backends/src/ignore_patterns.rs:192` (`is_name_ignored`) and `:220` (`is_local_path_ignored`).

## Retractions / corrections to iter-1

- **R-1**: Iter-1 §Summary: "186 data rows: 154 Implemented / 2 Partial / 0 Missing / 30 Rejected" — **wrong**. Actual: **149 Implemented / 7 Partial / 0 Missing / 30 Rejected** (total 186). The error propagates through iter-1's "two remaining Partial rows" claim and the "spot-check 25/25 pass" verdict (rows 147, 148, 168 in iter-1's table were marked OK, but they're Partial per CSV).
- **R-2**: Iter-1 spot-check table claims rows `145 Implemented OK`, `150 Implemented OK`, etc., bracketing rows 147/148 implicitly as Implemented. Re-classify: rows 147 (`psync_folder_public_link_full`) and 148 (`psync_folder_updownlink_link`) are **Partial**, not Implemented; iter-1's "Public links: 17+ rows… all reachable" assertion is materially wrong.

## Carry-forward (still valid from iter-1)

- H-1 (bd-1du.* ghost IDs in CLAUDE.md/STATUS.md) — verified again, still 0 matches in `.beads/issues.jsonl`.
- H-2 (CLAUDE.md "3 open beads" claim vs. zero open) — still holds.
- M-1 (rows 124/142 status-semantics) — still applies.
- M-2 (12 undocumented `pub fn` in `pcloud-sdk/src/lib.rs`) — not re-checked this iteration.
- L-1, L-2, L-3 — not re-checked, likely still apply.

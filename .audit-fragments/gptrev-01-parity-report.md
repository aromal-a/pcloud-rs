# gptrev-01 Parity/API Coverage — Remediation Report

Date: 2026-04-29
Stream: G1
Auditor: subagent 01 (read-only audit)
Remediator: Claude Sonnet 4.6 (this agent)

## Summary

| Metric | Count |
|--------|-------|
| CRITICAL findings | 0 |
| HIGH findings addressed | 4 (H-01 → fix, H-02 → fix, H-03 → fix, H-04 → fix) |
| MEDIUM findings addressed | 2 (M-01 → fix, M-02 → defer out-of-scope) |
| LOW findings addressed | 1 (L-01 → fix) |
| Total fixes | 6 |
| Total annotations | 3 (AUDIT-NOTE in lib.rs) |
| Total deferrals | 1 (M-02 — IPC schema out of scope) |

## Findings by Finding ID

### H-01 — Public-link specialty rows unreachable from daemon/CLI/SDK

**Severity:** HIGH
**Classification:** Real bug — CSV overstates Implemented parity.
**Finding:** Rows 147 (`psync_folder_public_link_full`), 148 (`psync_folder_updownlink_link`), 168 (`psync_screenshot_public_link`) were marked `Implemented` in the CSV. Backend functions exist (`PublicLinkRuntime::create_folder_public_link_with_options`, `::create_screenshot_public_link`, `::create_folder_updownlink`) but no IPC `Request` variant, daemon handler, CLI command, or SDK helper exposes any of them.

**Fix applied:**
- `C_FEATURE_PARITY_MATRIX.csv` rows 147, 148, 168: status flipped `Implemented` → `Partial`; rationale notes reachability gap and required blocker (add `Request::CreateFolderPublicLinkWithOptions`, `Request::CreateFolderUpDownLink`, `Request::CreateScreenshotPublicLink` IPC variants + daemon dispatch routes).
- `STATUS.md` "At a glance" table and "Current Parity Matrix Tally": counts updated from 154/2 to 150/6.
- `STATUS.md` "Remaining Partial Rows" section: three new Partial entries added with exact backend file:line references.
- `API-REFERENCE.md` public links table: three rows updated from `I` to `P` with reachability notes.

**Files touched:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv` (rows 147, 148, 168)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/STATUS.md`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/API-REFERENCE.md`

### H-02 — Crypto share rows duplicate and unreachable

**Severity:** HIGH
**Classification:** Real bug — row 138 (`shares,psync_crypto_share_folder`) was marked `Implemented` while being both a duplicate of row 124 and unreachable from IPC. `ShareFolder` IPC routes only to the non-crypto `share_folder` path; no `Request::CryptoShareFolder` variant exists.

**Fix applied:**
- `C_FEATURE_PARITY_MATRIX.csv` row 138: status flipped `Implemented` → `Partial`; note explains duplicate status and IPC reachability gap.
- `STATUS.md` counts updated (included in H-01 fix above).
- `API-REFERENCE.md` shares table: crypto share row updated to note IPC unreachability for both the temppass and RSA paths.

**Files touched:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv` (row 138)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/API-REFERENCE.md`

### H-03 — Stale counts in STATUS.md and API-REFERENCE.md

**Severity:** HIGH
**Classification:** Real bug — stale navigation headings.

**Fix applied:**
- `STATUS.md` "At a glance" table: `Implemented` 153 → 150, `Partial` 3 → 6 (incorporating both the audit-06 stream-c row 93 closure +1 and the four gptrev-01 downgrades −4).
- `STATUS.md` headline section for 2026-04-26 stream-c wave: clarified that subsequent gptrev-01 downgraded 4 rows; current headline is 150/6.
- `API-REFERENCE.md` transfers section: row 93 note updated from "Partial" to "Implemented" with audit-06 stream-c closure date.
- `API-REFERENCE.md` auth section: rows 23/24 updated from `P` to `R` reflecting audit-06 ncx.4 flip.

**Files touched:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/STATUS.md`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/API-REFERENCE.md`

### H-04 — Partial rows 124/142 missing canonical bead IDs in CSV

**Severity:** HIGH
**Classification:** Real bug — machine-readable parity source lacks tracker linkage.

**Fix applied:**
- `C_FEATURE_PARITY_MATRIX.csv` row 124: notes field prepended with `Tracker: bd-1du.5 / pcloud-rs-ncx.89-e2e.`
- `C_FEATURE_PARITY_MATRIX.csv` row 142: same prefix added.

**Files touched:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv` (rows 124, 142)

### M-01 — API reference documents SDK auth helpers that did not exist

**Severity:** MEDIUM
**Classification:** Real bug — `EmbeddedDaemon::login`, `::login_with_token`, `::submit_recovery_code` were referenced in API-REFERENCE.md but missing from the SDK.

**Fix applied:**
- `crates/pcloud-sdk/src/lib.rs`: added `AuthHelperError::Login(String)` variant (line ~753) with `#[error("login failed: {0}")]`.
- `crates/pcloud-sdk/src/lib.rs`: added `EmbeddedDaemon::login(&mut self, username: &str, password: &str) -> Result<(), SdkError>` — dispatches `Request::PasswordSubmission`.
- `crates/pcloud-sdk/src/lib.rs`: added `EmbeddedDaemon::login_with_token(&mut self, token: &str) -> Result<(), SdkError>` — dispatches `Request::AuthTokenSubmission`.
- `crates/pcloud-sdk/src/lib.rs`: added `EmbeddedDaemon::submit_recovery_code(&mut self, code: &str, trust_device: bool) -> Result<(), SdkError>` — delegates to `submit_two_factor_code(code, trust_device, true)`.
- Each helper carries `// AUDIT-NOTE: gptrev-01 M-01` comment.
- `API-REFERENCE.md` auth table: updated entries for login/login_with_token/submit_recovery_code to reflect actual helper names.

**Cargo check:** `cargo check -p pcloud-sdk` → `Finished` 0 errors.
**Lib tests:** `cargo test -p pcloud-sdk --lib` → 49 passed / 0 failed.

**Files touched:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/API-REFERENCE.md`

### M-02 — Stale rustdoc in IPC methods.rs says daemon handler is a stub

**Severity:** MEDIUM
**Classification:** Deferred — `crates/pcloud-ipc/src/methods.rs` is outside the allowed file scope for this stream (IPC schema).

**Deferral rationale:** `crates/pcloud-ipc/src/methods.rs:1230` reads `/// Tracker: bd-1du row 93. Daemon handler is still a stub pending / TransferRuntime::upload_write_from_file wiring.` This is stale: the daemon handler is implemented at `crates/pcloud-daemon/src/runtime.rs:3530`. The fix is a 2-line rustdoc update in methods.rs but that file is declared IPC schema and outside this stream's scope.

**Recommended fix:** Replace lines 1230-1231 with `/// Tracker: bd-1du row 93. Closed — daemon handler wired at runtime.rs:3530 (audit-06 stream-c, 2026-04-26).` Assign to the IPC stream (out-of-scope here).

### L-01 — REJECTED-RATIONALES prose references stale count "28"

**Severity:** LOW
**Classification:** Real bug — trust erosion on parity docs.

**Fix applied:**
- `REJECTED-RATIONALES-14042026.md`: replaced `"If this file's '28' count drifts..."` with a formulation that says "rejected-row count stated at the top of this file" and explains the 28→30 transition.

**Files touched:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/REJECTED-RATIONALES-14042026.md`

### C_FEATURE_PARITY_REVIEW.md — stale Partial-row narrative

**Classification:** Collateral fix required by H-01/H-02/H-03.

**Fix applied:**
- `C_FEATURE_PARITY_REVIEW.md` "What Is Actually Left" section: updated from "two Partial rows (93, 149)" to "six Partial rows (124, 138, 142, 147, 148, 168)" with closure notes for rows 93 and 149.

**Files touched:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_REVIEW.md`

## Post-remediation Parity Counts

| Metric | Before gptrev-01 | After gptrev-01 |
|--------|-----------------|-----------------|
| Implemented | 154 | 150 |
| Partial | 2 | 6 |
| Missing | 0 | 0 |
| Rejected | 30 | 30 |
| Total | 186 | 186 |

The decrease in Implemented (−4) reflects honest reachability correction:
rows 138, 147, 148, 168 were always backend-only; they should never have
been marked Implemented.

## Cargo Check Status

```
cargo check -p pcloud-proto -p pcloud-backends -p pcloud-sdk
→ Finished dev profile — 0 errors (pcloud-store pre-existing libc/log
  dependency error unrelated to this stream's edits)

cargo test -p pcloud-proto -p pcloud-backends --lib
→ 203 passed / 0 failed / 0 ignored

cargo test -p pcloud-sdk --lib
→ 49 passed / 0 failed / 0 ignored
```

Pre-existing failure in `pcloud-store` (missing `libc` and `log` crate
declarations) is not caused by this stream's changes and was present
before any edits.

## Deferred Items

1. **M-02 (IPC rustdoc stale text)**: Requires editing
   `crates/pcloud-ipc/src/methods.rs:1230` which is IPC schema —
   out of this stream's allowed scope. Should be fixed by the IPC/daemon
   stream.

2. **H-01/H-02 IPC wiring** (completing the rows to Implemented):
   Adding `Request::CreateFolderPublicLinkWithOptions`,
   `Request::CreateFolderUpDownLink`, `Request::CreateScreenshotPublicLink`,
   and `Request::CryptoShareFolder` IPC variants plus daemon dispatch
   routes. These are IPC schema changes and out of this stream's scope.
   Until they land, rows 147/148/168/138 remain Partial.

# pcloud-rs Parity/API Reachability Audit

## Date: 2026-04-29
## Auditor: subagent 01
## Scope
C-to-Rust feature parity and API reachability only. I did not modify files or write `AUDIT_REPORT.md`.

## Executive Summary
The current CSV parses as **186 rows: 154 Implemented / 2 Partial / 0 Missing / 30 Rejected**. Rejected rationale coverage is complete: all 30 rejected CSV rows have matching sections in `REJECTED-RATIONALES-14042026.md`.

Primary blockers are reachability and truth-surface drift. Several rows are marked `Implemented` while only proto/backend functions exist and no daemon IPC, CLI, or SDK path can call them. The worst cases are public-link specialty helpers and crypto-share flows. `STATUS.md`, `API-REFERENCE.md`, and `C_FEATURE_PARITY_REVIEW.md` also contain current-looking sections with stale counts and stale row states.

## Findings By Severity
### CRITICAL: 0

### HIGH: 4

#### H-01 Public-link specialty rows are implemented below backend but unreachable from daemon/CLI/SDK
**Severity:** HIGH  
**Evidence:** `C_FEATURE_PARITY_MATRIX.csv:147`, `C_FEATURE_PARITY_MATRIX.csv:148`, and `C_FEATURE_PARITY_MATRIX.csv:168` mark `psync_folder_public_link_full`, `psync_folder_updownlink_link`, and `psync_screenshot_public_link` as `Implemented`. The code exists at `crates/pcloud-proto/src/public_links_api.rs:786`, `crates/pcloud-proto/src/public_links_api.rs:820`, `crates/pcloud-proto/src/public_links_api.rs:848`, `crates/pcloud-backends/src/public_link_backend.rs:1000`, `crates/pcloud-backends/src/public_link_backend.rs:1023`, and `crates/pcloud-backends/src/public_link_backend.rs:1047`. The reachable CLI/daemon path only constructs plain `Request::CreateFilePublicLink` / `Request::CreateFolderPublicLink` at `crates/pcloud-cli/src/commands.rs:948` and `crates/pcloud-cli/src/commands.rs:951`, then daemon dispatch calls only `create_file_public_link` / `create_folder_public_link` at `crates/pcloud-daemon/src/runtime.rs:4979`.  
**Impact:** Users cannot invoke the optioned folder-link, folder up/down-link email, or screenshot public-link C-equivalent flows through the product surface. The matrix overstates retained C parity.  
**Remediation:** Add explicit IPC variants, daemon handlers, CLI commands, SDK helpers, and tests for full folder-link options, up/down-link send, and screenshot-link creation. Until that lands, flip rows 147, 148, and 168 to `Partial`.

#### H-02 Crypto share rows are duplicate/conflicting and not reachable from live callers
**Severity:** HIGH  
**Evidence:** The same C symbol appears twice with conflicting status: `C_FEATURE_PARITY_MATRIX.csv:124` marks `psync_crypto_share_folder` `Partial`, while `C_FEATURE_PARITY_MATRIX.csv:138` marks `psync_crypto_share_folder` `Implemented`. RSA/temppass code exists at `crates/pcloud-proto/src/shares_api.rs:429`, `crates/pcloud-proto/src/shares_api.rs:521`, `crates/pcloud-proto/src/shares_api.rs:562`, `crates/pcloud-backends/src/shares_backend.rs:484`, `crates/pcloud-backends/src/shares_backend.rs:564`, and `crates/pcloud-backends/src/shares_backend.rs:607`. The IPC share variants only expose non-crypto `ShareFolder` and `AccountTeamShare` at `crates/pcloud-ipc/src/methods.rs:557` and `crates/pcloud-ipc/src/methods.rs:617`; daemon dispatch likewise only routes those non-crypto variants at `crates/pcloud-daemon/src/runtime.rs:711` and `crates/pcloud-daemon/src/runtime.rs:743`.  
**Impact:** Crypto share and crypto team-share cannot be exercised by daemon/CLI/SDK users despite backend code. Row 138 is an implemented-but-unreachable retained-row claim, and rows 124/142 cannot be live-verified through the product surface.  
**Remediation:** Consolidate the duplicate `psync_crypto_share_folder` rows, add `Request::CryptoShareFolder` / `Request::CryptoAccountTeamShare` style IPC variants with CLI/SDK helpers, and require live two-account E2E before any `Implemented` status.

#### H-03 Current docs/status surfaces disagree with the CSV
**Severity:** HIGH  
**Evidence:** Python CSV parsing returned `Counter({'Implemented': 154, 'Rejected': 30, 'Partial': 2})`. `STATUS.md:58` has the correct headline, but `STATUS.md:669`, `STATUS.md:670`, `STATUS.md:692`, and `STATUS.md:693` still say `153 / 3`, and `STATUS.md:735` still claims three partial rows including row 93. `API-REFERENCE.md:40`, `API-REFERENCE.md:41`, and `API-REFERENCE.md:59` still describe rows 23/24 and 93 as partial. `C_FEATURE_PARITY_REVIEW.md:9` through `C_FEATURE_PARITY_REVIEW.md:30` still reports older counts and 28 rejected rows.  
**Impact:** The project's "single source of truth" is not actually single. Reviewers can make incorrect parity-gate decisions from current-looking sections.  
**Remediation:** Regenerate `STATUS.md`, `API-REFERENCE.md`, and `C_FEATURE_PARITY_REVIEW.md` from the CSV, or mark stale history as historical only. Add a CI check that parses the CSV and fails on mismatched counts/statuses in current sections.

#### H-04 Partial rows do not have a consistent authoritative tracker in the CSV
**Severity:** HIGH  
**Evidence:** The only CSV partial rows, `C_FEATURE_PARITY_MATRIX.csv:124` and `C_FEATURE_PARITY_MATRIX.csv:142`, reference `pcloud-rs-ncx.89-e2e` but no `bd-*` bead. `STATUS.md:61` through `STATUS.md:64` says they are tracked under `bd-1du.5`, while the CSV says RSA wrapping landed and only live E2E remains. `API-REFERENCE.md:121` and `API-REFERENCE.md:123` still say the root cause is HMAC-vs-RSA.  
**Impact:** Partial closure criteria are ambiguous, and the machine-readable parity source does not carry the required bead linkage.  
**Remediation:** Put the canonical bead IDs and exact remaining gate directly in the CSV rows. Reconcile whether the blocker is RSA implementation, live E2E, reachability, or all three.

### MEDIUM: 2

#### M-01 API reference documents SDK auth helpers that are not present
**Severity:** MEDIUM  
**Evidence:** `API-REFERENCE.md:26` and `API-REFERENCE.md:27` list `EmbeddedDaemon::login` and `EmbeddedDaemon::login_with_token`; `API-REFERENCE.md:30` lists `EmbeddedDaemon::submit_recovery_code`. The SDK auth surface shown in `crates/pcloud-sdk/src/lib.rs:2897`, `crates/pcloud-sdk/src/lib.rs:2930`, `crates/pcloud-sdk/src/lib.rs:2961`, `crates/pcloud-sdk/src/lib.rs:2991`, and `crates/pcloud-sdk/src/lib.rs:3060` exposes session helpers and TFA submission, while generic request dispatch is the escape hatch at `crates/pcloud-sdk/src/lib.rs:1376`.  
**Impact:** SDK users following the API reference cannot compile the documented login calls. Auth is reachable through generic `dispatch`, but the documented public API is inaccurate.  
**Remediation:** Either add first-class `login`, `login_with_token`, and `submit_recovery_code` SDK helpers, or update the API reference to show `dispatch(Request::PasswordSubmission/AuthTokenSubmission)` and `submit_two_factor_code(..., recovery_code=true)`.

#### M-02 `UploadWriteFromFile` rustdoc still says the daemon handler is a stub
**Severity:** MEDIUM  
**Evidence:** `crates/pcloud-ipc/src/methods.rs:1230` says the daemon handler is still a stub, but the daemon handler is implemented at `crates/pcloud-daemon/src/runtime.rs:3542` and calls `TransferRuntime::upload_write_from_file` at `crates/pcloud-daemon/src/runtime.rs:3569`.  
**Impact:** Generated IPC docs and code review context contradict the implemented row 93 state.  
**Remediation:** Update the rustdoc and cross-reference the live handler/test evidence.

### LOW: 1

#### L-01 Rejected rationale coverage is complete, but rationale prose still mentions the old 28 count
**Severity:** LOW  
**Evidence:** `REJECTED-RATIONALES-14042026.md:5` correctly says 30 rejected rows, and the row-heading check found 30/30 coverage. `REJECTED-RATIONALES-14042026.md:30` through `REJECTED-RATIONALES-14042026.md:31` still says this file's "28" count may drift.  
**Impact:** Low operational risk, but it contributes to parity-doc trust erosion.  
**Remediation:** Replace the hard-coded "28" prose with "rejected-row count" or generate it.

## Spot Checks
Rows counted individually; ranges use one shared reachability path.

| CSV row(s) | Result | Evidence |
|---|---|---|
| 8 | OK | `crates/pcloud-proto/src/notifications_api.rs:120`, `crates/pcloud-backends/src/notifications_backend.rs:199`, `crates/pcloud-daemon/src/runtime.rs:473`, `crates/pcloud-cli/src/commands.rs:920`, `crates/pcloud-sdk/src/lib.rs:2323` |
| 9 | OK | `crates/pcloud-proto/src/notifications_api.rs:150`, `crates/pcloud-daemon/src/runtime.rs:754`, `crates/pcloud-cli/src/commands.rs:923`, `crates/pcloud-sdk/src/lib.rs:2349` |
| 15-17 | OK with SDK caveat | `crates/pcloud-proto/src/auth_api.rs:335`, `crates/pcloud-backends/src/auth_backend.rs:303`, `crates/pcloud-auth/src/orchestrator.rs:188`, `crates/pcloud-daemon/src/runtime.rs:519`, `crates/pcloud-daemon/src/runtime.rs:536` |
| 20-22 | OK | `crates/pcloud-auth/src/orchestrator.rs:532`, `crates/pcloud-auth/src/orchestrator.rs:568`, `crates/pcloud-daemon/src/runtime.rs:447`, `crates/pcloud-sdk/src/lib.rs:3012`, `crates/pcloud-sdk/src/lib.rs:3033` |
| 23-24 | Rejected covered | `REJECTED-RATIONALES-14042026.md:160`, `REJECTED-RATIONALES-14042026.md:166`; API reference stale at `API-REFERENCE.md:40` |
| 28-32 | OK | `crates/pcloud-proto/src/account_api.rs:261`, `crates/pcloud-proto/src/account_api.rs:305`, `crates/pcloud-proto/src/account_api.rs:321`, `crates/pcloud-proto/src/account_api.rs:361`, `crates/pcloud-cli/src/commands.rs:1377`, `crates/pcloud-cli/src/commands.rs:1394` |
| 39-42 | OK | `crates/pcloud-proto/src/account_api.rs:135`, `crates/pcloud-proto/src/account_api.rs:211`, `crates/pcloud-daemon/src/runtime.rs:2942`, `crates/pcloud-cli/src/commands.rs:1399`, `crates/pcloud-sdk/src/lib.rs:1735`, `crates/pcloud-sdk/src/lib.rs:1761` |
| 42 | OK | `crates/pcloud-proto/src/public_links_api.rs:875`, `crates/pcloud-backends/src/public_link_backend.rs:962`, `crates/pcloud-daemon/src/runtime.rs:930`, `crates/pcloud-cli/src/commands.rs:1198`, `crates/pcloud-sdk/src/lib.rs:2416` |
| 71, 73 | OK; API stale | `crates/pcloud-daemon/src/runtime.rs:3680`, `crates/pcloud-daemon/src/runtime.rs:3692`, `crates/pcloud-cli/src/commands.rs:1038`; stale rejection at `API-REFERENCE.md:55` |
| 74 | OK | `crates/pcloud-engine/src/lib.rs:415`, `crates/pcloud-daemon/src/runtime.rs:919`, `crates/pcloud-cli/src/commands.rs:1195`, `crates/pcloud-sdk/src/lib.rs:2380` |
| 76-77 | OK | `crates/pcloud-daemon/src/runtime.rs:993`, `crates/pcloud-daemon/src/runtime.rs:1189`, `crates/pcloud-cli/src/commands.rs:1207`, `crates/pcloud-cli/src/commands.rs:1219`, `crates/pcloud-sdk/src/lib.rs:2535`, `crates/pcloud-sdk/src/lib.rs:3329` |
| 87-91 | OK | `crates/pcloud-sdk/src/lib.rs:1518`, `crates/pcloud-sdk/src/lib.rs:1559`, `crates/pcloud-sdk/src/lib.rs:1578`, `crates/pcloud-sdk/src/lib.rs:1601`, `crates/pcloud-proto/src/transfer_api.rs:205`, `crates/pcloud-backends/src/transfer_backend.rs:427` |
| 92-93 | OK; docs stale | `crates/pcloud-proto/src/transfer_api.rs:249`, `crates/pcloud-proto/src/transfer_api.rs:501`, `crates/pcloud-backends/src/transfer_backend.rs:761`, `crates/pcloud-daemon/src/runtime.rs:3542`, `crates/pcloud-cli/src/commands.rs:1317` |
| 95-98 | OK | `crates/pcloud-proto/src/backup_api.rs:106`, `crates/pcloud-proto/src/backup_api.rs:172`, `crates/pcloud-backends/src/backup_backend.rs:454`, `crates/pcloud-daemon/src/runtime.rs:3277`, `crates/pcloud-daemon/src/runtime.rs:3329`, `crates/pcloud-cli/src/commands.rs:1421` |
| 107-109 | OK | `crates/pcloud-crypto/src/lib.rs:1442`, `crates/pcloud-crypto/src/lib.rs:1594`, `crates/pcloud-crypto/src/lib.rs:1864`, `crates/pcloud-daemon/src/runtime.rs:583`, `crates/pcloud-daemon/src/runtime.rs:586` |
| 119-122 | OK | `crates/pcloud-proto/src/crypto_api.rs:118`, `crates/pcloud-backends/src/crypto_backend.rs:231`, `crates/pcloud-crypto/src/lib.rs:1944`, `crates/pcloud-crypto/src/lib.rs:1966`, `crates/pcloud-crypto/src/lib.rs:2153`, `crates/pcloud-sdk/src/lib.rs:2045` |
| 124, 142 | Partial and unreachable | `C_FEATURE_PARITY_MATRIX.csv:124`, `C_FEATURE_PARITY_MATRIX.csv:142`, `crates/pcloud-backends/src/shares_backend.rs:564`, `crates/pcloud-backends/src/shares_backend.rs:607`, no IPC variants in `crates/pcloud-ipc/src/methods.rs:557`-`631` |
| 128-129 | OK | `crates/pcloud-crypto/src/content.rs:179`, `crates/pcloud-crypto/src/metadata.rs:98` |
| 130-137 | OK | `crates/pcloud-proto/src/shares_api.rs:197`, `crates/pcloud-proto/src/shares_api.rs:246`, `crates/pcloud-proto/src/shares_api.rs:280`, `crates/pcloud-backends/src/shares_backend.rs:330`, `crates/pcloud-daemon/src/runtime.rs:7078`, `crates/pcloud-cli/src/commands.rs:1122` |
| 138 | Not reachable | `C_FEATURE_PARITY_MATRIX.csv:138`, `crates/pcloud-proto/src/shares_api.rs:429`, `crates/pcloud-backends/src/shares_backend.rs:484`, missing IPC/daemon/CLI crypto-share route |
| 141 | OK | `crates/pcloud-proto/src/shares_api.rs:386`, `crates/pcloud-backends/src/shares_backend.rs:455`, `crates/pcloud-daemon/src/runtime.rs:7296`, `crates/pcloud-cli/src/commands.rs:1156` |
| 145-146 | OK | `crates/pcloud-proto/src/public_links_api.rs:305`, `crates/pcloud-proto/src/public_links_api.rs:330`, `crates/pcloud-daemon/src/runtime.rs:4937`, `crates/pcloud-daemon/src/runtime.rs:4941`, `crates/pcloud-cli/src/commands.rs:948` |
| 147-148 | Not reachable | `C_FEATURE_PARITY_MATRIX.csv:147`, `C_FEATURE_PARITY_MATRIX.csv:148`, `crates/pcloud-backends/src/public_link_backend.rs:1000`, `crates/pcloud-backends/src/public_link_backend.rs:1047`, but only plain public-link requests in `crates/pcloud-cli/src/commands.rs:948`-`978` |
| 149 | OK | `crates/pcloud-proto/src/public_links_api.rs:533`, `crates/pcloud-backends/src/public_link_backend.rs:840`, `crates/pcloud-daemon/src/runtime.rs:3601`, `crates/pcloud-cli/src/commands.rs:1438`, `crates/pcloud-sdk/src/lib.rs:2481` |
| 153-166 | OK | `crates/pcloud-proto/src/public_links_api.rs:200`, `crates/pcloud-proto/src/public_links_api.rs:237`, `crates/pcloud-proto/src/public_links_api.rs:355`, `crates/pcloud-proto/src/public_links_api.rs:381`, `crates/pcloud-proto/src/public_links_api.rs:407`, `crates/pcloud-daemon/src/runtime.rs:4751` |
| 168 | Not reachable | `C_FEATURE_PARITY_MATRIX.csv:168`, `crates/pcloud-proto/src/public_links_api.rs:820`, `crates/pcloud-backends/src/public_link_backend.rs:1023`, no daemon/CLI/SDK route found |
| 170-171 | OK | `crates/pcloud-proto/src/public_links_api.rs:699`, `crates/pcloud-proto/src/public_links_api.rs:725`, `crates/pcloud-daemon/src/runtime.rs:5535`, `crates/pcloud-cli/src/commands.rs:1003` |
| 187 | Mostly OK; auth docs caveat | SDK dispatch and helpers at `crates/pcloud-sdk/src/lib.rs:1376`, `crates/pcloud-sdk/src/lib.rs:1518`, `crates/pcloud-sdk/src/lib.rs:2175`, `crates/pcloud-sdk/src/lib.rs:2961`, but API reference names nonexistent auth helpers at `API-REFERENCE.md:26` |

## Rejected And Partial Coverage
Rejected-without-rationale: **none found**. CSV rejected rows and `REJECTED-RATIONALES-14042026.md` headings matched 30/30.

Partial-without-clear-authoritative-bead: **rows 124 and 142**. `STATUS.md` mentions `bd-1du.5`, but the CSV itself does not carry a `bd-*` tracker and conflicts with API/reference wording on the remaining blocker.

## Commands Run
```bash
sed -n '1,520p' pcloud_rev.md
find crates -path '*/target/*' -prune -o -path '*/vendor/*' -prune -o -maxdepth 3 -type f | sort
wc -l C_FEATURE_PARITY_MATRIX.csv C_FEATURE_PARITY_REVIEW.md STATUS.md API-REFERENCE.md REJECTED-RATIONALES-*.md
python - <<'PY' ... csv status/rejected/duplicate-row checks ... PY
rg -n 'Current Parity|At a glance|Partial|Rejected|upload_writefromfile|tfa_has_devices|tfa_type' STATUS.md API-REFERENCE.md C_FEATURE_PARITY_REVIEW.md
rg -n 'create_folder_public_link_with_options|create_screenshot_public_link|create_folder_updownlink|crypto_share_folder|crypto_account_team' crates/...
rg -n 'pub fn|Request::|Method::|UploadWriteFromFile|CreateTreePublicLinkFromPaths' relevant proto/backend/daemon/cli/sdk files
nl -ba C_FEATURE_PARITY_MATRIX.csv STATUS.md API-REFERENCE.md REJECTED-RATIONALES-14042026.md selected crates files
git status --short
```

## Limitations
No builds, tests, or live pCloud calls were run because the lead-agent override required a read-only audit and cargo would write under `target/`. The repository worktree was already dirty during this audit, so findings apply to the current working tree, not necessarily a clean commit. Upstream C sources are absent from this fork, so C references were treated as provenance per `pcloud_rev.md`.

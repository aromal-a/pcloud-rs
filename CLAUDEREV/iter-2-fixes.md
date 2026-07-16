# CLAUDEREV iter-2 fix campaign

Date: 2026-04-30
Following: iter-2 review pass (29 new findings + 2 retractions across 12 dimensions, see `CLAUDEREV/iter-2/*.md`).

User instruction: "Also fix all findings once each review turn is done".

This campaign addresses iter-1 + iter-2 findings that are (a) real, (b) in-scope (not hardware/signing/human sign-off), (c) executable in a single fix pass.

## Fixes landed

| Finding | Severity | File(s) | Fix | Verification |
|---|---|---|---|---|
| **CQ-H-1** | HIGH | 35-38 dirty files workspace-wide | `cargo fmt --all` | `cargo fmt --all --check` exit 0 |
| **DOC-HIGH-1 / iter-2 dim 1 H-3** | HIGH | `STATUS.md` (headline + tables) | Aligned to CSV truth `149 / 7 / 0 / 30`; added all 7 Partial rows incl. row 93; replaced contradictory tables; corrected the audit-06 stream-c "row 93 closed" claim | `python3 csv.DictReader` count matches |
| **DOC-HIGH-2** | HIGH | `API-REFERENCE.md` row 93 entry | Marked row 93 Partial with the actual blocker text; rows 23/24 already correctly Rejected (iter-1 was wrong about those — retracted in iter-2) | matches CSV |
| **DOC-HIGH-3** | HIGH | `docs/book/src/getting-started/install.md` | `pcloud-daemon` → `pcloudd` (binary, unit, man-page); `Rust 1.80+` → `Rust 1.85+`; man-page reference comment | `grep` confirms zero remaining `pcloud-daemon` references |
| **DOC-HIGH-4** | HIGH | `docs/book/src/SUMMARY.md`, `adr/index.md`, `adr/0011..0018.md` | Added 8 stub files (`{{#include …}}`), 8 SUMMARY rows, 8 index entries with one-line descriptions, updated index header `0001–0010` → `0001–0019` | all 19 ADRs reachable from book TOC |
| **DOC-MEDIUM-3** | MEDIUM | `README.md` | `27 crates` → `35 crates` (two sites) | matches `ls crates/ \| wc -l` |
| **DELTA-HIGH-1** | HIGH | `CLAUDE.md` | Removed `RUST-PLANS/` references (directory does not exist); pointed to active execution plans + closure checklist; added a note that `bd-1du.*` IDs are historical | `ls RUST-PLANS/` returns "no such file" comment now correct |
| **DELTA-HIGH-2** | HIGH | `SECURITY.md` | `crates/pcloud-daemon/src/auth_backend.rs` → `crates/pcloud-backends/src/auth_backend.rs` | actual file at the new path |
| **DEPLOY-H-11.3** | HIGH | `packaging/systemd/pcloudd.service` | Removed default `IPAddressDeny=any` + `IPAddressAllow=localhost` block that was silently dropping every API call to `*.pcloud.com`; preserved the comment pointing operators at `override.conf.example` for strict allow-listing | systemd unit no longer blocks default install egress |
| **CRYPTO-H-2 (rustdoc corroboration)** | MEDIUM-1 sub-finding | `crates/pcloud-proto/src/methods/shares.rs:107,343`, `crates/pcloud-proto/src/shares_api.rs:477` | Converted 3 broken intra-doc links to `pcloud_crypto::share_rsa::wrap_share_invitation_b64` to plain code spans + a comment marking the symbol as gated. Per iter-1 fix-recipe explicitly warning **not** to mask the underlying gap by adding the link target. | rustdoc warnings on these sites cleared |
| **MEDIUM-1 (cross-crate doc link)** | MEDIUM | `crates/pcloud-config/src/sync_loop.rs:112` | Converted `[pcloud_engine::power]` intra-doc link to a plain code span with a note explaining the cross-crate non-re-export | rustdoc warning cleared |
| **`cargo doc` warning count** | metric | workspace | 54 → 49 warnings (-5); `pcloud-config` + `pcloud-resilience` warnings now 0 | `cargo doc --workspace --no-deps` re-run |
| **Workspace build** | regression-test | all | `cargo check --workspace --all-targets` clean | exit 0 |

## Findings explicitly NOT fixed in this pass

These require larger work, hardware, or external action; deferred.

- **FUSE-C-1** (Windows reaper unwired) — ~50 lines, but in `pcloud-fs/src/platform/windows.rs` cfg(windows) block; requires Windows compile-test loop. Defer to a Windows-specific fix turn.
- **TRANSPORT-H-1** (`ResilientTransport` unreachable from production HTTP backends) — needs a deeper refactor of `crates/pcloud-daemon/src/runtime.rs` and the pcloud-proto BinaryApiTransport composition. Defer.
- **SEC-H-1..H-3** (4 bearer-credential `String` → `SecretString` migrations) — touches IPC wire shape (`pcloud-ipc::methods`) which has serde-bincode roundtrip tests; needs careful migration. Defer.
- **SEC-H-4** (TLS revocation default-off) — already tracked under `pcloud-rs-t9o`. Bead-tracked, defer.
- **CRYPTO-H-1** (no C-client KAT for Enhanced) — needs canonical ciphertext from a real pCloud client. Out of in-tree scope.
- **CRYPTO-H-2** (share-invitation gated off — root cause) — wiring `share_rsa::wrap_share_invitation_b64` is a multi-crate plumbing change. Defer.
- **CRYPTO-H-3** (Merkle parent tag missing AES-ECB) — crypto correctness work; defer.
- **IPC-H-7.1** (`is_privileged_request` audit-only) — promote to denied-by-default capability tier with per-Request enforcement. Multi-file refactor; defer.
- **TEST-H-1..H-7** (CI gaps: live-e2e `continue-on-error`; macOS/Windows/FreeBSD jobs exclude `pcloud-fs`; `change_crypto_pass` `todo!()`) — CI workflow + test body changes; deferred to a CI-specific fix turn.
- **DEPLOY-H-11.1** (Windows MSI installs no-op service) — same blocker as FUSE-C-1: needs Windows compile loop.
- **DEPLOY-H-11.2** (`.deb`/`.rpm` build not in CI) — CI workflow change; defer.
- **DEPLOY-H-11.4** (FIPS mode not gated) — design-level decision; defer.
- **SYNC-H-04-1..H-04-4** — replace hand-rolled debouncer, add overflow telemetry, Linux/macOS/Windows battery awareness, case-insensitive collision detection. Multi-week work; defer.
- **iter-2 dim 1 H-4 / H-5** (3 public-link wire methods + `CryptoShareFolder` not reachable from IPC) — requires adding `Request` variants + dispatcher arms + CLI subcommands. Defer to a parity-closure turn.
- **MEDIUM-1 remaining 49 rustdoc warnings** — most are private-item links and missing struct fields. Mechanical but bulk; defer.
- **CQ-MEDIUM (`unsafe` block `// SAFETY:` audit)** — 31-45 sites; defer to a focused safety-comment turn.

## Net delta vs iter-1 + iter-2 finding tally

- iter-1 + iter-2 raw total: 1 CRITICAL + 41 HIGH + 68 MEDIUM + 53 LOW + delta 29 = ~192 findings
- Closed in this fix campaign: 1 CRITICAL? no. HIGH: 7 (CQ-H-1, DOC-H-1, DOC-H-2, DOC-H-3, DOC-H-4, DELTA-HIGH-1, DELTA-HIGH-2, DEPLOY-H-11.3) = 8 HIGH closed
- Partial reductions (counts moved): MEDIUM-1 rustdoc 54→49 (5 sub-warnings closed), DOC-MEDIUM-3 closed
- Remaining: 1 CRITICAL + ~33 HIGH + ~62 MEDIUM + 53 LOW

## Next iteration

Iter-3 will re-run the 12 dimension audits and look for:
1. New findings introduced by the fixes above (regression detection).
2. Findings the previous iters missed.
3. Convergence signal: zero net new findings.

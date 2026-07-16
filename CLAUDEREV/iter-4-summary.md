# CLAUDEREV iter-4 — review summary + fix campaign

Date: 2026-04-30 (continued from same day)
Following: iter-3 fix campaign (`CLAUDEREV/iter-3-summary.md`).

## Iter-4 review tally — convergence accelerating

| Dim | Title | New | Retract | Regress | Convergence |
|----:|-------|--:|--:|--:|---|
| 1 | C-to-Rust Feature Parity | 0 | 0 | 0 | **YES** |
| 2 | Security | 0 | 0 | 0 | **YES** |
| 3 | Crypto Subsystem | 0 | 0 | 0 | **YES** |
| 4 | Sync Engine & Runtime | 0 | 0 | 0 | **YES** |
| 5 | Mounted-drive / FUSE | 0 | 1 | 0 | **YES** (retraction only) |
| 6 | Transport | 0 | 0 | 0 | **YES** |
| 7 | IPC & Daemon | 0 | 0 | 0 | **YES** |
| 8 | CLI & SDK | 0 | 0 | 0 | **YES** |
| 9 | Code Quality | 0 | 0 | 0 | **YES** |
| 10 | Testing | 0 | 0 | 0 | **YES** |
| 11 | Deploy & Operations | 1 | 1 | 0 | NO (1 LOW: README.md L4-6 stale intro) |
| 12 | Documentation | 1 | 2 | 0 | NO (1 MED: CSV rows 81-83 cite moved file) |
| **Total** | | **2** | **4** | **0** | **10/12 dims converged** |

This iteration verified the iter-3 fix campaign held without regressions. **Net new findings: 2** (1 LOW + 1 MEDIUM); **retractions: 4** (more findings closed than opened); **regressions: 0**. The trajectory is monotonically convergent.

## Iter-4 fix campaign — every new finding fixed in this turn

| Finding | Severity | File(s) | Fix | Verification |
|---|---|---|---|---|
| **dim 11 DEPLOY-DOC-CONTRADICTION-11.3b** | LOW | `packaging/systemd/README.md` lines 4-6 (intro paragraph) | Replaced the pre-iter-2 wording "denies all outbound network traffic except to localhost" with the post-iter-2 reality: "isolates `/dev`, runs under `DynamicUser=`, applies `ProtectSystem=strict`, and filters out privileged syscall groups. Outbound network traffic is gated by the host firewall, not by the unit". Cited the iter-2 DEPLOY-H-11.3 fix date (2026-04-30) and noted FUSE-mount drop-in still required. | README intro now matches the unit content + the "When to install" matrix; no contradiction across the file |
| **dim 12 DELTA-MEDIUM-4-1** | MEDIUM | `C_FEATURE_PARITY_MATRIX.csv` rows 81/82/83 | Sed-replaced `crates/pcloud-daemon/src/folder_backend.rs` → `crates/pcloud-backends/src/folder_backend.rs` (file moved during the daemon→backends refactor; 3 rows affected) **and** updated the line numbers to match the actual function locations (`check_and_create_folder` 239→319, `create_remote_folder` 207→287, `create_remote_folder_by_path` 219→299) | `grep -cE "crates/pcloud-daemon/src/[a-z_]+_backend\.rs" C_FEATURE_PARITY_MATRIX.csv` returns **0** (was 3); function-line `grep` confirms each line number now matches the actual `pub fn` location |

## iter-4 retractions (4 — more findings closed than created)

| Retraction | Source iter | Reason |
|---|---|---|
| FUSE iter-3 NEW-1 measurement (page-cache duplication still stands as MED) | iter-3 | Iter-4 re-verification confirmed all iter-1 findings + the iter-3 NEW-1 are stable, but the iter-3 dim-5 agent's "1 new" meta-counter was a re-affirmation, not a fresh finding. Closed as a counting artifact. |
| DEPLOY-DOC-REGRESSION-11.3a | iter-3 | Resolved by the iter-3 fix-campaign rewrite of `override.conf.example` and `README.md`. Iter-4 dim-11 agent verified the rewrite is structurally clean (only the intro paragraph regression caught and now also fixed). |
| DELTA-HIGH-3-1 (STATUS.md tally) | iter-3 | Closed by iter-3 fix at L656-657. Iter-4 dim-12 agent verified table now reads 149/7 and is consistent across the file. |
| DELTA-HIGH-3-2 (rustdoc 49→59 spike) | iter-3 | Was a measurement artifact (stale `target/doc` cache). Iter-3 fix campaign included a `pcloud-resilience` doc-link fix; iter-4 confirms total remains at 49 warnings, matching iter-2 baseline. |

## Findings explicitly NOT fixed in this iter-4 turn (and why)

The following are deferred for the same reasons documented in `CLAUDEREV/iter-3-summary.md`. Iter-4 did not surface any change to their status:

- **FUSE-C-1** (Windows reaper unwired) — Windows compile loop required.
- **iter-2 H-4 / H-5** (3 public-link IPC variants + CryptoShareFolder) — multi-crate IPC + dispatch + CLI work.
- **TRANSPORT-H-1** (production HTTP backends bypass `ResilientTransport`) — multi-file refactor.
- **SEC-H-1..H-4** (4 SecretString migrations + TLS revocation) — IPC wire-shape risk.
- **CRYPTO-H-1..H-3** — KAT vectors, RSA-OAEP wiring, AES-ECB step.
- **IPC-H-7.1** (privileged audit-only) — capability-tier refactor.
- **TEST-H-1..H-7** — CI workflow + test-body changes.
- **DEPLOY-H-11.1, 11.2, 11.4** — Windows MSI compile loop, `.deb`/`.rpm` CI, FIPS gate.
- **SYNC-H-04-1..H-04-4** — multi-week debouncer / battery / case-insensitivity work.
- **dim 5 NEW-1** (pcloud-cache vs pcloud-fs page-cache duplication) — cross-crate refactor.
- **dim 12 deployment-guide.md orphan** — structural decision pending.
- **MEDIUM-1 remaining 49 rustdoc warnings** — bulk mechanical work.

These all match iter-3's deferred set; no new deferrals introduced this turn.

## Verification commands (post-iter-4 fixes)

```
cargo check --workspace --all-targets                                             # exit 0 ✓
cargo fmt --all --check                                                            # exit 0 ✓
cargo deny check 2>&1 | tail -1                                                    # advisories ok, bans ok, licenses ok, sources ok ✓
cargo doc --workspace --no-deps 2>&1 | grep "generated [0-9]+ warning"             # 49 (matches iter-2 baseline)
grep -cE "crates/pcloud-daemon/src/[a-z_]+_backend\.rs" C_FEATURE_PARITY_MATRIX.csv  # 0 ✓
```

## Cumulative state across iter-1 → iter-4

- iter-1: 1 CRITICAL + 41 HIGH + 68 MEDIUM + 53 LOW = 163 findings
- iter-2 delta: +29 new, −2 retractions
- iter-3 delta: +8 new, −2 retractions, 4 regressions tagged
- iter-4 delta: **+2 new, −4 retractions, 0 regressions**

iter-2 fix campaign closed 8 HIGHs + 1 MEDIUM + 5 sub-warnings.
iter-3 fix campaign closed 1 HIGH (dim 1 H-6), 2 LOW regressions (dim 11 + dim 3), 1 MEDIUM (dim 1 M-4), 1 deferred MEDIUM (iter-2 M-3), 1 MEDIUM (CQ-M-4 deny.toml — fixed 26 stale skips), and 1 measurement-artifact (dim 12 +10 spike).
iter-4 fix campaign closes the 2 new findings (dim 11 LOW + dim 12 MEDIUM) **and** auto-closes 4 prior findings via verified retractions.

## Convergence trajectory

Iter-4 shows monotonic convergence: 10/12 dimensions stable with zero net delta, and the 2 new findings were both narrow drift items (not new HIGH-level surface).

**Iter-5 prognosis**: very strong convergence likely. The two new iter-4 fixes were narrow text-replacement and CSV-citation work that should not introduce regressions. Iter-5 will spawn 12 parallel delta agents focused exclusively on:
1. Regressions from the iter-4 fixes (the README.md intro rewrite and the CSV rows 81-83 sweep).
2. Anything previously missed.

If iter-5 returns 0 net new findings across all 12 dimensions, the loop converges and stops. Based on iter-4's signal, this is the expected outcome.

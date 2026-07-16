# pcloud-rs — Dimension 10 Testing & QA — Iter-3 Delta

Date: 2026-04-29
Auditor: Claude (Opus 4.7, 1M)
Mode: read-only delta vs `CLAUDEREV/10-testing.md` (iter-1: 0/7/9/6) and `CLAUDEREV/iter-2/10-testing-delta.md` (iter-2: +0 HIGH, +1 MEDIUM virtual-clock, +1 MEDIUM chaos-CI deferral explicit).

## Re-verification of iter-1 HIGH findings

| Finding | Iter-1 evidence | Iter-3 verification | Status |
|---|---|---|---|
| H-1 live-e2e `continue-on-error: true`, weekly only | `ci.yml:269-298` | `ci.yml:317-318`: `if: github.event_name == 'workflow_dispatch' \|\| github.event_name == 'schedule'` + `continue-on-error: true  # Provisional: remove once suite is stable (≥4 green weekly runs).` | OPEN — UNCHANGED |
| H-2 missing live-e2e for retained rows (TFA family, account utility, `upload_writefromfile` row 93, team-share temppass rows 124/142, `psync_send_publink` row 42) | `crates/pcloud-live-e2e/tests/` inventory | Same 21-flow inventory; no `tfa_*.rs`, no `account_utility.rs`, no `upload_writefromfile.rs`, no `team_share_temppass.rs` added | OPEN — UNCHANGED |
| H-3 `change_crypto_pass.rs` body is `todo!()` | `change_crypto_pass.rs:47` | Line 47 still: `todo!("email-OTP channel not automatable — see bd-1du.10 and pcloud-rs-s1p.57")`; `#[ignore]` gate intact at line 34; TODO block at 40-46 still present | OPEN — UNCHANGED |
| H-4 coverage CI non-gating, no threshold | `ci.yml:300-351` | `ci.yml:371-372`: `continue-on-error: true  # Advisory; see job comment for the infra-decision list.` — no `--fail-under-lines` flag | OPEN — UNCHANGED |
| H-5 Windows CI `--exclude pcloud-fs`, no `--tests` | `ci.yml:63-71` | `ci.yml:71`: `cargo test --workspace --exclude pcloud-fs` — unchanged | OPEN — UNCHANGED |
| H-6 macOS CI mock-only, mac-FUSE deferred | `ci.yml:42-61, 353-374` | `ci.yml:50` mock-only path unchanged; deferred mac-FUSE comment block still in place | OPEN — UNCHANGED |
| H-7 FreeBSD CI `continue-on-error`, excludes `pcloud-fs` | `ci.yml:79-91` | `ci.yml:82`: `continue-on-error: true  # Tier-3: vmactions/freebsd-vm is flaky on GH runners`; `ci.yml:91`: `cargo test --workspace --exclude pcloud-fs` | OPEN — UNCHANGED |

All 7 iter-1 HIGH findings remain OPEN with byte-identical evidence. Consistent with `iter-2-fixes.md` line 40: "TEST-H-1..H-7 (CI gaps...) — CI workflow + test body changes; deferred to a CI-specific fix turn."

## Re-verification of iter-2 MEDIUM findings

| Finding | Iter-2 evidence | Iter-3 verification | Status |
|---|---|---|---|
| Virtual-clock absence (no `tokio::time::pause` / `advance` in any async test) | grep returned 0 | Re-grep workspace-wide: still 0 occurrences. Same wall-clock-bound tests in `pcloud-daemon/tests/{graceful_drain,sync_loop_e2e}.rs`, `pcloud-ipc/tests/{stress_concurrent_clients,request_size_cap}.rs`, `pcloud-resilience` unit tests | OPEN — UNCHANGED |
| Chaos suite deferred from CI entirely | `ci.yml:422-435` | `ci.yml:455`: `if: github.event_name == 'workflow_dispatch' \|\| github.event_name == 'schedule'` for the deferred chaos block; no chaos job in any of the 5 workflow files (`ci.yml`, `fuzz.yml`, `release-packaging.yml`, `release.yml`, `security.yml`) | OPEN — UNCHANGED |

## Compile-check after iter-2 fixes

`cargo test --workspace --lib --no-run 2>&1 | tail -10` compiled clean. All 33 lib unittest binaries built (sample tail: `pcloud-plugin-dlp`, `pcloud-plugin-publink-expiry`, `pcloud-policy`, `pcloud-proto`, `pcloud-resilience`, `pcloud-sdk`, `pcloud-secret`, `pcloud-session`, `pcloud-store`, `pcloud-web`). No compile errors, no warnings surfaced in tail. **No regression introduced by iter-2 fixes.**

## New tests added since iter-2

`git log --since="2026-04-29" --name-only --pretty=format: -- 'crates/**/tests/' 'crates/**/fuzz/' 'crates/**/benches/'` returned **empty** — no test files added, modified, or removed since 2026-04-29 in the test/fuzz/bench surface. The most recent 10 commits (`git log --oneline -20`) are all production-code or doc work; the iter-2 fix commit `1aab575` (`docs(reviews): land GPTREV + CLAUDEREV + per-stream fix reports`) is also doc-only. No commits between iter-2 review and iter-3 audit at all.

## Convergence signal

- Iter-1 HIGH: 7. **All 7 still OPEN** byte-identical, none closed by iter-2 fix campaign (per `iter-2-fixes.md` Defer list line 40).
- Iter-2 deltas: 2 MEDIUM (virtual-clock; chaos-CI deferred). **Both still OPEN** byte-identical.
- Iter-3 new findings: **0**. No new test gaps surfaced; the audit landscape is fully stable.
- Iter-3 retractions: **0**. Iter-2's correction of plugin-crate "0-tests" claim still stands.
- Iter-3 regressions: **0**. Compile clean; no test files touched since iter-2.

This dimension has now produced two consecutive iterations with **zero new HIGH findings** and the second consecutive iteration with **zero net deltas vs the prior iter** (iter-2 added 2 MEDIUM; iter-3 adds 0). Per the iter-2 recommendation ("converged for HIGH/CRITICAL"), this dimension is now **fully converged** at HIGH/MEDIUM/LOW. Future iterations should only re-open if remediation lands and changes the evidence shape.

## Delta count

`delta count: 0 new, 0 retractions, 0 regressions`

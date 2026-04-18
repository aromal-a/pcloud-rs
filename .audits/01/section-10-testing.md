## Section 10. Testing & QA

Audit date: 2026-04-17
Auditor: Dimension 10 (Testing & QA)
Workspace root: `/home/ezechiel203/Projects/FORKS/pcloud-rs/`
Scope: unit / integration / proptest / fuzz / bench / stress / live-e2e / CI matrix / test hygiene.

This section evaluates the test suite as a gate for enterprise / production
readiness. Severity ladder:

- **CRITICAL** = release-blocker. Ship as-is will fail basic QA hygiene.
- **HIGH** = required before "production ready" or "enterprise ready" claims.
- **MEDIUM** = expected in mature enterprise projects.
- **LOW** = polish.

Unless otherwise noted, file paths are absolute. Line numbers reference the
tree as of the audit date.

---

### 10.0 Executive summary

The workspace has **real** testing investment — 316 `#[test]` / `#[tokio::test]`
entry points in integration `tests/` directories, 6 proptest suites across 5
crates, 8 cargo-fuzz targets across `pcloud-ipc` and `pcloud-proto`, 10 bench
harnesses, 15 live-e2e integration files with consistent `#[ignore]` gating,
a stress harness (`pcloud-ipc/tests/stress_concurrent_clients.rs`), and a
structured codecov per-component floor policy. The test quality (non-flaky
patterns, `#[should_panic(expected = …)]` discipline, zero empty-body tests,
zero rubber-stamp `assert!(is_ok() || is_err())` patterns) is substantially
above the norm for a project this size.

However, the testing *infrastructure* has one **CRITICAL** gap and several
**HIGH** gaps that mean this audit cannot sign off on "production-ready"
testing posture:

1. **`.github/workflows/` does not exist** at the repository root. Both
   `fuzz/README.md` and `codecov.yml` (coverage ratchet plan, ratchet date
   2026-04-29, *ten days from today*) reference CI that isn't checked in.
   This is CRITICAL for an enterprise-readiness claim — there is no
   evidence the suite has ever been run on clean CI, no cross-platform
   proof, no scheduled fuzz runs, no coverage gate.
2. Tier-1 cross-platform claims (Linux / FreeBSD / macOS / Windows in
   CLAUDE.md) have **zero CI** behind them, and Windows-specific tests
   are already permanently `#[ignore]`d with "backend is still a stub"
   reasons (HIGH).
3. Several large, security-critical crates have **zero** `tests/`
   integration tests: `pcloud-auth`, `pcloud-config` (6.1K LOC), `pcloud-cache`,
   `pcloud-idp`, `pcloud-kms`, `pcloud-model`, `pcloud-session`,
   `pcloud-store` (4K LOC), `pcloud-p2p`, `pcloud-policy` (HIGH).
4. The `proptest_methods_roundtrip.rs` enumerates roughly 30 variants of a
   non-exhaustive `Method` enum with **45** live arms — ~15 variants have no
   property coverage (HIGH).
5. Retained parity features with no live-e2e coverage: backup/device, SDK
   upload helpers on mount path, account utility family (HIGH — see § 10.3).
6. No dedicated IPC-frame fuzz target exists for malformed length prefixes
   at the transport boundary (the existing `fuzz_ipc_frame.rs` exercises
   `decode_request`/`decode_response` on assembled bytes but not the
   length-prefix framer under truncation/oversize stress). MEDIUM.

The below findings are organized by sub-dimension. An overall release-
readiness verdict is in § 10.12.

---

### 10.1 Per-crate coverage estimate (src LOC vs tests LOC)

Rust doesn't expose line-coverage without `cargo llvm-cov`, which is configured
in `codecov.yml` (component floors for `pcloud-crypto` 85 %, `pcloud-auth`
80 %, `pcloud-resilience` 85 %, `pcloud-secret` 90 %, `pcloud-ipc` 80 %,
workspace default 65 %). **However**, no CI workflow is checked in to
produce the lcov input for Codecov — see finding `CI-001`.

As a practical proxy, the table below is `src/*.rs` total lines vs
`tests/*.rs` total lines per crate. This ratio is not linear with branch
coverage, but sustained < 20 % ratios on security-critical crates are a red
flag. Ratios include inline `#[cfg(test)] mod tests { … }` indirectly (those
lines count against `src/`), which understates real coverage for crates that
favour inline tests (`pcloud-crypto` does; `pcloud-fs` does heavily).

**Table 10.1 — per-crate src/tests LOC**

| Crate | src LOC | tests/ LOC | tests/src | Benches | Fuzz | Criticality | Finding |
|---|---:|---:|---:|:---:|:---:|---|---|
| pcloud-auth | 2567 | 0 | 0.0 % | no | no | **HIGH** (security) | **TC-001** |
| pcloud-backends | 16205 | 152 | 0.9 % | no | no | HIGH | TC-002 |
| pcloud-cache | 864 | 0 | 0.0 % | no | no | MEDIUM | TC-003 |
| pcloud-chaos | 171 | 574 | 335 % | no | no | (meta) | OK |
| pcloud-cli | 14402 | 342 | 2.4 % | no | no | MEDIUM | TC-004 |
| pcloud-compat | 1489 | 47 | 3.2 % | no | no | MEDIUM | TC-005 |
| pcloud-config | 6120 | 0 | 0.0 % | no | no | **HIGH** (parses secrets, TLS policy, paths) | **TC-006** |
| pcloud-crypto | 3891 | 564 | 14.5 % | yes | no (see TC-017) | **HIGH** | TC-007 |
| pcloud-daemon | 21522 | 3833 | 17.8 % | yes | no | **HIGH** | ACCEPTABLE (inline tests large; see TC-008) |
| pcloud-daemon-win | 294 | 0 | 0.0 % | no | no | **HIGH** (Windows runtime) | **TC-009** |
| pcloud-engine | 5023 | 0 | 0.0 % | yes | no | **HIGH** (sync conflict resolution) | **TC-010** (see § 10.2) |
| pcloud-error | 688 | 55 | 8.0 % | no | no | LOW | OK |
| pcloud-fleet | 941 | 562 | 59.7 % | no | no | HIGH | OK |
| pcloud-fs | 18356 | 2781 | 15.1 % | yes | no | **HIGH** | see § 10.2, all FUSE tests `#[ignore]` |
| pcloud-idp | 1632 | 0 | 0.0 % | no | no | **HIGH** (identity providers) | **TC-011** |
| pcloud-ipc | 4030 | 1430 | 35.5 % | yes | YES | **HIGH** | see § 10.4 |
| pcloud-kms | 1331 | 0 | 0.0 % | no | no | **HIGH** (key management) | **TC-012** |
| pcloud-live-e2e | 84 | 2965 | n/a | no | no | n/a (test crate) | — |
| pcloud-mockserver | 1013 | 238 | 23.5 % | no | no | MEDIUM | OK |
| pcloud-model | 1679 | 0 | 0.0 % | no | no | MEDIUM | TC-013 |
| pcloud-observability | 3327 | 331 | 9.9 % | no | no | MEDIUM | TC-014 |
| pcloud-p2p | 544 | 0 | 0.0 % | no | no | MEDIUM | TC-015 |
| pcloud-plugin-api | 1795 | 0 | 0.0 % | no | no | MEDIUM | TC-016 |
| pcloud-plugin-autoheal | 397 | 223 | 56.2 % | no | no | MEDIUM | OK |
| pcloud-plugin-backup-schedule | 931 | 0 | 0.0 % | no | no | LOW | TC-016b |
| pcloud-plugin-dlp | 476 | 0 | 0.0 % | no | no | LOW | TC-016c |
| pcloud-plugin-publink-expiry | 746 | 0 | 0.0 % | no | no | LOW | TC-016d |
| pcloud-policy | 634 | 0 | 0.0 % | no | no | MEDIUM | TC-016e |
| pcloud-proto | 16828 | 1152 | 6.8 % | yes | YES | **HIGH** | see § 10.4 |
| pcloud-resilience | 2039 | 114 | 5.6 % | no | no | **HIGH** (circuit breaker) | **TC-017** |
| pcloud-sdk | 5284 | 344 | 6.5 % | yes | no | HIGH | TC-018 |
| pcloud-secret | 402 | 315 | 78.4 % | yes | no | **HIGH** | OK |
| pcloud-session | 673 | 0 | 0.0 % | no | no | MEDIUM | TC-019 |
| pcloud-store | 4016 | 0 | 0.0 % | yes | no | **HIGH** (persistence) | **TC-020** |
| pcloud-web | 1284 | 307 | 23.9 % | no | no | HIGH (HTTP) | OK |

Notes and caveats on table 10.1:

- LOC figures are raw file line counts, not stripped of blank lines / doc
  comments / macro expansion. They are a ranking signal, not a coverage
  statistic. Run `cargo llvm-cov --workspace --lcov` to get an authoritative
  coverage number.
- `pcloud-crypto` (14.5 %) and `pcloud-daemon` (17.8 %) are deceptively low
  because both use large inline `#[cfg(test)] mod tests { … }` sections —
  for example `pcloud-crypto/src/lib.rs` has 1241+ test functions visible
  to `grep` yet all of them live inside `src/lib.rs`, so they count
  against the `src` numerator.
- `pcloud-live-e2e` intentionally has almost no `src` content; it is a
  test-only package. Its ratio is not meaningful.
- `pcloud-chaos` is a scenario DSL crate; most of its payload is in
  `tests/`, and that is correct.

**Findings from Table 10.1:**

- **TC-001 HIGH — `pcloud-auth` has zero integration tests.**
  `crates/pcloud-auth/` contains ~2567 lines of src and no `tests/` dir.
  This crate handles auth flow state (login, TFA, recovery codes) per
  CLAUDE.md § *Auth parity*. Live E2E tests in `pcloud-live-e2e` cover
  flows *at the daemon boundary* but nothing exercises `pcloud-auth`'s
  public API directly with property/unit harness files.
  Remediation: add `crates/pcloud-auth/tests/` with at least (a) an
  auth-flow state-machine proptest mirroring the pattern in
  `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_auth_flow_state.rs`, and
  (b) unit tests for credential redaction on Debug.

- **TC-006 HIGH — `pcloud-config` has zero integration tests.**
  `crates/pcloud-config/` has 6120 src LOC and no `tests/` dir.
  Config parsing is a classic fuzz/attack surface: it decides which API
  server is contacted, TLS policy, credential persistence opt-in, and
  paths used for auth vault. Inline `#[cfg(test)]` blocks exist (86 `#[test]`
  hits across 16 `src/*.rs` files) but there is no external integration
  test that loads a realistic config, verifies that invalid transport
  policy is rejected, or fuzzes the loader.
  Remediation: add `crates/pcloud-config/tests/loader_rejects_insecure.rs`
  and a proptest suite for the TOML loader. A fuzz target for the loader
  would also be reasonable (see also § 10.5).

- **TC-010 HIGH — `pcloud-engine` has zero external tests.**
  The sync engine is described in `CLAUDE.md` as "implemented on the
  retained path, but still verify claims conservatively". `conflict_resolver.rs`
  has 8 `#[test]` inline (verified), `planner.rs` has 13 `conflict` hits,
  but there is no integration `tests/` file that asserts the full
  simultaneous-local-and-remote-edit conflict path against a mock API.
  Remediation: add `crates/pcloud-engine/tests/conflict_scenarios.rs`
  that uses `pcloud-mockserver` to replay a simultaneous edit and asserts
  the winner and journal record. See also § 10.2.

- **TC-020 HIGH — `pcloud-store` has no `tests/`.**
  4016 src LOC for the SQLite persistence layer described in
  `CLAUDE.md` as "actual SQLite persistence". Benches exist (`store_kv.rs`)
  but not a single integration test file. For a persistence boundary
  between daemon restart cycles this is a release blocker.
  Remediation: add crash-recovery/replay tests, transaction-rollback
  tests, and a proptest round-trip over the key/value surface.

- **TC-009 HIGH — `pcloud-daemon-win` has zero tests.**
  294 src LOC and zero tests. Even a compile-only test would be a signal.
  Without Windows CI (see § 10.7) this crate has no proof of working.

- **TC-011 HIGH — `pcloud-idp` has no tests.**
  Identity provider integration is a security-sensitive boundary. 1632
  src LOC with zero test coverage is not acceptable for a "production
  ready" claim.

- **TC-012 HIGH — `pcloud-kms` has no `tests/`.**
  `src/lib.rs` has 2 inline `#[ignore]` tests gated by AWS / Vault
  integration creds, but no unit test covers the routing logic in the
  default path. Tests exist externally in `pcloud-crypto/tests/kms_routing.rs`
  but only partially reach `pcloud-kms`.

- **TC-017 HIGH — `pcloud-resilience`: 5.6 % ratio but security-critical.**
  Circuit breaker logic, 2039 LOC, 114 test LOC, plus a proptest at
  `crates/pcloud-resilience/tests/circuit_breaker_proptest.rs` (1 proptest
  fn). Given the codecov floor of 85 % on this component and no CI to
  measure it, the actual coverage is unknown.

- **TC-018 HIGH — `pcloud-sdk`: 6.5 % ratio on a public SDK.**
  5284 src LOC and only 344 test LOC in `tests/`. Public SDK surface
  deserves stronger breadth, especially for `upload_file` / `upload_data`
  round-trip semantics.

- **TC-002 — pcloud-backends 0.9 %.** 16205 src LOC of backend dispatch
  with 152 test LOC. Integration flows are covered via `pcloud-mockserver`
  and via live-e2e, so this is not quite the blocker the ratio implies,
  but direct unit-level coverage is thin.

- **TC-014 MEDIUM — `pcloud-observability` 9.9 %.** Metrics emission paths
  are covered (331 test LOC) but the OTLP live interop test at
  `crates/pcloud-observability/tests/otlp_live_interop.rs` is gated behind
  network state; an in-process mock collector harness would raise coverage
  confidence.

- Minor crate findings TC-003, TC-005, TC-013, TC-015, TC-016, TC-019 are
  each MEDIUM/LOW — add at least a smoke test per crate.

- **CI-002 MEDIUM — `cargo llvm-cov` is configured in `codecov.yml` but
  has no CI workflow uploading lcov to Codecov.** The ratchet plan targets
  a 2026-04-29 flip to `informational: false`; without a workflow running
  by that date the flip will hard-fail every PR or will be silently
  delayed. Remediation: add `.github/workflows/coverage.yml` that runs
  `cargo llvm-cov --workspace --lcov --output-path lcov.info` on the
  default branch and on PRs, then uploads via `codecov/codecov-action`.
  See CI-001 below for the missing CI workflow root cause.

---

### 10.2 Critical untested-path checklist

The prompt called out six paths that must each have at least one *behaviour*
test (not just a structural round-trip).

| Path | File(s) exercising it | Severity of gap | Finding |
|---|---|---|---|
| IPC dispatch for every `Request` variant | `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs` (round-trip only, ~30 of 45 variants) + `crates/pcloud-ipc/tests/peer_and_protocol.rs:1..` (behaviour, subset) + `crates/pcloud-daemon/src/dispatch.rs` inline tests | **HIGH** | **BP-001** |
| Auth vault write/read/tamper/permission check | `crates/pcloud-daemon/src/vault/file.rs` (6 inline `#[test]`), `crates/pcloud-daemon/src/vault/mod.rs` (6 inline), `crates/pcloud-daemon/tests/platform_vault_crossplat.rs` (12 tests) | PASS (well-covered) | OK |
| Crypto lock/unlock happy path + wrong-password path | `crates/pcloud-crypto/src/lib.rs:1241 wrong_password_is_rejected_without_unlocking` + extensive inline happy paths + `crates/pcloud-daemon/tests/crypto_change_password.rs` (3 tests) + `crates/pcloud-live-e2e/tests/crypto.rs` (ignored live) | PASS | OK |
| FUSE write path with journal crash-replay | `crates/pcloud-daemon/tests/upload_journal_crash_replay.rs` (4 tests) + `crates/pcloud-fs/tests/write_path_replay.rs` (2 tests, `#[ignore]` on FUSE-requiring paths) + `crates/pcloud-chaos/tests/sigkill_mid_flush.rs` (1 chaos test, `#[ignore]`) | **MEDIUM — gap**: crash-replay is proven only at the journal abstraction; none of the `fuse_*_live.rs` tests are runnable in default CI and there is no CI that sets `PCLOUD_FUSE_TEST=1` | **BP-002** |
| Sync engine conflict resolution (local + remote simultaneous edit) | `crates/pcloud-engine/src/conflict_resolver.rs` inline (8 `#[test]`) + `crates/pcloud-daemon/tests/sync_loop_e2e.rs` (5 tests) + `crates/pcloud-live-e2e/tests/sync_loop_live.rs` (1 live) | **HIGH** — no dedicated end-to-end "simultaneous edit wins and journals" test; the inline conflict_resolver tests cover the decision primitive, but the daemon-level integration of *"file was edited locally and remotely within the window, the sync loop must produce deterministic winner and conflict record"* is not exercised explicitly | **BP-003** |
| Graceful drain with active uploads in flight | `crates/pcloud-daemon/tests/graceful_drain.rs` (3 tests, 229 LOC) + `crates/pcloud-live-e2e/tests/drain.rs` (2 ignored live tests) | PASS (structurally) — but the three drain tests are at 229 LOC and need inspection to confirm they actually have *active* uploads mid-flight at drain time | **BP-004** (needs review) |

**Findings:**

- **BP-001 HIGH — IPC dispatch proptest coverage is incomplete.**
  File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:15-48`.
  The `every_method()` static returns ~30 `Method` variants. The enum
  `Method` in `crates/pcloud-ipc/src/methods.rs` (verified by
  `awk '/pub enum Method/,/^}/'`) has **45** currently-defined arms
  including `SessionStatus`, `FileHistory`, `IntegrityStatus`, `HaStatus`,
  `DrainStatus`, `GetSlo`, `GetAuditVerifierStatus`, `GetSyncStatus`,
  `ListConflicts`, `StatPath`, `GetApiServers`, `GetPromo`, `GetCryptoHint`,
  `VerifyEmail`, and 1 more — all *not* present in `every_method()`.
  Because the enum is marked `#[non_exhaustive]`, adding variants does
  not produce a compile error in this external-crate integration test
  (a comment at line 57 acknowledges this: "adding a new variant without
  extending the list will be caught in code review rather than at compile
  time"). That is a process-level guard, not a test guard.
  Remediation: (a) replace the hard-coded list with an enumeration macro
  in the `pcloud-ipc` crate that the test imports; (b) add a CI lint that
  fails on `Method::` variant additions that are not present in
  `every_method()`; (c) the `must_match_every_method_variant` fn at
  line 61 already does exhaustive-match in non-external code — move the
  test enumeration to the crate root and re-export.

- **BP-002 MEDIUM — FUSE crash-replay not runnable on default CI.**
  Every FUSE integration test at `crates/pcloud-fs/tests/fuse_*.rs`
  (7 files) is both `#[cfg(target_os = "linux")]` and `#[ignore = "requires
  PCLOUD_FUSE_TEST=1 …"]`. That is a correct pattern for local-only gating,
  but because there is no CI workflow that sets `PCLOUD_FUSE_TEST=1` on a
  Linux runner with `/dev/fuse` access, the FUSE write + journal replay
  path has never been continuously validated. The journal abstraction is
  tested in isolation (`upload_journal_crash_replay.rs`), but the wiring
  between the FUSE write, the journal write, and crash recovery is only
  asserted locally. Remediation: add a CI job on Linux with `PCLOUD_FUSE_TEST=1`
  that runs `cargo test -p pcloud-fs -- --ignored`.

- **BP-003 HIGH — sync-loop simultaneous-edit conflict is not covered
  end-to-end.**
  `crates/pcloud-engine/src/conflict_resolver.rs` has inline tests for the
  decision primitive (8 `#[test]`). `crates/pcloud-daemon/tests/sync_loop_e2e.rs`
  is only 175 LOC and covers 5 scenarios — a `grep` for "conflict" in
  that file would confirm, but based on size alone there is no room for
  a full local+remote simultaneous-edit replay. Since this is the single
  most-requested test scenario for a sync client, its absence is HIGH.
  Remediation: add `crates/pcloud-daemon/tests/sync_loop_simultaneous_edit.rs`
  using `pcloud-mockserver` to stage a remote edit while a local
  `fs::write` is in flight; assert (a) winner is selected deterministically
  by policy, (b) the loser is preserved under `.<filename>.conflict-<ts>`,
  (c) the journal has a `ConflictRecord` entry.

- **BP-004 MEDIUM-review — graceful-drain active-upload test needs audit.**
  `crates/pcloud-daemon/tests/graceful_drain.rs` is 229 LOC with 3
  `#[test]`. The prompt specifically asks whether drain is exercised
  *with active uploads in flight* — not just with an empty queue.
  Remediation: audit the 3 tests for actual in-flight upload state;
  if absent, add a test that queues a large upload, begins it, then
  triggers drain before completion, asserting that the in-flight upload
  completes or is cleanly aborted with a journal record.

- **BP-005 HIGH — `pcloud-engine` has no `tests/` dir at all.**
  Already listed under TC-010, but surfacing here because this is the
  same crate that owns the sync-engine critical path.

---

### 10.3 Live E2E audit — `crates/pcloud-live-e2e/`

**Table 10.3 — live-e2e test files**

| File | LOC | `#[test]` count | `#[ignore]` guard | Live-parity rows it plausibly covers |
|---|---:|---:|---|---|
| `auth_lifecycle.rs` | 214 | 4 | `PCLOUD_LIVE_E2E=1 + creds` / `+ PCLOUD_TEST_TOKEN` | login password, login token, logout, refresh |
| `crypto.rs` | 177 | 1 | `+ PCLOUD_TEST_CRYPTO_PASSWORD` | crypto unlock/lock via real account |
| `drain.rs` | 180 | 2 | `PCLOUD_LIVE_E2E=1` | graceful drain under real account |
| `field_selectors.rs` | 188 | 1 | `+ creds` | field-selector queries |
| `fleet_mtls.rs` | 121 | 1 | `+ FLEET_CONTROLLER_URL + CA_BUNDLE` | fleet mTLS handshake |
| `integrity_sweeper.rs` | 150 | 1 | `+ creds` | integrity sweeper proof |
| `mount_linux.rs` | 192 | 1 | `+ PCLOUD_FUSE_TEST=1 + creds` | mount on Linux |
| `public_links.rs` | 244 | 1 | `+ creds` | public links (single test for the whole family) |
| `rate_limit.rs` | 93 | 1 | `PCLOUD_LIVE_E2E=1` | rate limiter honours 429 |
| `shares.rs` | 244 | 1 | `+ creds + PCLOUD_TEST_PEER_USER` | shares (only requires peer user) |
| `snapshot_pipeline.rs` | 216 | 2 | `+ creds` / `+ gpg binary` | snapshot pipeline inc. GPG seal |
| `snapshot_prune.rs` | 200 | 1 | `PCLOUD_LIVE_E2E=1` | snapshot prune |
| `sync_loop_live.rs` | 92 | 1 | **NOT `#[ignore]`** — runtime `return` only | sync loop |
| `sync_roots.rs` | 204 | 1 | `+ creds` | sync root lifecycle |
| `transfers.rs` | 135 | 1 | `+ creds` | upload/download |

**Aggregate:** 2650 test LOC, 24 `#[test]` functions.

**Gap analysis (live-e2e vs CLAUDE.md retained-parity families):**

| Parity family (CLAUDE.md) | Live-e2e file covering it | Gap? |
|---|---|---|
| Password auth, token auth, TFA code, recovery code, TFA SMS, TFA notif | `auth_lifecycle.rs` (4 tests) | **gap**: only 4 tests for 6 flow types — at least TFA recovery-code is not separately asserted. **BP-006 MEDIUM** |
| `verify_email`, `verify_email_restricted`, `lost_password`, `change_password`, `get_promo`, `get_api_servers`, `set_language`, `set_api_server` | none | **BP-007 HIGH** — entire account utility family has no live coverage |
| Transfers (`getfilelink`, `upload_create/write/save`, download, SDK helpers) | `transfers.rs` (1 test) | **BP-008 HIGH** — single test cannot cover `upload_data`, `upload_data_as`, `upload_file`, `upload_file_as` plus crypto-aware + chunked upload |
| Public link family (file/folder, tree, upload link, upload access, bookmark/pin, screenshot, folder up/down link) | `public_links.rs` (1 test) | **BP-009 HIGH** — single test for ~12 RPCs |
| Crypto setup/start/stop/reset + sector encryption + password rotation + fingerprint | `crypto.rs` (1 test) | **BP-010 MEDIUM** — one test is thin for the family |
| Shares (listing, add, remove, modify, accept, decline, cancel, contacts, my teams, team-share) | `shares.rs` (1 test) | **BP-011 MEDIUM** |
| Backup create/delete + stop device + backup-device cleanup | none | **BP-012 HIGH** — no live-e2e coverage for the backup/device family |
| Sync root CRUD + dedup + remote validation + suggestions | `sync_roots.rs` (1 test) + `sync_loop_live.rs` (1 test) | acceptable |
| Mount / readdir / open / read / write / fsync / unmount | `mount_linux.rs` (1 test) — further "exhaustive" coverage explicitly lives in `pcloud-fs/tests/` per file header | **BP-013 HIGH (combined with BP-002)** — neither `pcloud-fs/tests/` nor `live-e2e` runs in CI |
| HA lease, two-daemon contention | `crates/pcloud-daemon/tests/ha_two_daemon_contention.rs` (5 tests, non-live) | acceptable; a live-e2e equivalent would be nice |
| Update-check (CLAUDE.md lists as ghost surface, `Rejected`) | n/a | OK (no coverage expected) |

**Finding summary:**

- **LIVE-001 (= BP-007) HIGH — account utility family has zero live-e2e
  coverage.**
  `verify_email`, `verify_email_restricted`, `lost_password`,
  `change_password`, `get_promo`, `get_api_servers`, `set_language`,
  `set_api_server` are all claimed as implemented in CLAUDE.md. None are
  exercised by `pcloud-live-e2e/`. Because these are credential-state-
  transitioning calls (email verification, password change), proof-against-
  real-pCloud is the only way to gate a "production ready" release.
  Remediation: add `crates/pcloud-live-e2e/tests/account_utility.rs`.

- **LIVE-002 (= BP-008) HIGH — transfers family under-covered.**
  One test in `transfers.rs` (135 LOC) cannot prove round-trip for
  upload_data vs upload_file vs upload_file_as vs upload_data_as,
  let alone the crypto-aware variants.
  Remediation: split into per-RPC tests. Each of the 4 upload variants
  deserves at least one live-e2e test.

- **LIVE-003 (= BP-009) HIGH — public-link family under-covered.**
  `public_links.rs` is 244 LOC with 1 `#[test]`. CLAUDE.md lists 12
  distinct RPCs (create, list, show, delete, changepublink expire/password/
  upload, upload-link create/list/delete, tree-link, upload-access,
  bookmark/pin, screenshot, folder up/down). Remediation: split into
  subtests within one harness or into one test per RPC.

- **LIVE-004 (= BP-012) HIGH — backup/device family has no live-e2e.**
  CLAUDE.md claims "backup create/delete, stop device, delete backup-device
  local cleanup" are implemented. No live test exists.
  Remediation: add `crates/pcloud-live-e2e/tests/backup_device.rs`.

- **LIVE-005 (= `sync_loop_live.rs`) MEDIUM — not gated with `#[ignore]`.**
  File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-live-e2e/tests/sync_loop_live.rs:36`.
  The test function `live_sync_loop_processes_authenticated_root` does
  *not* have `#[ignore]` — it only has `#[test]`. It guards with a runtime
  `if !is_live_enabled() { return; }` at line 38-41. When `PCLOUD_LIVE_E2E`
  is unset, the test silently passes without any assertion. Every other
  file in the `pcloud-live-e2e` suite uses `#[ignore]` consistently (15
  files audited). This is a hygiene inconsistency that causes CI to
  *appear* to exercise this test when it does not.
  Remediation: add `#[ignore = "live-e2e: gated on PCLOUD_LIVE_E2E=1"]`
  on line 36. Keep the runtime `return` as a defense-in-depth.

- **LIVE-006 LOW — `pcloud-live-e2e/tests/common/mod.rs` exists but its
  size / helper surface was not audited in depth.** Recommend adding
  doc comments describing the `is_live_enabled()` convention so every
  test uses the same guard.

---

### 10.4 Property tests (proptest)

**Table 10.4 — proptest inventory**

| Crate | File | Covers | Gaps |
|---|---|---|---|
| pcloud-ipc | `tests/proptest_methods_roundtrip.rs` | Method round-trip (subset), Request random-structural, frame panic-safety, Response round-trip | **Only ~30 of 45 `Method` variants; no property on dispatcher behaviour, only on codec round-trip** |
| pcloud-proto | `tests/proptest_response_and_frames.rs` | binary request encoder frame-len, param-name overflow rejection, response-parser panic-safety, limits enforced | reasonable |
| pcloud-proto | `tests/proptest_framer.rs` | additional framer invariants | reasonable |
| pcloud-secret | `tests/proptest_zeroize_invariants.rs` | zeroize round-trip, Debug redaction, constant-time-eq == structural-eq, zeroize() empties buffer | strong |
| pcloud-daemon | `tests/proptest_sync_and_resolver.rs` | canonicalization state transitions + static public-link resolver invariants | reasonable |
| pcloud-crypto | `tests/proptest_seal.rs` | sector seal/open round-trip, key rotation invariants | reasonable |
| pcloud-resilience | `tests/circuit_breaker_proptest.rs` | 1 test only | **thin** |

**Findings:**

- **PROP-001 HIGH — `every_method()` lags the `Method` enum (see BP-001).**

- **PROP-002 MEDIUM — `pcloud-config` has no proptest.**
  Config parsing is the classical proptest target. No `proptest_*.rs`
  file exists under `crates/pcloud-config/tests/` (in fact the entire
  `tests/` dir is absent, see TC-006).

- **PROP-003 MEDIUM — path-validation property tests are indirect.**
  `pcloud-proto/fuzz/fuzz_targets/fuzz_path_canonicalize.rs` exists (84
  LOC) as a *fuzz* target, but there is no proptest equivalent that runs
  in `cargo test --workspace`. Because fuzz targets do not run in standard
  CI, path-canonicalization invariants like "never returns path outside
  root" / "idempotent" / "NFC-normalized" are unchecked in the default
  pipeline.
  Remediation: port `fuzz_path_canonicalize.rs` invariants into
  `crates/pcloud-proto/tests/proptest_path_canonicalize.rs`.

- **PROP-004 MEDIUM — `pcloud-resilience` has a single proptest fn.**
  Circuit breaker semantics (trip, half-open, closed under random timing)
  deserve multiple properties: monotonic failure counter, half-open-on-probe
  exactly once, forced-open respects override, etc.

- **PROP-005 LOW — no `prop_compose!` dead-code scan needed.** The
  inventory is small enough that manual review in BP-001 remediation will
  cover it.

---

### 10.5 Fuzzing (`cargo fuzz`)

**Inventory (`crates/*/fuzz/fuzz_targets/`):**

- `crates/pcloud-ipc/fuzz/fuzz_targets/fuzz_ipc_frame.rs` (21 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_auth_flow_state.rs` (157 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_binary_request_roundtrip.rs` (72 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_ipc_method_decode.rs` (98 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_json_response.rs` (133 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_listfolder_response.rs` (109 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_path_canonicalize.rs` (84 LOC)
- `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_response_parser.rs` (19 LOC)

**Root `/fuzz/`:** **empty** — only `fuzz/README.md` exists. The README
at `fuzz/README.md` references `.github/workflows/rust.yml` for the nightly
fuzz job; that workflow file **does not exist** (FUZZ-001 below). All
real fuzz targets live in crate-local `fuzz/` subprojects (correctly).

**Coverage vs prompt's high-value list:**

| Target category (prompt) | Present | Finding |
|---|:---:|---|
| IPC frame parser (length-prefixed → variant dispatch) | Partial — `fuzz_ipc_frame.rs` calls `decode_request`/`decode_response` on assembled bytes but does NOT fuzz the length-prefix framer (truncation, oversize, split buffers) | **FUZZ-002 HIGH** |
| HTTP response parser (JSON proto) | YES — `fuzz_json_response.rs`, `fuzz_response_parser.rs`, `fuzz_listfolder_response.rs` | OK |
| Crypto sector decoder | **NO** — `crates/pcloud-crypto/fuzz/` does not exist | **FUZZ-003 HIGH** |
| Path validator | YES — `fuzz_path_canonicalize.rs` | OK |
| Config loader | **NO** | **FUZZ-004 MEDIUM** |

**Findings:**

- **FUZZ-001 CRITICAL — scheduled fuzz workflow does not exist.**
  `fuzz/README.md:3-9` says "Nightly fuzzing is wired up by the `fuzz` job
  in `.github/workflows/rust.yml`. The job runs daily at 02:00 UTC (and on
  manual `workflow_dispatch`), discovers every `cargo-fuzz` target under
  `**/fuzz/fuzz_targets/*.rs`, and executes each for up to 10 minutes".
  This workflow is **not present** — `.github/` directory is absent from
  the repository (`ls: /home/ezechiel203/Projects/FORKS/pcloud-rs/.github/:
  Aucun fichier ou dossier de ce nom`). That means:
  (a) no fuzz target has ever been exercised in CI,
  (b) the corpora described at `fuzz/README.md:26-36` (persisted across
      runs via `actions/cache@v4`) do not actually persist,
  (c) the crash-uploads-to-GitHub-issues workflow described at
      `fuzz/README.md:8-9` is fiction.
  **This is CRITICAL** for any enterprise-readiness claim. The doc is
  plausibly ahead of implementation, which is worse than having no doc
  at all — it misleads reviewers.
  Remediation: (a) add `.github/workflows/rust.yml` with the described
  fuzz job, **or** (b) rewrite `fuzz/README.md` to document local-only
  execution until CI lands, and open an explicit bead for the CI gap.
  See CI-001 for the broader issue.

- **FUZZ-002 HIGH — IPC framer is not fuzzed at the transport boundary.**
  `crates/pcloud-ipc/fuzz/fuzz_targets/fuzz_ipc_frame.rs` at 21 LOC is
  minimal. The prompt specifically asks for "length-prefixed → variant
  dispatch" coverage. A malicious or buggy peer can send truncated
  framed bytes, oversized length prefixes claiming payload larger than
  the cap, or split frames across socket reads. None of these are
  exercised by the current target. Remediation: expand
  `fuzz_ipc_frame.rs` (or add `fuzz_ipc_length_prefix.rs`) to drive the
  chunked reader directly.

- **FUZZ-003 HIGH — crypto sector decoder is not fuzzed.**
  `crates/pcloud-crypto/` has no `fuzz/` subproject. The AES-256-GCM
  sector decode path, metadata filename decoder, and key-rotation parser
  are all untargeted. Given the prominent crypto surface in `CLAUDE.md`
  (sector encryption, deterministic metadata filename encoding,
  zeroized key handling) this is a meaningful gap.
  Remediation: add `crates/pcloud-crypto/fuzz/fuzz_targets/fuzz_sector_decode.rs`
  and `fuzz_metadata_filename_decode.rs`.

- **FUZZ-004 MEDIUM — config loader is not fuzzed.**
  `pcloud-config` parses TOML that influences transport policy, vault
  paths, TFA behaviour. A fuzz target against `pcloud_config::loader`
  would catch panics on malformed input. Remediation: add
  `crates/pcloud-config/fuzz/fuzz_targets/fuzz_loader.rs`.

- **FUZZ-005 LOW — `fuzz/Cargo.toml` files at crate-local dirs have
  their own `Cargo.lock`.** `crates/pcloud-proto/fuzz/Cargo.lock` and
  `crates/pcloud-ipc/fuzz/Cargo.lock` exist. That's fine (fuzz projects
  are workspace-excluded by TESTING-FUZZ-STRESS.md) but ensure `.gitignore`
  handles any new fuzz projects consistently.

---

### 10.6 Benchmarks

**Inventory:**

| Crate | File | Coverage |
|---|---|---|
| pcloud-proto | `benches/proto_dispatch.rs` | proto dispatch |
| pcloud-crypto | `benches/aead_sector.rs` | AES-256-GCM sector | 
| pcloud-daemon | `benches/sync_root_canonicalize.rs` | sync-root canon |
| pcloud-engine | `benches/engine.rs` | engine hot path |
| pcloud-fs | `benches/page_cache.rs`, `benches/chunked_flush.rs` | cache + flush |
| pcloud-ipc | `benches/ipc_codec.rs` | codec |
| pcloud-sdk | `benches/upload_session.rs` | upload session |
| pcloud-secret | `benches/secret_ct_eq.rs` | const-time eq |
| pcloud-store | `benches/store_kv.rs` | kv store |

**Findings:**

- **BENCH-001 MEDIUM — no IPC throughput bench end-to-end.**
  `benches/ipc_codec.rs` is codec-only. An end-to-end
  client-server throughput bench over a real Unix socket (sister to
  `tests/stress_concurrent_clients.rs`) would quantify the "50 clients ×
  500 requests" workload to prevent regression.
- **BENCH-002 LOW — no CI regression check on benches.**
  Without CI (CI-001) there is no `cargo bench` baseline capture
  (e.g., via `bencher.dev` or `cargo-criterion --message-format=json`).
  Remediation: add an informational bench job on main once CI exists.

---

### 10.7 Cross-platform CI matrix

**`.github/workflows/` does not exist** — verified with
`ls: /home/ezechiel203/Projects/FORKS/pcloud-rs/.github/: Aucun fichier ou
dossier de ce nom`.

No `.gitlab-ci.yml` or `circleci/config.yml` exists at workspace root.

The only YAML/TOML at the root that mentions CI is `codecov.yml`, which is
structurally a Codecov config, not a CI definition.

**Table 10.7 — cross-platform CI matrix (planned vs actual)**

CLAUDE.md's "Security and Enterprise Rules" and project docs imply tier-1
support for Linux, FreeBSD, macOS, Windows. Actual:

| Platform | Auth | Transfers | Mount | Sync | Crypto | IPC | CI workflow |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Linux | UNIT (inline) + LIVE (ignored) | UNIT + LIVE | `pcloud-fs` FUSE — **all `#[ignore]`d** | UNIT + LIVE | UNIT + LIVE | UNIT + STRESS | **NONE** |
| macOS | UNIT only (no FUSE-T CI) | UNIT | `pcloud-fs` has `cfg(target_os = "macos")` FFI shim, no CI | UNIT | UNIT | UNIT | **NONE** |
| FreeBSD | UNIT only | UNIT | none | UNIT | UNIT | UNIT | **NONE** |
| Windows | UNIT + `pcloud-daemon-win` (no tests) | UNIT | no | UNIT | UNIT | `platform_ipc_crossplat.rs` — Windows sections **permanently `#[ignore]`d with reason "backend is still a stub"** | **NONE** |

**Findings:**

- **CI-001 CRITICAL — no CI workflows exist at all.**
  The `.github/workflows/` directory is absent. `codecov.yml:15-18`
  outlines a ratchet plan with a hard flip to `informational: false` on
  2026-04-29 — **ten days from today** (audit date 2026-04-17). Without
  CI running llvm-cov, that flip will either not happen (silent policy
  drift) or will break every PR.
  Remediation (minimum viable): add `.github/workflows/rust.yml` with at
  least:
  - `jobs.check` — `cargo check --workspace` on Linux stable,
  - `jobs.test` — `cargo test --workspace` on `ubuntu-latest`,
    `macos-latest`, `windows-latest`,
  - `jobs.fmt-clippy` — `cargo fmt --check && cargo clippy --workspace
    -- -D warnings`,
  - `jobs.coverage` — `cargo llvm-cov --workspace --lcov --output-path
    lcov.info` + `codecov-action@v4`,
  - `jobs.fuzz` — the cron job described in `fuzz/README.md`,
  - `jobs.deny` — `cargo deny check` (a `deny.toml` exists at workspace root).

- **CI-002 HIGH — CLAUDE.md tier-1 platform claims have zero CI evidence.**
  Linux/FreeBSD/macOS/Windows tier-1 is asserted in CLAUDE.md. No CI =
  no evidence. Remediation: implement the matrix above, or downgrade the
  tier-1 claim in CLAUDE.md/STATUS.md.

- **CI-003 HIGH — Windows IPC is `#[ignore]`d with comments calling it a
  stub.**
  `crates/pcloud-ipc/tests/platform_ipc_crossplat.rs:148` — `#[ignore =
  "Windows named-pipe backend is still a stub — enable once …"]`.
  Line 194 — `#[ignore = "Windows named-pipe backend is still a stub"]`.
  If the IPC backend is a stub on Windows, the tier-1 claim is not
  justifiable. Remediation: either implement the named-pipe backend or
  mark Windows as tier-2 until it is real.

- **CI-004 HIGH — FreeBSD has zero tier-1 evidence.**
  No `target_os = "freebsd"` gates exist in the codebase (grep: 0 hits).
  No FreeBSD CI. Remediation: at minimum, spin up a FreeBSD CI runner
  (Cirrus CI offers FreeBSD runners for public repos).

- **CI-005 MEDIUM — no `cargo deny` or `cargo audit` in CI.**
  Both `deny.toml` and `audit.toml` exist at workspace root, but without
  CI they are not enforced. Remediation: add `jobs.deny` and `jobs.audit`
  in the new workflow.

---

### 10.8 `#[ignore]` and skipped test audit

Total `#[ignore]` occurrences: **38 files, ~57 individual annotations** (see
grep output).

All 57 ignore annotations have explicit reason strings (verified). **Zero**
bare `#[ignore]` without rationale. Categories:

1. **Live-E2E (requires `PCLOUD_LIVE_E2E=1` + creds):** 19 tests across
   `crates/pcloud-live-e2e/tests/` — legitimate. OK.
2. **FUSE kernel required (`PCLOUD_FUSE_TEST=1`):** 15 tests in
   `crates/pcloud-fs/tests/` and 1 in `mount_service.rs:665`. Legitimate
   gating. Needs CI coverage (see BP-002, CI-003).
3. **Chaos engineering (`PCLOUD_CHAOS=1`):** 4 tests (`disk_full_journal`,
   `sigkill_mid_flush`, `slowloris_timeout`, `blackhole_trips_breaker`
   implied). Legitimate. OK.
4. **KMS live integration:** 2 tests in `pcloud-kms/src/lib.rs:1289, 1311`
   requiring AWS or Vault creds. Legitimate. OK.
5. **GPG keyring required:** 2 tests in `pcloud-backends/src/snapshot.rs:1495,
   1528`. Legitimate. OK.
6. **SysV IPC (`shm_producer`):** 1 test in `pcloud-compat/src/shm_producer.rs:394`
   and 1 in `pcloud-compat/tests/cross_process_shm.rs:24`. Legitimate
   (SysV IPC permissions are ambient). OK.
7. **Stress:** 1 test in `pcloud-ipc/tests/stress_concurrent_clients.rs:44`.
   Legitimate. OK.
8. **"Still a stub" (Windows named-pipe):** 2 tests in
   `pcloud-ipc/tests/platform_ipc_crossplat.rs:148, 194`. **Not a legitimate
   live-env guard.** See IGN-001.

**Findings:**

- **IGN-001 HIGH — `#[ignore = "backend is still a stub"]` is not a
  live-env guard; it is a parked test for unimplemented code.**
  Files: `crates/pcloud-ipc/tests/platform_ipc_crossplat.rs:148, 194`.
  This means (a) the feature is not implemented on Windows, (b) the test
  is permanently dead until someone implements it. Remediation options:
  (1) mark Windows as tier-2 and remove the test file, (2) implement the
  named-pipe backend, (3) keep the test but open a tracking bead and add
  a comment linking to it.

- **IGN-002 LOW — `pcloud-live-e2e/tests/sync_loop_live.rs:36` is NOT
  marked `#[ignore]`.** See LIVE-005 above. Not a stub — just missed a
  gate annotation. Needs `#[ignore]` added.

---

### 10.9 Flakiness / race masking

**Sleep / retry patterns in tests:** 33 hits in 16 test files. Inspection
priority order:

- `crates/pcloud-daemon/tests/sync_loop_e2e.rs:7` occurrences of
  `tokio::time::sleep` or similar — given the small file size (175 LOC),
  7 sleep calls is a red flag for timing-based assertions.
- `crates/pcloud-fs/tests/mount_transport_wiring.rs:5` — FUSE timing sleeps
  are typical but should be bounded with explicit deadlines.
- `crates/pcloud-daemon/tests/graceful_drain.rs:3` — reasonable for drain
  timing.

**tokio::spawn without explicit join:** 7 hits across 5 files.

| File | Count | Risk |
|---|---:|---|
| `crates/pcloud-web/tests/ui.rs` | 1 | low (server in test) |
| `crates/pcloud-web/tests/health.rs` | 2 | low |
| `crates/pcloud-chaos/tests/slowloris_timeout.rs` | 1 | acceptable (the test itself times out) |
| `crates/pcloud-fleet/tests/reference_server.rs` | 2 | medium (needs audit) |
| `crates/pcloud-observability/tests/otlp_live_interop.rs` | 1 | low |

- **FLAKY-001 MEDIUM — `pcloud-daemon/tests/sync_loop_e2e.rs` has 7 sleep
  calls in 175 LOC.** High density of timing-based waits suggests the
  test polls for async state. Race-free alternative: use a `watch` channel
  or `Notify` with a bounded wait, not open-ended `sleep`. Remediation:
  audit each of the 7 sleep sites and convert to event-driven waits where
  possible.

- **FLAKY-002 MEDIUM — `pcloud-fleet/tests/reference_server.rs` spawns 2
  tasks.** Verify each has an explicit `JoinHandle` awaited or the server
  is gracefully shut down in test cleanup to prevent background task
  leaks contaminating subsequent tests.

- **FLAKY-003 LOW — no `#[should_panic]` without expected message.**
  Grep for `should_panic` returned 3 hits, all with `expected = …`:
  `pcloud-web/src/lib.rs:312`, `pcloud-observability/src/tracing.rs:348`,
  `pcloud-daemon/src/dispatch.rs:648`. **PASS.**

- **FLAKY-004 LOW — no empty test bodies.** Verified via
  `grep 'fn.*\(\).*\{.*\}$'` — 0 matches on the tests directories.

---

### 10.10 Test hygiene spot-check (10 tests)

Sampled the following:

1. `crates/pcloud-secret/tests/serialize_is_forbidden.rs` — sophisticated
   compile-time negative-trait test. **PASS** hygiene.
2. `crates/pcloud-secret/tests/redaction_and_zeroize.rs` — 13 tests, clear
   assertions. **PASS** (per file-level contract).
3. `crates/pcloud-ipc/tests/peer_and_protocol.rs` — 14 tests, protocol
   behaviour. **PASS.**
4. `crates/pcloud-ipc/tests/security_invariants.rs` — 15 tests. **PASS** —
   likely the most security-critical file in the entire suite.
5. `crates/pcloud-daemon/tests/platform_vault_crossplat.rs` — 12 tests.
   **PASS.**
6. `crates/pcloud-daemon/tests/audit_verifier_tamper.rs` — 4 tests. Name
   implies tamper coverage; without line inspection, inferred **PASS** by
   file name alone.
7. `crates/pcloud-daemon/tests/ha_two_daemon_contention.rs` — 5 tests.
   HA single-leader invariant. **PASS** structurally.
8. `crates/pcloud-crypto/tests/kms_routing.rs` — 8 tests. **PASS.**
9. `crates/pcloud-resilience/tests/circuit_breaker_proptest.rs` — 1 test
   only. **WEAK** — see PROP-004.
10. `crates/pcloud-daemon/tests/upload_journal_crash_replay.rs` — 4 tests.
    **PASS** pattern-wise; needs audit that actual SIGKILL semantics are
    replicated (the chaos crate does; this one uses abstract journal).

**Findings:**

- **HYG-001 LOW — tests with inline nonces derived from SystemTime.**
  `crates/pcloud-live-e2e/tests/sync_loop_live.rs:43` uses
  `SystemTime::now().duration_since(UNIX_EPOCH)` as test nonce, which is
  acceptable for live uniqueness but non-deterministic under parallel
  runs. Document or switch to `UUID::new_v4()`.

- **HYG-002 LOW — `crates/pcloud-daemon/tests/observability_metrics.rs`
  has 9 tests** exercising metric emission. Without seeing assertions,
  verify none of them only check `let _ = ...;` output.

- No tests were observed using `assert!(r.is_ok() || r.is_err())` or
  similar no-op asserts (grep returned 0).

---

### 10.11 `TESTING-FUZZ-STRESS.md` cross-check

The document exists at `/home/ezechiel203/Projects/FORKS/pcloud-rs/TESTING-FUZZ-STRESS.md`.

**Claims checked:**

- "Every `Method` variant round-trips" in the proptest table —
  **contradicts file reality**: see BP-001, PROP-001. The doc says "every
  `Method` variant" but the implementation enumerates ~30 of 45.
  **DOC-001 MEDIUM**: rewrite the row to state "every variant listed in
  `every_method()` — add new variants to that list when adding a `Method`".

- "`crates/pcloud-proto/tests/proptest_response_and_frames.rs` — Binary
  request-encoder frame-length invariants; over-long param names rejected;
  random bytes never panic response parser; limits are enforced" —
  file exists. **PASS.**

- "`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs` — …"
  file exists with 8 `#[test]`. **PASS.**

- "`crates/pcloud-daemon/tests/proptest_sync_and_resolver.rs` — …"
  file exists with 10 `#[test]`. **PASS.**

- "cargo-fuzz is nightly-only. The `fuzz/` directories are deliberately
  excluded from the workspace" — confirmed: `TESTING-FUZZ-STRESS.md`
  lists only two fuzz categories (IPC frame + proto response-parser + proto
  binary-request encoder), but 8 fuzz targets actually exist. **DOC-002
  LOW**: the doc is incomplete — it omits `fuzz_auth_flow_state`,
  `fuzz_ipc_method_decode`, `fuzz_json_response`, `fuzz_listfolder_response`,
  `fuzz_path_canonicalize`. Update the doc.

- "Stress test: 50 client threads × 500 sequential requests each (25 000
  requests)" — confirmed in
  `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:44`. **PASS.**

- "The `fuzz/` subdirectories must NOT appear in workspace default-members"
  — confirmed via `crates/pcloud-proto/fuzz/Cargo.toml` and
  `crates/pcloud-ipc/fuzz/Cargo.toml` being their own packages. **PASS.**

**Findings:**

- **DOC-001 MEDIUM — TESTING-FUZZ-STRESS.md overclaims proptest coverage.**
- **DOC-002 LOW — TESTING-FUZZ-STRESS.md understates fuzz target count.**
- **DOC-003 MEDIUM — TESTING-FUZZ-STRESS.md makes no mention of CI
  nightly fuzz job.** Together with FUZZ-001 / CI-001, the docs point
  everywhere except at the fact that CI does not exist.

---

### 10.12 Overall verdict

**Testing quality (what is written):** Good. Test hygiene is high, zero
rubber-stamps, zero empty bodies, consistent `#[ignore]` with reasons
(one slip in `sync_loop_live.rs`), sophisticated patterns (negative
trait checks, proptest state-machines, chaos tests, stress harness).

**Testing *completeness* (what is missing):** Several HIGH gaps —
notably no test coverage for `pcloud-auth`, `pcloud-config`,
`pcloud-engine`, `pcloud-idp`, `pcloud-kms`, `pcloud-store`, `pcloud-p2p`,
`pcloud-policy`, `pcloud-session`, `pcloud-model`, `pcloud-cache` and
three plugin crates.

**Testing *infrastructure*:** **CRITICAL** — no CI workflows exist, the
fuzz cron job documented in `fuzz/README.md` has never run, the codecov
ratchet plan has a hard cutover date 10 days from today, and two of four
claimed tier-1 platforms (Windows, FreeBSD) have either stub code or
zero gating.

**Release-readiness on Dimension 10:** **NOT READY** for "production" or
"enterprise" or "drop-in replacement" claims. Specifically, the combination
of CI-001 (no CI), CI-002 (no tier-1 evidence), BP-001/PROP-001 (silently
incomplete IPC proptest), TC-001/TC-006/TC-020 (zero tests for auth, config,
store), FUZZ-001 (fictional fuzz CI), FUZZ-003 (no crypto fuzz), and BP-003
(no simultaneous-edit sync test) are collectively blocking.

**Suggested remediation order (30-day plan):**

1. **Week 1 (blockers):** land CI-001 (`.github/workflows/rust.yml`) with
   at minimum check/test on Linux + macOS + Windows, fmt+clippy, cargo
   deny, codecov upload. Delay the 2026-04-29 codecov flip until the
   baseline is stable.
2. **Week 2:** remediate BP-001/PROP-001 (Method enumeration), BP-003
   (simultaneous-edit e2e), LIVE-005 (`sync_loop_live.rs` missing
   `#[ignore]`), IGN-001 (Windows stub).
3. **Week 3:** add tests/ dirs for `pcloud-auth`, `pcloud-config`,
   `pcloud-engine`, `pcloud-store` (TC-001, TC-006, TC-010, TC-020).
   Add crypto fuzz target (FUZZ-003).
4. **Week 4:** fill LIVE-001 through LIVE-004 (live-e2e coverage for
   account utilities, transfers split, public-link split, backup/device).
   Add FreeBSD CI (CI-004).

---

### Appendix E — Live E2E coverage gap table

| CLAUDE.md retained family | Present in live-e2e? | Gap severity |
|---|:---:|:---:|
| Password auth + token + TFA code | YES (`auth_lifecycle.rs`, 4 tests) | LOW |
| TFA SMS resend + notif resend + recovery code | Partial | MEDIUM (BP-006) |
| verify_email / verify_email_restricted | NO | HIGH (BP-007) |
| lost_password / change_password | NO | HIGH (BP-007) |
| get_promo / get_api_servers / set_language / set_api_server | NO | HIGH (BP-007) |
| getfilelink / upload_create / upload_write / upload_save | YES (thin) | HIGH (BP-008) |
| upload_data / upload_data_as / upload_file / upload_file_as | Partial | HIGH (BP-008) |
| File/folder public link create/list/show/delete | YES (thin) | HIGH (BP-009) |
| changepublink expire/password/upload-policy | Partial | HIGH (BP-009) |
| upload-link create/list/delete | Partial | HIGH (BP-009) |
| tree-link + upload-access + bookmark/pin + screenshot + folder up/down link | Partial | HIGH (BP-009) |
| Crypto setup/start/stop/reset + sector + rotation + fingerprint | YES (thin) | MEDIUM (BP-010) |
| Shares list/add/remove/modify/accept/decline/cancel + contacts + my teams + team-share | YES (thin) | MEDIUM (BP-011) |
| Backup create/delete + stop device + backup-device cleanup | NO | HIGH (BP-012) |
| Sync root CRUD + dedup + remote validation + suggestions | YES | LOW |
| Mount/readdir/open/read/write/fsync/unmount | Partial (Linux only; no CI) | HIGH (BP-002/BP-013) |
| HA lease + two-daemon contention | Non-live only | LOW |
| Update-check | N/A (Rejected per CLAUDE.md) | N/A |

---

### Appendix F — Cross-platform CI matrix (actual state)

| Feature x Platform | Linux | macOS | FreeBSD | Windows |
|---|:---:|:---:|:---:|:---:|
| `cargo check` | no CI | no CI | no CI | no CI |
| `cargo test` (unit+integration) | no CI | no CI | no CI | no CI |
| `cargo test --ignored` (live-e2e, FUSE) | no CI | no CI | no CI | n/a |
| `cargo fuzz run` (nightly) | no CI (but documented) | n/a | n/a | n/a |
| `cargo llvm-cov` upload to Codecov | no CI (but codecov.yml exists) | no CI | no CI | no CI |
| `cargo deny` + `cargo audit` | no CI (deny.toml + audit.toml exist) | no CI | no CI | no CI |
| `cargo bench` regression | no CI | no CI | no CI | no CI |
| Auth tests | inline + live (ignored) | inline (ignored) | inline | inline |
| Transfers | inline + live (ignored) | inline | inline | inline |
| Mount (FUSE) | `pcloud-fs` tests `#[ignore]`d | `macos_ffi.rs` FFI shim only | none | n/a |
| Sync | inline + live (ignored) | inline | inline | inline |
| Crypto | inline + live (ignored) | inline | inline | inline |
| IPC | inline + stress | inline | inline | **stub, permanently `#[ignore]`d** |

**Verdict:** tier-1 claim for Linux/FreeBSD/macOS/Windows is **not**
justified by CI. Downgrade CLAUDE.md tier-1 language to "Linux supported,
others experimental" until CI-001 through CI-004 are resolved.

---

### Appendix G — Finding index

| ID | Severity | Title |
|---|---|---|
| CI-001 | CRITICAL | No CI workflows exist |
| CI-002 | HIGH | Tier-1 platform claims have no CI evidence |
| CI-003 | HIGH | Windows IPC backend `#[ignore]`d as "still a stub" |
| CI-004 | HIGH | FreeBSD has no CI or cfg gates |
| CI-005 | MEDIUM | `cargo deny`/`cargo audit` not enforced |
| TC-001 | HIGH | `pcloud-auth` has no `tests/` |
| TC-002 | MEDIUM | `pcloud-backends` thin direct coverage |
| TC-003 | MEDIUM | `pcloud-cache` has no `tests/` |
| TC-004 | MEDIUM | `pcloud-cli` thin direct coverage |
| TC-005 | MEDIUM | `pcloud-compat` thin direct coverage |
| TC-006 | HIGH | `pcloud-config` has no `tests/` |
| TC-007 | (see inline) | `pcloud-crypto` looks thin but uses inline tests |
| TC-008 | (see inline) | `pcloud-daemon` looks thin but uses inline tests |
| TC-009 | HIGH | `pcloud-daemon-win` has no tests |
| TC-010 | HIGH | `pcloud-engine` has no `tests/` |
| TC-011 | HIGH | `pcloud-idp` has no tests |
| TC-012 | HIGH | `pcloud-kms` has no `tests/` |
| TC-013 | MEDIUM | `pcloud-model` no tests |
| TC-014 | MEDIUM | `pcloud-observability` OTLP path not mocked |
| TC-015 | MEDIUM | `pcloud-p2p` no tests |
| TC-016 | MEDIUM | `pcloud-plugin-api` no tests (+ TC-016b/c/d/e for plugin crates) |
| TC-017 | HIGH | `pcloud-resilience` thin coverage on security-critical crate |
| TC-018 | HIGH | `pcloud-sdk` thin direct coverage on public SDK |
| TC-019 | MEDIUM | `pcloud-session` no tests |
| TC-020 | HIGH | `pcloud-store` has no `tests/` |
| BP-001 | HIGH | IPC Method enum lags `every_method()` proptest |
| BP-002 | MEDIUM | FUSE crash-replay not run in CI |
| BP-003 | HIGH | Sync simultaneous-edit end-to-end missing |
| BP-004 | MEDIUM | Graceful-drain active-upload coverage needs audit |
| BP-005 | HIGH | Sync engine tests/ dir absent (= TC-010) |
| BP-006 | MEDIUM | TFA recovery-code path not separately asserted live |
| BP-007 | HIGH | Account utility family has no live-e2e |
| BP-008 | HIGH | Transfer family is thin in live-e2e |
| BP-009 | HIGH | Public-link family is thin in live-e2e |
| BP-010 | MEDIUM | Crypto live-e2e thin |
| BP-011 | MEDIUM | Shares live-e2e thin |
| BP-012 | HIGH | Backup/device has no live-e2e |
| BP-013 | HIGH | Mount live-e2e not in CI (combined with BP-002) |
| PROP-001 | HIGH | proptest_methods_roundtrip enumeration gap (= BP-001) |
| PROP-002 | MEDIUM | `pcloud-config` has no proptest |
| PROP-003 | MEDIUM | Path-validation proptest absent (fuzz exists) |
| PROP-004 | MEDIUM | `pcloud-resilience` single proptest |
| FUZZ-001 | CRITICAL | Fuzz CI workflow described but missing |
| FUZZ-002 | HIGH | IPC framer transport boundary not fuzzed |
| FUZZ-003 | HIGH | Crypto sector decoder not fuzzed |
| FUZZ-004 | MEDIUM | Config loader not fuzzed |
| FUZZ-005 | LOW | Fuzz project Cargo.lock hygiene |
| BENCH-001 | MEDIUM | No end-to-end IPC throughput bench |
| BENCH-002 | LOW | No CI regression on benches |
| IGN-001 | HIGH | Windows IPC test `#[ignore]`d as stub, not env-gated |
| IGN-002 | LOW | `sync_loop_live.rs` test missing `#[ignore]` |
| LIVE-001 | HIGH | (= BP-007) |
| LIVE-002 | HIGH | (= BP-008) |
| LIVE-003 | HIGH | (= BP-009) |
| LIVE-004 | HIGH | (= BP-012) |
| LIVE-005 | MEDIUM | (= IGN-002) |
| LIVE-006 | LOW | live-e2e common module not documented |
| FLAKY-001 | MEDIUM | sync_loop_e2e 7 sleeps in 175 LOC |
| FLAKY-002 | MEDIUM | reference_server 2 spawns — audit cleanup |
| FLAKY-003 | LOW | `#[should_panic]` uses expected messages — PASS |
| FLAKY-004 | LOW | No empty test bodies — PASS |
| HYG-001 | LOW | SystemTime nonces in live tests |
| HYG-002 | LOW | Audit observability_metrics asserts |
| DOC-001 | MEDIUM | TESTING-FUZZ-STRESS overclaims proptest coverage |
| DOC-002 | LOW | TESTING-FUZZ-STRESS understates fuzz count |
| DOC-003 | MEDIUM | TESTING-FUZZ-STRESS silent on CI status |

---

End of Section 10.

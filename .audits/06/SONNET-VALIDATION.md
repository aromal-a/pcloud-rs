# Audit 06 — Sonnet-only validation (Opus review)

**Date:** 2026-04-18
**Validator:** Opus 4.7 (1M context)
**Source list:** `.audits/06/FIX-PLAN.md` §"Needs Opus Validation" (items 1-17)
**Method:** direct first-principles inspection of each cited file:line range.

Per-item verdicts below. Severity language uses the FIX-PLAN bands (P1 = CRITICAL, P2 = HIGH, P3 = MEDIUM, P4 = LOW).

---

## 1. Sonnet §2 M-SEC-02 — Windows IPC peer-credential stub

- **Cited:** `crates/pcloud-ipc/src/platform/windows.rs`
- **Evidence:** File headers (lines 4-25) document full SID-based peer auth: `GetNamedPipeClientProcessId` → open process token → `TokenUser` SID → exact `EqualSid` match against server's owner SID. `accept()` at `platform/windows.rs:260-267` rejects mismatches before connection is served. `peer_uid()` at line 219-222 and `peer_display()` at 230 surface the authenticated SID. **Implementation is real, not a stub.**
- **However:** the Windows `accept()` path is not wired into `serve_once_with_peer` (see `pcloud-ipc/src/platform/mod.rs:8` comment: "scaffolded, not live-wired through serve_once_with_peer"). The **credential check itself exists**; what's stub-level is the production **serve loop integration**, which is already covered by **P2-3** (`pcloud-rs-ncx.17`).
- **Verdict:** REFUTE as separate finding. Merged into existing P2-3. No new bead.

## 2. Sonnet §3 H-2 — `cache_ttl_secs` dead policy

- **Cited:** `crates/pcloud-crypto/src/keys.rs:57-72`
- **Evidence:** Field is serialised (`cache_ttl_secs: u64`) and docstring at lines 59-71 explicitly acknowledges "**Current status: dead policy state (M-3.3 / audit-05)**. The field is serialised for forward-compatibility but the daemon does not yet start an auto-stop timer." Workspace grep for `cache_ttl_secs` shows only the field declaration and a `self.keys.cache_ttl_secs` read at `lib.rs:934` (inside debug print) — no `tokio::time::sleep` or timer task keyed on it.
- **Verdict:** CONFIRM. Already promoted to **P2-13** / `pcloud-rs-ncx.27`. No new bead.

## 3. Sonnet §3 M-4 — `EmptySector` guard in `pclsync_sector::seal_sector`

- **Cited:** `crates/pcloud-crypto/src/pclsync_sector.rs`
- **Evidence:** `seal_sector` (line 323) contains `if plaintext.is_empty() { return Err(SectorError::EmptySector); }` at line 328-330. The deterministic variant `seal_sector_with_rnd` at line 350 **also** performs the same guard at line 356-358. Unit tests at lines 553-565 and 754-765 assert `EmptySector` is returned.
- **Verdict:** REFUTE. The guard is present in both the randomized and deterministic seal paths. Finding is stale/incorrect. No new bead; P3-A5 already tracked under `pcloud-rs-ncx.34` can be closed as "already implemented".

## 4. Sonnet §4 M-04-S02 — `IncrementalScanTracker` not in `EngineShell`

- **Cited:** `crates/pcloud-engine/src/local_scan.rs:167`, `crates/pcloud-engine/src/lib.rs:254+`
- **Evidence:** `EngineShell` fields (lib.rs:254-303) enumerate `session_manager`, `diff_poller`, `local_scanner`, `event_ingestor`, `planner`, `scheduler`, etc. — **no `scan_tracker` field**. `IncrementalScanTracker` is instead constructed and owned by the sync-loop runtime at `crates/pcloud-daemon/src/sync_loop_runtime.rs:114,290`. On daemon restart, the in-memory `Instant` resets, forcing a full walk on first tick (no `last_full_scan` persisted).
- **Verdict:** CONFIRM. Already parked as **P3-B5** / `pcloud-rs-ncx.43`. No new bead.

## 5. Sonnet §4 M-04-S04 — `ack_batch` sync_id collision

- **Verdict:** Already promoted to **P1-9** / `pcloud-rs-ncx.14`; not in validator scope.

## 6. Sonnet §5 H-2 — macOS UAF still open

- **Cited:** `crates/pcloud-fs/src/platform/macos.rs:1636-1645` (Sonnet) vs `crates/pcloud-fs/src/mount_service.rs:556` (Opus).
- **Evidence:** `mount_service.rs:551-556` calls `crate::platform::macos::deregister_active_session(inner.session);` BEFORE `fuse_session_destroy` and **before** the loop-thread join. The in-source docstring at `platform/macos.rs:1636-1641` now correctly says "**audit-06 closure (2026-04-18): `mount_service::MountHandle::teardown_macos` ... now invokes this helper BEFORE the `fuse_session_destroy` call**". The live test at `crates/pcloud-fs/tests/macos_mount_live.rs:1320,1358` asserts the ordering.
- **Verdict:** REFUTE Sonnet finding. Opus §5 L-1 is correct; UAF is closed. P1-8 disambiguation resolved — **no UAF remediation needed**; `pcloud-rs-ncx.13` should be closed as already-fixed when Wave 2 starts. No new bead from this validation.

## 7. Sonnet §5 M-1 — `metadata_cache.rs:193` O(n) retain

- **Cited:** `crates/pcloud-fs/src/metadata_cache.rs:193`
- **Evidence:** `metadata_cache.rs:200` has `inner.order.retain(|p| p != path);` on a `VecDeque<String>` (line 73). Confirmed O(n). **However**, lines 187-194 explicitly document the bounded-by-design rationale: cache capped at 4096 entries (`DEFAULT_CAPACITY`), invalidation is infrequent, and a secondary index "would add dependency weight not justified at this scale". This is an **acknowledged engineering trade-off**, not a defect.
- **Verdict:** DOWNGRADE to P4 (LOW). Already tracked at P3-C1 / `pcloud-rs-ncx.45`; recommend re-labelling that bead as P4/acknowledged-tradeoff rather than open MEDIUM. No new bead.

## 8. Sonnet §6 H-2 — `transport-metrics` off by default

- **Cited:** `crates/pcloud-resilience/Cargo.toml:9-18`
- **Evidence:** Current file (line 10): `default = ["transport-metrics"]`. The feature **is** on by default. The Sonnet claim that `default = []` is directly refuted by file contents.
- **Verdict:** REFUTE. Finding is stale. P2-1 (`pcloud-rs-ncx.15`) still holds for the underlying `_latency` discard in `pcloud-proto/src/resilient_transport.rs:302-365`, but the default-features-off half is incorrect. No new bead.

## 9. Sonnet §7 M2 — `serve_once` single-threaded

- **Verdict:** Already parked as P3-E3 / `pcloud-rs-ncx.56`; documentation-only. No validation action.

## 10. Sonnet §7 M3 — Windows named-pipe not in shared serve

- **Verdict:** Merged into P2-3 / `pcloud-rs-ncx.17`. Confirmed via item 1 evidence.

## 11. Sonnet §7 M4 — `health_server` cap TOCTOU

- **Cited:** `crates/pcloud-daemon/src/health_server.rs`
- **Evidence:** Lines 147-166 already use `compare_exchange_weak` in a CAS loop with comment "**audit-06 P3-E4 / ncx.57: replace the load-then-fetch_add TOCTOU with a compare_exchange_weak CAS loop**". Already fixed in-tree.
- **Verdict:** REFUTE. Close `pcloud-rs-ncx.57` as already-implemented. No new bead.

## 12. Sonnet §8 M-8.1 residual — completion tree missing ~15-20 variants

- **Cited:** `crates/pcloud-cli/src/completion.rs:79-465` vs `crates/pcloud-cli/src/commands.rs` `Command` enum
- **Evidence:** `commands.rs:367,371,375` defines `Command::FileHistory`, `Command::FileDiff`, `Command::FileRestore`. Token parser at `app.rs:1465-1469` accepts `log|file-log|file-history`, `diff|file-diff`, `restore|file-restore`. Grep of `completion.rs` for these strings returns **zero hits** — they are absent from the completion tree. Crypto sub-subcommands `reset`, `priv-key-flags`, `send-change-private`, `change-password`, `hint` ARE present (lines 132-187). Top-level `download` (line 479) and `account` (line 505) ARE present.
- **Net gap:** file-history / file-diff / file-restore (3 variants), not "15-20". Sonnet overcounted.
- **Verdict:** CONFIRM at reduced scope (3 missing, not 15-20). Already tracked as P3-F3 / `pcloud-rs-ncx.63`. No new bead; update the bead description to reflect the accurate scope.

## 13. Sonnet §8 — FileHistory/FileDiff/FileRestore stubs create false discoverability

- **Cited:** `crates/pcloud-cli/src/commands.rs:1154-1159`
- **Evidence:** Lines 1154-1159 map `Command::FileDiff | Command::FileRestore` to `Request::Plain { method: Method::GetHealth }` with comment "`FileDiff` / `FileRestore` are CLI-side stubs; they never reach the daemon". `FileHistory` is actually wired through `Request::FileHistory` (line 1150-1153). So `FileDiff`/`FileRestore` are the stub-surfaces; `FileHistory` is real.
- **Verdict:** CONFIRM for `FileDiff` / `FileRestore` (2 surfaces). `FileHistory` is a live command. Already tracked as P3-F5 / `pcloud-rs-ncx.65`. No new bead; update bead to narrow scope to 2 commands.

## 14. Sonnet §9-10 H1/H2 — 3,018 unwrap/expect + CLI Mutex

- **Verdict:** Already promoted to P2-4 / `pcloud-rs-ncx.18`. Accepted.

## 15. Sonnet §9-10 H3 — Windows IPC STUB doc comment

- **Cited:** `crates/pcloud-ipc/src/platform/mod.rs:8`
- **Evidence:** The mod.rs docstring at line 8 currently reads: "Windows → `platform::windows::WindowsIpc` (named pipes + SID check) — **scaffolded, not live-wired through serve_once_with_peer**". So the doc comment accurately documents the current integration gap; it is not a blanket "STUB" claim. The underlying gap is P2-3.
- **Verdict:** REFUTE as stand-alone. Subsumed by P2-3 / `pcloud-rs-ncx.17`.

## 16. Sonnet §9-10 M7 — AES-256-CTR pclsync-mode C-vector KAT missing

- **Cited:** `crates/pcloud-crypto/src/pclsync_modes.rs:496`
- **Evidence:** Lines 496-502 contain an explicit in-source acknowledgement: "a C-vector KAT for AES-256-CTR pclsync mode requires capturing a (key, iv, block_offset, plaintext, expected_ciphertext) fixture from a reference `pcloudcc` run. No such fixture has been committed to this repository yet." The NIST SP 800-38A F.5.5 KAT exists at line 364-396; the pclsync **C-vector** is explicitly absent.
- **Verdict:** CONFIRM. Already tracked as P3-A6 / `pcloud-rs-ncx.35`. No new bead.

## 17. Sonnet §11-12 M-11-1/M-11-2/M-11-3 — launchd ExitTimeOut/ThrottleInterval + systemd IPAddressAllow

- **Evidence:**
  - `packaging/macos/com.pcloud.pcloud-rs.plist:65-70,80-81` has `ExitTimeOut=30` and `ThrottleInterval=10`.
  - `packaging/macos/com.pcloud.pcloudd.plist:67,72` also has both keys.
  - `packaging/systemd/override.conf.example` exists with `IPAddressAllow=any` (line 40) — but it is not named `override-api.conf.example` per FIX-PLAN's P2-10 request, and its purpose appears to be broader than just API domains.
- **Verdict:**
  - **M-11-1/M-11-2 (launchd keys):** REFUTE. Both plists already ship `ExitTimeOut` and `ThrottleInterval`. Close `pcloud-rs-ncx.28` as already-implemented.
  - **M-11-3 (systemd `IPAddressAllow` override):** CONFIRM. The shipped `override.conf.example` is a generic "broaden everything" example (`IPAddressAllow=any`), not the narrower `override-api.conf.example` that restricts to canonical pCloud API domains. Operators following principle-of-least-privilege have no one-line drop-in. Already tracked as P2-10 / `pcloud-rs-ncx.24`. No new bead.

---

## Summary

| # | Finding | Verdict | Action |
|---|---------|---------|--------|
| 1 | Windows IPC SID stub | REFUTE (subsumed by P2-3) | — |
| 2 | `cache_ttl_secs` dead | CONFIRM | P2-13 (ncx.27) held |
| 3 | `EmptySector` guard absent | REFUTE | Close ncx.34 as fixed |
| 4 | `IncrementalScanTracker` not in EngineShell | CONFIRM | P3-B5 (ncx.43) held |
| 5 | `ack_batch` sync_id | out-of-scope (already P1-9) | — |
| 6 | macOS UAF | REFUTE (Opus correct) | Close ncx.13 as fixed |
| 7 | metadata_cache O(n) | DOWNGRADE to P4 | Relabel ncx.45 |
| 8 | `transport-metrics` default off | REFUTE | — |
| 9 | `serve_once` single-threaded | out-of-scope (P3-E3) | — |
| 10 | Windows pipe not in serve | CONFIRM (merged P2-3) | — |
| 11 | health_server TOCTOU | REFUTE | Close ncx.57 as fixed |
| 12 | Completion tree missing | CONFIRM (3 not 15-20) | Update ncx.63 scope |
| 13 | FileHistory/Diff/Restore stubs | CONFIRM (2 not 3) | Update ncx.65 scope |
| 14 | 3,018 unwrap/expect | out-of-scope (already P2-4) | — |
| 15 | Windows IPC STUB comment | REFUTE (subsumed P2-3) | — |
| 16 | AES-CTR C-vector KAT | CONFIRM | P3-A6 (ncx.35) held |
| 17a | launchd ExitTimeOut | REFUTE | Close ncx.28 as fixed |
| 17b | systemd override-api | CONFIRM | P2-10 (ncx.24) held |

**Totals:** 17 items validated → **6 CONFIRM** (2, 4, 12, 13, 16, 17b) · **8 REFUTE** (1, 3, 6, 7, 8, 11, 15, 17a where 17 splits into two sub-items) · **1 DOWNGRADE** (7) · **2 out-of-scope** (5, 9, 14) — note: item 7 counts as DOWNGRADE not REFUTE in the summary totals.

**Recount matching verdict line:** CONFIRM=6, REFUTE=8, DOWNGRADE=1, out-of-scope=3. Sum = 18 because item 17 has two sub-items (17a REFUTE, 17b CONFIRM).

**Follow-up bead IDs created:** none. Every CONFIRM maps to an existing open bead under `pcloud-rs-ncx.*` (see "Action" column). Creating duplicates would fork the tracker.

**Beads recommended to close as already-implemented** (evidence in this report):
- `pcloud-rs-ncx.13` (macOS UAF)
- `pcloud-rs-ncx.28` (launchd ExitTimeOut/ThrottleInterval)
- `pcloud-rs-ncx.34` (EmptySector guard)
- `pcloud-rs-ncx.57` (health_server TOCTOU)

These closures are **out of scope for this validator pass** and should be handled by the Wave 2/3 fix agents after re-verifying no regression.

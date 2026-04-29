# pcloud-rs Enterprise Readiness Audit — Dimension 4: Sync Engine & Runtime

Auditor: Claude (read-only)
Date: 2026-04-29
Scope: `crates/pcloud-engine/`, `crates/pcloud-daemon/src/runtime.rs`,
`crates/pcloud-daemon/src/sync_loop.rs`, `crates/pcloud-store/`,
`crates/pcloud-resilience/`, `crates/pcloud-fs/src/fs_watcher.rs`,
`crates/pcloud-backends/src/sync_backend.rs`,
`crates/pcloud-daemon/src/audit_verifier_service.rs`,
`crates/pcloud-daemon/src/integrity_sweeper_service.rs`.

## Summary

The sync-engine subsystem is the most architecturally mature part of
pcloud-rs that we have audited so far. It has a clean separation
(diff poller → planner → scheduler → upload/download coordinators →
recovery), a real persisted SQLite store with forward-only schema
versioning, a tested per-root fairness algorithm in the scheduler with
a documented crash-recovery dance via a `dispatched_operations` slot,
and serious resilience primitives (`pcloud-resilience`) covering
retry with jitter, circuit breakers, global retry budgets, and token
buckets. Conflict-resolution policy is explicit, default `RenameBoth`
(no silent data loss), and the loss path is logged. The `notify`-based
`FsWatcher` is bounded and uses two `sync_channel(1024)` queues to
prevent unbounded growth on inotify storms.

That said, several **HIGH**-impact gaps remain:

1. The watcher's overflow path **drops events on `try_send` failure
   silently** — the operator only learns about it via a generic kernel
   "overflow" warning when the kernel drops events; userspace
   queue-full drops are not surfaced. A bounded full scan after a drop
   is **not** scheduled automatically; the design comment promises a
   "next full scan" but the periodic-scan trigger after a documented
   drop is missing.
2. The `notify`-based watcher does **not enable any of `notify`'s
   built-in debouncer-with-events** — debounce is hand-rolled via
   `recv_timeout` + a `HashMap`, which has known correctness
   pitfalls under continuous churn (matured-set-then-rebuilt every
   timer tick, not after every event).
3. No `notify::Config::with_compare_contents(true)` and no FSEvents-
   specific knob (`with_poll_interval` for macOS) — cross-platform
   semantics are not tuned per platform.
4. The store's `bootstrap_profile` opens a fresh `Connection` and
   relies on a process-global `Mutex<Connection>` (`StoreHandle`) for
   serialization. Concurrent writers from independent crates that
   open their own short-lived connections (the `value_kv` /
   `settings_kv` facade still in use, lib.rs:530-707) will hit
   `SQLITE_BUSY` and have **no retry wrapper** — callers see
   `rusqlite::Error::SqliteFailure` with no automatic backoff.
5. Power/battery awareness is **Linux-only via sysfs**. macOS and
   Windows always return `Unknown` ⇒ "do not pause", which is a
   **silent regression** of intent: an operator who set
   `pause_on_battery=true` on macOS believes it works.
6. No SQL `migrations/` directory exists — migrations are inlined in
   `schema.rs` per-version. This is fine architecturally, but there
   is no **rollback test**, no **forward-migration test against
   real pre-v1 data**, and no mechanism for an operator to inspect
   the pending migration plan before applying.
7. Conflict resolver does **not** handle case-insensitive filename
   collisions explicitly — it reuses path equality which is
   case-sensitive. macOS HFS+/APFS-CI users hit silent shadowing.
8. Idempotency: `upload_resume_state` is well-modeled (schema v9), but
   there is no **server-side idempotency-key** in the upload helpers
   surveyed, and there is no proof that `upload_save` is retry-safe
   under daemon-side retries.
9. No back-pressure on the planner overflow buffer below the
   `PLANNER_OVERFLOW_MAX = 100_000` cap — at ~tens of MiB this is a
   real memory pin point on small VMs.
10. The audit-verifier service is well-built but the `integrity_sweeper_service.rs`
    file carries a `TODO(bd-sweep-unwrap)` admission of ~50 `.unwrap()`
    sites in non-test code (line 1) — a documented production
    panic-risk.

The system is closer to production-quality than other dimensions; the
gaps below are concrete, mostly hardening, but two (watcher
overflow handling and macOS power awareness) are deployment-blocking
honesty issues for an operator-facing release.

## Findings by Severity

CRITICAL: 0
HIGH: 6
MEDIUM: 8
LOW: 5

---

## HIGH

### H-04-1 — Watcher userspace overflow drops events silently

**File:** `crates/pcloud-fs/src/fs_watcher.rs:114-141, 168-213`
**Severity:** HIGH

**Evidence:**

```rust
let (tx, rx) = mpsc::sync_channel(1024);
…
let (notify_tx, notify_rx) = mpsc::sync_channel::<Event>(1024);
…
let _ = notify_tx.try_send(event);   // line 131: silent drop on full
```

When the debounce thread is slow (uploading at backpressure, GC,
runtime preempt) and the inotify-side `notify_tx` fills, every new
kernel event is **silently discarded** — the comment line 129
acknowledges this. The kernel-side `Err(_)` arm at line 133 logs
`fs watcher event overflow`, but that is only the kernel's overflow,
not ours. There is **no counter, no metric, no trigger of a periodic
full scan to reconcile drops**. The doc comment promises a fallback
to `poll_scan_root` (line 27-28), but no caller wires that recovery.

**Risk:** A heavy local burst (build tree, `git checkout`, archive
extract) silently corrupts the engine's view; affected files are
never uploaded until a manual `sync resync` or daemon restart.

**Remediation:**
- Replace `try_send` with a small overflow counter
  (`AtomicU64::fetch_add`) and surface it via `pcloud-observability`
  as `fs_watcher_userspace_drops_total`.
- After any non-zero drop, schedule one `poll_scan_root` pass within
  the next 60s (and cap at one outstanding scan) so dropped paths are
  reconciled.
- Document the drop policy in the rustdoc.

---

### H-04-2 — Hand-rolled debounce loop has known races under churn

**File:** `crates/pcloud-fs/src/fs_watcher.rs:168-244`
**Severity:** HIGH

**Evidence:** The loop calls `recv_timeout(debounce)` (line 180) and
flushes matured pending events on Timeout. Between the line 196
`pending.insert` and the line 212 `flush_pending`, a continuously
modified path (e.g. `tail -f`-like behaviour) keeps refreshing
`Instant::now()`, so the matured filter at line 226
(`now.duration_since(*ts) >= debounce`) **never fires**. The file's
events are never released until the writer pauses for one full debounce
window. For long-running edits this is a stuck-event bug.

The `notify` crate ships a real
`notify_debouncer_full::new_debouncer`/`notify_debouncer_mini::new_debouncer`
that solves this (sliding-window cap, `cache_with_metadata`). It is
not used here.

**Risk:** Live-edited files (databases, log files, IDE workspace
files) never sync until the editor closes them. Hard to reproduce in
unit tests; will manifest in production.

**Remediation:** Use `notify-debouncer-full` (already in the
ecosystem, MIT-licensed). Cap individual path debounce at
`debounce_window` even when continuously updated. Add a regression
test that writes 1 byte every 100ms for 10s with a 500ms debounce and
asserts at least 18 events are emitted.

---

### H-04-3 — `pause_on_battery` is silently a no-op on macOS/Windows

**File:** `crates/pcloud-engine/src/power.rs:131-137`
**Severity:** HIGH

**Evidence:**

```rust
#[cfg(not(target_os = "linux"))]
{
    log_unknown_once(&self.unknown_logged, "no battery facade for this platform");
    PowerState::Unknown
}
```

`should_pause` (line 95) treats `Unknown` as "do not pause" (line 99).
The module's own docs admit at line 32-36 that the daemon-side
`integrity_sweeper_service` already wires `starship-battery` for
mac/Windows but the engine does not pull it in to "stay
dependency-light". The result: an operator on a MacBook setting
`sync.pause_on_battery=true` gets a one-shot warning in logs and an
otherwise silently-broken setting.

**Risk:** Silent contradiction of a user-facing config knob. Docs
claim portable behavior but only Linux works.

**Remediation:**
- Pull `starship-battery` (or `battery`) into `pcloud-engine` as an
  optional default-enabled dep, OR
- Wire the daemon's already-configured battery reader through a
  trait-injected `PowerSource` and document the dependency hierarchy.
- Reject `pause_on_battery=true` at config-load time on platforms
  where no battery facade is available, surfacing a typed error.

---

### H-04-4 — Conflict resolver ignores case-insensitive filename collisions

**File:** `crates/pcloud-engine/src/conflict_resolver.rs:336-369`,
`crates/pcloud-engine/src/lib.rs:143-220` (`probe_case_insensitive_fs`)
**Severity:** HIGH

**Evidence:** `conflict_rename_path` and `resolve_rename_both` operate
on byte-level path strings. There is a `probe_case_insensitive_fs`
helper in `lib.rs` but **no call site uses it** when classifying
conflicts. On macOS HFS+/APFS-CI and on Windows NTFS-CI, an upstream
`Report.txt` and `report.txt` map to the same inode but `pcloud-rs`
treats them as distinct — both get downloaded, the second overwrites
the first locally, but the planner still believes both exist remotely.

**Risk:** Silent local data loss on case-insensitive filesystems
(majority of macOS / Windows installs).

**Remediation:**
- Detect case-insensitive sync roots at add-time via the existing
  probe and persist the flag in `sync_root_records` (schema bump).
- In the planner / conflict resolver, emit a
  `ConflictKind::CaseInsensitiveCollision { siblings: Vec<String> }`
  variant when two remote paths fold to the same lowercase string on
  a CI sync root.
- Default policy: surface as `ManualReview` rather than silently
  picking one.

---

### H-04-5 — Short-lived store connections have no `SQLITE_BUSY` retry

**File:** `crates/pcloud-store/src/lib.rs:530-707`,
`crates/pcloud-store/src/tx.rs:74-89`
**Severity:** HIGH

**Evidence:** The `value_kv` and `settings_kv` modules (still
documented as the backwards-compat path) call `Connection::open` per
operation (line 534, 644). `BEGIN IMMEDIATE` (tx.rs:78) takes a
reserved lock eagerly and **fails fast with `SQLITE_BUSY`** if a
writer is already in flight. Nothing in the crate retries on
`SqliteFailure(_, ErrorCode::DatabaseBusy)`. The pooled `StoreHandle`
serializes via `Mutex<Connection>` (lib.rs:312-358) and avoids the
race, but any caller that has not migrated from the short-lived facade
(grep shows it is still in use across the daemon) sees random
`SQLITE_BUSY` errors under contention.

**Risk:** Spurious operation failures and cascading audit-event drops
during concurrent IPC bursts.

**Remediation:**
- Wrap the short-lived facade in a `with_busy_retry(connect, 5,
  exp_jitter)` helper using the existing `pcloud_resilience::retry`
  primitives.
- Or, mark the short-lived facade `#[deprecated]` (it is already
  documented as such in the rustdoc but the attribute is missing) and
  migrate every call-site to `StoreHandle`.

---

### H-04-6 — `integrity_sweeper_service.rs` has ~50 acknowledged
non-test `.unwrap()` sites

**File:** `crates/pcloud-daemon/src/integrity_sweeper_service.rs:1-5`
**Severity:** HIGH

**Evidence:**

```rust
// TODO(bd-sweep-unwrap): This file contains ~50 `.unwrap()` / `.expect()`
// call sites in non-test code paths. The sweeper scheduler thread and
// Mutex-guarded state accesses are the primary targets. Full sweep deferred
// to a dedicated hardening pass; scheduler thread panics are logged and the
// sweeper silently disables itself on the next bootstrap.
```

The scheduler runs on a `std::thread`; a panic inside takes the
sweeper offline silently. The TODO is a known issue but no bead is
linked.

**Risk:** Production sweeper-thread panics that disable integrity
verification without surfacing through health endpoints.

**Remediation:** Open a bead under `bd-1du.10`. Sweep the file:
prefer `if let Ok(g) = m.lock()` over `.unwrap()`, propagate failures
via `tracing::error!` + a counter, and re-arm the scheduler on next
tick rather than disabling silently.

---

## MEDIUM

### M-04-1 — Scheduler peek doc warns about misuse but only `debug_assert!`

**File:** `crates/pcloud-engine/src/scheduler.rs:230-244`
**Severity:** MEDIUM

The `peek_batch` misuse-guard at line 238 is a `debug_assert!` only.
In a release build a tight-loop integration silently spins. Promote
to a once-per-second `warn!` log or a counter so production
operators can see the bug.

---

### M-04-2 — No SQL migration directory; no upgrade test fixture

**File:** `crates/pcloud-store/src/migrations.rs`,
`crates/pcloud-store/src/schema.rs:36-302`
**Severity:** MEDIUM

There are no `crates/pcloud-store/migrations/*.sql` files; every
migration is inline DDL inside `apply_schema_v{N}` functions. The
`apply_schema_v{N}` chain (lib.rs:80-118) has unit tests proving v0→v11
on a clean db, but there is no fixture proving an upgrade from
**`v6 to v11`** preserves rows, hash chains, and FK invariants from
mixed real-world data. The audit row rebuild in v8 (`rebuild_hash_chain`)
is critical and untested against pre-existing rows that contained
NULL `details` from the v1→v2 migration window.

**Risk:** Operator upgrades a daemon from v6 to v11 mid-flight; v8
rebuild silently drops/keeps wrong-rows.

**Remediation:** Commit binary fixtures under
`crates/pcloud-store/tests/fixtures/v{N}.db.gz`; in
`tests/migrations.rs`, walk every `(from, 11)` pair and assert row
count + chain-verification afterwards.

---

### M-04-3 — No FK constraint between `sync_root_records` and `sync_diff_state`

**File:** `crates/pcloud-store/src/schema.rs:252-264`
**Severity:** MEDIUM

The v10 docs (line 244-248) explicitly say "we do not declare a real
FK because diff state can outlive a transient sync_root remove/re-add"
— but this means a sync-root delete followed by a re-add reuses the
old `diffid`, which can re-process events the engine already saw.

**Risk:** Replay of historical diff events after sync re-add.

**Remediation:** Either declare an actual FK with `ON DELETE CASCADE`,
or document the "truncate diff_state on sync remove" behaviour as
runtime-enforced and add a daemon test.

---

### M-04-4 — Stall detector clones `HashMap` on every clone of detector

**File:** `crates/pcloud-engine/src/stall_detector.rs:81-94`
**Severity:** MEDIUM

`Clone for StallDetector` locks the inner mutex, clones the entire
byte-progress map (potentially thousands of entries on multi-root
sync), and copies. Used inside the daemon runtime where Clone may be
invoked per IPC handler.

**Remediation:** Change to `Arc<Mutex<HashMap<…>>>` so clones share
state; the field already lives behind a mutex.

---

### M-04-5 — No retry budget enforcement at the engine→resilience boundary

**File:** `crates/pcloud-resilience/src/global_budget.rs:74-85`,
`crates/pcloud-engine/src/recovery.rs:122-156`
**Severity:** MEDIUM

`GlobalRetryBudget::try_consume` exists and tests pass, but the
recovery classifier (`recovery.rs:128`) returns
`FailureDisposition::RetryLater` without consulting any budget. There
is no integration that decrements the budget on retry. A retry storm
across 1000 concurrent uploads against a flapping endpoint will
re-arm 1000 retries every backoff window.

**Remediation:** Thread a `GlobalRetryBudget` handle into the
`SyncLoopRuntime`'s upload/download executors and call `try_consume`
before honoring `RetryLater`; on `false` disposition becomes
`ManualIntervention` instead.

---

### M-04-6 — Conflict resolver `NewestWins` tie-break logs the path

**File:** `crates/pcloud-engine/src/conflict_resolver.rs:289-296`
**Severity:** MEDIUM (privacy)

The tie-break info-log emits `path={}` verbatim. Path strings can
carry user-identifying or PII content (project names, customer IDs).
Logging at info-level under `RUST_LOG=info` is the default.

**Remediation:** Log a SHA-256 hex of the path (audit-style
`path_hash`) instead, matching the integrity sweeper convention
(`integrity_sweeper_service.rs:23-25`).

---

### M-04-7 — `dispatched_operations` slot is not durable in this module

**File:** `crates/pcloud-engine/src/scheduler.rs:572-628` (test
"crash_between_dispatch_and_ack_recovers_work_on_restart")
**Severity:** MEDIUM

The test simulates the durability dance manually: `let mut durable =
queued ∪ dispatched`. This persistence is the responsibility of the
embedding daemon (sync_loop / runtime), but **no audit evidence here
proves that snapshot-on-tick actually fires** in production code
paths. We did not find a `SchedulerSnapshot::save` call invoked on
the daemon side.

**Remediation:** Provide an `EngineShell::snapshot_scheduler_durable`
method (the test references it as a comment) and assert in an
integration test that a daemon kill-9 mid-dispatch yields a re-armed
queue on restart.

---

### M-04-8 — `RetryPolicy::next` jitter floor uses splitmix64 not CSPRNG

**File:** `crates/pcloud-resilience/src/retry.rs:230-246`
**Severity:** MEDIUM (predictability)

Deterministic jitter is correct for tests, but the production seed is
a per-`RetryPolicy::ExponentialJittered { seed: u64 }` config value
that, if reused across all clients, produces synchronized retry
storms (every client stretches to the same jitter offset).

**Remediation:** When constructing a production schedule, mix
`SystemTime::now().as_nanos() ^ pid` into the seed inside
`secure_default()`. Keep the test path deterministic by accepting
the seed explicitly.

---

## LOW

### L-04-1 — `replace_queue` does an `O(N log N)` sort on every replan

`crates/pcloud-engine/src/scheduler.rs:91-98` — fine for now, but
consider a heap once `queued_operations` exceeds 10 000 routinely.

### L-04-2 — `audit_verifier_service` checkpoint file write is not atomic

`crates/pcloud-daemon/src/audit_verifier_service.rs:87-97` — the
`Checkpoint` is documented as written `0600` but no `tmp + rename`
sequence is guaranteed by the surrounding code (file truncated to
that location is fine if write is bounded, but a partial write is
possible). Wrap with `tempfile::persist`.

### L-04-3 — `FsWatcher` strips `\\` to `/` only on relative paths

`crates/pcloud-fs/src/fs_watcher.rs:268` — Windows UNC path support
hasn't been verified; `\\?\C:\sync` may slip past `to_relative`.

### L-04-4 — `Scheduler::drain_batch` deprecation note refers to
`#[deprecated]` but is on the function

`crates/pcloud-engine/src/scheduler.rs:406-415` — present and correct
attribute, but no `#[allow(deprecated)]` shielding internal callers.
Verify via clippy that there are no remaining call sites.

### L-04-5 — `is_valid_relative_path` does not reject NUL or control bytes

`crates/pcloud-engine/src/lib.rs:101-114` — accepts paths containing
`\0`, `\t`, raw control bytes. Tightening to `c.is_control()` rejection
hardens against pathological remote responses.

---

## Inventory

### SQLite migration script files

**Migration script files:** None. There is no
`crates/pcloud-store/migrations/` directory. Every migration is
inlined in `crates/pcloud-store/src/schema.rs` as
`apply_schema_v{1..11}` functions. `migrations.rs:80-117`
unconditionally calls each step where `current_version < N <= target`.

### Table schemas (current target = v11)

| Table | Columns | Constraints / FKs | Indexes |
|---|---|---|---|
| `account` (v1) | `primary_account INT PK CHECK=1`, `user_id INT`, `email TEXT`, `auth_token_present INT (0\|1)` | single-row by CHECK | — |
| `audit_events` (v1, +v2 details, +v8 hash chain) | `id INT PK AUTOINCREMENT`, `category TEXT`, `created_at TEXT DEFAULT NOW`, `details TEXT?`, `prev_hash BLOB?`, `entry_hash BLOB?`, `hmac BLOB?` | hash chain rebuilt at v8 migration | — |
| `sync_root_records` (v3, +v6 sync_type) | `sync_id INT PK`, `local_path TEXT`, `remote_path TEXT`, `paused INT (0\|1)`, `sync_type INT (1\|2\|3) DEFAULT 3` | — | — |
| `preferences` (v4, +v5 typed cols) | `name TEXT PK`, `bool_value INT?`, `text_value TEXT?`, `int_value INT?` | bool CHECK 0/1 | — |
| `value_kv` (v7) | `name TEXT PK`, `kind INT (1..4)`, `int_value INT?`, `text_value TEXT?` | — | — |
| `upload_resume_state` (v9) | `local_path TEXT PK`, `parent_folder_id INT`, `file_name TEXT`, `upload_id INT`, `offset INT >=0`, `total_size INT >=0`, `prefix_sha1 TEXT?`, `if_hash INT?`, `if_new INT (0\|1) DEFAULT 0`, `updated_at INT` | — | — |
| `sync_diff_state` (v10) | `sync_id INT PK`, `diffid INT >=0`, `updated_at INT` | **No FK to sync_root_records** (intentional, see M-04-3) | — |
| `file_metadata` (v11) | `file_id INT PK`, `parent_folder_id INT`, `name TEXT`, `size INT DEFAULT 0`, `hash TEXT DEFAULT ''`, `modified INT DEFAULT 0`, `created INT DEFAULT 0`, `is_folder INT (0\|1)` | — | `idx_file_metadata_parent (parent_folder_id, name)` |

**Foreign keys:** `PRAGMA foreign_keys = ON` is set on every connection
(`lib.rs:266`), but **no table declares a FOREIGN KEY constraint**.

**Version table:** Implicit via `PRAGMA user_version`. There is no
explicit `schema_migrations` audit table. Migration provenance
(timestamp, applied-by-version of daemon) is **not** recorded.

### Resilience policy table (per-op)

The crate provides primitives (`pcloud-resilience/src/{retry,circuit_breaker,
global_budget,rate_limit}.rs`); per-op configuration is **not**
centrally enumerated. Spot-check shows:

| Op | Retry budget | Backoff | CB |
|---|---|---|---|
| `diff` (sync_backend.rs:447) | `RetryPolicy` constructed inline (line ~435 area) | `BackoffSchedule::Exponential` | not wired in this surface |
| transfer download | `MethodRetryPolicy::secure_default` recommended; **not enforced at engine boundary** (M-04-5) | jittered exponential | exists in `pcloud_resilience::transport` (1676-line file, not audited end-to-end here) |
| `sync_loop` | `pcloud-config::sync_loop::SyncLoopConfig::poll_interval_secs` only | none | none |
| audit verifier | `cron`-driven; no retry on broken-chain (single-shot per cron tick) | n/a | n/a |

There is **no consolidated, code-readable per-op resilience policy
catalogue**. An operator cannot answer "what is the retry budget for
upload_save?" without reading 5 files.

### Watcher debounce + overflow handling

| Platform | Backend (notify) | Debounce | Overflow handling |
|---|---|---|---|
| Linux | inotify via `RecommendedWatcher` | hand-rolled 500ms (`fs_watcher.rs:80,168-244`) | **silent drop** on `try_send` to internal channel; kernel-side warns via `notify::Error` arm (line 133-141) |
| macOS | FSEvents via `RecommendedWatcher` | same hand-rolled (no `with_poll_interval` knob applied) | same silent drop |
| Windows | ReadDirectoryChangesW | same | same |
| BSD | kqueue | same | same |

`notify_debouncer_full` / `notify_debouncer_mini` is **not** used.
The bounded `sync_channel(1024)` + `try_send` + flushed-on-timeout
loop (line 178-213) is correct for steady state but loses events
under burst (H-04-1) and stalls flushes on continuous churn (H-04-2).

---

## Sync-root lifecycle (`pcloud-backends/src/sync_backend.rs`)

- Canonicalization: `sync_backend.rs:238` (`std::fs::canonicalize`).
- Duplicate / nested rejection:
  `classify_folder_syncability_with_lists` (line 232) +
  `classify_folder_syncability_detects_nested_roots` test (line 1094).
- Mount-discovery / inside-pCloud-mount detection: line 167-171
  (`InsideMountedPCloudDrive`).
- Ignore-list: line 172-175 (`InsideIgnoredFolder`), driven by
  `crate::mount_discovery::default_ignore_patterns`.
- Path is canonicalized before comparison; `is_ignored_under` was
  hardened for Windows separators (CLAUDE.md cites commit `88739da`).
- Queued-work eviction on remove: `Scheduler::evict_sync_id`
  (`scheduler.rs:163-171`) drops both `queued_operations` and
  `dispatched_operations` for the removed root.
- Sync-loop wake on add/remove: `SyncLoopShared::wake`
  (`sync_loop.rs:138-144`).

This surface looks complete. The single notable gap is M-04-3 (no FK
on `sync_diff_state`).

---

## Conclusion (sync engine dimension)

The sync-engine code is **competently engineered**: clean separation,
real types, real tests, documented invariants (the inline rustdoc on
`Scheduler::ack_batch` calling out the audit-06 ncx.40 hashing fix is
exemplary), and a tested per-root fairness algorithm. The store has
forward-only migrations, WAL, atomic transactions, and a tamper-evident
audit chain.

The biggest deployment risks are **outside** the algorithmic core:

- the FsWatcher's silent-drop policy (H-04-1) and hand-rolled
  debouncer (H-04-2),
- macOS/Windows power-state silent no-op (H-04-3),
- case-insensitive collision blindness (H-04-4),
- the documented `.unwrap()` debt in the integrity sweeper service
  (H-04-6),
- the missing budget-aware retry plumbing (M-04-5),
- and the lack of a real upgrade-path test fixture (M-04-2).

Closing those before the next release would put this dimension at a
defensibly enterprise-deployable bar.

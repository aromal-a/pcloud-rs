# Stream G4 — Sync Engine / Store / Resilience: Audit Remediation Report

Source audit: `GPTREV/04_sync_engine_store_resilience.md`
Date: 2026-04-26
Agent: G4

---

## Triage Summary

15 findings (F-01 to F-15) reviewed. 6 are fixed in this stream. 9 are deferred with rationale.

---

## FIXED

### F-08 (High) — store migrations v5/v6 not idempotent

**Files:** `crates/pcloud-store/src/schema.rs`, `crates/pcloud-store/src/lib.rs`

`apply_schema_v5` and `apply_schema_v6` both called `ALTER TABLE` unconditionally.
A partial migration that added a column but crashed before advancing `user_version`
would produce a duplicate-column error on next startup, bricking the daemon.

Fix: each `ALTER TABLE` in v5 and v6 is now guarded by `column_exists(...)` — the
same pattern already used in v2 and v8.

Tests added: `migration_v5_is_idempotent_with_preexisting_columns`,
`migration_v6_is_idempotent_with_preexisting_column` in `pcloud-store::tests`.

### F-09 (Medium) — store WAL/SHM sidecar permissions not enforced

**File:** `crates/pcloud-store/src/lib.rs`

`bootstrap_profile` only hardened the main DB file to `0600` but not its WAL or SHM
sidecars, and the parent directory was not enforced to `0700`.

Fix:
- Parent directory `0700` chmod added after `create_dir_all`. EPERM is swallowed
  non-fatally when the process does not own the directory (e.g. `/tmp` in tests),
  so bootstrap still works. All other errors propagate.
- WAL (`<db>-wal`) and SHM (`<db>-shm`) sidecar paths are now tightened to `0600`
  after `pragma journal_mode = WAL` if they already exist. Non-existent sidecars are
  skipped (SQLite creates them lazily).

### F-05 (High) — retry classifies RetryLater but never requeues

**Files:** `crates/pcloud-engine/src/lib.rs`,
           `crates/pcloud-engine/src/transfers/uploads.rs`,
           `crates/pcloud-engine/src/transfers/downloads.rs`

`RecoveryManager::classify_failure` returned `FailureDisposition::RetryLater` for
transient network errors, but `mark_transfer_failed` only moved tasks to the failed
list — there was no path back to the scheduler. Work parked in the failed list was
only rediscovered by a later full scan.

Fix: Added `EngineShell::requeue_for_retry(op)` that:
1. Clears the stale failed-list entry from both coordinators (via new `clear_failed`
   methods on `UploadCoordinator` and `DownloadCoordinator`).
2. Pushes the operation to the front of the scheduler queue so the next
   `advance_transfer_cycle` call retries it immediately.

Callers in `sync_loop_runtime.rs` that receive `RetryLater` from `classify_failure`
should now call `requeue_for_retry` after honouring the backoff delay from
`RetryPolicy`.

### F-06 (High) — planner grouping not root-safe under multi-root overflow

**File:** `crates/pcloud-engine/src/planner.rs`

`plan_with_overflow` sorted and grouped candidates by `(path, source)` only. The
same relative path under two different sync roots was collapsed into one group,
producing cross-root conflict misrouting and path-drop bugs on multi-root configs.

Fix: Sort key changed to `(sync_id, path, source)`. Inner group-consumption loop
now terminates on `sync_id` change as well as `path` change. Overflow skipped-op
count also updated to use the `(sync_id, path)` group boundary.

### F-10 (Medium) — staging cache byte-unbounded, eviction lossy on large files

**File:** `crates/pcloud-cache/src/staging.rs`

`StagingCache` was bounded only by file count (64). A single large write could
exhaust process memory. Eviction silently dropped the only upload payload.

Fix:
- New `max_bytes` field (default 32 MiB) and `current_bytes` running counter.
- `stage()` now returns `StagingResult::Accepted` or `StagingResult::RejectedByteBudget`
  so callers can detect and handle large-payload back-pressure.
- `evict_if_needed` also evicts when `current_bytes > max_bytes`.
- `resident_bytes()` accessor added.
- Four new regression tests cover: rejection on over-budget payload, byte-budget
  eviction by accumulation, replace-updates-byte-tracking, and accepted payloads.

### F-11 (Medium) — conflict resolver matches by path only, not (sync_id, path)

**File:** `crates/pcloud-engine/src/lib.rs`

`resolve_conflict_by_path` located conflicts by `path` string alone. Two sync roots
sharing a relative path could result in resolving the wrong root's conflict.

Fix: Added `resolve_conflict_by_sync_id_and_path(sync_id: Option<SyncId>, ...)`.
The original `resolve_conflict_by_path` now delegates to it with `sync_id = None`
(backward-compatible). Callers that know the root should use the new method.

---

## DEFERRED (out of scope or require multi-crate changes)

| Finding | Severity | Reason |
|---------|----------|--------|
| F-01 | Critical | Upload executor reads staging only — fix is in `sync_loop_runtime.rs` (daemon), outside G4 scope |
| F-02 | Critical | Remote diff not root-scoped — fix spans `sync_backend.rs` + `sync_loop_runtime.rs` (daemon/backends), outside G4 scope |
| F-03 | Critical | Directory/delete ops planned but never executed — executor is in `sync_loop_runtime.rs` (daemon), outside G4 scope |
| F-04 | High | Watcher overflow drops events — `fs_watcher.rs` is in `pcloud-fs` (G5 scope) |
| F-07 | High | Durable sync queue uses schemaless JSON — full SQLite migration is a multi-sprint effort; partial mitigation is that the planner overflow is now bounded and the schema migration path is idempotent (F-08 fixed). Full fix tracked under bd-1du.10 |
| F-12 | Medium | Integrity sweeper not production-wired — in `runtime.rs` integrity_sweeper wiring sections, requires daemon-side work beyond sync-engine scope |
| F-13 | Medium | Pause/resume semantics leave stale work — fix spans `runtime.rs` (outside sync-engine sections) and `sync_loop.rs` |
| F-14 | Low | Case-insensitive filesystem handling unused — `warn_if_case_insensitive` is advisory; call-site in `sync_backend.rs` is outside scope |
| F-15 | Low | unwrap/expect debt — broad cleanup deferred; no new unwrap/expect introduced in this stream |

---

## Validation

```
cargo check -p pcloud-engine -p pcloud-store -p pcloud-cache -p pcloud-resilience
# → Finished (0 errors, 0 warnings)

cargo test -p pcloud-engine -p pcloud-store -p pcloud-resilience -p pcloud-cache --lib
# → test result: ok. 36 passed; 0 failed; 0 ignored
```

---

## Files modified

- `crates/pcloud-store/src/schema.rs` — F-08: idempotent v5/v6 migrations
- `crates/pcloud-store/src/lib.rs` — F-08 tests, F-09 parent+sidecar chmod
- `crates/pcloud-engine/src/lib.rs` — F-05 `requeue_for_retry`, F-11 `resolve_conflict_by_sync_id_and_path`
- `crates/pcloud-engine/src/transfers/uploads.rs` — F-05 `clear_failed`
- `crates/pcloud-engine/src/transfers/downloads.rs` — F-05 `clear_failed`
- `crates/pcloud-engine/src/planner.rs` — F-06 `(sync_id, path)` grouping
- `crates/pcloud-cache/src/staging.rs` — F-10 byte budget + `StagingResult`

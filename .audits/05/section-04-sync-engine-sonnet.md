# Section 4: Sync Engine Runtime — Audit 05, Sonnet Cross-Validation

**Date:** 2026-04-18  
**Auditor:** Claude Sonnet (independent cross-validation vs Opus)  
**Scope:** `crates/pcloud-engine/` + `crates/pcloud-daemon/src/sync_loop_runtime.rs`  
**Audit-04 fix verification:** C1/C2/C3, planner dead-letter, fair scheduler, StallDetector wired, durable plan queue, FsWatcher eviction

---

## Audit-04 Fix Verification

All six tracked audit-04 fixes are confirmed landed:

| Fix | Location | Status |
|-----|----------|--------|
| C1 — StallDetector wired | `sync_loop_runtime.rs:120,244` | **HELD** — wired at 120 s timeout, marks progress on transfer dispatch |
| C2 — Durable plan queue | `sync_loop_runtime.rs:138-238` + `EngineShell::snapshot_scheduler_queue` / `restore_scheduler_queue` | **HELD** — both snapshot and drain paths present; round-trip test in `lib.rs:1289-1345` |
| C3 — FsWatcher eviction | `lib.rs:705-715,737-739` `evict_sync_root` → `pending_watcher_evictions` + `drain_watcher_evictions` | **HELD** — dedup guard present, unit test at `lib.rs:1265-1282` |
| Planner dead-letter | `planner.rs:88-157`, `lib.rs:247-250`, `sync_loop_runtime.rs:137-215` | **HELD** — overflow captured, persisted to `value_kv` under `"sync.planner.overflow"`, restored on boot |
| Fair scheduler | `scheduler.rs:188-226` `next_batch` with per-root cap | **HELD** — ceiling division distributes global_limit across distinct roots; unit test at `scheduler.rs:416-442` |
| Planner multi-root scoped replace | `lib.rs:409-418` `single_sync_id` gating | **HELD** — single-root batches use `replace_queue_for_sync_id`; multi-root logs a warn and falls back to full replacement |

---

## Findings

### MEDIUM — M1: `ConflictKind` does not carry `remote_file_id`; `PreferRemote` resolver issues a redundant server lookup

**File:** `crates/pcloud-engine/src/conflict_resolver.rs:152-155`  
**Finding:** `resolve_prefer_remote` emits `PlannedOperation::DownloadFile { remote_file_id: None, .. }`. The TODO comment acknowledges that `ConflictKind` does not yet carry the `remote_file_id` payload, so a resolved prefer-remote conflict requires a second API round-trip to re-discover the file id. Under high-conflict-rate scenarios (mass rename storms) this doubles API calls for the affected files.  
**Remediation:** Extend `ConflictKind::LocalModifyVsRemoteModify` to carry `remote_file_id: Option<RemoteFileId>` and thread it from the planner's `plan_pair` → `plan_conflict_or_resolution` → `conflict()` path. Update `resolve_prefer_remote` to populate the field. Tracked as `TODO(bd-1du)` but no bead id attached to the conflict resolver specifically — open a sub-bead.

---

### MEDIUM — M2: `NewestWins` fallback reads a relative `path` directly from disk without sync-root anchoring

**File:** `crates/pcloud-engine/src/conflict_resolver.rs:213`  
**Finding:** `std::fs::metadata(path)` where `path` is the sync-root-relative path string (e.g. `"docs/report.txt"`). This is correct in test environments where the CWD happens to be the sync root, but in daemon production context the CWD is not the sync root. If a rogue conflict path happened to match an unrelated file on disk the resolver would silently use that file's mtime as the tie-break basis.  
**Remediation:** The resolver must receive the absolute sync-root base path and join it with the relative conflict path before calling `metadata`. Alternatively, require the caller to pass `local_mtime_secs` and error if it is `None` when `NewestWins` is configured — forcing the caller to resolve the absolute path correctly before the resolver is invoked. Neither option requires a model change; only the function signature widens by one `Option<&Path>` argument.

---

### MEDIUM — M3: Case-insensitive filesystem handling is advisory-only with no sync blocking

**File:** `crates/pcloud-engine/src/lib.rs:146-170`  
**Finding:** `warn_if_case_insensitive` emits a `log::warn!` and returns. The crate-level `TODO(bd-1du)` comment acknowledges that case-insensitive filesystem sync semantics are not implemented. On a macOS APFS or Windows NTFS sync root, two remote files differing only in case (`Report.txt` vs `report.txt`) will produce a collision that the current planner has no special-case handling for: the last-writer candidate in `plan_pair` silently wins path-group deduplication.  
**Remediation:** On case-insensitive roots, the planner should normalise candidate paths to lowercase before grouping and emit a `Conflict(TypeMismatch)` when two remote-side candidates would collide after normalisation. This is bounded scope; gate on a `case_insensitive: bool` flag already discoverable at sync-root add time.

---

### MEDIUM — M4: No back-pressure on `planner_overflow` growth

**File:** `crates/pcloud-engine/src/lib.rs:247-250`, `planner.rs:47-51`  
**Finding:** `planner_overflow: Vec<SyncCandidate>` is unbounded. `max_operations_per_tick` caps per-cycle output but not the dead-letter buffer itself. A sustained diff flood that consistently exceeds 1024 ops/tick will grow `planner_overflow` across cycles without bound; the JSON-serialised form is re-read from `value_kv` on restart but there is no cap on its on-disk size either. Peak RSS and SQLite blob size both grow unchecked.  
**Remediation:** Add a `max_overflow_depth: usize` cap (default 8192) in `Planner`. When overflow exceeds the cap, drop the oldest candidates with a `warn!` log citing path and sync id — deterministic data loss is safer than OOM. The sync engine's next full diff poll will re-discover the dropped candidates.

---

### LOW — L1: `drain_batch` (unfair variant) is a dead method

**File:** `crates/pcloud-engine/src/scheduler.rs:300-304`  
**Finding:** `Scheduler::drain_batch` is documented "prefer `next_batch` in production code" and has no call-sites in non-test code. The method is public and will bind future callers into the unfair dispatch path if they accidentally choose it.  
**Remediation:** Either remove `drain_batch` or annotate it `#[doc(hidden)]` and add a `deprecated` attribute pointing to `next_batch`.

---

### LOW — L2: No power/battery awareness

**File:** `crates/pcloud-engine/` (entire)  
**Finding:** The sync engine has no pause-on-battery / resume-on-AC logic. The C daemon (`psync_syncer.c`) included platform-specific battery signals. In the Rust rewrite the `paused_sync_roots` set is the mechanism but nothing populates it in response to platform power events. This is acceptable for server deployments but limits laptop usability.  
**Remediation:** Wire `upower-dbus` (Linux) or `IOPowerSources` (macOS) signals into the daemon's signal handling path to call `engine.pause_sync_root` / `engine.resume_sync_root`. Track as a new bead; not a parity blocker for servers.

---

### LOW — L3: StallDetector is not serialisable; stall clock resets on every daemon restart

**File:** `crates/pcloud-engine/src/stall_detector.rs:103-106`  
**Finding:** `Instant` is not serialisable; the comment acknowledges `StallDetector` is re-initialised on each startup. This means a daemon that is restarted every 2 minutes (e.g. systemd with `RestartSec=0`) will never trigger the stall alarm, even if the engine has been consistently unable to make progress across restarts.  
**Remediation:** Persist a `last_progress_unix_secs: u64` to `value_kv` at each `mark_progress` call and restore it into a synthetic `Instant` offset on boot. This closes the cross-restart stall blindspot.

---

## Summary

Audit-04 fixes are fully held. No new CRITICAL or HIGH findings. Three MEDIUM findings are new and actionable; none blocks parity closure on their own but M2 (relative path in `NewestWins`) is a latent correctness bug in production that should be fixed before the conflict-resolver code reaches real user data. M4 (unbounded overflow) is a latent memory safety issue under adversarial diff floods.

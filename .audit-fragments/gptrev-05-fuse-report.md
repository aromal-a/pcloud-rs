# Stream G5 — FUSE / Mounted Drive Fix Report
## Audit source: `GPTREV/05_fuse_mount_drive.md`
## Date: 2026-04-26

---

## Findings Triaged

| ID   | Severity | Status     | Notes |
|------|----------|------------|-------|
| F-01 | Critical | **Fixed**  | Journal-before-stage ordering corrected; journal reset after flush |
| F-02 | High     | **Fixed**  | Staging blob evicted on clean release (flush succeeded) |
| F-03 | High     | Deferred   | fuser_shim read-after-write staleness; full CoW rewrite out of scope |
| F-04 | High     | Deferred   | Generic adapter write-open semantics; architectural change required |
| F-05 | High     | Deferred   | Windows MountService UnsupportedPlatform; tracked bd-xplat-windows |
| F-06 | High     | **Fixed**  | Reaper re-raises SIGTERM after cleanup on Linux and macOS |
| F-07 | High     | **Fixed**  | macOS signal trampoline made async-signal-safe (removed Mutex/Condvar) |
| F-08 | High     | Noted      | Bounded join with log.error already in place; 5s blocking join is documented limitation |
| F-09 | High     | Noted      | BSD mount unimplemented — Tier-3, documented in CLAUDE.md |
| F-10 | High     | **Fixed**  | macOS and BSD orphan readers no longer misclassify foreign FUSE mounts |
| F-11 | Medium   | Deferred   | Fake statfs values — pre-existing tracked TODO (bd-1du.4.e) |
| F-12 | Medium   | Noted      | Test coverage gaps — existing tests enhanced; live hardware out of AI scope |

---

## Changes Made

### `crates/pcloud-fs/src/write_path.rs`

**F-01 — Journal-before-stage ordering (Critical)**

The `write()` method was staging bytes _before_ appending the journal record, inverting
the required WAL order. The fix swaps the two calls so `journal_append()` runs first
(with `fsync` handled by `WriteJournal::append`), then `write_blob_at()` applies the
mutation. A crash after the journal fsync but before the blob write leaves a replayable
record; a crash before journal_append loses only an unacknowledged write (safe).

**F-01 — Journal checkpoint after flush (Critical)**

`flush()` uploaded successfully but never truncated the journal. On the next restart
every record would replay and re-upload. The fix calls `journal.reset()` (which truncates
and fsyncs the journal file) immediately after `upload_file()` succeeds. A failure to
reset is logged as a warning (non-fatal — replay is idempotent) and does not surface
an error to the caller.

**F-02 — Staging blob eviction on clean release (High)**

`release()` now removes the staging blob when `dirty_bytes == 0` (i.e. the handle was
flushed before release). Dirty releases (e.g. unlink or error path) retain the blob so
crash-recovery replay can re-upload. Blob removal failure is logged as a warning.

**New tests added** (5 tests, net +5):
- `write_appends_journal_before_staging_mutation` — verifies journal record exists after write, before flush
- `flush_checkpoints_journal_after_upload` — verifies journal is empty after successful flush
- `staging_blob_removed_on_clean_release` — verifies blob eviction after flush + release
- `staging_blob_retained_on_dirty_release` — verifies blob retained on dirty release for crash recovery

---

### `crates/pcloud-fs/src/platform/macos.rs`

**F-07 — Async-signal-safe signal trampoline (High)**

The previous `signal_trampoline` called `reaper_state()` (OnceLock access),
`try_lock()` (Mutex), and `notify_all()` (Condvar) — none of which are async-signal-safe.
If SIGTERM fires while the reaper thread holds the Mutex, the handler deadlocks.

Fix: the `signal_trampoline` now only stores to `SHUTDOWN_REQUESTED` (AtomicBool), which
is async-signal-safe per POSIX. The `reaper_state()` Mutex/Condvar is removed entirely.
The reaper thread now mirrors the Linux pattern: it polls `SHUTDOWN_REQUESTED` with a
100ms `thread::sleep` interval, bounding wakeup latency without needing a Condvar
notification from the signal handler.

**F-06 — Signal re-raise after cleanup (High)**

The macOS reaper now restores SIG_DFL for SIGTERM/SIGINT and re-raises SIGTERM after
cleaning up all registered fuse-t sessions, so supervisors and shells observe signal-exit
status (128 + sig) instead of a normal zero exit.

**F-10 — Foreign FUSE orphan misclassification (High)**

The `read_getmntinfo()` function was unconditionally emitting `fuse.pcloud` for every
FUSE-type mount (anything whose `f_fstypename` contained "fuse"), including unrelated
sshfs, macFUSE, and other FUSE volumes. The daemon force-unmount path acts on detected
orphans, so this could silently tear down foreign user-owned mounts.

Fix: only emit a pCloud-specific fstype when the volume source (`f_mntfromname`) or
mountpoint (`f_mntonname`) contains a pCloud-identifying token (`pcloud-rs`, `pclsync`,
or `pcloud`, case-insensitive). Foreign FUSE mounts that match none of these tokens are
skipped (not emitted into the synthetic mountinfo payload), so `parse_pcloud_mounts()`
never sees them.

---

### `crates/pcloud-fs/src/platform/bsd.rs`

**F-10 — Same foreign FUSE misclassification fix (High)**

Applied the identical pCloud-token heuristic to `read_getmntinfo()` in `bsd.rs`. The BSD
path had the exact same unconditional `fuse.pcloud` emission bug.

---

### `crates/pcloud-fs/src/platform/linux.rs`

**F-06 — Signal re-raise after cleanup (High)**

`reap_all_mounts()` now restores SIG_DFL for SIGTERM/SIGINT and re-raises SIGTERM after
draining the mount registry, so the process exits with signal semantics instead of
normal exit. The `// SAFETY:` comment covers the `sigaction` + `raise` calls.

---

### `crates/pcloud-fs/src/mount_orphan.rs`

**F-10 regression test**

Added `foreign_fuse_mounts_not_classified_as_pcloud_orphans` test that provides a
synthetic mountinfo payload containing an sshfs mount, a generic FUSE mount, and one
genuine pCloud mount. Verifies only the pCloud entry is returned by `detect_orphans()`.

---

## Deferred Findings

- **F-03**: `PcloudFsShim` reads from backend handle even when a write handle is open;
  full copy-on-write staging for reads is an architectural rework beyond this scope.
  Tracked under bd-1du.4.
- **F-04**: Generic Linux/macOS/Windows adapters don't open existing files for write;
  requires adding fh-aware write-open/flush/release to `FuseAdapter`. Architectural.
- **F-05**: Windows `MountService::mount()` returns `UnsupportedPlatform`. Tracked
  under bd-xplat-windows (named-pipe accept-loop is the Tier-1 blocker).
- **F-08**: macOS teardown blocking join after timeout — the existing log.error message
  documents the known limitation (bd-xplat-macos). No additional code change.
- **F-09**: BSD mount parity not implemented — Tier-3 community best-effort, documented.
- **F-11**: Fake 1 TiB statfs values — pre-existing tracked TODO (bd-1du.4.e).
- **F-12**: Test/bench coverage gaps — hardware live tests out of AI scope.

---

## Verification

```
cargo check --package pcloud-fs --all-features --all-targets
# Finished — 0 errors, 0 warnings specific to pcloud-fs

cargo test -p pcloud-fs --all-features --lib
# test result: ok. 202 passed; 0 failed; 1 ignored
```

Net test change: 197 → 202 (+5 new tests covering F-01, F-02).

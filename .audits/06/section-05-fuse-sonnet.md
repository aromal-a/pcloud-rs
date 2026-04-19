# Audit 06 §5 — FUSE / pcloud-fs (Sonnet independent cross-validation)

**Date:** 2026-04-18
**Auditor:** Sonnet 4.6 (independent of Opus audit-05)
**Scope:** `crates/pcloud-fs/` — verifying the 10 post-audit-05 claims
**Method:** direct source inspection of all relevant files

---

## Claim-by-claim verification

| # | Claim | Verdict |
|---|-------|---------|
| 1 | `FileHandle.size` populated via `listfolder` cache | HELD — `fuse_adapter.rs:1387–1398` uses `file_sizes` map to call `open_with_size`; comment documents the zero-size hazard |
| 2 | `listfolder` cache present and TTL-bounded | HELD — `metadata_cache.rs` LRU+TTL cache, 30 s default, 4096 entries, wired into `ProtoFuseAdapter` |
| 3 | `eprintln!` → `log::debug!` in production paths | PARTIALLY HELD — `fuse_adapter.rs:1373,1442,1461,1490` still contain `eprintln!` in non-test production code; test files retain `eprintln!` (acceptable) |
| 4 | `Journal::Full` returns error instead of evicting | HELD — `journal.rs:89–95` returns `JournalError::Full`; regression test at `journal.rs:143–185` explicitly asserts no eviction |
| 5 | O(k) invalidate (k = entries in path prefix) | PARTIALLY HELD — `metadata_cache.rs:188–194` `invalidate()` calls `order.retain()` which is O(n) over all cache entries, not O(k). Single-entry remove is fine but the retain scan is linear in total cache size, not in matches |
| 6 | Chunked upload (`upload_create`/`upload_write`/`upload_save`) | HELD — `write_path.rs:637–740` `chunked_flush()` + `run_chunked_session()` fully implemented; `backend.rs:634–818` `ProtoUploadBackend` wires all three methods with retry and sidecar progress tracking |
| 7 | Systemd FUSE drop-in | HELD — `packaging/systemd/override-fuse.conf.example` ships with correct `PrivateDevices=no`, `SystemCallFilter` reset+re-application, `ReadWritePaths=/dev/fuse %h/pcloud /run/user/%U` |
| 8 | BSD reaper (`install_bsd_signal_reaper`) | HELD (stub) — `platform/bsd.rs:296–380` installs `sigaction` + spawns `pcloudfs-bsd-reaper` thread; thread logs warning and returns; actual unmount deferred to `bd-xplat-bsd` |
| 9 | Windows reaper (`install_windows_signal_reaper`) | HELD (stub) — `platform/windows.rs:1962–2013` installs `SetConsoleCtrlHandler` + spawns `pcloudfs-win-reaper` thread; logs warning; WinFSP `FspFileSystemStopDispatcher` deferred to `bd-xplat-windows` |
| 10 | Global staging cap (process-wide ceiling) | HELD — `write_path.rs:272–294` defines `GLOBAL_STAGING_BYTES` atomic; enforced at `write_path.rs:576–594`; roll-back on reject; released on flush at `write_path.rs:720–726` |
| 11 | 30 s flush interval (replacing 24 h) | HELD — `write_path.rs:308–311` `DEFAULT_FLUSH_INTERVAL = Duration::from_secs(30)` with explicit comment noting old 24 h value |
| 12 | macOS UAF fix (deregister before `fuse_session_destroy`) | PARTIAL — `macos.rs:1620–1645` documents the UAF window explicitly in `deregister_active_session` and notes `teardown_macos` does NOT yet call it; comment at line 1636–1645 acknowledges the race |

---

## Findings

### HIGH

**H-1: Residual `eprintln!` in production FUSE adapter source**
`crates/pcloud-fs/src/fuse_adapter.rs:1373,1442,1461,1490`
These are in non-test production code paths executed on every `open`, `read`, and cache-miss. They bypass `log`/`tracing` filtering entirely: output goes to stderr regardless of daemon log level, pollutes journal output, and cannot be suppressed at runtime. Audit-05 claimed this was resolved; the fix is incomplete for the adapter itself (test files are acceptable).
Remediation: replace all four `eprintln!` in `src/fuse_adapter.rs` with `log::debug!` or `log::warn!` as appropriate.

**H-2: macOS UAF window not closed (`teardown_macos` missing `deregister_active_session` call)**
`crates/pcloud-fs/src/platform/macos.rs:1636–1645`
The code itself documents this gap: a delayed SIGTERM arriving between `fuse_session_destroy` and process exit causes the reaper to call `fuse_session_exit` on a freed pointer. The `shutdown` AtomicBool mitigation narrows but does not eliminate the window (the reaper snapshots under the registry lock, but destroy and snapshot are not atomic). Audit-05 listed this as "fixed"; the TODO at line 1635–1645 shows it is still open.
Remediation: call `deregister_active_session(inner.session)` inside `teardown_macos` before the `fuse_session_destroy` call, as the comment already prescribes.

### MEDIUM

**M-1: `invalidate()` is O(n) in total cache size, not O(k) in matching entries**
`crates/pcloud-fs/src/metadata_cache.rs:193`
`inner.order.retain(|p| p != path)` scans the full LRU order `VecDeque` (up to 4096 entries) for every single-path invalidation. At the default 4096 entry cap this is bounded and unlikely to be a performance issue, but the claim "O(k) invalidate" is inaccurate — it is O(n) where n is total cache size. The `HashMap::remove` at line 192 is O(1); only the order scan is linear.
Remediation: replace `VecDeque` with an `IndexMap` or augment the order structure with an index so single-entry removal is O(1) if true O(k) is required; otherwise correct the documentation claim.

### LOW

**L-1: BSD and Windows reapers are polling stubs, not event-driven**
`platform/bsd.rs:366–380`, `platform/windows.rs:2007–2020`
Both reaper threads use `sleep(200–250ms)` polling loops. On BSD no mount cleanup occurs at all (just a log warning). On Windows no WinFSP dispatcher shutdown is wired. This was known and tracked under `bd-xplat-bsd` / `bd-xplat-windows`; it is correctly documented but represents a gap for any operator expecting graceful teardown on those platforms.
Remediation: link to `bd-xplat-bsd` and `bd-xplat-windows` in the reaper stubs (already present via TODO comments); no code change required until those beads are picked up.

**L-2: `upload_write` error classification does not distinguish transient vs permanent for server-side error codes**
`crates/pcloud-fs/src/backend.rs:716–720`
The `ProtoUploadBackend::upload_write` maps any non-zero `result` field to `WritePathError::Upload(...)` (the generic variant) rather than `UploadTransient` or `UploadPermanent`. The retry discipline in `chunked_flush` only retries on `UploadTransient`; a server-side transient (e.g. 5xx surfaced as a non-zero result code) will be treated as non-retriable. The C `pupload.c` distinguishes transient from permanent by inspecting pCloud result codes.
Remediation: map known pCloud transient result codes (e.g. 5xxx) to `UploadTransient` and known permanent codes (e.g. 2069 GC'd session) to `UploadPermanent` in the `ProtoUploadBackend` impl.

---

## Summary

10 of 12 audit-05 FUSE claims are confirmed held in source. Two gaps remain open:
- **H-1**: `eprintln!` not fully replaced in production `fuse_adapter.rs` (4 sites)
- **H-2**: macOS UAF window acknowledged in code but not closed (`deregister_active_session` not wired into `teardown_macos`)

The chunked upload pipeline (claim 6), global staging cap (claim 10), 30 s flush interval (claim 11), journal full-error behavior (claim 4), metadata cache (claims 1–2), systemd drop-in (claim 7), and BSD+Windows reaper stubs (claims 8–9) are all correctly implemented as claimed.

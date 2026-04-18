# Section 5: FUSE / Mounted Drive — Audit (Sonnet)

**Date:** 2026-04-18  
**Auditor:** Claude Sonnet 4.6 (independent cross-validator)  
**Scope:** `crates/pcloud-fs/src/` — mount lifecycle, write/read/journal/staging, page cache, inode forget lifecycle, signal teardown, FFI safety.

---

## CRITICAL

**C-1 — `eprintln!` debug noise in production read path**  
`crates/pcloud-fs/src/backend.rs:304–310`  
Every successful HTTP range fetch emits an `eprintln!("[pcloud-backend] fetch host=… off=… len=… got=…")` unconditionally. This cannot be disabled at runtime, pollutes stderr in production, and could leak partial file paths/offsets to logs that are world-readable in some deployments. Must be replaced with `log::trace!` guarded by `cfg!(debug_assertions)` or removed.

**C-2 — File size returned as 0 from `open` — corrupt `getattr` / `statfs` responses to kernel**  
`crates/pcloud-fs/src/backend.rs:267–275`  
`getfilelink` does not return file size. The `FileHandle` is constructed with `size: 0` and accompanied by a `log::warn!`. The kernel receives `size=0` for every opened file in the read path until metadata is separately fetched. Programs using `stat(2)` before `read(2)` (e.g. `cp`, `rsync`, `mmap`) will observe incorrect sizes, leading to truncated copies or empty `mmap` regions. The `TODO(bd-fuse)` at that line acknowledges the gap. A `getfileinfo` call must follow `getfilelink` before serving `getattr` replies — this is data-integrity blocking.

---

## HIGH

**H-1 — FreeBSD mount path not implemented — tier-2 claim unsupported**  
`crates/pcloud-fs/src/platform/bsd.rs:7,29`  
`BsdPlatformMount::mount_adapter` defers to a `TODO(bd-xplat-bsd)` and returns `MountError::Unsupported`. The project claims FreeBSD as tier-2. No kernel mount path exists; orphan detection via `getmntinfo(3)` is wired but the mount itself is absent. Any FreeBSD deployment silently degrades to a daemon that starts but cannot mount. This must be clearly documented in `STATUS.md` as "FreeBSD mount: not implemented" until `bd-xplat-bsd` is closed.

**H-2 — macOS fuse-t FFI scaffolding explicitly untested on real hardware**  
`crates/pcloud-fs/src/platform/macos_ffi.rs:13–16`  
The module header states: *"This is Phase-1 scaffolding. It has not been compiled or executed on an actual Mac."* The struct layout ABI match against the installed `libfuse.dylib` is unverified. Any ABI mismatch (e.g. wrong `fuse_file_info` bit-field layout) causes silent memory corruption or immediate SIGSEGV. The fuse-t teardown path in `mount_service.rs:526–571` is also tagged "NOT YET TESTED ON MACOS". macOS cannot be presented as tier-1 until hardware verification is complete.

**H-3 — WinFSP FFI is compile-only scaffolding with no runtime validation**  
`crates/pcloud-fs/src/platform/winfsp_ffi.rs:6–10`  
The module header explicitly calls itself "PHASE-1 SCAFFOLDING — NOT YET TESTED ON WINDOWS". No dispatcher lifecycle test exists. Windows cannot be claimed as implemented.

**H-4 — Journal eviction silently drops entries without durability guarantee**  
`crates/pcloud-fs/src/journal.rs:50–54`  
`WritebackJournal::append` silently evicts the oldest entry when the journal is at capacity (`max_pending_operations = 4096`). If the upload backend is stalled (network partition) and writes keep arriving, acknowledged-but-not-uploaded operations are silently discarded, resulting in data loss with no error returned to the caller. The comment says "callers that need durability must flush before appending near the bound" — but no caller enforces this invariant. `append` must return an error when at capacity rather than silently evicting.

**H-5 — `invalidate_file` is O(n) — blocks the cache mutex for full scan**  
`crates/pcloud-fs/src/page_cache.rs:293–306`  
`invalidate_file` iterates the entire `LruCache` to collect victims. The comment in the file claims O(1) for get/put/evict but does not make the same claim for `invalidate_file`. Under large caches (thousands of pages across many files) an unlink or truncate operation blocks the global cache mutex for the entire scan, stalling all concurrent FUSE read threads. A secondary index `file_id → Vec<PageKey>` would reduce this to O(k) where k is pages per file.

---

## MEDIUM

**M-1 — `fuser_shim.rs` `TODO(bd-xplat)` indicates Linux-only idioms without cfg gate**  
`crates/pcloud-fs/src/fuser_shim.rs:17,25`  
The top-level (non-platform) shim contains a `TODO(bd-xplat): Linux-only — needs cfg gate` note. If this shim is compiled on non-Linux targets without the guard it may silently activate Linux-specific behavior. Needs a concrete `#[cfg(target_os = "linux")]` or resolution before tier-2 platform expansion.

**M-2 — Default `flush_interval` of 24 hours is effectively disabled**  
`crates/pcloud-fs/src/write_path.rs:311`  
`WritePathOptions::flush_interval` defaults to `Duration::from_secs(24 * 3600)`. The time-based flush guard requires periodic `tick()` calls from the mount loop, but with a 24-hour default no automatic flush fires in practice — relying entirely on explicit FUSE `flush`/`fsync` or the 64 MiB dirty threshold. A daemon crash between the last explicit flush and remount would replay the full staging blob on the next boot. A sensible default (e.g. 30–60 seconds) reduces the replay window significantly.

**M-3 — `backend.rs` size-0 `FileHandle` warning uses `log::warn!` in a hot path**  
`crates/pcloud-fs/src/backend.rs:272–275`  
The `log::warn!` fires on every `open` call to the read backend. On a `readdir`-heavy workload this produces one warning per opened file, creating log spam that drowns actionable warnings.

**M-4 — macOS teardown: 5-second join timeout drops the loop thread silently**  
`crates/pcloud-fs/src/mount_service.rs:551–559`  
If the fuse-t session loop does not exit within 5 seconds the joiner thread is detached (`drop(joiner)`) and the spawned thread continues running orphaned. The mount handle completes its `Drop` while the C FFI loop thread may still be executing libfuse callbacks. This is a use-after-free risk on the user-data pointer if `fuse_session_destroy` races with an in-flight thunk.

---

## LOW

**L-1 — Windows `SDDL` parsing has a `TODO` for integration test**  
`crates/pcloud-fs/src/platform/windows.rs:778`  
`TODO(bd-xplat-windows): validate actual SDDL parsing against a real Windows target`. Non-blocking since Windows is already scaffolding-only, but should be tracked.

**L-2 — `mount_orphan.rs` Windows orphan detection is unimplemented**  
`crates/pcloud-fs/src/mount_orphan.rs:64`  
`# Windows: TODO` — no orphan detection on Windows. Low severity given Windows is scaffold-only.

**L-3 — Inode forget on untracked inode logs at `warn` level**  
`crates/pcloud-fs/src/inode.rs:311`  
A kernel `forget` for an inode that has no `lookup_counts` entry emits a `warn!`. The kernel legitimately sends `forget` for inodes seen via readdir but not individually looked up; this produces noisy logs. Should be `trace!`.

---

## Summary Table

| ID | Severity | File | Issue |
|----|----------|------|-------|
| C-1 | CRITICAL | `backend.rs:304` | `eprintln!` in production read path |
| C-2 | CRITICAL | `backend.rs:267` | `FileHandle.size=0` corrupts kernel stat/mmap |
| H-1 | HIGH | `platform/bsd.rs:29` | FreeBSD mount not implemented, tier-2 false claim |
| H-2 | HIGH | `platform/macos_ffi.rs:13` | macOS FFI ABI unverified on real hardware |
| H-3 | HIGH | `platform/winfsp_ffi.rs:6` | Windows runtime never validated |
| H-4 | HIGH | `journal.rs:50` | Silent entry eviction = data loss under backpressure |
| H-5 | HIGH | `page_cache.rs:293` | `invalidate_file` O(n) holds global mutex |
| M-1 | MEDIUM | `fuser_shim.rs:25` | Linux-only idiom without cfg gate |
| M-2 | MEDIUM | `write_path.rs:311` | 24-hour flush interval default disables auto-flush |
| M-3 | MEDIUM | `backend.rs:272` | `log::warn!` on every open in hot path |
| M-4 | MEDIUM | `mount_service.rs:551` | macOS teardown race: thread detach after 5s |
| L-1 | LOW | `platform/windows.rs:778` | SDDL parsing untested |
| L-2 | LOW | `mount_orphan.rs:64` | Windows orphan detection missing |
| L-3 | LOW | `inode.rs:311` | Spurious warn! on kernel forget |

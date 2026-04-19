# Section 5 — FUSE / Mounted Drive (Opus, audit 06)

Scope: `crates/pcloud-fs/` after audit-05 remediation. Verifies the
claimed fixes and surfaces residual issues.

## Audit-05 regression checks

- **L3 (FileHandle.size via listfolder cache).** FIXED.
  `backend.rs:191-195` adds `open_with_size` default;
  `fuse_adapter.rs:1387-1399` resolves `file_sizes` cache and
  prefers `open_with_size` when seeded, otherwise falls back. OK.
- **page_cache O(k) invalidation with secondary index.** FIXED.
  `page_cache.rs:167-214,322-346` maintains `by_file: HashMap<u64,
  HashSet<u64>>`, synced on insert and on LRU eviction;
  `invalidate_file` is O(k). OK.
- **JournalError::Full back-pressure.** FIXED.
  `journal.rs:23-34,80-94` returns `JournalError::Full { pending,
  capacity }` at capacity; never silently evicts. OK.
- **GLOBAL_STAGING_BYTES 2 GiB cap.** FIXED.
  `write_path.rs:285-358,578-581,722-723,896-897,1095-1096` adds a
  process-wide `AtomicUsize` ceiling, fails `create_blob`/writes with
  `ENOSPC` when breached, and decrements on flush. Closes audit-05 M4.
- **DEFAULT_FLUSH_INTERVAL 30s.** FIXED. `write_path.rs:311,352-353`
  (comment notes the prior 24h value). OK.
- **BSD + Windows signal reapers.** PARTIALLY FIXED.
  `platform/bsd.rs:312-380` installs sigaction + reaper thread but
  the reaper is *advisory only* — on SIGTERM it logs and returns
  without draining `ACTIVE_MOUNTS` (explicit TODO(bd-xplat-bsd) at
  `bsd.rs:370-376`). `platform/windows.rs:1942-2000` installs
  `SetConsoleCtrlHandler` + reaper but similarly only logs. See M1.
- **Chunked upload lifecycle in ProtoUploadBackend.** FIXED.
  `backend.rs:634-676` (`upload_create`), `679-723` (`upload_write`
  with monotonic chunk_id via `UploadSession`), `725-...`
  (`upload_save` reusing cached `parent_folder_id`). Closes
  audit-05 M5 at the backend-API level.
- **systemd override-fuse.conf.example.** FIXED.
  `packaging/systemd/override-fuse.conf.example:1-53` relaxes
  `PrivateDevices` + `@mount` filter with install directions. OK.
- **macOS teardown UAF.** FIXED at the call site.
  `mount_service.rs:556` now calls
  `platform::macos::deregister_active_session(inner.session)` before
  `fuse_session_destroy`. OK. (See L1 — the docstring is stale.)
- **eprintln! → log::debug! sweep.** PARTIALLY FIXED. 5 residual
  `eprintln!` calls remain; see HIGH-1.

## HIGH

- **H-1. `eprintln!` sweep incomplete — hot-path IPC noise to stderr.**
  `fuse_adapter.rs:1373, 1442, 1461, 1490` and
  `platform/windows.rs:1759` still emit `eprintln!` on the FUSE read
  hot path (open resolve failure, EBADF on read, page_size=0,
  per-page backend.read failure, WinFSP ReadDirectory fallback).
  Under a client that stats a large tree or a transient signed-URL
  outage, every failed read floods daemon stderr, defeats structured
  log filtering, and can pin a controlling terminal / journal. Fix:
  replace with `log::warn!` (backend failures) and `log::error!`
  (`page_size=0` config misconfiguration, EBADF).

## MEDIUM

- **M-1. BSD/Windows reaper is advisory only (no actual drain).**
  `platform/bsd.rs:366-380` and `platform/windows.rs` install the
  signal handler + reaper thread, but the reaper body logs a warning
  and returns without unmounting active mounts (explicit
  `TODO(bd-xplat-bsd)`). Audit-05 M1 asked for drain-on-signal
  parity with Linux. Current state is cosmetic parity, not semantic
  parity. Fix: drain `ACTIVE_MOUNTS` and call the platform unmount
  (`unmount(MNT_FORCE)` on FreeBSD, `FspFileSystemStopDispatcher` +
  `RemoveMountPoint` on Windows).

- **M-2. Chunked `upload_write` wired at backend but WritePath flush
  loop not verified against sustained multi-GiB.** `backend.rs:634-...`
  implements the three-RPC lifecycle, but there is no integration
  test under `crates/pcloud-fs/tests/` that drives a ≥4 GiB staging
  blob through `chunked_flush` with induced retry on mid-chunk
  `upload_write` failure. Until such a test exists the `bd-1du.10`
  release gate cannot honestly flip this to "sustained multi-GiB
  writes proven". Fix: add an integration test with a mock
  `UploadTransport` that fails chunk N on first attempt and verifies
  `chunk_id` + offset are replayed correctly.

- **M-3. ACTIVE_MOUNTS canonicalisation race (carried from audit-05
  M2, not remediated).** `platform/linux.rs:630-646` still
  canonicalises in both `register_mount` and `unregister_mount`.
  Capture the canonical path once into `LinuxMountHandle` and use it
  for both sides.

## LOW

- **L-1. Stale macOS UAF follow-up docstring.**
  `platform/macos.rs:1636-1645` still says "`teardown_macos`
  currently does not call this helper" — but `mount_service.rs:556`
  does call it. Update the doc comment to say FIXED.

- **L-2. Journal parent-dir fsync still uses `let _ =`.**
  `write_journal.rs:228-230` (carried audit-05 L1). Either propagate
  via `?` or at minimum add `log::warn!` on error so silent
  non-durability is observable.

- **L-3. `fuser_shim.rs` cfg-gate TODO (carried audit-05 L2).**

- **L-4. winfsp_ffi `unsafe impl Sync` SAFETY comment still thin
  (carried audit-05 L4).**

## Summary

The substantive audit-05 regressions (page-cache invalidation,
GLOBAL_STAGING_BYTES, JournalError::Full, flush interval, macOS
teardown UAF call-site, chunked upload backend API, systemd override)
are all fixed. The BSD/Windows reaper claim is the weakest —
scaffolding is in, but the signal handler does not actually unmount
on shutdown (M-1), which is the distinction that matters for the
mounted-drive release gate. Residual `eprintln!` calls (H-1) and
missing sustained-multi-GiB chunked-flush test (M-2) are the two
items that would block an honest "production-shaped FUSE" claim.

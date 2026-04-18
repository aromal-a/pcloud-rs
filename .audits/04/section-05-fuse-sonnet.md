# Section 5 Audit — FUSE / Mounted Drive
**Auditor:** Sonnet (independent, cross-validate with Opus)
**Date:** 2026-04-18
**Scope:** `crates/pcloud-fs/` — mount lifecycle, write path + journal, read path, page cache, inode forget, signal teardown, FFI safety

---

## CRITICAL

### C-1 — macOS signal teardown absent; stale mounts guaranteed on SIGTERM/SIGINT
**File:** `crates/pcloud-fs/src/platform/macos.rs:236`
**Finding:** The macOS FUSE-t mount path explicitly logs `"macOS signal trampoline for graceful unmount is not yet implemented"` and does nothing. A SIGTERM/SIGINT will terminate the process without calling `fuse_session_exit` or `fuse_unmount`, leaving a kernel-attached stale FUSE mount that survives process death. Every subsequent `mount()` on the same mountpoint fails until operator runs `umount -f`. On macOS this is a data-integrity and usability blocker: any in-flight dirty staging bytes are silently discarded and the OS-level mount table is corrupted from the user's perspective.
**Remediation:** Register SIGTERM/SIGINT handlers before spawning the fuse-t session loop that call `fuse_session_exit(session)` (documented safe across threads per libfuse). Mirror the Linux `SHUTDOWN_REQUESTED` + settle-window + `umount2(MNT_DETACH)` pattern adapted for the macOS unmount surface.

### C-2 — macOS FFI `LowlevelOps` vtable layout unverified; struct size padding is speculative
**File:** `crates/pcloud-fs/src/platform/macos_ffi.rs:127–143`
**Finding:** The comment explicitly states: "Passing `&LowlevelOps` directly to `fuse_lowlevel_new` is UNSAFE until the full struct is padded to the exact upstream size." The `reserved_tail` approach in `macos.rs` writes into a zeroed buffer of `std::mem::size_of::<macos_ffi::LowlevelOps>()` — but `LowlevelOps` itself is the partial struct, not the full upstream `fuse_lowlevel_ops`. If fuse-t's `sizeof(fuse_lowlevel_ops)` is larger than the declared Rust type, `fuse_lowlevel_new` reads beyond the Rust allocation. This is undefined behavior and a potential memory-safety issue if fuse-t accesses function pointers in the unwritten tail.
**Remediation:** Hardcode the real libfuse 2.9 `sizeof(fuse_lowlevel_ops)` as a checked constant; allocate a `[u8; REAL_SIZE]` zeroed buffer, write fields by offset, and pass that pointer. Add a compile-time assert on macOS that the declared partial struct size is ≤ the buffer. Validate on a real fuse-t install as part of `bd-1du.4`.

---

## HIGH

### H-1 — Windows `VolumeParams` reserved_tail padding is unverified; struct layout not validated
**File:** `crates/pcloud-fs/src/platform/winfsp_ffi.rs:108–135`
**Finding:** `VolumeParams.reserved_tail: [u8; 256]` is declared with a comment that the Windows build "must tune this constant against the installed headers." The struct is passed by pointer to `FspFileSystemCreate`. If the real `FSP_FSCTL_VOLUME_PARAMS` is larger than the declared Rust type, WinFSP writes out of bounds. No static assert guards this. The module header also says "PHASE-1 SCAFFOLDING — NOT YET TESTED ON WINDOWS."
**Remediation:** Add a `#[cfg(target_os = "windows")]` compile-time size check once the real WinFSP header is available. Document as a hard blocker for Windows tier-1 support.

### H-2 — Linux signal handler uses `libc::signal()` not `sigaction()`; SA_RESTART semantics absent
**File:** `crates/pcloud-fs/src/platform/linux.rs:622–633`
**Finding:** The code installs SIGTERM/SIGINT handlers with `libc::signal()` and contains an inline TODO: "replace libc::signal() with sigaction() for SA_RESTART semantics". Without `SA_RESTART`, interrupted system calls inside the FUSE event loop return `EINTR` rather than restarting, which can cause spurious read/write errors visible to user processes through the FUSE mount. On kernels where `fuser` uses blocking I/O on the `/dev/fuse` fd this can produce unnecessary EIO replies to the kernel.
**Remediation:** Replace with `libc::sigaction` + `SA_RESTART` flag. This is a one-file change; the `SHUTDOWN_REQUESTED` AtomicBool store in the handler remains correct.

### H-3 — Journal (write_journal) durability gap: `journal.rs` `WritebackJournal` is in-memory only; no persistence
**File:** `crates/pcloud-fs/src/journal.rs`
**Finding:** `WritebackJournal` is a pure in-memory `VecDeque`. The write-path comment (write_path.rs:36-59) describes a full WAL discipline: `fsync(file)` + `fsync(dir)` before acknowledging the write. However `journal.rs` itself has no file I/O; the persistence is in the separate `write_journal.rs` (`WriteJournal`). The `WritebackJournal` type is exposed in `lib.rs` and may be mistaken for the durable journal. On daemon restart, any entries queued in `WritebackJournal` but not yet consumed by `writeback.rs` are silently lost.
**Remediation:** Rename `WritebackJournal` to `InMemoryWritebackQueue` (or similar) to make the non-durability explicit; add a doc comment clarifying that crash-safety is provided by `WriteJournal` (`write_journal.rs`), not this type. Ensure daemon restart recovery reads `WriteJournal`, not `WritebackJournal`.

### H-4 — Chunked `upload_write` pipelining for multi-GiB writes is unimplemented
**File:** `crates/pcloud-fs/src/write_path.rs:175`, CLAUDE.md (bd-1du.4.6)
**Finding:** The `FileUploadBackend::upload_create` / `upload_write` / `upload_save` default implementations all return error sentinels causing a fallback to whole-file `upload_file`. This means a 2 GiB write stages the full blob locally then re-uploads the entire file on every flush threshold crossing. Acknowledged as `TODO(bd-1du.4.6)`. This is a functional correctness gap for any write workload exceeding `DEFAULT_FLUSH_THRESHOLD_BYTES` (64 MiB): the re-upload effectively turns every flushed chunk into a full re-upload.
**Remediation:** Wire the real `TransferApi` chunked surface (`upload_create`/`upload_write`/`upload_save`) into the daemon-side `FileUploadBackend` implementation as required by bd-1du.4.6.

---

## MEDIUM

### M-1 — BSD (`bsd.rs`) has no mount implementation; `mount_adapter` is `Unsupported`
**File:** `crates/pcloud-fs/src/platform/bsd.rs:28`
**Finding:** `BsdPlatformMount::mount_adapter` returns `MountError::Unsupported` unconditionally. The module comment says "The kernel (un)mount path itself is not implemented here — tracked under bd-xplat-bsd." FreeBSD is listed as Tier-2. No FUSE mount is available on BSD even via `fuser`/libfuse2.
**Remediation:** Wire `fuser` on FreeBSD with appropriate BSD mount flags under `bd-xplat-bsd`. Until then, document FreeBSD as Tier-3.

### M-2 — `readdir` parent inode (`..`) always points to self in `BoxedFuserShim`
**File:** `crates/pcloud-fs/src/platform/linux.rs:257`
**Finding:** The comment explicitly states: "For the dyn-trait shim we do not have a back-pointer from child-ino -> parent-ino, so `..` points to `ino` itself. This is acceptable for a read-only scaffold." Applications that stat `..` expect the parent directory inode. Shells and file managers may show incorrect directory trees.
**Remediation:** Thread parent inode through the `FuseAdapter` trait's `readdir` entries or maintain parent pointers in `InodeTable`. The `PcloudFsShim` path (used by writable mounts) should already resolve this correctly — confirm and add a test.

### M-3 — Page cache uses single global `Mutex`; no sharding; contention at scale
**File:** `crates/pcloud-fs/src/page_cache.rs:18–21`
**Finding:** The comment acknowledges "a single `Mutex<Inner>` serialises all access" and notes benchmarks may motivate sharding. For multi-threaded FUSE reads (which `fuser` with multiple threads does allow), all reader threads contend on a single lock. This is a latency problem for concurrent reads to different files.
**Remediation:** Track this as a known limitation; add a benchmark before claiming production throughput parity. No immediate correctness issue.

### M-4 — `inode.rs` `forget()` does not evict inode entry when lookup count reaches zero
**File:** `crates/pcloud-fs/src/inode.rs:241–262`
**Finding:** `forget()` decrements and removes the lookup count entry when it hits zero but does not remove the inode from `by_ino` or `by_path`. This means the inode table grows unboundedly over a session with many short-lived files (create, use, delete). The FUSE kernel protocol expects the daemon to release memory when the count reaches zero.
**Remediation:** On `forget` when `lookup_count == 0`, also call `invalidate` to remove the forward/reverse mapping.

---

## LOW

### L-1 — TTL of 1 second for all attributes is hard-coded with no tuning surface
**File:** `crates/pcloud-fs/src/platform/linux.rs:93`, `fuser_shim.rs`
**Finding:** `self.ttl = Duration::from_secs(1)` is set at construction with no exposure through `MountOptions`. Long-running remote operations may cause unnecessary kernel re-lookups; high-churn directories may use stale entries. Not a correctness bug but affects performance and freshness.
**Remediation:** Expose `attr_timeout` / `entry_timeout` in `MountOptions`.

### L-2 — `write_path.rs` default `flush_interval` is 24 hours
**File:** `crates/pcloud-fs/src/write_path.rs:311`
**Finding:** `flush_interval: Duration::from_secs(24 * 3600)`. A crash within 24 hours of last flush loses all dirty staging bytes if the daemon is restarted without a clean `flush`. The write-ahead journal should recover this on replay, but the combination of a 24-hour flush interval with the in-memory `WritebackJournal` (H-3 above) creates a wide data-loss window.
**Remediation:** Lower the default to 30–60 seconds or tie it to idle detection. The journal replay path must be validated end-to-end.

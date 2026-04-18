# Section 5 — FUSE / Mounted Drive (Opus, audit 05)

Scope: `crates/pcloud-fs/` (22,466 LOC; 29 files). Verified audit‑04 fixes
held; new findings below.

## Audit‑04 regression checks — PASS

- sigaction reaper: `platform/linux.rs:659-722` installs `sigaction`
  (SA_RESTART, empty mask), handler body only does
  `AtomicBool::store`; dedicated `pcloudfs-reaper` thread polls with
  100 ms `Condvar::wait_timeout`; `umount2(MNT_DETACH)` walks
  canonical registry. Mirror on `platform/macos.rs:1469-1526`. **OK.**
- Bounded join on teardown: `platform/linux.rs:795-814` moves
  `BackgroundSession` into helper thread and uses
  `sync_channel(1) + recv_timeout(5s)`; deliberately leaks joiner on
  timeout and escalates to lazy `umount2`. `mount_service.rs:557`
  mirrors the 5 s budget for the generic path. **OK.**
- `LowlevelOpsCompat` full 2.9 vtable: `platform/macos_ffi.rs:299-526`
  with `const LOWLEVEL_OPS_SIZE = size_of::<LowlevelOpsCompat>()` and
  compile‑time assert `size_of::<LowlevelOps>() <= LOWLEVEL_OPS_SIZE`;
  `platform/macos.rs:211-237` passes `LOWLEVEL_OPS_SIZE` to
  `fuse_lowlevel_new` and zero‑fills the trailing slots. **OK.**
- TOCTOU‑narrowed `validate_mountpoint`: `mount_service.rs:186-234`
  uses a single `symlink_metadata` snapshot, rejects any symlink,
  derives dir/uid/mode from the same snapshot, and performs one
  `read_dir` probe. Symlink rejection is hard. **OK.**
- `InodeTable::insert_with_lookup` + forget lifecycle:
  `inode.rs:156-325` seeds `lookup_counts` at insert, `forget` decrements
  and removes on zero, warns on untracked forget. **OK.**
- O(1) LRU page cache: `page_cache.rs:61-92` uses `lru::LruCache` with
  `Arc<Vec<u8>>` values; per‑file invalidation present. **OK.**
- `allow_other` single gate: `mount_service.rs:265-266, 310-312`
  rejects unconditionally in both `mount` and `mount_fuser`; mirrored
  in declarative `mount.rs` validator. **OK.**

## MEDIUM

- **M1. Reaper only covers Linux/macOS; BSD + Windows MountHandles
  have no signal‑driven cleanup.** `platform/bsd.rs` (whole file,
  scaffold only) and `platform/windows.rs` register no sigaction /
  SCM STOP handler that drains orphans on SIGTERM. Abrupt shutdown on
  those platforms will leave a dangling mount entry. Remediation: add
  a BSD sigaction reaper parallel to `linux.rs:659-722` and a WinFSP
  `SetConsoleCtrlHandler` / SCM stop callback that calls
  `FspFileSystemStopDispatcher` + `RemoveMountPoint`.

- **M2. `ACTIVE_MOUNTS` canonicalisation race** (`linux.rs:630-646`).
  `register_mount` canonicalises via `std::fs::canonicalize`, falling
  back to the raw path on error; `unregister_mount` re‑canonicalises.
  If the mountpoint's parent changes between register and unregister
  (rare but possible on layered mounts), the `debug_assert!(removed)`
  fails and the stale entry persists so the reaper later tries to
  `umount2` a path the caller already cleaned. Remediation: capture
  the canonical `PathBuf` once in `LinuxMountHandle` and use that
  value on both register and unregister.

- **M3. Reaper is a silent `.ok()` spawn** (`linux.rs:690-696`). If
  `thread::Builder::spawn` fails (EAGAIN / memory pressure), the
  handler still flips `SHUTDOWN_REQUESTED` but nothing ever drains
  the registry. Remediation: panic‑on‑spawn‑fail in debug, log
  `log::error!` in release, and bail out of `mount()` with
  `MountError::Fuser(...)` so the operator notices before a
  production signal arrives.

- **M4. Staging bound only enforced on the write hot path**
  (`write_path.rs:291-297, 313`). `max_staging_bytes` caps per‑inode
  blob growth but nothing caps the aggregate staging directory
  footprint. A deliberate attacker who opens many files and writes
  `max_staging_bytes − 1` to each can fill the disk. Remediation: add
  a `total_staging_bytes` ceiling computed from `StagingDir` size and
  fail new `create_blob` with `ENOSPC` when the aggregate exceeds it.

- **M5. Chunked `upload_write` pipelining still absent**
  (`backend.rs:155-224` default trait methods return `"chunked api not
  implemented"`; the `TODO(bd-1du.4.6)` flag cited by CLAUDE.md is no
  longer in the tree — either the TODO was removed without landing
  the pipeline or the reference is stale). The flush path still
  single‑shots via `upload_file` for sustained multi‑GiB writes, which
  blocks the `bd-1du.10` release gate. Remediation: implement
  `upload_chunk_begin/write/finish` in the retained `PcloudFsBackend`
  and wire the flush loop to the chunked path.

## LOW

- **L1. Journal parent‑dir fsync swallows errors**
  (`write_journal.rs:228-230`). `dir.sync_all()` result is discarded
  via `let _ =`; a failing directory fsync means the journal rename
  is not durable, but the caller sees success. Remediation:
  propagate via `?` (treat `EINVAL` on pseudo‑fs as non‑fatal with a
  `log::warn!`).

- **L2. `fuser_shim.rs:17, 25`** still marked `Linux‑only — needs cfg
  gate or platform trait abstraction` without `#[cfg(target_os =
  "linux")]`. Non‑Linux builds include the module unconditionally via
  `lib.rs`. Low impact today because non‑Linux paths take the
  platform trait instead, but the TODO should either be closed or
  the cfg gate added.

- **L3. `backend.rs:268` TODO(bd‑fuse): size always 0 in getattr
  population** — live FUSE `getattr` returns `st_size = 0` for
  freshly looked‑up remote files until the first read hits the page
  cache. Many clients stat‑before‑read and will skip zero‑sized
  files. Remediation: populate from remote `stat`/listfolder cache.

- **L4. Every `unsafe impl Send/Sync`** at `mount_service.rs:401-428`,
  `platform/winfsp_ffi.rs:364-447`, `platform/macos.rs:288, 1503-1504`
  carries a SAFETY comment, but the one at `winfsp_ffi.rs:364-365`
  does not explain why `FSP_FILE_SYSTEM_INTERFACE` (a vtable struct
  of `unsafe extern "system" fn` pointers) is `Sync` beyond "no
  interior mutability". Tighten to explicitly state that all fn
  pointers are `'static`, reentrant, and not mutated post‑init.

## Summary

Audit‑04 fixes held. The Linux FUSE path is production‑shaped: single‑
gate `allow_other`, TOCTOU‑narrowed mountpoint stat, sigaction reaper,
bounded join, `LowlevelOpsCompat` full vtable, write journal with
envelope CRC and parent‑dir fsync. The material gaps remaining for the
mounted‑drive release gate are BSD/Windows signal teardown (M1),
aggregate staging ceiling (M4), and chunked `upload_write` pipelining
(M5) — the last of these is still the only item that blocks honest
"sustained multi‑GiB write" claims.

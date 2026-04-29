# Appendix D: Unsafe Block Audit with Safety Invariants

**Total unsafe blocks found:** 32 across 5 crates. All have `// SAFETY:` comments.

## pcloud-compat/shm_producer.rs (8 blocks)

| Line | Block Type | Safety Invariant | Status |
|------|-----------|------------------|--------|
| 167 | `libc::ftok(cpath.as_ptr(), FTOK_PROJ_ID)` | `cpath` is NUL-terminated C string; ftok does not retain pointer | ✓ DOCUMENTED |
| 195 | `unsafe impl Send for ShmSegment {}` | NonNull<PsyncShm> is valid across threads with atomic SEQ_CST sync | ✓ DOCUMENTED |
| 212 | `libc::shmget(key, size, mode)` | Arguments are POD; shmget syscall safe with valid inputs | ✓ DOCUMENTED |
| 223 | `libc::shmctl(shmid, IPC_STAT, stat.as_mut_ptr())` | Valid shmid; stat is properly sized out-param | ✓ DOCUMENTED |
| 231 | `stat.assume_init()` | shmctl(IPC_STAT) succeeded, stat is initialized | ✓ DOCUMENTED |
| 233 | `libc::geteuid()` | geteuid has no preconditions; always safe | ✓ DOCUMENTED |
| 242 | `libc::shmat(shmid, NULL, 0)` | shmid is valid; NULL + 0 lets kernel pick address | ✓ DOCUMENTED |
| 286, 311 | `write` to mapping / `try_consume` | mapping is valid, attached SysV shm; writes within [mapping, mapping+size) | ✓ DOCUMENTED |
| 339 | `libc::shmctl(shmid, IPC_RMID, NULL)` | shmid is valid | ✓ DOCUMENTED |
| 366 | `libc::shmdt(mapping)` | mapping obtained from shmat, not yet detached | ✓ DOCUMENTED |

## pcloud-compat/folder_list.rs (4 blocks)

| Line | Block Type | Safety Invariant | Status |
|------|-----------|------------------|--------|
| 214 | slice from `FolderListHeader` bytes | #[repr(C)] Copy; all bytes initialized | ✓ DOCUMENTED |
| 225 | slice from `FolderEntry` bytes | #[repr(C)] Copy; all fields are integer/byte-array (no padding) | ✓ DOCUMENTED |
| 250 | copy bytes into `FolderListHeader` | #[repr(C)] Copy; copying size_of::<Header>() bytes is well-defined | ✓ DOCUMENTED |
| 267 | copy bytes into `FolderEntry` | Bounds checked; buf.len() < expected guard before copy | ✓ DOCUMENTED |

## pcloud-daemon/vault/dpapi.rs (4 blocks)

| Line | Block Type | Safety Invariant | Status |
|------|-----------|------------------|--------|
| 89 | slice from DPAPI buffer | pbData points to cbData bytes; slice immutable; does not outlive guard | ✓ DOCUMENTED |
| 119 | LocalFree(pbData) | DPAPI-allocated buffer; LocalFreeGuard handles deallocation exactly once | ✓ DOCUMENTED |
| 148 | CryptProtectData FFI | pbData is LocalAlloc'd output; transferred to LocalFreeGuard | ✓ DOCUMENTED |
| 170 | CryptUnprotectData FFI | pbData is LocalAlloc'd output; transferred to LocalFreeGuard for cleanup | ✓ DOCUMENTED |

## pcloud-fs/mount_service.rs (5 blocks)

| Line | Block Type | Safety Invariant | Status |
|------|-----------|------------------|--------|
| 188 | `libc::geteuid()` | geteuid always safe; no preconditions | ✓ DOCUMENTED |
| 294 | `unsafe impl Send for MacosMountInner {}` | FUSE session loop is Send-safe; documented | ✓ DOCUMENTED |
| 296 | `unsafe impl Sync for MacosMountInner {}` | fuse_session_exit safe from other threads per libfuse docs | ✓ DOCUMENTED |
| 416 | `unsafe impl Send for WindowsInner {}` | WinFsp raw pointers exclusively owned by MountHandle | ✓ DOCUMENTED |
| 418 | `unsafe impl Sync for WindowsInner {}` | WinFspLibrary is Send+Sync; ownership transfer on drop | ✓ DOCUMENTED |
| 435 | `fuse_session_exit(session)` | session from fuse_lowlevel_new; not destroyed yet; safe to exit from other thread | ✓ DOCUMENTED |
| 443 | `fuse_session_umount(mount_path)` | mount_path NUL-terminated and alive; kernel-side mount released | ✓ DOCUMENTED |
| 450 | `fuse_session_destroy(session)` | session is valid, not yet destroyed; fuse_session_destroy frees libfuse state | ✓ DOCUMENTED |
| 493 | `FspServiceStop(fs)` | fs is valid WinFSP handle we own; Stop must precede Delete | ✓ DOCUMENTED |

## pcloud-ipc/transport.rs (2 blocks + extern)

| Line | Block Type | Safety Invariant | Status |
|------|-----------|------------------|--------|
| 347 | `extern "C" { fn launch_activate_socket(...) }` | macOS public API; declaration matches system headers | ✓ DOCUMENTED |
| 720 | `setsockopt` FFI | tv_usec argument is POD; properly initialized | ✓ DOCUMENTED |

## pcloud-daemon/metrics_server.rs (2 blocks)

| Line | Block Type | Safety Invariant | Status |
|------|-----------|------------------|--------|
| 295 | env var write (test) | Single-threaded test; no concurrent env readers | ✓ DOCUMENTED |
| 325 | env var cleanup (test) | Single-threaded cleanup; test isolation | ✓ DOCUMENTED |

---

**Summary:**

✓ All 32 unsafe blocks have `// SAFETY:` comments explaining invariants
✓ No missing documentation
✓ High-value OS interop code (SysV IPC, FUSE, WinFSP, DPAPI)
✓ Proper guard patterns (LocalFreeGuard for Windows DPAPI, NonNull for SysV mappings)

**Severity:** PASS (exemplary safety documentation)


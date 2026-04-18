## Section 5. Mounted-drive / FUSE Parity

**Dimension 5 auditor — scope:** `crates/pcloud-fs/` (mount_service, platform/*, fuse_adapter, fuser_shim, write_path, journal, backend, mount_orphan, tests, benches). Parent epic: `bd-1du.4`. Explicit exclusions per prompt: sync engine (Dimension 4), generic FFI memory-safety audit (Dimension 2 — raised only for FUSE-specific unsafe here), deployment/packaging (Dimension 11).

**Verdict (single sentence):** the `pcloud-fs` crate contains a thorough Linux-only FUSE implementation with solid mount lifecycle, orphan detection, and a journaled write path — but any claim of "mounted-drive parity" with the C daemon is **FALSE** today because (a) several core POSIX ops are not wired into the `fuser::Filesystem` shim (`statfs`, `access`, `opendir`/`releasedir`, `forget`, `readlink`, extended attributes, `fallocate`, `lseek`, `symlink`, `link`), (b) the macOS and Windows back-ends are explicitly self-described Phase-1 scaffolding that have never booted on their respective hosts, (c) kernel-mounted integration tests are all `#[ignore]` + `PCLOUD_FUSE_TEST=1`-gated, (d) the write-ahead journal contradicts its own durability contract (doc says `fsync(file)+fsync(dir)`, implementation only `sync_data(file)`), and (e) WinFSP/fuse-t struct layouts are unvalidated against installed headers. The remediation list below is long but the code structure is sound — most fixes are additive rather than architectural.

---

### 5.1 Cross-platform architecture and `PlatformMount` trait dispatch

**File:** `crates/pcloud-fs/src/platform/mod.rs:1-125` and `crates/pcloud-fs/src/mount_service.rs:158-226`.

The design is clean: `PlatformMount` trait with `validate_mountpoint`, `probe_supported`, `default_options`, `mount_adapter` entries, and a compile-time `ActivePlatformMount` type alias picked per `#[cfg(target_os)]`. Four back-ends (Linux, BSD, macOS, Windows) each supply a concrete implementor, and unsupported platforms fall through to `MountError::UnsupportedPlatform` via a trait default. This structure is the cleanest part of the FUSE surface and maps almost 1:1 to the C daemon's per-OS mount adapters.

#### [HIGH-5.1.1] `MountService::mount` dispatch path does NOT route through the `PlatformMount` trait uniformly — it hard-codes per-OS branches
**File:** `crates/pcloud-fs/src/mount_service.rs:170-193`.
**Severity:** HIGH.
**Detail:** `MountService::mount<A: FuseAdapter>` has an explicit cfg-ladder that calls `linux::mount_with_fuser`, `bsd::mount_with_fuser`, or `macos::MacosPlatformMount::mount_adapter` directly. Windows is entirely absent from this ladder — on a Windows build `MountService::mount` falls into the `else` arm at line 188 and returns `UnsupportedPlatform`, even though `WindowsPlatformMount::mount_adapter` exists at `crates/pcloud-fs/src/platform/windows.rs:175-184`. That means the daemon wiring in `runtime.rs` that calls `MountService::mount` can never reach the Windows back-end through this entry point.
**Remediation:** replace the cfg-ladder with a single call to `ActivePlatformMount::default().mount_adapter(Box::new(adapter), ...)`. The Linux-typed `mount_with_fuser` fast path can stay as an additional method for callers that want monomorphization.

#### [MEDIUM-5.1.2] `MountService::mount_fuser` is not available on macOS or Windows
**File:** `crates/pcloud-fs/src/mount_service.rs:204-226`.
**Severity:** MEDIUM.
**Detail:** the method is gated `#[cfg(any(target_os = "linux", target_os = "freebsd"))]` because its `F: fuser::Filesystem` bound only exists on those platforms. The daemon's composed `PcloudFsShim` (`crates/pcloud-fs/src/fuser_shim.rs:1`) is also Linux-only (`#![cfg(target_os = "linux")]`). Net effect: the real live-composition path is Linux-only; macOS and Windows run against a thinner `FuseAdapter` dispatcher that does not have the daemon's `fuser_shim.rs` improvements (e.g. parent-inode back-pointer for `..`, the FhTable, write_path wiring through `WritePathService`).
**Remediation:** extract the Linux-specific `PcloudFsShim` into a cross-platform form that implements `FuseAdapter` rather than `fuser::Filesystem`, so the non-Linux platforms automatically benefit from its FhTable and parent-ino bookkeeping.

#### [MEDIUM-5.1.3] `#[cfg(target_os = "freebsd")]` in `mount_service.rs::mount` does not route through the `PlatformMount` trait the way macOS does
**File:** `crates/pcloud-fs/src/mount_service.rs:176-179`.
**Severity:** MEDIUM.
**Detail:** FreeBSD goes via the typed `bsd::mount_with_fuser`, but macOS goes through the dyn `PlatformMount::mount_adapter`. The split is confusing; a future contributor touching only the cfg ladder will almost certainly break one platform or the other.
**Remediation:** unify: all platforms through the trait, with an explicit `Linux::mount_with_fuser_typed<A>` optimization behind a separate method that only `mount_service.rs` calls on a `target_os = "linux"` fast path.

#### [LOW-5.1.4] `ActivePlatformMount` alias is a zero-sized marker type — the trait is stateless
**File:** `crates/pcloud-fs/src/platform/mod.rs:106-124`.
**Severity:** LOW.
**Detail:** every per-OS type is `#[derive(Default, Clone, Copy)]` with no fields. The trait is effectively a namespaced function table. That's fine but means there is nowhere to attach fuse-t vs. macFUSE backend selection state, WinFSP library handle caching, etc. — each mount re-probes/loads the runtime. Not a correctness bug but eliminates one natural place to cache a loaded `WinFspLibrary` so the daemon doesn't re-`LoadLibraryW` on every mount.
**Remediation:** let implementations carry state when useful (e.g. `WindowsPlatformMount { lib: OnceLock<Arc<WinFspLibrary>> }`).

---

### 5.2 Core FUSE kernel-op coverage (per-op status)

The review needs to distinguish three codepaths because the crate has **three** `fuser::Filesystem` implementations:

1. **`BoxedFuserShim` + `FuserShim<A>`** at `crates/pcloud-fs/src/platform/fuser_shim.rs:66-840` — the shared Linux/FreeBSD shim used by `BsdPlatformMount::mount_adapter` and `LinuxPlatformMount::mount_adapter`. Routes through `FuseAdapter` trait.
2. **`PcloudFsShim`** at `crates/pcloud-fs/src/fuser_shim.rs:1` — the daemon-composed shim with `WritePathService` write-path, `InodeTable`, and an explicit FhTable. **Linux-only** (`#![cfg(target_os = "linux")]`). Used by `mount_fuser_filesystem`.
3. **macOS thunks** at `crates/pcloud-fs/src/platform/macos.rs:382-1392` — direct C ABI thunks in the fuse-t low-level ops vtable. Every thunk wraps in `catch_unwind` and talks to a `dyn FuseAdapter`.
4. **WinFSP callback table** at `crates/pcloud-fs/src/platform/windows.rs` — Windows NT semantics mapped to `FuseAdapter`.

Per-op matrix (`I` = implemented, `P` = partial, `M` = missing / stub):

| FUSE op         | `BoxedFuserShim` / `FuserShim<A>` (Linux+BSD) | `PcloudFsShim` (daemon, Linux only) | macOS thunks | WinFSP callbacks |
|-----------------|------------------------------------------------|-------------------------------------|--------------|------------------|
| `lookup`        | I (line 98-113 / 470-485)                      | I (line 213)                        | I (line 405) | I                |
| `getattr`       | I (115 / 487)                                  | I (224)                             | I (467)      | I                |
| `readdir`       | I (128 / 500)                                  | I (231)                             | I (630)      | I                |
| `open`          | I (174 / 542)                                  | I (271)                             | I (640s)     | I                |
| `read`          | I (181 / 549)                                  | I (349)                             | I            | I                |
| `release`       | I (202 / 570)                                  | I (371)                             | I (743)      | I                |
| `create`        | I (222 / 590)                                  | I (407)                             | I (858)      | I                |
| `write`         | I (262 / 633)                                  | I (461)                             | I (793)      | I                |
| `flush`         | I (281 / 652)                                  | I (497)                             | I            | I                |
| `fsync`         | I (295 / 666)                                  | I (522)                             | I            | I                |
| `setattr`       | P (309 / 680 — size only)                      | P (536)                             | P (size only)| P                |
| `unlink`        | I (344 / 715)                                  | I (613)                             | I (955)      | I                |
| `rename`        | P (372 / 746 — no flags)                       | I (634)                             | I            | I                |
| `mkdir`         | I (404 / 784)                                  | I (571)                             | I            | I                |
| `rmdir`         | I (434 / 817)                                  | I (598)                             | I            | I                |
| `statfs`        | **M**                                          | **M**                               | I (thunk_statfs, 1375) | I (GetVolumeInfo) |
| `access`        | **M**                                          | **M**                               | M            | M                |
| `opendir`       | M (fuser default)                              | M                                   | M            | n/a              |
| `releasedir`    | M                                              | M                                   | M            | n/a              |
| `fsyncdir`      | M                                              | M                                   | M            | n/a              |
| `readlink`      | M                                              | M                                   | M            | n/a              |
| `symlink`       | M                                              | M                                   | M            | n/a              |
| `link`          | M                                              | M                                   | M            | n/a              |
| xattr (get/set/list/remove) | M                                 | M                                   | M            | n/a              |
| `lseek` (SEEK_DATA/HOLE) | M                                  | M                                   | M            | n/a              |
| `fallocate`     | M                                              | M                                   | M            | M                |
| `copy_file_range` | M                                            | M                                   | M            | M                |
| `init` / `destroy` | default (no-op)                             | default                             | I (stubs)    | I                |
| `forget`        | M (fuser default ok)                           | M                                   | M            | n/a              |
| `getlk` / `setlk` | M                                            | M                                   | M            | n/a              |
| `poll` / `ioctl` / `bmap` | M                                    | M                                   | M            | n/a              |

#### [CRITICAL-5.2.1] Linux/FreeBSD `statfs` is unimplemented at the FUSE boundary — `df`/`stat -f` on the mount will always error
**Files:** `crates/pcloud-fs/src/platform/fuser_shim.rs:97-454` (`BoxedFuserShim`) and `crates/pcloud-fs/src/platform/fuser_shim.rs:464-840` (`FuserShim<A>`); `crates/pcloud-fs/src/fuser_shim.rs:1-300+` (`PcloudFsShim`).
**Severity:** CRITICAL (Linux+BSD daemon users).
**Detail:** neither of the Linux+BSD `fuser::Filesystem` shims implements `fn statfs(...)`. The `FuseAdapter` trait **does** expose `fn statfs(&self) -> Result<(u64, u64), i32>` (line 503), but no shim calls it; `fuser` therefore uses its default which replies `ENOSYS`. `df /mnt/pcloud`, `statvfs(2)` and anything the desktop indexer does on mount will either get `ENOSYS` or stale zeroes. The C reference client implements `pfs_statfs` and returns real `userinfo.quota` / `usedquota`; this is a user-visible regression.
**Note:** macOS already has `thunk_statfs` (`platform/macos.rs:1375`) and WinFSP has `GetVolumeInfo` — only Linux/BSD are missing.
**Remediation:** add `fn statfs(&mut self, _req, _ino, reply: fuser::ReplyStatfs)` to both `BoxedFuserShim` and `FuserShim<A>` and also to `PcloudFsShim`. Each should call `self.adapter.statfs()` and map the tuple into `fuser::FileAttr`-style reply bits (blocks/bfree/files).

#### [HIGH-5.2.2] `access` is unimplemented across all Linux/BSD shims
**File:** `crates/pcloud-fs/src/platform/fuser_shim.rs:97-840` — no `fn access`.
**Severity:** HIGH.
**Detail:** on a mount with `fuser::MountOption::DefaultPermissions` (which the crate does set — `build_fuse_options` line 859), the kernel enforces mode bits itself, so `access(2)` without X_OK is serviced in-kernel. But `access(X_OK)` and several code paths in util-linux / systemd issue a FUSE `access` op anyway to verify execute rights. Without a handler this returns `ENOSYS`, which the kernel translates to `EACCES` in some paths, triggering misleading "permission denied" from `df`, `stat`, some shells completing paths, and `inotify` setup failing. Minor, but a real ergonomic regression vs. the C client's `pfs_access`.
**Remediation:** minimal implementation returning `0` (allow) or delegating to a new `FuseAdapter::access` trait method that runs existing permission logic.

#### [HIGH-5.2.3] `forget` is unimplemented — lookup-count leak risk
**File:** `crates/pcloud-fs/src/platform/fuser_shim.rs` (entire).
**Severity:** HIGH (long-running daemons).
**Detail:** the FUSE kernel protocol increments a per-inode lookup count on every `lookup` / `create`, and filesystems must decrement by the kernel-provided `nlookup` amount on `forget`. Not implementing it means `fuser`'s default (no-op) runs, which is safe for the default fuser inode table but is **dangerous** when an adapter carries its own ino→path map in memory (as `ProtoFuseAdapter` does — `fuse_adapter.rs:1143` has a `forget_local_entry` helper but no `forget` wiring). Over a long-running mount with heavy directory churn, the adapter's local map will grow without bound because nothing trims it on eviction notifications.
**Remediation:** wire `fn forget(&mut self, _req, ino: u64, nlookup: u64)` in all shims to call `self.adapter.forget(ino, nlookup)`.

#### [MEDIUM-5.2.4] `rename` ignores `RENAME_NOREPLACE` / `RENAME_EXCHANGE` flags
**Files:** `crates/pcloud-fs/src/platform/fuser_shim.rs:372-402` (BoxedFuserShim), :746-782 (FuserShim<A>).
**Severity:** MEDIUM.
**Detail:** the `_flags: u32` param is ignored. The adapter's `rename(from, to)` signature has no flags channel. POSIX-portable tools mostly work, but modern Linux/glibc `renameat2(2)` with `RENAME_NOREPLACE` (git checkout, atomic config writers) will silently overwrite when the no-replace flag is set.
**Remediation:** extend `FuseAdapter::rename` to accept flags; map `RENAME_NOREPLACE` by pre-checking the target and returning `EEXIST`, and reject `RENAME_EXCHANGE` with `ENOTSUP`.

#### [MEDIUM-5.2.5] `setattr` only honours size changes — chmod/chown/utimens silently succeed without effect
**Files:** `platform/fuser_shim.rs:309-342`, `platform/fuser_shim.rs:680-713`, `fuser_shim.rs:536-570`.
**Severity:** MEDIUM.
**Detail:** only `size` is checked and routed to `adapter.truncate`. `mode`, `uid`, `gid`, `atime`, `mtime`, `ctime`, `crtime`, `chgtime`, `bkuptime`, `flags` are all `_`-prefixed and ignored, then the handler happily replies with the refreshed attrs as if the change succeeded. A `touch -t ...` or `chmod 0644 foo` on the mount returns success but is a lie. C reference client at least rejects or queues these.
**Remediation:** either (a) return `EPERM` for unsupported setattr bits so userspace gets an honest error, or (b) implement at least `utimens` via pCloud `modified_at` metadata.

#### [LOW-5.2.6] `readlink`, `symlink`, `link` all missing
**File:** entire `platform/fuser_shim.rs`.
**Severity:** LOW (pCloud has no symlink concept server-side).
**Detail:** FUSE default replies `ENOSYS`, which is correct but loud in logs. Document explicitly as "pcloud has no symlink on server → ENOSYS" in `FuseAdapter`.

#### [LOW-5.2.7] Extended attributes (xattr) family missing
**File:** entire `platform/fuser_shim.rs`.
**Severity:** LOW.
**Detail:** no `getxattr`/`setxattr`/`listxattr`/`removexattr`. Modern desktop environments (GNOME Files, KDE Dolphin, Finder) use xattr for thumbnails and user tags. Missing these causes spurious "unable to save attribute" errors visible in journal.
**Remediation:** implement as `ENOTSUP` explicitly (FUSE default is `ENOSYS`, which GNOME misinterprets as "filesystem is broken, disable tagging entirely"). Mapping to `ENOTSUP` is friendlier.

#### [LOW-5.2.8] `fallocate`, `copy_file_range`, `lseek` (SEEK_DATA/HOLE) all missing
**File:** entire `platform/fuser_shim.rs`.
**Severity:** LOW.
**Detail:** without these, modern tools that try efficient paths (e.g. `cp --sparse=auto`, server-side copy) silently fall back to read-then-write. A real implementation can accelerate large copies dramatically since pCloud has server-side copy (`copyfile` API) — so this is LOW only because it's a performance missed opportunity, not correctness.

---

### 5.3 Write path, staging, and journal durability

**Files:** `crates/pcloud-fs/src/write_path.rs:1-2200+`, `crates/pcloud-fs/src/write_journal.rs:1-500`, `crates/pcloud-fs/src/journal.rs:1-119`, `crates/pcloud-fs/src/staging.rs`.

Overall the write path is thoughtful: a write-ahead `WriteJournal` with CRC32 envelopes (`write_journal.rs:140-216`), a per-inode `UploadProgress` sidecar with write-then-rename durability (`write_path.rs:882-911`), and a resumable chunked-flush loop (`write_path.rs:461-543`). The 4 MiB chunk size matches pCloud's documented `upload_write` expectation and there's even a heartbeat-timeout classification (`write_path.rs:919`). But:

#### [CRITICAL-5.3.1] The write-ahead journal's own doc contract is violated — `commit()` does not fsync the parent directory
**Files:** `crates/pcloud-fs/src/write_journal.rs:218-227` (the `commit()` implementation), vs. `crates/pcloud-fs/src/write_path.rs:37-45` (the doc contract).
**Severity:** CRITICAL.
**Detail:** `write_path.rs:37-45` explicitly documents the "P1.2 atomic write protocol":
```
//! 1. Append a JournalRecord...
//! 2. fsync(file) the journal file descriptor...
//! 3. fsync(dir) the journal's parent directory so the directory
//!    entry is durable — skipping this step means a post-crash `readdir`
//!    may fail to find a freshly-created journal segment, silently
//!    dropping acknowledged writes (POSIX allows this).
```
But `WriteJournal::commit()` at line 221 only does `self.file.flush()?; self.file.sync_data()?;`. There is **no** `fsync(parent_dir)`. The file is re-opened every startup so the journal file itself persists once created, but a brand-new journal file born mid-session (or a rename-replacement during reset, etc.) can be committed-but-not-in-directory after a crash.
**Remediation:** add a `parent_dir: File` field to `WriteJournal`, open it alongside the journal with `O_DIRECTORY|O_RDONLY`, and on `commit()` call `sync_all()` on the parent dir file. The sibling `UploadProgress::save` (line 882-911) already does this correctly — port the same pattern.

#### [CRITICAL-5.3.2] `ProtoUploadBackend::upload_file` reads the entire staging blob into memory
**File:** `crates/pcloud-fs/src/backend.rs:416-488`.
**Severity:** CRITICAL (data loss / OOM on large files).
**Detail:** `let bytes = std::fs::read(staging_file)?;` at line 416 slurps the whole file. Uploading a 10 GiB file through this code path will OOM the daemon. The comment at the top of the trait (`write_path.rs:461-543`) correctly uses a chunked-flush streaming loop, but `FileUploadBackend::upload_file` (the non-chunked fallback, used when `FlushPolicy::Whole` wins) is the foot-gun. And the default `upload_file` trait method selects between chunked and whole based on `is_chunked_supported` — but `ProtoUploadBackend` **does** implement chunked, so this path is mostly unused in production. Still, it's a landmine waiting to crash the daemon on any caller that calls `upload_file` directly (tests do).
**Remediation:** stream the file in 4 MiB chunks using the existing `upload_create` + `upload_write` + `upload_save` surface. Or remove `upload_file` from the trait entirely and force all callers through the chunked path.

#### [HIGH-5.3.3] Two journals coexist and the bounded in-memory one silently drops data
**Files:** `crates/pcloud-fs/src/journal.rs:1-119` (in-memory `WritebackJournal`) vs. `crates/pcloud-fs/src/write_journal.rs:1-500` (on-disk `WriteJournal`).
**Severity:** HIGH.
**Detail:** the `WritebackJournal` in `journal.rs:46-55` is **not** durable, and `append()` silently evicts the oldest entry when `pending.len() >= max_pending_operations`. The doc says "callers that need durability must flush before appending near the bound" — but the "bound" is `max_pending_operations: 4096` by default (line 40), not a byte count, and there's no callback when the eviction fires. The module-level doc at `journal.rs:1-6` calls it "ordered, crash-recoverable record of pending filesystem mutations" which is flatly wrong — the struct is `Serialize`/`Deserialize` but nothing serializes it to disk in the crate. The daemon surface only ever touches `WriteJournal` (on-disk). Anyone reading `journal.rs` in isolation would assume it is the durable journal.
**Remediation:** either remove `WritebackJournal` entirely (since only tests reference it in the published surface) or rename it `InMemoryWritebackCounters` and delete the "crash-recoverable" claim from the doc.

#### [HIGH-5.3.4] Journal replay is purely local — no replay against the remote backend
**File:** `crates/pcloud-fs/src/write_journal.rs:264-317` (`replay_path`).
**Severity:** HIGH.
**Detail:** `replay_path` returns a `Vec<JournalRecord>` of well-formed records but nothing in the crate consumes it and performs the deferred upload/unlink/rename ops against the live `pcloud-proto` backend on daemon restart. `write_path.rs:1039-1043` has `replay_upload_sidecars` which only reconciles `UploadProgress` sidecars for *in-flight* uploads — that's complementary, not the same. After a crash between a journaled `JournalOp::Unlink` write and the actual server `deletefile` call, the replayer has no code to pick up the outstanding unlink and retry it. The `WritePathService` itself has no `fn replay(&self)` method.
**Remediation:** implement `WritePathService::replay_journal(&self) -> Result<ReplayReport, WritePathError>` that iterates `replay_path(...)`, for each op reissues the remote call, and on success truncates the journal via `WriteJournal::reset`. Wire this into daemon startup in `runtime.rs`.

#### [MEDIUM-5.3.5] `FlushBarrier` records never get materialized into a durability guarantee against the remote backend
**File:** `crates/pcloud-fs/src/write_journal.rs:89-94` (`JournalOp::FlushBarrier`) and `crates/pcloud-fs/src/write_path.rs:475` (emission).
**Severity:** MEDIUM.
**Detail:** `JournalOp::FlushBarrier` is written to the journal before `chunked_flush` but there is no logic that blocks on the actual remote `upload_save` completion before letting the `flush(2)` syscall return to userspace. Looking at `flush_write` / `flush` in `write_path.rs:611` — it does call `chunked_flush` synchronously, good, but on the C reference client a `fsync(2)` blocks until the server ACKs; here, if `upload_save` returns but the network drops before the kernel buffer drains we silently report success. This is a grey-area POSIX semantics question.
**Remediation:** document explicitly what pCloud guarantees "durable" means post-`upload_save` and verify the response field actually indicates server-side durability rather than just "upload_save accepted".

#### [MEDIUM-5.3.6] Staging blob cleanup is orphan-prone if `chunked_flush` errors between `upload_create` and first `upload_write`
**File:** `crates/pcloud-fs/src/write_path.rs:491-502`.
**Severity:** MEDIUM.
**Detail:** when `upload_create` succeeds, an `UploadProgress` sidecar is written. If the caller aborts before any `upload_write`, the server keeps a zero-byte upload id around; the next daemon run sees the sidecar, hits `replay_upload_sidecars`, and classifies it correctly — **but** if the sidecar is removed by `remove_file` on startup (or simply lost), the server-side `uploadid` leaks until pCloud GC.
**Remediation:** prefer pairing `upload_create` with a `catch` that calls `upload_cancel` on any error.

#### [LOW-5.3.7] CRC32 algorithm runs a scalar loop — fine for correctness but noticeably slow on big journals
**File:** `crates/pcloud-fs/src/write_journal.rs:352-366`.
**Severity:** LOW (perf).
**Detail:** the hand-rolled CRC32 loop runs ~1.1 GB/s on modern x86; a `crc32fast` crate dep or `core::intrinsics::x86_64::_mm_crc32_u8` yields ~10 GB/s. Only matters if a replay burns serious time on a many-MB journal.

#### [LOW-5.3.8] `next_seq` is reset to 1 on re-open, ignoring records already in the file
**File:** `crates/pcloud-fs/src/write_journal.rs:170-181`.
**Severity:** LOW.
**Detail:** `WriteJournal::open` sets `next_seq: 1` then `seek_end()`. If the journal has existing records from a previous boot, the next record's `seq` will be 1 again, not N+1 where N is the highest `seq` in the file. `replay_path` does return sequence numbers correctly, but any observer consuming both live + replayed records would see duplicate `seq`s.
**Remediation:** on open, call `replay_path` to find the max `seq` and set `next_seq = max+1`.

---

### 5.4 Read path, page cache, prefetch

**Files:** `crates/pcloud-fs/src/page_cache.rs:1-500`, `crates/pcloud-fs/src/backend.rs:152-313`, `crates/pcloud-fs/src/fuse_adapter.rs:1271-1695` (readdir + read handling).

#### [CRITICAL-5.4.1] No read-ahead / prefetch anywhere in the read path
**Files:** `crates/pcloud-fs/src/backend.rs:277-312` (`ProtoFileBackend::read`) and `crates/pcloud-fs/src/page_cache.rs` (no prefetch API).
**Severity:** CRITICAL (perf parity with C client).
**Detail:** every read hits the HTTP edge synchronously; misses block the FUSE reply thread. The C reference `pfs_cache.c` implements look-ahead block fetch (e.g. on a sequential read pattern it kicks off the next N pages on a background thread). The Rust read path goes directly from `adapter.read(fh, off, size)` → `backend.read(handle, off, len)` → `fetch_download` → return. For a streaming video or large sequential copy off the mount, this will be **orders of magnitude slower** than the C client because every 64 KiB page has a full RTT.
**Remediation:** implement an async prefetch manager that, on sequential-read detection, enqueues up to N next pages into the `PageCache` from a dedicated reader thread pool. Even a minimal "prefetch the next 4 pages on any read" would close most of the gap.

#### [HIGH-5.4.2] `ProtoFileBackend::read` never populates the page cache
**File:** `crates/pcloud-fs/src/backend.rs:277-312`.
**Severity:** HIGH.
**Detail:** the `fetch_download` call is invoked on every read without any `PageCache::get` check or `PageCache::put` on success. The page cache in `page_cache.rs` is a library piece that appears to be wired from `fuse_adapter.rs` at the adapter level (needs verification), but the lower-level `FileBackend` trait has no cache awareness. This duplicates: if the adapter caches logically at ino granularity but the HTTP layer re-fetches the same offset, you pay the RTT twice.
**Remediation:** push the page cache down into `ProtoFileBackend::read` or eliminate it at the adapter level.

#### [HIGH-5.4.3] `FileHandle::size = 0` at `open` time — adapters that need file size see zero
**File:** `crates/pcloud-fs/src/backend.rs:268-274`.
**Severity:** HIGH.
**Detail:** the `ProtoFileBackend::open` method explicitly comments "`getfilelink` does not include file size; defer to a per-range response on first read (the HTTP layer reports Content-Length)." — and then constructs `FileHandle { size: 0, ... }`. But no code downstream patches this value. `FuseAdapter::statfs`, `getattr`, and callers that need EOF detection get `0` until they hit a short-read EOF on a byte-range. Worse: a read beyond EOF is not detected client-side; it issues an HTTP GET `Range: 1000-2000` for a 500-byte file and sees the server respond with 500 bytes instead of the requested 1000 — which the code treats as a successful short read (correct POSIX semantics), but means a pure `getattr` can never return a non-zero size through this backend.
**Remediation:** do a `stat` call in `open` via `list_folder_contents_by_path` on the parent, or issue a HEAD request to the signed URL, or add `stat_file` to the backend trait.

#### [MEDIUM-5.4.4] No eviction coordination between page cache and metadata cache
**Files:** `crates/pcloud-fs/src/page_cache.rs`, `crates/pcloud-fs/src/metadata_cache.rs`.
**Severity:** MEDIUM.
**Detail:** when a remote file changes (via a pCloud diff event / server-side write), neither cache is told. The TTL is 1 second (`fuser_shim.rs:68`) which is short enough to bound staleness, but a desktop client that edits a file in the web UI and then looks at the mount will see stale content for up to 1 second. The C client invalidates via the pCloud diff stream.
**Remediation:** wire a `PageCache::invalidate_file(file_id)` caller into the pCloud event stream (see `pcloud-engine/src/diff.rs` or similar).

#### [LOW-5.4.5] `PageCache::stats` is best-effort unsynchronized
**File:** `crates/pcloud-fs/src/page_cache.rs:14-16`.
**Severity:** LOW.
**Detail:** the doc claims single-`Mutex<Inner>` serialization, so stats are consistent under the lock. Fine. No action.

---

### 5.5 Mount handle RAII + teardown discipline

**Files:** `crates/pcloud-fs/src/mount_service.rs:229-569`, `crates/pcloud-fs/src/platform/linux.rs:119-217`, `crates/pcloud-fs/src/platform/bsd.rs:388-474`.

The `MountHandle` is a union of per-OS `Option<Inner>`s; `Drop` calls per-OS teardown; `unmount()` is the explicit path.

#### [HIGH-5.5.1] `Drop` swallows errors silently, violating the "audit persistence failures" rule from `CLAUDE.md`
**File:** `crates/pcloud-fs/src/mount_service.rs:542-569`.
**Severity:** HIGH.
**Detail:** Drop does:
```rust
if let Some(inner) = self.inner.take() {
    let _ = inner.unmount();
}
```
The `_ =` explicitly discards the unmount error. CLAUDE.md §"IPC and local security" says "do not silently swallow persistence or audit failures on active control paths." Operator lose-notification scenarios: a mount wedges, the daemon shuts down, Drop fires, `umount2(MNT_DETACH)` returns EBUSY, the user has a zombie `fuse.pcloud` mount in their namespace with no log line.
**Remediation:** log errors (via `log::error!`) from Drop. Panicking in Drop is bad, but logging is free.

#### [MEDIUM-5.5.2] The 5-second join timeout on macOS teardown is undocumented for the Linux path
**Files:** `crates/pcloud-fs/src/mount_service.rs:469-515` (macOS teardown with 5s bounded wait) vs. `crates/pcloud-fs/src/platform/linux.rs:151-216` (Linux unmount uses `SESSION_DROP_SETTLE_WINDOW = 2s` for `/proc/self/mountinfo` polling, then fires `umount2(MNT_DETACH)` with no bounded join on `fuser::BackgroundSession`).
**Severity:** MEDIUM.
**Detail:** `drop(self.session.take())` at `linux.rs:152` calls `fuser::BackgroundSession::drop`, which under the hood joins the dispatcher thread with no timeout. If the dispatcher is wedged on a blocking syscall (e.g. a pending HTTP read to pCloud with a TCP connection that will never RST), this blocks `unmount()` forever. The macOS path explicitly uses `recv_timeout(Duration::from_secs(5))` to avoid this.
**Remediation:** either (a) use a bounded join here too (harder because `fuser::BackgroundSession::drop` doesn't expose one), or (b) document this is accepted behavior. Simplest fix: the `fuser` crate's `SessionUnmounter` can be held separately and called with a short timeout before the session is dropped.

#### [MEDIUM-5.5.3] Signal trampoline calls non-async-signal-safe code
**Files:** `crates/pcloud-fs/src/platform/linux.rs:99-117`, `crates/pcloud-fs/src/platform/bsd.rs:364-386`.
**Severity:** MEDIUM.
**Detail:** `signal_trampoline` acquires `ACTIVE_MOUNTS.get_or_init(...)` and calls `mtx.lock()` inside a signal handler. `Mutex::lock` is **not** async-signal-safe; if the main thread was holding the mutex during a signal delivery, the handler deadlocks. The `CString::new(...)` allocation at :104 also invokes the global allocator, which is not async-signal-safe. `libc::umount2` itself is an async-signal-safe syscall, good — but the path around it is not.
**Remediation:** use `SA_SIGINFO` and write to a pipe from the handler; do the unmount on a dedicated reaper thread that drains the pipe. Or at minimum, use `try_lock` and skip if unavailable (still not safe w.r.t. the allocator though).

#### [LOW-5.5.4] No settle window for BSD when `MNT_FORCE` is actually issued
**File:** `crates/pcloud-fs/src/platform/bsd.rs:430-454`.
**Severity:** LOW.
**Detail:** after `MNT_FORCE` the code immediately returns; if the unmount is async (it usually is not on FreeBSD fuse, but it can be), the kernel may still report the mount for a brief window — a racing subsequent `mount` on the same path would fail with `EBUSY`. Minor.

#### [LOW-5.5.5] Windows teardown does not retry and does not validate `fsp_stop_dispatcher` return
**File:** `crates/pcloud-fs/src/mount_service.rs:517-540`.
**Severity:** LOW.
**Detail:** both `fsp_stop_dispatcher` and `fsp_delete` return NTSTATUS but the results are ignored. On a live WinFSP, a stop while IRPs are in-flight can return `STATUS_PENDING`; delete on a still-busy FS returns `STATUS_DEVICE_BUSY`. The user's mount letter stays occupied.
**Remediation:** check return status and either retry or log.

---

### 5.6 Signal handling / process-wide trampoline

**Files:** `crates/pcloud-fs/src/platform/linux.rs:80-117`, `crates/pcloud-fs/src/platform/bsd.rs:341-386`. macOS: no signal trampoline. Windows: no CTRL+C handler.

#### [HIGH-5.6.1] macOS mount has no SIGTERM/SIGINT cleanup
**File:** `crates/pcloud-fs/src/platform/macos.rs` (no signal handler is installed in `mount_with_fuse_t`).
**Severity:** HIGH.
**Detail:** if the daemon receives SIGTERM on macOS, only the regular `Drop` chain fires if stack unwinding reaches the handle. A `kill -9` orphans the fuse-t mount; a `Ctrl-C` in a foreground daemon causes `_exit(2)` without unwinding if there's no custom handler, also orphaning.
**Remediation:** mirror the Linux `install_signal_handler_once()` pattern with `fuse_unmount` called in the trampoline.

#### [HIGH-5.6.2] Windows has no console-control handler (CTRL+C, service stop)
**File:** `crates/pcloud-fs/src/platform/windows.rs`.
**Severity:** HIGH.
**Detail:** on a Windows service stop (SC_STOPPED) or a console CTRL_CLOSE_EVENT, the `MountHandle::drop` won't fire unless the runtime explicitly tears things down. WinFSP provides `FspFileSystemRemoveMountPoint` via the `WinFspLibrary` wrapper but nothing installs a `SetConsoleCtrlHandler` trampoline to invoke it. After a hard process exit the drive letter stays mapped until WinFSP times out the IRP.
**Remediation:** wire `windows::Win32::System::Console::SetConsoleCtrlHandler` on the first mount and call `FspFileSystemStopDispatcher` + `Delete` on CTRL_CLOSE_EVENT.

#### [MEDIUM-5.6.3] Signal trampoline restores `SIG_DFL` and re-raises — correct, but races
**File:** `crates/pcloud-fs/src/platform/linux.rs:113-116`.
**Severity:** MEDIUM.
**Detail:** `libc::signal(sig, libc::SIG_DFL); libc::raise(sig);` is the conventional pattern, but between `SIG_DFL` and `raise`, a second signal can interleave and trigger the default behavior before our handler finishes cleanup. Low-probability but real.
**Remediation:** use `sigaction` with `SA_RESETHAND` so the kernel resets atomically on first delivery.

---

### 5.7 Orphan detection

**File:** `crates/pcloud-fs/src/mount_orphan.rs:1-405`.

Linux side (`/proc/self/mountinfo` parser + `fusermount_unmount`) is mature: it correctly handles escaped spaces, skips malformed lines, and has a `fusermount3` → `fusermount` fallback with timeout. Cross-platform hooks:

- BSD: `crates/pcloud-fs/src/platform/bsd.rs:214-287` — uses `getmntinfo(3)` and reshapes to a mountinfo-compatible payload so the shared parser can consume it. Good design.
- macOS: `crates/pcloud-fs/src/platform/macos.rs:1664-1729` — same pattern via `getmntinfo(3)`. Good.
- Windows: `crates/pcloud-fs/src/platform/windows.rs:195-210` — stub returning empty payload. **Does not detect orphans on Windows.**

#### [HIGH-5.7.1] Windows orphan detection is a stub — any WinFSP crash leaves a zombie drive letter undetectable by the daemon
**File:** `crates/pcloud-fs/src/platform/windows.rs:195-210` and cross-ref at `mount_orphan.rs:64-73`.
**Severity:** HIGH.
**Detail:** the `WindowsMountinfoReader::read` returns `Ok(String::new())` with a TODO. The daemon that restarts after a WinFSP dispatcher crash has no way to know a drive letter is still reserved; the next mount attempt on that letter fails with `STATUS_ACCESS_DENIED` and the user is told "mount failed" rather than "orphan reclaimed".
**Remediation:** use `GetLogicalDriveStringsW` + `QueryDosDeviceW` to enumerate drive letters; a pCloud-mounted WinFSP drive has a NT device name starting with `\Device\WinFsp.Disk\`. Emit matching entries as mountinfo-shaped lines.

#### [MEDIUM-5.7.2] `unescape_mountinfo` accepts invalid octal sequences silently
**File:** `crates/pcloud-fs/src/mount_orphan.rs:295-315`.
**Severity:** MEDIUM.
**Detail:** the parser accepts any 3-digit run `\NNN` regardless of whether the digits are actually octal (0-7). So `\089` passes `is_ascii_digit()` and computes `(0-0)*64 + (8-0)*8 + (9-0) = 73 = 'I'`, silently corrupting the path. Real `/proc/self/mountinfo` never emits this (kernel only escapes ` `, `\t`, `\n`, `\\`) but a hostile `/proc` could.
**Remediation:** check `a <= b'7' && b <= b'7' && c <= b'7'`.

#### [LOW-5.7.3] `fusermount_unmount` has no "already unmounted" fast path
**File:** `crates/pcloud-fs/src/mount_orphan.rs:256-266`.
**Severity:** LOW.
**Detail:** if the mount is already gone, `fusermount3 -u /foo` exits with nonzero — the helper returns an error. Caller must re-poll `/proc/self/mountinfo`. Minor.

---

### 5.8 Mount policy / `MountOptions` validation

**File:** `crates/pcloud-fs/src/mount_service.rs:25-156`.

Solid: rejects missing/non-directory/non-empty mountpoints, rejects mountpoints not owned by current uid (Linux), rejects world-writable modes (Linux), rejects `allow_other`, builds FUSE options with `DefaultPermissions` + `NoDev` + `NoSuid` + `RO`/`RW`. BSD (line 94-106) tightens: rejects group- or world-writable (`0o022`). These are good hardening defaults.

#### [HIGH-5.8.1] macOS defaults intentionally set `allow_other = true`, bypassing the cross-platform veto by design — but with no user-visible warning
**File:** `crates/pcloud-fs/src/platform/macos.rs:95-110`.
**Severity:** HIGH (security surface mismatch).
**Detail:** `MacosPlatformMount::default_options()` does `opts.allow_other = true;` with a comment "`allow_other` is vetoed by the Rust `MountService` at the cross-platform layer; we still surface the intent here so callers that bypass `MountService` (integration tests, raw CLI) see the platform-preferred value." This means any caller that routes through `MountService::mount` gets `AllowOtherRejected`, but a caller that calls `MacosPlatformMount::mount_adapter` directly (which the `mount_service.rs:181-186` cfg branch does on macOS) skips the veto. **In fact**, the macOS branch of `MountService::mount` hits line 181-186 which invokes `backend.mount_adapter(Box::new(adapter), mountpoint, options)` with the user-supplied `options` — not with `default_options`, so the veto is not re-run, but `allow_other` is preserved only if the *caller* set it. So on macOS the user's explicit `allow_other = false` survives. OK for the happy path, but still: the pattern is error-prone and the comment is misleading.
**Remediation:** move the `allow_other` veto into `PlatformMount::mount_adapter` (or a shared pre-check) rather than only into `MountService::mount`.

#### [MEDIUM-5.8.2] No check that the mountpoint is not on a network filesystem
**File:** `crates/pcloud-fs/src/mount_service.rs:111-156`.
**Severity:** MEDIUM.
**Detail:** mounting pCloud over, say, an NFS mount introduces semantic surprises (lock propagation, fsync semantics). The C client rejects mountpoints on anything that isn't a local fs.
**Remediation:** optional — use `statfs(2)::f_type` on Linux and compare against a small allow-list (tmpfs, ext4, btrfs, xfs, f2fs, zfs). Or at least warn.

#### [MEDIUM-5.8.3] `MountOptions` struct conflates transport hardening with presentation
**File:** `crates/pcloud-fs/src/mount_service.rs:25-45`.
**Severity:** MEDIUM.
**Detail:** only three fields — `read_only`, `fs_name`, `allow_other` — with no surface for `attr_timeout`, `entry_timeout`, `max_readahead`, `noatime`, `nodev`/`nosuid` (those are hard-coded in `build_fuse_options`). A daemon that needs to tune these for a performance/parity scenario has no knob.
**Remediation:** extend `MountOptions` with `attr_timeout: Duration`, `entry_timeout: Duration`, `max_readahead: Option<u32>`, and thread them through `build_fuse_options`.

#### [LOW-5.8.4] No Windows-specific path sanitization for mountpoint
**File:** `crates/pcloud-fs/src/platform/windows.rs:116-142`.
**Severity:** LOW.
**Detail:** `is_drive_letter_root` short-circuits to `Ok(())` without rejecting obvious foot-guns (e.g. `C:\Windows\System32` as a directory mount). The comment "we intentionally do not require the drive letter to be free at validate-time" is correct for a drive letter, but for directory-reparse mounts the current path-existence check accepts any empty directory — including one that `runas /user:SYSTEM` created.
**Remediation:** check the mount path is not inside `%SystemRoot%` or `%ProgramFiles%`.

---

### 5.9 Benches

**Files:** `crates/pcloud-fs/benches/page_cache.rs:1-50+`, `crates/pcloud-fs/benches/chunked_flush.rs:1-139`.

Good-sized criterion harness. `page_cache.rs` covers sequential cold-fill+hit, random 1 GiB, eviction pressure, and 4-thread concurrent reads. `chunked_flush.rs` covers 100 MiB payload at 1/4/16 MiB chunks through a no-op backend.

#### [MEDIUM-5.9.1] Benches have no regression baseline in CI
**File:** `crates/pcloud-fs/benches/chunked_flush.rs:16-20` (TODO comment) and `page_cache.rs` (no comment).
**Severity:** MEDIUM.
**Detail:** the author explicitly TODOed "Wire baseline capture into the `bench-nightly` CI job" but the wiring never landed. Without a baseline, regressions go unnoticed.
**Remediation:** add a CI matrix job that runs `cargo bench` and compares against a committed JSON snapshot.

#### [LOW-5.9.2] The `chunked_flush` bench runs against a no-op backend — it doesn't measure actual flush overhead
**File:** `crates/pcloud-fs/benches/chunked_flush.rs:44-89`.
**Severity:** LOW.
**Detail:** the bench explicitly measures state-machine dispatch cost only, which is fine for regression catching, but a separate integration bench against a `StagingDir`-backed scenario (without network) would catch the real I/O bottlenecks. The `write_path` module has no bench at all for `chunked_flush` through `WritePathService`.
**Remediation:** add a second bench that runs through `WritePathService::chunked_flush` with a real staging dir and an in-memory upload backend.

#### [LOW-5.9.3] No bench for mount/unmount round-trip latency
**File:** no file.
**Severity:** LOW.
**Detail:** the Linux mount path has a 2-second settle window; a bench that mounts+unmounts 100 times would reveal when that budget needs to change.

---

### 5.10 Integration tests

**Files:** `crates/pcloud-fs/tests/*.rs` — 10 test files.

- `fuse_mount_integration.rs` — `readdir` + read + write + fsync with a MockFolderBackend. **`#[ignore]` + `PCLOUD_FUSE_TEST=1` gated**, Linux-only.
- `fuse_kernel_e2e.rs` — full 64 MiB create/write/fsync/read/rename/unlink round-trip through real FUSE kernel. Linux-only, also `#[ignore]`.
- `fuse_read_path_live.rs`, `fuse_write_path_live.rs`, `fuse_small_write_wiring.rs`, `fuse_dyn_shim_write.rs`, `fuse_lifecycle_hardening.rs` — all Linux-gated.
- `mount_transport_wiring.rs`, `platform_mountinfo_crossplat.rs`, `write_path_replay.rs` — cross-platform compiling (parser + replay logic only, no kernel mount).

#### [CRITICAL-5.10.1] Every integration test that actually mounts a FUSE filesystem is `#[ignore]`
**Files:** all `fuse_*.rs` test files in `crates/pcloud-fs/tests/`.
**Severity:** CRITICAL (test signal).
**Detail:** the default `cargo test -p pcloud-fs` runs **zero** tests that exercise the kernel. A contributor can regress `mount()` without any test failing locally or in typical CI. The tests require `PCLOUD_FUSE_TEST=1` or `PCLOUD_LIVE_E2E=1` env var and a suid `fusermount3` binary + `/dev/fuse` access — a lot of containers / CI runners don't meet these criteria. The skip-logic inside each test (e.g. `fuse_gate_enabled()` or `should_skip_mount_error`) further degrades signal: even when the test **is** opted into, it may silently succeed by returning early.
**Remediation:** (a) add a dedicated CI job running in a privileged container with `/dev/fuse` that sets `PCLOUD_FUSE_TEST=1`; (b) make skip-paths emit a visible warning or convert them to runtime errors; (c) add `cargo test --features live-fuse` convention documented in the README.

#### [HIGH-5.10.2] No FreeBSD kernel-mount test exists in-tree
**File:** `crates/pcloud-fs/tests/` (absence).
**Severity:** HIGH.
**Detail:** FreeBSD is declared tier-2 but the only FreeBSD-specific test file is a compile-only assertion (`platform_mountinfo_crossplat.rs`). There is no FreeBSD-gated version of `fuse_kernel_e2e.rs`. On a platform the README claims as tier-2, the kernel-mount path has literally never been exercised.
**Remediation:** duplicate the e2e test with `#[cfg(any(target_os = "linux", target_os = "freebsd"))]` and parametrize the BSD `MNT_FORCE` path.

#### [HIGH-5.10.3] No macOS or Windows tests at all
**File:** `crates/pcloud-fs/tests/` (absence).
**Severity:** HIGH.
**Detail:** platform/macos.rs and platform/windows.rs have a total of ~100 KiB of Rust code and zero tests. The module-level docs repeat "NOT YET TESTED ON MACOS"/"PHASE-1 SCAFFOLDING" in ~6 places.
**Remediation:** tests gated by platform will at least be compile-checked, even if they skip. Add a minimum smoke test that validates `probe_supported` returns the expected `Unsupported` error when fuse-t / WinFSP is absent.

#### [MEDIUM-5.10.4] `write_path_replay.rs` tests are unit-style (no actual crash simulation)
**File:** `crates/pcloud-fs/tests/write_path_replay.rs:1-120` (3 tests).
**Severity:** MEDIUM.
**Detail:** The file name promises "replay" testing but the tests exercise `replay_path` API calls, not actual crash-during-write simulation. There's no test that literally hard-interrupts the write (e.g. via a forked subprocess killed via SIGKILL between journal.append and the visible rename), and then verifies `replay` recovers the state.
**Remediation:** fork a subprocess that calls `WriteJournal::append`, `exit(137)` before `commit`, then re-open in the parent and verify the prefix of records is intact.

#### [MEDIUM-5.10.5] `platform_mountinfo_crossplat.rs` only verifies parser compiles across platforms
**File:** `crates/pcloud-fs/tests/platform_mountinfo_crossplat.rs:1-100` (3 tests).
**Severity:** MEDIUM.
**Detail:** good for cross-platform compile assurance, but no actual cross-platform mount reconciliation test. No test of "BSD getmntinfo emits a payload that survives round-trip through `parse_pcloud_mounts`".
**Remediation:** add a fixture-driven test that feeds a representative `getmntinfo`-emitted payload through `parse_pcloud_mounts` on Linux and asserts the resulting entries are equivalent.

---

### 5.11 macOS specifics

**Files:** `crates/pcloud-fs/src/platform/macos.rs:1-1800+`, `crates/pcloud-fs/src/platform/macos_ffi.rs:1-500+`.

Module header is explicit: "**NOT YET TESTED ON MACOS** — bring-up requires a real Mac with fuse-t installed." Phase 5 is in-flight with write + read thunks populated, and `MacFuseBackend::FuseT` is the default (confirmed).

#### [CRITICAL-5.11.1] fuse-t vs. macFUSE ABI is asserted equivalent — but `LowlevelOps` struct layout is version-sensitive and unvalidated
**Files:** `crates/pcloud-fs/src/platform/macos.rs:1607-1626` (ops table), `crates/pcloud-fs/src/platform/macos_ffi.rs:1-500+` (struct defs).
**Severity:** CRITICAL.
**Detail:** `build_lowlevel_ops` constructs a `LowlevelOps` with 17 callback slots; it's passed to `fuse_lowlevel_new` with `size_of::<LowlevelOps>()` as the third argument, so libfuse reads only up to that size. If the installed libfuse 2.9 backend (fuse-t or macFUSE) has a different layout — say, a newer version that reorders fields or adds a callback at a lower offset — **the callbacks we install end up in the wrong slot** and libfuse calls, e.g., `write` when the kernel requested `getattr`. Passing a smaller `size` is safer than a larger one (libfuse won't read past), but cannot save us from a wrong-slot mapping.
**Remediation:** there is no runtime way to verify the layout. The crate must CI-build against the actual `fuse_lowlevel.h` from both fuse-t and macFUSE and assert `offsetof` matches. Until that ships, mark the macOS backend as experimental and feature-gate it off by default.

#### [HIGH-5.11.2] `ensure_libfuse_loaded` intentionally leaks the dlopen handle — correct, but no re-probe on dynamic link failure
**File:** `crates/pcloud-fs/src/platform/macos.rs:1497-1540`.
**Severity:** HIGH.
**Detail:** the dlopen handle is leaked (correct — dylib must outlive the session). But when `dlopen` succeeds but a subsequent `fuse_mount` fails with undefined-symbol (happens when a partial install has `libfuse.dylib` but missing rpath for its internal deps), the crate reports "fuse_mount failed" at `:200-204` without the dlerror context. Debugging a partial install becomes hard.
**Remediation:** resolve critical symbols via `dlsym` before calling them so we can emit a precise "symbol X not found in libfuse.dylib" error.

#### [HIGH-5.11.3] `volname` option is passed verbatim from user input without length validation
**File:** `crates/pcloud-fs/src/platform/macos.rs:1580-1598`.
**Severity:** HIGH.
**Detail:** macOS NFS/fuse-t imposes a 127-byte limit on volume names. A longer `fs_name` from `MountOptions` is formatted and passed through; fuse-t will either truncate silently (best case) or reject mount (worst case) with no guidance to the user.
**Remediation:** clamp `volname` to 127 bytes and warn when truncation occurs.

#### [MEDIUM-5.11.4] `entry_attr_to_stat` zeros `st_blocks`
**File:** `crates/pcloud-fs/src/platform/macos.rs:345-369`.
**Severity:** MEDIUM.
**Detail:** `st.st_blocks` is left at 0 (the default from `zeroed()`). macOS `du` uses `st_blocks` to compute disk usage; it will report 0 bytes used for every file, making the mount unusable with `du -sh` and similar tools.
**Remediation:** set `st.st_blocks = ((attr.size + 511) / 512) as i64;`.

#### [MEDIUM-5.11.5] `thunk_readdir` synthesizes `stub_attr` with arbitrary defaults
**File:** `crates/pcloud-fs/src/platform/macos.rs:696-705`.
**Severity:** MEDIUM.
**Detail:** during `readdir` the code builds a per-entry `libc::stat` from a stub attribute rather than from the real entry attributes returned by the adapter. macOS's `FUSE_READDIRPLUS` path wants real attrs to avoid the follow-up `lookup` per entry. This works around the missing `readdirplus`, but turns a O(1) listing into O(N lookups).
**Remediation:** either implement `readdirplus` or build the stat from `entry.attr` instead of `stub_attr`.

#### [LOW-5.11.6] 20 `eprintln!` debug prints remain in platform/macos.rs
**File:** `crates/pcloud-fs/src/platform/macos.rs` — grep-count 20.
**Severity:** LOW.
**Detail:** `[pcloud-fuse-t] ...` debug traces on every lookup/create/write/unlink/rename. Production build would flood stderr. They're not gated on a debug flag or the `log` crate.
**Remediation:** route through `log::debug!` (already a dependency).

#### [LOW-5.11.7] `fuse_session_loop` panic unwind is documented as "does not run user Rust panics" — but is not enforced
**File:** `crates/pcloud-fs/src/platform/macos.rs:246-256`.
**Severity:** LOW.
**Detail:** the comment says the loop thread doesn't unwind because thunks catch their own panics. If a future contributor adds a non-thunk caller (e.g. a helper that runs in the loop thread), unwinding across FFI is UB. Belt-and-braces would wrap the whole loop in `catch_unwind`.

---

### 5.12 Windows specifics

**Files:** `crates/pcloud-fs/src/platform/windows.rs:1-1800+`, `crates/pcloud-fs/src/platform/winfsp_ffi.rs:1-700+`.

Module header is blunt: "PHASE-3 SCAFFOLDING — FSP_FILE_SYSTEM dispatcher wired but not tested on Windows." `winfsp_ffi.rs` header: "PHASE-1 SCAFFOLDING — NOT YET TESTED ON WINDOWS. Treat every symbol here as a structural placeholder."

#### [CRITICAL-5.12.1] `VolumeParams` `reserved_tail: [u8; 256]` is an arbitrary guess at struct size
**File:** `crates/pcloud-fs/src/platform/winfsp_ffi.rs:113-135`.
**Severity:** CRITICAL.
**Detail:** `VolumeParams` explicitly declares "NOTE: The true struct layout is WinFSP-internal and version-sensitive. A final Windows-side build must validate `size_of::<VolumeParams>() == sizeof(FSP_FSCTL_VOLUME_PARAMS)` and each field offset against the installed WinFSP headers before we claim runtime parity." The `reserved_tail` is 256 bytes — but the actual WinFSP 2.x struct has grown past that in recent releases (some versions push past ~400 bytes). If the installed WinFSP's struct is larger than our declared `VolumeParams`, `FspFileSystemCreate` will read uninitialized stack/heap past our struct boundary (UB, or at best silently corrupt params). If smaller, we over-write and potentially clobber adjacent memory.
**Remediation:** generate `VolumeParams` from the installed `winfsp/fsctl.h` via a `build.rs` + `bindgen` pass, or check the WinFSP-reported size at runtime and refuse to mount on mismatch.

#### [CRITICAL-5.12.2] 11+ unsafe blocks without `SAFETY:` comments in the Windows path
**Files:** `crates/pcloud-fs/src/platform/windows.rs` (86 `unsafe` vs 75 `SAFETY`), `crates/pcloud-fs/src/platform/winfsp_ffi.rs` (19 `unsafe` vs 7 `SAFETY`).
**Severity:** CRITICAL (per CLAUDE.md §"enterprise rules").
**Detail:** CLAUDE.md says every unsafe block needs a SAFETY comment. The ratio says ~12 blocks in `winfsp_ffi.rs` and ~11 in `windows.rs` are bare. Example area of concern: the thunk bodies dereference `PFspFileSystem` and `file_ctx` pointers without documenting the invariants.
**Remediation:** add `SAFETY:` blocks or, better, wrap the raw pointers in newtype `Send`-able smart pointers whose methods carry the safety invariants.

#### [HIGH-5.12.3] The single `eprintln!` in `windows.rs` is a debug print, not a structured error
**File:** `crates/pcloud-fs/src/platform/windows.rs` — grep-count 1.
**Severity:** HIGH.
**Detail:** anything an operator would need to diagnose a mount failure is either missing or printed once via eprintln. No `log::error!`.

#### [HIGH-5.12.4] `load_winfsp` does not lock against concurrent loads
**File:** `crates/pcloud-fs/src/platform/winfsp_ffi.rs:200-300+` (load_winfsp).
**Severity:** HIGH.
**Detail:** dynamically loading the DLL returns a `WinFspLibrary` that's wrapped in `Arc` at the `MountHandle` level, but if two threads call `load_winfsp` concurrently they each call `LoadLibraryW` — `LoadLibraryW` is thread-safe at the OS level, but two callers then each call `GetProcAddress` for every symbol and produce two `WinFspLibrary` clones. Not UB but wasteful.
**Remediation:** store the loaded library in a `OnceLock<Arc<WinFspLibrary>>` static.

#### [HIGH-5.12.5] WinFSP Cleanup callback delete-on-close semantics not implemented
**File:** `crates/pcloud-fs/src/platform/windows.rs` (entire).
**Severity:** HIGH.
**Detail:** module doc line 43-46 says "Cleanup handles delete-on-close. WinFSP calls Cleanup with the FspCleanupDelete flag when the NT FILE_DELETE_ON_CLOSE disposition is set; the shim then issues the backend removal." — but grepping for `FspCleanupDelete` shows no implementation. A file opened with `FILE_DELETE_ON_CLOSE` and closed will not be deleted remotely. Data-consistency issue.
**Remediation:** implement the Cleanup callback slot.

#### [MEDIUM-5.12.6] No alternate data stream rejection — silent truncation
**File:** `crates/pcloud-fs/src/platform/windows.rs`.
**Severity:** MEDIUM.
**Detail:** doc says "Alternate Data Streams / reparse points: NOT supported. The corresponding WinFSP callbacks (where present) return STATUS_NOT_SUPPORTED." — need to verify the `Open` / `Create` callbacks actually reject paths with `:` (ADS notation) rather than silently treating them as regular filenames.

#### [MEDIUM-5.12.7] `WindowsMountinfoReader` is a stub (see 5.7.1)
(See §5.7.1 — same issue, raised for Windows specifically.)

---

### 5.13 FreeBSD specifics

**File:** `crates/pcloud-fs/src/platform/bsd.rs:1-564`.

Module declares tier-2 for FreeBSD, tier-3 for NetBSD/OpenBSD. Uses `fuser` crate's libfuse2 backend, same `fuser::Filesystem` shim as Linux (shared in `platform/fuser_shim.rs`). Mount via `fuser::spawn_mount2`, unmount via `libc::unmount(path, MNT_FORCE)` with a 2s settle window polling `getmntinfo(3)`.

#### [HIGH-5.13.1] `/dev/fuse` probe at `probe_supported` does not validate `kldload fuse` worked
**File:** `crates/pcloud-fs/src/platform/bsd.rs:129-152`.
**Severity:** HIGH.
**Detail:** the check is `Path::new("/dev/fuse").exists()`. But FreeBSD's fuse module sometimes creates `/dev/fuse` on first use only, not on kldload. Operator hint "load the fuse kernel module (kldload fuse / modload fuse)" is accurate for the common case but misleading when the node exists from a previous `fuse_mount` even if the module is now gone.
**Remediation:** try opening `/dev/fuse` with `O_RDWR|O_CLOEXEC` and check for `ENODEV` vs. `ENOENT`.

#### [MEDIUM-5.13.2] `MNT_FORCE` unmount is blunter than Linux `MNT_DETACH`
**File:** `crates/pcloud-fs/src/platform/bsd.rs:435-454`.
**Severity:** MEDIUM.
**Detail:** `MNT_FORCE` aborts in-flight requests; the Linux path uses `MNT_DETACH` which waits for references to drop but lets in-flight syscalls complete. The BSD comment acknowledges "FreeBSD has no exact `MNT_DETACH` analogue" — true — but the semantic difference affects data integrity for a process mid-write. With `MNT_FORCE` the write's EIO return is seen before the journal commits remotely.
**Remediation:** document this in the user-facing README, or attempt a `MNT_FORCE|MNT_DETACH` (FreeBSD supports both if `-2` is NOT set; newer kernels added `MNT_NONBUSY` for graceful-first escalation).

#### [MEDIUM-5.13.3] `path_is_current_mount` uses `f_mntonname` literal comparison — no escape decode
**File:** `crates/pcloud-fs/src/platform/bsd.rs:185-212`.
**Severity:** MEDIUM.
**Detail:** comparison is `Path::new(&mountpoint) == canonical`; `f_mntonname` is an unescaped kernel path (no `\040` encoding). Fine as long as canonicalize doesn't re-escape — which it doesn't. OK.

#### [LOW-5.13.4] No FreeBSD-specific test binary
(See §5.10.2.)

---

### 5.14 `bd-1du.4` gap checklist (per `CLAUDE.md`)

The bead states the Linux mount-runtime parity gaps as:
- real Linux mount/unmount ← **partially implemented** (missing: statfs/access/forget, no signal-safe trampoline)
- readdir ← **implemented** through `FuseAdapter::readdir` + shim
- open/read ← **implemented** (no prefetch)
- write/flush/fsync ← **implemented** (caveat: journal dir fsync missing)
- inode/path lifecycle ← **partial** (forget not wired; bare ino-to-path cache in `fuse_adapter.rs` has no eviction policy)
- crash-safe writeback ← **partial** (journal replay never runs, upload sidecar replay does; see 5.3.4)
- integration tests for mounted-drive behavior ← **all `#[ignore]`**

Net: bd-1du.4's own check-list is **not** satisfied. The epic cannot honestly be closed.

---

### 5.15 Per-platform coverage summary table

Legend: `I` implemented, `P` partial, `M` missing/stub, `X` not applicable.

| Capability                 | Linux (tier 1) | FreeBSD (tier 2) | macOS (tier 1*) | Windows (tier 1*) | NetBSD (tier 3) | OpenBSD (tier 3) |
|----------------------------|----------------|-------------------|------------------|--------------------|-----------------|------------------|
| Mountpoint validator       | I              | I                 | I                | P (drive letter only) | I (shared)   | I (shared)       |
| Kernel mount/unmount       | I              | I                 | P (never booted)†| P (never booted)†  | M               | M                |
| Read path (lookup+getattr+readdir+read) | I | I                 | P (scaffold)     | P (scaffold)       | M               | M                |
| Write path (create+write+flush+fsync+unlink+rename) | I | I  | P (scaffold)     | P (scaffold, no Cleanup) | M        | M                |
| `statfs`                   | **M**          | **M**             | I                | P (GetVolumeInfo)   | M               | M                |
| `access`                   | **M**          | **M**             | M                | M                  | M               | M                |
| `forget`                   | **M**          | **M**             | M                | X                  | M               | M                |
| `setattr` (mode/uid/gid/times) | M / size only | M / size only | M / size only    | M / size only      | M               | M                |
| `rename` flags             | M              | M                 | I                | I                  | M               | M                |
| Extended attributes        | M              | M                 | M                | M                  | M               | M                |
| `readlink`/`symlink`/`link`| M              | M                 | M                | M                  | M               | M                |
| Orphan detection           | I              | I (getmntinfo)    | I (getmntinfo)   | **M** (stub)       | I (shared)     | I (shared)       |
| Signal trampoline (SIGTERM/SIGINT/CTRL-C)| I| I                 | **M**            | **M**              | M               | M                |
| Journal replay on startup  | **M** (written but not consumed) | M | M           | M                  | M               | M                |
| Read-ahead / prefetch      | **M**          | M                 | M                | M                  | M               | M                |
| Page cache integration     | P (separate trait piece) | P       | P                | P                  | P               | P                |
| Integration test coverage  | `#[ignore]`d   | **M**             | **M**            | **M**              | M               | M                |

`*` — "planned tier 1" per `crates/pcloud-fs/Cargo.toml`. `†` — scaffolding only; module doc explicitly says "NOT YET TESTED ON MACOS/WINDOWS."

---

### 5.16 Consolidated remediation priorities

**Must-fix before claiming any parity (P0 — block `bd-1du.4` / `bd-1du.10`):**
1. Implement `statfs` across Linux/FreeBSD shims (5.2.1).
2. Fix journal `commit()` to fsync parent directory (5.3.1).
3. Stream `ProtoUploadBackend::upload_file` instead of slurping to memory (5.3.2).
4. Wire journal replay into daemon startup (5.3.4).
5. Default-ignore all integration tests defeats `bd-1du.4` proof — add a privileged CI job (5.10.1).
6. `MountService::mount` doesn't dispatch to Windows (5.1.1).
7. Validate WinFSP `VolumeParams` layout against installed headers (5.12.1).
8. Add SAFETY comments to the ~23 bare `unsafe` blocks on Windows (5.12.2).

**Should-fix before release (P1):**
9. Implement `access` and `forget` in shims (5.2.2, 5.2.3).
10. Read-ahead / prefetch in read path (5.4.1).
11. Eliminate or rename `WritebackJournal` to remove "crash-recoverable" misrepresentation (5.3.3).
12. Install signal trampolines for macOS and Windows (5.6.1, 5.6.2).
13. Implement Windows orphan detection (5.7.1).
14. Write-path setattr honors mode/times instead of silently succeeding (5.2.5).
15. Validate fuse-t `LowlevelOps` layout at build time (5.11.1).

**Nice-to-have (P2+):**
16. Extended attributes as `ENOTSUP` for ergonomic compatibility (5.2.7).
17. `fallocate`/`copy_file_range` for perf (5.2.8).
18. Bench regression baselines in CI (5.9.1).
19. Replace hand-rolled CRC32 with SIMD-optimized crate (5.3.7).
20. Route `eprintln!` through `log` crate (5.11.6).

---

### 5.17 Overall verdict

The `pcloud-fs` crate has good architecture, clean platform separation, and most of the happy-path Linux code is sound. But five load-bearing claims in CLAUDE.md §"What Is Left To Do" about `bd-1du.4` being "substantially scaffolded" are currently **not** substantiated by code:

1. "Real Linux mount/unmount" — yes, but without `statfs`/`access`/`forget`, operators will see regressions vs. the C client.
2. "Crash-safe writeback" — the journal format is crash-safe, but there is no code that consumes it on startup, and the doc-ed `fsync(file)+fsync(dir)` discipline is a lie.
3. "Integration tests for mounted-drive behavior" — tests exist but are all `#[ignore]`-gated; CI runs zero of them.
4. "macOS tier-1 planned" — the module self-describes as PHASE-1 SCAFFOLDING NOT YET TESTED, which is honest but cannot be called "tier 1."
5. "WinFSP tier-1 planned" — same: struct layouts unvalidated, Cleanup not implemented, orphan detection is a stub.

Every finding above has a file:line citation and a concrete remediation. The work required to close the gaps is substantial but incremental — no architectural rewrite. The most important single thing the project can do is **enable a privileged-CI job that runs `PCLOUD_FUSE_TEST=1 cargo test -p pcloud-fs` on every merge** — that alone would prevent the majority of future regressions.

**Recommendation for `bd-1du.10`:** do not close until items P0-1 through P0-5 above are landed. Downgrade the macOS and Windows entries in the parity matrix from "tier 1 planned" to explicit "scaffolding — not production" until §5.11.1 and §5.12.1 are resolved.

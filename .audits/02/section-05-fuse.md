# Section 5: Mounted-drive / FUSE Parity
## Date: 2026-04-17
## Scope: crates/pcloud-fs (bd-1du.4)

## Findings

### CRITICAL [6]
- 5.1  WinFSP `VolumeParams` layout is a hand-rolled guess — unvalidated against any real WinFSP header, runtime UB risk
- 5.2  Linux/BSD `FuseAdapter`-dispatching shims in `platform/linux.rs` (BoxedFuserShim + FuserShim<A>) still do NOT wire `statfs`, even though the shared `platform/fuser_shim.rs` sibling does — the live Linux mount path reaches the linux.rs duplicates, not the shared ones
- 5.3  `ProtoUploadBackend::upload_file` still slurps the entire staging blob with `std::fs::read` — OOM on large files
- 5.4  `WritePathService::replay_journal` exists as a method but NO production caller invokes it on daemon startup → crashed journal records are never replayed
- 5.5  macOS `LowlevelOps` vtable layout unvalidated against the installed fuse-t / macFUSE `fuse_lowlevel.h` — wrong-slot dispatch would produce silent data corruption
- 5.6  Every kernel-mount integration test is `#[ignore]` + env-var gated — default `cargo test` runs zero FUSE tests

### HIGH [12]
- H.1  `MountService::mount` has a cfg-ladder that NEVER dispatches to Windows (`WindowsPlatformMount::mount_adapter` is unreachable from this entry point)
- H.2  macOS mount installs NO SIGTERM/SIGINT trampoline — `kill -9` or Ctrl-C orphans the fuse-t mount
- H.3  Windows mount installs NO `SetConsoleCtrlHandler` — service stop / CTRL-CLOSE leaves a zombie drive letter
- H.4  Windows orphan detection is a stub (`WindowsMountinfoReader::read` returns empty String) — any WinFSP dispatcher crash leaves undetectable zombie mounts
- H.5  `access` is unimplemented across every Linux/BSD shim — `access(X_OK)` calls from util-linux/systemd get `ENOSYS`
- H.6  `forget` is unimplemented in every shim — long-running daemons leak ino→path entries in `ProtoFuseAdapter`'s local map
- H.7  `FileHandle::size = 0` at open time in `ProtoFileBackend::open` (backend.rs) → `getattr`/`statfs` return zero size
- H.8  No read-ahead / prefetch in the read path — every `read(2)` blocks a FUSE worker thread on an HTTP RTT (severe perf parity regression)
- H.9  macOS `volname` passed through without clamping to the 127-byte fuse-t/NFS limit — mount silently fails or truncates
- H.10 Windows: 81 `unsafe { }` blocks in `platform/windows.rs` with only 74 `SAFETY:` comments; 3 unsafe blocks in `platform/winfsp_ffi.rs` with 7 `SAFETY:` comments (ratio misaligned — several unsafe blocks are uncommented)
- H.11 Windows `Cleanup` callback is wired but `FspCleanupDelete` flag handling to issue the backend delete is NOT implemented — `FILE_DELETE_ON_CLOSE` files never get removed server-side
- H.12 `MountHandle::Drop` silently discards unmount errors (`let _ = inner.unmount()`) — violates CLAUDE.md §"do not silently swallow persistence/audit failures"

### MEDIUM [11]
- M.1  `write_journal.rs::WriteJournal::commit` fsyncs the file (`sync_data`) but NOT the parent directory — contradicts the doc-stated "P1.2 `fsync(file)+fsync(dir)` discipline" in write_path.rs:37-45. Only `UploadProgress::save` fsyncs the parent dir
- M.2  `setattr` accepts chmod/chown/utimens without effect and still replies success — silent lie
- M.3  `rename` ignores `RENAME_NOREPLACE`/`RENAME_EXCHANGE` flags (renameat2 silent overwrite)
- M.4  `journal.rs::WritebackJournal` is documented as "ordered, crash-recoverable record of pending filesystem mutations" but is entirely in-memory and silently evicts on overflow — misleading module doc
- M.5  Linux `signal_trampoline` calls `Mutex::lock` + `CString::new` (global allocator) inside a signal handler — not async-signal-safe, potential deadlock / UB
- M.6  Linux Drop path invokes `drop(fuser::BackgroundSession)` which joins the dispatcher thread unbounded — no 5-second timeout like macOS teardown has
- M.7  `MountOptions` exposes only `read_only`/`fs_name`/`allow_other`; no `attr_timeout` / `entry_timeout` / `max_readahead` knobs
- M.8  Page cache and metadata cache have no invalidation hook tied to a pCloud diff stream — 1s TTL covers most cases but remote mutations are invisible for up to 1 s
- M.9  macOS `thunk_readdir` uses `stub_attr` per entry instead of real attrs — defeats READDIRPLUS
- M.10 macOS `entry_attr_to_stat` leaves `st_blocks = 0` → `du -sh` reports 0 for every file
- M.11 Staging orphan risk: if `chunked_flush` errors after `upload_create` but before any `upload_write`, the server-side upload_id leaks until pCloud GC (no `upload_cancel` paired)

### LOW [8]
- L.1  `unescape_mountinfo` accepts non-octal digits `\089` (digit check should be `<= '7'`)
- L.2  `WriteJournal` CRC32 loop is hand-rolled scalar (~1.1 GB/s) — replace with `crc32fast` SIMD
- L.3  `next_seq` resets to 1 on journal re-open, ignoring records already in the file
- L.4  ~20 `eprintln!` debug prints remain in `platform/macos.rs` (should use `log` crate)
- L.5  No mount/unmount round-trip latency bench
- L.6  `xattr`/`readlink`/`symlink`/`link`/`fallocate`/`copy_file_range`/`lseek` all missing (pCloud server has no symlink; xattr should map to `ENOTSUP` rather than `ENOSYS` for desktop-env friendliness)
- L.7  No Windows-specific mountpoint path hardening (accepts `%SystemRoot%` / `%ProgramFiles%` subpaths)
- L.8  `fusermount_unmount` has no fast path for "already unmounted" (nonzero exit treated as error)

---

## Detailed Findings

### 5.1 Cross-platform architecture — `PlatformMount` trait and platform implementations

**Trait definition:** `crates/pcloud-fs/src/platform/mod.rs:48-99` defines
`PlatformMount` with `validate_mountpoint`, `probe_supported`, `default_options`,
`mount_adapter`, `unsupported`. Compile-time selection through
`ActivePlatformMount` type alias at `mod.rs:115-122`.

Per-OS status:

- **Linux (`platform/linux.rs:1-1116`)** — REAL. Uses `fuser::spawn_mount2` via
  both typed `mount_with_fuser<A>` (linux.rs:1067) and dyn
  `mount_fuser_filesystem<F>` (linux.rs:1094). Options built with `FSName`,
  `Subtype`, `DefaultPermissions`, `NoDev`, `NoSuid` + RO/RW (linux.rs:1045-1064).
  Signal trampoline installed on first mount via
  `install_signal_handler_once()` (linux.rs:519-549) hooking SIGTERM+SIGINT and
  calling `libc::umount2(..., MNT_DETACH)`. Unmount handle
  (linux.rs:568-649) polls `/proc/self/mountinfo` for up to 2 s before
  escalating to `umount2(MNT_DETACH)`.
- **BSD (`platform/bsd.rs:1-310`)** — PARTIAL. Only validation + `getmntinfo(3)`
  orphan reader + `probe_supported`. `mount_adapter` is the default trait impl
  returning `UnsupportedPlatform` — i.e. BSD cannot actually mount through
  `PlatformMount::mount_adapter`. However `mount_service.rs` has a
  `#[cfg(target_os = "freebsd")]` branch that routes to
  `bsd::mount_with_fuser` (but that function does not exist in bsd.rs — the
  kernel mount path is explicitly `TODO(bd-xplat-bsd)` at bsd.rs:28).
  **Severity:** HIGH — FreeBSD is declared tier-2 but the mount call path is
  unrealized.
- **macOS (`platform/macos.rs:1-1758`)** — REAL FFI code, NOT YET TESTED
  on hardware (module doc bsd.rs:4-6 + repeated NOT-YET-TESTED markers). Full
  low-level op thunk set wired: `init`, `destroy`, `lookup`, `getattr`, `open`,
  `read`, `readdir`, `release`, `statfs`, `write`, `create`, `unlink`, `mkdir`,
  `rmdir`, `rename`, `flush`, `fsync`, `setattr` (macos.rs:1607-1626). FFI
  declarations in `macos_ffi.rs:286-385`. Installs libfuse dylib via
  `dlopen(RTLD_LAZY|RTLD_GLOBAL)` (macos.rs:1521). Runs session loop on a
  dedicated thread (macos.rs:247-267). Teardown via `teardown_macos`
  (mount_service.rs:430-475) orders `fuse_session_exit` →
  `fuse_unmount` → 5-second bounded join → `fuse_session_destroy`.
- **Windows (`platform/windows.rs:1-1754`, `winfsp_ffi.rs:1-587`)** — REAL FFI
  code, NOT YET TESTED on hardware. Full WinFSP callback table populated
  (windows.rs:390-413). Lifecycle: `FspFileSystemCreate` →
  `SetMountPoint` → `StartDispatcher` (windows.rs:267-321). Teardown:
  `FspFileSystemStopDispatcher` + `FspFileSystemDelete`
  (mount_service.rs:478-499). DLL loaded via `LoadLibraryW("winfsp-x64.dll")`
  (winfsp_ffi.rs:462-533) with optional `FspFileSystemAddDirInfo` probe.
- **`fuser_shim.rs` (shared shim — `platform/fuser_shim.rs:1-968`)** — REAL.
  Gated `#[cfg(any(target_os = "linux", target_os = "freebsd"))]`.
  Implements statfs (fuser_shim.rs:105-151, 529-575), lookup/getattr/readdir/
  open/read/release/create/write/flush/fsync/setattr/unlink/rename/mkdir/rmdir.
  HOWEVER `platform/linux.rs` defines its OWN `BoxedFuserShim` (linux.rs:84-506)
  and `FuserShim<A>` (linux.rs:663-1039) that DUPLICATE the shared shim
  WITHOUT statfs — and `LinuxPlatformMount::mount_adapter` (linux.rs:62-71)
  routes through the linux.rs `BoxedFuserShim`, not through the shared
  `platform/fuser_shim.rs` one. **CRITICAL-5.2** (see below).

---

#### [CRITICAL-5.1] WinFSP `VolumeParams` layout unvalidated
**File:** `crates/pcloud-fs/src/platform/winfsp_ffi.rs:113-135`
**Severity:** CRITICAL
**Detail:** `VolumeParams` struct has a trailing
`reserved_tail: [u8; 256]` that is an ARBITRARY guess at the size needed
to pad up to the real `FSP_FSCTL_VOLUME_PARAMS`. The doc comment itself
flags this at winfsp_ffi.rs:108-112:
`"The true struct layout is WinFSP-internal and version-sensitive. A
final Windows-side build must validate size_of::<VolumeParams>() == ..."`
Since `FspFileSystemCreate` reads `sizeof(VolumeParams)` bytes from our
pointer, a real WinFSP whose struct exceeds our declaration reads past
our stack allocation (UB); if smaller we overwrite adjacent memory.
**Fix:** generate `VolumeParams` via `build.rs` + `bindgen` against the
installed `winfsp/fsctl.h`, OR query the WinFSP-reported size and refuse
to mount on mismatch.

#### [CRITICAL-5.2] `platform/linux.rs` duplicate shims lack `statfs`
**Files:** `crates/pcloud-fs/src/platform/linux.rs:84-506` (`BoxedFuserShim`),
`linux.rs:663-1039` (`FuserShim<A>`), `linux.rs:62-71`
(`LinuxPlatformMount::mount_adapter` routing through the linux.rs
`BoxedFuserShim`).
**Severity:** CRITICAL
**Detail:** The shared `platform/fuser_shim.rs` sibling DOES implement
`fn statfs` (fuser_shim.rs:105, 529), but the Linux `mount_adapter` entry
point (`LinuxPlatformMount::mount_adapter` at linux.rs:62) wraps the adapter
in a LOCAL `BoxedFuserShim` defined in linux.rs:84 that has no statfs
method. The `fuser::Filesystem` default reply is `ENOSYS` → every `df`,
`stat -f`, or desktop indexer query on the mount gets EIO/ENOSYS. This is
the authoritative live-mount code path — so despite the cleanup in the
shared shim, the actual live Linux mount does not expose statfs.
**Fix:** delete the duplicate shim bodies in `platform/linux.rs` and route
`LinuxPlatformMount::mount_adapter` through the shared
`platform/fuser_shim::BoxedFuserShim`. (The duplicate exists only because
of the earlier refactor; remove the dead copy.)

#### [CRITICAL-5.3] `upload_file` OOM on large staging blob
**File:** `crates/pcloud-fs/src/backend.rs:403-488` (esp. line 416:
`let bytes = std::fs::read(staging_file)?;`).
**Severity:** CRITICAL
**Detail:** For 10 GiB upload, `std::fs::read` allocates a 10 GiB Vec. On
a daemon host with 4 GiB RAM this OOM-kills the daemon. Because the
trait's default `upload_create` returns "chunked not supported", any
caller that falls through to `upload_file` (tests, or any backend that
opts out of chunked) crashes.
**Fix:** stream in 4 MiB chunks via `BufReader` + `upload_create` +
`upload_write`/`upload_save` OR remove `upload_file` from the trait and
force every implementor through the chunked surface.

#### [CRITICAL-5.4] Journal replay never runs on daemon startup
**Files:** `write_path.rs:770-778` (`replay_journal` method exists);
`write_journal.rs:252-317` (`replay_path`); no callers outside tests
(ripgrep: the only non-test references are the method's own body).
**Severity:** CRITICAL
**Detail:** `WritePathService::replay_journal` returns the well-formed
records from the journal, but nothing in `pcloud-daemon` or any startup
path invokes it. A crash between "journal record appended+fsynced" and
"backend ack" leaves the operation in the journal forever with no retry.
`replay_upload_sidecars` (write_path.rs, separate) covers only in-flight
chunked uploads, not stand-alone `Unlink`/`Rename`/`Truncate` journal
records.
**Fix:** wire `pcloud-daemon` startup to call
`WritePathService::replay_journal`, drive each op against the live
`pcloud-proto` backend, and `WriteJournal::reset` on success.

#### [CRITICAL-5.5] macOS `LowlevelOps` vtable layout unvalidated
**Files:** `platform/macos.rs:1605-1626` (`build_lowlevel_ops`),
`platform/macos_ffi.rs:141-271` (`LowlevelOps` struct definition).
**Severity:** CRITICAL
**Detail:** `LowlevelOps` is a 17-slot `#[repr(C)]` struct that mirrors
`struct fuse_lowlevel_ops` from fuse-t's `libfuse.dylib`. The module
doc at macos_ffi.rs:118-127 explicitly warns: "Omitting trailing fields
would corrupt the vtable layout, so we pad with enough Option<...> slots
to cover the libfuse 2.9 ABI. Real-Mac bring-up must reconcile the exact
slot count against the installed header." This has not happened — the
workspace has never booted on macOS. If the installed library has
different ordering or additional ops between `statfs` (our slot 24) and
`create` (our slot 30), the kernel's `create` request fires `thunk_statfs`
(silent data corruption).
**Fix:** codegen via `build.rs` + `bindgen` against a committed checkout
of fuse-t's `fuse_lowlevel.h`. Until validated on hardware, feature-gate
the macOS backend off by default.

#### [CRITICAL-5.6] All FUSE integration tests are `#[ignore]`-gated
**Files:** `crates/pcloud-fs/tests/fuse_mount_integration.rs`,
`fuse_kernel_e2e.rs`, `fuse_read_path_live.rs`, `fuse_write_path_live.rs`,
`fuse_small_write_wiring.rs`, `fuse_dyn_shim_write.rs`,
`fuse_lifecycle_hardening.rs`. Each uses a `fuse_gate_enabled()` or
equivalent env-var check (e.g. `PCLOUD_FUSE_TEST=1`) and `#[ignore]`.
**Severity:** CRITICAL
**Detail:** Default `cargo test -p pcloud-fs` executes ZERO kernel-mount
tests. CI cannot validate regressions in mount/unmount, readdir, read,
write, fsync, rename, unlink unless the job explicitly sets the env var.
Per CLAUDE.md, the `bd-1du.4` proof gate REQUIRES "integration tests for
mounted-drive behavior" — those tests exist but do not execute.
**Fix:** add a privileged CI job (e.g. GitHub Actions runner with
`--device /dev/fuse --cap-add SYS_ADMIN`, or a bare-metal VM) that sets
`PCLOUD_FUSE_TEST=1` and runs `cargo test -p pcloud-fs -- --ignored`.

---

### 5.2 Core FUSE op coverage

**FuseAdapter trait surface** (fuse_adapter.rs:111-510) — all ops:
`lookup`, `getattr`, `readdir`, `open`, `read`, `release`, `create`,
`write`, `flush_write`, `fsync_write`, `truncate`, `unlink`, `rename`,
`mkdir`, `rmdir`, `resolve_ino_to_path`, `setattr` (WinFSP-specific),
`set_basic_info` (WinFSP-specific), `statfs`, `forget` (fuse_adapter.rs:503).
Defaults are `ENOSYS`.

**Per-op live-wire matrix** — Linux via `platform/linux.rs` BoxedFuserShim
(the ACTUALLY LIVE shim), and also `fuser_shim.rs::PcloudFsShim` when the
daemon uses the typed entry point. Uppercase = wired, *=partial, —=missing:

| Op        | linux.rs shim | fuser_shim.rs PcloudFsShim | shared platform/fuser_shim.rs | macos.rs | windows.rs |
|-----------|---------------|----------------------------|-------------------------------|----------|------------|
| lookup    | Y (151)       | Y                          | Y (153)                       | Y (405)  | Y (Open 787) |
| getattr   | Y (167)       | Y                          | Y                             | Y (467)  | Y (cb_get_file_info) |
| readdir   | Y (180)       | Y                          | Y                             | Y (656)  | Y (cb_read_directory) |
| open      | Y (226)       | Y                          | Y                             | Y (509)  | Y (cb_open) |
| read      | Y (233)       | Y                          | Y                             | Y (558)  | Y (cb_read) |
| release   | Y (254)       | Y                          | Y                             | Y (743)  | Y (cb_close) |
| create    | Y (274)       | Y                          | Y                             | Y (858)  | Y (cb_create) |
| write     | Y (314)       | Y                          | Y                             | Y (793)  | Y (cb_write) |
| flush     | Y (333)       | Y                          | Y                             | Y (1235) | Y (cb_flush) |
| fsync     | Y (347)       | Y                          | Y                             | Y (1272) | — (cb_flush is no-op) |
| setattr   | * (361 size-only) | * (size-only)            | * (size-only)                 | * (1310 size-only) | * (set_basic_info; mode/times as no-op) |
| unlink    | Y (396)       | Y                          | Y                             | Y (955)  | Y (via rename + cb_cleanup) |
| rename    | * (424 no flags)  | * (no flags)             | * (no flags)                  | Y (1149) | * (replace_if_exists only) |
| mkdir     | Y (456)       | Y                          | Y                             | Y (1017) | — (falls into Create) |
| rmdir     | Y (486)       | Y                          | Y                             | Y (1083) | — |
| **statfs**| **— (MISSING)** | **— (MISSING)**         | **Y (105)**                   | Y (1376) | Y (cb_get_volume_info) |
| access    | —             | —                          | —                             | —        | — |
| forget    | —             | —                          | —                             | —        | — |
| opendir/releasedir/fsyncdir | —  | —                         | —                             | —        | — |
| xattr (get/set/list/remove) | — | —                         | —                             | —        | — |
| readlink/symlink/link       | — | —                         | —                             | —        | — |
| fallocate / copy_file_range | — | —                         | —                             | —        | — |

Critical omissions are **statfs** on the live Linux/BSD path (CRITICAL-5.2
above) and **forget** (HIGH below — leaks adapter's ino→path map for
long-running daemons).

#### [HIGH-5.2.1] `access` unimplemented
**Files:** every shim. No `fn access` anywhere in the crate.
**Severity:** HIGH
**Detail:** With `DefaultPermissions` set the kernel enforces mode bits
in-kernel, so most `access(2)` traffic never reaches FUSE. But
`access(X_OK)` and several util-linux / systemd code paths still issue
the op. Default `ENOSYS` → kernel converts to `EACCES` in several paths
→ desktop indexer, `df`, path-completion shells spuriously fail.
**Fix:** minimal `fn access` returning `0` (allow) or delegating to a new
`FuseAdapter::access` trait method.

#### [HIGH-5.2.2] `forget` unimplemented → inode-map leak
**Files:** every shim. `ProtoFuseAdapter::forget_local_entry` exists
(fuse_adapter.rs — grep) but nothing calls it.
**Severity:** HIGH
**Detail:** FUSE kernel increments per-inode lookup count on every
`lookup`/`create` and expects filesystems to decrement by `nlookup` on
`forget`. The adapter carries its own ino→path cache in
`fuse_adapter.rs:1143` (`forget_local_entry`), but no shim forwards the
FUSE `forget` op to it. A long-running daemon with heavy directory churn
grows the map without bound.
**Fix:** wire
`fn forget(&mut self, _req, ino, nlookup) { self.adapter.forget(ino, nlookup); }`
in all three linux-path shims.

#### [MEDIUM-5.2.3] `setattr` silently accepts chmod/chown/utimens
**Files:** `platform/linux.rs:361-394`, `platform/fuser_shim.rs`,
`fuser_shim.rs (PcloudFsShim)`, `platform/macos.rs:1335`,
`platform/windows.rs::cb_set_basic_info:1096-1177`.
**Severity:** MEDIUM
**Detail:** Only `size` is routed to `adapter.truncate`. `mode`, `uid`,
`gid`, `atime`, `mtime`, `ctime`, `crtime`, `chgtime`, `bkuptime`, `flags`
are `_`-prefixed and ignored. `touch -t` on the mount returns success but
is a lie; `chmod 0644 foo` returns success without effect.
**Fix:** either return `EPERM`/`ENOSYS` explicitly for unsupported
setattr bits, or implement `utimens` via pCloud `modified_at`.

#### [MEDIUM-5.2.4] `rename` ignores `RENAME_NOREPLACE`/`RENAME_EXCHANGE`
**Files:** `platform/linux.rs:424-454`, `platform/fuser_shim.rs:rename`,
`fuser_shim.rs::PcloudFsShim::rename`.
**Severity:** MEDIUM
**Detail:** `_flags: u32` param is ignored. `renameat2(2)` with
`RENAME_NOREPLACE` (git/atomic writers) silently overwrites.
**Fix:** extend `FuseAdapter::rename(from, to, flags)` and honor
`RENAME_NOREPLACE` by pre-checking target existence and returning
`EEXIST`; reject `RENAME_EXCHANGE` with `ENOTSUP`.

---

### 5.3 Write path & journal

**Files:** `write_path.rs:1-2206`, `write_journal.rs:1-522`,
`staging.rs:1-382`, `journal.rs:1-118`.

Overall the write path is thoughtful: an on-disk WAL with CRC32 envelopes
(write_journal.rs:140-216), per-inode `UploadProgress` sidecar with
write-then-rename durability (write_path.rs:882-911), resumable
chunked-flush loop at 4 MiB (write_path.rs:455-543), heartbeat-timeout
stall classification (write_path.rs:919).

**Staging blob:** `staging.rs::StagingDir` creates `<root>/journal.log`
and `<root>/blobs/<name>` with mode `0o700`/`0o600`, rejects path
traversal (staging.rs:82-93), fsyncs every blob write
(`sync_data()` at staging.rs:173, 199, 214).

**Flush threshold:** `DEFAULT_FLUSH_THRESHOLD_BYTES = 64 * 1024 * 1024`
(write_path.rs:243). Enforced at write_path.rs:431:
`size_trigger = dirty >= self.options.flush_threshold_bytes`.

**fsync on journal AND parent dir:** journal file commit does
`self.file.flush()?; self.file.sync_data()?;` (write_journal.rs:221-225)
— **NO parent-dir fsync**. Parent-dir fsync appears ONLY in
`UploadProgress::save` (write_path.rs:900-907). See MEDIUM-M.1 below.

**Journal replay tested on simulated crash:** `write_path_replay.rs` is
a unit-style test that calls `replay_path` directly (tests/write_path_replay.rs
— 114 lines). NO forked-subprocess SIGKILL simulation.

#### [MEDIUM-M.1] `WriteJournal::commit` does not fsync parent directory
**File:** `write_journal.rs:219-227`
**Detail:** Contract doc at write_path.rs:37-45 claims:
```
1. Append a JournalRecord ...
2. fsync(file) the journal file descriptor ...
3. fsync(dir) the journal's parent directory so the directory
   entry is durable — skipping this step means a post-crash `readdir`
   may fail to find a freshly-created journal segment, silently
   dropping acknowledged writes.
```
Implementation only calls `self.file.sync_data()` → directory entry for
freshly-created journal segment is not durable. `UploadProgress::save`
already does this correctly (write_path.rs:900-907) — port the same
pattern.
**Fix:** open parent dir with `O_DIRECTORY|O_RDONLY`, store in
`WriteJournal` struct, `sync_all()` on it after every `commit()`.

#### [HIGH-5.3.1] `WritebackJournal` misrepresentation
**File:** `journal.rs:1-118`
**Detail:** Module doc says "ordered, crash-recoverable record of pending
filesystem mutations" but the struct is a bounded in-memory `VecDeque`
with silent-eviction on overflow (journal.rs:50-55). Nothing in the
crate serializes it to disk.
**Fix:** either delete the module (only tests reference it) or rename to
`InMemoryWritebackCounters` and delete "crash-recoverable" claim.

---

### 5.4 Read path & cache

**Files:** `read_path.rs:1-255`, `page_cache.rs:1-504`,
`backend.rs::ProtoFileBackend`.

- Read latency: no explicit budget — each `adapter.read` spawns a
  `fetch_download` on the FUSE worker thread (backend.rs:277-312).
- Prefetch: `ReadPathService` has a `prefetch_window_bytes: 256 KiB`
  (read_path.rs:70) that pre-fills the page cache within a single
  `read()` call — but this is only in the in-memory staging-cache read
  path (used by the FilesystemShell scaffold); the real kernel-facing
  read path goes through `ProtoFuseAdapter::read` → `ProtoFileBackend`
  with NO async background prefetch.
- Memory bounds: `PageCache` defaults to 128 MiB
  (`DEFAULT_MAX_BYTES = 128 * 1024 * 1024`, page_cache.rs:71) with LRU
  eviction. `Arc<Vec<u8>>` values share on hit (page_cache.rs:24-40).

#### [HIGH-5.4.1] No background / sequential-read prefetch
**File:** `backend.rs:277-312` (`ProtoFileBackend::read`).
**Detail:** Every read is synchronous HTTP. Streaming video or large
sequential copy off the mount is orders of magnitude slower than the C
reference (`pfs_cache.c` look-ahead fetch).
**Fix:** sequential-read detector + background prefetch thread filling
the `PageCache`.

#### [HIGH-5.4.2] `FileHandle::size` is zero after open
**File:** `backend.rs:268-274` (comment acknowledges).
**Detail:** `getfilelink` does not return size; no follow-up stat. Any
caller relying on size via the handle sees 0.
**Fix:** HEAD the signed URL or issue `list_folder_contents_by_path` on
the parent during `open`.

---

### 5.5 Mount handle RAII

**File:** `mount_service.rs:229-500`.

Union `MountHandle` with per-OS `Option<Inner>`. Per the module doc
(mount_service.rs:234-259) the ordered teardown is: shutdown flag →
native exit → native unmount → bounded join (5 s) → native destroy →
reclaim leaked adapter Box.

- **Linux:** `LinuxMountHandle::unmount` (linux.rs:573-648) drops
  `fuser::BackgroundSession`, polls `/proc/self/mountinfo` for 2 s, then
  escalates to `umount2(MNT_DETACH)`. Reg-entry removed regardless of
  success.
- **macOS:** `teardown_macos` (mount_service.rs:430-475) with 5-second
  bounded `recv_timeout` on the loop-join channel.
- **Windows:** `teardown_windows` (mount_service.rs:478-499) calls
  `FspFileSystemStopDispatcher` + `FspFileSystemDelete` — NTSTATUS
  returns discarded.

#### [HIGH-H.12] Drop silently swallows unmount errors
**File:** `mount_service.rs:502-523`
**Detail:** `let _ = inner.unmount();` in Drop discards errors. CLAUDE.md
§"IPC and local security" prohibits silent persistence/audit-failure
swallowing on active control paths.
**Fix:** `if let Err(e) = inner.unmount() { log::error!(...) }`.

#### [MEDIUM-M.6] Linux Drop join is unbounded
**File:** `platform/linux.rs:583-585` (`drop(self.session.take())`).
**Detail:** `fuser::BackgroundSession::drop` joins the dispatcher thread
without a timeout. A wedged worker (stuck in a pending HTTP read with
TCP half-open) blocks Drop forever. macOS path uses
`recv_timeout(Duration::from_secs(5))` (mount_service.rs:461).
**Fix:** pull out `fuser::SessionUnmounter` separately and call with a
bounded wait, or document the risk.

---

### 5.6 Signal handling

- **Linux:** `install_signal_handler_once()` at `platform/linux.rs:519-549`
  installs a process-wide handler for SIGTERM + SIGINT that iterates
  `ACTIVE_MOUNTS` and calls `libc::umount2(..., MNT_DETACH)` before
  restoring `SIG_DFL` and re-raising.
- **BSD:** `platform/bsd.rs` does NOT install a signal trampoline — no
  `libc::signal` call in the file.
- **macOS:** `platform/macos.rs` does NOT install a signal trampoline —
  no `libc::signal` call in the file.
- **Windows:** `platform/windows.rs` does NOT install a
  `SetConsoleCtrlHandler` — no console-control handler.

#### [HIGH-H.2] No signal trampoline on macOS
**File:** `platform/macos.rs:155-277` (`mount_with_fuse_t`).
**Fix:** mirror Linux's `install_signal_handler_once` and call
`fuse_unmount` from the trampoline for each ACTIVE_MOUNTS entry.

#### [HIGH-H.3] No console-control handler on Windows
**File:** `platform/windows.rs::mount_with_winfsp_dyn`.
**Fix:** `SetConsoleCtrlHandler` that invokes
`FspFileSystemStopDispatcher` + `FspFileSystemDelete` for every live
mount, on CTRL_CLOSE_EVENT / CTRL_SHUTDOWN_EVENT.

#### [MEDIUM-M.5] Linux trampoline is not async-signal-safe
**File:** `platform/linux.rs:531-549`
**Detail:** `mtx.lock()` + `CString::new(path.as_os_str().as_encoded_bytes())`
(heap allocation) inside the handler. If the main thread holds the mutex
at signal delivery, the handler deadlocks; allocator is not AS-safe.
**Fix:** use `SA_SIGINFO` and write a byte to a pipe; perform the unmount
on a dedicated reaper thread that drains the pipe.

---

### 5.7 Orphan detection

**File:** `mount_orphan.rs:1-404`.

- Parser (`parse_pcloud_mounts`, mount_orphan.rs:158-185) matches against
  `PCLOUD_FUSE_TYPES = ["fuse.pcloud", "fuse.pclsync", "fuse.pcloud-rs"]`.
- `detect_orphans` (mount_orphan.rs:193-203) filters against a known-mount
  set.
- `fusermount_unmount` (mount_orphan.rs:256-293) shells out to
  `fusermount3`/`fusermount -u` with bounded timeout.
- Linux reader: `ProcMountinfoReader` at `platform/linux.rs:29-36` reads
  `/proc/self/mountinfo`.
- BSD reader: `BsdMountinfoReader` (bsd.rs:191-198) wraps
  `getmntinfo(3)` and emits mountinfo-shaped output.
- macOS reader: `MacosMountinfoReader` (macos.rs:1674-1681) same
  `getmntinfo` approach.
- Windows reader: `WindowsMountinfoReader::read` (windows.rs:197-203)
  returns empty `String` — STUB.

#### [HIGH-H.4] Windows orphan detection is a stub
**File:** `platform/windows.rs:195-210`
**Detail:** Empty payload returned → daemon never detects a zombie
WinFSP drive letter after dispatcher crash. Next mount attempt fails
with opaque "mount failed".
**Fix:** enumerate via `GetLogicalDriveStringsW` + `QueryDosDeviceW`;
pCloud WinFSP drives have NT device names starting with
`\Device\WinFsp.Disk\`. Emit entries in the mountinfo schema.

---

### 5.8 Mount policy / `MountOptions`

**File:** `mount_service.rs:25-156`.

`MountOptions { read_only, fs_name, allow_other }`. `allow_other = true`
is rejected at the cross-platform layer (`AllowOtherRejected` at
mount_service.rs:165-167). `validate_mountpoint` checks: exists →
directory → empty → owned by current uid (Linux) → not world-writable
(Linux). `build_fuse_options` (linux.rs:1045-1064) adds `NoDev`, `NoSuid`,
`DefaultPermissions`.

#### [HIGH-H.1] `MountService::mount` never dispatches to Windows
**File:** `mount_service.rs:170-189`
**Detail:** cfg-ladder has Linux and macOS branches but falls into the
`else` arm returning `UnsupportedPlatform` on Windows even though
`WindowsPlatformMount::mount_adapter` (windows.rs:175-184) exists.
**Fix:** replace the ladder with
`ActivePlatformMount::default().mount_adapter(Box::new(adapter), mountpoint, options)`.

#### [HIGH-5.8.2] macOS `default_options` sets `allow_other = true`
**File:** `platform/macos.rs:95-110`
**Detail:** Cross-platform `MountService::mount` re-runs the
`AllowOtherRejected` check so this is not a live bypass via
`MountService::mount`. But any caller of
`MacosPlatformMount::mount_adapter` directly bypasses the veto. Pattern
is error-prone; comment at macos.rs:98-103 itself calls it "platform-
preferred value" which is surprising.
**Fix:** enforce `AllowOtherRejected` inside
`PlatformMount::mount_adapter` default body, not only in
`MountService::mount`.

---

### 5.9 Benchmarks

- `benches/page_cache.rs` — 4 criterion groups
  (sequential_cold_fill_hit, random_1gib, eviction_pressure_256mib,
  concurrent_read_4_threads). Registered in Cargo.toml:59-61.
- `benches/chunked_flush.rs` — 3 chunk sizes (1/4/16 MiB) through a
  100-MiB no-op backend. Registered in Cargo.toml:63-64.

#### [MEDIUM-5.9.1] No CI regression baseline
**File:** `benches/chunked_flush.rs:16-20` (TODO comment).
**Fix:** add `bench-nightly` CI job that runs `cargo bench` and diffs
Criterion's JSON output against a committed baseline.

---

### 5.10 Integration tests

All 10 test files in `tests/`:

- **Kernel-mount (all `#[ignore]` + env-gated, Linux-only):**
  `fuse_mount_integration.rs`, `fuse_kernel_e2e.rs`, `fuse_read_path_live.rs`,
  `fuse_write_path_live.rs`, `fuse_small_write_wiring.rs`,
  `fuse_dyn_shim_write.rs`, `fuse_lifecycle_hardening.rs`.
- **Parser/replay (runs default):** `mount_transport_wiring.rs`,
  `platform_mountinfo_crossplat.rs`, `write_path_replay.rs`.

See CRITICAL-5.6. No FreeBSD / macOS / Windows kernel test exists.

---

### 5.11 FFI safety

**macos_ffi.rs (390 lines):** the sole `unsafe extern "C"` block declares
22 fuse-t symbols (macos_ffi.rs:287-385); struct defs are `#[repr(C)]`
(fuse_args, fuse_file_info, fuse_entry_param, LowlevelOps). **Layout
unvalidated** — see CRITICAL-5.5.

**macos.rs unsafe usage:** every thunk guards with `std::panic::catch_unwind`
(prevents panic across FFI). `adapter_from_req` / `adapter_from_userdata`
carry `# Safety` doc comments (macos.rs:301-333). CString round-trips are
safe (macos.rs:1550-1557 `path_to_cstring` handles interior NULs).
`fuse_add_direntry` call at macos.rs:713-722 bounds the copy with
`remaining = size.saturating_sub(used)` and checks `needed > remaining`.
`fuse_reply_buf` at macos.rs:731 is bounded by `used`.

**windows.rs unsafe usage:** 81 unsafe blocks vs 74 SAFETY comments →
7 bare unsafe blocks. Every callback wraps with `guarded`/`guarded_void`
panic-unwind shims. `pwstr_to_posix_string` (windows.rs:1640-1659)
bounds the NUL-walk at 32 KiB.
`cb_write` slice copy at windows.rs:1019-1020 trusts WinFSP's
`length` (which the WinFSP contract provides correctly, but a SAFETY
comment documenting that contract is present at windows.rs:1016-1017).
`cb_read` copy at windows.rs:1366-1369 bounds `n = data.len().min(length)`.

**winfsp_ffi.rs unsafe usage:** 3 unsafe blocks vs 7 SAFETY comments —
here SAFETY-count exceeds unsafe-count because several SAFETY blocks
annotate nested function-pointer resolution.

#### [HIGH-H.10] 7 bare unsafe blocks in `platform/windows.rs`
**Fix:** audit each; add `// SAFETY:` comments stating invariant (WinFSP
caller-writable buffer, caller lifetime for pointer, etc.).

---

### 5.12 bd-1du.4 "Needed" checklist

Per CLAUDE.md:300-308 the bead's "Needed" list is:

| Need                                          | Status            | Evidence |
|-----------------------------------------------|-------------------|----------|
| real Linux mount/unmount                      | PARTIAL           | Works but live shim misses statfs/access/forget (CRITICAL-5.2) |
| readdir                                       | IMPL              | FuseAdapter::readdir + shim wiring |
| open/read                                     | IMPL              | Correct but no prefetch (HIGH-5.4.1) |
| write/flush/fsync                             | IMPL w/ caveat    | Journal `fsync(file)+fsync(dir)` contract violated (MEDIUM-M.1) |
| setattr                                       | PARTIAL           | size only; chmod/chown/utimens silent lie (MEDIUM-5.2.3) |
| unlink/rename                                 | IMPL w/ caveat    | rename ignores flags (MEDIUM-5.2.4) |
| mkdir/rmdir                                   | IMPL              | — |
| inode/path lifecycle                          | PARTIAL           | forget unwired, map leaks (HIGH-5.2.2) |
| crash-safe writeback                          | PARTIAL           | journal replay never invoked on startup (CRITICAL-5.4) |
| integration tests for mounted-drive behavior  | INSUFFICIENT      | all kernel tests `#[ignore]`d (CRITICAL-5.6) |
| mount policy validation                       | IMPL              | mount_service.rs:111-156 + bsd tightening |
| orphan detection                              | PARTIAL           | Windows is a stub (HIGH-H.4) |
| signal handling                               | PARTIAL           | macOS + Windows missing (HIGH-H.2, H.3); Linux trampoline not AS-safe (MEDIUM-M.5) |
| cross-platform mount                          | INCOMPLETE        | MountService::mount does not reach Windows (HIGH-H.1) |
| WinFSP layout validation                      | MISSING           | VolumeParams unvalidated (CRITICAL-5.1) |
| fuse-t layout validation                      | MISSING           | LowlevelOps unvalidated (CRITICAL-5.5) |

**bd-1du.4 cannot be honestly closed** until at minimum the six CRITICAL
items (5.1-5.6) above are resolved and the two platform-parity vtable
layouts are validated on real hardware.

---

### 5.13 Consolidated remediation priority

**P0 (must-fix before any parity claim):**
1. Validate WinFSP `VolumeParams` against installed `winfsp/fsctl.h` (CRITICAL-5.1).
2. Delete duplicate `BoxedFuserShim`/`FuserShim<A>` in `platform/linux.rs` and route through the shared `platform/fuser_shim.rs` (adds statfs) (CRITICAL-5.2).
3. Stream `upload_file` in 4 MiB chunks (CRITICAL-5.3).
4. Wire `WritePathService::replay_journal` into daemon startup (CRITICAL-5.4).
5. Validate fuse-t `LowlevelOps` via bindgen (CRITICAL-5.5).
6. Enable a privileged CI job that runs `PCLOUD_FUSE_TEST=1 cargo test -p pcloud-fs -- --ignored` (CRITICAL-5.6).
7. Fix `MountService::mount` cfg-ladder to dispatch to Windows (HIGH-H.1).

**P1:**
8. Implement `access`, `forget` in all shims (HIGH-5.2.1, HIGH-5.2.2).
9. Install signal trampolines on macOS and Windows (HIGH-H.2, HIGH-H.3).
10. Implement Windows orphan detection (HIGH-H.4).
11. Fix `WriteJournal::commit` to fsync parent dir (MEDIUM-M.1).
12. Windows `FspCleanupDelete` handling (HIGH-H.11).
13. Log Drop unmount errors (HIGH-H.12).
14. Remove OOM risk in `upload_file` (covered by P0-3).
15. Background read-ahead / prefetch (HIGH-5.4.1).

**P2:**
16. `setattr` honors mode/times or returns `EPERM` (MEDIUM-5.2.3).
17. `rename` flag handling (MEDIUM-5.2.4).
18. Bench regression baseline in CI (MEDIUM-5.9.1).
19. Replace scalar CRC32 with SIMD crate (LOW-L.2).
20. Eliminate in-memory `WritebackJournal` or rename it (HIGH-5.3.1).
21. Route macOS `eprintln!` through `log` (LOW-L.4).

---

### 5.14 Verdict

The crate architecture is sound and the Linux happy-path code is
reviewable. However, **`bd-1du.4` cannot be honestly closed** today:

1. The live Linux mount path dispatches through a DUPLICATE shim copy
   that misses `statfs`, despite the correct shared copy existing.
2. Crash-safe writeback is advertised but the journal replay method has
   no caller on startup.
3. macOS and Windows both ship explicit "NOT YET TESTED" markers, with
   struct layouts that require real-hardware validation before any
   parity claim is credible.
4. Every integration test that actually talks to the FUSE kernel is
   `#[ignore]`-gated; default CI runs zero of them.
5. `MountService::mount` does not reach the Windows back-end at all
   through the primary public entry point.

The remediation list is long but every item has a concrete file+line
reference and a surgical fix. No architectural rewrite is required. The
single highest-leverage action is enabling a privileged CI job that
exercises `PCLOUD_FUSE_TEST=1 cargo test -- --ignored` on every merge.

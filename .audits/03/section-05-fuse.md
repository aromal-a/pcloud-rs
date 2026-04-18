# Section 5: Mounted-drive / FUSE Parity
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 5)

## Scope
Read-only audit of `crates/pcloud-fs/` (`src/` + `tests/` + `benches/`) for
`bd-1du.4` mounted-drive parity. Focus: cross-platform mount seam, FUSE
adapter wiring, write journal / staging durability, RAII teardown, signal
handling, orphan detection, policy enforcement, and read/write integration
tests.

## Findings

### CRITICAL [6]

#### C1 — Write journal violates its own documented `fsync(file)+fsync(dir)` durability discipline
- **Severity:** CRITICAL
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/write_journal.rs:221-227` (`WriteJournal::commit`)
- **Description:** The module docstring (lines 30–45) and the `write_path.rs`
  comment block (`write_path.rs:40-59`, the "Atomic write protocol (P1.2)")
  both guarantee a strict 3-step durability barrier: append → `fsync(file)` →
  `fsync(dir)`. The actual `commit()` implementation only calls
  `self.file.sync_data()` — there is no parent-directory `fsync`. This means
  a newly-created journal segment (or the journal file itself on first write
  when it is created by `OpenOptions::create(true)`) may have its directory
  entry lost after a power cut, silently dropping acknowledged writes. This
  is the classic POSIX `fsync(dir)` requirement and is explicitly called out
  in the docstring as load-bearing for the write-path atomicity proof.
- **Remediation:** Add a helper that, on first create of the journal file
  and periodically (or at least on each `commit` after a size-increasing
  append), opens the parent directory and calls `File::sync_all` on it, as
  is already done in `write_path.rs:905-909` for the `UploadProgress`
  sidecar. Gate behind `fsync_on_commit` like the file-level fsync.

#### C2 — `MountHandle` teardown uses `fuser::BackgroundSession` drop only; no bounded join, no `fuse_session_exit` / loop shutdown coordination on Linux
- **Severity:** CRITICAL
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:664-729` (`LinuxMountHandle::unmount`)
- **Description:** The documented drop discipline on `MountHandle`
  (`mount_service.rs:254-281`) promises a 6-step ordered teardown including
  cooperative shutdown flag, `fuse_session_exit`, `fuse_unmount`, and a
  bounded 5-second join on the dispatcher thread. The macOS path
  (`mount_service.rs:450-496`) implements this. **The Linux path does
  not.** It only calls `drop(self.session.take())` and relies entirely on
  `fuser::BackgroundSession::Drop` semantics — which internally joins the
  background thread with an *unbounded* wait. If a kernel-side FUSE request
  is wedged in the dispatch loop (classic failure mode for a
  poorly-written adapter), `drop` blocks forever, stalling SIGTERM
  handlers, daemon shutdown, and integration tests. The trailing
  `umount2(MNT_DETACH)` escalation only fires after the (potentially
  infinite) drop completes.
- **Remediation:** Wrap `BackgroundSession` drop in a dedicated helper
  thread and use `mpsc::recv_timeout(Duration::from_secs(5))` as the macOS
  path does; if the join times out, escalate directly to `umount2(MNT_DETACH)`
  to release the kernel side and let the background thread exit on its own.
  Alternatively, wire the `fuser` signal channel APIs the crate exposes in
  newer releases.

#### C3 — SIGTERM / SIGINT trampoline uses `libc::signal(2)` instead of `sigaction(2)` and is not async-signal-safe
- **Severity:** CRITICAL
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:600-630` (`install_signal_handler_once`, `signal_trampoline`)
- **Description:**
  1. `libc::signal()` has undefined behaviour across threads on Linux (POSIX
     explicitly warns it is a legacy API with race hazards between setting
     and receiving). `sigaction(2)` with `SA_RESTART` is the correct path.
  2. The trampoline body (`registry().lock()`, `CString::new`) calls
     `pthread_mutex_lock` and the Rust allocator from signal context. Both
     are NOT async-signal-safe — only a whitelist of syscalls (e.g.
     `write`, `umount2`, `_exit`, `raise`) may be called. A concurrent
     pthread holding the `ACTIVE_MOUNTS` Mutex when SIGTERM arrives will
     deadlock the signal handler; a malloc-internal lock held by the same
     thread is worse.
  3. Comment on line 618 claims "umount2 is async-signal-safe", but the
     surrounding `CString::new` and `Vec` iteration are not.
- **Remediation:** Move the registry into a pre-allocated fixed-size array
  of `(sig_atomic_t, [u8; PATH_MAX])` slots written at mount time; the
  handler should only walk the array and invoke `umount2` / `_exit`. Use
  `sigaction` with `SA_RESTART | SA_SIGINFO` and a separate signal-handler
  stack (`sigaltstack`). Alternatively, install a `signalfd` reader thread
  and do the unmount from ordinary thread context — this is the standard
  Rust-on-Linux approach.

#### C4 — `BsdPlatformMount::default_options()` and `WindowsPlatformMount::default_options()` construct `MountOptions` missing three required fields — will not compile on BSD/Windows targets
- **Severity:** CRITICAL
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/bsd.rs:138-145`; `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/windows.rs:174-180`
- **Description:** `MountOptions` (`mount_service.rs:26-53`) now has five
  fields: `read_only`, `fs_name`, `allow_other`, `attr_timeout_secs`,
  `entry_timeout_secs`, `max_readahead`. BSD and Windows both construct
  `MountOptions { read_only, fs_name, allow_other }` using positional field
  initialization — the missing three fields will cause **E0063 missing
  field** compile errors as soon as a BSD or Windows cross-build is run. The
  `cfg`-gating that hides these modules from the Linux CI workspace is
  masking a platform-parity build failure. This directly contradicts the
  crate-level doc claim (`lib.rs:33-36`) "public API type-checks on all
  supported targets".
- **Remediation:** Either add `..MountOptions::default()` to both
  struct-literal expressions or list the three new fields explicitly. Add a
  CI job that runs `cargo check --target x86_64-pc-windows-msvc` and
  `--target x86_64-unknown-freebsd` to prevent regressions.

#### C5 — `MacosPlatformMount::default_options()` sets `allow_other = true`, which `MountService::mount` unconditionally rejects
- **Severity:** CRITICAL
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/macos.rs:95-110`
- **Description:** The macOS default options flip `allow_other = true`
  (with a comment acknowledging that `MountService` vetoes it). But
  `MountService::mount()` is the documented cross-platform entry point and
  will reject any call routed through it with `MountError::AllowOtherRejected`
  (`mount_service.rs:186-188`). A daemon caller who does
  `MacosPlatformMount.default_options()` → `MountService::new().mount(...)`
  gets an immediate error. The comment's "callers that bypass MountService"
  escape hatch means the security policy is only enforced on one of two
  paths — a daemon that uses `ActivePlatformMount` directly (the stated
  cross-platform call pattern in `platform/mod.rs:27-35`) would **silently
  enable world-readable FUSE mounts on macOS**.
- **Remediation:** Either (a) default `allow_other = false` and surface the
  macOS-specific need via a dedicated `MountOptions::macos_defer_permissions`
  field, or (b) make `PlatformMount::mount_adapter` reject `allow_other`
  uniformly on all platforms (not just via `MountService`).

#### C6 — `platform/fuser_shim.rs` is a 39 KiB orphaned file — declared nowhere, never compiled
- **Severity:** CRITICAL
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/fuser_shim.rs` (entire file, 39730 bytes)
- **Description:** The file's docstring claims it is gated at
  `mod fuser_shim;` in `platform/mod.rs`, but `platform/mod.rs:106-113`
  does not declare `fuser_shim`. Verified via grep: no `mod fuser_shim` or
  `pub mod fuser_shim` exists inside `crates/pcloud-fs/src/platform/`. The
  file's `pub(crate)` `BoxedFuserShim` / `FuserShim` types are instead
  re-duplicated inline in `platform/linux.rs:79-587` and `:736-1194`. This
  is:
  - dead code that can diverge from its twin under every refactor,
  - auditor-hostile (reviewers cannot tell which of the two is live),
  - hides the actual "shared Linux+BSD shim" design claimed in `platform/mod.rs`.
  FreeBSD (`platform/bsd.rs`) does not implement `mount_adapter` at all, so
  the claim that the file is "shared between Linux and BSD" is materially
  false.
- **Remediation:** Either (a) delete `platform/fuser_shim.rs` outright and
  keep the two copies inside `platform/linux.rs`, or (b) actually wire
  `mod fuser_shim;` in `platform/mod.rs` with a proper `cfg(any(linux,
  freebsd))` gate, replace the two duplicates in `linux.rs` with
  `use platform::fuser_shim::{BoxedFuserShim, FuserShim};`, and implement
  `mount_adapter` on `BsdPlatformMount` so the "shared" claim is true.

### HIGH [9]

#### H1 — `FuserShim`/`BoxedFuserShim` `forget(ino, nlookup)` is stubbed — inode map grows unbounded
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:471-475, 1063-1067`
- **Description:** Both shim copies implement `forget()` as a pure
  `log::trace!` with the comment
  `TODO(bd-fuse): inode map cleanup not yet implemented`. Meanwhile
  `InodeTable::insert_or_get` (used by every `lookup`/`readdir`/`create`)
  only ever grows `by_ino` / `by_path`. A long-lived mount listing many
  directories will retain every inode ever observed in process memory until
  unmount. For a 10 M-file drive this is hundreds of MiB of wasted memory
  and, more importantly, there is no way to invalidate a stale path/ino
  binding. This is the exact "stale kernel handle" failure mode the
  `invalidate_path` + generation-counter mechanism in `inode.rs` was
  designed to prevent.
- **Remediation:** Wire `adapter.forget_ino(ino, nlookup)` on the
  `FuseAdapter` trait (the surface is already reserved in the trait doc at
  `fuse_adapter.rs:541-544`), have it decrement a per-ino kernel lookup
  refcount and `invalidate_path` when the refcount reaches zero.

#### H2 — `readdir` does not respect `fuser::ReplyDirectory::add` full-buffer return correctly; always replies `ok` on buffer-full
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:262-273, 851-862`
- **Description:** `reply.add(...)` returns `true` when the caller's buffer
  is full. The loop body correctly `break`s in that case, but immediately
  falls through to `reply.ok()` at line 273/862 — which sends the
  already-built buffer ending exactly at the *full* marker. The kernel
  then calls `readdir` again with `offset = last_next`, which on the next
  pass re-sends the `.`/`..` stanza only if `offset == 0`. Because the
  code stops emitting `.`/`..` the moment offset > 0 this is arguably
  correct, **but** the `next += 1` increments are done **before** `reply.add`
  determines fullness for the `.`/`..` pair (lines 246-260, 839-849). On a
  buffer-full case for `..` specifically, the next offset the kernel
  supplies is `offset+2` which skips the first real entry. Net: with a
  pathologically small readdir buffer (or at the boundary of a large
  directory), the first child may be silently dropped.
- **Remediation:** Restructure the offset cursor so `next` is only
  advanced when `reply.add` returns `false`. Better: switch to the
  `ReplyDirectoryPlus` API with an explicit "remaining" idiom, matching
  the Linux fuse reference implementation.

#### H3 — `mount_service::MountOptions` lacks a way to pin the FUSE adapter's uid/gid separately from the mounting user
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_service.rs:27-53`
- **Description:** The option struct has no `uid`/`gid` fields. Every file
  on the mount is owned by whoever called `AdapterOptions::default()`
  (`fuse_adapter.rs:739-761`), which captures `libc::getuid()` at adapter
  construction time. For a daemon running as `root` but mounting for user
  `alice`, every file appears as `root:root` with mode 0644 (`default
  file_mode`) — a **privilege-drop issue** because Alice cannot write her
  own pCloud files through the mount. The kernel permission check (with
  `DefaultPermissions` mount flag) then refuses `alice`'s writes.
- **Remediation:** Add `uid: Option<u32>, gid: Option<u32>` to
  `MountOptions` (or a dedicated `FileOwnership` struct), wire it through
  `mount_adapter` into `AdapterOptions`, and document the "daemon runs as
  root, mount is owned by target user" path.

#### H4 — No file/directory lock around the `fuser::spawn_mount2` invocation; race between orphan-detection and mount
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:1222-1245, 1249-1271`
- **Description:** `mount_with_fuser` and `mount_fuser_filesystem` call
  `install_signal_handler_once` then `fuser::spawn_mount2`, then insert
  into `ACTIVE_MOUNTS`. Two daemon threads racing to mount the same path
  will both pass `validate_mountpoint` (which does not hold a lock across
  the mount syscall), both call `spawn_mount2`, and the loser's
  `spawn_mount2` returns an `EBUSY` error that bubbles up as a generic
  `MountError::Fuser`. More importantly, the `mountpoint_is_already_mounted`
  check in `mount_orphan.rs` is called by the daemon separately, with a
  TOCTOU window between the check and the actual mount.
- **Remediation:** Acquire a filesystem lock (`flock(2)` on a
  `$runtime_dir/pcloud-mount.lock` file) before validating; hold it across
  the full `validate → check-orphan → spawn_mount2 → registry-insert`
  sequence.

#### H5 — `MountService::mount` does not call `mount_orphan::mountpoint_is_already_mounted` before delegating
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_service.rs:180-235`
- **Description:** The cross-platform mount entry point validates
  ownership, emptiness, and `allow_other`, but never calls
  `mountpoint_is_already_mounted`. BSD's `validate_mountpoint`
  (`bsd.rs:92-97`) does check, but Linux's `validate_mountpoint`
  (`linux.rs:53-55`) delegates to `MountService::validate_mountpoint`
  which does not. An operator mounting on top of a leftover
  `fuse.pcloud` mount from a crashed prior daemon will **shadow** the
  original — the kernel happily stacks FUSE mounts. Orphan-detection
  exists in `mount_orphan.rs` but the mount service does not invoke it.
- **Remediation:** In `MountService::mount`, after the empty-dir and
  permission checks, call `mount_orphan::mountpoint_is_already_mounted`
  with `ProcMountinfoReader` and reject with a new
  `MountError::MountpointAlreadyMounted` variant if any mount exists at
  the target.

#### H6 — Page cache documented `Arc<Vec<u8>>` value sharing is not implemented; returns `Vec::clone`
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/page_cache.rs:129-132, 220-230`
- **Description:** The module docstring lines 24-40 spend 20 lines of
  rationale on `Arc<Vec<u8>>` storage and the 3-orders-of-magnitude
  speedup of `Arc::clone` vs memcpy. The actual implementation stores
  `struct Slot { bytes: Vec<u8> }` (`line 129-132`) and `get` does
  `let bytes = slot.bytes.clone();` (`line 223`) — a full memcpy of up
  to 64 KiB under the Mutex on every cache hit. The P5.1 performance
  promise is unmet. Benchmarks in `benches/page_cache.rs` will report
  numbers that do not reflect the design.
- **Remediation:** Change `Slot { bytes: Arc<Vec<u8>> }`, return
  `Option<Arc<Vec<u8>>>` from `get`, and update callers
  (`fuse_adapter.rs:1428` etc.) to handle the `Arc` return type.

#### H7 — Inode table invalidation still O(n) due to VecDeque LRU (inherited from page_cache pre-P1.1 comment)
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/page_cache.rs:157-163, 243-246`
- **Description:** The doc at `page_cache.rs:42-56` claims "LinkedHashMap
  O(1) eviction (P1.1) ... Earlier revisions used a separate VecDeque of
  keys for the LRU order, which made eviction O(n) due to the
  mid-vector removal on every `get`. P1.1 replaced that with the
  intrusive list; benchmarks showed a 40-60× speedup". **But the actual
  type is still `order: VecDeque<PageKey>`** (line 139), and
  `Inner::touch` does `self.order.iter().position(|k| k == key)` + remove
  — exactly the O(n) pattern the doc claims was removed. The P1.1
  performance claim and the benchmarks fuel a false parity story.
- **Remediation:** Actually implement the intrusive list promised by
  the doc, or downgrade the doc to match reality. Given the complexity,
  pulling `linked_hash_map` or `indexmap` is the pragmatic answer.

#### H8 — Errno constants in `errors.rs` are hard-coded Linux values — will be wrong on BSD / macOS
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/errors.rs:13-26`
- **Description:** `ENOENT = 2; EIO = 5; EACCES = 13; ENOTDIR = 20; EINVAL
  = 22; EROFS = 30;`. On FreeBSD, `ENOTDIR = 20` but other values differ
  (e.g., `EAGAIN = 35` vs Linux `EAGAIN = 11`). `EROFS = 30` is Linux-
  specific; BSD has `EROFS = 30` too but `EMFILE`, `ENFILE`, `EDQUOT`
  (which the trait docs promise for various ops) have wildly different
  numeric values across platforms. Since `pcloud-fs/src/errors.rs:22` is
  used by `write_path::WritePathError::to_errno` and fed straight into
  `fuse_reply_err(req, errno)` on both Linux (`linux.rs:214`) and macOS
  (`macos.rs:466`), a cross-platform test would see confused errno
  translation on the non-Linux path.
- **Remediation:** Replace the consts with `pub use libc::{ENOENT, EIO,
  EACCES, ENOTDIR, EINVAL, EROFS};` and rely on the `libc` crate to pick
  the correct per-platform values.

#### H9 — Staging dir permission verification has a TOCTOU hole
- **Severity:** HIGH
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/staging.rs:57-65, 224-254`
- **Description:** `StagingDir::open` calls `create_secure_dir` (which
  tightens perms) and then `verify_secure_dir` (which reads perms and
  checks mask). Between the `set_permissions` and the second `metadata`
  call, a concurrent process could `chmod` the directory to `0o777`.
  The check passes because we read after the (racing) widen. The window
  is small but nonzero; for a secret-bearing staging area this is worth
  closing. Additionally, `create_secure_dir` uses `path.exists()` +
  `builder.create()` — classic symlink-attack window: if an attacker
  replaces the directory with a symlink to `/tmp/evil/` between the
  `exists()` check and `set_permissions()`, the attacker's directory
  gets `0o700`'d but our subsequent blob writes land in the attacker's
  path.
- **Remediation:** Open the directory with `O_DIRECTORY | O_NOFOLLOW`,
  read permissions via `fstat` on the FD, and reject if not `0o700`.
  Alternatively, use `openat2(2)` with `RESOLVE_NO_SYMLINKS` on Linux.

### MEDIUM [11]

#### M1 — Duplicate copy of `BoxedFuserShim` and `FuserShim` code in `linux.rs` (500+ lines duplicated)
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:90-587, 744-1194`
- **Description:** Two near-identical filesystem shims live in the same
  file (one owns a `Box<dyn FuseAdapter>`, the other is generic `<A>`).
  Everything from `statfs` through `rmdir` is copy-pasted. Bug fixes
  (e.g., the `forget` issue in H1) must be applied twice or drift.
- **Remediation:** Extract the shared methods into a private helper
  trait or macro. Alternatively, delete `FuserShim<A>` and route
  `mount_with_fuser<A>` through `mount_adapter(Box::new(adapter), ...)`.

#### M2 — `SESSION_DROP_SETTLE_WINDOW` hard-coded to 2 s; no override for slow hosts
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:645`
- **Description:** A 2-second settle loop with 25 ms poll interval (line
  683) works on most hosts, but a contended NFS-over-FUSE stack or a
  busy dispatcher can exceed 2 s. The constant is private, so operators
  cannot tune it via config. The escalation to `umount2(MNT_DETACH)`
  handles the case, but the log output gives no hint to operators that
  they are hitting the fallback repeatedly.
- **Remediation:** Expose via `MountOptions::unmount_timeout`, default 2
  s; emit a `log::warn!` when the fallback triggers so operator
  dashboards can alert.

#### M3 — `chunked_flush` progress sidecar renamed-onto-target without a dedicated `fsync(dir)` for the *staging root* after the rename
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/write_path.rs:880-912`
- **Description:** `UploadProgress::save` does the correct write-then-
  rename-then-fsync-dir pattern (lines 903-909). However, it opens the
  parent via `std::fs::File::open(parent)` and only lets the `sync_all`
  error fall through as a `let _ = …` — any I/O error is silently
  ignored. On an ENOSPC or transient EIO, the sidecar may be visible
  but the directory entry not durable; a crash then loses the
  acknowledged chunk.
- **Remediation:** Propagate the `sync_all` error back as
  `WritePathError::Upload` so the write-path caller can distinguish
  "bytes unstable on disk" from "bytes stable".

#### M4 — `WritebackService::flush_threshold_bytes` doc/default conflicts with `WritePathOptions::DEFAULT_FLUSH_THRESHOLD_BYTES`
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/writeback.rs:30` (4 MiB) vs `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/write_path.rs:243` (64 MiB); `lib.rs:15-22` says 64 MiB
- **Description:** Two flush threshold constants in the same crate with
  values differing by 16×. The `lib.rs` doc block advertises 64 MiB.
  The legacy `WritebackService` in `writeback.rs` defaults to 4 MiB.
  `FilesystemShell` (the scaffold) uses `WritebackService`, so the
  public summary string "`flush_threshold=4096KiB`" (`lib.rs:162`) will
  not match the production 64 MiB threshold. Docs and tests will
  diverge silently.
- **Remediation:** Delete `writeback.rs`'s redundant threshold field,
  import `DEFAULT_FLUSH_THRESHOLD_BYTES` from `write_path.rs`. Or, if
  the scaffold must stay for test coverage of the in-memory pathway,
  unify the constants.

#### M5 — No `.` / `..` / embedded `/` name validation in `FuserShim::rename`; relies on `join_child`
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:523-535`
- **Description:** `BoxedFuserShim::rename` does `Self::join_child(&parent,
  name)` which rejects empty/NUL/slash. It does **not** reject `"."` /
  `".."`. A rename of `foo` → `..` or `.` would succeed at the shim
  layer and flow into the adapter, which then tries to construct a
  remote path with a traversal component. Since `path_norm::canonicalise`
  is called later (`fuse_adapter.rs:1611`), this is intercepted at
  `PathError::InvalidComponent`, but the error surface is `EINVAL`
  rather than `EISDIR` or `EEXIST` which POSIX mandates.
- **Remediation:** Reject `name == "."` / `name == ".."` in
  `join_child` itself. Same for `create`, `mkdir`, `unlink`, `rmdir`.

#### M6 — macOS signal trampoline is not implemented; log::warn on every mount
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/macos.rs:235-243`
- **Description:** The macOS mount path emits a
  `log::warn!("macOS signal trampoline for graceful unmount is not yet
  implemented")` on every mount. A SIGTERM leaves a stale mount that
  requires `umount -f`. This is visible operator-facing damage. Linux
  has the trampoline (albeit with the C3 bugs above); macOS has none.
- **Remediation:** Port the signal-handler pattern from `linux.rs`
  after C3 is fixed; use `pthread_kill` to break the session loop since
  `fuse_session_exit` is documented cross-thread safe.

#### M7 — `mount_orphan::fusermount_unmount` uses 50 ms busy-poll instead of `SIGCHLD` / `waitpid` with a pipe
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_orphan.rs:268-293`
- **Description:** `spawn_and_wait` polls `child.try_wait()` every 50
  ms. For a typical `fusermount3 -u` that completes in <100 ms, this
  adds up to one extra poll of latency; for a timeout-bound (say 10 s)
  it churns 200 times. Not a correctness issue but wasted wakeups on
  systems where every watt matters. The same pattern reappears in the
  Linux `unmount` settle loop (linux.rs:683).
- **Remediation:** Use a `popen`-style `Stdio::inherit()` with
  `child.wait_timeout()` from the `wait-timeout` crate, or fall back
  to a `pipe(2)` closed on child exit.

#### M8 — `WinFspLibrary` and the WinFSP FFI thunks are unaudited — `winfsp_ffi.rs` 23 KiB of hand-rolled C ABI
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/winfsp_ffi.rs` (entire file)
- **Description:** The file implements `NTSTATUS`, `FSP_FSCTL_VOLUME_PARAMS`,
  `FSP_FILE_SYSTEM_INTERFACE`, et al. from scratch (no winfsp crate).
  The crate `Cargo.toml` explicitly avoids the `winfsp` crate
  (lines 42-46). None of this has been cross-checked against
  `winfsp/fsctl.h` on a Windows build, per the Phase-3 disclaimer
  (`windows.rs:5-28`). Struct-layout bugs here are undetectable on
  Linux CI; they will only manifest as silent memory corruption on the
  first Windows deployment. Given the earlier parity fork
  (`windows.rs:178` — missing fields on `MountOptions`), the surface is
  likely riddled with drift already.
- **Remediation:** Block `bd-1du.4` proof gate on (a) compiling for
  `x86_64-pc-windows-msvc`, (b) bindgen-generating one reference header
  and diffing against this hand-roll, (c) running at least the
  smoke-test mount/unmount path on a real Windows + WinFSP host.

#### M9 — `read_path::ReadPathService` (the scaffold) and `ProtoFuseAdapter::read` have divergent page-cache semantics
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/read_path.rs`, `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/fuse_adapter.rs:1385-1472`
- **Description:** The 4.a scaffold `ReadPathService::read` uses a
  `prefetch_window_bytes` model and serves from `staging` + `page_cache`;
  the production `ProtoFuseAdapter::read` uses page-indexed LRU. They
  have completely different hit-ratio behaviour. Consumers (tests,
  CLI diagnostics like `FilesystemShell::summary`) conflate the two.
- **Remediation:** Mark `ReadPathService` `#[deprecated]` or scope it
  visibly as test-only.

#### M10 — `statfs` default for unknown `ino` returns `1 TiB total / 500 GiB free` without querying pCloud userinfo quota
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:160-198, 753-791`
- **Description:** The Linux `statfs` impl tries a host `statvfs64` on
  the mountpoint path and, failing that, returns hard-coded 1 TiB /
  500 GiB sentinels. `df -h /mnt/pcloud` will cheerfully report 500
  GiB free even when the user's pCloud account is at 99% of a 10 GiB
  plan. The `FuseAdapter::statfs` trait method (`fuse_adapter.rs:503-
  505`) exists exactly for this but is defaulted to `ENOSYS` and never
  wired in `ProtoFuseAdapter`. The shim uses `statvfs` on the local
  FS — which is wrong: that reports the `/tmp`-style host FS beneath
  the mount, not the pCloud quota.
- **Remediation:** Implement `FuseAdapter::statfs` on `ProtoFuseAdapter`
  using the pCloud `userinfo.quota` / `userinfo.usedquota` fields;
  route `BoxedFuserShim::statfs` through the trait instead of doing a
  host `statvfs64`.

#### M11 — `fs_watcher.rs` (22 KiB) is not reviewed here — unclear whether it is wired into the live FUSE runtime
- **Severity:** MEDIUM
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/fs_watcher.rs`
- **Description:** The file is in the module tree
  (`lib.rs:51`) but I did not audit its 22 KiB body in this pass. Its
  relationship to the mount runtime (does `ProtoFuseAdapter` consume
  it? Is it used only by sync?) is not documented in the module
  preamble I skimmed. An orphaned watcher that claims to drive
  cache-invalidation but is not called is a common failure mode.
- **Remediation:** Out of scope for this audit; flag for follow-up by
  Agent 6 or bd-1du.10 proof gate.

### LOW [6]

#### L1 — `#[deny(missing_docs)]` but root module has no `//!` header for the `pub mod fs_watcher` / `integrity_sweeper` lines
- **Severity:** LOW
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/lib.rs:51, 59`
- **Description:** `deny(missing_docs)` + `lib.rs` exposes `pub mod
  fs_watcher;` and `pub mod integrity_sweeper;` whose crate-level
  documentation I did not verify. Should be audited alongside M11.

#### L2 — Test binary gating with env var `PCLOUD_FUSE_TEST=1` silently passes on CI hosts without FUSE
- **Severity:** LOW
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/tests/fuse_mount_integration.rs:36-39, 69-76`
- **Description:** Every live-mount test body has
  `if !fuse_gate_enabled() { return; }`, which **silently passes** the
  test (marking it green) when FUSE is absent. Combined with
  `#[ignore = "..."]`, the test is ignored by default and passes if
  forced. A CI matrix that runs `cargo test --ignored` on a non-FUSE
  host happily reports success without running a single assertion.
- **Remediation:** Return a
  `Err(io::Error::new(io::ErrorKind::Unsupported, "PCLOUD_FUSE_TEST=1 required"))`
  that `#[should_panic(expected = "...")]` can match, or use Rust
  1.70's experimental `test::ShouldPanic::ExpectedIncludingBacktrace`
  harness. At minimum, emit `eprintln!` visible with `cargo test --nocapture`.

#### L3 — `page_cache::DEFAULT_MAX_BYTES = 128 MiB` — no per-mount override from `MountOptions`
- **Severity:** LOW
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/page_cache.rs:71`
- **Description:** An operator running two simultaneous mounts gets 256
  MiB resident. For a low-memory deployment (512 MiB container) this
  OOMs the daemon. The field exists on `AdapterOptions::page_cache`
  (`fuse_adapter.rs:736`) but is not plumbed through `MountOptions`.
- **Remediation:** Expose via `MountOptions::page_cache_bytes: Option<u64>`.

#### L4 — Journal file mode tightening is `0o600` per-open, not `0o700` for the parent — redundant with `staging.rs` but worth explicit contract
- **Severity:** LOW
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/write_journal.rs:319-334`
- **Description:** `open_journal_file` both
  `.mode(0o600).open(path)` and immediately
  `std::fs::set_permissions(path, 0o600)`. The second call is
  a belt-and-braces guard for the case where the file pre-exists
  with `0o644`. Documented, but on a paranoid system one would
  add `O_NOFOLLOW` to defeat a symlink race between open and chmod.
- **Remediation:** Use `OpenOptionsExt::custom_flags(libc::O_NOFOLLOW)`.

#### L5 — `validate_mountpoint` does not check that the parent directory is owned by the current user
- **Severity:** LOW
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_service.rs:136-177`
- **Description:** We verify mountpoint ownership but not the parent's.
  An attacker with write access to the mountpoint's parent can
  `rmdir` the mountpoint and substitute a symlink between
  `validate_mountpoint` and `spawn_mount2`. Narrow window, but
  documented to be tightened under H4.

#### L6 — Criterion bench `eviction_pressure_256mib` allocates 256 MiB per iteration — may OOM CI runners
- **Severity:** LOW
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/benches/page_cache.rs:117-146`
- **Description:** `sample_size(10)` reduces the count but each sample
  writes 256 MiB of fresh `Vec<u8>`. A 4 GiB GitHub runner is fine; a
  1 GiB container is not. Flag as operational, not a correctness
  bug.

## Summary of Known FUSE Gaps vs "Fully Wired" Linux Mount

Even assuming all critical/high items are fixed:

- **inode map GC not implemented** (H1). Unlimited memory growth over long mounts.
- **`statfs` not wired to pCloud quota** (M10). Wrong `df` output.
- **No `fsync(dir)` on journal create** (C1). Core durability promise unmet.
- **Signal trampoline has async-signal-safety bugs** (C3). SIGTERM may deadlock or UAF.
- **Cross-platform `MountOptions` field drift** (C4). BSD/Windows builds break.
- **macOS default `allow_other = true`** (C5). Security policy leak.
- **Page-cache fast-path cloning memcpy** (H6). Documented perf claim unmet.
- **WinFSP FFI never compiled on Windows** (M8).

All eight of these must land or be explicitly rejected + documented
before `bd-1du.4` is closed and `bd-1du.10` can gate "mounted-drive
parity" honestly. The current state is "scaffolding, Linux read path
works on a happy-path host, write path is durable-on-paper but
durable-in-code for only 2 of 3 required fsyncs."

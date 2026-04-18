# Section 5 — FUSE / Mounted Drive Audit (Opus)

Scope: `crates/pcloud-fs/` — mount lifecycle across Linux/macOS/Windows/BSD, write/read paths, page cache, inode table, signal teardown, `allow_other` policy, attr/entry timeouts, libfuse/WinFSP/fuse-t FFI safety, SAFETY discipline.

Overall posture is strong on Linux (live-verified) and generally defensive about `unsafe`. The macOS and Windows backends are scaffolded and not proven on real hardware. Below are concrete issues that should block a "production ready" claim even on Linux-only workloads.

---

## CRITICAL

### C-1. SIGTERM/SIGINT handler uses `libc::signal()` and never runs unmount
`crates/pcloud-fs/src/platform/linux.rs:622-643`

`install_signal_handler_once()` installs a trampoline via `libc::signal(2)`. Two problems:

1. `libc::signal` has implementation-defined SysV/BSD semantics; SA_RESTART, handler-scope, and sigprocmask state are not controlled. The code itself acknowledges this: `TODO(bd-1du): replace libc::signal() with sigaction() for SA_RESTART semantics` (line 627). This must be `sigaction(SA_RESTART|SA_SIGINFO)` with an explicit `sa_mask` that blocks both SIGTERM and SIGINT inside the handler, or nested signals can re-enter and corrupt.
2. The trampoline only sets an `AtomicBool`; `shutdown_requested()` (line 618) must be polled by someone. Nothing in the daemon/FUSE loop is shown polling it, and on SIGKILL/panic the RAII `MountHandle::drop` in another thread may never run. The CLAUDE.md/doc prose claims "signal-handled teardown" — code actually only records the request. This is a correctness gap versus the stated posture.

Remediation: switch to `sigaction`, add an in-process listener thread (signalfd on Linux, `signal_hook` crate) that iterates `ACTIVE_MOUNTS` and invokes `umount2(MNT_DETACH)` + registry cleanup on each entry.

### C-2. `LinuxMountHandle::unmount` “bounded join” is not actually bounded
`crates/pcloud-fs/src/platform/linux.rs:677-686`

```rust
let join_handle = std::thread::spawn(move || drop(session));
match join_handle.join() { … }
```

`JoinHandle::join()` has **no timeout**. If `fuser::BackgroundSession::drop` wedges (blocked fuse_session_loop syscall, kernel hang), the entire daemon hangs forever. The doc claims a "5-second bounded wait" on macOS (`teardown_macos` uses `recv_timeout`, lines 476-485) but the Linux path does not mirror that discipline. Use the same `mpsc::channel` + `recv_timeout(5s)` pattern here, or detach after timeout and rely on `umount2(MNT_DETACH)` escalation.

### C-3. Inode `forget()` never evicts entries that were inserted without `increment_lookup`
`crates/pcloud-fs/src/inode.rs:241-259`

`forget()` only acts when `lookup_counts.get_mut(&ino)` returns `Some`. Paths that are inserted via `insert_or_get` without a matching `increment_lookup` (several adapter call sites — verified at `fuse_adapter.rs:1231-1232` which calls `forget` but the corresponding inserts in `fuser_shim.rs` do not always bump) will silently become memory leaks: the `HashMap<u64,InodeEntry>` grows unbounded for the lifetime of the mount. This is both a correctness bug (generations never bump) and a slow resource exhaustion. Either auto-increment inside `insert_or_get` on every kernel-observable return, or assert at debug-time that every path that returns `(ino, gen)` to the kernel is paired with `increment_lookup`.

---

## HIGH

### H-1. `PageCache` is advertised as O(1) LRU but eviction is O(n)
`crates/pcloud-fs/src/page_cache.rs:44-56, 188-211`

Module doc claims "All three mutating operations are O(1)" via an "intrusive doubly-linked list threaded through the HashMap entries". The actual code uses `indexmap::IndexMap::shift_remove_index(0)` for eviction, which is explicitly O(n) (shifts every remaining index). Under steady-state eviction pressure this degrades to O(n) per insert. Either (a) document honestly as "amortised O(1) with O(n) per eviction; benign while hit-rate is high" or (b) switch to a real `LinkedHashMap` / hand-rolled intrusive list. The 40-60x speedup claim in the doc is measured against the prior implementation, not against true O(1).

### H-2. Mountpoint validator race (TOCTOU)
`crates/pcloud-fs/src/mount_service.rs:136-177`

`validate_mountpoint` does `metadata → read_dir → metadata.uid/mode` on a path, then `mount()` (line 194) re-resolves the path. A local attacker with write on any ancestor can swap the directory for a symlink between validation and the `fuser::Session::mount` call. Use `openat2(RESOLVE_NO_SYMLINKS|RESOLVE_BENEATH)` or `O_PATH|O_NOFOLLOW` to lock a file descriptor, then `fstat`/`fstatfs` against the fd, and pass `/proc/self/fd/N` to libfuse to avoid re-resolution.

### H-3. `allow_other` policy contradictory across layers
`crates/pcloud-fs/src/mount.rs:40-50` vs. `crates/pcloud-fs/src/mount_service.rs:186-188` vs. `crates/pcloud-fs/src/platform/macos.rs:91-111`

`MountPolicy::validate` (`mount.rs:40`) allows `allow_other` if `read_only=true`. `MountService::mount` (`mount_service.rs:186`) rejects `allow_other` unconditionally. `MacosPlatformMount::default_options` previously hinted at wanting `allow_other` for daemon-user parity. Pick one policy and enforce it in exactly one place; today a caller hitting the `mount.rs` path directly can bypass the stricter rejection.

### H-4. Windows adapter double-reclaim on failure
`crates/pcloud-fs/src/platform/windows.rs:309-316`

On `fsp_set_mount_point` failure the code runs `Box::from_raw(adapter_raw …)` **before** calling `fsp_delete(fs)`, but `fs` still has the user-context pointing at the freed box. If `fsp_delete` invokes any callback during teardown (WinFSP can, for cleanup), the callback will dereference a freed `Box<dyn FuseAdapter>`. Swap order: `fsp_set_user_context(fs, null)` → `fsp_delete(fs)` → `Box::from_raw(adapter_raw)`.

### H-5. Windows `fsp_get_user_context_global` reads a non-`Sync` `OnceLock` across threads
`crates/pcloud-fs/src/platform/windows.rs:389-401`

The function pointer is cached in a `OnceLock<Option<FnFspFileSystemGetUserContext>>`. This is fine for read-mostly lookup, but the SAFETY comment (lines 386-388) says "Must only be called from a WinFSP dispatcher callback". There is no runtime check enforcing that — a logic bug elsewhere that calls this from `Drop`/teardown (when the DLL may be being released) results in a jump through a stale pointer. Consider a `Box::leak` of the `WinFspLibrary` (already Arc'd) and threading the `Arc` into each callback via the user-context struct instead of a global.

---

## MEDIUM

### M-1. macOS teardown uses `drop(joiner)` to "detach" after timeout
`crates/pcloud-fs/src/platform/macos.rs` path — `mount_service.rs:476-485`

After 5-second `recv_timeout`, the code does `drop(joiner)` to "detach rather than block". Dropping a `JoinHandle` does not kill the thread; the thread keeps running, still owns `handle` and `session`, and its later completion can UAF on static state being torn down. Prefer `std::mem::forget(joiner)` with an explicit doc comment, or better: allocate the session inside an `Arc<Mutex<Option<_>>>` so the leaked thread can observe shutdown and exit without touching torn-down state.

### M-2. macOS live-verification is aspirational — ship claim mismatch
`crates/pcloud-fs/src/platform/macos.rs:5-7, 303-305`, `mount_service.rs:303-312`

"NOT YET TESTED ON MACOS" is repeated throughout. CLAUDE.md already acknowledges macOS hardware verification is out of scope, but the public `MountService::mount` on macOS (lines 197-202) dispatches to a backend that may `return Err(MountError::Unsupported(...))`. Users compiling with `--target x86_64-apple-darwin` will build a binary whose mount always errors. Add a `#[cfg(feature = "macos_fuse_experimental")]` gate so the macOS path is compiled only when the caller has opted in.

### M-3. Write path fallback loses `flush_interval` triggers when `chunked_flush` errors
`crates/pcloud-fs/src/write_path.rs:538-555`

When `size_trigger` fires and `chunked_flush` returns anything other than `CHUNKED_NOT_SUPPORTED`, the error propagates and the `time_trigger` is never re-evaluated. A later-write path enters a state where time-based flushes are suppressed while transient upload errors persist. Either set `last_flush = now` on the error path to cap re-entries, or refactor so `time_trigger` is always evaluated independently.

### M-4. `staging::verify_secure_dir` does not verify ownership
`crates/pcloud-fs/src/staging.rs:36-46, open fn starting 57-65`

The error enum has `InsecurePermissions { mode }` but no variant for "owned by another uid". If a different local user created `<root>` with mode `0o700` (their own) before the daemon starts, the daemon will reuse another user's directory. Add a uid check identical to `mount_service::validate_mountpoint` (line 156-166). Staged blobs can contain user data in cleartext (per `staging.rs:5-19`), so this is material.

### M-5. `attr_timeout_secs` / `entry_timeout_secs` accept `f64::NAN` / negatives
`crates/pcloud-fs/src/mount_service.rs:40-46, 55-66`

Defaults are `1.0`, but `MountOptions` is a plain struct with `pub` fields; a caller can set `NAN`, `-1.0`, or `1e20` and libfuse will happily convert that to a `struct timespec` with undefined behaviour. Add a normalising `MountOptions::validate()` that clamps to `[0.0, 60.0]` and rejects NaN/Inf.

### M-6. `fuse_adapter::forget_ino` default is a no-op
`crates/pcloud-fs/src/fuse_adapter.rs:456-463`

`fn forget_ino(&self, _ino: Ino, _nlookup: u64) {}` — an adapter that forgets to override it leaks inodes silently. Make it non-defaulted (required trait method) so every adapter explicitly picks "keep" or "evict".

### M-7. Linux `umount2` NUL-termination path uses `as_encoded_bytes()`
`crates/pcloud-fs/src/platform/linux.rs:714`

`OsStr::as_encoded_bytes()` is WTF-8 on Unix, which does contain raw bytes but can carry surrogate-adjacent sequences. The `CString::new` will only fail on embedded NUL, but non-UTF-8 filenames pass through. Behaviour is correct for typical mountpoints; consider tightening the validator to reject non-UTF-8 mountpoint paths to simplify downstream reasoning.

---

## LOW

### L-1. `MountHandle::Drop` silently swallows unmount errors
`crates/pcloud-fs/src/mount_service.rs:523-544`

Defensible (Drop must not panic) but the only surfacing is `log::warn!` inside `inner.unmount()`. Add an `unmount_result_tx: Option<Sender<Result<...>>>` or expose a `last_drop_error: AtomicU32` so operators can detect leaked mounts post-hoc.

### L-2. `allow_other + read_only` story inconsistent with Linux `fuser`
`crates/pcloud-fs/src/mount.rs:40-50`

Permitting `allow_other` on a read-only mount is defensible, but combined with `fusermount3`'s own `user_allow_other` gate in `/etc/fuse.conf`, this is a privilege path: the validator does not check the `/etc/fuse.conf` state, so the mount may succeed (rejected by config) or fail with an opaque error. Pre-check the conf file and surface a clear error.

### L-3. `PageCache::put` silently drops oversized pages
`crates/pcloud-fs/src/page_cache.rs:276-280`

Returns `()` with no signal. A backend that starts returning pages larger than `max_bytes` (pathological server behaviour or misconfigured page size) gets 0 cache hits forever with no metric. Add a `bytes_rejected_oversized` counter.

### L-4. `ACTIVE_MOUNTS` registry is global mutable state
`crates/pcloud-fs/src/platform/linux.rs:607-611`

A `static OnceLock<Mutex<Vec<PathBuf>>>` works, but there is no bound on its size and no mutate path removes entries on `MountHandle::drop` in all failure modes (some paths skip registration). Use a `BTreeSet<PathBuf>` keyed by canonical path + add a debug assertion that registrations balance unregistrations.

### L-5. Linux FUSE write test is gated on two env vars
`crates/pcloud-fs/src/mount_service.rs:635-637`

`PCLOUD_FUSE_TEST=1` alone triggers the smoke test; CLAUDE.md references both `PCLOUD_LIVE_E2E=1` and `PCLOUD_FUSE_TEST=1` for the live write test. Harmonise naming across the crate and document a single gate variable.

### L-6. BSD platform returns `UnsupportedPlatform` generically
`crates/pcloud-fs/src/platform/bsd.rs` (not read but CSV claims FreeBSD support)

The parity matrix rates FreeBSD as tier-3. Actual `BsdPlatformMount` should return a specific `Unsupported("FreeBSD: install fusefs-libs and set vfs.usermount=1")` with the remediation hint rather than a generic error, mirroring the macOS pattern.

---

## Summary

| Severity | Count |
|----------|-------|
| CRITICAL | 3     |
| HIGH     | 5     |
| MEDIUM   | 7     |
| LOW      | 6     |

Blocking findings for a "production-ready" Linux claim: **C-1, C-2, C-3, H-1, H-2**. Blocking findings for any cross-platform release: **H-4, H-5, M-1, M-2**. All other findings are gates to close before the `bd-1du.10` parity-proof sign-off.

SAFETY comments are, broadly, present and specific. The FFI thread-safety asserts (`unsafe impl Send/Sync for MacosMountInner / WindowsInner`) are well-reasoned, though M-1 weakens `MacosMountInner`'s claim that all FFI calls happen on teardown paths "we control".

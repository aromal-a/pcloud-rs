# pcloud-rs Mounted-Drive / FUSE Parity Audit

Scope: `crates/pcloud-fs` mounted-drive/FUSE implementation, with visible daemon wiring where needed. I read `pcloud_rev.md` and did not intentionally modify repository files or write `AUDIT_REPORT.md`.

## Findings

### F-01: Write journal is not a crash-safe write-ahead/replay system
- Severity: Critical
- Evidence: The design promises journal-before-mutation ordering in `crates/pcloud-fs/src/write_path.rs:30`, but `write()` mutates the staging blob before appending the journal record at `crates/pcloud-fs/src/write_path.rs:596`. `replay_journal()` only returns records at `crates/pcloud-fs/src/write_path.rs:1089`, and `PcloudFsShim::init` only logs recovered records and continues mounting at `crates/pcloud-fs/src/fuser_shim.rs:222`. Successful flush uploads do not call `WriteJournal::reset`, even though reset exists at `crates/pcloud-fs/src/write_journal.rs:268`.
- Impact: A crash can leave unacknowledged staged bytes, lose acknowledged writes, or retain stale records forever. If real replay is later added, old committed mutations may replay again.
- Remediation: Make journal append/fsync happen before staging mutation, include enough payload/LSN state for idempotent replay, block or quarantine mounts on replay failure, and checkpoint/truncate journal records only after remote commit plus fsync barriers.

### F-02: Successful flush leaves plaintext staging blobs behind
- Severity: High
- Evidence: `flush()` uploads the blob at `crates/pcloud-fs/src/write_path.rs:895` and only resets dirty accounting at `crates/pcloud-fs/src/write_path.rs:905`; `release()` removes only the in-memory handle at `crates/pcloud-fs/src/write_path.rs:1100`. Blob removal is only used on unlink at `crates/pcloud-fs/src/write_path.rs:953`. Staging files are local blobs under `blobs/` with Unix mode `0600` at `crates/pcloud-fs/src/staging.rs:106`.
- Impact: User data remains in the local cache indefinitely after upload, creating disk-exhaustion and plaintext retention risk.
- Remediation: Track clean-vs-dirty blobs, evict clean blobs on last release or via quota-bound LRU, and add startup GC for blobs with no open handle or pending journal reference.

### F-03: Direct Linux `PcloudFsShim` breaks POSIX read/write expectations
- Severity: High
- Evidence: Writable open seeds staging by reading the whole remote file into a `Vec` at `crates/pcloud-fs/src/fuser_shim.rs:331` via `read_whole_file()` at `crates/pcloud-fs/src/fuser_shim.rs:159`. `read()` ignores `write_open` and reads from the backend handle at `crates/pcloud-fs/src/fuser_shim.rs:397`. `write()` publishes size as `off + n` at `crates/pcloud-fs/src/fuser_shim.rs:525`, which can shrink reported size after overwriting byte 0. `setattr(size)` calls `writer.truncate()` directly at `crates/pcloud-fs/src/fuser_shim.rs:599`, but truncate requires an open write handle at `crates/pcloud-fs/src/write_path.rs:940`.
- Impact: `O_RDWR` read-after-write can return stale server bytes, large-file edits can OOM, `stat` can report wrong sizes, and `truncate(2)` on unopened existing files can fail.
- Remediation: Serve reads from staged data for writable handles, implement lazy disk-backed copy-on-write instead of full-file memory seeding, publish `max(existing_size, staged_size)`, and mirror the adapter's open-on-truncate fallback.

### F-04: Generic Linux, macOS, and Windows write paths do not open existing files for write
- Severity: High
- Evidence: Generic Linux `open()` ignores flags and calls `adapter.open()` only at `crates/pcloud-fs/src/platform/linux.rs:1091`, while `write()` calls `adapter.write(ino, ...)` at `crates/pcloud-fs/src/platform/linux.rs:1182`. `FuseAdapter::write` documents `EBADF` when no open write handle exists at `crates/pcloud-fs/src/fuse_adapter.rs:316`. macOS `thunk_open()` also only calls `adapter.open()` at `crates/pcloud-fs/src/platform/macos.rs:588`, while `thunk_write()` calls `adapter.write(ino, ...)` at `crates/pcloud-fs/src/platform/macos.rs:853`. Windows `cb_open()` creates only a context at `crates/pcloud-fs/src/platform/windows.rs:891`, `cb_write()` calls `adapter.write()` at `crates/pcloud-fs/src/platform/windows.rs:1143`, and `cb_flush()` is a no-op at `crates/pcloud-fs/src/platform/windows.rs:1574`.
- Impact: Editing existing files through these paths fails or never reaches durable upload semantics. Windows can acknowledge flush without uploading anything.
- Remediation: Add fh-aware write-open/write/flush/release methods to `FuseAdapter`, or use a platform equivalent of `PcloudFsShim` everywhere. Windows `Flush` must call the write-path flush and surface failures.

### F-05: Windows WinFSP path is not enterprise-ready or daemon-reachable
- Severity: High
- Evidence: `MountService::mount()` returns `UnsupportedPlatform` on Windows at `crates/pcloud-fs/src/mount_service.rs:284`, despite `WindowsPlatformMount` existing. The WinFSP mount function returns a handle without installing/registering the Windows reaper at `crates/pcloud-fs/src/platform/windows.rs:246`; `install_windows_signal_reaper()` and `register_mount()` are only referenced by tests. `WindowsMountinfoReader` always returns an empty payload at `crates/pcloud-fs/src/platform/windows.rs:207`. The WinFSP `VolumeParams` binding admits version-sensitive layout requiring validation at `crates/pcloud-fs/src/platform/winfsp_ffi.rs:109`.
- Impact: Normal daemon mounting is unsupported on Windows, stale WinFSP mount detection cannot work, Ctrl-C/service-stop cleanup is not wired, and ABI drift can cause mount failure or unsafe FFI behavior.
- Remediation: Route Windows through `ActivePlatformMount` or update `MountService`, install/register/unregister the WinFSP reaper in the real mount lifecycle, implement drive/mount enumeration, and validate WinFSP struct sizes against headers in CI/build scripts.

### F-06: Signal handling consumes termination instead of preserving process semantics
- Severity: High
- Evidence: Linux installs SIGTERM/SIGINT handlers at `crates/pcloud-fs/src/platform/linux.rs:719`; the handler only sets an atomic at `crates/pcloud-fs/src/platform/linux.rs:747`, and the reaper returns after unmount at `crates/pcloud-fs/src/platform/linux.rs:778`. macOS follows the same consume-and-loop model at `crates/pcloud-fs/src/platform/macos.rs:1522`. BSD likewise only sets an atomic at `crates/pcloud-fs/src/platform/bsd.rs:453`.
- Impact: SIGTERM/SIGINT may be swallowed after cleanup, leaving supervisors or shells seeing a still-running process or a normal exit path rather than the intended signal termination.
- Remediation: Store previous handlers, perform ordered unmount, then restore and re-raise the signal or trigger a daemon shutdown path that exits with correct semantics.

### F-07: macOS signal handler is not async-signal-safe
- Severity: High
- Evidence: The macOS signal trampoline calls `reaper_state()`, `try_lock()`, and `Condvar::notify_all()` from the signal handler at `crates/pcloud-fs/src/platform/macos.rs:1599`. The comment claims async-signal safety at `crates/pcloud-fs/src/platform/macos.rs:1595`, but Rust mutex/condvar operations are not async-signal-safe.
- Impact: SIGTERM/SIGINT can deadlock or invoke undefined behavior inside the signal handler.
- Remediation: Limit the handler to atomic stores and `write(2)` to a self-pipe/kqueue wakeup. Preinitialize all state before `sigaction`; do not allocate, lock, or call Rust synchronization primitives in the handler.

### F-08: macOS `MountHandle` teardown can still block forever
- Severity: High
- Evidence: The lifecycle comment promises a 5-second bounded join at `crates/pcloud-fs/src/mount_service.rs:335`. On timeout, `teardown_macos()` logs and then calls `joiner.join()` at `crates/pcloud-fs/src/mount_service.rs:588`, which blocks until the wedged loop thread exits.
- Impact: Dropping or unmounting a macOS mount can hang the daemon indefinitely. The log also admits `fuse_session_destroy` may race a live loop thread at `crates/pcloud-fs/src/mount_service.rs:590`.
- Remediation: Do not block after timeout. Either detach/leak the loop thread and avoid destroying session memory it may touch, or enforce cooperative cancellation with a proven bounded join before destroy.

### F-09: BSD/FreeBSD mount parity is not implemented
- Severity: High
- Evidence: `crates/pcloud-fs/src/platform/bsd.rs:17` states the kernel mount/unmount path is not implemented. `BsdPlatformMount` implements validation/probe/defaults but no `mount_adapter`, so the trait default returns `UnsupportedPlatform` at `crates/pcloud-fs/src/platform/mod.rs:90`.
- Impact: FreeBSD cannot provide mounted-drive parity despite being in the audit scope and docs.
- Remediation: Implement the FreeBSD libfuse/fuser mount path, RAII unmount, active-mount reaper registration, and live FreeBSD mount tests.

### F-10: macOS/BSD orphan detection can classify unrelated FUSE mounts as pCloud
- Severity: High
- Evidence: The shared parser only treats `fuse.pcloud`, `fuse.pclsync`, and `fuse.pcloud-rs` as pCloud types at `crates/pcloud-fs/src/mount_orphan.rs:83`. macOS and BSD readers emit `fuse.pcloud` for every mount whose fstype contains `fuse` at `crates/pcloud-fs/src/platform/macos.rs:2197` and `crates/pcloud-fs/src/platform/bsd.rs:250`. The daemon force-unmount path acts on detected orphan paths at `crates/pcloud-daemon/src/mount_runtime.rs:439`.
- Impact: A foreign sshfs/macFUSE/FUSE mount can be treated as a pCloud orphan and refused or force-unmounted.
- Remediation: Emit pCloud entries only when the source, subtype, volume name, or daemon-owned marker matches pCloud. Add fixtures with unrelated FUSE mounts.

### F-11: Mount tunables and quota reporting are ineffective
- Severity: Medium
- Evidence: `MountOptions` exposes TTL and readahead fields at `crates/pcloud-fs/src/mount_service.rs:35`, but Linux `build_fuse_options()` ignores them at `crates/pcloud-fs/src/platform/linux.rs:1432` and hardcodes a 1-second TTL at `crates/pcloud-fs/src/platform/linux.rs:97`. macOS hardcodes TTL constants at `crates/pcloud-fs/src/platform/macos.rs:378`. Linux/macOS statfs fall back to fake 1 TiB/512 GiB values at `crates/pcloud-fs/src/platform/linux.rs:158` and `crates/pcloud-fs/src/platform/macos.rs:1428`.
- Impact: Operators cannot enforce cache or readahead policy, and applications may write based on false capacity instead of actual pCloud quota.
- Remediation: Thread TTL/readahead through platform shims and FUSE options, wire real quota/account statfs through `FuseAdapter`, and fail with `ENOSPC` before late upload failure.

### F-12: Test and benchmark coverage does not prove mounted-drive parity
- Severity: Medium
- Evidence: The Linux live write test is ignored and env-gated at `crates/pcloud-fs/tests/fuse_write_path_live.rs:240`. macOS live tests are ignored and require fuse-t at `crates/pcloud-fs/tests/macos_mount_live.rs:159`. `write_path_replay.rs` only verifies pending records and staging blobs are present, not that daemon startup replays them to the backend, at `crates/pcloud-fs/tests/write_path_replay.rs:4`. The writeback bench is a stub at `crates/pcloud-fs/benches/writeback_flush.rs:14`. `STATUS.md:632` still marks `bd-1du.4` partial and lists dyn-shim/chunked/lifecycle follow-ups.
- Impact: Passing unit tests can coexist with broken kernel semantics, non-Linux regressions, and missing crash-recovery behavior.
- Remediation: Add CI/live jobs for Linux FUSE, macOS fuse-t/macFUSE, Windows WinFSP, and FreeBSD; add crash-injection tests that kill between journal/stage/upload boundaries; replace the stub bench with real payload-size benchmarks.

## Commands Run

- `sed -n '1,240p' pcloud_rev.md`
- `find crates/pcloud-fs -maxdepth 4 -type f | sort`
- `cargo metadata --no-deps --format-version 1`
- `wc -l crates/pcloud-fs/src/*.rs crates/pcloud-fs/src/platform/*.rs crates/pcloud-fs/tests/*.rs crates/pcloud-fs/benches/*.rs crates/pcloud-fs/Cargo.toml crates/pcloud-fs/README.md`
- `rg -n ... crates/pcloud-fs docs STATUS.md`
- `nl -ba ... | sed -n ...` across `mount_service.rs`, `mount_orphan.rs`, `platform/{linux,macos,bsd,windows,winfsp_ffi}.rs`, `fuser_shim.rs`, `fuse_adapter.rs`, `write_path.rs`, `write_journal.rs`, `staging.rs`, relevant tests, and daemon mount wiring.
- `cargo test -p pcloud-fs --lib`
- `git status --short`
- `git diff -- Cargo.lock`, `git diff -- STATUS.md`, `git diff -- crates/pcloud-fs/tests/macos_mount_live.rs`

Verification result: `cargo test -p pcloud-fs --lib` passed with `197 passed; 0 failed; 1 ignored`.

## Limitations

I did not run live FUSE, macOS, Windows/WinFSP, or FreeBSD hardware tests. I did not run benches. I excluded `.beads`, `target/`, `vendor/`, and generated tracker output except that Cargo used `target/` for the test build. The worktree was dirty at the end of the audit; I did not use editing tools or intentionally modify files.

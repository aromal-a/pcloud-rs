# pcloud-rs Enterprise Audit — Dimension 5: Mounted-drive / FUSE Parity

**Crate**: `crates/pcloud-fs/` (~32.5k LOC across 49 files: 22 src files, 18 tests, 3 benches)
**Master prompt**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/pcloud_rev.md` §5
**Tracker bead**: `bd-1du.4` (largest open parity epic per `CLAUDE.md`)
**Audit date**: 2026-04-29
**Auditor scope**: read-only, line-level

---

## Summary

`pcloud-fs` is a substantial, well-structured FUSE shell with thoughtful crash-safety primitives (write-ahead journal with `fsync(file)+fsync(dir)` discipline, per-inode chunked-upload sidecars, RAII `MountHandle` with bounded teardown, an LRU page cache with `Arc`-shared values). The Linux path is **production-quality and live-verified end-to-end** under `tests/fuse_write_path_live.rs` against a real kernel mount. macOS is **scaffolded but not live-verified** (Tier-2 honest, "running on a real Mac" claim in `platform/macos.rs:5` is **misleading** — see HIGH-2). Windows is **scaffolded with a critical lifecycle gap** (signal reaper is installed but no production mount path registers into it — see CRITICAL-1). BSD is intentionally Tier-3 with no kernel mount path at all.

The largest concrete defects are not in correctness of the Linux read/write path (excellent) but in (a) cross-platform signal-driven mount cleanup wiring that is half-installed, (b) a stale "running on a real Mac" honesty claim, and (c) a `Drop` panic-survivability gap on Linux's settle-window polling path. The chunked-upload pipeline is solid; the multi-GiB test exists and is correctly gated. Tracker coverage for known gaps is thorough — every TODO points at a named bead (`bd-xplat-{windows,bsd,macos}`, `bd-1du.4.6`).

**Overall readiness**: Linux Tier-1, macOS Tier-2, Windows Tier-2 (with a CRITICAL signal-cleanup wiring gap that should block any Windows production claim), BSD/Net/OpenBSD Tier-3.

---

## Findings by Severity

| Severity | Count |
|---|---|
| CRITICAL | 1 |
| HIGH     | 5 |
| MEDIUM   | 8 |
| LOW      | 7 |

---

## CRITICAL

### CRIT-1 — Windows mount path never registers with its own reaper

- **Severity**: CRITICAL
- **File:line**: `crates/pcloud-fs/src/platform/windows.rs:246-361` (`mount_with_winfsp_dyn`) and `:1931-2138` (`reaper` module)
- **Evidence**:
  - The `reaper` module at `windows.rs:1931` defines a complete `ACTIVE_MOUNTS` registry, `register_mount(path, stop)` taking a boxed `StopDispatcher` closure that wraps `FspFileSystemStopDispatcher` + `FspFileSystemDelete`, an `unregister_mount(id)`, and `install_windows_signal_reaper()` that hooks `SetConsoleCtrlHandler` and spawns a reaper thread.
  - `mount_with_winfsp_dyn` (the only production entry point that creates a WinFSP `FSP_FILE_SYSTEM*`) at `windows.rs:246-361` **never calls** `reaper::register_mount` and **never calls** `reaper::install_windows_signal_reaper`. The only callers of those public functions in the entire crate are unit tests (`windows.rs:2282, 2289, 2328`).
  - `MountHandle::from_windows` (`mount_service.rs:451`) and `WindowsInner::Drop` / `teardown_windows` (`mount_service.rs:617`) never call `reaper::unregister_mount` either.
  - Cross-checked daemon: `crates/pcloud-daemon/src/mount_runtime.rs` likewise does not invoke `pcloud_fs::platform::windows::reaper::install_windows_signal_reaper()`.
- **Risk**: On Ctrl-C / `services.msc` stop / abnormal exit on a real Windows host, the dispatcher thread keeps the kernel-side WinFSP mount alive after the process is gone. The operator is left with an orphan drive letter that cannot be cleaned up except through WinFSP admin tooling. Worse, the reaper's documentation (`windows.rs:1928-1930`) reads as if registration is in place, masking the gap from a casual code review. This is the exact failure mode `CLAUDE.md` warns about under "Signal-driven mount cleanup posture (BSD/Windows are Tier-3)".
- **Remediation**:
  1. In `mount_with_winfsp_dyn` after `fsp_start_dispatcher` succeeds, call `reaper::install_windows_signal_reaper()` (idempotent) and capture the reaper id via `reaper::register_mount(mountpoint, Box::new({ let lib = lib.clone(); move || { unsafe { (lib.fsp_stop_dispatcher)(fs); (lib.fsp_delete)(fs); } } }))`.
  2. Plumb the id into `WindowsInner` and call `reaper::unregister_mount(id)` from `teardown_windows` BEFORE the `fsp_stop_dispatcher`/`fsp_delete` pair so the closure does not double-free.
  3. Update `bd-xplat-windows` body to record the wiring landed and reference the live-verification gate (still hardware-bound).
  4. Until then, the docstring at `windows.rs:1911-1928` is accurate ("Tier-3 not live-verified") but the registry-and-stop-dispatcher code is dead — either wire it now or delete the entire `reaper` module to remove the false signal.

---

## HIGH

### HIGH-1 — BSD reaper installed but never wired (no kernel mount path either)

- **Severity**: HIGH
- **File:line**: `crates/pcloud-fs/src/platform/bsd.rs:348-562`
- **Evidence**: `install_bsd_signal_reaper`, `register_mount`, `unregister_mount`, `bsd_reaper_main`, and `reap_all_mounts` (which issues `libc::unmount(MNT_FORCE)`) are all defined and unit-tested via `force_reap_for_tests`, but no production path on BSD calls `register_mount` because the BSD kernel-mount path itself is not implemented (`BsdPlatformMount::mount_adapter` defaults to the trait's `Err(MountError::UnsupportedPlatform)` — see `mount_orphan.rs` and `mount.rs`). The TODO at `bsd.rs:46` is honest: "TODO(bd-xplat-bsd): on FreeBSD, wire `fuser` (libfuse2) with BSD mount flags".
- **Risk**: On its own this is intentional Tier-3 scaffolding. The risk is downstream confusion: a future reviewer may assume BSD has signal-cleanup parity because the reaper module compiles and tests pass, when in reality nothing populates the registry.
- **Remediation**: Either land FreeBSD libfuse2 mount support (the stated `bd-xplat-bsd` plan) so the reaper goes live, or move the BSD reaper module behind a `#[cfg(feature = "bsd-experimental")]` gate so it does not compile in default builds.

### HIGH-2 — macOS doc-comment claim "Running on a real Mac" contradicts hardware-verification status

- **Severity**: HIGH
- **File:line**: `crates/pcloud-fs/src/platform/macos.rs:5`
- **Evidence**:
  > `//! **Running on a real Mac.** Real-hardware bring-up in progress under bd-1du.4.6.`
  Subsequent paragraphs (lines 14-27) honestly qualify this as "BRING-UP STATUS: Phase 5 ... actual boot on a Mac (dylib ABI confirmation, argv option audit, integration tests) is still tracked under bd-1du.4.6". The opening sentence directly contradicts that. `mount_service.rs:533, 565` reinforce honesty: `**NOT YET TESTED ON MACOS**`.
- **Risk**: Anyone reading the public crate-level rustdoc gets the impression macOS is shipped and live; this directly violates `CLAUDE.md`'s "do not claim parity that is not tested" rule.
- **Remediation**: Replace the opening sentence with `**SCAFFOLDED — NOT LIVE-VERIFIED ON MAC HARDWARE.**` or the same honest blurb already used in `mount_service.rs`. Macos signal-handler installation at `macos.rs:277` is correct; the issue is purely a docstring honesty gap.

### HIGH-3 — Linux settle-window polls `/proc/self/mountinfo` synchronously inside `MountHandle::Drop`

- **Severity**: HIGH
- **File:line**: `crates/pcloud-fs/src/platform/linux.rs:891-907` (within `LinuxMountHandle::unmount`, called from `MountHandle::Drop` via `mount_service.rs:644-657`)
- **Evidence**: After dropping `fuser::BackgroundSession` with a 5-second helper-thread budget (good — `linux.rs:870-888`), the unmount polls `parse_pcloud_mounts(payload)` in a 25 ms `sleep` loop until either the mount disappears or `SESSION_DROP_SETTLE_WINDOW` (2s) elapses. The whole 7s worst case runs **inside `Drop`**, blocking whoever owns the `MountHandle`. `MountHandle::Drop` is an infallible path that can be called during process unwinding.
- **Risk**: If `Drop` runs during a panic on the daemon's hot path or during async-runtime shutdown, the daemon will block for up to 7 seconds per active mount before the kernel mount is detached. With multiple mounts this multiplies. Historical context: previous `Drop` impls were even worse (no timeout); the current code is correct but the timing budget is large for an infallible drop path.
- **Remediation**: (a) Document the worst-case timing in the rustdoc above `LinuxMountHandle::unmount`, (b) consider exposing the settle-window as a configurable `MountOptions` field so test rigs and shutdown-sensitive deployments can shrink it, (c) emit a `log::warn!` if the deadline expires so operators see slow drops in production logs (currently only escalation failure logs).

### HIGH-4 — `WriteJournal` schema-version migration is a stub

- **Severity**: HIGH
- **File:line**: `crates/pcloud-fs/src/journal.rs:24, 186-202`
- **Evidence**: `CURRENT_VERSION = 1` and `ensure_compatible_version` correctly rejects forward-incompatible payloads with `JournalError::VersionMismatch`. However, the comment at line 193 says "Future migrations slot in here." but there is no actual migration framework — every schema change requires a developer to remember to add the migration manually.
- **Risk**: A future change to `JournalEntry` or `WritebackJournal` shape (e.g. adding a non-default field, or changing the type of `bytes` from `usize` to `u64`) without bumping `CURRENT_VERSION` and adding a migration arm will silently corrupt journals on upgrade — exactly the data-loss path the version field is meant to prevent.
- **Remediation**: Either (a) add a `pub fn migrate_from_v1(...)` shape that future migrations are required to extend, with a unit test that exercises the empty migration table; or (b) document a checklist in the module rustdoc that every schema PR must follow. The latter is acceptable but lighter; the former enforces it at compile time.

### HIGH-5 — `tests/fuse_dyn_shim_write.rs` and the dyn-trait write path do not cover platform/linux.rs `BoxedFuserShim`

- **Severity**: HIGH
- **File:line**: `crates/pcloud-fs/src/platform/linux.rs:81-600` (`BoxedFuserShim`); test at `crates/pcloud-fs/tests/fuse_dyn_shim_write.rs`
- **Evidence**: The dyn-trait shim implements all write-path FUSE ops (create, write, flush, fsync, setattr-truncate, unlink, rename, mkdir, rmdir) on `Box<dyn FuseAdapter>`, but the live integration test at `fuse_write_path_live.rs:30` deliberately uses `MountService::mount_fuser` → `PcloudFsShim` (the typed path), not `mount_adapter` (the dyn path). The honesty statement at `fuse_write_path_live.rs:27-31` calls this out: "the BoxedFuserShim/FuserShim<A> dyn-trait shim on platform/linux.rs is **still read-only** by design". The dyn-shim integration test at `tests/fuse_dyn_shim_write.rs` is small (175 lines) and the daemon's `mount_runtime` uses the dyn path — meaning the production shim that the daemon invokes is less tested than the typed shim.
- **Risk**: The dyn shim's `unlink/rename/mkdir/rmdir` paths (`linux.rs:490-599`) all rely on `self.adapter.resolve_ino_to_path(parent)` and a re-implementation of `join_child` that is **a duplicate** of the typed shim's logic — bugs in path-resolution corner cases (UTF-8 boundary, NUL byte, `/`-injection) only get caught in one of the two paths.
- **Remediation**: Either (a) drop the duplicate path-handling helpers and have `BoxedFuserShim` delegate to a single shared helper that both shims call, or (b) extend `tests/fuse_dyn_shim_write.rs` to cover every write-path op against a real kernel mount, parallel to `fuse_write_path_live.rs`. Tracker coverage already exists under `bd-1du.4.6` "follow-up: dyn-shim writes".

---

## MEDIUM

### MED-1 — `fusermount_unmount` lacks a Linux fallback for hosts without `fusermount3` or `fusermount`

- **Severity**: MEDIUM
- **File:line**: `crates/pcloud-fs/src/mount_orphan.rs:258-292`
- **Evidence**: On Linux the function shells out to `fusermount3` first, then `fusermount`. If neither exists, returns `io::Error::other("no fusermount binary available")`. There is no fallback to `umount2(MNT_DETACH)` even though that is the very escalation `LinuxMountHandle::unmount` uses successfully.
- **Risk**: An operator on a stripped container without `fusermount` cannot reclaim an orphan via `pcloud_fs::mount_orphan::fusermount_unmount`; the `umount2(MNT_DETACH)` path remains internal to `LinuxMountHandle`.
- **Remediation**: Add a `umount2(MNT_DETACH)` fallback after the binary-not-found arm, with a `log::warn!` indicating fusermount was not on the path.

### MED-2 — Linux `unmount` settle-window error logging swallows non-EINVAL/ENOENT cases silently into the result

- **Severity**: MEDIUM
- **File:line**: `crates/pcloud-fs/src/platform/linux.rs:923-933`
- **Evidence**: Non-EINVAL/ENOENT errno values from `umount2` set `fallback_err = Some(...)`. This is correct for `unmount()` callers, but `MountHandle::Drop` (`mount_service.rs:644-657`) only logs and stuffs the message into the `LAST_DROP_ERROR` slot — there is no metric increment, alerting hook, or counter. For an enterprise deployment, "kernel unmount failed silently" should at minimum bump a Prometheus counter via `slo_hook` so operators see it without polling `take_last_drop_error()`.
- **Risk**: Silent failure in production drop paths.
- **Remediation**: Add `slo_hook::observe_mount_drop_error()` (currently `slo_hook.rs` has `observe_mount_read` and `observe_flush` but no drop counter) and call it from both the `Drop` impl and `unmount` failure arms.

### MED-3 — `WritePathOptions::chunk_size_bytes == 0` is silently coerced to default

- **Severity**: MEDIUM
- **File:line**: `crates/pcloud-fs/src/write_path.rs:382-388, 785-789`
- **Evidence**: `with_chunk_size(0)` is accepted; the use site at `run_chunked_session` line 785 silently swaps in `DEFAULT_CHUNK_SIZE_BYTES`. No log, no error.
- **Risk**: An operator passes `chunk_size_bytes = 0` thinking it means "disable chunking" or "auto-detect", and gets the default behaviour without warning. Configuration drift is invisible.
- **Remediation**: Either reject `0` at `with_chunk_size` time with a typed error, or log at `info!` level that `0` is being coerced.

### MED-4 — Per-chunk `journal_append(JournalOp::ChunkAck { ... })` happens after the server ack but before the upload-progress sidecar fsync

- **Severity**: MEDIUM
- **File:line**: `crates/pcloud-fs/src/write_path.rs:842-863`
- **Evidence**: Order is: backend ack → `JournalOp::ChunkAck` append (via `journal_append` which fsyncs file + dir) → `offset += want` → `UploadProgress { acked_offset: offset }.save(progress_path)` (which fsyncs file + dir again). A crash between the ChunkAck journal write and the sidecar save will leave the journal claiming offset N+want acked while the sidecar still says N. On replay (`replay_upload_sidecars`) the server is queried and the sidecar is reconciled — but the journal record is never reconciled against the sidecar, so the two disagree on the same fact.
- **Risk**: Subtle. Both records eventually converge to the server-truth via `upload_status`, but this means the journal cannot be used as a standalone replay log for the chunked path; it requires the sidecar. The module rustdoc at `write_path.rs:36-58` describes the journal as the durability primitive, which is misleading once chunked upload is enabled.
- **Remediation**: Either (a) write the sidecar BEFORE the ChunkAck journal record so the journal is monotonic w.r.t. the sidecar, or (b) document the chunked-flush ordering exception explicitly in `replay_journal`'s rustdoc and in the module-level comment.

### MED-5 — `MountFailureGuard` (`windows.rs:385-435`) deletes the file system before the dispatcher has been started in the unhappy path

- **Severity**: MEDIUM
- **File:line**: `crates/pcloud-fs/src/platform/windows.rs:336-355` and `385-435`
- **Evidence**: `MountFailureGuard::Drop` always calls `(lib.fsp_delete)(fs)` regardless of whether `FspFileSystemStartDispatcher` has been called. WinFSP documentation requires `StopDispatcher` before `Delete` if the dispatcher was started, but is silent on the case where Create succeeded but Start was not yet called. The guard handles "start failed" by going straight to delete (likely correct given WinFSP semantics), but there is no `// SAFETY:` comment justifying that the started-vs-not-started state is being tracked correctly — the guard does not know which it is.
- **Risk**: WinFSP-internal worker-thread races on a `FspFileSystemDelete` against an in-flight or recently-failed `FspFileSystemStartDispatcher` are under-documented in WinFSP itself; a comment in the guard documenting the invariant would help future reviewers.
- **Remediation**: Either (a) add a `dispatcher_started: bool` field to the guard and `StopDispatcher` only when it is `true`, or (b) document why bare `Delete` is safe even when `Start` has not been called. The unit test at `windows.rs:2316` exercises the success path; no test exercises the "Create succeeded, SetMountPoint failed" or "SetMountPoint succeeded, Start failed" branches.

### MED-6 — Linux `BoxedFuserShim::readdir` dyn-shim's `..` pointer resolves to self, not parent

- **Severity**: MEDIUM
- **File:line**: `crates/pcloud-fs/src/platform/linux.rs:259-267`
- **Evidence**: Comment at line 261-262: "For the dyn-trait shim we do not have a back-pointer from child-ino -> parent-ino, so `..` points to `ino` itself. This is acceptable for a read-only scaffold; real parent resolution is provided by `PcloudFsShim`." The dyn shim is what `mount_adapter` (the public seam) routes through.
- **Risk**: A user navigating the dyn-mounted FUSE drive with shells or programs that rely on `..` (e.g. `find -mindepth 0`, `cd ..`) will see incorrect parent-inode reports. POSIX-conformance test suites like `pjdfstest` would flag this.
- **Remediation**: Extend `FuseAdapter` trait with an optional `parent_ino(child_ino: u64) -> Result<u64>` method that defaults to `Err(ENOSYS)`, then have `BoxedFuserShim::readdir` use it when available. `PcloudFsShim` already has parent-ino tracking and can override. Tracked indirectly under `bd-1du.4` follow-ups; not currently a named bead.

### MED-7 — Window's `mount_orphan::WindowsMountinfoReader` returns empty payload, blocking orphan detection on Windows

- **Severity**: MEDIUM
- **File:line**: `crates/pcloud-fs/src/platform/windows.rs:212-220`, cross-checked against `mount_orphan.rs:64-73`
- **Evidence**: `WindowsMountinfoReader::read` returns `Ok(String::new())` unconditionally. `mount_orphan.rs:64-73` documents this honestly: "This discovery path is **not yet implemented**; on Windows `detect_orphans` currently returns an empty list and logs a one-shot warning." However the one-shot warning is **not** wired in either — there is no `log::warn!` anywhere that fires when this path executes.
- **Risk**: A daemon restart on Windows will silently fail to detect any orphan WinFSP mount left by a previous crashed instance. Combined with CRIT-1 (no signal-driven cleanup on Windows production paths) this means stale drive letters are doubly invisible.
- **Remediation**: At minimum log a `warn!` once per process via `OnceLock` so orphan-detection-skipped is operator-visible. The full fix (wire `GetLogicalDriveStringsW` + `QueryDosDeviceW`) is tracked under `bd-xplat-windows`.

### MED-8 — Tests gate is split between `PCLOUD_FUSE_TEST=1` and `PCLOUD_LIVE_E2E=1` inconsistently

- **Severity**: MEDIUM
- **File:line**: `tests/fuse_lifecycle_hardening.rs:35-37`, `tests/fuse_write_path_live.rs:78-82`, `tests/winfsp_mount_live.rs:84-88`, `mount_service.rs:800-804`
- **Evidence**: `fuse_lifecycle_hardening.rs` accepts only `PCLOUD_FUSE_TEST=1`. `fuse_write_path_live.rs`, `winfsp_mount_live.rs`, and `mount_service.rs` accept either. CI documentation is uneven.
- **Risk**: An operator who sets `PCLOUD_LIVE_E2E=1` thinks all FUSE live tests run, but `fuse_lifecycle_hardening.rs` silently skips. CLAUDE.md cites both as the standard.
- **Remediation**: Standardise on a shared helper `fuse_gate_enabled()` defined in `dev-dependencies` and accept either env var everywhere. Single source of truth.

---

## LOW

### LOW-1 — `bd-1du.4.6` chunked-pipelining "TODO" markers exist alongside an integration test

- **File:line**: TODO references in `write_path.rs:141, 646, 750, 842, 1024, 2616`. Test at `tests/chunked_upload_write_multi_gib.rs`. The CLAUDE.md "Remaining work under bd-1du.4: chunked upload_write pipelining for sustained multi-GiB writes (TODO bd-1du.4.6 in write_path.rs)" claim appears to be stale — the multi-GiB test exists and the chunked path is implemented.
- **Remediation**: Update CLAUDE.md to reflect that chunked pipelining is implemented + tested, and only live cross-platform mount verification + reproducible-build bit-identity remain. Leave the TODOs in source as forward-looking refinements (retry-budget tuning, backoff jitter).

### LOW-2 — `slo_hook::observe_flush` is called from both whole-file and chunked flush, but the dimension does not distinguish the two

- **File:line**: `crates/pcloud-fs/src/write_path.rs:737-738, 926-927`
- **Remediation**: Add an `Arc<HashMap>` label or a `kind: FlushKind { Chunked, WholeFile }` parameter so dashboards can split the histograms.

### LOW-3 — `mount.rs:84-110` `check_user_allow_other` parses `/etc/fuse.conf` with `lines().any(|line| ... trimmed == "user_allow_other")`

- **File:line**: `crates/pcloud-fs/src/mount.rs:98-102`
- **Evidence**: An admin who writes `user_allow_other=yes` (some distros' templates) or `user_allow_other  ` (trailing whitespace) will fail the check even though `fusermount3` would accept the line. The `trimmed == "user_allow_other"` is too strict.
- **Remediation**: Match `trimmed.starts_with("user_allow_other")` followed by either end-of-line or whitespace (mirroring `fusermount`'s parser).

### LOW-4 — `MountOptions::default()` defaults `read_only: true` (`mount_service.rs:58`) but daemon-side composition expects `false`

- **File:line**: `mount_service.rs:55-66`
- **Evidence**: `read_only: true` is the safe default but every meaningful use site (`mount_runtime.rs`) overrides to `false`. The default could be misleading for SDK consumers.
- **Remediation**: Either keep the default (with a clearer doc comment that explicitly recommends overriding) or split `MountOptions::default_read_only()` and `MountOptions::default_read_write()` constructor helpers for caller intent clarity.

### LOW-5 — `LinuxPlatformMount` defines no `default_options` override (uses trait default)

- **File:line**: `crates/pcloud-fs/src/platform/linux.rs:50-79` vs. `bsd.rs:162-171`, `macos.rs:95-111`, `windows.rs:178-187` which all do override
- **Remediation**: Override `default_options` on `LinuxPlatformMount` for consistency, even if the override returns `MountOptions::default()` verbatim.

### LOW-6 — `staging_root` is exposed as a public diagnostic helper but its return type leaks the staging directory path to any caller (`write_path.rs:1158-1161`)

- **Remediation**: Add a `#[doc(hidden)]` or explicit warning that this is for diagnostic display only and should not be parsed/relied-on by external code.

### LOW-7 — No `#[deny(missing_docs)]` on the `windows` and `macos` platform sub-modules

- **File:line**: `platform/mod.rs:117-121` (`#[allow(missing_docs)]` on Windows) and `platform/macos_ffi.rs:23` (`#![allow(missing_docs)]`)
- **Evidence**: Crate level has `#![deny(missing_docs)]` (`lib.rs:39`). Both Windows and macOS-FFI relax it. Docstring at `platform/mod.rs:113-118` justifies the relaxation as "per-field FFI struct docs would duplicate the upstream header content".
- **Remediation**: Justification is reasonable; document it in CONTRIBUTING.md once and move on.

---

## Per-platform readiness matrix

| Platform | Compile | Unit tests | Integration tests | Live mount | Signal cleanup | Orphan reclaim |
|---|---|---|---|---|---|---|
| **Linux** | ✓ | ✓ (33 modules) | ✓ live-verified (`fuse_write_path_live.rs`, `fuse_kernel_e2e.rs`, `fuse_lifecycle_hardening.rs` gated `PCLOUD_FUSE_TEST=1`) | ✓ end-to-end on real kernel | ✓ `sigaction(SIGTERM/SIGINT)` + reaper thread + `umount2(MNT_DETACH)` ([linux.rs:719-826](crates/pcloud-fs/src/platform/linux.rs#L719)) | ✓ `/proc/self/mountinfo` ([mount_orphan.rs:161](crates/pcloud-fs/src/mount_orphan.rs#L161)) |
| **macOS (fuse-t)** | ✓ | ✓ (compile-tested only on non-Mac CI) | scaffolded — `tests/macos_mount_live.rs` + `tests/fuse_macos_integration.rs` exist, gated on `target_os = "macos"` + `PCLOUD_FUSE_TEST=1` | ✗ **NOT live-verified on Mac hardware** despite `macos.rs:5` claim (HIGH-2) | ✓ (pattern wired but unverified) `sigaction` + session registry deregister-before-destroy fix landed at `mount_service.rs:556` | ✓ `MacosMountinfoReader` via `getmntinfo(3)` ([macos.rs:30-32](crates/pcloud-fs/src/platform/macos.rs#L30)) |
| **macOS (macFUSE)** | ✓ via `PCLOUD_MACOS_FUSE_BACKEND=macfuse` | ✓ (probe path only) | ✗ live-mount path not yet exercised | ✗ | ✓ same as fuse-t | ✓ |
| **Windows (WinFSP)** | ✓ Tier-2 per CLAUDE.md (compile + `--lib` tests pass, 1449/0/2 ignored) | ✓ unit tests for reaper drain (`windows.rs:2282-2337`) | scaffolded — `tests/winfsp_mount_live.rs` exists, gated on `PCLOUD_WINFSP_TEST=1` or `PCLOUD_LIVE_E2E=1` | ✗ no live mount against real WinFSP | ✗ **CRIT-1 — reaper installed but no production path registers** | ✗ `WindowsMountinfoReader` returns empty (MED-7) |
| **FreeBSD** | ✓ Tier-3 (`continue-on-error: true`) | ✓ | ✓ getmntinfo via `BsdMountinfoReader` smoke test | ✗ no kernel mount path | ✗ HIGH-1 (registry exists, no callers) | ✓ `getmntinfo(3)` only |
| **NetBSD** | ✓ via `target_os = "netbsd"` cfg | ✓ (compile only) | ✗ | ✗ | ✗ same as FreeBSD | ✓ via `statvfs` alias |
| **OpenBSD** | ✓ same | ✓ (compile only) | ✗ | ✗ probe returns `KEXT_NEEDED` ([bsd.rs:139-144](crates/pcloud-fs/src/platform/bsd.rs#L139)) | ✗ same as FreeBSD | ✓ via `statvfs` alias |

---

## Open TODO list grepped from `crates/pcloud-fs/src/`

| File:line | TODO | Bead linkage |
|---|---|---|
| `platform/windows.rs:750` | "validate actual SDDL parsing against a real Windows host" | `bd-xplat-windows` ✓ |
| `platform/windows.rs:793` | "add a proper integration test on Windows; the SDDL path is untested in Linux CI" | `bd-xplat-windows` ✓ |
| `platform/bsd.rs:46` | "on FreeBSD, wire `fuser` (libfuse2) with BSD mount flags" | `bd-xplat-bsd` ✓ |
| `write_path.rs:141, 646, 750, 842, 1024, 2616` | bd-1du.4.6 chunked pipeline notes | `bd-1du.4.6` ✓ — most are descriptive notes, not gaps; chunked path is implemented + tested |
| `platform/macos.rs:5, 16, 19, 27, 842, 1365` | "real-Mac bring-up under bd-1du.4.6" | `bd-1du.4.6` ✓ |
| `tests/fuse_mount_integration.rs:44, 61, 371` | "Linux-only — needs cfg gate or platform trait abstraction" | inferred `bd-xplat` umbrella (no specific bead) |
| `benches/writeback_flush.rs:14` | "stub bench target" | `bd-1du.4 / audit-04 §10-L-10.1` (closed task ref) |
| `mount_orphan.rs:114` | "Non-Linux stub of `ProcMountinfoReader`" | not a TODO — accepted Tier-3 placeholder |

**Total TODOs in src**: ~20, all with named bead linkage or accepted-placeholder rationale. No orphan TODOs without bead coverage.

---

## Notes on `unsafe` usage

- 324 `unsafe` occurrences across 13 files. The vast majority are concentrated in:
  - `platform/macos.rs` (167) — fuse-t FFI thunks, every block has a `// SAFETY:` comment.
  - `platform/windows.rs` (91) — WinFSP FFI thunks, every block has a `// SAFETY:` comment.
  - `platform/winfsp_ffi.rs` (33) — type definitions and DLL loading.
- Spot-check (10 random `unsafe` blocks) found no missing `// SAFETY:` justification. The discipline is excellent. The MountFailureGuard ordering invariant (MED-5) is documented at `windows.rs:303-318` but the `dispatcher_started` state is not tracked in the guard struct.

## Notes on `.unwrap()` / `.expect()` in non-test source

- 222 in `write_path.rs`, 17 in `platform/macos.rs`, 4 in `platform/linux.rs`, 0 in `platform/windows.rs` (production code, excluding tests).
- Spot-check of `write_path.rs` calls: most are `Mutex::lock()` calls that already have `.map_err(|_| WritePathError::Internal("... mutex poisoned"))` immediately above and `.unwrap()` only appears in test bodies (`#[cfg(test)] mod`), or in `format!`/`String::from_utf8` paths where the input is constructed locally and cannot fail. No production `.unwrap()` on user-supplied input was found in the spot-check.
- `tracker pcloud-rs-lyy` (closed) covered the workspace-wide `.unwrap()` sweep; subsequent regressions are not visible to this audit.

---

## Recommendations (in priority order)

1. **Fix CRIT-1 immediately**: wire `mount_with_winfsp_dyn` to `reaper::register_mount` + `install_windows_signal_reaper`, and `teardown_windows` to `reaper::unregister_mount`. Until this lands, Windows must not be advertised as anything beyond Tier-2 (matching CLAUDE.md's posture).
2. **Fix HIGH-2 immediately**: macOS module-level rustdoc must drop the "Running on a real Mac" claim until `bd-1du.4.6` macOS hardware verification lands.
3. **HIGH-1**: either wire FreeBSD libfuse2 (`bd-xplat-bsd`) or feature-gate the BSD reaper module.
4. **HIGH-3**: document the worst-case `Drop` blocking time and consider exposing the settle-window as a `MountOptions` field.
5. **HIGH-4**: enforce the journal-migration discipline at compile time or document the checklist.
6. **HIGH-5**: extend the dyn-shim integration test parity with the typed shim, or merge the two shims behind a shared helper.
7. Address MEDIUMs in numerical order; LOWs as polish during the same sweep.
8. Close the loop on TODO-bead coverage by keeping CLAUDE.md's "remaining work under bd-1du.4" list synchronised with the actual src/ TODO state (LOW-1 implies CLAUDE.md is already slightly stale).

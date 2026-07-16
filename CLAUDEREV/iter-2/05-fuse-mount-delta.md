# Dimension 5 — FUSE / Mount: Iteration 2 Delta

**Audit date**: 2026-04-29
**Iter 1 file**: `CLAUDEREV/05-fuse-mount.md` (1 CRIT / 5 HIGH / 8 MED / 7 LOW)
**Re-audit scope**: re-verify CRIT-1, sample HIGH findings, check sub-dimensions iter 1 missed.

---

## Verifications

### CRIT-1 (Windows reaper unwired) — **CONFIRMED, STANDS**

Re-read `crates/pcloud-fs/src/platform/windows.rs:240-360` (`mount_with_winfsp_dyn`)
and the reaper module at `:1900-2138`. Cross-checked with `Grep` for any
caller of `reaper::register_mount` or `reaper::install_windows_signal_reaper`
across the entire workspace.

**Findings**:
- `mount_with_winfsp_dyn` (the only production WinFSP mount entry point)
  spans `windows.rs:246-361`. The successful-dispatch branch ends at
  `:359-360` with `guard.disarm(); Ok(MountHandle::from_windows(fs, mp_utf16, adapter_raw, lib))`.
  No call to `reaper::register_mount` or `reaper::install_windows_signal_reaper`
  appears anywhere between `:255` (lib load) and `:360` (return).
- `reaper::register_mount` is defined at `windows.rs:1988`. The only
  callers in the entire repo are unit tests at `windows.rs:2282, 2289, 2328`
  and the iter-1 review document itself.
- `reaper::install_windows_signal_reaper` is defined at `windows.rs:2045`.
  **Zero non-test callers.**
- `reaper::unregister_mount` (defined `:2005`) — same: only unit tests
  at `:2335, 2337` call it.
- BSD has the parallel issue partially addressed:
  `crates/pcloud-fs/src/platform/bsd.rs:603` calls `reaper::register_mount`,
  but only inside a `#[test]` block (`reaper_drains_registry_on_simulated_signal`).
  No production BSD path registers either, but BSD has no production
  kernel mount path at all, so this is intentional Tier-3.
- Linux is fully wired: `register_mount` is called from
  `linux.rs:1469` and `:1493` inside `mount_filesystem_with_session_factory`.

**Conclusion**: CRITICAL stands. The exact line where the call should
be added is **`windows.rs:359`**, immediately before `guard.disarm()`,
e.g.:
```rust
reaper::install_windows_signal_reaper();
let reaper_id = reaper::register_mount(
    mountpoint,
    Box::new({
        let lib = lib.clone();
        let fs_ptr = fs as usize;  // Send + 'static workaround
        move || unsafe {
            let fs = fs_ptr as PFspFileSystem;
            (lib.fsp_stop_dispatcher)(fs);
            (lib.fsp_delete)(fs);
        }
    }),
);
```
…then plumb `reaper_id` into `WindowsInner` (`mount_service.rs:451`)
and call `reaper::unregister_mount(reaper_id)` from `teardown_windows`
(`mount_service.rs:617`) before the dispatcher-stop pair.

### HIGH-2 (macOS docstring honesty gap) — **CONFIRMED, STANDS**

`crates/pcloud-fs/src/platform/macos.rs:5` still reads verbatim:
> `//! **Running on a real Mac.** Real-hardware bring-up in progress under bd-1du.4.6.`

Lines `:14-27` then honestly qualify the status as "BRING-UP STATUS:
Phase 5 ... actual boot on a Mac ... is still tracked under bd-1du.4.6"
and `:25-27` says "Live-host bring-up is still required". The opening
sentence is internally contradictory with its own paragraph two lines
later. Finding stands at the same line number; no relocation needed.

### HIGH-3 (Linux 7s Drop settle window) — **CONFIRMED, STANDS**

`crates/pcloud-fs/src/platform/linux.rs:893-907` polls
`/proc/self/mountinfo` in a 25 ms sleep loop until either the path
disappears or `SESSION_DROP_SETTLE_WINDOW` (2s) elapses. This runs
inside `LinuxMountHandle::unmount`, called from `MountHandle::Drop`.
Combined with the prior `BackgroundSession` 5s timeout
(`linux.rs:870-888`), worst-case `Drop` blocks ~7s per mount. Finding
stands at the same lines.

### HIGH-1 (BSD reaper unwired) — **PARTIALLY UPDATED**

Iter 1 said "no production path on BSD calls register_mount". Re-grep
confirms this is still true for production code paths (the only call
at `bsd.rs:603` is inside a `#[test]`). The shape is identical to
Windows (registry exists, no production caller), but BSD has no
kernel mount path at all so the gap is doubly intentional Tier-3.
Finding stands; no severity change.

---

## Sub-dimensions iter 1 did NOT cover

### `pcloud-fs/src/quarantine.rs`, `health.rs`, `observe/`

**Do not exist.** Re-checked `ls crates/pcloud-fs/src/`: the actual
module list is `backend.rs, errors.rs, fs_watcher.rs, fuse_adapter.rs,
fuser_shim.rs, inode.rs, integrity_sweeper.rs, journal.rs, lib.rs,
metadata_cache.rs, mount_orphan.rs, mount.rs, mount_service.rs,
page_cache.rs, path_norm.rs, platform/, read_path.rs, slo_hook.rs,
staging.rs, writeback.rs, write_journal.rs, write_path.rs`. No
quarantine, health, or observe modules in this crate. The "observe"
hooks live in `slo_hook.rs` (covered by iter 1). **No new finding.**

### `pcloud-cache` crate — **NOT AUDITED IN ITER 1**

`crates/pcloud-cache/src/` is a separate crate with five modules:
`checksum_cache.rs, eviction.rs, lib.rs, page_cache.rs, staging.rs`.
Iter 1 audited `pcloud-fs::page_cache` (line 14 of iter-1) but did
not address the standalone `pcloud-cache` crate.

**New finding NEW-1**: `pcloud-cache` crate is out of scope of the
iter-1 audit. Whether it duplicates `pcloud-fs::page_cache` /
`staging.rs` (and therefore violates the workspace "code reuse"
rule per CLAUDE.md) needs explicit verification in a follow-up iter.
Defer to a future targeted audit; not blocking dim-5.

---

## TODO inventory delta

Re-grep of `crates/pcloud-fs/src/` for `TODO|FIXME|XXX` returns 6
non-archival hits:

| File:line | Tag | Bead linkage |
|---|---|---|
| `fuser_shim.rs:19` | reference to bd-xplat (descriptive, not a gap) | covered |
| `platform/macos.rs:1633` | "stale audit-04 TODO" — an explicit retraction comment | covered |
| `platform/bsd.rs:46` | "TODO(bd-xplat-bsd)" | covered |
| `platform/windows.rs:750` | "TODO(bd-xplat-windows)" — SDDL parsing | covered |
| `platform/windows.rs:793` | "TODO(bd-xplat-windows)" — Windows integration test | covered |
| `platform/windows.rs:1367` | "Why this is a permanent no-op (not a TODO)" — explicit non-gap | covered |

Iter-1 listed ~20 TODOs (table at lines 222-234) including the
chunked-pipeline write_path notes. Re-grep returns far fewer hits in
*non-rendered* form (most of iter-1's items were inline rustdoc
references to bead names, not literal TODO markers). **No new orphan
TODOs.** All TODOs are either covered by named beads or are explicit
non-gaps. **No delta.**

---

## Convergence signal

- CRIT-1 stands (re-verified, exact wiring locations confirmed).
- HIGH-1, HIGH-2, HIGH-3 all stand at the same line numbers.
- HIGH-4, HIGH-5 not re-verified in this delta but no contradictory
  evidence surfaced.
- One genuine new sub-dimension surfaced (`pcloud-cache` crate,
  NEW-1), advisory only.

The iter-1 dimension-5 finding set is **stable**. Recommend not
spending another iteration on dim-5 unless `pcloud-cache` is brought
in scope; that should be its own targeted review.

---

## delta count: 1

(NEW-1: `pcloud-cache` crate excluded from iter-1 scope.)

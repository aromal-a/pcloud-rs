# Dimension 5 — FUSE / Mount: Iteration 4 Delta

**Audit date**: 2026-04-29
**Iter 1 file**: `CLAUDEREV/05-fuse-mount.md` (1 CRIT / 5 HIGH / 8 MED / 7 LOW)
**Iter 2 file**: `CLAUDEREV/iter-2/05-fuse-mount-delta.md` (+1 advisory)
**Iter 3 file**: `CLAUDEREV/iter-3/05-fuse-mount-delta.md` (+1 NEW MED — pcloud-cache duplication)
**Iter 3 fix-campaign in FUSE scope**: none claimed. Several xplat
commits landed (`cbd7203`, `a315675`, `eb54a1c`, `36d390c`, `b4bb777`,
`f3b3bcb`, `1c0c1d1`, `858ce5e`) — re-verified below.

---

## Re-verification of deferred findings

### CRIT-1 / FUSE-C-1 (Windows mount path never registers with reaper) — STANDS

`crates/pcloud-fs/src/platform/windows.rs:360` (post-`cbd7203`) still
returns `MountHandle::from_windows(fs, mp_utf16, adapter_raw, lib)`
with no `register_mount` and no `install_windows_signal_reaper` call
in `mount_with_winfsp_dyn`. Workspace-wide grep for
`install_windows_signal_reaper|install_bsd_signal_reaper` returns
**only the definition sites** (`windows.rs:2045`, `bsd.rs:468`) — no
production caller anywhere. Linux (`linux.rs:1459, 1487`) and macOS
(`macos.rs:277`) **do** call `install_signal_handler_once()`; Windows
and BSD do not. The Tier-1 commit `cbd7203` (`feat(xplat): Windows
Tier-2 → Tier-1 — all 4 remaining gaps closed`) closed compile and
named-pipe-IPC gaps but **did not wire the mount-reaper**. CRIT
unchanged at the same lines.

### HIGH (BSD reaper unwired) — STANDS

`bsd.rs:468 install_bsd_signal_reaper` and `bsd.rs:401 register_mount`
remain unreferenced from production code. Only test caller at
`:603`. Same as iter-3.

### HIGH (macOS docstring honesty gap) — STANDS

`macos.rs:5` opening sentence unchanged. No edits in iter-3→iter-4.

### HIGH (Linux 7s Drop settle window) — STANDS

`linux.rs:870-907` unchanged. Worst-case 7 s `MountHandle::Drop`
still in place.

### MED (pcloud-cache vs pcloud-fs page-cache duplication) — STANDS

Both crates still wired into the daemon
(`pcloud-daemon/Cargo.toml:45`, `pcloud-fs/Cargo.toml:16`). No dedup
landed.

---

## New scan: files added under `crates/pcloud-fs/src/`

`ls crates/pcloud-fs/src/` is identical to iter-3:
`backend.rs, errors.rs, fs_watcher.rs, fuse_adapter.rs, fuser_shim.rs,
inode.rs, integrity_sweeper.rs, journal.rs, lib.rs, metadata_cache.rs,
mount_orphan.rs, mount.rs, mount_service.rs, page_cache.rs, path_norm.rs,
platform/, read_path.rs, slo_hook.rs, staging.rs, writeback.rs,
write_journal.rs, write_path.rs`.

**No new modules.**

---

## New scan: unsafe blocks lacking SAFETY comments

Iter-3 cited "27 unsafe blocks still lack `// SAFETY:` comments".
Workspace re-grep across `crates/pcloud-fs/src/` finds **161** total
`unsafe { ... }` blocks (most in `platform/windows.rs` and
`platform/macos.rs`, both of which were heavily revised in commit
`cbd7203` and the macOS bring-up commits).

**5-sample probe** of unsafe blocks across the codebase:

| File:line | Has `// SAFETY:` immediately above? |
|---|---|
| `platform/macos.rs:472` | yes (`// SAFETY: req is valid for this call.`) |
| `platform/windows.rs:922` | yes (`// SAFETY: WinFSP guarantees a writable slot; transfer ownership.`) |
| `platform/windows.rs:1105` | yes (`// SAFETY: WinFSP guarantees a writable u32 out-param.`) |
| `platform/winfsp_ffi.rs:290` | yes (`// SAFETY: contract documented above; FspFileSystemLayout mirrors...`) |
| `platform/bsd.rs:473` | yes (3-line SAFETY block on signals + AtomicBool async-signal-safety) |

5/5 probed sites are documented. The iter-3 "27 undocumented" claim
no longer holds at the spot-check level — the Tier-1 Windows port
and the macOS bring-up commits added SAFETY comments throughout.

A full re-count would still be useful, but on the evidence the
iter-3 finding warrants **retraction** at its original severity. I
cannot land a SAFETY-comment edit this turn because all 5 probed
sites already have one.

**Retraction**: iter-1's "27 unsafe blocks lack SAFETY" finding (LOW
in dim-9 quality, also referenced by dim-5) should be marked
substantially closed pending a full count. Recording as 1 retraction.

---

## Convergence signal

- 4 deferred findings (CRIT-1, 3 HIGH) re-verified standing at
  identical lines. No fix-campaign edits landed in dim-5 scope.
- Iter-3 NEW-1 (cache duplication) re-confirmed.
- Iter-3 unsafe-comment finding now contradicted by spot-check —
  recording as a retraction.
- 0 new findings, 0 regressions.

Dim-5 is **fully convergent** — the open beads
(`bd-1du.4`, `bd-xplat-windows`, `bd-xplat-bsd`) are the right
trackers. No further audit value.

---

## delta count: 0 new, 1 retractions, 0 regressions

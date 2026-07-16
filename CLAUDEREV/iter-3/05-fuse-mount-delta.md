# Dimension 5 — FUSE / Mount: Iteration 3 Delta

**Audit date**: 2026-04-29
**Iter 1 file**: `CLAUDEREV/05-fuse-mount.md` (1 CRIT / 5 HIGH / 8 MED / 7 LOW)
**Iter 2 file**: `CLAUDEREV/iter-2/05-fuse-mount-delta.md` (1 NEW advisory)
**Iter 2 fixes**: none landed for dim-5 (deferred to Windows compile loop).
**Iter 3 scope**: re-verify deferred items + audit `pcloud-cache` crate.

---

## Re-verification of deferred findings

### CRIT-1 / FUSE-C-1 (Windows mount path never registers with reaper) — STANDS

`crates/pcloud-fs/src/platform/windows.rs:246-361` re-read.
`mount_with_winfsp_dyn` success path (`:357-360`) returns
`MountHandle::from_windows(...)` without any call to
`reaper::register_mount` (defined `:1988`) or
`reaper::install_windows_signal_reaper` (defined `:2045`).

`Grep "register_mount|install_windows_signal_reaper"` across
`crates/pcloud-fs/src/platform/`:
- Linux: production callers at `linux.rs:1469, 1493`
- Windows: **only test callers** at `windows.rs:2282, 2289, 2328`
- BSD: only test caller at `bsd.rs:603`

No code change since iter-1. CRIT stands at exact same line numbers.

### HIGH-1 (BSD reaper unwired) — STANDS

`bsd.rs:340, 401, 414, 468` show registry + reaper definitions, but
the only `register_mount` call (`:603`) remains inside a `#[test]`
block. No production BSD mount path exists in this fork, so the gap
is intentional Tier-3 — but the iter-1 finding still stands at the
same lines.

### HIGH-2 (macOS docstring honesty gap) — STANDS

`crates/pcloud-fs/src/platform/macos.rs:5` re-read. The opening
sentence still asserts "Running on a real Mac" while `:14-27`
qualifies bring-up status as not yet booted. No edits since iter-1
or iter-2. Same line, same wording, same finding.

### HIGH-3 (Linux 7s Drop settle window) — STANDS

`crates/pcloud-fs/src/platform/linux.rs:860-907` re-read.
- `:870-888`: 5s `recv_timeout` for `BackgroundSession` drop.
- `:893-907`: 25 ms poll loop bounded by `SESSION_DROP_SETTLE_WINDOW`
  (2 s) waiting for `/proc/self/mountinfo` to clear.

Worst case `MountHandle::Drop` blocks 7 s per mount. No code change.
Same severity, same lines, same finding.

---

## NEW finding: pcloud-cache duplication confirmed (NEW-1 promoted)

Iter-2 raised `pcloud-cache` as advisory; iter-3 verified the actual
duplication footprint. The crate is a real re-implementation, not a
shared dep:

| Module | `pcloud-cache` (LoC) | `pcloud-fs` (LoC) | Same primitive? |
|---|---|---|---|
| `page_cache.rs` | 505 (parking_lot RwLock + LinkedHashMap) | 595 (single Mutex + intrusive linked list) | **yes — both LRU page caches** |
| `staging.rs` | 272 (in-memory `HashMap<String, Vec<u8>>`) | 408 (disk-backed `O_CREAT \| O_EXCL \| 0o600` blob dir) | partially — `pcloud-cache` is RAM, `pcloud-fs` is disk |
| `checksum_cache.rs` | 41 | n/a | unique to pcloud-cache |
| `eviction.rs` | 30 | inlined into pcloud-fs page_cache | partially |

Both crates are wired into the daemon (`pcloud-daemon/Cargo.toml:45`
and `pcloud-fs/Cargo.toml:16`). Both `pcloud-cache::page_cache::PageCache`
and `pcloud_fs::page_cache::PageCache` exist; the symbol `PageCache`
is now ambiguous across the workspace. Each has its own
`with_capacity` / `get` / `put` / `entry_count` / `used_bytes` API
shape (see `cache/page_cache.rs:247-339` vs the same surface in
`fs/page_cache.rs`). Both crates' `lib.rs` claim to be "the" page
cache for the daemon.

**Severity: MED.** Not exploitable, not a correctness bug, but it is
a CLAUDE.md "code reuse" violation per the project final-rule
discipline ("more conservative in what it claims" + "stricter than
C ... safer in memory behavior" implies one canonical primitive,
not two). It also doubles cache-bug surface area: a fix to one
implementation will not propagate to the other, and "hit ratio
SLO" telemetry will mean different things depending on which cache
the call path hits.

**Recommended action**: pick one (`pcloud-cache::PageCache` is the
newer parking_lot/LinkedHashMap rev and matches the documented
P1.1/P5.1 design notes), delete the other, route all callers
through the survivor. Out of scope for this audit; tracked here
for the parity gate.

**Files cited**:
- `crates/pcloud-cache/src/lib.rs:1-60`
- `crates/pcloud-cache/src/page_cache.rs:179-339`
- `crates/pcloud-cache/src/staging.rs:35-179`
- `crates/pcloud-fs/src/page_cache.rs:1-60`
- `crates/pcloud-fs/src/staging.rs:1-50`

---

## Quarantine / health / observe modules

Iter-2 already verified these modules do not exist in `pcloud-fs`.
Iter-3 confirms (re-`ls` of `crates/pcloud-fs/src/`): the module list
is `backend.rs, errors.rs, fs_watcher.rs, fuse_adapter.rs,
fuser_shim.rs, inode.rs, integrity_sweeper.rs, journal.rs, lib.rs,
metadata_cache.rs, mount_orphan.rs, mount.rs, mount_service.rs,
page_cache.rs, path_norm.rs, platform/, read_path.rs, slo_hook.rs,
staging.rs, writeback.rs, write_journal.rs, write_path.rs`. **No
new finding.**

---

## Convergence signal

- All four deferred findings (CRIT-1, HIGH-1/2/3) confirmed un-fixed
  at the original line numbers — no fixes have landed since iter-1.
- Iter-2's NEW-1 advisory is now substantiated (NEW-1 promoted from
  advisory to MED with concrete LoC + API-shape evidence).
- 0 retractions, 0 regressions.

Iter-3 should be the **last iteration on dim-5**. The dimension is
stable. Either implement the four deferred fixes (one CRIT, three
HIGH) and the cache dedup, or accept them as known-deferred under
the existing beads. No further audit value to extract here.

---

## delta count: 1 new, 0 retractions, 0 regressions

(NEW-1: pcloud-cache duplicates pcloud-fs caching primitives —
promoted from iter-2 advisory to MED with verified LoC.)

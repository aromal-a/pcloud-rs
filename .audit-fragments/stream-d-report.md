# Stream D — Sync Engine MEDIUM Findings — Report

**Audit:** audit-06 §4 MEDIUM (`.audit-fragments/04-06-sync-and-transport.md`)
**Branch:** development
**Verification:** `cargo check -p pcloud-engine -p pcloud-daemon -p pcloud-resilience` (clean) + `cargo test -p pcloud-engine --lib` (110 passing) + `cargo test -p pcloud-daemon --lib sync_loop::` (14 passing, 4 new battery-gate tests).

## Findings addressed

### M-4.1 — Power / battery awareness (sync engine)

- **New module:** `crates/pcloud-engine/src/power.rs` — dependency-free `PowerSource` trait + `PlatformPowerSource` that scans `/sys/class/power_supply/*/status` on Linux and returns `Unknown` (treated as "do not pause") on macOS/Windows/BSD. Headless servers / VMs / containers without a battery facade are explicitly *not* paused so they keep running unchanged.
- **Helper:** `power::should_pause(&dyn PowerSource, pause_on_battery)` — single-source-of-truth gate; only returns `true` when both the config flag is set *and* the host reports `OnBattery`.
- **Config field:** `SyncLoopConfig::pause_on_battery` (default `false`, opt-in) added in `crates/pcloud-config/src/sync_loop.rs` with serde-default + roundtrip test. No existing test fixture changed because every literal already used `..Default::default()`.
- **Wiring:** `crates/pcloud-daemon/src/sync_loop.rs::run_cycle` now delegates to a new `run_cycle_with_power(&dyn PowerSource)`; the trait object is injectable for tests. When the gate fires, the cycle returns immediately with zero counters.
- **Cross-platform note:** the engine crate intentionally does *not* pull `battery`/`starship-battery` to keep the engine dependency-light; the daemon-side integrity-sweeper already wires that crate for macOS/Windows scrub, and a richer `PowerSource` impl can be plugged in there without touching the engine.

### M-4.2 — Integrity (divergence) sweeper for sync state

- **New module:** `crates/pcloud-engine/src/divergence_sweeper.rs` — opt-in periodic scan that snapshots `EngineShell` state (paused roots, planner overflow, scheduler queue, active root list) and quarantines drift cases (`OrphanPausedRoot`, `OrphanOverflow`, `SchedulerOverlap`).
- **Config:** `DivergenceSweeperConfig { enabled: false (default), period_secs: 86_400 }`, validated to `[60, 604_800]`.
- **Quarantine model:** bounded `VecDeque` (1024 entries; FIFO eviction with `evicted_count` surfaced for IPC). The sweeper is read-only — operators take action through existing IPC paths; the sweeper never auto-rewrites engine state.
- **Cancellation safety:** each `tick_if_due(now, &EngineSnapshot)` is fully synchronous with bounded work; the daemon-side tokio task wrapper can drop between ticks safely (no `await` mid-scan).
- **Snapshot helpers:** added `EngineShell::paused_sync_root_ids`, `overflow_sync_root_ids`, `scheduler_sync_root_ids` so the daemon can build the snapshot from a borrowed `EngineShell` view.
- This is **distinct** from the existing daemon `integrity_sweeper_service` (which scrubs cached files on disk). The new sweeper operates at the engine-state layer.

### Circuit breaker (M-6.x)

- `crates/pcloud-resilience/src/circuit_breaker.rs` already exists from Stream C — left untouched per task instructions.

## Files modified / created

- `crates/pcloud-engine/src/power.rs` (new, 200 LOC, 4 unit tests)
- `crates/pcloud-engine/src/divergence_sweeper.rs` (new, 415 LOC, 9 unit tests)
- `crates/pcloud-engine/src/lib.rs` (module registration + 3 snapshot accessors on `EngineShell`)
- `crates/pcloud-config/src/sync_loop.rs` (added `pause_on_battery` field + 2 tests)
- `crates/pcloud-daemon/src/sync_loop.rs` (added `run_cycle_with_power` + 4 unit tests covering all four PowerState branches)

## Constraint compliance

- **Opt-in defaults:** both new features default to off (`pause_on_battery = false`, `DivergenceSweeperConfig::default().enabled = false`).
- **No behavioural change** unless a config opts in.
- **Cancellation-safe:** `tick_if_due` is synchronous; sync-loop battery gate is a single bool check.
- **`cargo fmt` applied** (auto-applied by editor lint hook on save).
- **No untouched-zone modifications:** circuit breaker, transport retry, secret wrappers, crypto, FUSE, docs, packaging all untouched.
- **No new workspace dependencies needed.**

## Verification commands

```
cargo check -p pcloud-engine -p pcloud-daemon -p pcloud-resilience  # clean
cargo test -p pcloud-engine --lib                                    # 110 passing
cargo test -p pcloud-daemon --lib sync_loop::                        # 14 passing (4 new)
```

## Open follow-ups (not in scope)

- The divergence sweeper is plumbed but not yet **wired** into the daemon runtime as a tokio interval task; doing so requires integrating with `RuntimeShell` lock semantics, which crosses into runtime composition territory more usefully owned by a follow-up bead. The sweeper is fully unit-tested against synthetic snapshots; daemon integration is one tokio task on top.
- `SyncLoopRuntime::poll_remote_diff` and friends could expose engine snapshots directly so the daemon can drive the sweeper from a tokio task without re-locking the engine. Out of scope here.

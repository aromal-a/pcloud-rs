//! Sync-state divergence sweeper (audit-06 M-4.2).
//!
//! ## Purpose
//!
//! Periodic, low-priority background scan that walks the sync-engine
//! state and detects divergences between the local DB view and the
//! remote tree (e.g. orphaned planner overflow entries, stale paused
//! sync IDs not in the active root list, unreconciled candidates with
//! no matching scan record). Divergent rows are recorded in an
//! in-memory **quarantine list** for operator review through the IPC
//! surface — the sweeper never silently rewrites or deletes engine
//! state on its own.
//!
//! This is distinct from the daemon's `integrity_sweeper_service`,
//! which scrubs cached files on disk. That service operates at the
//! filesystem layer; this sweeper operates at the sync-engine state
//! layer.
//!
//! ## Security and behavioural posture
//!
//! - **Opt-in.** [`DivergenceSweeperConfig::default`] returns
//!   `enabled = false`. A daemon that has never seen this block
//!   behaves exactly as it did before this module existed. Calling
//!   [`DivergenceSweeper::tick_if_due`] is a no-op when the config has
//!   `enabled = false`.
//! - **Read-only.** The sweeper never mutates the engine. It snapshots
//!   the divergence count and quarantine entries; an operator must
//!   take action through the standard IPC paths (`pause_sync_root`,
//!   `resume_sync_root`, etc.) — the sweeper deliberately does **not**
//!   call those itself.
//! - **Cancellation-safe.** The daemon-side wrapper drives this
//!   sweeper from a tokio task that calls [`tick_if_due`] on a timer.
//!   Each tick is a synchronous, bounded scan of in-memory engine
//!   state — there is no `await` point inside, so cancellation drops
//!   the next tick at most, never mid-scan.
//!
//! ## Quarantine model
//!
//! Each detected divergence is recorded as a [`QuarantineEntry`].
//! The list is a bounded ring (default 1024 entries) so a runaway
//! divergence storm cannot exhaust memory. Once full, oldest entries
//! are evicted with the eviction count surfaced to operators via
//! [`DivergenceSweeper::evicted_count`].

// **PLATFORM:** all
// **GATING:** none (portable; pure in-memory state machine).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use pcloud_model::ids::SyncId;
use serde::{Deserialize, Serialize};

/// Default sweep period: 24 hours. Operators that want a more
/// aggressive sweep set `period_secs` explicitly in config.
pub const DEFAULT_PERIOD_SECS: u64 = 24 * 60 * 60;

/// Hard cap on quarantine ring size. Older entries are evicted FIFO
/// once this is exceeded. Sized so a runaway storm cannot exceed a few
/// hundred KiB of in-memory state at typical entry size.
pub const MAX_QUARANTINE_ENTRIES: usize = 1024;

/// Minimum sweep period accepted by validation. Sweeps shorter than 60
/// seconds are rejected because the scan walks every active sync
/// root's queues; sub-minute cadence offers no value over the existing
/// per-cycle reconciliation.
pub const MIN_PERIOD_SECS: u64 = 60;

/// Maximum sweep period accepted by validation: 7 days. Longer cadence
/// would let drift accumulate beyond audit relevance.
pub const MAX_PERIOD_SECS: u64 = 7 * 24 * 60 * 60;

/// Configuration for the divergence sweeper. Lives next to
/// `SyncLoopConfig` in [`pcloud_config::sync_loop`] and is wired
/// through the daemon runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DivergenceSweeperConfig {
    /// Master switch. Default: `false`. When `false`, the sweeper task
    /// performs no I/O and produces no quarantine entries regardless
    /// of the other fields.
    #[serde(default)]
    pub enabled: bool,
    /// Sweep period in seconds. Default: 86 400 (24 h). Validation
    /// clamps to `[60, 604_800]`.
    #[serde(default = "default_period_secs")]
    pub period_secs: u64,
}

impl Default for DivergenceSweeperConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            period_secs: default_period_secs(),
        }
    }
}

fn default_period_secs() -> u64 {
    DEFAULT_PERIOD_SECS
}

impl DivergenceSweeperConfig {
    /// Validate config bounds.
    ///
    /// # Errors
    ///
    /// Returns a static description of the first violation.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.period_secs < MIN_PERIOD_SECS || self.period_secs > MAX_PERIOD_SECS {
            return Err("sync.divergence_sweeper.period_secs must be between 60 and 604800");
        }
        Ok(())
    }
}

/// Reason a row was quarantined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DivergenceKind {
    /// A `SyncId` is recorded as paused in the engine but is not
    /// present in the active sync-root list — likely a stale entry
    /// from a removed root that wasn't fully cleaned up.
    OrphanPausedRoot,
    /// A planner-overflow entry exists for a `SyncId` that has no
    /// active root record.
    OrphanOverflow,
    /// A scheduler queue entry references a paused root — should have
    /// been evicted on pause but wasn't.
    SchedulerOverlap,
}

/// One quarantine record. Operator inspects via the IPC surface and
/// decides whether to ignore, pause, or remove the offending root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineEntry {
    /// Sync root the divergence concerns. `None` if the divergence is
    /// not tied to a specific root.
    pub sync_id: Option<u64>,
    /// What kind of divergence was detected.
    pub kind: DivergenceKind,
    /// Free-form detail, e.g. the path or count. Intentionally **not**
    /// secret-bearing.
    pub detail: String,
    /// Monotonic tick number this entry was recorded on (for ordering
    /// + cheap ring eviction tracking).
    pub recorded_at_tick: u64,
}

/// Live state container. The daemon-side task owns one of these and
/// calls [`tick_if_due`] from a tokio interval.
pub struct DivergenceSweeper {
    config: DivergenceSweeperConfig,
    last_run: Option<Instant>,
    tick_count: u64,
    evicted: u64,
    quarantine: VecDeque<QuarantineEntry>,
}

/// Snapshot of sweeper state for IPC reporting. Reusable from the
/// daemon's status endpoints without exposing internal types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DivergenceSweeperStatus {
    /// Whether the sweeper is enabled in config.
    pub enabled: bool,
    /// Number of completed sweep ticks.
    pub ticks_completed: u64,
    /// Current size of the quarantine ring.
    pub quarantine_len: usize,
    /// Number of entries evicted from the ring due to overflow.
    pub evicted: u64,
    /// Whether the sweeper has at least one quarantine entry pending
    /// operator review.
    pub has_pending: bool,
}

/// Read-only view of the engine state that the sweeper inspects. The
/// engine in this crate currently exposes the live state through
/// `EngineShell` accessors; the sweeper takes a borrowed view rather
/// than `&EngineShell` directly so callers can build the snapshot from
/// any combination of sources (engine + DB + remote tree).
#[derive(Debug, Clone)]
pub struct EngineSnapshot<'a> {
    /// All `SyncId`s the engine considers paused.
    pub paused_roots: &'a [SyncId],
    /// All `SyncId`s with at least one operation in the planner
    /// overflow buffer.
    pub overflow_sync_ids: &'a [SyncId],
    /// All `SyncId`s with at least one operation queued in the
    /// scheduler.
    pub scheduler_sync_ids: &'a [SyncId],
    /// `SyncId`s the runtime considers active (i.e. present in the
    /// sync-root DB and not removed).
    pub active_roots: &'a [SyncId],
}

impl DivergenceSweeper {
    /// Construct from validated config.
    #[must_use]
    pub fn new(config: DivergenceSweeperConfig) -> Self {
        Self {
            config,
            last_run: None,
            tick_count: 0,
            evicted: 0,
            quarantine: VecDeque::new(),
        }
    }

    /// Whether the sweeper is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Snapshot for IPC reporting.
    #[must_use]
    pub fn status(&self) -> DivergenceSweeperStatus {
        DivergenceSweeperStatus {
            enabled: self.config.enabled,
            ticks_completed: self.tick_count,
            quarantine_len: self.quarantine.len(),
            evicted: self.evicted,
            has_pending: !self.quarantine.is_empty(),
        }
    }

    /// Number of quarantine entries evicted due to ring overflow.
    #[must_use]
    pub fn evicted_count(&self) -> u64 {
        self.evicted
    }

    /// Borrow the current quarantine list.
    #[must_use]
    pub fn quarantine(&self) -> &VecDeque<QuarantineEntry> {
        &self.quarantine
    }

    /// Drain and return all quarantine entries. Intended for IPC
    /// "review and clear" operator workflows.
    pub fn drain_quarantine(&mut self) -> Vec<QuarantineEntry> {
        self.quarantine.drain(..).collect()
    }

    /// Run a sweep tick if the configured period has elapsed since the
    /// last run (or this is the first call). Returns `true` if a sweep
    /// actually executed, `false` if it was skipped (disabled or not
    /// yet due).
    ///
    /// Cancellation: each tick is fully synchronous; dropping the
    /// future that hosts the tokio interval drops the next call at
    /// most, never mid-scan.
    pub fn tick_if_due(&mut self, now: Instant, snapshot: &EngineSnapshot<'_>) -> bool {
        if !self.config.enabled {
            return false;
        }
        let due = match self.last_run {
            None => true,
            Some(t) => {
                now.saturating_duration_since(t) >= Duration::from_secs(self.config.period_secs)
            }
        };
        if !due {
            return false;
        }
        self.run_tick(now, snapshot);
        true
    }

    /// Force a sweep tick regardless of the period (used by IPC "run
    /// now" admin command).
    pub fn run_now(&mut self, now: Instant, snapshot: &EngineSnapshot<'_>) {
        self.run_tick(now, snapshot);
    }

    fn run_tick(&mut self, now: Instant, snapshot: &EngineSnapshot<'_>) {
        self.last_run = Some(now);
        self.tick_count = self.tick_count.saturating_add(1);

        // Detect orphan paused roots.
        for &sid in snapshot.paused_roots {
            if !snapshot.active_roots.contains(&sid) {
                self.push(QuarantineEntry {
                    sync_id: Some(sid.0),
                    kind: DivergenceKind::OrphanPausedRoot,
                    detail: format!("paused sync_id {} not present in active root list", sid.0),
                    recorded_at_tick: self.tick_count,
                });
            }
        }
        // Detect overflow entries with no active root.
        for &sid in snapshot.overflow_sync_ids {
            if !snapshot.active_roots.contains(&sid) {
                self.push(QuarantineEntry {
                    sync_id: Some(sid.0),
                    kind: DivergenceKind::OrphanOverflow,
                    detail: format!(
                        "planner overflow contains entries for inactive sync_id {}",
                        sid.0
                    ),
                    recorded_at_tick: self.tick_count,
                });
            }
        }
        // Detect scheduler entries for paused roots.
        for &sid in snapshot.scheduler_sync_ids {
            if snapshot.paused_roots.contains(&sid) {
                self.push(QuarantineEntry {
                    sync_id: Some(sid.0),
                    kind: DivergenceKind::SchedulerOverlap,
                    detail: format!("scheduler retains queued ops for paused sync_id {}", sid.0),
                    recorded_at_tick: self.tick_count,
                });
            }
        }
    }

    fn push(&mut self, entry: QuarantineEntry) {
        if self.quarantine.len() >= MAX_QUARANTINE_ENTRIES {
            self.quarantine.pop_front();
            self.evicted = self.evicted.saturating_add(1);
        }
        self.quarantine.push_back(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap<'a>(
        paused: &'a [SyncId],
        overflow: &'a [SyncId],
        sched: &'a [SyncId],
        active: &'a [SyncId],
    ) -> EngineSnapshot<'a> {
        EngineSnapshot {
            paused_roots: paused,
            overflow_sync_ids: overflow,
            scheduler_sync_ids: sched,
            active_roots: active,
        }
    }

    #[test]
    fn default_config_is_disabled() {
        let cfg = DivergenceSweeperConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.period_secs, DEFAULT_PERIOD_SECS);
        cfg.validate().unwrap();
    }

    #[test]
    fn validation_rejects_out_of_range_period() {
        let mut cfg = DivergenceSweeperConfig {
            period_secs: 10,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        cfg.period_secs = MAX_PERIOD_SECS + 1;
        assert!(cfg.validate().is_err());
        cfg.period_secs = 3600;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn disabled_sweeper_never_ticks() {
        let cfg = DivergenceSweeperConfig::default();
        let mut sweeper = DivergenceSweeper::new(cfg);
        let s = snap(&[], &[], &[], &[]);
        assert!(!sweeper.tick_if_due(Instant::now(), &s));
        assert_eq!(sweeper.status().ticks_completed, 0);
    }

    #[test]
    fn first_tick_runs_on_due() {
        let cfg = DivergenceSweeperConfig {
            enabled: true,
            period_secs: 60,
        };
        let mut sweeper = DivergenceSweeper::new(cfg);
        let active = [SyncId::new(1)];
        let s = snap(&[], &[], &[], &active);
        assert!(sweeper.tick_if_due(Instant::now(), &s));
        assert_eq!(sweeper.status().ticks_completed, 1);
        assert_eq!(sweeper.status().quarantine_len, 0);
    }

    #[test]
    fn detects_orphan_paused_root() {
        let cfg = DivergenceSweeperConfig {
            enabled: true,
            period_secs: 60,
        };
        let mut sweeper = DivergenceSweeper::new(cfg);
        let paused = [SyncId::new(99)];
        let active = [SyncId::new(1)];
        let s = snap(&paused, &[], &[], &active);
        assert!(sweeper.tick_if_due(Instant::now(), &s));
        let q = sweeper.quarantine();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, DivergenceKind::OrphanPausedRoot);
        assert_eq!(q[0].sync_id, Some(99));
    }

    #[test]
    fn detects_scheduler_overlap_with_paused_root() {
        let cfg = DivergenceSweeperConfig {
            enabled: true,
            period_secs: 60,
        };
        let mut sweeper = DivergenceSweeper::new(cfg);
        let paused = [SyncId::new(7)];
        let sched = [SyncId::new(7)];
        let active = [SyncId::new(7)];
        let s = snap(&paused, &[], &sched, &active);
        assert!(sweeper.tick_if_due(Instant::now(), &s));
        assert_eq!(sweeper.quarantine().len(), 1);
        assert_eq!(
            sweeper.quarantine()[0].kind,
            DivergenceKind::SchedulerOverlap
        );
    }

    #[test]
    fn does_not_run_before_period_elapsed() {
        let cfg = DivergenceSweeperConfig {
            enabled: true,
            period_secs: 60,
        };
        let mut sweeper = DivergenceSweeper::new(cfg);
        let s = snap(&[], &[], &[], &[]);
        let t0 = Instant::now();
        assert!(sweeper.tick_if_due(t0, &s));
        // Same instant → not due yet.
        assert!(!sweeper.tick_if_due(t0, &s));
        assert_eq!(sweeper.status().ticks_completed, 1);
    }

    #[test]
    fn drain_quarantine_clears_state() {
        let cfg = DivergenceSweeperConfig {
            enabled: true,
            period_secs: 60,
        };
        let mut sweeper = DivergenceSweeper::new(cfg);
        let paused = [SyncId::new(99)];
        let active = [SyncId::new(1)];
        let s = snap(&paused, &[], &[], &active);
        assert!(sweeper.tick_if_due(Instant::now(), &s));
        let drained = sweeper.drain_quarantine();
        assert_eq!(drained.len(), 1);
        assert!(sweeper.quarantine().is_empty());
        assert!(!sweeper.status().has_pending);
    }

    #[test]
    fn run_now_bypasses_period() {
        let cfg = DivergenceSweeperConfig {
            enabled: true,
            period_secs: 60,
        };
        let mut sweeper = DivergenceSweeper::new(cfg);
        let s = snap(&[], &[], &[], &[]);
        let t0 = Instant::now();
        sweeper.run_now(t0, &s);
        sweeper.run_now(t0, &s);
        // Both ticks ran despite zero elapsed time.
        assert_eq!(sweeper.status().ticks_completed, 2);
    }

    #[test]
    fn quarantine_ring_evicts_oldest_when_full() {
        let cfg = DivergenceSweeperConfig {
            enabled: true,
            period_secs: 60,
        };
        let mut sweeper = DivergenceSweeper::new(cfg);
        // Force-fill past capacity by repeatedly running with a new
        // orphan ID each tick.
        for i in 0..(MAX_QUARANTINE_ENTRIES + 5) {
            let paused = [SyncId::new((i + 1) as u64)];
            let active = [SyncId::new(0)];
            let s = snap(&paused, &[], &[], &active);
            sweeper.run_now(Instant::now(), &s);
        }
        assert_eq!(sweeper.quarantine().len(), MAX_QUARANTINE_ENTRIES);
        assert_eq!(sweeper.evicted_count(), 5);
    }
}

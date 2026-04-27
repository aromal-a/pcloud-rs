//! Priority-aware scheduler queue for planned operations.
//!
//! The scheduler owns the ordered queue of [`PlannedOperation`]s
//! produced by [`crate::planner::Planner`] and hands out the next
//! batch to the transfer coordinators. Ordering is
//! **priority-then-path**: operation priorities are defined by
//! [`PlannedOperation::priority`] (lower = more urgent), with lexical
//! path ordering as a deterministic tiebreaker.
//!
//! # Batch semantics
//!
//! `Scheduler::next_batch` is a **peek** — it does not mutate the
//! queue. The embedding engine call
//! [`crate::EngineShell::advance_transfer_cycle`] hands each batch to
//! the upload and download coordinators; those coordinators manage
//! their own in-flight slots. The batch width is bounded by
//! `max_parallel_uploads + max_parallel_downloads` (minimum 1) so the
//! coordinators can always make forward progress.
//!
//! # Eviction
//!
//! `Scheduler::evict_sync_id` removes every queued operation that
//! belongs to a sync root being paused or removed, matching the C
//! `psync_delete_sync` teardown behavior.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::ids::SyncId;
use pcloud_model::sync::PlannedOperation;

/// Priority-ordered queue of [`PlannedOperation`]s plus parallelism
/// limits, used to hand out the next ready batch to the transfer
/// coordinators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheduler {
    /// Maximum number of upload operations that may be in flight at
    /// once across all sync roots. Default is 4.
    pub max_parallel_uploads: usize,
    /// Maximum number of download operations that may be in flight at
    /// once across all sync roots. Default is 4.
    pub max_parallel_downloads: usize,
    /// Operations waiting to be dispatched, ordered by
    /// [`PlannedOperation::priority`] (ascending) then path. Conflicts
    /// therefore appear first, followed by deletes, directory
    /// operations, then file transfers.
    pub queued_operations: Vec<PlannedOperation>,
    /// P2-b (H2): operations that have been handed out via
    /// [`Self::peek_batch`] (or the draining `next_batch`) and are
    /// currently in flight with a transfer coordinator. They are kept
    /// here until [`Self::ack_batch`] confirms durable remote
    /// acknowledgement (upload save / download complete). Persisted
    /// snapshots include this slot so that a crash between dispatch
    /// and ack re-queues the in-flight work on restart rather than
    /// silently losing it.
    #[serde(default)]
    pub dispatched_operations: Vec<PlannedOperation>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            max_parallel_uploads: 4,
            max_parallel_downloads: 4,
            queued_operations: Vec::new(),
            dispatched_operations: Vec::new(),
        }
    }
}

impl Scheduler {
    /// Replace the queued operations with `operations`, sorting them by
    /// operation priority and then path for deterministic ordering.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.replace_queue(vec![PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "a".into(),
    /// }]);
    /// assert_eq!(s.next_batch().len(), 1);
    /// ```
    pub fn replace_queue(&mut self, mut operations: Vec<PlannedOperation>) {
        operations.sort_by(|left, right| {
            left.priority()
                .cmp(&right.priority())
                .then(left.path().cmp(right.path()))
        });
        self.queued_operations = operations;
    }

    /// Replace the queued operations belonging to `sync_id` with
    /// `operations`, preserving every queued item whose `sync_id` differs.
    ///
    /// This is the primary entry point for per-root re-planning: each
    /// sync root independently re-plans its own `PlannedOperation` list
    /// without clobbering work that other roots have already enqueued.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.replace_queue(vec![
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(1), path: "a".into() },
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(2), path: "b".into() },
    /// ]);
    /// // Replan only root 1. Root 2's entry survives.
    /// s.replace_queue_for_sync_id(
    ///     SyncId::new(1),
    ///     vec![PlannedOperation::DeleteLocal { sync_id: SyncId::new(1), path: "c".into() }],
    /// );
    /// assert_eq!(s.queued_operations.len(), 2);
    /// assert!(s.queued_operations.iter().any(|op| op.sync_id() == SyncId::new(2)));
    /// ```
    pub fn replace_queue_for_sync_id(
        &mut self,
        sync_id: SyncId,
        operations: Vec<PlannedOperation>,
    ) {
        let mut merged: Vec<PlannedOperation> = self
            .queued_operations
            .drain(..)
            .filter(|op| op.sync_id() != sync_id)
            .collect();
        merged.extend(operations);
        merged.sort_by(|left, right| {
            left.priority()
                .cmp(&right.priority())
                .then(left.path().cmp(right.path()))
        });
        self.queued_operations = merged;
    }

    /// Remove all queued operations belonging to `sync_id`.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.replace_queue(vec![
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(1), path: "a".into() },
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(2), path: "b".into() },
    /// ]);
    /// s.evict_sync_id(SyncId::new(1));
    /// assert_eq!(s.queued_operations.len(), 1);
    /// ```
    pub fn evict_sync_id(&mut self, sync_id: SyncId) {
        self.queued_operations
            .retain(|operation| operation.sync_id() != sync_id);
        // P2-b (H2): also drop any dispatched-but-not-acked operations
        // for this sync root so a paused/removed root does not leave
        // phantom entries in the durable snapshot.
        self.dispatched_operations
            .retain(|operation| operation.sync_id() != sync_id);
    }

    /// Drain and return the next fair batch of operations, removing the
    /// dispatched items from the queue.
    ///
    /// The batch width is bounded by `max_parallel_uploads +
    /// max_parallel_downloads`. Per-root fairness is enforced: at most
    /// `(max_parallel_uploads + max_parallel_downloads) / 2` (min 1)
    /// operations from any single [`SyncId`] are included, so a
    /// high-throughput root cannot starve sibling roots.
    ///
    /// Items removed by `next_batch` will not appear in subsequent calls
    /// until they are re-enqueued (e.g. on retry).
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// // An empty scheduler returns an empty batch (never a panic).
    /// assert!(s.next_batch().is_empty());
    ///
    /// s.replace_queue(vec![PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "a.txt".into(),
    /// }]);
    /// let batch = s.next_batch();
    /// assert_eq!(batch.len(), 1);
    /// assert_eq!(s.total_queued(), 0, "items drained after next_batch");
    /// ```
    pub fn next_batch(&mut self) -> Vec<PlannedOperation> {
        let batch = self.take_fair_batch();
        // P2-b (H2): record dispatched work until ack_batch confirms it.
        for op in &batch {
            self.dispatched_operations.push(op.clone());
        }
        batch
    }

    /// P2-b (H2) peek-variant: return the next fair batch **without**
    /// removing items from the queue. The caller is expected to
    /// call [`Self::ack_batch`] on each operation once the upload /
    /// download has been durably acknowledged by the server. If the
    /// daemon crashes between peek and ack the peeked items stay in
    /// `queued_operations` and are re-dispatched on restart.
    ///
    /// Note: because this peek does **not** mutate the queue, a naive
    /// tight loop calling `peek_batch` will return the same items over
    /// and over. Integrations that want at-most-in-flight semantics
    /// must either (a) drain with [`Self::next_batch`] and re-enqueue
    /// on failure, or (b) track in-flight paths externally. The
    /// current sync-loop path uses [`Self::next_batch`] + the
    /// coordinator `active_*` lists to achieve the same effect while
    /// also benefitting from the audit-05 H2 durability semantics via
    /// `dispatched_operations`.
    #[must_use]
    pub fn peek_batch(&self) -> Vec<PlannedOperation> {
        // Misuse guard: callers that call peek_batch in a tight loop without
        // draining or tracking in-flight paths externally will spin on the
        // same items forever. This is not incorrect (peek is non-mutating by
        // design) but indicates a logic error in the integration. The
        // recommended pattern is next_batch() + ack_batch() or external
        // in-flight tracking. See doc-comment above.
        // audit-06 LOW sync L-4.3 / ncx.81-c.
        debug_assert!(
            !self.queued_operations.is_empty() || self.dispatched_operations.is_empty(),
            "peek_batch called on a scheduler with no queued ops but with dispatched ops:              likely a tight-loop integration that should use next_batch + ack_batch instead"
        );
        self.peek_fair_batch_cloned()
    }

    fn peek_fair_batch_cloned(&self) -> Vec<PlannedOperation> {
        let global_limit = (self.max_parallel_uploads + self.max_parallel_downloads).max(1);
        if self.queued_operations.is_empty() {
            return Vec::new();
        }
        let distinct_roots: std::collections::HashSet<SyncId> = self
            .queued_operations
            .iter()
            .map(|op| op.sync_id())
            .collect();
        let num_roots = distinct_roots.len().max(1);
        let per_root_cap = global_limit.div_ceil(num_roots).max(1);
        let mut per_root: std::collections::HashMap<SyncId, usize> =
            std::collections::HashMap::new();
        let mut out = Vec::with_capacity(global_limit);
        for op in &self.queued_operations {
            if out.len() >= global_limit {
                break;
            }
            let count = per_root.entry(op.sync_id()).or_insert(0);
            if *count < per_root_cap {
                *count += 1;
                out.push(op.clone());
            }
        }
        out
    }

    /// P2-b (H2): remove the named `(sync_id, path)` entries from the
    /// `dispatched_operations` slot. Called by the embedding runtime
    /// after each successful upload/download completion. Operations
    /// still in `dispatched_operations` after a daemon shutdown will
    /// be restored into `queued_operations` on the next boot so retry
    /// is guaranteed.
    ///
    /// **Audit-06 §4-sonnet M-04-S04 / P1-9:** match is on
    /// `(sync_id, path)` tuples rather than path alone. Two sync roots
    /// that happen to share the same relative path (e.g. a README
    /// inside each of two independent sync trees) otherwise caused a
    /// single ack to evict both dispatched entries, silently dropping
    /// the crash-recovery guarantee for the un-acked root.
    ///
    /// **Audit-06 §4-opus M-4.2 / ncx.40:** for wide batches the naive
    /// O(N·M) `retain` nested-scan scales poorly (N = dispatched slot
    /// size, M = ack batch width). We build a `HashSet<(SyncId,
    /// &str)>` index over `items` once, then do a single O(N) pass
    /// over `dispatched_operations` with O(1) membership tests. The
    /// index is only built when both the ack batch and the dispatched
    /// slot are non-trivial, so the common tight-loop case (single
    /// ack) does not pay the hashing overhead.
    pub fn ack_batch(&mut self, items: &[(SyncId, &str)]) {
        if items.is_empty() || self.dispatched_operations.is_empty() {
            return;
        }
        // Fast path: a single ack is cheaper as a direct scan than as a
        // hashed lookup (cache-friendly, no allocation).
        if items.len() == 1 {
            let (sid, p) = items[0];
            self.dispatched_operations
                .retain(|op| !(op.sync_id() == sid && op.path() == p));
            return;
        }
        // General path: build an index of (sync_id, path) keys and do a
        // single linear pass with O(1) lookups. ncx.40 hardening.
        let index: std::collections::HashSet<(SyncId, &str)> =
            items.iter().map(|(sid, p)| (*sid, *p)).collect();
        self.dispatched_operations
            .retain(|op| !index.contains(&(op.sync_id(), op.path())));
    }

    fn take_fair_batch(&mut self) -> Vec<PlannedOperation> {
        let global_limit = (self.max_parallel_uploads + self.max_parallel_downloads).max(1);
        if self.queued_operations.is_empty() {
            return Vec::new();
        }

        // Count distinct roots present in the queue so we can compute a
        // fair per-root cap. Scanning the whole queue is O(N) but N is
        // bounded by the planner's per-tick cap (~1000 items) in practice.
        let distinct_roots: std::collections::HashSet<SyncId> = self
            .queued_operations
            .iter()
            .map(|op| op.sync_id())
            .collect();
        let num_roots = distinct_roots.len().max(1);
        // Each root gets at least 1 slot; distribute remaining slots evenly.
        let per_root_cap = global_limit.div_ceil(num_roots).max(1);

        let mut per_root: std::collections::HashMap<SyncId, usize> =
            std::collections::HashMap::new();
        let mut batch_indices: Vec<usize> = Vec::with_capacity(global_limit);
        for (i, op) in self.queued_operations.iter().enumerate() {
            if batch_indices.len() >= global_limit {
                break;
            }
            let count = per_root.entry(op.sync_id()).or_insert(0);
            if *count < per_root_cap {
                *count += 1;
                batch_indices.push(i);
            }
        }
        // Remove in reverse order to preserve index validity.
        let mut batch: Vec<PlannedOperation> = Vec::with_capacity(batch_indices.len());
        for &i in batch_indices.iter().rev() {
            batch.push(self.queued_operations.remove(i));
        }
        batch.reverse();
        batch
    }

    /// Total number of operations currently in the queue (across all roots).
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// assert_eq!(s.total_queued(), 0);
    /// s.replace_queue(vec![PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "x.txt".into(),
    /// }]);
    /// assert_eq!(s.total_queued(), 1);
    /// ```
    #[must_use]
    pub fn total_queued(&self) -> usize {
        self.queued_operations.len()
    }

    /// Drain and return the next batch, removing the dispatched items from
    /// the queue. Equivalent to [`Self::next_batch`] but without per-root
    /// fairness: takes the first `max_parallel_uploads +
    /// max_parallel_downloads` items in priority order, which may all
    /// belong to a single [`SyncId`].
    ///
    /// # Deprecation — M-4.6
    ///
    /// Prefer [`Self::next_batch`] which enforces per-root fairness. This
    /// method is retained for backward compatibility but is no longer
    /// recommended in production sync-loop code; a high-throughput root
    /// can starve sibling roots when this variant is used.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.replace_queue(vec![PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "a.txt".into(),
    /// }]);
    /// let batch = s.drain_batch();
    /// assert_eq!(batch.len(), 1);
    /// assert_eq!(s.total_queued(), 0);
    /// ```
    #[deprecated(
        since = "0.1.0",
        note = "Unfair: a single high-throughput sync root can starve siblings. \
                Use `next_batch` instead (M-4.6)."
    )]
    pub fn drain_batch(&mut self) -> Vec<PlannedOperation> {
        let limit = self.max_parallel_uploads + self.max_parallel_downloads;
        let limit = limit.max(1).min(self.queued_operations.len());
        self.queued_operations.drain(..limit).collect()
    }

    /// Append a single operation to the end of the queue without replacing
    /// existing entries. Useful in tests and for dead-letter replay where
    /// individual items must be re-enqueued without clobbering other roots.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.enqueue(PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "y.txt".into(),
    /// });
    /// assert_eq!(s.total_queued(), 1);
    /// ```
    pub fn enqueue(&mut self, operation: PlannedOperation) {
        self.queued_operations.push(operation);
    }

    /// Peek at the next batch with a per-root operation cap applied.
    ///
    /// Works identically to [`Self::next_batch`] but enforces that at most
    /// `max_per_root` operations from any single [`SyncId`] appear in the
    /// returned slice. This prevents a high-throughput root from monopolising
    /// the entire batch window.
    ///
    /// The overall batch width is still bounded by
    /// `max_parallel_uploads + max_parallel_downloads`.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.replace_queue(vec![
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(1), path: "a".into() },
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(1), path: "b".into() },
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(1), path: "c".into() },
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(2), path: "d".into() },
    /// ]);
    /// // With max_per_root=2, root 1 contributes at most 2 slots even though
    /// // it has 3 queued items.
    /// let batch = s.next_batch_fair(2);
    /// let root1_count = batch.iter().filter(|op| op.sync_id() == SyncId::new(1)).count();
    /// assert!(root1_count <= 2);
    /// ```
    #[must_use]
    pub fn next_batch_fair(&self, max_per_root: usize) -> Vec<&PlannedOperation> {
        self.peek_batch_fair(max_per_root)
    }

    /// Peek a fair batch without mutating scheduler state.
    ///
    /// This is the non-misleading name for [`Self::next_batch_fair`]:
    /// the method returns borrowed references to the front of
    /// `queued_operations` without consuming them, exactly mirroring the
    /// non-fair [`Self::peek_batch`]. The `next_batch_fair` name is
    /// retained for source compatibility but will be removed in a future
    /// release.
    ///
    /// audit-06 LOW sync L-04 / pcloud-rs-ncx.81-e.
    #[must_use]
    pub fn peek_batch_fair(&self, max_per_root: usize) -> Vec<&PlannedOperation> {
        let global_limit = (self.max_parallel_uploads + self.max_parallel_downloads).max(1);
        let mut per_root: std::collections::HashMap<SyncId, usize> =
            std::collections::HashMap::new();
        let mut batch = Vec::with_capacity(global_limit);
        for op in &self.queued_operations {
            if batch.len() >= global_limit {
                break;
            }
            let count = per_root.entry(op.sync_id()).or_insert(0);
            if *count < max_per_root {
                *count += 1;
                batch.push(op);
            }
        }
        batch
    }
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        conflict::ConflictKind,
        ids::{RemoteFileId, SyncId},
        sync::PlannedOperation,
    };

    use super::Scheduler;

    #[test]
    fn scheduler_orders_conflicts_before_transfers() {
        let mut scheduler = Scheduler::default();
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "b.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "b.txt".to_owned(),
            },
            PlannedOperation::Conflict {
                sync_id: SyncId::new(1),
                path: "a.txt".to_owned(),
                kind: ConflictKind::LocalModifyVsRemoteModify,
            },
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "a.bin".to_owned(),
                remote_file_id: Some(RemoteFileId::new(9)),
            },
        ]);

        assert!(matches!(
            scheduler.queued_operations.first(),
            Some(PlannedOperation::Conflict { .. })
        ));
    }

    #[test]
    fn scheduler_limits_batch_by_parallel_capacity() {
        let mut scheduler = Scheduler {
            max_parallel_uploads: 1,
            max_parallel_downloads: 1,
            queued_operations: Vec::new(),
            dispatched_operations: Vec::new(),
        };
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "a".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "a".to_owned(),
            },
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "b".to_owned(),
                remote_file_id: Some(RemoteFileId::new(2)),
            },
            PlannedOperation::DeleteLocal {
                sync_id: SyncId::new(1),
                path: "c".to_owned(),
            },
        ]);

        assert_eq!(scheduler.next_batch().len(), 2);
    }

    #[test]
    fn crash_between_dispatch_and_ack_recovers_work_on_restart() {
        // P2-b (H2) regression test. Simulates the crash window:
        //   1. `next_batch` dispatches work (drains queued, records
        //      `dispatched_operations`).
        //   2. Daemon crashes BEFORE `ack_batch` is called.
        //   3. On restart, the durable snapshot (queue ∪ dispatched)
        //      is used to rebuild the queue.
        //   4. The previously-dispatched-but-not-acked item must
        //      re-appear in `queued_operations` and not be silently
        //      lost.
        let mut scheduler = Scheduler::default();
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "a.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "a.txt".to_owned(),
            },
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "b.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "b.txt".to_owned(),
            },
        ]);

        // Step 1: dispatch.
        let batch = scheduler.next_batch();
        assert_eq!(batch.len(), 2);
        assert_eq!(scheduler.queued_operations.len(), 0);
        assert_eq!(
            scheduler.dispatched_operations.len(),
            2,
            "dispatched_operations must record in-flight work for H2 durability"
        );

        // Step 2: ack ONE but crash before the second (simulated by not
        // calling ack on b.txt).
        scheduler.ack_batch(&[(SyncId::new(1), "a.txt")]);
        assert_eq!(scheduler.dispatched_operations.len(), 1);
        assert_eq!(scheduler.dispatched_operations[0].path(), "b.txt");

        // Step 3: build the combined "durable snapshot" — what the
        // embedding runtime would persist to `sync.scheduler.queue`.
        // This mirrors EngineShell::snapshot_scheduler_durable.
        let mut durable: Vec<PlannedOperation> = scheduler.queued_operations.to_vec();
        durable.extend(scheduler.dispatched_operations.iter().cloned());

        // Step 4: simulate restart: fresh Scheduler, replace queue from
        // persisted snapshot. The unacked "b.txt" must be present.
        let mut restarted = Scheduler::default();
        restarted.replace_queue(durable);
        assert_eq!(restarted.queued_operations.len(), 1);
        assert_eq!(restarted.queued_operations[0].path(), "b.txt");
        assert!(
            restarted.dispatched_operations.is_empty(),
            "restart must start with clean dispatched slot; retry will re-populate"
        );
    }

    /// Audit-06 §4-sonnet M-04-S04 / P1-9 regression: two sync roots
    /// dispatch operations that happen to share the same relative path.
    /// Acking the path for root 1 MUST NOT silently evict root 2's
    /// dispatched entry (which would defeat H2 crash-recovery for that
    /// root's in-flight work).
    #[test]
    fn ack_batch_respects_sync_id_scope() {
        let mut scheduler = Scheduler::default();
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "README.md".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "README.md".to_owned(),
            },
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(2),
                path: "README.md".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "README.md".to_owned(),
            },
        ]);

        // Dispatch both: the dispatched slot holds one entry per root
        // even though both share the same relative path.
        let batch = scheduler.next_batch();
        assert_eq!(batch.len(), 2);
        assert_eq!(
            scheduler.dispatched_operations.len(),
            2,
            "both dispatched entries must be recorded"
        );

        // Ack only root 1's README.md. Root 2's entry MUST remain.
        scheduler.ack_batch(&[(SyncId::new(1), "README.md")]);
        assert_eq!(
            scheduler.dispatched_operations.len(),
            1,
            "only root 1's entry must be removed"
        );
        assert_eq!(
            scheduler.dispatched_operations[0].sync_id(),
            SyncId::new(2),
            "surviving dispatched entry must belong to root 2"
        );

        // Now ack root 2's README.md; dispatched slot drains.
        scheduler.ack_batch(&[(SyncId::new(2), "README.md")]);
        assert!(
            scheduler.dispatched_operations.is_empty(),
            "both dispatched entries must now be evicted"
        );
    }

    /// Audit-06 §4-opus M-4.2 / ncx.40 regression: `ack_batch` must
    /// handle wide batches against large dispatched slots without
    /// O(N·M) blow-up. This is a correctness test (not a strict timing
    /// assertion) that drives 1 000 acks against a 10 000-item
    /// dispatched slot and asserts (a) the survivors are exactly the
    /// unacked 9 000, (b) the operation completes in well under a
    /// wall-clock budget a quadratic scan would blow through.
    #[test]
    fn ack_batch_handles_wide_batches_in_linear_time() {
        let mut scheduler = Scheduler::default();
        let mut ops = Vec::with_capacity(10_000);
        for i in 0..10_000u64 {
            ops.push(PlannedOperation::UploadFile {
                sync_id: SyncId::new((i % 4) + 1),
                path: format!("file-{i:06}.bin"),
                remote_parent_folder_id: None,
                remote_name: format!("file-{i:06}.bin"),
            });
        }
        // Seed the dispatched slot directly (bypass next_batch to keep
        // the test independent of the fair-batch limits).
        scheduler.dispatched_operations = ops.clone();

        // Ack the first 1 000 entries.
        let ack_items: Vec<(SyncId, &str)> = ops[..1_000]
            .iter()
            .map(|op| (op.sync_id(), op.path()))
            .collect();

        let start = std::time::Instant::now();
        scheduler.ack_batch(&ack_items);
        let elapsed = start.elapsed();

        assert_eq!(
            scheduler.dispatched_operations.len(),
            9_000,
            "1000 acked out of 10000"
        );
        // Budget: 250ms is ~1000x larger than what a hashed index needs
        // on modern hardware but still tight enough to catch a true
        // quadratic regression on this workload. Tuned loose to avoid
        // CI flakiness while still asserting the hot path is not O(N*M).
        assert!(
            elapsed < std::time::Duration::from_millis(250),
            "ack_batch took {elapsed:?} for 1000 acks over 10000 dispatched — \
             likely O(N*M) regression of ncx.40"
        );

        // Additionally verify the survivors are exactly the unacked
        // tail: O(N*M) retains can also get correctness wrong under
        // aliasing, so we double-check.
        for (i, op) in scheduler.dispatched_operations.iter().enumerate() {
            let expected_idx = i + 1_000;
            assert_eq!(op.path(), format!("file-{expected_idx:06}.bin"));
        }
    }

    #[test]
    fn scheduler_evicts_operations_for_removed_sync_root() {
        let mut scheduler = Scheduler::default();
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "a".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "a".to_owned(),
            },
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(2),
                path: "b".to_owned(),
                remote_file_id: Some(RemoteFileId::new(2)),
            },
        ]);

        scheduler.evict_sync_id(SyncId::new(1));

        assert_eq!(scheduler.queued_operations.len(), 1);
        assert_eq!(scheduler.queued_operations[0].sync_id(), SyncId::new(2));
    }
}

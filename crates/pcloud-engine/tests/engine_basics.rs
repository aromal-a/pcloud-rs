//! Integration tests for the pcloud-engine crate.
//!
//! These tests exercise the public API surface of `EngineShell`,
//! `Scheduler`, and `ConflictResolver` to verify the correctness fixes
//! described in the audit:
//!
//! - `next_batch` actually removes items from the queue.
//! - Round-robin fairness: multiple sync roots are both served.
//! - `newest_wins` picks the item with the later mtime.
//! - `rename_both` produces a concrete `RenameBoth` resolution (two paths,
//!   not a no-op `ManualReview`).

use pcloud_engine::{
    EngineShell,
    conflict_resolver::{ConflictPolicy, ConflictResolver},
    scheduler::Scheduler,
};
use pcloud_model::{
    conflict::{ConflictKind, ConflictResolution},
    ids::{RemoteFileId, SyncId},
    sync::{ChangeKind, ChangeSource, EntryKind, PlannedOperation, SyncCandidate},
};

// ---------------------------------------------------------------------------
// Scheduler: next_batch removes items
// ---------------------------------------------------------------------------

#[test]
fn next_batch_removes_items_from_queue() {
    let mut scheduler = Scheduler::default();
    scheduler.replace_queue(vec![
        PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(1),
            path: "a.txt".into(),
        },
        PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(1),
            path: "b.txt".into(),
        },
        PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(1),
            path: "c.txt".into(),
        },
    ]);

    assert_eq!(scheduler.total_queued(), 3, "three items enqueued");

    let batch = scheduler.next_batch();
    assert_eq!(batch.len(), 3, "all three items returned (limit=8)");
    assert_eq!(
        scheduler.total_queued(),
        0,
        "queue is empty after batch is processed"
    );

    // Calling next_batch again on an empty queue is safe and returns empty.
    let second_batch = scheduler.next_batch();
    assert!(second_batch.is_empty(), "no double-emission of same ops");
}

#[test]
fn next_batch_with_limit_leaves_remainder_in_queue() {
    let mut scheduler = Scheduler {
        max_parallel_uploads: 1,
        max_parallel_downloads: 1, // limit = 2
        ..Default::default()
    };
    // Enqueue 5 items for the same root.
    for i in 0u64..5 {
        scheduler.enqueue(PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(1),
            path: format!("file{i}.txt"),
        });
    }

    assert_eq!(scheduler.total_queued(), 5);
    let batch = scheduler.next_batch();
    assert_eq!(batch.len(), 2, "batch bounded by parallelism limit");
    assert_eq!(
        scheduler.total_queued(),
        3,
        "remaining items still in queue"
    );
}

// ---------------------------------------------------------------------------
// Scheduler: round-robin fairness
// ---------------------------------------------------------------------------

#[test]
fn both_roots_get_scheduled_with_round_robin() {
    let mut scheduler = Scheduler {
        max_parallel_uploads: 2,
        max_parallel_downloads: 2, // limit = 4
        ..Default::default()
    };

    // Each root gets 4 items; limit = 4.
    for i in 0u64..4 {
        scheduler.enqueue(PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(1),
            path: format!("root1/file{i}.txt"),
        });
        scheduler.enqueue(PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(2),
            path: format!("root2/file{i}.txt"),
        });
    }

    assert_eq!(scheduler.total_queued(), 8);

    let batch = scheduler.next_batch();
    assert_eq!(batch.len(), 4);

    // Both roots must appear in the batch (round-robin fairness).
    let root1_count = batch
        .iter()
        .filter(|op| op.sync_id() == SyncId::new(1))
        .count();
    let root2_count = batch
        .iter()
        .filter(|op| op.sync_id() == SyncId::new(2))
        .count();

    assert!(root1_count >= 1, "root 1 starved: got 0 ops in batch");
    assert!(root2_count >= 1, "root 2 starved: got 0 ops in batch");

    // After one batch both roots still have items.
    assert!(
        scheduler.total_queued() < 8,
        "items were actually removed from queues"
    );
}

#[test]
fn single_busy_root_does_not_starve_idle_root() {
    // Root 1 has 10 items, root 2 has 1 item, limit = 4.
    let mut scheduler = Scheduler {
        max_parallel_uploads: 2,
        max_parallel_downloads: 2,
        ..Default::default()
    };
    for i in 0u64..10 {
        scheduler.enqueue(PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(1),
            path: format!("busy/file{i}.txt"),
        });
    }
    scheduler.enqueue(PlannedOperation::DeleteLocal {
        sync_id: SyncId::new(2),
        path: "quiet/file.txt".into(),
    });

    let batch = scheduler.next_batch();
    // Root 2's single item must appear in the first batch despite root 1
    // having more items.
    let root2_in_batch = batch.iter().any(|op| op.sync_id() == SyncId::new(2));
    assert!(
        root2_in_batch,
        "root 2 (quiet root) should appear in first batch; batch={batch:?}"
    );
}

#[test]
#[allow(deprecated)]
fn scheduler_peek_fair_drain_and_ack_edge_matrix() {
    let mut scheduler = Scheduler {
        max_parallel_uploads: 2,
        max_parallel_downloads: 2,
        ..Default::default()
    };
    assert!(scheduler.peek_batch().is_empty());
    assert!(scheduler.peek_batch_fair(1).is_empty());
    assert!(scheduler.next_batch_fair(1).is_empty());
    assert!(scheduler.drain_batch().is_empty());
    scheduler.ack_batch(&[]);
    scheduler.ack_batch(&[(SyncId::new(1), "absent")]);

    for (sync_id, path) in [
        (1, "a/one"),
        (1, "a/two"),
        (1, "a/three"),
        (2, "b/one"),
        (2, "b/two"),
        (3, "c/one"),
    ] {
        scheduler.enqueue(PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(sync_id),
            path: path.to_owned(),
        });
    }

    assert!(scheduler.peek_batch_fair(0).is_empty());
    let fair = scheduler.peek_batch();
    assert_eq!(fair.len(), 4);
    assert_eq!(scheduler.total_queued(), 6, "peek must not consume work");
    assert!(fair.iter().any(|op| op.sync_id() == SyncId::new(2)));
    assert_eq!(
        fair.iter()
            .filter(|op| op.sync_id() == SyncId::new(1))
            .count(),
        2
    );
    assert_eq!(scheduler.next_batch_fair(1).len(), 3);

    let dispatched = scheduler.next_batch();
    assert_eq!(dispatched.len(), 4);
    assert_eq!(scheduler.dispatched_operations.len(), 4);
    scheduler.ack_batch(&[(SyncId::new(1), "a/one")]);
    assert_eq!(scheduler.dispatched_operations.len(), 3);
    scheduler.ack_batch(&[
        (SyncId::new(1), "a/two"),
        (SyncId::new(2), "b/one"),
        (SyncId::new(99), "absent"),
    ]);
    assert_eq!(scheduler.dispatched_operations.len(), 1);

    let drained = scheduler.drain_batch();
    assert_eq!(drained.len(), 2);
    assert_eq!(scheduler.total_queued(), 0);

    let mut minimum_capacity = Scheduler {
        max_parallel_uploads: 0,
        max_parallel_downloads: 0,
        ..Default::default()
    };
    minimum_capacity.enqueue(PlannedOperation::DeleteLocal {
        sync_id: SyncId::new(9),
        path: "minimum".to_owned(),
    });
    assert_eq!(minimum_capacity.peek_batch().len(), 1);
}

// ---------------------------------------------------------------------------
// ConflictResolver: newest_wins correctness
// ---------------------------------------------------------------------------

fn make_conflict(kind: ConflictKind) -> PlannedOperation {
    PlannedOperation::Conflict {
        sync_id: SyncId::new(42),
        path: "shared/doc.txt".into(),
        kind,
    }
}

#[test]
fn newest_wins_picks_local_when_local_mtime_is_greater() {
    let resolver = ConflictResolver {
        default_policy: ConflictPolicy::NewestWins,
    };
    let op = make_conflict(ConflictKind::LocalModifyVsRemoteModify);

    // local_mtime (2000) > remote_mtime (1000) → prefer local → UploadFile
    let resolution = resolver
        .resolve(&op, Some(2_000), Some(1_000))
        .expect("should resolve");

    assert!(
        matches!(
            resolution,
            ConflictResolution::Apply(PlannedOperation::UploadFile { .. })
        ),
        "expected UploadFile (local wins), got {resolution:?}"
    );
}

#[test]
fn newest_wins_picks_remote_when_remote_mtime_is_greater() {
    let resolver = ConflictResolver {
        default_policy: ConflictPolicy::NewestWins,
    };
    let op = make_conflict(ConflictKind::LocalModifyVsRemoteModify);

    // remote_mtime (9999) > local_mtime (1000) → prefer remote → DownloadFile
    let resolution = resolver
        .resolve(&op, Some(1_000), Some(9_999))
        .expect("should resolve");

    assert!(
        matches!(
            resolution,
            ConflictResolution::Apply(PlannedOperation::DownloadFile { .. })
        ),
        "expected DownloadFile (remote wins), got {resolution:?}"
    );
}

#[test]
fn newest_wins_uses_remote_as_tiebreaker_on_equal_timestamps() {
    let resolver = ConflictResolver {
        default_policy: ConflictPolicy::NewestWins,
    };
    let op = make_conflict(ConflictKind::LocalModifyVsRemoteModify);

    // Equal timestamps → server-wins tie-break → DownloadFile
    let resolution = resolver
        .resolve(&op, Some(5_000), Some(5_000))
        .expect("should resolve");

    assert!(
        matches!(
            resolution,
            ConflictResolution::Apply(PlannedOperation::DownloadFile { .. })
        ),
        "expected DownloadFile (tie-break to remote), got {resolution:?}"
    );
}

#[test]
fn newest_wins_falls_back_to_remote_with_no_timestamps() {
    let resolver = ConflictResolver {
        default_policy: ConflictPolicy::NewestWins,
    };
    let op = make_conflict(ConflictKind::LocalModifyVsRemoteModify);

    // No timestamps → fall back to prefer-remote
    let resolution = resolver.resolve(&op, None, None).expect("should resolve");

    assert!(
        matches!(
            resolution,
            ConflictResolution::Apply(PlannedOperation::DownloadFile { .. })
        ),
        "expected DownloadFile (no-timestamp fallback), got {resolution:?}"
    );
}

// ---------------------------------------------------------------------------
// ConflictResolver: rename_both produces two concrete rename paths
// ---------------------------------------------------------------------------

#[test]
fn rename_both_produces_two_concrete_paths_not_manual_review() {
    let resolver = ConflictResolver {
        default_policy: ConflictPolicy::RenameBoth,
    };
    let op = make_conflict(ConflictKind::LocalModifyVsRemoteModify);

    let resolution = resolver.resolve(&op, None, None).expect("should resolve");

    match resolution {
        ConflictResolution::RenameBoth {
            local_renamed_path,
            remote_renamed_path,
            original_path,
            sync_id,
        } => {
            assert_eq!(original_path, "shared/doc.txt");
            assert_eq!(sync_id, SyncId::new(42));
            assert_ne!(
                local_renamed_path, original_path,
                "local rename must differ from original"
            );
            assert_ne!(
                remote_renamed_path, original_path,
                "remote rename must differ from original"
            );
            assert_ne!(
                local_renamed_path, remote_renamed_path,
                "local and remote rename paths must be distinct"
            );
            assert!(
                local_renamed_path.contains("conflict-local"),
                "local path should contain 'conflict-local': {local_renamed_path}"
            );
            assert!(
                remote_renamed_path.contains("conflict-remote"),
                "remote path should contain 'conflict-remote': {remote_renamed_path}"
            );
        }
        ConflictResolution::ManualReview { .. } => {
            panic!("rename_both must not produce ManualReview — that was the bug being fixed");
        }
        other => panic!("unexpected resolution: {other:?}"),
    }
}

#[test]
fn rename_both_preserves_file_extension() {
    let resolver = ConflictResolver {
        default_policy: ConflictPolicy::RenameBoth,
    };
    let op = PlannedOperation::Conflict {
        sync_id: SyncId::new(1),
        path: "archive/report.2024.csv".into(),
        kind: ConflictKind::LocalModifyVsRemoteModify,
    };

    let resolution = resolver.resolve(&op, None, None).expect("should resolve");

    match resolution {
        ConflictResolution::RenameBoth {
            local_renamed_path,
            remote_renamed_path,
            ..
        } => {
            assert!(
                local_renamed_path.ends_with(".csv"),
                "extension must be preserved in local rename: {local_renamed_path}"
            );
            assert!(
                remote_renamed_path.ends_with(".csv"),
                "extension must be preserved in remote rename: {remote_renamed_path}"
            );
        }
        other => panic!("expected RenameBoth, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// EngineShell integration: end-to-end ingest → dispatch
// ---------------------------------------------------------------------------

#[test]
fn engine_ingest_then_advance_dispatches_and_empties_queue() {
    let mut engine = EngineShell::new();
    let count = engine.ingest_candidates(&[
        SyncCandidate {
            sync_id: SyncId::new(1),
            source: ChangeSource::Local,
            path: "doc.txt".into(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: None,
            remote_folder_id: None,
        },
        SyncCandidate {
            sync_id: SyncId::new(1),
            source: ChangeSource::Remote,
            path: "img.png".into(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: Some(RemoteFileId::new(7)),
            remote_folder_id: None,
        },
    ]);

    assert_eq!(count.len(), 2, "two ops planned");
    // Queue still holds items before dispatch.
    assert_eq!(engine.scheduler.total_queued(), 2);

    let batch = engine.advance_transfer_cycle();
    assert_eq!(batch.len(), 2, "both ops dispatched");
    // Queue is now empty.
    assert_eq!(
        engine.scheduler.total_queued(),
        0,
        "queue empty after advance"
    );
}

#[test]
fn engine_fairness_across_two_roots() {
    let mut engine = EngineShell::new();

    // Ingest 3 items for root 10, 3 items for root 20.
    let _ = engine.ingest_candidates(&[
        SyncCandidate {
            sync_id: SyncId::new(10),
            source: ChangeSource::Remote,
            path: "r10/a.txt".into(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: Some(RemoteFileId::new(1)),
            remote_folder_id: None,
        },
        SyncCandidate {
            sync_id: SyncId::new(10),
            source: ChangeSource::Remote,
            path: "r10/b.txt".into(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: Some(RemoteFileId::new(2)),
            remote_folder_id: None,
        },
        SyncCandidate {
            sync_id: SyncId::new(20),
            source: ChangeSource::Remote,
            path: "r20/x.txt".into(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: Some(RemoteFileId::new(3)),
            remote_folder_id: None,
        },
        SyncCandidate {
            sync_id: SyncId::new(20),
            source: ChangeSource::Remote,
            path: "r20/y.txt".into(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: Some(RemoteFileId::new(4)),
            remote_folder_id: None,
        },
    ]);

    // Drain with limit=8 (default) — all 4 items come out in one cycle.
    let batch = engine.advance_transfer_cycle();
    assert_eq!(batch.len(), 4);

    let root10_count = batch
        .iter()
        .filter(|op| op.sync_id() == SyncId::new(10))
        .count();
    let root20_count = batch
        .iter()
        .filter(|op| op.sync_id() == SyncId::new(20))
        .count();

    assert!(root10_count >= 1, "root 10 should be scheduled");
    assert!(root20_count >= 1, "root 20 should be scheduled");
    assert_eq!(engine.scheduler.total_queued(), 0, "all items consumed");
}

#[test]
fn engine_public_state_lifecycle_and_filesystem_guards_are_complete() {
    for valid in ["file", "docs/report.txt", " spaced "] {
        assert!(pcloud_engine::is_valid_relative_path(valid), "{valid:?}");
    }
    for invalid in [
        "",
        " ",
        "/absolute",
        "./relative",
        r"windows\path",
        "a//b",
        "a/./b",
        "a/../b",
    ] {
        assert!(
            !pcloud_engine::is_valid_relative_path(invalid),
            "{invalid:?}"
        );
    }

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("a/b")).unwrap();
    std::fs::write(root.path().join("a/file"), b"x").unwrap();
    assert!(!pcloud_engine::probe_case_insensitive_fs(root.path()).unwrap());
    assert!(!pcloud_engine::warn_if_case_insensitive(root.path()));
    assert!(!pcloud_engine::warn_if_case_insensitive(
        root.path().join("missing").as_path()
    ));
    let mut visited = Vec::new();
    pcloud_engine::walk_local_tree(root.path(), 8, &mut |path, is_dir| {
        visited.push((path.to_path_buf(), is_dir));
    })
    .unwrap();
    assert!(visited.iter().any(|(_, is_dir)| *is_dir));
    assert!(visited.iter().any(|(_, is_dir)| !*is_dir));

    let mut engine = EngineShell::default();
    assert_eq!(engine, EngineShell::new());
    assert!(engine.summary().starts_with("engine("));
    assert_eq!(engine.wake_localscan(), 1);
    assert_ne!(engine, EngineShell::new());
    assert!(!engine.mark_transfer_completed("missing"));
    assert!(!engine.mark_transfer_failed("missing", "error"));
    assert!(
        engine
            .resolve_conflict_by_path("missing", "prefer_local")
            .is_err()
    );
    assert!(
        engine
            .resolve_conflict_by_sync_id_and_path(Some(SyncId::new(9)), "missing", "prefer_remote")
            .is_err()
    );

    let candidate = SyncCandidate {
        sync_id: SyncId::new(4),
        source: ChangeSource::Local,
        path: "queued.txt".into(),
        entry_kind: EntryKind::File,
        change_kind: ChangeKind::Upsert,
        remote_file_id: None,
        remote_folder_id: None,
    };
    engine.restore_planner_overflow(vec![candidate.clone()]);
    assert_eq!(engine.overflow_sync_root_ids(), vec![SyncId::new(4)]);
    assert_eq!(engine.drain_planner_overflow(), vec![candidate]);
    assert!(engine.drain_planner_overflow().is_empty());

    let op_b = PlannedOperation::DeleteLocal {
        sync_id: SyncId::new(2),
        path: "b".into(),
    };
    let op_a = PlannedOperation::DeleteLocal {
        sync_id: SyncId::new(1),
        path: "a".into(),
    };
    engine.restore_scheduler_queue(vec![op_b.clone(), op_a.clone()]);
    assert_eq!(
        engine.scheduler_sync_root_ids(),
        vec![SyncId::new(1), SyncId::new(2)]
    );
    assert_eq!(
        engine.snapshot_scheduler_queue(),
        vec![op_a.clone(), op_b.clone()]
    );
    assert_eq!(
        engine.snapshot_scheduler_durable(),
        vec![op_a.clone(), op_b.clone()]
    );
    engine.ack_dispatched_path(SyncId::new(1), "absent");
    assert_eq!(engine.drain_scheduler_queue(), vec![op_a, op_b]);
    assert!(engine.snapshot_scheduler_queue().is_empty());

    assert!(engine.pause_sync_root(SyncId::new(7)));
    assert!(!engine.pause_sync_root(SyncId::new(7)));
    assert!(engine.is_sync_root_paused(SyncId::new(7)));
    assert_eq!(engine.paused_sync_root_ids(), vec![SyncId::new(7)]);
    assert!(engine.resume_sync_root(SyncId::new(7)));
    assert!(!engine.resume_sync_root(SyncId::new(7)));
    engine.evict_sync_root(SyncId::new(7));
    engine.evict_sync_root(SyncId::new(7));
    assert_eq!(engine.drain_watcher_evictions(), vec![SyncId::new(7)]);
    assert!(engine.drain_watcher_evictions().is_empty());
}

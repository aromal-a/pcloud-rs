#![allow(clippy::pedantic)]
//! Criterion micro-benchmarks for `pcloud-engine` hot paths.
//!
//! Coverage requested by Reviewer 07:
//!
//! * `canonicalise_path` throughput — driven through the engine's
//!   [`local_scan::LocalScanner::normalize_entries`] entry, which is the
//!   canonicalising surface the engine exposes for inbound scan paths.
//! * `ReconcileWorker::tick` single-op latency — both the fast "not due"
//!   return and the due-scan return, which is what the daemon invokes on
//!   every supervision cadence.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

use pcloud_engine::local_scan::{LocalScanEntry, LocalScanner};
use pcloud_engine::reconcile_worker::{ReconcileTickOutcome, ReconcileWorker};
use pcloud_model::ids::SyncId;
use pcloud_model::sync::EntryKind;
use pcloud_resilience::clock::{Clock, ManualClock};

/// Synthesise a batch of scan entries with varied path shapes so the
/// canonicaliser sees realistic segment counts.
fn make_scan_batch(n: usize) -> Vec<LocalScanEntry> {
    (0..n)
        .map(|i| LocalScanEntry {
            sync_id: SyncId::new((i % 8 + 1) as u64),
            path: format!("docs/reports/2026/q{}/file-{:06}.csv", (i % 4) + 1, i),
            entry_kind: if i % 5 == 0 {
                EntryKind::Folder
            } else {
                EntryKind::File
            },
            deleted: i % 13 == 0,
            remote_parent_folder_id: None,
        })
        .collect()
}

fn bench_canonicalise_path(c: &mut Criterion) {
    let scanner = LocalScanner::default();
    let batch = make_scan_batch(1024);

    let mut group = c.benchmark_group("engine/canonicalise_path");
    group.throughput(Throughput::Elements(batch.len() as u64));
    group.bench_function("normalize_1024_entries", |b| {
        b.iter(|| {
            let out = scanner
                .normalize_entries(black_box(&batch))
                .expect("normalize ok");
            black_box(out.len());
        })
    });
    group.finish();
}

fn bench_reconcile_worker_tick(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine/reconcile_worker");

    // Fast-path: idle tick when no time has elapsed.
    group.bench_function("tick_idle", |b| {
        let clock = ManualClock::new();
        let arc: Arc<dyn Clock> = Arc::new(clock.clone());
        let mut w = ReconcileWorker::with_clock(Duration::from_secs(300), arc);
        w.track(SyncId::new(1));
        // Prime `last_scan_at` so the first tick fires once, then future
        // ticks with no time advance return Idle.
        let _ = w.tick();
        b.iter(|| {
            match w.tick() {
                ReconcileTickOutcome::Idle | ReconcileTickOutcome::NoSyncRoots => {}
                ReconcileTickOutcome::RunScan { .. } => {}
            };
            black_box(());
        });
    });

    // Due-path: advance time so every tick returns `RunScan`.
    group.bench_function("tick_due_run_scan", |b| {
        let clock = ManualClock::new();
        let arc: Arc<dyn Clock> = Arc::new(clock.clone());
        let mut w = ReconcileWorker::with_clock(Duration::from_millis(1), arc);
        for id in 1..=8u64 {
            w.track(SyncId::new(id));
        }
        b.iter(|| {
            clock.advance(Duration::from_millis(2));
            black_box(w.tick());
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_canonicalise_path,
    bench_reconcile_worker_tick
);
criterion_main!(benches);

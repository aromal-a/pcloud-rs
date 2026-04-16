#![allow(clippy::pedantic)]
//! Sync-root canonicalization throughput.
//!
//! Benchmarks `classify_folder_syncability` from
//! `pcloud-daemon::sync_backend` over a realistic tree of temp directories
//! with a warm existing-root list. Represents the hot path invoked by
//! `SyncRootAdd` before persistence.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use pcloud_daemon::mount_discovery::MountDiscovery;
use pcloud_daemon::sync_backend::{FolderSyncabilityOverrides, classify_folder_syncability};

fn build_tree(depth: usize) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "pcloud-bench-sync-{}-{}",
        std::process::id(),
        depth
    ));
    let _ = std::fs::remove_dir_all(&base);
    let mut path = base.clone();
    for i in 0..depth {
        path = path.join(format!("level-{i}-segment-with-moderately-long-name"));
    }
    std::fs::create_dir_all(&path).expect("mkdir tree");
    path
}

fn bench_classify(c: &mut Criterion) {
    let mut group = c.benchmark_group("sync_root_canonicalize");

    let shallow = build_tree(2);
    let deep = build_tree(8);
    let other1 = build_tree(3);
    let other2 = build_tree(4);
    let o1 = other1.to_string_lossy().into_owned();
    let o2 = other2.to_string_lossy().into_owned();
    let existing: Vec<&str> = vec![o1.as_str(), o2.as_str()];
    let discovery = MountDiscovery::default();
    let overrides = FolderSyncabilityOverrides::default();

    group.bench_function("classify_shallow_path", |b| {
        b.iter(|| {
            let out = classify_folder_syncability(
                black_box(&shallow),
                black_box(&existing),
                black_box(&discovery),
                black_box(&overrides),
            )
            .expect("classify");
            black_box(out);
        });
    });

    group.bench_function("classify_deep_path", |b| {
        b.iter(|| {
            let out = classify_folder_syncability(
                black_box(&deep),
                black_box(&existing),
                black_box(&discovery),
                black_box(&overrides),
            )
            .expect("classify");
            black_box(out);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_classify);
criterion_main!(benches);

#![allow(clippy::pedantic)]
//! Cold-start latency profiling (CLAUDEREV T3.7).
//!
//! Establishes a Criterion baseline for daemon bootstrap so a future CI
//! gate can flag ≥20% regression. Three measurements:
//!
//! - `cold_bootstrap`        — fresh `RuntimeShell` against an empty
//!   store path, end-to-end via `bootstrap_with_config`. The store
//!   directory is rotated per iteration so every sample pays the
//!   cold-cache cost (schema migrations, audit-chain init, vault
//!   provisioning).
//! - `bootstrap_to_first_request` — same fresh bootstrap, then dispatch
//!   a single cheap `Method::GetHealth` IPC and stop the timer once the
//!   response is in hand. This is the user-perceived "daemon is ready
//!   to answer" latency.
//! - `repeat_bootstrap_warm` — bootstrap against a store directory that
//!   has already been bootstrapped at least once. Confirms the warm
//!   path is faster and gives the comparison point for regression
//!   triage (cold vs warm delta should stay roughly stable).
//!
//! Bench is hermetic: `ConfigProfile::secure_defaults` with
//! `Environment::Development` produces a runtime that does not touch
//! the network. The store path lives under the per-process temp
//! directory and is removed between iterations for cold runs.
//!
//! Run with:
//!   cargo bench -p pcloud-daemon --bench cold_start \
//!     -- --save-baseline cold_start_v1
//!
//! Compare a later run with:
//!   cargo bench -p pcloud-daemon --bench cold_start \
//!     -- --baseline cold_start_v1
//!
//! Criterion will fail an iteration measurement that drifts beyond the
//! configured noise threshold; a future CI workflow wraps this with a
//! ≥20% regression gate.

// **PLATFORM:** all
// **GATING:** none (portable, no network I/O).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::{RuntimeShell, bootstrap_with_config, dispatch};
use pcloud_ipc::{Method, Request};

/// Allocate a unique-per-call store root. We use `/tmp` directly (not
/// `std::env::temp_dir()`) so the fully-qualified Unix-socket path the
/// runtime derives from this root stays under `SUN_LEN` on macOS, where
/// the per-user `/var/folders/.../T/` prefix already eats ~49 chars.
/// This mirrors the convention used by the in-tree `bootstrap_test_shell`
/// helper in `crates/pcloud-daemon/src/lib.rs`.
fn unique_root(tag: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    PathBuf::from("/tmp").join(format!(
        "pd-bench-cold-{tag}-{}-{nonce}",
        std::process::id(),
    ))
}

fn fresh_config(tag: &str) -> (ConfigProfile, PathBuf) {
    let root = unique_root(tag);
    let config = ConfigProfile::secure_defaults(root.clone(), Environment::Development);
    (config, root)
}

/// Wipe the store directory after a sample so the next iteration pays
/// the cold-cache cost. Best-effort: any failure to remove is ignored
/// because the path may not yet exist (e.g. bootstrap failed mid-way).
fn remove_root(root: &PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

/// `cold_bootstrap`: time `bootstrap_with_config` from a guaranteed-
/// empty store path. Each iteration rotates the path and removes any
/// state left behind, so the schema-migration and audit-chain-init
/// cost is paid every sample.
fn bench_cold_bootstrap(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_start");
    // Bootstrap is dominated by SQLite open + migrations + vault
    // provisioning — typically tens to hundreds of milliseconds. 10
    // samples keep total wall-clock under the 60s budget while still
    // giving Criterion enough data points for a stable mean estimate.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    group.bench_function("cold_bootstrap", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let (config, root) = fresh_config("boot");
                let start = Instant::now();
                let runtime = bootstrap_with_config(black_box(config))
                    .expect("runtime bootstrap should succeed");
                total += start.elapsed();
                drop(runtime);
                remove_root(&root);
            }
            total
        });
    });

    group.finish();
}

/// `bootstrap_to_first_request`: time bootstrap *and* a first
/// `Method::GetHealth` round-trip. `GetHealth` is the cheapest IPC
/// surface that does not require an authenticated session, so it
/// approximates "user sent the first command after the daemon
/// started". Network is never touched.
fn bench_bootstrap_to_first_request(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_start");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    group.bench_function("bootstrap_to_first_request", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let (config, root) = fresh_config("first-rpc");
                let start = Instant::now();
                let mut runtime: RuntimeShell = bootstrap_with_config(black_box(config))
                    .expect("runtime bootstrap should succeed");
                let response = dispatch(
                    &mut runtime,
                    Request::Plain {
                        method: Method::GetHealth,
                    },
                );
                let elapsed = start.elapsed();
                // Force the response onto the read side so the
                // optimiser cannot elide the dispatch call.
                black_box(&response);
                total += elapsed;
                drop(runtime);
                remove_root(&root);
            }
            total
        });
    });

    group.finish();
}

/// `repeat_bootstrap_warm`: bootstrap, drop the runtime, then
/// re-bootstrap the *same* store root and time only the second open.
/// SQLite WAL pages, schema-version reads, and any cached file-system
/// metadata stay warm in the page cache, so the second open should
/// land below the cold path. The delta gives reviewers a comparison
/// point — if `cold_bootstrap` regresses but `repeat_bootstrap_warm`
/// holds, the regression is in cold-path init (migrations, vault
/// provisioning); if both regress equally, it's in steady-state
/// startup (config parsing, runtime composition).
fn bench_repeat_bootstrap_warm(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_start");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    group.bench_function("repeat_bootstrap_warm", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                // Pre-warm: pay the cold cost once on this root, then
                // immediately drop and re-open.
                let (config_cold, root) = fresh_config("warm");
                let cold = bootstrap_with_config(config_cold)
                    .expect("cold pre-warm bootstrap should succeed");
                drop(cold);

                let config_warm =
                    ConfigProfile::secure_defaults(root.clone(), Environment::Development);
                let start = Instant::now();
                let runtime = bootstrap_with_config(black_box(config_warm))
                    .expect("warm bootstrap should succeed");
                total += start.elapsed();
                drop(runtime);
                remove_root(&root);
            }
            total
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_cold_bootstrap,
    bench_bootstrap_to_first_request,
    bench_repeat_bootstrap_warm,
);
criterion_main!(benches);

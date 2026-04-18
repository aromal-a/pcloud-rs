// End-to-end dispatch benchmark.
//
// Measures the round-trip latency of common `dispatch()` paths without any
// real network I/O. The test shell bootstrapped here is the same lightweight
// in-process shell used by the `lib.rs` unit tests.

use std::time::{SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use pcloud_config::{ConfigProfile, Environment};
use pcloud_ipc::{Method, Request};

fn make_shell() -> pcloud_daemon::RuntimeShell {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "pd-bench-{}-{}",
        std::process::id(),
        nonce % 1_000_000_000
    ));
    let config = ConfigProfile::secure_defaults(root, Environment::Development);
    pcloud_daemon::bootstrap_with_config(config).expect("runtime bootstrap should succeed")
}

fn bench_dispatch_get_status(c: &mut Criterion) {
    let mut runtime = make_shell();
    c.bench_function("dispatch_get_status", |b| {
        b.iter(|| {
            pcloud_daemon::dispatch(
                &mut runtime,
                Request::Plain {
                    method: Method::GetStatus,
                },
            )
        });
    });
}

fn bench_dispatch_get_health(c: &mut Criterion) {
    let mut runtime = make_shell();
    c.bench_function("dispatch_get_health", |b| {
        b.iter(|| {
            pcloud_daemon::dispatch(
                &mut runtime,
                Request::Plain {
                    method: Method::GetHealth,
                },
            )
        });
    });
}

fn bench_dispatch_get_pending(c: &mut Criterion) {
    let mut runtime = make_shell();
    c.bench_function("dispatch_get_pending", |b| {
        b.iter(|| {
            pcloud_daemon::dispatch(
                &mut runtime,
                Request::Plain {
                    method: Method::GetPending,
                },
            )
        });
    });
}

criterion_group!(
    benches,
    bench_dispatch_get_status,
    bench_dispatch_get_health,
    bench_dispatch_get_pending,
);
criterion_main!(benches);

#![allow(clippy::pedantic)]
//! Criterion micro-benchmarks for the chunked-upload flush path landed in
//! G2 (write-path state machine) + G8 (chunked flush).
//!
//! The bench drives the full `upload_create` / `upload_write*` /
//! `upload_save` loop against a no-op [`FileUploadBackend`] implementation,
//! seeded with a 100 MiB staging buffer. Three chunk sizes are measured —
//! 1 MiB, 4 MiB, and 16 MiB — so the throughput curve (MB/s) shows the
//! per-chunk overhead of the chunked surface independent of any wire I/O.
//!
//! The backend is deliberately no-op (no disk writes, no network) so the
//! measured cost is the state-machine and trait-dispatch overhead only.
//! A baseline regression catch is the point — real wire throughput is
//! bounded by the transport, not by this crate.
//!
//! TODO(P7): Wire baseline capture into the `bench-nightly` CI job —
//! Fixer 09 territory. Criterion already emits a machine-readable JSON
//! summary under `target/criterion/`; the CI job should diff the reported
//! throughput against the previous green run and fail on a configured
//! regression tolerance.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::atomic::{AtomicUsize, Ordering};

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

use pcloud_fs::write_path::{FileUploadBackend, WritePathError};

const MIB: usize = 1024 * 1024;
const PAYLOAD_BYTES: usize = 100 * MIB;
const CHUNK_SIZES: &[usize] = &[MIB, 4 * MIB, 16 * MIB];

/// No-op upload backend. Counts calls so the optimiser cannot elide the
/// loop, but performs no I/O and holds no state worth benchmarking.
#[derive(Default)]
struct NoopBackend {
    creates: AtomicUsize,
    writes: AtomicUsize,
    saves: AtomicUsize,
}

impl FileUploadBackend for NoopBackend {
    fn upload_file(
        &self,
        _parent_path: &str,
        _name: &str,
        _staging_file: &std::path::Path,
    ) -> Result<(), WritePathError> {
        Ok(())
    }

    fn unlink_remote(&self, _path: &str) -> Result<(), WritePathError> {
        Ok(())
    }

    fn rename_remote(&self, _from: &str, _to: &str) -> Result<(), WritePathError> {
        Ok(())
    }

    fn upload_create(&self, _parent_path: &str, _name: &str) -> Result<u64, WritePathError> {
        let id = self.creates.fetch_add(1, Ordering::Relaxed) as u64 + 1;
        Ok(id)
    }

    fn upload_write(
        &self,
        _upload_id: u64,
        _offset: u64,
        chunk: &[u8],
    ) -> Result<(), WritePathError> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        // Touch the slice so the compiler cannot hoist the read away.
        black_box(chunk.first().copied());
        Ok(())
    }

    fn upload_save(
        &self,
        _upload_id: u64,
        _parent_path: &str,
        _name: &str,
        _total_size: u64,
    ) -> Result<(), WritePathError> {
        self.saves.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Drive the chunked-flush shape: one `upload_create`, `ceil(total /
/// chunk_size)` `upload_write`s, one `upload_save`. The staging buffer is
/// allocated once and reused across iterations so we measure the
/// per-chunk dispatch cost, not allocator noise.
fn run_chunked_flush(backend: &NoopBackend, payload: &[u8], chunk_size: usize) {
    let upload_id = backend
        .upload_create("/bench", "blob.bin")
        .expect("noop create");
    let mut offset: u64 = 0;
    for chunk in payload.chunks(chunk_size) {
        backend
            .upload_write(upload_id, offset, chunk)
            .expect("noop write");
        offset += chunk.len() as u64;
    }
    backend
        .upload_save(upload_id, "/bench", "blob.bin", payload.len() as u64)
        .expect("noop save");
}

fn bench_chunked_flush(c: &mut Criterion) {
    // Seed once — 100 MiB of zeroes is enough to dominate per-iteration
    // setup and still lets the no-op backend loop fit comfortably.
    let payload = vec![0u8; PAYLOAD_BYTES];
    let backend = NoopBackend::default();

    let mut group = c.benchmark_group("chunked_flush/100mib");
    group.throughput(Throughput::Bytes(PAYLOAD_BYTES as u64));
    // 100 MiB per iteration is large; keep sample count modest so the
    // total wall-clock stays reasonable on CI.
    group.sample_size(10);

    for &chunk_size in CHUNK_SIZES {
        let label = format!("{}mib_chunks", chunk_size / MIB);
        group.bench_with_input(
            BenchmarkId::from_parameter(&label),
            &chunk_size,
            |b, &cs| {
                b.iter(|| {
                    run_chunked_flush(&backend, black_box(&payload), cs);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_chunked_flush);
criterion_main!(benches);

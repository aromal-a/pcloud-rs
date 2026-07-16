#![allow(clippy::pedantic)]
//! Criterion micro-benchmark for the [`UploadSession`] state machine
//! landed in G2, routed through the SDK surface re-exported from
//! `pcloud-embedded-sdk`.
//!
//! The bench walks 100 × 4 MiB chunks through
//! `start` → `write_chunk` ×N → `save_and_complete` with a no-op
//! [`UploadSessionDriver`]. No wire I/O, no disk I/O, no journal — we
//! measure the state-machine and `Mutex`/`watch` dispatch cost only.
//! Real throughput is transport-bound and is tracked separately under
//! the live-verification work in `bd-1du.10`.
//!
//! Total payload per iteration: 100 × 4 MiB = 400 MiB. Throughput is
//! reported by Criterion via `Throughput::Bytes` so the output shows
//! MB/s directly.
//!
//! TODO(P7): Wire baseline capture into the `bench-nightly` CI job —
//! Fixer 09 territory. The CI job should persist the Criterion JSON
//! summary under `target/criterion/` and gate PRs on regression
//! thresholds.

// **PLATFORM:** all
// **GATING:** none (portable).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

use pcloud_embedded_sdk::{
    FileMetadata, UploadError, UploadHandle, UploadSession, UploadSessionDriver,
};

const MIB: usize = 1024 * 1024;
const CHUNK_SIZE: usize = 4 * MIB;
const CHUNK_COUNT: u64 = 100;
const TOTAL_BYTES: u64 = CHUNK_SIZE as u64 * CHUNK_COUNT;

/// No-op driver: acks every wire call immediately without touching the
/// network or disk. `upload_id` is hard-coded; `write_chunk` returns the
/// post-write offset so the state machine's monotonic-offset invariant
/// is upheld.
struct NoopDriver {
    saved: u64,
}

impl NoopDriver {
    fn new() -> Self {
        Self { saved: 0 }
    }
}

impl UploadSessionDriver for NoopDriver {
    fn create(
        &mut self,
        folder_id: u64,
        file_name: &str,
        _total: u64,
    ) -> Result<UploadHandle, UploadError> {
        Ok(UploadHandle {
            upload_id: 0xBEEF,
            parent_folder_id: folder_id,
            file_name: file_name.to_owned(),
        })
    }

    fn write_chunk(
        &mut self,
        _handle: &UploadHandle,
        offset: u64,
        buf: &[u8],
    ) -> Result<u64, UploadError> {
        // Prevent the compiler from eliding the slice.
        black_box(buf.first().copied());
        Ok(offset + buf.len() as u64)
    }

    fn save(&mut self, handle: &UploadHandle) -> Result<FileMetadata, UploadError> {
        self.saved += 1;
        Ok(FileMetadata {
            file_id: Some(1),
            parent_folder_id: handle.parent_folder_id,
            name: handle.file_name.clone(),
            bytes_uploaded: TOTAL_BYTES,
            conflicted: false,
            server_hash: None,
        })
    }

    fn delete(&mut self, _handle: &UploadHandle) -> Result<(), UploadError> {
        Ok(())
    }
}

fn drive_one_session(chunk: &[u8]) {
    let mut driver = NoopDriver::new();
    let session = UploadSession::start(42, "bench.bin", TOTAL_BYTES, &mut driver, None)
        .expect("start session");
    for _ in 0..CHUNK_COUNT {
        session
            .write_chunk(&mut driver, chunk)
            .expect("write_chunk");
    }
    let meta = session
        .save_and_complete(&mut driver, None)
        .expect("save_and_complete");
    black_box(meta);
}

fn bench_upload_session(c: &mut Criterion) {
    // Allocate the 4 MiB chunk buffer once and reuse it across iterations.
    let chunk = vec![0u8; CHUNK_SIZE];

    let mut group = c.benchmark_group("upload_session/state_machine");
    group.throughput(Throughput::Bytes(TOTAL_BYTES));
    group.sample_size(10);
    group.bench_function("100x_4mib_chunks_noop_driver", |b| {
        b.iter(|| drive_one_session(black_box(&chunk)));
    });
    group.finish();
}

criterion_group!(benches, bench_upload_session);
criterion_main!(benches);

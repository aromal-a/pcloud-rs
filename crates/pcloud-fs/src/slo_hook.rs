//! Process-wide SLO observation hook for the FUSE read path and the
//! integrity-sweeper scheduler loop.
//!
//! The FUSE adapter in `platform/linux.rs` is constructed by the `fuser`
//! crate, not by the daemon directly, so there is no natural place to
//! thread an `Arc<Slo>` through `fn read`. This module exposes a
//! [`set_slo_registry`] entry point the daemon calls once at boot to
//! install the shared [`pcloud_observability::slo::Slo`] registry, plus
//! zero-cost [`observe_mount_read`] / [`observe_integrity_sweeper_run`]
//! / [`observe_audit_chain_verify`] helpers that the relevant hot paths
//! call on completion.
//!
//! The same registry backs `Method::GetSlo` and the `/slo` HTTP endpoint
//! on the daemon.
//!
//! When no registry is installed (unit tests, early boot, non-daemon
//! consumers) `observe_mount_read` is a no-op — there is no dependency on
//! the daemon being fully bootstrapped before the FUSE adapter handles
//! its first read.
//!
//! # Why not inject through the adapter?
//!
//! The `fuser::Filesystem` trait is consumed by value by the background
//! session, and the adapter is boxed through [`crate::fuse_adapter`]
//! which already has a fixed interface used by tests. A process-wide
//! `OnceLock<Arc<Slo>>` keeps the wiring non-invasive: the adapter is
//! unchanged, the daemon owns registration, and a single read path
//! branch (a nullary load of an `AtomicPtr`) is the only hot-path cost.
//!
//! # Platform
//!
//! **PLATFORM:** all (the hook compiles on every OS). The hot call site
//! is Linux-only today because only `platform/linux.rs` contains a real
//! FUSE `read` shim.
//!
//! **GATING:** none.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use pcloud_observability::metrics::{HistogramHandle, register_histogram};
use pcloud_observability::slo::Slo;

/// Default histogram buckets (in seconds) for `flush_latency_seconds`,
/// emitted on every successful FUSE flush by [`observe_flush_latency`].
/// Buckets span 50 ms .. 60 s to cover both small synchronous writes and
/// multi-GiB staged flushes.
const FLUSH_LATENCY_BUCKETS: &[f64] = &[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0];

/// Default histogram buckets (in bytes) for `flush_bytes`, the
/// per-flush payload size distribution emitted by [`observe_flush_bytes`].
/// Ranges from 4 KiB up to 1 GiB to capture both small overwrites and
/// large chunked flushes.
const FLUSH_BYTES_BUCKETS: &[f64] = &[
    4_096.0,
    65_536.0,
    262_144.0,
    1_048_576.0,
    4_194_304.0,
    16_777_216.0,
    67_108_864.0,
    268_435_456.0,
    1_073_741_824.0,
];

/// Lazily-registered handle to the `flush_latency_seconds` histogram.
/// Registration is idempotent (the observability crate deduplicates by
/// name), so the `OnceLock` is a local fast-path to avoid re-hashing the
/// registry mutex on every flush.
fn flush_latency_histogram() -> &'static HistogramHandle {
    static HISTO: OnceLock<HistogramHandle> = OnceLock::new();
    HISTO.get_or_init(|| register_histogram("flush_latency_seconds", FLUSH_LATENCY_BUCKETS))
}

/// Lazily-registered handle to the `flush_bytes` histogram.
fn flush_bytes_histogram() -> &'static HistogramHandle {
    static HISTO: OnceLock<HistogramHandle> = OnceLock::new();
    HISTO.get_or_init(|| register_histogram("flush_bytes", FLUSH_BYTES_BUCKETS))
}

/// Process-wide SLO registry set by the daemon at bootstrap.
///
/// The hook is a `OnceLock` so concurrent daemon launches cannot
/// overwrite an already-installed registry. The daemon uses the same
/// `Arc<Slo>` held on its `ObservabilityShell`, so every caller
/// (`Method::GetSlo`, `/slo`, this FUSE hook) observes the same counters.
static SLO_REGISTRY: OnceLock<Arc<Slo>> = OnceLock::new();

/// Install the shared SLO registry. Safe to call from a single
/// bootstrap path; subsequent calls are ignored (the first registration
/// wins, mirroring `OnceLock` semantics). Returns `true` when the
/// registration was accepted, `false` otherwise.
pub fn set_slo_registry(slo: Arc<Slo>) -> bool {
    SLO_REGISTRY.set(slo).is_ok()
}

/// Observe a single `mount.read.latency.p99` sample. No-op when no
/// registry is installed. Intended to be called from the FUSE `read`
/// shim around the call into [`crate::fuse_adapter::FuseAdapter::read`].
pub fn observe_mount_read(elapsed: Duration) {
    if let Some(slo) = SLO_REGISTRY.get() {
        slo.observe_mount_read_latency(elapsed.as_secs_f64());
    }
}

/// Observe a single `integrity_sweeper.run.p95` sample. No-op when no
/// registry is installed. Called from the scheduler loop at the end of
/// a `run_once` invocation; the caller decides whether to record
/// scheduler ticks only (SLI spec) or also ad-hoc operator-driven runs.
pub fn observe_integrity_sweeper_run(elapsed: Duration) {
    if let Some(slo) = SLO_REGISTRY.get() {
        slo.observe_integrity_sweeper_run(elapsed.as_secs_f64());
    }
}

/// Observe a single `audit.hash_chain.verify.daily_pass_rate` sample.
/// `pass = true` means the chain verified cleanly. No-op when no
/// registry is installed.
pub fn observe_audit_chain_verify(pass: bool) {
    if let Some(slo) = SLO_REGISTRY.get() {
        slo.observe_audit_verify(pass);
    }
}

/// Observe a successful FUSE flush of `bytes` payload that completed
/// in `elapsed` wall-clock. Feeds:
///
/// - the `flush_latency_seconds` user histogram (Prometheus `/metrics`),
/// - the `flush_bytes` user histogram (per-flush payload size),
///
/// and — when the process-wide SLO registry is installed — the upload
/// throughput SLI as MB/s so that the daemon's `/slo` endpoint reflects
/// sustained write-path performance without requiring a separate upload
/// path observation.
///
/// Called by the `WritePathService` `flush` and chunked-flush paths on
/// the success arm. Safe to call on the hot path: both handles are
/// `Arc`-backed and
/// lock-free, and the SLO registry lookup is a single atomic load.
pub fn observe_flush(bytes: u64, elapsed: Duration) {
    let secs = elapsed.as_secs_f64();
    flush_latency_histogram().observe(secs);
    // Guard against division blow-up on a zero-byte flush (allowed — a
    // crash-safe `upload_save` with no bytes is legal), and on very fast
    // sub-millisecond flushes where `secs` underflows to zero.
    #[allow(clippy::cast_precision_loss)]
    let bytes_f = bytes as f64;
    flush_bytes_histogram().observe(bytes_f);
    if let Some(slo) = SLO_REGISTRY.get() {
        if bytes > 0 {
            if secs > 0.0 {
                let mbps = (bytes_f / 1_000_000.0) / secs;
                slo.observe_upload_throughput_mbps(mbps);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_without_registry_is_noop() {
        // No installation performed; must not panic and must not mutate
        // any global state (nothing to assert beyond the "does not
        // panic" contract since the registry is process-global).
        observe_mount_read(Duration::from_millis(1));
    }
}

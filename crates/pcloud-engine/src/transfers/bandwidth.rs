// **PLATFORM:** all
// **GATING:** none (portable).

//! Token-bucket bandwidth limiter.
//!
//! # Design
//!
//! Thin strongly-typed wrapper around
//! [`pcloud_resilience::BandwidthPacer`] — the workspace's canonical
//! token-bucket implementation (see `crates/pcloud-resilience/src/
//! pacing.rs`).  This module exists so callers inside the engine can
//! configure, serialise, and thread a limiter through
//! `SyncLoopConfig` / `RealSyncLoopRuntime` without naming the
//! resilience crate directly, and so the limiter is a value type (not
//! an `Arc`) that fits naturally in config serde.
//!
//! # Wiring (bead pcloud-rs-6mx)
//!
//! The engine-side stub is now connected:
//!
//! - `limit_bytes_per_sec = None`  → unlimited (zero overhead).
//! - `limit_bytes_per_sec = Some(n)` → token-bucket with refill rate
//!   `n` bytes/second and burst size `n`.
//!
//! Callers invoke [`BandwidthLimiter::acquire_blocking`] before each
//! transfer I/O unit; the call blocks just long enough for the bucket
//! to refill the requested budget.  For async callers,
//! [`BandwidthLimiter::acquire`] returns the required
//! [`std::time::Duration`] without sleeping so the caller can hand it
//! to an async runtime.
//!
//! Integration points (also wired by bead pcloud-rs-6mx):
//!
//! - `pcloud-proto::http_download`: `HttpDownloadConfig::bandwidth_pacer`
//!   controls the download byte loop.
//! - `pcloud-backends::transfer_backend::TransferRuntime`: the
//!   `with_bandwidth_pacer` / `set_bandwidth_pacer` setters install a
//!   shared [`BandwidthPacer`](pcloud_resilience::BandwidthPacer) for
//!   both upload and download loops.

use std::sync::Arc;
use std::time::Duration;

use pcloud_resilience::BandwidthPacer;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Token-bucket bandwidth limiter.
///
/// When `limit_bytes_per_sec` is `None` (the default), all acquire
/// calls return immediately with [`Duration::ZERO`]. When set, the
/// bucket refills at that rate and callers block (via
/// [`Self::acquire_blocking`]) or receive a sleep duration (via
/// [`Self::acquire`]) until tokens are available.
///
/// # Example
///
/// ```
/// use pcloud_engine::transfers::bandwidth::BandwidthLimiter;
///
/// // Default: unlimited — acquire always succeeds instantly.
/// let limiter = BandwidthLimiter::default();
/// limiter.acquire_blocking(1024); // no blocking
/// assert!(limiter.is_unlimited());
/// ```
#[derive(Debug, Clone)]
pub struct BandwidthLimiter {
    /// Maximum bytes per second allowed, or `None` for unlimited.
    /// Default: `None` (no throttling).
    pub limit_bytes_per_sec: Option<u64>,

    /// Shared token bucket. Rebuilt from `limit_bytes_per_sec` on
    /// deserialisation so round-tripping preserves limiter semantics
    /// without leaking bucket state between processes.
    pacer: Arc<BandwidthPacer>,
}

/// Wire format for serde round-trips: serialise only the configured
/// limit and reconstruct the token bucket on deserialisation.
#[derive(Serialize, Deserialize)]
struct BandwidthLimiterWire {
    limit_bytes_per_sec: Option<u64>,
}

impl Serialize for BandwidthLimiter {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        BandwidthLimiterWire {
            limit_bytes_per_sec: self.limit_bytes_per_sec,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for BandwidthLimiter {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = BandwidthLimiterWire::deserialize(d)?;
        Ok(Self::new(wire.limit_bytes_per_sec))
    }
}

impl Default for BandwidthLimiter {
    fn default() -> Self {
        Self::new(None)
    }
}

impl PartialEq for BandwidthLimiter {
    // Equality is defined on the configured limit: two limiters with
    // the same `limit_bytes_per_sec` are considered equal even if
    // their token buckets have drifted. Necessary because `Arc` does
    // not participate in structural equality and we want serde
    // round-trips to be stable.
    fn eq(&self, other: &Self) -> bool {
        self.limit_bytes_per_sec == other.limit_bytes_per_sec
    }
}

impl Eq for BandwidthLimiter {}

impl BandwidthLimiter {
    /// Create a limiter with the given cap. Pass `None` for unlimited.
    #[must_use]
    pub fn new(limit_bytes_per_sec: Option<u64>) -> Self {
        Self {
            limit_bytes_per_sec,
            pacer: Arc::new(BandwidthPacer::new(limit_bytes_per_sec)),
        }
    }

    /// Reserve budget for `bytes` and return the required sleep
    /// duration without actually sleeping. Returns [`Duration::ZERO`]
    /// when the budget is immediately available (or the limiter is
    /// unlimited). Intended for async callers that hand the returned
    /// duration to an async sleep.
    ///
    /// Bead: pcloud-rs-6mx.
    pub fn acquire(&self, bytes: usize) -> Duration {
        self.pacer.acquire(bytes as u64)
    }

    /// Reserve budget for `bytes` and, if a wait is required, block
    /// the current thread until the budget is available.
    ///
    /// Synchronous counterpart of [`Self::acquire`]. Use this inside
    /// synchronous byte loops (HTTP `read()` / `write()`).
    ///
    /// Bead: pcloud-rs-6mx.
    pub fn acquire_blocking(&self, bytes: usize) {
        self.pacer.acquire_blocking(bytes as u64);
    }

    /// Returns `true` if this limiter is unlimited.
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.limit_bytes_per_sec.is_none()
    }

    /// Return a clone of the underlying [`BandwidthPacer`] so callers
    /// can wire the same instance into
    /// `TransferRuntime::with_bandwidth_pacer` and
    /// `HttpDownloadConfig::bandwidth_pacer`. Sharing the pacer is how
    /// a single global cap is enforced across concurrent
    /// upload/download streams.
    #[must_use]
    pub fn pacer(&self) -> Arc<BandwidthPacer> {
        self.pacer.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::BandwidthLimiter;
    use std::time::{Duration, Instant};

    #[test]
    fn default_limiter_is_unlimited() {
        let limiter = BandwidthLimiter::default();
        assert!(limiter.is_unlimited());
        // Must not block or panic, even for huge requests.
        limiter.acquire_blocking(1024 * 1024);
        assert_eq!(limiter.acquire(1024 * 1024), Duration::ZERO);
    }

    #[test]
    fn limited_limiter_is_not_unlimited() {
        let limiter = BandwidthLimiter::new(Some(1_000_000));
        assert!(!limiter.is_unlimited());
        limiter.acquire_blocking(512);
    }

    #[test]
    fn bandwidth_limiter_none_is_unlimited() {
        // Bead pcloud-rs-6mx acceptance test.
        let limiter = BandwidthLimiter::new(None);
        assert_eq!(limiter.acquire(0), Duration::ZERO);
        assert_eq!(limiter.acquire(usize::MAX / 4), Duration::ZERO);

        let start = Instant::now();
        limiter.acquire_blocking(1_000_000_000);
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn bandwidth_limiter_throttles_to_configured_rate() {
        // Bead pcloud-rs-6mx acceptance test. Uses mock-time semantics
        // (inspects the returned Duration without sleeping).
        let limit: u64 = 100 * 1024; // 100 KB/s
        let limiter = BandwidthLimiter::new(Some(limit));

        // Drain the initial burst.
        assert_eq!(limiter.acquire(limit as usize), Duration::ZERO);

        // Next 1 MiB request must require ≈ 1 MiB / 100 KiB/s ≈ 10.24 s.
        let request: usize = 1024 * 1024;
        let wait = limiter.acquire(request);
        let expected = request as f64 / limit as f64;
        let observed = wait.as_secs_f64();
        assert!(
            observed >= expected * 0.9 && observed <= expected * 1.1,
            "observed {observed:.3}s not within ±10% of expected {expected:.3}s"
        );
    }

    #[test]
    fn serde_roundtrip_unlimited() {
        let limiter = BandwidthLimiter::default();
        let json = serde_json::to_string(&limiter).unwrap();
        let back: BandwidthLimiter = serde_json::from_str(&json).unwrap();
        assert_eq!(limiter, back);
        assert!(back.is_unlimited());
    }

    #[test]
    fn serde_roundtrip_limited() {
        let limiter = BandwidthLimiter::new(Some(5_000_000));
        let json = serde_json::to_string(&limiter).unwrap();
        let back: BandwidthLimiter = serde_json::from_str(&json).unwrap();
        assert_eq!(limiter, back);
        assert!(!back.is_unlimited());
        // Re-hydrated pacer must enforce the same limit.
        assert_eq!(back.pacer().limit(), Some(5_000_000));
    }
}

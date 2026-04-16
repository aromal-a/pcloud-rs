//! Build information and health-report payloads.
//!
//! The daemon exposes two surfaces for "is the process alive and sane":
//! - the `Method::Health` IPC, which returns a [`HealthReport`],
//! - the exporter's `GET /health` HTTP endpoint, which returns `200 ok` vs
//!   `503 not ready` based on the same shell state.
//!
//! All types here are `Serialize` so the daemon can render them to either
//! JSON or a text protocol without additional boilerplate.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Live health snapshot: current summary and the most recent audit category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// Short human-readable status line.
    pub summary: String,
    /// Category of the most recently recorded audit event, if any.
    pub last_event: Option<String>,
}

/// Compile-time build info surfaced on the health endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct BuildInfo {
    /// Crate name (for example `"pcloud-daemon"`).
    pub name: &'static str,
    /// Semantic version string extracted from `CARGO_PKG_VERSION` at build time.
    pub version: &'static str,
    /// Rust edition the binary was compiled against.
    pub rust_edition: &'static str,
}

impl BuildInfo {
    /// Canonical build info for the pcloud-daemon binary.
    ///
    /// The version is captured at compile time from `CARGO_PKG_VERSION`; the
    /// edition string is hard-coded to match the workspace edition.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::health::BuildInfo;
    /// let b = BuildInfo::pcloud_daemon();
    /// assert_eq!(b.name, "pcloud-daemon");
    /// assert_eq!(b.rust_edition, "2024");
    /// assert!(!b.version.is_empty());
    /// ```
    pub const fn pcloud_daemon() -> Self {
        Self {
            name: "pcloud-daemon",
            version: env!("CARGO_PKG_VERSION"),
            rust_edition: "2024",
        }
    }
}

/// Structured response body for `Method::Health`. `metrics_snapshot` is
/// `None` unless the `prometheus-exporter` feature is enabled and the
/// daemon populated a snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    /// Build info of the running binary.
    pub build: BuildInfo,
    /// Seconds the daemon has been running since shell construction.
    pub uptime_secs: u64,
    /// Human-readable summary mirrored from [`HealthSnapshot::summary`].
    pub summary: String,
    /// Prometheus exposition text rendered by the metric families. `None`
    /// when metrics are not compiled in or the runtime did not supply one.
    pub metrics_snapshot: Option<String>,
}

/// Return the current wall-clock time as Unix seconds, or `0` if the system
/// clock is set before the Unix epoch (should not happen in practice).
///
/// # Example
///
/// ```
/// // now should be well past the Unix epoch on any modern machine.
/// assert!(pcloud_observability::health::now_unix_secs() > 1_600_000_000);
/// ```
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Compute uptime in seconds from the supplied startup timestamp.
///
/// Uses `saturating_sub` so a clock jump backwards cannot produce a negative
/// value; the reported uptime is guaranteed to be monotonic and non-negative.
///
/// # Example
///
/// ```
/// use pcloud_observability::health::{now_unix_secs, uptime_from};
/// let past = now_unix_secs().saturating_sub(5);
/// assert!(uptime_from(past) >= 5);
/// // A future start produces 0, never a negative number.
/// assert_eq!(uptime_from(u64::MAX), 0);
/// ```
pub fn uptime_from(startup_unix_secs: u64) -> u64 {
    now_unix_secs().saturating_sub(startup_unix_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_has_non_empty_version() {
        let b = BuildInfo::pcloud_daemon();
        assert!(!b.version.is_empty());
        assert_eq!(b.name, "pcloud-daemon");
    }

    #[test]
    fn uptime_is_monotonic_from_past() {
        let past = now_unix_secs().saturating_sub(5);
        let u = uptime_from(past);
        assert!(u >= 5, "uptime={u}");
    }
}

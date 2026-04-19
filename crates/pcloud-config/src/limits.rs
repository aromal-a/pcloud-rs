//! Resource-consumption bounds attached to a [`crate::ConfigProfile`].
//!
//! These are *safety* bounds, not performance knobs — they exist to cap
//! memory usage under adversarial input (large frames, hung uploads) and
//! to prevent the daemon from saturating the uplink with unbounded
//! concurrent transfers.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Upper bounds on concurrent work and per-frame parser allocation.
///
/// Persists in the envelope's `profile.limits` object. The three
/// core fields (`max_concurrent_uploads`, `max_concurrent_downloads`,
/// `max_parser_frame_bytes`) are required by the schema; the two
/// IPC-cap fields are optional and `#[serde(default)]` to the
/// compile-time defaults exported from `pcloud-ipc` (ncx.59).
/// [`crate::ConfigProfile::secure_defaults`] uses:
///
/// - `max_concurrent_uploads = 4`
/// - `max_concurrent_downloads = 4`
/// - `max_parser_frame_bytes = 8 MiB` (`8 * 1024 * 1024`)
/// - `max_ipc_connections = 128`
/// - `max_ipc_connections_per_peer = 32`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum upload tasks allowed to run concurrently. Default: `4`.
    /// Valid values: any `usize`; `0` effectively halts uploads.
    /// **Security:** caps uplink saturation and memory footprint per
    /// client. Example: `max_concurrent_uploads = 4`.
    pub max_concurrent_uploads: usize,
    /// Maximum download tasks allowed to run concurrently. Default: `4`.
    /// Valid values: any `usize`; `0` effectively halts downloads.
    /// **Security:** same role as the upload cap — protects the host
    /// against unbounded fan-out. Example:
    /// `max_concurrent_downloads = 4`.
    pub max_concurrent_downloads: usize,
    /// Hard cap on a single parsed wire frame, in bytes. Default:
    /// `8 * 1024 * 1024` (8 MiB). Valid values: any `usize` large enough
    /// to carry a legitimate API frame. **Security:** the parser checks
    /// the peer-announced length against this bound *before* allocating,
    /// so a hostile server cannot coerce the client into multi-GiB
    /// allocations by advertising a huge frame. Example:
    /// `max_parser_frame_bytes = 8388608`.
    pub max_parser_frame_bytes: usize,
    /// Hard cap on concurrently-active IPC connections across the
    /// whole daemon process. Default: `128`. Runtime override for the
    /// previously-compile-time constant `pcloud_ipc::MAX_IPC_CONNECTIONS`
    /// (ncx.59). Enterprise deployments with many automation peers can
    /// raise this without rebuilding; lowering it throttles resource
    /// usage on constrained hosts. Validated range: `[1, 65_535]`.
    #[serde(default = "default_max_ipc_connections")]
    pub max_ipc_connections: usize,
    /// Per-peer (per-UID) cap on simultaneously-active IPC connections.
    /// Default: `32`. Runtime override for the previously-compile-time
    /// constant `pcloud_ipc::MAX_IPC_CONNECTIONS_PER_PEER` (ncx.59).
    /// **Security:** prevents a single misbehaving local user from
    /// monopolising the process-global slot pool. Must not exceed
    /// [`Self::max_ipc_connections`]; on load the daemon silently
    /// clamps the per-peer value down to the global cap. Validated
    /// range: `[1, 65_535]`.
    #[serde(default = "default_max_ipc_connections_per_peer")]
    pub max_ipc_connections_per_peer: usize,
}

/// Default for `ResourceLimits::max_ipc_connections`. Mirrors the
/// compile-time default in `pcloud-ipc::MAX_IPC_CONNECTIONS`.
#[must_use]
const fn default_max_ipc_connections() -> usize {
    128
}

/// Default for `ResourceLimits::max_ipc_connections_per_peer`. Mirrors
/// the compile-time default in `pcloud-ipc::MAX_IPC_CONNECTIONS_PER_PEER`.
#[must_use]
const fn default_max_ipc_connections_per_peer() -> usize {
    32
}

impl ResourceLimits {
    /// Validate the IPC-cap bounds added for ncx.59. Returns a typed
    /// `ConfigError::InvalidIpcLimits` on any of:
    /// - `max_ipc_connections == 0` (accept loop would be frozen);
    /// - `max_ipc_connections > 65_535` (unrealistic; guards against
    ///   accidental `usize::MAX`);
    /// - `max_ipc_connections_per_peer == 0`;
    /// - `max_ipc_connections_per_peer > max_ipc_connections`.
    ///
    /// Called from [`crate::ConfigProfile::validate`]; callers that need
    /// to validate a bare `ResourceLimits` (e.g. migration paths) may
    /// invoke this directly.
    pub fn validate_ipc_limits(&self) -> Result<(), crate::ConfigError> {
        const UPPER: usize = 65_535;
        if self.max_ipc_connections == 0 {
            return Err(crate::ConfigError::InvalidIpcLimits(
                "max_ipc_connections must be >= 1",
            ));
        }
        if self.max_ipc_connections > UPPER {
            return Err(crate::ConfigError::InvalidIpcLimits(
                "max_ipc_connections must be <= 65535",
            ));
        }
        if self.max_ipc_connections_per_peer == 0 {
            return Err(crate::ConfigError::InvalidIpcLimits(
                "max_ipc_connections_per_peer must be >= 1",
            ));
        }
        if self.max_ipc_connections_per_peer > UPPER {
            return Err(crate::ConfigError::InvalidIpcLimits(
                "max_ipc_connections_per_peer must be <= 65535",
            ));
        }
        if self.max_ipc_connections_per_peer > self.max_ipc_connections {
            return Err(crate::ConfigError::InvalidIpcLimits(
                "max_ipc_connections_per_peer must be <= max_ipc_connections",
            ));
        }
        Ok(())
    }
}

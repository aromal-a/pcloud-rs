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
/// Persists in the envelope's `profile.limits` object. All three fields
/// are required by the schema; there are no env-var overrides today.
/// [`crate::ConfigProfile::secure_defaults`] uses:
///
/// - `max_concurrent_uploads = 4`
/// - `max_concurrent_downloads = 4`
/// - `max_parser_frame_bytes = 8 MiB` (`8 * 1024 * 1024`)
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
}

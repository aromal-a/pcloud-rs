// **PLATFORM:** all
// **GATING:** none (portable).

/// Token-bucket bandwidth limiter (placeholder, disabled by default).
///
/// Real throttling is deferred until bandwidth-limit UX is confirmed
/// (tracked as TODO(bd-1du-bandwidth)). The struct and config field exist
/// so the feature can be toggled on without an API break.
pub mod bandwidth;
/// Download coordinator: tracks in-flight, completed, and failed
/// downloads.
pub mod downloads;
/// Upload coordinator: tracks in-flight, completed, and failed uploads.
pub mod uploads;

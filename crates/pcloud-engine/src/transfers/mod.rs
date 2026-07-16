// **PLATFORM:** all
// **GATING:** none (portable).

/// Token-bucket bandwidth limiter (placeholder, disabled by default).
///
/// Real throttling is deferred until bandwidth-limit UX is confirmed
/// (tracked as TODO(bd-1du-bandwidth)). The struct and config field exist
/// so the feature can be toggled on without an API break.
pub mod bandwidth;
/// T2.1.c — plan-side differential-upload strategy. Decides whether a
/// queued upload should pre-compute an rsync-style delta or fall back
/// to a full-file upload. Pure compute; the actual execute-side
/// `upload_writefromfile` wiring is the next sub-step.
pub mod differential;
/// Download coordinator: tracks in-flight, completed, and failed
/// downloads.
pub mod downloads;
/// Upload coordinator: tracks in-flight, completed, and failed uploads.
pub mod uploads;

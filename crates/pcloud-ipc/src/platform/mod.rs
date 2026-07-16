//! **PLATFORM: all.** Trait layer over peer-authenticated local IPC.
//!
//! - Linux      → `platform::linux::LinuxIpc`   (SO_PEERCRED on AF_UNIX)
//! - FreeBSD,   → `platform::unix::UnixIpc`     (getpeereid on AF_UNIX)
//! - OpenBSD,
//! - NetBSD,
//! - macOS      → `platform::unix::UnixIpc`     (getpeereid on AF_UNIX)
//! - illumos,
//! - Solaris    → `platform::solarish::SolarishIpc` (getpeerucred on AF_UNIX)
//! - Windows    → `platform::windows::WindowsIpc` (named pipes + SID check)
//!
//! This module defines the [`PlatformIpc`] trait and the [`ActivePlatform`]
//! type alias resolved at compile time via `#[cfg]`. Callers in
//! `transport.rs` dispatch through [`ActivePlatform`] to obtain peer
//! credentials in a way that is correct for the current target OS.

use std::path::Path;

use crate::transport::IpcTransportError;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "macos",
    target_os = "dragonfly"
))]
pub mod unix;

#[cfg(any(target_os = "illumos", target_os = "solaris"))]
pub mod solarish;

#[cfg(windows)]
pub mod windows;

/// Abstraction over the platform-specific local IPC primitives:
/// listener creation, peer UID recovery, and peer display rendering.
///
/// Implementations choose their own concrete `Listener`/`Stream` types so
/// each platform can stay native (AF_UNIX on Unix, named pipes on
/// Windows) without paying for an erasure step.
pub trait PlatformIpc {
    /// Concrete listener type for this platform (e.g. `UnixListener`).
    type Listener;
    /// Concrete connected-peer stream type for this platform
    /// (e.g. `UnixStream`).
    type Stream;

    /// Bind a listener inside `runtime_dir` using the platform's peer-
    /// authenticated local IPC mechanism.
    fn bind_listener(&self, runtime_dir: &Path) -> Result<Self::Listener, IpcTransportError>;

    /// Recover the peer's uid from an accepted connection.
    fn peer_uid(&self, stream: &Self::Stream) -> Result<u32, IpcTransportError>;

    /// Render a short human-readable description of the peer for audit
    /// logs. Never contains secrets.
    fn peer_display(&self, stream: &Self::Stream) -> Result<String, IpcTransportError>;

    /// Short, stable identifier for the active backend implementation.
    ///
    /// Used by diagnostics, audit logs, and cross-platform tests to
    /// identify which peer-authentication mechanism is compiled in
    /// without relying on `std::any::type_name`. Symmetric with
    /// `pcloud_secret::platform::PlatformVault::backend_name`.
    ///
    /// The return value is one of the following fixed strings:
    ///
    /// ```text
    /// "linux-so-peercred"   // Linux (SO_PEERCRED on AF_UNIX)
    /// "unix-getpeereid"     // FreeBSD / OpenBSD / NetBSD / macOS (getpeereid(3))
    /// "solarish-getpeerucred" // illumos/Solaris (getpeerucred(3))
    /// "windows-named-pipe"    // Windows (named pipe + TokenUser SID match)
    /// ```
    ///
    /// A fourth value may be added here if a future backend lands; callers
    /// should treat the string as opaque beyond equality comparison.
    fn backend_name(&self) -> &'static str;
}

/// Alias resolved at compile time to the platform backend for the
/// current target OS. Used by the rest of the crate so call sites do not
/// carry `#[cfg]` themselves.
#[cfg(target_os = "linux")]
pub type ActivePlatform = linux::LinuxIpc;

/// Alias resolved at compile time to the platform backend for the
/// current target OS.
#[cfg(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "macos",
    target_os = "dragonfly"
))]
pub type ActivePlatform = unix::UnixIpc;

/// Alias resolved at compile time to the illumos/Solaris AF_UNIX backend.
#[cfg(any(target_os = "illumos", target_os = "solaris"))]
pub type ActivePlatform = solarish::SolarishIpc;

/// Alias resolved at compile time to the platform backend for the
/// current target OS.
///
/// The Windows backend is wired through the shared client and server loops.
/// Native Windows CI executes its same-user SID-authentication round trip;
/// cross-user rejection remains a privileged qualification case.
#[cfg(windows)]
pub type ActivePlatform = windows::WindowsIpc;

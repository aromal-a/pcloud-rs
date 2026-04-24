//! **PLATFORM: all** (Linux | FreeBSD | macOS | Windows).
//! **GATING: none at the module level** -- per-OS submodules use
//! `#[cfg(target_os = "...")]` and are re-exported through a glob that is
//! itself `#[cfg]`-gated.
//!
//! Platform abstraction for mount operations.
//!
//! # Architecture
//!
//! A single [`PlatformMount`] trait hides four very different native
//! stacks behind a uniform `mount_adapter(Box<dyn FuseAdapter>, &Path,
//! MountOptions) -> MountHandle` seam. The daemon picks an
//! `ActivePlatformMount` at compile time via `#[cfg(target_os)]`; no
//! runtime dispatch to unsupported platforms is possible.
//!
//! # Supported platforms
//!
//! | OS | Stack | Transport | Notes |
//! |----|-------|-----------|-------|
//! | Linux (tier 1) | libfuse3 via the `fuser` crate | `/dev/fuse` character device + `mount(2)` with `fstype="fuse"` | Typed generic entry point exists alongside the dyn one for efficiency. |
//! | FreeBSD (tier 2) | libfuse2 via `fuser` | `/dev/fuse` + `nmount(2)` | Same API shape as Linux but pinned to libfuse2 ABI. |
//! | macOS (tier 1, planned) | fuse-t via direct FFI | UNIX-domain socket to the userspace fuse-t server + `mount_nfs`-style vnode ops | No kext required; fuse-t acts as NFS translator. |
//! | Windows (tier 1, planned) | WinFSP via direct FFI | `\\Device\\Volume{GUID}` reparse + `FspFileSystemStartDispatcher` | Volume names, drive letters, and security descriptors differ from POSIX. |
//! | OpenBSD / NetBSD (tier 3) | community | n/a | No implementation in-tree. |
//!
//! This module defines two traits:
//!
//! * [`PlatformMount`] -- the kernel (un)mount seam. Implemented per OS.
//! * [`MountinfoReader`] (re-exported from [`crate::mount_orphan`]) --
//!   enumerates live pCloud FUSE mounts for orphan detection. Linux reads
//!   `/proc/self/mountinfo`; BSD is expected to wrap `getmntinfo(3)`;
//!   macOS/Windows expose their native APIs.
//!
//! Downstream callers should program against these traits rather than
//! against the concrete per-OS types so the daemon stays portable.

use std::path::Path;

use crate::fuse_adapter::FuseAdapter;
use crate::mount_service::{MountError, MountHandle, MountOptions};

/// Kernel-level mount/unmount seam. Each supported platform provides a
/// concrete implementation. Unsupported platforms fail fast with a clear
/// `MountError::UnsupportedPlatform`.
///
/// Implementations must be `Send + Sync + 'static` so they can be stored
/// inside the daemon runtime registry.
pub trait PlatformMount: Send + Sync + 'static {
    /// Validate the mountpoint (existence, type, ownership, permissions).
    ///
    /// Default behavior delegates to
    /// [`crate::mount_service::MountService::validate_mountpoint`]; platforms
    /// may override to add OS-specific checks (e.g. Windows drive letters).
    fn validate_mountpoint(&self, mountpoint: &Path) -> Result<(), MountError>;

    /// Probe whether this platform can actually perform a mount right now.
    /// Returns `Ok(())` when the platform has a live implementation,
    /// [`MountError::UnsupportedPlatform`] otherwise.
    ///
    /// This trait intentionally does **not** take `F: fuser::Filesystem`
    /// directly, because `fuser` is Linux/FreeBSD-only. The Linux
    /// implementation exposes a typed entry point
    /// ([`crate::platform::linux::mount_fuser_filesystem`]); other
    /// platforms are expected to expose their own typed entry points.
    fn probe_supported(&self) -> Result<(), MountError>;

    /// Default (shared) mount-option flavor for this platform.
    fn default_options(&self) -> MountOptions {
        MountOptions::default()
    }

    /// Return `MountError::UnsupportedPlatform` as a `MountHandle` result.
    /// Kept on the trait to force every OS impl to answer the question
    /// "can you mount today?" explicitly.
    fn unsupported(&self) -> Result<MountHandle, MountError> {
        Err(MountError::UnsupportedPlatform)
    }

    /// Mount a type-erased [`FuseAdapter`] at `mount_point` using this
    /// platform's kernel integration. Default implementation returns
    /// [`MountError::UnsupportedPlatform`] so each OS can opt in
    /// incrementally without breaking older call sites.
    ///
    /// The signature is boxed/dyn rather than generic so the trait stays
    /// dyn-safe (callers store `Box<dyn PlatformMount>` in the daemon
    /// registry) and so the adapter can be moved across FFI thunks that
    /// require a stable `*mut c_void` user-data pointer (macOS fuse-t,
    /// Windows WinFSP). Platforms with a more efficient generic path
    /// (Linux) retain their existing typed entry points alongside this.
    fn mount_adapter(
        &self,
        adapter: Box<dyn FuseAdapter>,
        mount_point: &Path,
        opts: MountOptions,
    ) -> Result<MountHandle, MountError> {
        let _ = (adapter, mount_point, opts);
        Err(MountError::UnsupportedPlatform)
    }
}

/// Re-export of the canonical `MountinfoReader` trait. Its definition
/// lives in [`crate::mount_orphan`] because its string-payload contract
/// is cross-platform (each OS impl just supplies its own payload).
pub use crate::mount_orphan::MountinfoReader;

#[cfg(any(target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
pub mod bsd;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
// `#[allow(missing_docs)]`: Windows platform scaffolding (WinFSP FFI +
// ACL helpers) is Tier-3 per CLAUDE.md — compile-tested only, not yet
// live-verified. Its FFI structs mirror WinFSP's C headers verbatim;
// documenting each u32/u64 field one-by-one duplicates the upstream
// header content. Crate-level `#![deny(missing_docs)]` is relaxed here
// until bd-xplat-windows promotes the module to Tier-2 and the FFI
// surface stabilises.
#[cfg(target_os = "windows")]
#[allow(missing_docs)]
pub mod windows;

#[cfg(any(target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
pub use self::bsd::BsdPlatformMount as ActivePlatformMount;
#[cfg(target_os = "linux")]
pub use self::linux::LinuxPlatformMount as ActivePlatformMount;
#[cfg(target_os = "macos")]
pub use self::macos::MacosPlatformMount as ActivePlatformMount;
#[cfg(target_os = "windows")]
pub use self::windows::WindowsPlatformMount as ActivePlatformMount;

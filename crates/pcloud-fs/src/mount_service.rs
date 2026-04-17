//! **PLATFORM: all** (Linux | FreeBSD | macOS | Windows) at the type
//! level; **Linux-live** at the implementation level.
//! **GATING: `#[cfg(target_os = "linux")]`** on the Linux back-end
//! delegations in this file; other platforms return
//! [`MountError::UnsupportedPlatform`].
//!
//! Mount lifecycle scaffold for bd-1du.4.a.
//!
//! Provides [`MountService`], the public entry point for mounting a
//! `FuseAdapter` at a filesystem path, and [`MountHandle`], an RAII guard
//! that unmounts on drop. A process-wide SIGTERM/SIGINT handler is
//! registered on first mount so that the kernel mount is cleaned up even
//! on abrupt shutdown.
//!
//! The concrete OS implementation of "open a FUSE session and hand back a
//! RAII handle" lives in [`crate::platform::linux`]. This module is a
//! thin cross-platform shim that validates the mountpoint and delegates.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::fuse_adapter::FuseAdapter;

/// Options accepted by [`MountService::mount`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountOptions {
    /// Mount read-only. Defaults to `true` for the 4.a scaffold.
    pub read_only: bool,
    /// Optional filesystem name shown in `/proc/mounts`. Defaults to `pcloud`.
    pub fs_name: Option<String>,
    /// When `true`, the mount would allow other users on the host to access
    /// it. This crate always rejects that configuration.
    pub allow_other: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            read_only: true,
            fs_name: None,
            allow_other: false,
        }
    }
}

/// Errors that can be returned when validating a mountpoint or mounting.
#[derive(Debug, Error)]
pub enum MountError {
    /// The mountpoint path does not exist on disk.
    #[error("mountpoint does not exist: {0}")]
    MountpointMissing(PathBuf),
    /// The mountpoint path exists but is not a directory.
    #[error("mountpoint is not a directory: {0}")]
    MountpointNotDirectory(PathBuf),
    /// The mountpoint directory is not empty.
    #[error("mountpoint is not empty: {0}")]
    MountpointNotEmpty(PathBuf),
    /// The mountpoint is owned by a different uid than the current process.
    #[error("mountpoint is not owned by current uid ({current}): {path} (owner uid={owner})")]
    MountpointNotOwned {
        /// Mountpoint that was checked.
        path: PathBuf,
        /// Owning uid reported by `stat`.
        owner: u32,
        /// Current effective uid.
        current: u32,
    },
    /// The mountpoint has the world-writable bit set.
    #[error("mountpoint is world-writable (mode=0o{mode:o}): {path}")]
    MountpointWorldWritable {
        /// Mountpoint that was checked.
        path: PathBuf,
        /// Full mode bits reported by `stat`.
        mode: u32,
    },
    /// The caller requested `allow_other`, which this service rejects by
    /// policy (non-opt-in broad access to other users).
    #[error("allow_other is rejected by the Rust mount service")]
    AllowOtherRejected,
    /// The current platform has no FUSE implementation linked in.
    #[error("mount is only supported on Linux")]
    UnsupportedPlatform,
    /// Platform is theoretically supported (e.g. macOS via fuse-t) but a
    /// required runtime component is missing or the scaffolding has not
    /// yet been brought up end-to-end on that platform. The payload
    /// carries a human-readable remediation hint.
    #[error("mount unsupported on this platform: {0}")]
    Unsupported(String),
    /// Unexpected I/O error while inspecting the mountpoint.
    #[error("mount i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The Linux `fuser` crate reported an error while setting up the
    /// session.
    #[cfg(target_os = "linux")]
    #[error("fuser session error: {0}")]
    Fuser(String),
}

/// Mount service scaffold.
#[derive(Debug, Default, Clone, Copy)]
pub struct MountService;

impl MountService {
    /// Construct a zero-sized mount service handle.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Validate that `mountpoint` is safe to use.
    ///
    /// Checks, in order: path exists, is a directory, is empty, is owned by
    /// current uid (Linux), and is not world-writable (Linux).
    pub fn validate_mountpoint(mountpoint: &Path) -> Result<(), MountError> {
        let meta = match std::fs::metadata(mountpoint) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(MountError::MountpointMissing(mountpoint.to_path_buf()));
            }
            Err(e) => return Err(MountError::Io(e)),
        };

        if !meta.is_dir() {
            return Err(MountError::MountpointNotDirectory(mountpoint.to_path_buf()));
        }

        let mut entries = std::fs::read_dir(mountpoint)?;
        if entries.next().is_some() {
            return Err(MountError::MountpointNotEmpty(mountpoint.to_path_buf()));
        }

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            // SAFETY: geteuid is always safe.
            let current_uid = unsafe { libc::geteuid() };
            let owner = meta.uid();
            if owner != current_uid {
                return Err(MountError::MountpointNotOwned {
                    path: mountpoint.to_path_buf(),
                    owner,
                    current: current_uid,
                });
            }
            let mode = meta.mode();
            if mode & 0o002 != 0 {
                return Err(MountError::MountpointWorldWritable {
                    path: mountpoint.to_path_buf(),
                    mode: mode & 0o7777,
                });
            }
        }

        Ok(())
    }

    /// Mount `adapter` at `mountpoint` with `options`.
    pub fn mount<A: FuseAdapter>(
        &self,
        mountpoint: &Path,
        adapter: A,
        options: MountOptions,
    ) -> Result<MountHandle, MountError> {
        if options.allow_other {
            return Err(MountError::AllowOtherRejected);
        }

        Self::validate_mountpoint(mountpoint)?;

        #[cfg(target_os = "linux")]
        {
            crate::platform::linux::mount_with_fuser(mountpoint, adapter, options)
        }

        #[cfg(target_os = "macos")]
        {
            use crate::platform::PlatformMount;
            let backend = crate::platform::macos::MacosPlatformMount;
            backend.mount_adapter(Box::new(adapter), mountpoint, options)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = adapter;
            let _ = options;
            Err(MountError::UnsupportedPlatform)
        }
    }

    /// Mount an arbitrary [`fuser::Filesystem`] implementation at `mountpoint`.
    ///
    /// This is the live-composition path used by the daemon: the caller
    /// supplies a real `fuser::Filesystem` (e.g. [`crate::fuser_shim::PcloudFsShim`])
    /// whose kernel operations are actually wired through to backends.
    ///
    /// Mountpoint validation, `allow_other` rejection, and the NoDev/NoSuid/
    /// DefaultPermissions hardening are identical to [`Self::mount`].
    #[cfg(target_os = "linux")]
    pub fn mount_fuser<F>(
        &self,
        mountpoint: &Path,
        filesystem: F,
        options: MountOptions,
    ) -> Result<MountHandle, MountError>
    where
        F: fuser::Filesystem + Send + 'static,
    {
        if options.allow_other {
            return Err(MountError::AllowOtherRejected);
        }
        Self::validate_mountpoint(mountpoint)?;
        crate::platform::linux::mount_fuser_filesystem(mountpoint, filesystem, options)
    }
}

/// RAII guard for an active mount. Dropping the handle triggers an
/// unmount.
///
/// # Lifecycle
///
/// `MountHandle` is constructed exclusively via the per-OS
/// `from_linux` / `from_macos` / `from_windows` factory constructors;
/// end users never build one directly. Construction transfers
/// ownership of:
///
/// * the native session/filesystem pointer,
/// * any retained OS buffers that FFI callbacks reference by pointer
///   (UTF-16 mount path on Windows, `CString` mountpoint on macOS),
/// * the boxed `dyn FuseAdapter` whose raw address was installed in
///   the platform's user-data slot.
///
/// # Ordered teardown (5-second timeout)
///
/// Both [`Self::unmount`] and `Drop` execute the same ordered sequence:
///
/// 1. Flip the cooperative `shutdown` flag so worker threads observe
///    exit ASAP.
/// 2. Call the native "break the dispatch loop" API
///    (`fuse_session_exit` / `FspFileSystemStopDispatcher` /
///    `fuser::Session::exit`).
/// 3. Issue the native unmount (`fuse_unmount` / `FspFileSystemRemove
///    MountPoint` / `umount2`).
/// 4. Join the background dispatcher thread with a **5-second bounded
///    wait** — the `JoinHandle` is moved into a helper thread and a
///    `recv_timeout` gates the wait so a wedged loop cannot block
///    `Drop` forever.
/// 5. Destroy native state (`fuse_session_destroy` /
///    `FspFileSystemDelete`).
/// 6. Reclaim and drop the leaked `Box<dyn FuseAdapter>`.
///
/// # Drop discipline
///
/// * `Drop` is infallible — failures are logged and swallowed because
///   panicking in `Drop` risks a double-panic on unwinding.
/// * Prefer [`Self::unmount`] when you need to observe unmount errors;
///   it returns `Result<(), MountError>` and makes `Drop` a no-op.
/// * The `#[must_use]` attribute nudges callers to bind the handle to
///   a name rather than let it drop immediately after `mount()`
///   returns.
#[must_use = "dropping the MountHandle unmounts the filesystem"]
pub struct MountHandle {
    #[cfg(target_os = "linux")]
    inner: Option<crate::platform::linux::LinuxMountHandle>,
    #[cfg(target_os = "windows")]
    windows_inner: Option<WindowsInner>,
    #[cfg(target_os = "macos")]
    macos_inner: Option<MacosMountInner>,
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    _phantom: std::marker::PhantomData<()>,
}

/// macOS-specific inner state for an active fuse-t mount.
///
/// **PLATFORM: macOS only.** Bundles the fuse-t session, the
/// mountpoint `CString` (kept alive for `fuse_unmount`), the
/// background thread running `fuse_session_loop`, and the shutdown
/// flag used to coordinate teardown. `user_data` holds the type-
/// erased `FuseAdapter` box whose address was installed as
/// `user_data` on the fuse-t session; it must outlive the session.
///
/// **NOT YET TESTED ON MACOS** — bring-up requires a real Mac with
/// fuse-t installed. Ships pending PHASE-4 live verification.
#[cfg(target_os = "macos")]
pub(crate) struct MacosMountInner {
    pub(crate) session: *mut crate::platform::macos::macos_ffi::fuse_session,
    pub(crate) chan: *mut crate::platform::macos::macos_ffi::fuse_chan,
    pub(crate) mountpoint_cstring: std::ffi::CString,
    pub(crate) loop_thread: Option<std::thread::JoinHandle<()>>,
    pub(crate) shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) user_data: Option<Box<Box<dyn crate::fuse_adapter::FuseAdapter>>>,
}

// SAFETY: fuse-t session/chan pointers are opaque kernel handles. We
// own the unique reference and all FFI calls involving them happen
// on teardown/initialization paths we control. The loop thread only
// invokes `fuse_session_loop`; `fuse_session_exit` is documented safe
// to call from a different thread than the loop.
#[cfg(target_os = "macos")]
unsafe impl Send for MacosMountInner {}
#[cfg(target_os = "macos")]
unsafe impl Sync for MacosMountInner {}

/// Windows-gated inner state for a live WinFSP mount.
///
/// * `fs` is the opaque `FSP_FILE_SYSTEM*` returned by `FspFileSystemCreate`.
/// * `mount_point` is the UTF-16 NUL-terminated buffer we passed to
///   `FspFileSystemSetMountPoint`; it is retained so its address remains
///   valid for the dispatcher's lifetime even though WinFSP copies it
///   internally.
/// * `adapter` is a `Box<dyn FuseAdapter>` that was leaked via
///   `Box::into_raw` so callback thunks can recover it from the WinFSP
///   user-context slot; `Drop` reclaims it to free the adapter.
#[cfg(target_os = "windows")]
pub(crate) struct WindowsInner {
    pub fs: *mut std::ffi::c_void,
    pub mount_point: Vec<u16>,
    pub adapter: *mut std::ffi::c_void,
    pub lib: std::sync::Arc<crate::platform::windows::winfsp_ffi::WinFspLibrary>,
}

// SAFETY: the raw pointers are owned exclusively by `MountHandle`; ownership
// is transferred on Drop / unmount. `WinFspLibrary` is Send+Sync.
#[cfg(target_os = "windows")]
unsafe impl Send for WindowsInner {}
#[cfg(target_os = "windows")]
unsafe impl Sync for WindowsInner {}

impl MountHandle {
    /// Construct a `MountHandle` from a Linux inner handle. Used by
    /// [`crate::platform::linux`] mount entry points.
    #[cfg(target_os = "linux")]
    pub(crate) fn from_linux(inner: crate::platform::linux::LinuxMountHandle) -> Self {
        Self { inner: Some(inner) }
    }

    /// Construct a `MountHandle` from a live WinFSP file-system handle.
    ///
    /// The `adapter` pointer must be a `Box<Box<dyn FuseAdapter>>`-equivalent
    /// raw pointer that was produced via `Box::into_raw`, so `Drop` can
    /// reclaim and free it. `mount_point` is the UTF-16 NUL-terminated
    /// buffer passed to `FspFileSystemSetMountPoint`; we retain it for the
    /// mount's lifetime.
    #[cfg(target_os = "windows")]
    pub(crate) fn from_windows(
        fs: *mut std::ffi::c_void,
        mount_point: Vec<u16>,
        adapter: *mut std::ffi::c_void,
        lib: std::sync::Arc<crate::platform::windows::winfsp_ffi::WinFspLibrary>,
    ) -> Self {
        Self {
            windows_inner: Some(WindowsInner {
                fs,
                mount_point,
                adapter,
                lib,
            }),
        }
    }

    /// Construct a `MountHandle` from macOS fuse-t mount state. Used
    /// by [`crate::platform::macos`] mount entry points.
    ///
    /// **NOT YET TESTED ON MACOS** — real-Mac bring-up pending.
    #[cfg(target_os = "macos")]
    pub(crate) fn from_macos(
        session: *mut crate::platform::macos::macos_ffi::fuse_session,
        chan: *mut crate::platform::macos::macos_ffi::fuse_chan,
        mountpoint_cstring: std::ffi::CString,
        loop_thread: std::thread::JoinHandle<()>,
        shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
        user_data: Box<Box<dyn crate::fuse_adapter::FuseAdapter>>,
    ) -> Self {
        Self {
            macos_inner: Some(MacosMountInner {
                session,
                chan,
                mountpoint_cstring,
                loop_thread: Some(loop_thread),
                shutdown,
                user_data: Some(user_data),
            }),
        }
    }

    /// Explicitly unmount. After this call, drop is a no-op.
    pub fn unmount(mut self) -> Result<(), MountError> {
        #[cfg(target_os = "linux")]
        {
            if let Some(inner) = self.inner.take() {
                return inner.unmount();
            }
            Ok(())
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(inner) = self.windows_inner.take() {
                Self::teardown_windows(inner);
            }
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(inner) = self.macos_inner.take() {
                Self::teardown_macos(inner);
            }
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            Ok(())
        }
    }

    /// Teardown a live macOS fuse-t mount.
    ///
    /// Order is load-bearing:
    /// 1. flip shutdown flag so cooperating paths observe exit,
    /// 2. `fuse_session_exit` breaks the session loop,
    /// 3. `fuse_unmount` releases the kernel mount (loop will return),
    /// 4. join the loop thread with a 5-second bounded wait — we move
    ///    the `JoinHandle` into a helper thread and `recv_timeout` on
    ///    a channel so a wedged loop cannot block `Drop` forever,
    /// 5. `fuse_session_destroy` frees session state,
    /// 6. drop the adapter `user_data` once the session is gone.
    ///
    /// **NOT YET TESTED ON MACOS** — ships pending PHASE-4 live
    /// verification on real hardware with fuse-t installed.
    #[cfg(target_os = "macos")]
    fn teardown_macos(mut inner: MacosMountInner) {
        use std::sync::atomic::Ordering;
        use std::sync::mpsc;
        use std::time::Duration;

        inner.shutdown.store(true, Ordering::SeqCst);

        // SAFETY: `session` was returned by `fuse_lowlevel_new` and
        // has not been destroyed yet; `fuse_session_exit` is
        // documented safe to call from a thread other than the loop.
        unsafe {
            crate::platform::macos::macos_ffi::fuse_session_exit(inner.session);
        }

        // SAFETY: `chan` came from `fuse_mount`; `mountpoint_cstring`
        // is NUL-terminated and alive for this call. After this
        // returns the kernel-side mount is released and the loop
        // will exit.
        unsafe {
            crate::platform::macos::macos_ffi::fuse_unmount(
                inner.mountpoint_cstring.as_ptr(),
                inner.chan,
            );
        }

        if let Some(handle) = inner.loop_thread.take() {
            let (tx, rx) = mpsc::channel::<()>();
            let joiner = std::thread::spawn(move || {
                let _ = handle.join();
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(Duration::from_secs(5));
            // If the loop wedged, detach the joiner rather than block.
            drop(joiner);
        }

        // SAFETY: no further FFI references `session` after this;
        // `fuse_session_destroy` frees libfuse-owned state.
        unsafe {
            crate::platform::macos::macos_ffi::fuse_session_destroy(inner.session);
        }

        // Drop adapter last: the session is dead, so no thunk can
        // still dereference the user-data pointer.
        drop(inner.user_data.take());
    }

    #[cfg(target_os = "windows")]
    fn teardown_windows(mut inner: WindowsInner) {
        if inner.fs.is_null() {
            return;
        }
        // SAFETY: `fs` is a valid WinFSP handle we own. Stop must precede
        // Delete; after Delete the user-context pointer is no longer
        // referenced by WinFSP so we can reclaim the boxed adapter.
        unsafe {
            (inner.lib.fsp_stop_dispatcher)(inner.fs);
            (inner.lib.fsp_delete)(inner.fs);
            if !inner.adapter.is_null() {
                // SAFETY: the adapter pointer was produced via
                // `Box::into_raw(Box::new(Box::<dyn FuseAdapter>::...))`
                // in `mount_with_winfsp`; dropping the reconstructed Box
                // releases the trait object.
                let _ =
                    Box::from_raw(inner.adapter as *mut Box<dyn crate::fuse_adapter::FuseAdapter>);
            }
        }
        inner.fs = std::ptr::null_mut();
        inner.adapter = std::ptr::null_mut();
    }
}

impl Drop for MountHandle {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            if let Some(inner) = self.inner.take() {
                let _ = inner.unmount();
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(inner) = self.windows_inner.take() {
                Self::teardown_windows(inner);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(inner) = self.macos_inner.take() {
                Self::teardown_macos(inner);
            }
        }
    }
}

impl std::fmt::Debug for MountHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MountHandle").finish_non_exhaustive()
    }
}

// Linux FUSE glue has moved to `crate::platform::linux`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuse_adapter::NullFuseAdapter;
    use tempfile::tempdir;

    #[test]
    fn rejects_missing_mountpoint() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = MountService::validate_mountpoint(&missing).unwrap_err();
        assert!(matches!(err, MountError::MountpointMissing(_)));
    }

    #[test]
    fn rejects_non_directory_mountpoint() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("file.txt");
        std::fs::write(&file, b"hi").unwrap();
        let err = MountService::validate_mountpoint(&file).unwrap_err();
        assert!(matches!(err, MountError::MountpointNotDirectory(_)));
    }

    #[test]
    fn rejects_non_empty_mountpoint() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("leftover"), b"x").unwrap();
        let err = MountService::validate_mountpoint(tmp.path()).unwrap_err();
        assert!(matches!(err, MountError::MountpointNotEmpty(_)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_world_writable_mountpoint() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("ww");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = MountService::validate_mountpoint(&dir).unwrap_err();
        assert!(
            matches!(err, MountError::MountpointWorldWritable { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn rejects_allow_other_option() {
        let tmp = tempdir().unwrap();
        let svc = MountService::new();
        let err = svc
            .mount(
                tmp.path(),
                NullFuseAdapter,
                MountOptions {
                    allow_other: true,
                    ..MountOptions::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, MountError::AllowOtherRejected));
    }

    #[test]
    fn accepts_clean_empty_private_directory() {
        let tmp = tempdir().unwrap();
        MountService::validate_mountpoint(tmp.path()).expect("fresh tempdir must validate");
    }
}

// -----------------------------------------------------------------------------
// Linux integration test: real mount + immediate unmount.
// Gated so CI environments without FUSE do not fail by default.
// -----------------------------------------------------------------------------

#[cfg(all(test, target_os = "linux"))]
mod linux_integration {
    use super::*;
    use crate::fuse_adapter::NullFuseAdapter;
    use tempfile::tempdir;

    fn fuse_gate_enabled() -> bool {
        std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1")
    }

    #[test]
    #[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
    fn mount_and_immediate_unmount_cleanly() {
        if !fuse_gate_enabled() {
            return;
        }
        let tmp = tempdir().unwrap();
        let svc = MountService::new();
        let handle = svc
            .mount(tmp.path(), NullFuseAdapter, MountOptions::default())
            .expect("mount must succeed when libfuse is available");
        handle.unmount().expect("unmount must succeed");
    }
}

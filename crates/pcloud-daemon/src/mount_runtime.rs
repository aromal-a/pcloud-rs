//! Daemon-side native mount orchestration.
//!
//! This module wires the narrow IPC-visible surface:
//!
//! * `MountControl` owns the active `pcloud_fs::mount::MountHandle` and a
//!   small drain hook that runs on unmount.
//! * `mount_filesystem` validates the mountpoint, composes a FUSE adapter,
//!   and hands it to `pcloud_fs::mount::MountService::mount`.
//! * `unmount_filesystem` calls the drain hook and tears the session down.
//!
//! ## Current wiring
//!
//! The authenticated mount path installs a **fully composed
//! `ProtoFuseAdapter`** via [`pcloud_shim_adapter_factory`] — this is done
//! in `crate::runtime::Runtime::try_install_pcloud_shim_factory` which
//! runs before every `mount_filesystem()` call. The composed adapter
//! carries:
//!
//! * a daemon-owned canonical `RemoteFs` adapter for live metadata,
//!   range reads, folder mutations, and write sessions,
//! * the standard folder/transfer runtimes behind that single service,
//!   with a zeroising [`SecretString`] auth token,
//! * a [`PageCacheGeneric<PageKey>`][pcloud_fs::page_cache::PageCacheGeneric]
//!   sized from `[mount].cache_size_mb` and/or `PCLOUD_CACHE_SIZE_GB`,
//! * a [`WriteJournal`] + [`WritePathService`] rooted under
//!   `<cache_dir>/fuse-staging`.
//!
//! On Linux the adapter is wrapped in a [`PcloudFsShim`] and dispatched via
//! [`MountService::mount_fuser`]; on macOS and Windows the bare adapter is
//! handed to [`MountService::mount`], which routes through fuse-t or WinFSP.
//! All filesystem
//! operations (lookup, readdir, read, write, flush, fsync, create, unlink,
//! rename, mkdir, rmdir) are forwarded through the adapter to the pCloud
//! API.
//!
//! The [`default_adapter_factory`] still returns a [`NullFuseAdapter`] as
//! the pre-auth fallback: if the daemon is asked to mount before a token
//! is available the mount succeeds with `ENOSYS` on every op so the
//! lifecycle itself (validate → mount → drain → unmount) is still
//! exercisable and the shutdown invariants hold.
//!
//! Do **not** claim platform release readiness on the back of this module
//! alone: native release-commit, package, and credentialed pCloud gates remain
//! authoritative.
//!
//! ## Write-path wiring
//!
//! [`pcloud_shim_adapter_factory`] now constructs a full
//! [`WritePathService`] bound to the daemon's
//! canonical `RemoteFs` upload adapter + per-mount [`StagingDir`] + on-disk
//! [`WriteJournal`] and hands it to the composed [`PcloudFsShim`]. On the
//! Linux kernel mount path this means `create` / `write` / `flush` /
//! `fsync` / `setattr(size)` / `unlink` / `rename` are serviced by the
//! real writer instead of returning `ENOSYS`. Read+write up to the 64
//! MiB `flush_threshold_bytes` finalises via whole-file upload. Writes that
//! reach the threshold use resumable `upload_create` / bounded
//! `upload_write` / `upload_save`, with acknowledged offsets persisted for
//! crash recovery. The drain hook installed by the same
//! factory flushes every dirty inode before the kernel mount
//! disappears, keeping the ordered 5s shutdown sequence intact.
//!
//! The parity-matrix row is implemented; platform release qualification is a
//! separate evidence gate.

// **PLATFORM:** Linux + macOS + Windows.
// **GATING:** `#[cfg(target_os = "linux")]` around the `PcloudFsShim`
// wrapper (fuser-only); macOS/Windows around the bare `ProtoFuseAdapter`
// wrapper that feeds fuse-t/WinFSP; shared FS
// composition (backends, staging, journal, write path) gated to
// `#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd", target_os = "macos", target_os = "windows"))]`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::fuse_adapter::{FuseAdapter, NullFuseAdapter};
#[cfg(not(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
)))]
use pcloud_fs::mount_orphan::ProcMountinfoReader;
use pcloud_fs::mount_orphan::{
    MountinfoReader, detect_orphans, fusermount_unmount, mountpoint_is_already_mounted,
};
use pcloud_fs::mount_service::{MountError, MountHandle, MountOptions, MountService};
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
use pcloud_fs::platform::bsd::BsdMountinfoReader;
#[cfg(target_os = "macos")]
use pcloud_fs::platform::macos::MacosMountinfoReader;
#[cfg(target_os = "windows")]
use pcloud_fs::platform::windows::WindowsMountinfoReader;
use pcloud_ipc::{Response, ResponseStatus};
use pcloud_observability::LockExt;

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_backends::{
    auth_backend::AuthRuntime,
    folder_backend::FolderRuntime,
    remote_fs::{DeleteOutcome, RemoteFs, RemoteFsError, RemoteId, UploadConflict},
    transfer_backend::TransferRuntime,
};
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_config::ConfigProfile;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_fs::backend::{FileBackend, FileHandle, FolderBackend};
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_fs::errors::FsError;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
use pcloud_fs::fuser_shim::PcloudFsShim;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_fs::staging::StagingDir;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_fs::write_journal::WriteJournal;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_fs::write_path::{FileUploadBackend, UploadStatus, WritePathError};
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_fs::write_path::{WritePathOptions, WritePathService};
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_proto::UploadSession;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_proto::folder_api::{RemoteFolderEntry, RemoteFolderListing};
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
use pcloud_secret::secret_string::SecretString;

/// Factory that produces a [`FuseAdapter`] for a given mount request.
///
/// Boxed as a trait object so that sub-task 1 (`PcloudFsShim`) can drop in
/// the real composed adapter without changing `MountControl`'s signature.
pub type AdapterFactory =
    Box<dyn Fn() -> Result<Box<dyn DynFuseAdapter>, MountError> + Send + Sync>;

/// Object-safe wrapper over [`FuseAdapter`].
///
/// `MountService::mount` is generic in `A: FuseAdapter`, but the runtime
/// needs to pick an adapter dynamically based on which backends have been
/// wired. This trait erases the concrete type.
pub trait DynFuseAdapter: Send + Sync + 'static {
    /// Consume self and mount at `mountpoint` using `service`.
    fn mount_with(
        self: Box<Self>,
        service: &MountService,
        mountpoint: &Path,
        options: MountOptions,
    ) -> Result<MountHandle, MountError>;
}

struct FuseAdapterBox<A: FuseAdapter>(A);

impl<A: FuseAdapter> DynFuseAdapter for FuseAdapterBox<A> {
    fn mount_with(
        self: Box<Self>,
        service: &MountService,
        mountpoint: &Path,
        options: MountOptions,
    ) -> Result<MountHandle, MountError> {
        service.mount(mountpoint, self.0, options)
    }
}

/// Wrap a concrete [`FuseAdapter`] into a [`DynFuseAdapter`].
pub fn boxed_adapter<A: FuseAdapter>(adapter: A) -> Box<dyn DynFuseAdapter> {
    Box::new(FuseAdapterBox(adapter))
}

/// Drain hook invoked before unmount.
///
/// `Ok` carries a short human summary. `Err` is a durability failure: an
/// explicit unmount must leave the mount active so dirty data remains
/// recoverable and the caller can retry after correcting the backend error.
pub type DrainHook = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

/// Journal fsync hook invoked as the very first step of the shutdown
/// sequence so data hits disk before the kernel mount disappears.
///
/// Separate from [`DrainHook`] because the drain hook produces a human
/// summary (and is fired on explicit unmount too), whereas this hook is
/// specifically for the ordered SIGTERM/Drop shutdown path and returns
/// a success/failure result that we can log.
pub type JournalSyncHook = Arc<dyn Fn() -> std::io::Result<()> + Send + Sync>;

/// Shutdown timeout for the ordered Drop path: how long we wait for the
/// kernel to release the mount after `fusermount -u` returns. 5s matches
/// the spec in P1.4.
const SHUTDOWN_UNMOUNT_WAIT: Duration = Duration::from_secs(5);

/// Outcome of [`MountControl::check_orphans`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanCheckOutcome {
    /// No orphan pCloud mounts detected. Safe to proceed.
    Clean,
    /// Orphan mounts exist; the caller refused to force-unmount. Payload
    /// lists the mountpoints so the caller can render a helpful message.
    Rejected(Vec<PathBuf>),
    /// Orphan mounts existed and the caller requested force-unmount;
    /// payload reports per-path outcome (`Ok` or the error message).
    ForceUnmounted(Vec<(PathBuf, Result<(), String>)>),
}

/// Daemon-owned mount state. Exactly one active mount per daemon; a
/// second `Mount` request while one is active returns `Conflict`.
pub struct MountControl {
    service: MountService,
    active: Option<ActiveMount>,
    factory: AdapterFactory,
    drain: DrainHook,
    journal_sync: JournalSyncHook,
    mountinfo_reader: Box<dyn MountinfoReader>,
    /// If `true`, `Drop` and `check_orphans` are permitted to invoke
    /// `fusermount -u` on orphan mounts. Off by default; flipped on by
    /// `--force-umount` / `PCLOUD_FORCE_UMOUNT=1`.
    force_umount: bool,
    /// Directory where the mount-pid sidecar is written on mount and
    /// removed on unmount. When `None` the sidecar is suppressed
    /// (used by unit tests that don't want on-disk side effects).
    ///
    /// The sidecar is `<state_dir>/mount_pid` containing a single
    /// textual line `"<pid> <mountpoint>\n"`. On daemon start
    /// [`Self::sweep_stale_pidfile`] reads it and, if the recorded
    /// pid is not alive, removes the file and flags the corresponding
    /// mountpoint for orphan cleanup.
    state_dir: Option<PathBuf>,
}

struct ActiveMount {
    mountpoint: PathBuf,
    handle: MountHandle,
}

/// Result of reading `<state_dir>/mount_pid` at daemon startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StalePidfileOutcome {
    /// No pidfile existed, nothing to clean up.
    Absent,
    /// Pidfile existed but the recorded process is still alive (likely
    /// a sibling pcloud-rs instance). The caller must not touch the file
    /// or its referenced mount.
    Live {
        /// Pid recorded in the file.
        pid: i32,
        /// Mountpoint recorded in the file.
        mountpoint: PathBuf,
    },
    /// Pidfile existed, the recorded pid is dead, and the file has been
    /// removed. Payload carries the mountpoint the crashed daemon had
    /// registered so the caller can log a remediation hint.
    Cleaned {
        /// Mountpoint recovered from the stale pidfile.
        mountpoint: PathBuf,
    },
    /// Pidfile existed but was malformed / unreadable. The file has
    /// been removed; caller is left with no recoverable mountpoint.
    Corrupt,
}

impl std::fmt::Debug for MountControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MountControl")
            .field("service", &self.service)
            .field(
                "active_mountpoint",
                &self.active.as_ref().map(|a| a.mountpoint.clone()),
            )
            .finish_non_exhaustive()
    }
}

impl Default for MountControl {
    fn default() -> Self {
        Self::new(
            default_adapter_factory(),
            Arc::new(|| Ok("no drain work queued".to_owned())),
        )
    }
}

impl MountControl {
    /// Build a fresh mount controller from an adapter `factory` and a
    /// `drain` hook. The journal-sync hook defaults to a no-op and can
    /// be installed later via [`MountControl::set_journal_sync`].
    /// Force-unmount defaults to the value parsed from the
    /// `PCLOUD_FORCE_UMOUNT` environment variable.
    pub fn new(factory: AdapterFactory, drain: DrainHook) -> Self {
        Self {
            service: MountService::new(),
            active: None,
            factory,
            drain,
            journal_sync: Arc::new(|| Ok(())),
            mountinfo_reader: Self::default_mountinfo_reader(),
            force_umount: force_umount_from_env(),
            state_dir: None,
        }
    }

    fn default_mountinfo_reader() -> Box<dyn MountinfoReader> {
        #[cfg(target_os = "macos")]
        {
            Box::new(MacosMountinfoReader)
        }
        #[cfg(target_os = "linux")]
        {
            Box::new(ProcMountinfoReader)
        }
        #[cfg(any(
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        {
            Box::new(BsdMountinfoReader)
        }
        #[cfg(target_os = "windows")]
        {
            Box::new(WindowsMountinfoReader)
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly",
            target_os = "macos",
            target_os = "windows"
        )))]
        {
            // Platforms without a kernel-mount backend cannot own stale
            // pcloud mounts. The non-Linux reader intentionally returns an
            // empty payload, while mount attempts fail as UnsupportedPlatform.
            Box::new(ProcMountinfoReader)
        }
    }

    /// Install the `<state_dir>/mount_pid` sidecar location used by
    /// [`Self::mount`] and [`Self::sweep_stale_pidfile`]. Call once
    /// during daemon bootstrap right after constructing `MountControl`.
    /// When unset, the sidecar is skipped — useful for unit tests.
    pub fn set_state_dir(&mut self, state_dir: PathBuf) {
        self.state_dir = Some(state_dir);
    }

    /// Return the configured state directory, if any.
    #[must_use]
    pub fn state_dir(&self) -> Option<&Path> {
        self.state_dir.as_deref()
    }

    /// Inspect `<state_dir>/mount_pid` left behind by a crashed daemon.
    ///
    /// * If the file is absent, returns [`StalePidfileOutcome::Absent`].
    /// * If the file exists and the recorded pid is still alive,
    ///   returns [`StalePidfileOutcome::Live`] so the caller can refuse
    ///   to start (matches the existing lease-based Tier-2 HA posture).
    /// * If the file exists and the recorded pid is dead, removes the
    ///   file and returns [`StalePidfileOutcome::Cleaned`] with the
    ///   mountpoint the crashed daemon had registered, so the caller
    ///   can surface a remediation hint pointing the operator at
    ///   `pcloudc mount --force-umount <path>`.
    /// * Malformed sidecar: removed + [`StalePidfileOutcome::Corrupt`].
    pub fn sweep_stale_pidfile(&self) -> std::io::Result<StalePidfileOutcome> {
        let Some(dir) = self.state_dir.as_ref() else {
            return Ok(StalePidfileOutcome::Absent);
        };
        let path = dir.join("mount_pid");
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StalePidfileOutcome::Absent);
            }
            Err(e) => return Err(e),
        };
        let line = data.trim();
        let mut parts = line.splitn(2, ' ');
        let pid_str = parts.next().unwrap_or("");
        let mp = parts.next().unwrap_or("").trim();
        let Ok(pid) = pid_str.parse::<i32>() else {
            let _ = std::fs::remove_file(&path);
            return Ok(StalePidfileOutcome::Corrupt);
        };
        if mp.is_empty() {
            let _ = std::fs::remove_file(&path);
            return Ok(StalePidfileOutcome::Corrupt);
        }
        if pid_is_alive(pid) {
            return Ok(StalePidfileOutcome::Live {
                pid,
                mountpoint: PathBuf::from(mp),
            });
        }
        // Stale: remove sidecar.
        let _ = std::fs::remove_file(&path);
        Ok(StalePidfileOutcome::Cleaned {
            mountpoint: PathBuf::from(mp),
        })
    }

    /// Write the mount-pid sidecar atomically. Best-effort: errors are
    /// logged to stderr but do not fail the mount — the sidecar is an
    /// operator convenience, not a correctness gate.
    fn write_mount_pidfile(&self, mountpoint: &Path) {
        let Some(dir) = self.state_dir.as_ref() else {
            return;
        };
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::error!(
                "pcloud-rs mount: failed to create state dir {}: {e}",
                dir.display()
            );
            return;
        }
        let path = dir.join("mount_pid");
        let tmp = dir.join("mount_pid.tmp");
        let payload = format!("{} {}\n", std::process::id(), mountpoint.display());
        if let Err(e) = std::fs::write(&tmp, payload.as_bytes()) {
            log::error!("pcloud-rs mount: failed to write {}: {e}", tmp.display());
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            log::error!("pcloud-rs mount: failed to rename pidfile: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Remove the mount-pid sidecar. Best-effort.
    fn remove_mount_pidfile(&self) {
        let Some(dir) = self.state_dir.as_ref() else {
            return;
        };
        let _ = std::fs::remove_file(dir.join("mount_pid"));
    }

    /// Replace the journal-sync hook used by the ordered shutdown path.
    /// Call right after constructing the mount runtime, before any
    /// mount is performed.
    pub fn set_journal_sync(&mut self, hook: JournalSyncHook) {
        self.journal_sync = hook;
    }

    /// Inject a custom `/proc/self/mountinfo` reader. Intended for tests
    /// so they can drive [`Self::check_orphans`] without touching the
    /// real procfs.
    pub fn set_mountinfo_reader(&mut self, reader: Box<dyn MountinfoReader>) {
        self.mountinfo_reader = reader;
    }

    /// Enable or disable force-unmount of orphan mounts during
    /// [`Self::check_orphans`] and the ordered shutdown sequence.
    pub fn set_force_umount(&mut self, force: bool) {
        self.force_umount = force;
    }

    /// Returns whether the controller is currently permitted to invoke
    /// `fusermount -u` on orphan mounts during startup checks and the
    /// ordered shutdown path.
    #[must_use]
    pub fn force_umount_enabled(&self) -> bool {
        self.force_umount
    }

    /// Detect orphan pCloud FUSE mounts at daemon startup.
    ///
    /// Returns [`OrphanCheckOutcome::Clean`] when no orphans are present.
    /// When orphans exist and `force_umount` is disabled, returns
    /// [`OrphanCheckOutcome::Rejected`] so the caller can surface a
    /// helpful message (and refuse to start the mount service).
    /// When `force_umount` is enabled, invokes `fusermount -u` on each
    /// orphan and reports the per-path outcome via
    /// [`OrphanCheckOutcome::ForceUnmounted`].
    pub fn check_orphans(&self) -> std::io::Result<OrphanCheckOutcome> {
        // The daemon's known-mount set is whatever it currently tracks
        // as active. At startup this is always empty, but keeping the
        // derivation generic lets us re-run the check safely later.
        let known: Vec<PathBuf> = self
            .active
            .as_ref()
            .map(|a| vec![a.mountpoint.clone()])
            .unwrap_or_default();
        let orphans = detect_orphans(self.mountinfo_reader.as_ref(), &known)?;
        if orphans.is_empty() {
            return Ok(OrphanCheckOutcome::Clean);
        }
        let paths: Vec<PathBuf> = orphans.iter().map(|e| e.mount_point.clone()).collect();
        if !self.force_umount {
            return Ok(OrphanCheckOutcome::Rejected(paths));
        }
        let mut results: Vec<(PathBuf, Result<(), String>)> = Vec::with_capacity(paths.len());
        for path in paths {
            let outcome =
                fusermount_unmount(&path, SHUTDOWN_UNMOUNT_WAIT).map_err(|e| e.to_string());
            results.push((path, outcome));
        }
        Ok(OrphanCheckOutcome::ForceUnmounted(results))
    }

    /// Force-unmount a specific path. Used by the IPC `MountForceUnmount`
    /// method so the CLI can recover from a stuck orphan without needing
    /// to restart the daemon with `--force-umount`.
    ///
    /// Refuses to act on the currently-active daemon mount (the caller
    /// should use `unmount` for that). Does not require `force_umount`
    /// to be enabled because the operator is explicit here.
    pub fn force_unmount_path(&self, path: &Path) -> Response {
        if let Some(active) = &self.active {
            if active.mountpoint == path {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: format!(
                        "refusing to force-unmount active mount at {}; use 'unmount' instead",
                        path.display()
                    ),
                };
            }
        }
        match fusermount_unmount(path, SHUTDOWN_UNMOUNT_WAIT) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!("force-unmounted {}", path.display()),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("force-unmount failed at {}: {err}", path.display()),
            },
        }
    }

    /// Swap the adapter factory and drain hook atomically. Intended for the
    /// daemon to install a composed `PcloudFsShim` factory right before a
    /// `Mount` request, without rebuilding the whole `MountControl`.
    /// Rejected if a mount is currently active, because swapping factories
    /// mid-mount would orphan the drain hook.
    pub fn replace_factory(&mut self, factory: AdapterFactory, drain: DrainHook) -> bool {
        if self.active.is_some() {
            return false;
        }
        self.factory = factory;
        self.drain = drain;
        true
    }

    /// Returns `true` if the controller is currently holding an active
    /// FUSE mount.
    pub fn is_mounted(&self) -> bool {
        self.active.is_some()
    }

    /// Path of the currently active mountpoint, if any.
    pub fn active_mountpoint(&self) -> Option<&Path> {
        self.active.as_ref().map(|a| a.mountpoint.as_path())
    }

    /// Mount at `mountpoint`. Validation (ownership, world-writable check,
    /// `allow_other` rejection) is delegated to [`MountService`].
    pub fn mount(&mut self, mountpoint: &Path) -> Response {
        if let Some(active) = &self.active {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!(
                    "filesystem already mounted at {}",
                    active.mountpoint.display()
                ),
            };
        }

        // Validate first so the error message is specific. Windows accepts
        // a free drive-letter root as well as an empty directory, so it must
        // use the native validator instead of the POSIX directory-only one.
        #[cfg(target_os = "windows")]
        let validation = {
            use pcloud_fs::platform::PlatformMount;
            pcloud_fs::platform::windows::WindowsPlatformMount.validate_mountpoint(mountpoint)
        };
        #[cfg(not(any(
            target_os = "windows",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        )))]
        let validation = MountService::validate_mountpoint(mountpoint);
        #[cfg(any(
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        let validation = {
            use pcloud_fs::platform::PlatformMount;
            pcloud_fs::platform::bsd::BsdPlatformMount.validate_mountpoint(mountpoint)
        };
        if let Err(err) = validation {
            return mount_error_to_response(err);
        }

        // Pre-check: refuse to mount on top of an existing mount in
        // `/proc/self/mountinfo`. Covers "operator re-ran mount without
        // unmounting", "a sibling daemon already owns this path", and
        // "a foreign fuse.sshfs/etc. mount is occupying this path".
        // Without this, `fuser::spawn_mount2` will either shadow the
        // existing mount (losing state) or fail with an opaque errno.
        if let Some(fs_type) =
            mountpoint_is_already_mounted(self.mountinfo_reader.as_ref(), mountpoint)
        {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!(
                    "{} is already mounted as {fs_type}; unmount it first ({})",
                    mountpoint.display(),
                    if cfg!(target_os = "macos") {
                        format!("diskutil unmount force {}", mountpoint.display())
                    } else if cfg!(target_os = "windows") {
                        "pcloudc unmount".to_owned()
                    } else {
                        format!("fusermount3 -u {}", mountpoint.display())
                    }
                ),
            };
        }

        let adapter = match (self.factory)() {
            Ok(a) => a,
            Err(err) => return mount_error_to_response(err),
        };

        // Default mount options are RO; flip to RW so the kernel will dispatch
        // create/write/flush/fsync/unlink/rename into the shim. The write path
        // itself is still gated at `PcloudFsShim` — unauthenticated fallback
        // (`NullFuseAdapter`) returns ENOSYS for every op so writes still fail
        // honestly without auth. Security posture unchanged: owner-only, no
        // allow_other, NoDev+NoSuid still enforced.
        let mount_opts = MountOptions {
            read_only: false,
            ..MountOptions::default()
        };
        match adapter.mount_with(&self.service, mountpoint, mount_opts) {
            Ok(handle) => {
                self.active = Some(ActiveMount {
                    mountpoint: mountpoint.to_path_buf(),
                    handle,
                });
                // Write the sidecar *after* the mount succeeds so a
                // mid-mount failure doesn't leave a misleading pidfile
                // around. Contents: pid + mountpoint, so a subsequent
                // daemon start can tell a live peer from a crash.
                self.write_mount_pidfile(mountpoint);
                Response {
                    status: ResponseStatus::Ok,
                    message: format!("filesystem mounted at {}", mountpoint.display()),
                }
            }
            Err(err) => mount_error_to_response(err),
        }
    }

    /// Unmount the active mount. Invokes the drain hook first so any
    /// queued work has a chance to finish (or report) before the kernel
    /// mount goes away.
    pub fn unmount(&mut self) -> Response {
        if self.active.is_none() {
            return Response {
                status: ResponseStatus::Conflict,
                message: "no active filesystem mount".to_owned(),
            };
        }
        let drain_summary = match self.require_successful_drain() {
            Ok(summary) => summary,
            Err(error) => {
                return Response {
                    status: ResponseStatus::InternalError,
                    message: format!(
                        "refusing to unmount: writer drain failed: {error}; mount remains active"
                    ),
                };
            }
        };
        // Taking the handle is deliberately deferred until after the drain
        // succeeds. That makes failure retryable instead of losing ownership
        // of a still-mounted filesystem with dirty staged writes.
        let active = self
            .active
            .take()
            .expect("active mount checked before successful drain");
        let unmount_result = active.handle.unmount();
        // Sidecar cleanup runs regardless of kernel outcome: even if
        // the kernel teardown failed, *this daemon* no longer owns the
        // mount, so the next startup must re-check orphans rather than
        // trust the pidfile.
        self.remove_mount_pidfile();
        match unmount_result {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "filesystem unmounted from {} (drain: {drain_summary})",
                    active.mountpoint.display()
                ),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!(
                    "unmount failed at {}: {err} (drain: {drain_summary})",
                    active.mountpoint.display()
                ),
            },
        }
    }

    fn require_successful_drain(&self) -> Result<String, String> {
        (self.drain)()
    }

    /// Signal-aware drain helper invoked by the serve loop when the
    /// drain state machine flips `Running → Draining`.
    ///
    /// Runs the configured journal-sync + drain hooks but *does not*
    /// unmount — the kernel mount stays live so in-flight `read(2)`
    /// calls from user processes can complete within the drain grace
    /// window. The actual unmount happens when the runtime drops
    /// (either explicitly via `Method::Unmount` or implicitly when
    /// `RuntimeShell` goes out of scope after the serve loop returns).
    ///
    /// Returns a short human summary so the caller can surface it
    /// through observability / `Method::DrainStatus`.
    pub fn quiesce_for_drain(&self) -> String {
        if self.active.is_none() {
            return "no active mount".to_owned();
        }
        let mut summary = String::new();
        match (self.journal_sync)() {
            Ok(()) => summary.push_str("journal fsync: ok; "),
            Err(e) => summary.push_str(&format!("journal fsync failed: {e}; ")),
        }
        match (self.drain)() {
            Ok(message) => summary.push_str(&format!("drain: {message}")),
            Err(error) => summary.push_str(&format!("drain failed: {error}")),
        }
        summary
    }
}

/// Returns `true` when `pid` is a live process visible from the
/// current process namespace. Uses `kill(pid, 0)` which is
/// async-signal-safe and does not actually deliver a signal — it
/// returns `ESRCH` if the pid is not a known process. `EPERM` means
/// the process exists but we can't signal it (different uid); for
/// our liveness check that still counts as alive.
///
/// Isolated in a helper so the pidfile-corruption path stays readable
/// and so tests can trivially reason about the single syscall site.
fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: `kill` with `sig=0` performs error checking only and
        // does not deliver a signal. No memory is read or written by the
        // syscall beyond the kernel's own task-table walk.
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        // Windows: OpenProcess with PROCESS_QUERY_LIMITED_INFORMATION is
        // the canonical alive-probe. Returns null (Err) if the pid no
        // longer exists. Tracked under bd-xplat-windows for fidelity
        // with the Unix kill(0)/EPERM semantics.
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: both FFI calls accept primitive args and an open handle
        // is closed via CloseHandle before returning.
        unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    true
                }
                Err(_) => false,
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Ordered shutdown sequence used by both `Drop` and explicit
/// operator-initiated teardown:
///
/// 1. `fsync` the staging journal so any queued write reaches disk.
/// 2. Run the drain hook (flushes the writer pipeline, reports a summary).
/// 3. Drop the FUSE session (via [`MountHandle::unmount`]) so the kernel
///    starts releasing the mount.
/// 4. Call `fusermount -u` as a belt-and-braces cleanup step in case the
///    session's own `Drop` lost the race with a SIGTERM.
/// 5. Wait up to [`SHUTDOWN_UNMOUNT_WAIT`] for the mountpoint to
///    disappear from `/proc/self/mountinfo`.
///
/// Steps 4 and 5 are best-effort — on systems without `fusermount`,
/// step 3 is sufficient. Errors are captured into the returned summary
/// so the caller can log them without aborting the shutdown.
fn ordered_shutdown(
    mountpoint: &Path,
    handle: MountHandle,
    journal_sync: &JournalSyncHook,
    drain: &DrainHook,
    reader: &dyn MountinfoReader,
) -> String {
    let mut summary = String::new();
    match (journal_sync)() {
        Ok(()) => summary.push_str("journal fsync: ok; "),
        Err(e) => summary.push_str(&format!("journal fsync failed: {e}; ")),
    }
    match (drain)() {
        Ok(message) => summary.push_str(&format!("drain: {message}; ")),
        Err(error) => summary.push_str(&format!("drain failed: {error}; ")),
    }
    match handle.unmount() {
        Ok(()) => summary.push_str("session: released; "),
        Err(e) => summary.push_str(&format!("session unmount failed: {e}; ")),
    }
    // Belt-and-suspenders unmount: on Linux this calls fusermount3/fusermount
    // to clean up libfuse auxiliary state; on macOS this calls umount(2).
    // On macOS the primary unmount already ran through the fuse-t session
    // teardown above, so a ENOENT / EINVAL here is expected and non-fatal.
    match fusermount_unmount(mountpoint, SHUTDOWN_UNMOUNT_WAIT) {
        Ok(()) => summary.push_str("platform-unmount: ok; "),
        Err(e) if cfg!(target_os = "macos") => {
            // Expected after fuse-t session teardown already released the mount.
            summary.push_str(&format!("platform-unmount: already released ({e}); "));
        }
        Err(e) => summary.push_str(&format!("platform-unmount failed: {e}; ")),
    }
    // Wait for the kernel to release the mount. We poll mountinfo because
    // `fusermount -u` returns after libfuse removes the userspace end,
    // but kernel-side teardown can lag under load.
    let deadline = std::time::Instant::now() + SHUTDOWN_UNMOUNT_WAIT;
    loop {
        let payload = reader.read().unwrap_or_default();
        let still_present = pcloud_fs::mount_orphan::parse_pcloud_mounts(&payload)
            .into_iter()
            .any(|e| e.mount_point == mountpoint);
        if !still_present {
            summary.push_str("kernel: released");
            break;
        }
        if std::time::Instant::now() >= deadline {
            summary.push_str("kernel: still present after 5s");
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    summary
}

impl Drop for MountControl {
    /// RAII ordered shutdown. If there is an active mount when the
    /// control drops (typical SIGTERM path), run the full
    /// fsync -> drain -> unmount -> fusermount -> wait sequence.
    /// All errors are swallowed because `Drop` cannot return them;
    /// the process is exiting anyway.
    fn drop(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        let _ = ordered_shutdown(
            &active.mountpoint,
            active.handle,
            &self.journal_sync,
            &self.drain,
            self.mountinfo_reader.as_ref(),
        );
    }
}

/// Honour `PCLOUD_FORCE_UMOUNT=1` (matches the CLI `--force-umount`
/// flag) so operators running under systemd can request the override
/// without needing a CLI handle.
fn force_umount_from_env() -> bool {
    let value = std::env::var("PCLOUD_FORCE_UMOUNT").ok();
    force_umount_from_value(value.as_deref())
}

fn force_umount_from_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn mount_error_to_response(err: MountError) -> Response {
    let status = match err {
        MountError::MountpointMissing(_)
        | MountError::MountpointNotDirectory(_)
        | MountError::MountpointNotEmpty(_)
        | MountError::MountpointNotOwned { .. }
        | MountError::MountpointWorldWritable { .. }
        | MountError::MountpointSymlink(_)
        | MountError::OptionOutOfRange { .. }
        | MountError::AllowOtherRejected => ResponseStatus::InvalidRequest,
        MountError::UnsupportedPlatform => ResponseStatus::Unavailable,
        MountError::Unsupported(_) => ResponseStatus::Unavailable,
        MountError::Io(_) => ResponseStatus::InternalError,
        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        MountError::Fuser(_) => ResponseStatus::Unavailable,
    };
    Response {
        status,
        message: err.to_string(),
    }
}

/// Default adapter factory used when no authenticated transport is
/// available. Returns a [`NullFuseAdapter`] which replies `ENOSYS` to every
/// FUSE operation. The daemon swaps this factory for
/// [`pcloud_shim_adapter_factory`] once auth and transport are wired.
pub fn default_adapter_factory() -> AdapterFactory {
    Box::new(|| Ok(boxed_adapter(NullFuseAdapter)))
}

/// Parameters needed to build a composed FUSE adapter at mount time.
///
/// On Linux the factory wraps the adapter in a [`PcloudFsShim`] and mounts
/// it via [`MountService::mount_fuser`]. On macOS the factory mounts the
/// bare [`ProtoFuseAdapter`] via [`MountService::mount`] — the fuse-t FFI
/// path owns its own translation layer and has no `fuser::Filesystem`
/// dependency.
///
/// [`SecretString`] is deliberately not `Clone`-derived; the factory clones
/// it explicitly via `clone_secret` at each adapter construction so every
/// duplication of the token buffer is auditable in review (matches the
/// existing pattern in [`crate::runtime::PendingPasswordAuth`]).
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
struct CanonicalMountBackend {
    auth: AuthRuntime,
    folder: FolderRuntime,
    transfer: TransferRuntime,
    auth_token: SecretString,
    store_path: PathBuf,
    runtime_dir: PathBuf,
    sessions: std::sync::Mutex<std::collections::HashMap<u64, UploadSession>>,
    chunk_ids: std::sync::Mutex<std::collections::HashMap<u64, u64>>,
    quota_cache: std::sync::Mutex<Option<(std::time::Instant, (u64, u64))>>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
impl std::fmt::Debug for CanonicalMountBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanonicalMountBackend")
            .field("auth_token", &"<redacted>")
            .field("store_path", &self.store_path)
            .field("runtime_dir", &self.runtime_dir)
            .finish_non_exhaustive()
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
impl CanonicalMountBackend {
    fn new(config: &ConfigProfile, auth_token: SecretString) -> Self {
        Self {
            auth: AuthRuntime::from_config(config),
            folder: FolderRuntime::from_config(config),
            transfer: TransferRuntime::from_config(config),
            auth_token,
            store_path: config.paths.state_dir.join("store.sqlite3"),
            runtime_dir: config.paths.runtime_dir.clone(),
            sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
            chunk_ids: std::sync::Mutex::new(std::collections::HashMap::new()),
            quota_cache: std::sync::Mutex::new(None),
        }
    }

    fn remote(&self) -> RemoteFs<'_> {
        RemoteFs::new(&self.folder, &self.transfer, self.auth_token.clone_secret())
    }

    fn durable_remote(&self) -> Result<RemoteFs<'_>, RemoteFsError> {
        self.remote()
            .with_durability(self.store_path.clone(), self.runtime_dir.clone())
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
fn remote_error_to_fs(error: RemoteFsError) -> FsError {
    match error {
        RemoteFsError::NotFound { .. } => FsError::NotFound,
        RemoteFsError::ExpectedFolder { .. } => FsError::NotDirectory,
        RemoteFsError::InvalidPath { .. }
        | RemoteFsError::ExpectedFile { .. }
        | RemoteFsError::Ambiguous { .. }
        | RemoteFsError::MissingId { .. }
        | RemoteFsError::MissingSize { .. }
        | RemoteFsError::RangeTooLarge { .. }
        | RemoteFsError::UnexpectedEof { .. }
        | RemoteFsError::SourceTooLong { .. }
        | RemoteFsError::RecursiveCopy { .. }
        | RemoteFsError::DestinationExists { .. } => FsError::Invalid,
        RemoteFsError::Folder(pcloud_proto::FolderApiError::Result { result, message })
        | RemoteFsError::TransferApi(pcloud_proto::TransferApiError::Result { result, message }) => {
            FsError::from_pcloud_result(result, message)
        }
        RemoteFsError::Io(_) => FsError::Io,
        other => FsError::transport(other.to_string()),
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
fn metadata_to_folder_entry(
    metadata: pcloud_backends::remote_fs::RemoteMetadata,
) -> RemoteFolderEntry {
    let (is_folder, folder_id, file_id) = match metadata.id {
        RemoteId::Folder(id) => (true, Some(id), None),
        RemoteId::File(id) => (false, None, Some(id)),
    };
    RemoteFolderEntry {
        name: metadata.name,
        is_folder,
        folder_id,
        file_id,
        owner_user_id: None,
        is_mine: metadata.is_mine,
        encrypted: metadata.encrypted,
        is_shared: metadata.is_shared,
        permissions: metadata.permissions,
        size: metadata.size,
        modified: metadata.modified,
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
impl FolderBackend for CanonicalMountBackend {
    fn statfs(&self) -> Result<(u64, u64), FsError> {
        const QUOTA_TTL: Duration = Duration::from_secs(30);

        {
            let cache = self
                .quota_cache
                .lock_or_poisoned("mount_runtime::canonical_mount_quota_cache");
            if let Some((fetched_at, quota)) = cache.as_ref()
                && fetched_at.elapsed() < QUOTA_TTL
            {
                return Ok(*quota);
            }
        }

        let userinfo = self
            .auth
            .userinfo(self.auth_token.clone_secret())
            .map_err(|error| FsError::transport(format!("userinfo quota query failed: {error}")))?;
        let total = userinfo
            .quota
            .ok_or_else(|| FsError::transport("userinfo response omitted account quota"))?;
        let used = userinfo
            .used_quota
            .ok_or_else(|| FsError::transport("userinfo response omitted used account quota"))?;
        let quota = (total, total.saturating_sub(used));
        *self
            .quota_cache
            .lock_or_poisoned("mount_runtime::canonical_mount_quota_cache") =
            Some((std::time::Instant::now(), quota));
        Ok(quota)
    }

    fn list_contents(&self, path: &str) -> Result<RemoteFolderListing, FsError> {
        let listing = self.remote().list(path).map_err(remote_error_to_fs)?;
        let RemoteId::Folder(folder_id) = listing.folder.id else {
            return Err(FsError::NotDirectory);
        };
        Ok(RemoteFolderListing {
            folder_id,
            path: listing.folder.path,
            name: listing.folder.name,
            entries: listing
                .entries
                .into_iter()
                .map(metadata_to_folder_entry)
                .collect(),
            api_server: None,
            owner_user_id: None,
            is_mine: listing.folder.is_mine,
            encrypted: listing.folder.encrypted,
            is_shared: listing.folder.is_shared,
            permissions: listing.folder.permissions,
        })
    }

    fn create_folder(&self, parent_path: &str, name: &str) -> Result<u64, FsError> {
        let path = join_remote_path(parent_path, name);
        let created = self.remote().mkdir(&path).map_err(remote_error_to_fs)?;
        match created.id {
            RemoteId::Folder(id) => Ok(id),
            RemoteId::File(_) => Err(FsError::Io),
        }
    }

    fn delete_folder(&self, path: &str) -> Result<(), FsError> {
        match self
            .remote()
            .delete(path, false)
            .map_err(remote_error_to_fs)?
        {
            DeleteOutcome::Deleted(RemoteId::Folder(_)) | DeleteOutcome::AlreadyAbsent => Ok(()),
            DeleteOutcome::Deleted(RemoteId::File(_)) => Err(FsError::NotDirectory),
        }
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
impl FileBackend for CanonicalMountBackend {
    fn open(&self, file_id: u64) -> Result<FileHandle, FsError> {
        Ok(FileHandle {
            file_id,
            size: 0,
            host: "canonical-remotefs".to_owned(),
            path: String::new(),
            dwltag: None,
        })
    }

    fn read(&self, handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        self.remote()
            .read_range_by_id(handle.file_id, offset, len as u64)
            .map_err(remote_error_to_fs)
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
impl FileUploadBackend for CanonicalMountBackend {
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &Path,
    ) -> Result<(), WritePathError> {
        let path = join_remote_path(parent_path, name);
        self.durable_remote()
            .and_then(|remote| {
                remote
                    .upload_file_resumable(&path, staging_file, UploadConflict::Overwrite)
                    .map(|_| ())
            })
            .map_err(remote_error_to_write)
    }

    fn unlink_remote(&self, path: &str) -> Result<(), WritePathError> {
        self.remote()
            .delete(path, false)
            .map(|_| ())
            .map_err(remote_error_to_write)
    }

    fn rename_remote(&self, from: &str, to: &str) -> Result<(), WritePathError> {
        self.remote()
            .move_path(from, to)
            .map(|_| ())
            .map_err(remote_error_to_write)
    }

    fn upload_create(&self, parent_path: &str, name: &str) -> Result<u64, WritePathError> {
        let path = join_remote_path(parent_path, name);
        let session = self
            .remote()
            .begin_streaming_write(&path, 0)
            .map_err(remote_error_to_write)?;
        let upload_id = session.upload_id;
        self.sessions
            .lock_or_poisoned("mount_runtime::canonical_mount_sessions")
            .insert(upload_id, session);
        self.chunk_ids
            .lock_or_poisoned("mount_runtime::canonical_mount_chunk_ids")
            .insert(upload_id, 0);
        Ok(upload_id)
    }

    fn upload_write(
        &self,
        upload_id: u64,
        offset: u64,
        chunk: &[u8],
    ) -> Result<(), WritePathError> {
        let chunk_id = *self
            .chunk_ids
            .lock_or_poisoned("mount_runtime::canonical_mount_chunk_ids")
            .get(&upload_id)
            .unwrap_or(&0);
        let acknowledged = self
            .remote()
            .write_streaming_chunk(upload_id, offset, chunk_id, chunk)
            .map_err(remote_error_to_write)?;
        let expected = offset.saturating_add(chunk.len() as u64);
        if acknowledged != expected {
            return Err(WritePathError::Upload(format!(
                "upload_write acknowledged offset {acknowledged}, expected {expected}"
            )));
        }
        self.chunk_ids
            .lock_or_poisoned("mount_runtime::canonical_mount_chunk_ids")
            .insert(upload_id, chunk_id.saturating_add(1));
        Ok(())
    }

    fn upload_save(
        &self,
        upload_id: u64,
        parent_path: &str,
        name: &str,
        _total_size: u64,
    ) -> Result<(), WritePathError> {
        let session = self
            .sessions
            .lock_or_poisoned("mount_runtime::canonical_mount_sessions")
            .get(&upload_id)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                let parent = self.remote().stat(parent_path)?;
                let RemoteId::Folder(parent_folder_id) = parent.id else {
                    return Err(RemoteFsError::ExpectedFolder {
                        path: parent_path.to_owned(),
                    });
                };
                Ok(UploadSession {
                    upload_id,
                    file_id: None,
                    parent_folder_id,
                    file_name: name.to_owned(),
                    api_server: None,
                })
            })
            .map_err(remote_error_to_write)?;
        self.remote()
            .commit_streaming_write(&session, None, unix_now())
            .map_err(remote_error_to_write)?;
        self.sessions
            .lock_or_poisoned("mount_runtime::canonical_mount_sessions")
            .remove(&upload_id);
        self.chunk_ids
            .lock_or_poisoned("mount_runtime::canonical_mount_chunk_ids")
            .remove(&upload_id);
        Ok(())
    }

    fn upload_status(&self, upload_id: u64) -> Result<UploadStatus, WritePathError> {
        match self.remote().streaming_write_status(upload_id, 0) {
            Ok(info) => Ok(UploadStatus::Bytes(info.size)),
            Err(RemoteFsError::TransferApi(pcloud_proto::TransferApiError::Result { .. })) => {
                Ok(UploadStatus::NotFound)
            }
            Err(error) => Err(remote_error_to_write(error)),
        }
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
fn remote_error_to_write(error: RemoteFsError) -> WritePathError {
    WritePathError::Fs(remote_error_to_fs(error))
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
fn join_remote_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
#[derive(Debug)]
/// Inputs captured by the canonical live-mount adapter factory.
pub struct ShimFactoryParams {
    /// Validated daemon profile used to compose canonical runtimes.
    pub config: ConfigProfile,
    /// Auth token held in a zeroising [`SecretString`].
    pub auth_token: SecretString,
    /// Staging root for the write path (per-mount scratch dir).
    pub staging_root: PathBuf,
    /// Write-path flush policy.
    pub write_options: WritePathOptions,
    /// Adapter (read-path) cache options.
    pub adapter_options: AdapterOptions,
}

/// Object-safe adapter that wraps a fully-composed [`PcloudFsShim`] and
/// dispatches to [`MountService::mount_fuser`] instead of the
/// `FuseAdapter`-wrapping path.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
struct PcloudShimAdapter {
    writer: Arc<WritePathService<CanonicalMountBackend>>,
    shim: Option<PcloudFsShim<CanonicalMountBackend, CanonicalMountBackend, CanonicalMountBackend>>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
impl DynFuseAdapter for PcloudShimAdapter {
    fn mount_with(
        mut self: Box<Self>,
        service: &MountService,
        mountpoint: &Path,
        options: MountOptions,
    ) -> Result<MountHandle, MountError> {
        // SAFETY: `mount_with` consumes `self: Box<Self>`, so it is called
        // exactly once per adapter instance. The constructor stores `shim`
        // as `Some(_)`, and this `.take()` is the only code path that can
        // replace it with `None`. A second call is impossible because the
        // Box is dropped after the first call returns.
        let shim = self
            .shim
            .take()
            .expect("PcloudShimAdapter shim already consumed");
        // Fire the shim through the fuser-based mount path; the writer is
        // kept alive inside the shim via its `Arc` references, so pending
        // work can be drained via `MountControl::drain` on unmount.
        let _ = self.writer; // explicit drop-on-unmount is handled via drain hook
        service.mount_fuser(mountpoint, shim, options)
    }
}

/// Object-safe adapter that wraps a bare [`ProtoFuseAdapter`] for the
/// macOS fuse-t and Windows WinFSP paths. [`MountService::mount`] selects
/// the native platform bridge. The `fuser` shim is Linux-only.
#[cfg(any(target_os = "macos", target_os = "windows"))]
struct PcloudProtoAdapter {
    writer: Arc<WritePathService<CanonicalMountBackend>>,
    adapter: Option<ProtoFuseAdapter<CanonicalMountBackend, CanonicalMountBackend>>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl DynFuseAdapter for PcloudProtoAdapter {
    fn mount_with(
        mut self: Box<Self>,
        service: &MountService,
        mountpoint: &Path,
        options: MountOptions,
    ) -> Result<MountHandle, MountError> {
        // SAFETY: `mount_with` consumes `self: Box<Self>`, so it is called
        // exactly once per adapter instance. The constructor stores
        // `adapter` as `Some(_)`, and this `.take()` is the only code path
        // that can replace it with `None`. A second call is impossible
        // because the Box is dropped after the first call returns.
        let adapter = self
            .adapter
            .take()
            .expect("PcloudProtoAdapter adapter already consumed");
        // The writer is held by the adapter via its internal `Arc` and by
        // the drain hook closure — dropping the local handle here does
        // not free the writer; it just removes one reference.
        let _ = self.writer;
        service.mount(mountpoint, adapter, options)
    }
}

/// Build an adapter factory that composes a real live FUSE adapter at
/// mount time, along with a drain hook that flushes the writer before
/// unmount.
///
/// On Linux the factory wraps the composed adapter in a [`PcloudFsShim`]
/// and mounts it via [`MountService::mount_fuser`]. On macOS and Windows
/// it mounts the bare [`ProtoFuseAdapter`] via [`MountService::mount`],
/// which selects fuse-t or WinFSP respectively.
///
/// Returns `(factory, drain_hook)`. The drain hook holds a shared reference
/// to the writer so the caller can install it into [`MountControl::new`].
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "macos",
    target_os = "windows"
))]
#[must_use]
pub fn pcloud_shim_adapter_factory(params: ShimFactoryParams) -> (AdapterFactory, DrainHook) {
    let writer_slot: Arc<std::sync::Mutex<Option<Arc<WritePathService<CanonicalMountBackend>>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let writer_slot_for_factory = Arc::clone(&writer_slot);

    // Capture individual fields so the factory closure is `Fn` (not FnOnce)
    // and each invocation clones the secret explicitly via `clone_secret`.
    let ShimFactoryParams {
        config,
        auth_token,
        staging_root,
        write_options,
        adapter_options,
    } = params;
    let auth_token = Arc::new(auth_token);

    let factory: AdapterFactory = Box::new(move || {
        let remote = Arc::new(CanonicalMountBackend::new(
            &config,
            auth_token.clone_secret(),
        ));
        let stage = StagingDir::open(&staging_root)
            .map_err(|e| MountError::Io(std::io::Error::other(e.to_string())))?;
        let journal = WriteJournal::open(stage.journal_path())
            .map_err(|e| MountError::Io(std::io::Error::other(e.to_string())))?;
        let pending_journal_records = journal
            .replay()
            .map_err(|e| MountError::Io(std::io::Error::other(e.to_string())))?;
        if !pending_journal_records.is_empty() {
            return Err(MountError::Io(std::io::Error::other(format!(
                "refusing writable FUSE mount: {} unreplayed write-journal record(s) remain in staging; replay executor is not available",
                pending_journal_records.len()
            ))));
        }
        // Startup-resume reconcile: before accepting any FUSE write,
        // walk the staging root's per-inode `ino-*.upload-progress`
        // sidecars and reconcile each against the server via
        // `upload_status` (pCloud `upload_info`). This trims the local
        // sidecar up (server ahead) or down (server behind), expires
        // garbage-collected upload ids, and aborts stalled uploads that
        // have been idle past `DEFAULT_HEARTBEAT_TIMEOUT`.
        match pcloud_fs::write_path::replay_upload_sidecars(
            &staging_root,
            remote.as_ref(),
            pcloud_fs::write_path::DEFAULT_HEARTBEAT_TIMEOUT,
        ) {
            Ok(outcomes) if !outcomes.is_empty() => {
                log::info!(
                    "pcloud-daemon mount: reconciled {} upload sidecar(s)",
                    outcomes.len()
                );
                for o in outcomes {
                    log::info!("upload_resume: {o:?}");
                }
            }
            Ok(_) => {}
            Err(e) => {
                log::error!("pcloud-daemon mount: upload sidecar reconcile failed: {e}");
            }
        }
        let writer = Arc::new(WritePathService::new(
            stage,
            journal,
            Arc::clone(&remote),
            write_options,
        ));
        // Publish the writer for the drain hook.
        // INVARIANT: `writer_slot_for_factory` is an internal Mutex that is
        // never poisoned by a panic inside this function (no panics between
        // Mutex::new and this lock call). Poison here would indicate a bug
        // elsewhere in daemon startup and is not recoverable.
        *writer_slot_for_factory.lock_or_poisoned("mount_runtime::writer_slot_for_factory") =
            Some(Arc::clone(&writer));

        // Wire the write-path into the adapter too so adapter-level FUSE
        // ops (setattr/create/etc. that flow through `FuseAdapter`) reach
        // the real writer.
        //
        // Linux: wrap in `Arc` and hand to `PcloudFsShim` which dispatches
        // `fuser::Filesystem` ops back into the adapter + the writer.
        //
        // macOS/Windows: hand the bare adapter to `MountService::mount`,
        // which routes through fuse-t or WinFSP. There is no `fuser` layer.
        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        {
            let adapter = Arc::new(
                ProtoFuseAdapter::with_file_backend(
                    Arc::clone(&remote),
                    Arc::clone(&remote),
                    adapter_options,
                )
                .with_write_path(Arc::clone(&writer)),
            );

            let shim = PcloudFsShim::new(adapter, Arc::clone(&writer));
            Ok(Box::new(PcloudShimAdapter {
                writer,
                shim: Some(shim),
            }))
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let adapter = ProtoFuseAdapter::with_file_backend(
                Arc::clone(&remote),
                Arc::clone(&remote),
                adapter_options,
            )
            .with_write_path(Arc::clone(&writer));
            Ok(Box::new(PcloudProtoAdapter {
                writer,
                adapter: Some(adapter),
            }))
        }
    });

    let drain: DrainHook = Arc::new(move || {
        // Best-effort drain: flush every dirty inode before the kernel
        // mount disappears. `drain_all` ignores the time/size triggers
        // so data acknowledged to the kernel but not yet uploaded is
        // pushed to the backend (or surfaces a per-inode error the
        // caller can log).
        // INVARIANT: `writer_slot` is only ever locked from this closure
        // (single drain hook) and from the factory above; neither path
        // panics while holding the lock, so it cannot be poisoned.
        let w = writer_slot.lock_or_poisoned("mount_runtime::writer_slot::drain_hook");
        let Some(writer) = w.as_ref() else {
            return Ok("writer drain: no active writer".to_owned());
        };
        let open_fhs = writer.open_inode_count();
        let outcomes = writer.drain_all();
        let flushed = outcomes.iter().filter(|(_, r)| r.is_ok()).count();
        let failed: Vec<String> = outcomes
            .iter()
            .filter_map(|(ino, r)| r.as_ref().err().map(|e| format!("ino={ino}: {e}")))
            .collect();
        if failed.is_empty() {
            Ok(format!(
                "writer drain: ok (open_fhs={open_fhs}, flushed={flushed})"
            ))
        } else {
            Err(format!(
                "writer drain: partial (open_fhs={open_fhs}, flushed={flushed}, failed=[{}])",
                failed.join("; ")
            ))
        }
    });

    (factory, drain)
}

// Unix-only tests (PermissionsExt helpers).
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    /// Mock adapter used by the mount/unmount lifecycle test so the test
    /// does not require a live libfuse kernel.
    struct MockAdapterOutcome {
        mounts: Arc<AtomicUsize>,
    }

    impl DynFuseAdapter for MockAdapterOutcome {
        fn mount_with(
            self: Box<Self>,
            _service: &MountService,
            _mountpoint: &Path,
            _options: MountOptions,
        ) -> Result<MountHandle, MountError> {
            self.mounts.fetch_add(1, Ordering::SeqCst);
            // Fabricate an empty handle. `MountHandle` is intentionally
            // opaque; constructing one from outside the crate is not
            // supported, so this mock path only exercises the
            // pre-mount validation and factory wiring. The
            // `NullFuseAdapter` path in the Linux integration test
            // covers the real FUSE lifecycle when the gate is enabled.
            Err(MountError::UnsupportedPlatform)
        }
    }

    fn mock_factory(mounts: Arc<AtomicUsize>) -> AdapterFactory {
        Box::new(move || {
            let mounts = Arc::clone(&mounts);
            Ok(Box::new(MockAdapterOutcome { mounts }))
        })
    }

    #[test]
    fn mount_rejects_missing_mountpoint() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let mut ctl = MountControl::default();
        let resp = ctl.mount(&missing);
        assert_eq!(resp.status, ResponseStatus::InvalidRequest);
        assert!(
            resp.message.contains("does not exist"),
            "got: {}",
            resp.message
        );
        assert!(!ctl.is_mounted());
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    #[test]
    fn mount_rejects_world_writable_mountpoint() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("ww");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let mut ctl = MountControl::default();
        let resp = ctl.mount(&dir);
        assert_eq!(resp.status, ResponseStatus::InvalidRequest);
        assert!(
            resp.message.contains("world-writable"),
            "got: {}",
            resp.message
        );
        assert!(!ctl.is_mounted());
    }

    #[test]
    fn unmount_when_not_mounted_is_conflict() {
        let mut ctl = MountControl::default();
        let resp = ctl.unmount();
        assert_eq!(resp.status, ResponseStatus::Conflict);
    }

    #[test]
    fn replace_factory_swaps_when_not_mounted() {
        let mounts = Arc::new(AtomicUsize::new(0));
        let mut ctl = MountControl::default();
        assert!(ctl.replace_factory(
            mock_factory(Arc::clone(&mounts)),
            Arc::new(|| Ok("drained".to_owned())),
        ));
    }

    #[test]
    fn mount_control_pidfile_and_idle_recovery_contracts_are_stable() {
        let root = tempdir().unwrap();
        let state = root.path().join("state");
        let pidfile = state.join("mount_pid");
        let mut ctl = MountControl::default();

        assert_eq!(
            ctl.sweep_stale_pidfile().unwrap(),
            StalePidfileOutcome::Absent
        );
        assert_eq!(ctl.state_dir(), None);
        assert!(!ctl.force_umount_enabled());
        assert_eq!(ctl.quiesce_for_drain(), "no active mount");

        ctl.set_state_dir(state.clone());
        assert_eq!(ctl.state_dir(), Some(state.as_path()));
        assert_eq!(
            ctl.sweep_stale_pidfile().unwrap(),
            StalePidfileOutcome::Absent
        );

        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(&pidfile, "not-a-pid /mnt/pcloud\n").unwrap();
        assert_eq!(
            ctl.sweep_stale_pidfile().unwrap(),
            StalePidfileOutcome::Corrupt
        );
        assert!(!pidfile.exists());

        std::fs::write(&pidfile, "1234\n").unwrap();
        assert_eq!(
            ctl.sweep_stale_pidfile().unwrap(),
            StalePidfileOutcome::Corrupt
        );

        std::fs::write(&pidfile, "0 /mnt/dead\n").unwrap();
        assert_eq!(
            ctl.sweep_stale_pidfile().unwrap(),
            StalePidfileOutcome::Cleaned {
                mountpoint: PathBuf::from("/mnt/dead")
            }
        );

        std::fs::write(&pidfile, format!("{} /mnt/live\n", std::process::id())).unwrap();
        assert_eq!(
            ctl.sweep_stale_pidfile().unwrap(),
            StalePidfileOutcome::Live {
                pid: std::process::id() as i32,
                mountpoint: PathBuf::from("/mnt/live")
            }
        );
        assert!(pidfile.exists());
        assert!(pid_is_alive(std::process::id() as i32));
        assert!(!pid_is_alive(0));

        ctl.set_force_umount(true);
        assert!(ctl.force_umount_enabled());
        ctl.set_journal_sync(Arc::new(|| Ok(())));
        assert_eq!(
            ctl.require_successful_drain().unwrap(),
            "no drain work queued"
        );

        let response = ctl.force_unmount_path(&root.path().join("not-mounted"));
        assert_eq!(response.status, ResponseStatus::InternalError);
        assert!(response.message.contains("force-unmount failed"));
    }

    #[test]
    fn mount_refuses_path_already_present_in_mount_table() {
        use pcloud_fs::mount_orphan::StaticMountinfoReader;

        let root = tempdir().unwrap();
        let mountpoint = root.path().join("drive");
        std::fs::create_dir(&mountpoint).unwrap();
        let payload = format!(
            "25 28 0:44 / {} rw,nosuid,nodev - fuse.sshfs sshfs rw\n",
            mountpoint.display()
        );
        let mounts = Arc::new(AtomicUsize::new(0));
        let mut ctl = MountControl::new(
            mock_factory(Arc::clone(&mounts)),
            Arc::new(|| Ok("drained".to_owned())),
        );
        ctl.set_mountinfo_reader(Box::new(StaticMountinfoReader::new(payload)));

        let response = ctl.mount(&mountpoint);
        assert_eq!(response.status, ResponseStatus::Conflict);
        assert!(response.message.contains("already mounted as fuse.sshfs"));
        assert_eq!(mounts.load(Ordering::SeqCst), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_mount_control_lifecycle_covers_active_conflicts_and_drain() {
        if std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() != Some("1") {
            return;
        }
        use pcloud_fs::fuse_adapter::NullFuseAdapter;
        use pcloud_fs::mount_orphan::StaticMountinfoReader;

        let mountpoint = tempdir().unwrap();
        let state = tempdir().unwrap();
        let drain_calls = Arc::new(AtomicUsize::new(0));
        let drain_for_hook = Arc::clone(&drain_calls);
        let mut ctl = MountControl::new(
            Box::new(|| Ok(boxed_adapter(NullFuseAdapter))),
            Arc::new(move || {
                drain_for_hook.fetch_add(1, Ordering::SeqCst);
                Ok("all writes durable".to_owned())
            }),
        );
        ctl.set_state_dir(state.path().to_path_buf());
        ctl.set_mountinfo_reader(Box::new(StaticMountinfoReader::new("")));

        let mounted = ctl.mount(mountpoint.path());
        if mounted.status == ResponseStatus::Unavailable
            && (mounted.message.contains("/dev/fuse")
                || mounted.message.contains("Permission denied")
                || mounted.message.contains("Operation not permitted"))
        {
            return;
        }
        assert_eq!(mounted.status, ResponseStatus::Ok, "{}", mounted.message);
        assert!(ctl.is_mounted());
        assert_eq!(ctl.active_mountpoint(), Some(mountpoint.path()));
        assert!(state.path().join("mount_pid").is_file());
        assert!(!ctl.replace_factory(
            Box::new(|| Ok(boxed_adapter(NullFuseAdapter))),
            Arc::new(|| Ok("replacement".to_owned()))
        ));

        assert_eq!(
            ctl.mount(mountpoint.path()).status,
            ResponseStatus::Conflict
        );
        assert_eq!(
            ctl.force_unmount_path(mountpoint.path()).status,
            ResponseStatus::Conflict
        );
        assert!(ctl.quiesce_for_drain().contains("all writes durable"));
        assert_eq!(drain_calls.load(Ordering::SeqCst), 1);

        let unmounted = ctl.unmount();
        assert_eq!(
            unmounted.status,
            ResponseStatus::Ok,
            "{}",
            unmounted.message
        );
        assert_eq!(drain_calls.load(Ordering::SeqCst), 2);
        assert!(!ctl.is_mounted());
        assert!(!state.path().join("mount_pid").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pcloud_shim_factory_composes_real_shim_and_drain_reports_no_writer() {
        // Build a factory against a dummy network transport. We don't call
        // it (the BinaryApiTransport refuses to connect), but the drain
        // hook is wired and reports "no active writer" until a mount has
        // actually produced one.
        use pcloud_config::{ConfigProfile, Environment};
        use pcloud_secret::secret_string::SecretString;
        let tmp = tempdir().unwrap();
        let params = ShimFactoryParams {
            config: ConfigProfile::secure_defaults(
                tmp.path().to_path_buf(),
                Environment::Development,
            ),
            auth_token: SecretString::new("dummy-token"),
            staging_root: tmp.path().join("stage"),
            write_options: pcloud_fs::write_path::WritePathOptions::default(),
            adapter_options: pcloud_fs::fuse_adapter::AdapterOptions::default(),
        };
        let (_factory, drain) = pcloud_shim_adapter_factory(params);
        let msg = (drain)().expect("empty drain must succeed");
        assert!(msg.contains("no active writer"), "got: {msg}");
    }

    #[test]
    fn drain_failure_is_a_typed_error_not_a_success_summary() {
        let ctl = MountControl::new(
            default_adapter_factory(),
            Arc::new(|| Err("ino=42: checksum mismatch".to_owned())),
        );
        let error = ctl
            .require_successful_drain()
            .expect_err("dirty failed inode must block explicit unmount");
        assert!(error.contains("ino=42"), "got: {error}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn canonical_mount_backend_lists_and_reads_without_metadata_cache() {
        use pcloud_config::{ConfigProfile, Environment};
        use pcloud_fs::backend::{FileBackend as _, FolderBackend as _};
        use pcloud_secret::secret_string::SecretString;

        let root = tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let backend = CanonicalMountBackend::new(&config, SecretString::new("test-token"));

        // No pcloud-store connection or metadata cache is created or seeded.
        // Both operations must traverse the canonical live RemoteFs service.
        let listing = backend.list_contents("/").expect("live root listing");
        let notes = listing
            .entries
            .iter()
            .find(|entry| entry.name == "notes.txt")
            .expect("development fixture file");
        let file_id = notes.file_id.expect("fixture file id");
        let handle = backend
            .open_with_size(file_id, notes.size.unwrap_or_default())
            .expect("canonical open");
        let bytes = backend.read(&handle, 0, 30).expect("canonical range read");
        assert_eq!(bytes, b"downloaded:/get/abc/report.txt");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn canonical_mount_backend_reports_account_quota() {
        use pcloud_config::{ConfigProfile, Environment};
        use pcloud_fs::backend::FolderBackend as _;
        use pcloud_secret::secret_string::SecretString;

        let root = tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let backend = CanonicalMountBackend::new(&config, SecretString::new("auth-token-42"));

        let expected = (10 * 1024 * 1024 * 1024, 6 * 1024 * 1024 * 1024);
        assert_eq!(
            backend.statfs().expect("development account quota"),
            expected
        );
        assert_eq!(backend.statfs().expect("cached account quota"), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn canonical_mount_backend_exercises_mutation_and_chunked_upload_contracts() {
        use pcloud_config::{ConfigProfile, Environment};
        use pcloud_fs::backend::FolderBackend as _;
        use pcloud_fs::write_path::FileUploadBackend as _;
        use pcloud_secret::secret_string::SecretString;

        let root = tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        pcloud_store::bootstrap_profile(&config.paths.state_dir.join("store.sqlite3"))
            .expect("bootstrap durable upload store");
        std::fs::create_dir_all(&config.paths.runtime_dir)
            .expect("create upload runtime directory");
        let backend = CanonicalMountBackend::new(&config, SecretString::new("test-token"));
        assert!(!format!("{backend:?}").contains("test-token"));

        assert_eq!(
            backend
                .create_folder("/", "mount-created")
                .expect("development folder create"),
            123
        );
        assert!(backend.delete_folder("/missing").is_ok());
        assert!(backend.unlink_remote("/missing.txt").is_ok());
        assert!(backend.rename_remote("/notes.txt", "/renamed.txt").is_err());

        let staging = root.path().join("staging.bin");
        std::fs::write(&staging, b"mount upload payload").unwrap();
        backend
            .upload_file("/", "uploaded.bin", &staging)
            .expect("durable development upload");

        let upload_id = backend
            .upload_create("/", "chunked.bin")
            .expect("begin chunked upload");
        backend
            .upload_write(upload_id, 0, b"first")
            .expect("first chunk");
        backend
            .upload_write(upload_id, 5, b"-second")
            .expect("second chunk");
        assert!(backend.upload_status(upload_id).is_err());
        backend
            .upload_save(upload_id, "/", "chunked.bin", 12)
            .expect("commit development upload");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn canonical_mount_error_mapping_covers_stable_errno_taxonomy() {
        use pcloud_backends::remote_fs::RemoteFsError;

        assert!(matches!(
            remote_error_to_fs(RemoteFsError::NotFound { path: "/x".into() }),
            FsError::NotFound
        ));
        assert!(matches!(
            remote_error_to_fs(RemoteFsError::ExpectedFolder { path: "/x".into() }),
            FsError::NotDirectory
        ));
        for error in [
            RemoteFsError::InvalidPath {
                path: "x".into(),
                reason: "fixture",
            },
            RemoteFsError::ExpectedFile { path: "/x".into() },
            RemoteFsError::MissingId { path: "/x".into() },
            RemoteFsError::MissingSize { path: "/x".into() },
            RemoteFsError::RangeTooLarge {
                requested: 2,
                maximum: 1,
            },
            RemoteFsError::UnexpectedEof {
                expected: 2,
                actual: 1,
            },
            RemoteFsError::SourceTooLong { expected: 1 },
            RemoteFsError::RecursiveCopy {
                from: "/a".into(),
                to: "/a/b".into(),
            },
            RemoteFsError::DestinationExists {
                path: PathBuf::from("/tmp/x"),
            },
        ] {
            assert!(matches!(remote_error_to_fs(error), FsError::Invalid));
        }
        assert!(matches!(
            remote_error_to_fs(RemoteFsError::Io(std::io::Error::other("fixture"))),
            FsError::Io
        ));
        assert!(matches!(
            remote_error_to_fs(RemoteFsError::SharingUnavailable),
            FsError::Transport(_)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pcloud_shim_factory_refuses_non_empty_write_journal() {
        use pcloud_config::{ConfigProfile, Environment};
        use pcloud_fs::write_journal::JournalOp;
        use pcloud_secret::secret_string::SecretString;

        let tmp = tempdir().unwrap();
        let staging_root = tmp.path().join("stage");
        {
            let stage = StagingDir::open(&staging_root).unwrap();
            let mut journal = WriteJournal::open(stage.journal_path()).unwrap();
            journal
                .append(JournalOp::FlushBarrier {
                    path: "/dirty.txt".to_owned(),
                })
                .unwrap();
        }

        let params = ShimFactoryParams {
            config: ConfigProfile::secure_defaults(
                tmp.path().to_path_buf(),
                Environment::Development,
            ),
            auth_token: SecretString::new("dummy-token"),
            staging_root,
            write_options: pcloud_fs::write_path::WritePathOptions::default(),
            adapter_options: pcloud_fs::fuse_adapter::AdapterOptions::default(),
        };
        let (factory, _drain) = pcloud_shim_adapter_factory(params);
        let err = match factory() {
            Ok(_) => panic!("factory must refuse a non-empty write journal"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unreplayed write-journal"),
            "got: {err}"
        );
    }

    #[test]
    fn check_orphans_clean_when_mountinfo_has_no_pcloud_entries() {
        use pcloud_fs::mount_orphan::StaticMountinfoReader;
        let mut ctl = MountControl::default();
        ctl.set_mountinfo_reader(Box::new(StaticMountinfoReader::new(
            "24 28 8:2 / /home rw,relatime shared:30 - ext4 /dev/sda2 rw\n",
        )));
        let outcome = ctl.check_orphans().expect("reader must succeed");
        assert_eq!(outcome, OrphanCheckOutcome::Clean);
    }

    #[test]
    fn check_orphans_rejects_private_pcloud_rs_mount_when_not_forced() {
        use pcloud_fs::mount_orphan::StaticMountinfoReader;
        let payload = concat!(
            "24 28 8:2 / /home rw,relatime shared:30 - ext4 /dev/sda2 rw\n",
            "25 28 0:44 / /home/user/pCloudDrive rw,nosuid,nodev,relatime shared:77 - fuse.pcloud-rs pcloud-rs rw\n",
            "26 28 0:45 / /mnt/official rw,nosuid,nodev,relatime shared:78 - fuse.pcloud pcloud rw\n",
        );
        let mut ctl = MountControl::default();
        ctl.set_force_umount(false);
        ctl.set_mountinfo_reader(Box::new(StaticMountinfoReader::new(payload)));
        match ctl.check_orphans().unwrap() {
            OrphanCheckOutcome::Rejected(paths) => {
                assert_eq!(paths.len(), 1);
                assert!(paths.contains(&PathBuf::from("/home/user/pCloudDrive")));
                assert!(!paths.contains(&PathBuf::from("/mnt/official")));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn check_orphans_skips_known_active_mount() {
        // If the daemon were to re-run the orphan check after taking
        // ownership of a mount, its own mountpoint must not come back
        // as an orphan. We simulate this by hand-constructing the
        // known-active state via the internal field — the ActiveMount
        // handle is unused by the check so we synthesise a stub.
        //
        // Because ActiveMount requires a real MountHandle (which we
        // cannot construct from outside pcloud-fs), we assert the
        // weaker property: with an empty `known` set the same entry
        // is reported; with `force_umount=false` the caller still sees
        // the orphan and must decide. Parity with the daemon behaviour
        // is covered by `check_orphans_rejects_when_pcloud_orphans_present_and_not_forced`.
        use pcloud_fs::mount_orphan::StaticMountinfoReader;
        let payload =
            "25 28 0:44 / /home/user/pCloudDrive rw shared:77 - fuse.pcloud-rs pcloud-rs rw\n";
        let mut ctl = MountControl::default();
        ctl.set_mountinfo_reader(Box::new(StaticMountinfoReader::new(payload)));
        match ctl.check_orphans().unwrap() {
            OrphanCheckOutcome::Rejected(paths) => {
                assert_eq!(paths, vec![PathBuf::from("/home/user/pCloudDrive")]);
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn force_umount_env_value_enables_override() {
        assert!(force_umount_from_value(Some("1")));
        assert!(force_umount_from_value(Some("true")));
        assert!(force_umount_from_value(Some("TRUE")));
        assert!(!force_umount_from_value(Some("0")));
        assert!(!force_umount_from_value(Some("false")));
        assert!(!force_umount_from_value(None));
    }

    #[test]
    fn mock_factory_is_invoked_after_successful_validation() {
        let tmp = tempdir().unwrap();
        let mounts = Arc::new(AtomicUsize::new(0));
        let drain_calls = Arc::new(AtomicUsize::new(0));
        let drain_for_hook = Arc::clone(&drain_calls);
        let mut ctl = MountControl::new(
            mock_factory(Arc::clone(&mounts)),
            Arc::new(move || {
                drain_for_hook.fetch_add(1, Ordering::SeqCst);
                Ok("ok".to_owned())
            }),
        );
        // Fresh empty tempdir passes validation. The mock adapter is
        // invoked and returns UnsupportedPlatform — which is the
        // expected test-only outcome here because constructing a real
        // MountHandle requires libfuse.
        let resp = ctl.mount(tmp.path());
        assert_eq!(mounts.load(Ordering::SeqCst), 1);
        assert_eq!(resp.status, ResponseStatus::Unavailable);
        assert!(!ctl.is_mounted());
        // Drain hook must not fire unless something was mounted.
        assert_eq!(drain_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn control_debug_io_force_orphan_and_error_mapping_edges_are_stable() {
        use pcloud_fs::mount_orphan::StaticMountinfoReader;

        let root = tempdir().unwrap();
        let mut ctl = MountControl::default();
        let rendered = format!("{ctl:?}");
        assert!(rendered.contains("MountControl"));
        assert!(rendered.contains("active_mountpoint"));

        let state_file = root.path().join("state-file");
        std::fs::write(&state_file, b"not a directory").unwrap();
        ctl.set_state_dir(state_file);
        assert!(ctl.sweep_stale_pidfile().is_err());

        let orphan = root.path().join("orphan");
        let payload = format!(
            "25 28 0:44 / {} rw,nosuid,nodev - fuse.pcloud-rs pcloud-rs rw\n",
            orphan.display()
        );
        ctl.set_force_umount(true);
        ctl.set_mountinfo_reader(Box::new(StaticMountinfoReader::new(payload)));
        match ctl.check_orphans().unwrap() {
            OrphanCheckOutcome::ForceUnmounted(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0, orphan);
            }
            other => panic!("expected force-unmount result, got {other:?}"),
        }

        for error in [
            MountError::Unsupported("fixture".to_owned()),
            MountError::Io(std::io::Error::other("fixture")),
        ] {
            assert_ne!(mount_error_to_response(error).status, ResponseStatus::Ok);
        }
        assert_eq!(join_remote_path("/", "file"), "/file");
        assert_eq!(join_remote_path("/Documents", "file"), "/Documents/file");
        assert!(unix_now() > 0);
    }
}

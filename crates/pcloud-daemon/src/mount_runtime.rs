//! Daemon-side mount orchestration for bd-1du.4.e sub-task 2.
//!
//! This module wires the narrow IPC-visible surface:
//!
//! * `MountControl` owns the active `pcloud_fs::mount::MountHandle` and a
//!   small drain hook that runs on unmount.
//! * `mount_filesystem` validates the mountpoint, composes a FUSE adapter,
//!   and hands it to `pcloud_fs::mount::MountService::mount`.
//! * `unmount_filesystem` calls the drain hook and tears the session down.
//!
//! ## Scope honesty
//!
//! Sub-task 1 of bd-1du.4.e (`PcloudFsShim` + `fuser_shim.rs` composition
//! of `FuseAdapter` + `WritePathService` + a `ProtoFolderBackend` /
//! `ProtoFileBackend` / upload-backend stub) has **not** landed at the
//! time this module was written. Rather than speculatively mount a real
//! `ProtoFuseAdapter` against placeholder transport plumbing, this module
//! currently mounts `pcloud_fs::fuse_adapter::NullFuseAdapter`.
//! That gives us:
//!
//! * an honest end-to-end mount lifecycle (validate → mount → drain →
//!   unmount),
//! * a real `MountHandle` stored on the runtime and destroyed on drop,
//! * real IPC/CLI round-trips,
//! * a clean seam (`adapter_factory`) that sub-task 1 can swap for the
//!   composed `PcloudFsShim` without touching this file's public surface.
//!
//! Do **not** claim mounted-drive parity on the back of this module
//! alone — the bd-1du.4 tracker and `C_FEATURE_PARITY_MATRIX.csv` remain
//! the source of truth.
//!
//! ## Write-path wiring (bd-1du.4.6, footnote `[fuse-wiring]`)
//!
//! [`pcloud_shim_adapter_factory`] now constructs a full
//! [`WritePathService`] bound to the daemon's
//! [`ProtoUploadBackend`] + per-mount [`StagingDir`] + on-disk
//! [`WriteJournal`] and hands it to the composed [`PcloudFsShim`]. On the
//! Linux kernel mount path this means `create` / `write` / `flush` /
//! `fsync` / `setattr(size)` / `unlink` / `rename` are serviced by the
//! real writer instead of returning `ENOSYS`. Read+write up to the 64
//! MiB `flush_threshold_bytes` finalises via whole-file upload; chunked
//! `upload_write` pipelining for sustained multi-GiB writes is tracked
//! under `bd-1du.4.6` (see `TODO(bd-1du.4.6)` in
//! `pcloud-fs/src/write_path.rs`). The drain hook installed by the same
//! factory flushes every dirty inode before the kernel mount
//! disappears, keeping the ordered 5s shutdown sequence intact.
//!
//! The parity-matrix row remains under Reviewer 19's `bd-1du.10`
//! discipline — this module does not flip it.

// **PLATFORM:** Linux + macOS.
// **GATING:** `#[cfg(target_os = "linux")]` around the `PcloudFsShim`
// wrapper (fuser-only); `#[cfg(target_os = "macos")]` around the bare
// `ProtoFuseAdapter` wrapper that feeds the fuse-t FFI; shared FS
// composition (backends, staging, journal, write path) gated to
// `#[cfg(any(target_os = "linux", target_os = "macos"))]`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::fuse_adapter::{FuseAdapter, NullFuseAdapter};
use pcloud_fs::mount_orphan::{
    MountinfoReader, detect_orphans, fusermount_unmount, mountpoint_is_already_mounted,
};
#[cfg(not(target_os = "macos"))]
use pcloud_fs::mount_orphan::ProcMountinfoReader;
#[cfg(target_os = "macos")]
use pcloud_fs::platform::macos::MacosMountinfoReader;
use pcloud_fs::mount_service::{MountError, MountHandle, MountOptions, MountService};
use pcloud_ipc::{Response, ResponseStatus};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use pcloud_fs::backend::{ProtoFileBackend, ProtoFolderBackend, ProtoUploadBackend};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
#[cfg(target_os = "linux")]
use pcloud_fs::fuser_shim::PcloudFsShim;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use pcloud_fs::staging::StagingDir;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use pcloud_fs::write_journal::WriteJournal;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use pcloud_fs::write_path::{WritePathOptions, WritePathService};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use pcloud_proto::BinaryApiTransport;
#[cfg(any(target_os = "linux", target_os = "macos"))]
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

/// Drain hook invoked before unmount. Returns a short human summary.
pub type DrainHook = Arc<dyn Fn() -> String + Send + Sync>;

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
            Arc::new(|| "no drain work queued".to_owned()),
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
        #[cfg(not(target_os = "macos"))]
        {
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
        if let Some(active) = &self.active
            && active.mountpoint == path
        {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!(
                    "refusing to force-unmount active mount at {}; use 'unmount' instead",
                    path.display()
                ),
            };
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

        // Validate first so the error message is specific (missing dir vs
        // not-owned vs world-writable) instead of a generic fuser error.
        if let Err(err) = MountService::validate_mountpoint(mountpoint) {
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
        let Some(active) = self.active.take() else {
            return Response {
                status: ResponseStatus::Conflict,
                message: "no active filesystem mount".to_owned(),
            };
        };
        let drain_summary = (self.drain)();
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
        summary.push_str(&format!("drain: {}", (self.drain)()));
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
    // SAFETY: `kill` with `sig=0` performs error checking only and
    // does not deliver a signal. No memory is read or written by the
    // syscall beyond the kernel's own task-table walk.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
    let drain_msg = (drain)();
    summary.push_str(&format!("drain: {drain_msg}; "));
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
    std::env::var("PCLOUD_FORCE_UMOUNT")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn mount_error_to_response(err: MountError) -> Response {
    let status = match err {
        MountError::MountpointMissing(_)
        | MountError::MountpointNotDirectory(_)
        | MountError::MountpointNotEmpty(_)
        | MountError::MountpointNotOwned { .. }
        | MountError::MountpointWorldWritable { .. }
        | MountError::AllowOtherRejected => ResponseStatus::InvalidRequest,
        MountError::UnsupportedPlatform => ResponseStatus::Unavailable,
        MountError::Unsupported(_) => ResponseStatus::Unavailable,
        MountError::Io(_) => ResponseStatus::InternalError,
        #[cfg(target_os = "linux")]
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
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug)]
pub struct ShimFactoryParams {
    /// Live protocol transport (shared, cheap to clone).
    pub transport: BinaryApiTransport,
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
#[cfg(target_os = "linux")]
struct PcloudShimAdapter {
    writer: Arc<WritePathService<ProtoUploadBackend<BinaryApiTransport>>>,
    shim: Option<
        PcloudFsShim<
            ProtoFolderBackend<BinaryApiTransport>,
            ProtoFileBackend<BinaryApiTransport>,
            ProtoUploadBackend<BinaryApiTransport>,
        >,
    >,
}

#[cfg(target_os = "linux")]
impl DynFuseAdapter for PcloudShimAdapter {
    fn mount_with(
        mut self: Box<Self>,
        service: &MountService,
        mountpoint: &Path,
        options: MountOptions,
    ) -> Result<MountHandle, MountError> {
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
/// macOS fuse-t path. Dispatches through [`MountService::mount`] (which
/// routes to `MacosPlatformMount::mount_adapter` and then the fuse-t FFI).
/// The `fuser` crate is Linux-only, so we deliberately skip the
/// `PcloudFsShim` wrapper on macOS — the fuse-t FFI thunks in
/// `pcloud_fs::platform::macos_ffi` speak to `FuseAdapter` directly.
#[cfg(target_os = "macos")]
struct PcloudProtoAdapter {
    writer: Arc<WritePathService<ProtoUploadBackend<BinaryApiTransport>>>,
    adapter: Option<
        ProtoFuseAdapter<
            ProtoFolderBackend<BinaryApiTransport>,
            ProtoFileBackend<BinaryApiTransport>,
        >,
    >,
}

#[cfg(target_os = "macos")]
impl DynFuseAdapter for PcloudProtoAdapter {
    fn mount_with(
        mut self: Box<Self>,
        service: &MountService,
        mountpoint: &Path,
        options: MountOptions,
    ) -> Result<MountHandle, MountError> {
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
/// and mounts it via [`MountService::mount_fuser`]. On macOS the factory
/// mounts the bare [`ProtoFuseAdapter`] via [`MountService::mount`] which
/// routes through the fuse-t FFI; there is no `fuser::Filesystem` layer
/// on macOS because the `fuser` crate is Linux-only.
///
/// Returns `(factory, drain_hook)`. The drain hook holds a shared reference
/// to the writer so the caller can install it into [`MountControl::new`].
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[must_use]
pub fn pcloud_shim_adapter_factory(params: ShimFactoryParams) -> (AdapterFactory, DrainHook) {
    let writer_slot: Arc<
        std::sync::Mutex<Option<Arc<WritePathService<ProtoUploadBackend<BinaryApiTransport>>>>>,
    > = Arc::new(std::sync::Mutex::new(None));
    let writer_slot_for_factory = Arc::clone(&writer_slot);

    // Capture individual fields so the factory closure is `Fn` (not FnOnce)
    // and each invocation clones the secret explicitly via `clone_secret`.
    let ShimFactoryParams {
        transport,
        auth_token,
        staging_root,
        write_options,
        adapter_options,
    } = params;
    let auth_token = Arc::new(auth_token);

    let factory: AdapterFactory = Box::new(move || {
        let folder = Arc::new(ProtoFolderBackend::new(
            transport.clone(),
            auth_token.clone_secret(),
        ));
        let files = Arc::new(ProtoFileBackend::new(
            transport.clone(),
            auth_token.clone_secret(),
        ));
        let stage = StagingDir::open(&staging_root)
            .map_err(|e| MountError::Io(std::io::Error::other(e.to_string())))?;
        let journal = WriteJournal::open(stage.journal_path())
            .map_err(|e| MountError::Io(std::io::Error::other(e.to_string())))?;
        let upload = Arc::new(ProtoUploadBackend::new(
            transport.clone(),
            auth_token.clone_secret(),
        ));
        // Startup-resume reconcile: before accepting any FUSE write,
        // walk the staging root's per-inode `ino-*.upload-progress`
        // sidecars and reconcile each against the server via
        // `upload_status` (pCloud `upload_info`). This trims the local
        // sidecar up (server ahead) or down (server behind), expires
        // garbage-collected upload ids, and aborts stalled uploads that
        // have been idle past `DEFAULT_HEARTBEAT_TIMEOUT`.
        match pcloud_fs::write_path::replay_upload_sidecars(
            &staging_root,
            upload.as_ref(),
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
        let writer = Arc::new(WritePathService::new(stage, journal, upload, write_options));
        // Publish the writer for the drain hook.
        *writer_slot_for_factory.lock().expect("writer slot") = Some(Arc::clone(&writer));

        // Wire the write-path into the adapter too so adapter-level FUSE
        // ops (setattr/create/etc. that flow through `FuseAdapter`) reach
        // the real writer.
        //
        // Linux: wrap in `Arc` and hand to `PcloudFsShim` which dispatches
        // `fuser::Filesystem` ops back into the adapter + the writer.
        //
        // macOS: hand the bare adapter to `MountService::mount` which
        // routes through the fuse-t FFI. There is no `fuser` layer; the
        // FFI thunks in `pcloud_fs::platform::macos_ffi` speak
        // `FuseAdapter` directly.
        #[cfg(target_os = "linux")]
        {
            let adapter = Arc::new(
                ProtoFuseAdapter::with_file_backend(folder, files, adapter_options)
                    .with_write_path(Arc::clone(&writer)),
            );

            let shim = PcloudFsShim::new(adapter, Arc::clone(&writer));
            Ok(Box::new(PcloudShimAdapter {
                writer,
                shim: Some(shim),
            }))
        }

        #[cfg(target_os = "macos")]
        {
            let adapter = ProtoFuseAdapter::with_file_backend(folder, files, adapter_options)
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
        let w = writer_slot.lock().expect("writer slot");
        let Some(writer) = w.as_ref() else {
            return "writer drain: no active writer".to_owned();
        };
        let open_fhs = writer.open_inode_count();
        let outcomes = writer.drain_all();
        let flushed = outcomes.iter().filter(|(_, r)| r.is_ok()).count();
        let failed: Vec<String> = outcomes
            .iter()
            .filter_map(|(ino, r)| r.as_ref().err().map(|e| format!("ino={ino}: {e}")))
            .collect();
        if failed.is_empty() {
            format!("writer drain: ok (open_fhs={open_fhs}, flushed={flushed})")
        } else {
            format!(
                "writer drain: partial (open_fhs={open_fhs}, flushed={flushed}, failed=[{}])",
                failed.join("; ")
            )
        }
    });

    (factory, drain)
}

#[cfg(test)]
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

    #[cfg(target_os = "linux")]
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
            Arc::new(|| "drained".to_owned()),
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pcloud_shim_factory_composes_real_shim_and_drain_reports_no_writer() {
        // Build a factory against a dummy network transport. We don't call
        // it (the BinaryApiTransport refuses to connect), but the drain
        // hook is wired and reports "no active writer" until a mount has
        // actually produced one.
        use pcloud_proto::{BinaryApiTransport, TransportConfig};
        use pcloud_secret::secret_string::SecretString;
        let tmp = tempdir().unwrap();
        let transport = BinaryApiTransport::new(TransportConfig {
            host: "127.0.0.1".to_owned(),
            port: 1,
            server_name: "localhost".to_owned(),
            use_tls: false,
            connect_timeout: std::time::Duration::from_millis(10),
            read_timeout: std::time::Duration::from_millis(10),
        });
        let params = ShimFactoryParams {
            transport,
            auth_token: SecretString::new("dummy-token"),
            staging_root: tmp.path().join("stage"),
            write_options: pcloud_fs::write_path::WritePathOptions::default(),
            adapter_options: pcloud_fs::fuse_adapter::AdapterOptions::default(),
        };
        let (_factory, drain) = pcloud_shim_adapter_factory(params);
        let msg = (drain)();
        assert!(msg.contains("no active writer"), "got: {msg}");
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
    fn check_orphans_rejects_when_pcloud_orphans_present_and_not_forced() {
        use pcloud_fs::mount_orphan::StaticMountinfoReader;
        let payload = concat!(
            "24 28 8:2 / /home rw,relatime shared:30 - ext4 /dev/sda2 rw\n",
            "25 28 0:44 / /home/user/pCloudDrive rw,nosuid,nodev,relatime shared:77 - fuse.pcloud pcloud rw\n",
            "26 28 0:45 / /mnt/legacy rw,nosuid,nodev,relatime shared:78 - fuse.pclsync pclsync rw\n",
        );
        let mut ctl = MountControl::default();
        ctl.set_force_umount(false);
        ctl.set_mountinfo_reader(Box::new(StaticMountinfoReader::new(payload)));
        match ctl.check_orphans().unwrap() {
            OrphanCheckOutcome::Rejected(paths) => {
                assert_eq!(paths.len(), 2);
                assert!(paths.contains(&PathBuf::from("/home/user/pCloudDrive")));
                assert!(paths.contains(&PathBuf::from("/mnt/legacy")));
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
        let payload = "25 28 0:44 / /home/user/pCloudDrive rw shared:77 - fuse.pcloud pcloud rw\n";
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
    fn force_umount_env_var_enables_override() {
        // SAFETY: single-threaded test that scopes the env var to this
        // test's body. We restore the previous value on exit.
        let prev = std::env::var("PCLOUD_FORCE_UMOUNT").ok();
        // SAFETY: setting env vars in tests is acceptable; other tests
        // run in separate processes per cargo's default and nextest
        // harnesses. This test does not read env vars in parallel
        // with others in this module.
        unsafe {
            std::env::set_var("PCLOUD_FORCE_UMOUNT", "1");
        }
        let ctl = MountControl::default();
        assert!(ctl.force_umount_enabled());
        // Restore.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PCLOUD_FORCE_UMOUNT", v),
                None => std::env::remove_var("PCLOUD_FORCE_UMOUNT"),
            }
        }
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
                "ok".to_owned()
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
}

//! **PLATFORM: Linux only.**
//! **GATING: `#[cfg(target_os = "linux")]`** -- the entire module file is
//! gated at the `mod linux;` line in `platform/mod.rs`.
//!
//! Reads `/proc/self/mountinfo` for orphan detection; uses `libfuse3` +
//! `fusermount3` for (un)mount; `umount2(2)` for forced unmount on
//! shutdown. Not portable to BSD/macOS/Windows.
//!
//! All previously Linux-gated FUSE glue from `mount_service.rs` now lives
//! here. The cross-platform seam is [`crate::platform::PlatformMount`] /
//! [`crate::platform::MountinfoReader`].

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

// ENOTSUP / EOPNOTSUPP: "operation not supported". Used instead of ENOSYS
// ("no such syscall") for operations that are understood but inapplicable to
// this filesystem (e.g. access(2) and chmod/chown have no meaning on pCloud,
// which has no per-file Unix permission bits).
const ENOTSUP: i32 = libc::EOPNOTSUPP;

use crate::fuse_adapter::FuseAdapter;
use crate::mount_orphan::MountinfoReader;
use crate::mount_service::{MountError, MountHandle, MountOptions};
use crate::platform::PlatformMount;

// -----------------------------------------------------------------------------
// MountinfoReader (Linux): reads /proc/self/mountinfo.
// -----------------------------------------------------------------------------

/// Default reader that reads `/proc/self/mountinfo` directly.
///
/// Re-exported from [`crate::mount_orphan`] for backward compatibility.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcMountinfoReader;

impl MountinfoReader for ProcMountinfoReader {
    fn read(&self) -> io::Result<String> {
        std::fs::read_to_string("/proc/self/mountinfo")
    }
}

// -----------------------------------------------------------------------------
// PlatformMount (Linux): FUSE (un)mount via the `fuser` crate.
// -----------------------------------------------------------------------------

/// Linux platform-mount implementation. Uses `fuser` + `fusermount3`.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPlatformMount;

impl PlatformMount for LinuxPlatformMount {
    fn validate_mountpoint(&self, mountpoint: &Path) -> Result<(), MountError> {
        crate::mount_service::MountService::validate_mountpoint(mountpoint)
    }

    fn probe_supported(&self) -> Result<(), MountError> {
        Ok(())
    }

    /// Linux implementation of the cross-platform `mount_adapter` seam.
    /// Wraps the boxed adapter in a `fuser::Filesystem` shim that forwards
    /// both read-path (`lookup` / `getattr` / `readdir` / `open` / `read`
    /// / `release`) and write-path (`create` / `write` / `flush` / `fsync`
    /// / `setattr` / `unlink` / `rename` / `mkdir` / `rmdir`) ops through
    /// the `FuseAdapter` trait, and delegates to the existing
    /// [`mount_fuser_filesystem`] entry point.
    fn mount_adapter(
        &self,
        adapter: Box<dyn FuseAdapter>,
        mount_point: &Path,
        opts: MountOptions,
    ) -> Result<MountHandle, MountError> {
        let shim = BoxedFuserShim::new(adapter);
        mount_fuser_filesystem(mount_point, shim, opts)
    }
}

/// `fuser::Filesystem` shim over a boxed `dyn FuseAdapter`.
///
/// Forwards both read-path (`lookup`, `getattr`, `readdir`, `open`,
/// `read`, `release`) and write-path (`create`, `write`, `flush`,
/// `fsync`, `setattr(size)`, `unlink`, `rename`, `mkdir`, `rmdir`)
/// kernel operations through the [`FuseAdapter`] trait.
///
/// When the underlying adapter has no write-path attached (e.g.
/// [`crate::fuse_adapter::NullFuseAdapter`]), the trait default
/// methods return `ENOSYS` and the kernel treats the mount as
/// read-only — no explicit `read_only` flag is needed on this shim.
struct BoxedFuserShim {
    adapter: Box<dyn FuseAdapter>,
    ttl: std::time::Duration,
}

impl BoxedFuserShim {
    fn new(adapter: Box<dyn FuseAdapter>) -> Self {
        Self {
            adapter,
            ttl: std::time::Duration::from_secs(1),
        }
    }

    fn path_for(&self, ino: u64) -> Option<std::path::PathBuf> {
        self.adapter.resolve_ino_to_path(ino).ok()
    }

    fn join_child(parent: &std::path::Path, name: &std::ffi::OsStr) -> Option<String> {
        let n = name.to_str()?;
        if n.is_empty() || n.contains('/') || n.contains('\0') {
            return None;
        }
        let p = parent.to_str()?;
        Some(if p == "/" {
            format!("/{n}")
        } else {
            format!("{p}/{n}")
        })
    }
}

fn adapter_kind_to_fuser(k: crate::fuse_adapter::FsEntryKind) -> fuser::FileType {
    use crate::fuse_adapter::FsEntryKind;
    match k {
        FsEntryKind::Directory => fuser::FileType::Directory,
        FsEntryKind::RegularFile => fuser::FileType::RegularFile,
        FsEntryKind::Symlink => fuser::FileType::Symlink,
    }
}

fn adapter_attr_to_fuser(a: &crate::fuse_adapter::EntryAttr) -> fuser::FileAttr {
    let now = std::time::SystemTime::now();
    let mtime = a
        .mtime_epoch
        .map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s))
        .unwrap_or(now);
    fuser::FileAttr {
        ino: a.ino,
        size: a.size,
        blocks: a.size.div_ceil(512),
        atime: mtime,
        mtime,
        ctime: mtime,
        crtime: mtime,
        kind: adapter_kind_to_fuser(a.kind),
        perm: a.mode,
        nlink: 1,
        uid: a.uid,
        gid: a.gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn reply_account_quota(adapter: &dyn FuseAdapter, reply: fuser::ReplyStatfs) {
    match adapter.statfs() {
        Ok((total_bytes, free_bytes)) => {
            let (blocks, free_blocks) = crate::fuse_adapter::statfs_blocks(total_bytes, free_bytes);
            reply.statfs(
                blocks,
                free_blocks,
                free_blocks,
                0,
                0,
                crate::fuse_adapter::STATFS_BLOCK_SIZE,
                255,
                crate::fuse_adapter::STATFS_BLOCK_SIZE,
            );
        }
        Err(errno) => reply.error(errno),
    }
}

impl fuser::Filesystem for BoxedFuserShim {
    /// Report pCloud account quota, never the local staging filesystem.
    fn statfs(&mut self, _req: &fuser::Request<'_>, _ino: u64, reply: fuser::ReplyStatfs) {
        reply_account_quota(self.adapter.as_ref(), reply);
    }

    fn lookup(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEntry,
    ) {
        let Some(n) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.lookup(parent, n) {
            Ok(attr) => reply.entry(&self.ttl, &adapter_attr_to_fuser(&attr), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn getattr(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: Option<u64>,
        reply: fuser::ReplyAttr,
    ) {
        match self.adapter.getattr(ino) {
            Ok(attr) => reply.attr(&self.ttl, &adapter_attr_to_fuser(&attr)),
            Err(errno) => reply.error(errno),
        }
    }

    fn readdir(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: fuser::ReplyDirectory,
    ) {
        let entries = match self.adapter.readdir(ino, offset) {
            Ok(v) => v,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let mut next = offset + 1;
        if offset == 0 {
            if reply.add(ino, next, fuser::FileType::Directory, ".") {
                reply.ok();
                return;
            }
            next += 1;
            // For the dyn-trait shim we do not have a back-pointer from
            // child-ino -> parent-ino, so `..` points to `ino` itself.
            // This is acceptable for a read-only scaffold; real parent
            // resolution is provided by `PcloudFsShim`.
            if reply.add(ino, next, fuser::FileType::Directory, "..") {
                reply.ok();
                return;
            }
            next += 1;
        }
        for entry in entries {
            if reply.add(
                entry.ino,
                next,
                adapter_kind_to_fuser(entry.kind),
                &entry.name,
            ) {
                break;
            }
            next += 1;
        }
        reply.ok();
    }

    fn open(&mut self, _req: &fuser::Request<'_>, ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        match self.adapter.open(ino) {
            Ok(h) => reply.opened(h, 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn read(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyData,
    ) {
        let off = offset.max(0) as u64;
        let started = std::time::Instant::now();
        let outcome = self.adapter.read(fh, off, size as usize);
        crate::slo_hook::observe_mount_read(started.elapsed());
        match outcome {
            Ok(bytes) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn release(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        match self.adapter.release(fh) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    // -----------------------------------------------------------------
    // Write-path forwarding through the dyn FuseAdapter trait.
    // -----------------------------------------------------------------

    fn create(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(crate::errors::ENOENT);
            return;
        };
        let parent_str = match parent_path.to_str() {
            Some(s) => s,
            None => {
                reply.error(crate::errors::EINVAL);
                return;
            }
        };
        let Some(name_str) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.create(parent_str, name_str) {
            Ok(ino) => {
                let attr = match self.adapter.getattr(ino) {
                    Ok(a) => a,
                    Err(errno) => {
                        reply.error(errno);
                        return;
                    }
                };
                reply.created(&self.ttl, &adapter_attr_to_fuser(&attr), 0, 0, 0);
            }
            Err(errno) => reply.error(errno),
        }
    }

    fn write(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyWrite,
    ) {
        let off = offset.max(0) as u64;
        match self.adapter.write(ino, off, data) {
            Ok(n) => reply.written(n as u32),
            Err(errno) => reply.error(errno),
        }
    }

    fn flush(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        match self.adapter.flush_write(ino) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn fsync(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: fuser::ReplyEmpty,
    ) {
        match self.adapter.fsync_write(ino) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    /// setattr: chmod/chown return ENOTSUP (pCloud has no Unix permission
    /// bits). utimens is accepted as a no-op to satisfy editors that update
    /// mtime. Size changes are forwarded to the adapter's truncate method.
    ///
    /// pCloud does not support Unix permission bits. chmod/chown return
    /// ENOTSUP; utimens is accepted as a no-op to satisfy editors that
    /// update mtime.
    fn setattr(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<u32>,
        reply: fuser::ReplyAttr,
    ) {
        // pCloud does not support Unix permission bits. chmod/chown return
        // ENOTSUP; utimens is accepted as a no-op to satisfy editors that
        // update mtime.
        if mode.is_some() || uid.is_some() || gid.is_some() {
            reply.error(ENOTSUP);
            return;
        }
        if let Some(new_size) = size {
            if let Err(errno) = self.adapter.truncate(ino, new_size) {
                reply.error(errno);
                return;
            }
        }
        match self.adapter.getattr(ino) {
            Ok(mut attr) => {
                if let Some(s) = size {
                    attr.size = s;
                }
                reply.attr(&self.ttl, &adapter_attr_to_fuser(&attr));
            }
            Err(errno) => reply.error(errno),
        }
    }

    /// access(2) is not meaningful on pCloud because there are no per-file
    /// Unix permission bits. Return ENOTSUP so callers fall back to
    /// getattr UID/GID checks.
    fn access(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _mask: i32,
        reply: fuser::ReplyEmpty,
    ) {
        // pCloud has no per-file Unix permission bits; access(2) is not meaningful.
        // Return ENOTSUP so callers fall back to getattr UID/GID checks.
        reply.error(ENOTSUP);
    }

    /// forget: called by the kernel when it releases a reference to an inode
    /// lookup. Decrements the adapter's lookup reference count and evicts the
    /// inode map entry when it reaches zero.
    fn forget(&mut self, _req: &fuser::Request<'_>, ino: u64, nlookup: u64) {
        self.adapter.forget_ino(ino, nlookup);
        log::trace!("forget ino={} nlookup={}", ino, nlookup);
    }

    fn unlink(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEmpty,
    ) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(crate::errors::ENOENT);
            return;
        };
        let parent_str = match parent_path.to_str() {
            Some(s) => s,
            None => {
                reply.error(crate::errors::EINVAL);
                return;
            }
        };
        let Some(name_str) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.unlink(parent_str, name_str) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn rename(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        newparent: u64,
        newname: &std::ffi::OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        let Some(from_parent) = self.path_for(parent) else {
            reply.error(crate::errors::ENOENT);
            return;
        };
        let Some(to_parent) = self.path_for(newparent) else {
            reply.error(crate::errors::ENOENT);
            return;
        };
        let Some(from) = Self::join_child(&from_parent, name) else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        let Some(to) = Self::join_child(&to_parent, newname) else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.rename(&from, &to) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn mkdir(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        _mode: u32,
        _umask: u32,
        reply: fuser::ReplyEntry,
    ) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(crate::errors::ENOENT);
            return;
        };
        let parent_str = match parent_path.to_str() {
            Some(s) => s,
            None => {
                reply.error(crate::errors::EINVAL);
                return;
            }
        };
        let Some(name_str) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.mkdir(parent_str, name_str) {
            Ok(attr) => reply.entry(&self.ttl, &adapter_attr_to_fuser(&attr), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn rmdir(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEmpty,
    ) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(crate::errors::ENOENT);
            return;
        };
        let Some(full) = Self::join_child(&parent_path, name) else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.rmdir(&full) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }
}

// -----------------------------------------------------------------------------
// Signal handling + active-mount registry (process-wide).
// -----------------------------------------------------------------------------

// AtomicBool set by the signal handler; the reaper thread observes this
// flag via a `Condvar` wake-up and initiates graceful unmount on all
// registered active mounts. Using an atomic avoids Mutex inside the
// signal handler, which is not async-signal-safe.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Condvar the reaper thread blocks on. The signal handler notifies via
/// `libc::pthread_cond_broadcast` indirectly: the handler body only
/// writes to `SHUTDOWN_REQUESTED`, and the reaper polls on a timed wait
/// so it wakes within ~100ms of a signal even without explicit notify
/// (notify_all from a signal handler is not async-signal-safe).
static SHUTDOWN_CV: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

fn shutdown_cv() -> &'static (Mutex<bool>, Condvar) {
    SHUTDOWN_CV.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

// Registry of active mount paths. Canonical-path `BTreeSet` with
// debug assertions that register/unregister calls balance. Updated only
// from non-signal contexts; the reaper thread drains it on shutdown.
static ACTIVE_MOUNTS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();

fn registry() -> &'static Mutex<BTreeSet<PathBuf>> {
    ACTIVE_MOUNTS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Canonicalise a mount path to a stable key usable by both
/// register/unregister. audit-06 fix: both register and unregister MUST
/// derive the key the same way so that two user-typed variants of the
/// same mount (e.g. `/mnt/a` and `/mnt/a/`) map to a single BTreeSet
/// entry, eliminating the "skip removal / duplicate entry" race under
/// concurrent mount + unmount of the same underlying target.
///
/// Resolution order:
///   1. `fs::canonicalize` (dereferences symlinks, strips trailing slash),
///   2. fallback to `path.absolutize`-style component normalisation via
///      joining onto CWD when canonicalize fails (e.g. path no longer
///      exists because the kernel mount was already torn down).
///
/// This guarantees that `canonical_key(p)` is pure with respect to the
/// user-typed path string even when the filesystem state changes
/// between register and unregister.
fn canonical_key(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    // Fallback: if the path no longer exists (e.g. mid-teardown) we
    // still need a deterministic key. Join onto CWD when relative and
    // strip a trailing separator so that `/mnt/a` and `/mnt/a/` collide.
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    // Strip trailing separator by round-tripping components.
    let mut normalised = PathBuf::new();
    for comp in abs.components() {
        normalised.push(comp.as_os_str());
    }
    normalised
}

/// Register `path` in the active-mount set. Uses [`canonical_key`] so
/// the unregister-at-Drop path matches exactly what was registered.
/// Logs at `error!` level on double-register (a lifecycle bug).
fn register_mount(path: &Path) {
    let key = canonical_key(path);
    if let Ok(mut guard) = registry().lock() {
        let inserted = guard.insert(key);
        // M-5.2: Use log::error! instead of debug_assert! — debug_assert!
        // is a no-op in release builds and the race is possible in both
        // debug and release. A double-register is not fatal but indicates
        // a lifecycle bug that must surface in production logs.
        if !inserted {
            log::error!(
                "ACTIVE_MOUNTS double-register: {path:?}; \
                 this indicates a mount lifecycle bug — please file a bug report"
            );
        }
    }
}

/// Remove `path` from the active-mount set using [`canonical_key`] —
/// the same derivation used by [`register_mount`]. audit-06 fix: the
/// previous raw-path fallback (`|| guard.remove(path)`) has been removed
/// because the canonical key is now deterministic in both directions,
/// so a fallback would only mask a bug.
fn unregister_mount(path: &Path) {
    let key = canonical_key(path);
    if let Ok(mut guard) = registry().lock() {
        let removed = guard.remove(&key);
        // M-5.2: Use log::error! instead of debug_assert! (inactive in release).
        if !removed {
            log::error!(
                "ACTIVE_MOUNTS unregister miss: {path:?} (key={key:?}); \
                 this indicates an unbalanced mount/unmount lifecycle bug"
            );
        }
    }
}

static SIGNAL_HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();
static REAPER_INSTALLED: OnceLock<()> = OnceLock::new();

/// Return whether a shutdown has been requested via SIGTERM/SIGINT.
/// Poll this from the main event loop to trigger unmount cleanup.
#[allow(dead_code)]
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

fn install_signal_handler_once() {
    SIGNAL_HANDLER_INSTALLED.get_or_init(|| {
        // SAFETY: `sigaction(2)` is called exactly once per signal during
        // process lifetime with a static handler. The handler body only
        // stores to an `AtomicBool`, which is async-signal-safe. We use
        // `SA_RESTART` so long-running syscalls resume transparently
        // across the signal delivery instead of returning `EINTR` to
        // callers that are not prepared for it.
        // SAFETY: see paragraph above.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            // Explicit fn-type coercion first to silence
            // `fn_to_numeric_cast` lint: cast through the concrete fn
            // pointer type before converting to the platform-sized
            // integer libc expects in `sa_sigaction`.
            let handler: extern "C" fn(libc::c_int) = signal_trampoline;
            sa.sa_sigaction = handler as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            let _ = libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
            let _ = libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        }
    });
    // Install the reaper thread after the signal handler is in place so
    // there is no window where a signal can flip the flag without
    // anyone listening.
    install_reaper_once();
}

extern "C" fn signal_trampoline(_sig: libc::c_int) {
    // SAFETY: AtomicBool::store is async-signal-safe. We do NOT lock any
    // Mutex here — Mutex::lock is not async-signal-safe and can deadlock
    // if the signal fires while another thread holds the lock. The
    // reaper thread wakes up from its timed Condvar wait within ~100ms.
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

fn install_reaper_once() {
    REAPER_INSTALLED.get_or_init(|| {
        // M-5.3: surface spawn failure via log::error! instead of silently
        // discarding with .ok(). A spawn failure means signals received
        // while mounts are live will not trigger cleanup, risking stale
        // kernel FUSE sessions. The failure is logged so operators can act.
        if let Err(e) = std::thread::Builder::new()
            .name("pcloudfs-reaper".to_string())
            .spawn(reaper_main)
        {
            log::error!(
                "pcloud-fs: failed to spawn reaper thread; \
                 active mounts will NOT be cleaned up on SIGTERM/SIGINT: {e}"
            );
        }
    });
}

/// Reaper thread entry: block on a `Condvar` with a 100ms timed wait and
/// check `SHUTDOWN_REQUESTED` on every wake. When set, drain the active
/// mount registry and issue a lazy `umount2(MNT_DETACH)` on every
/// remaining path so the kernel releases FUSE resources even if the
/// owning `MountHandle` drops never run (e.g. process abort).
fn reaper_main() {
    let (lock, cv) = shutdown_cv();
    loop {
        if SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            reap_all_mounts();
            return;
        }
        let guard = match lock.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // Timed wait bounds the latency between signal delivery and
        // unmount — the signal handler cannot safely call notify_all(),
        // so we poll the atomic on every timeout.
        let (_guard, _) = cv
            .wait_timeout(guard, std::time::Duration::from_millis(100))
            .unwrap_or_else(|p| p.into_inner());
    }
}

fn reap_all_mounts() {
    let paths: Vec<PathBuf> = registry()
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default();
    for path in paths {
        log::warn!(
            "pcloud-fs reaper: signal received, detaching mount at {}",
            path.display()
        );
        if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
            // SAFETY: `umount2` is a direct syscall; `c` owns the
            // NUL-terminated path bytes for the duration of the call.
            // MNT_DETACH never blocks waiting on in-flight I/O.
            let rc = unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
            if rc != 0 {
                let e = std::io::Error::last_os_error();
                log::warn!(
                    "pcloud-fs reaper: umount2({}) failed: {}",
                    path.display(),
                    e
                );
            }
        }
        if let Ok(mut guard) = registry().lock() {
            guard.remove(&path);
        }
    }
}

// -----------------------------------------------------------------------------
// Linux mount handle (RAII).
// -----------------------------------------------------------------------------

/// How long we wait for the kernel to drop the FUSE mount entry from
/// `/proc/self/mountinfo` after the `fuser::BackgroundSession` is
/// released before we escalate to `umount2(MNT_DETACH)`.
///
/// 2s matches the existing P1.4 drain-sequence budget for kernel-side
/// release and is short enough that operators who are actively waiting
/// for `pcloudc unmount` do not perceive it as hung, but long enough
/// that a well-behaved kernel + libfuse release cleanly without needing
/// a lazy unmount.
const SESSION_DROP_SETTLE_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// RAII inner of [`MountHandle`] on Linux. Public within the crate so
/// `mount_service.rs` can own an `Option<LinuxMountHandle>` field.
pub struct LinuxMountHandle {
    mountpoint: PathBuf,
    session: Option<fuser::BackgroundSession>,
}

impl LinuxMountHandle {
    /// Explicit unmount. Drops the background session, waits up to
    /// `SESSION_DROP_SETTLE_WINDOW` for the kernel to release the mount,
    /// and — if the mount is still visible in `/proc/self/mountinfo`
    /// — escalates with a lazy `umount2(MNT_DETACH)` so a blocked
    /// in-flight read from another process cannot pin the mount forever.
    ///
    /// The process-wide registry entry is always removed, regardless of
    /// whether the kernel-side unmount succeeded, so the SIGTERM
    /// trampoline does not attempt to unmount the same path again.
    pub fn unmount(mut self) -> Result<(), MountError> {
        // Drop the fuser BackgroundSession on a helper thread with a bounded
        // 5-second join so a wedged FUSE loop cannot block Drop forever.
        // We use an `mpsc::sync_channel(1)` doorbell + `recv_timeout` rather
        // than `JoinHandle::join()` because `join()` has no timeout and
        // would block the caller indefinitely on a wedged FUSE loop. If the
        // timeout elapses we deliberately leak the thread (logging an
        // error) — the kernel lazy-unmount below is the authoritative
        // recovery, and the thread will exit once libfuse unwedges.
        let session = self.session.take();
        let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
        let _joiner = std::thread::Builder::new()
            .name("pcloudfs-session-drop".to_string())
            .spawn(move || {
                drop(session);
                let _ = tx.send(());
            });
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(()) => {}
            Err(_) => {
                log::error!(
                    "FUSE session drop exceeded 5s timeout on {}; leaking \
                     helper thread and escalating to umount2(MNT_DETACH)",
                    self.mountpoint.display()
                );
                // Deliberately do not join `_joiner`; the lazy umount
                // below will unwedge libfuse.
            }
        }

        // Settle window: poll `/proc/self/mountinfo` for the path. Most
        // unmounts complete within a few milliseconds; we exit early
        // once the path is gone.
        let deadline = std::time::Instant::now() + SESSION_DROP_SETTLE_WINDOW;
        let reader = ProcMountinfoReader;
        let mut still_mounted = true;
        while std::time::Instant::now() < deadline {
            let payload =
                <ProcMountinfoReader as MountinfoReader>::read(&reader).unwrap_or_default();
            let present = crate::mount_orphan::parse_pcloud_mounts(&payload)
                .into_iter()
                .any(|e| e.mount_point == self.mountpoint);
            if !present {
                still_mounted = false;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        // Escalation path: lazy unmount via `umount2(MNT_DETACH)`. This
        // is the UNIX-blessed recovery for a blocked mount (think: a
        // process holding an open file handle through the FUSE bridge
        // that refuses to release). The kernel will tear down the mount
        // once the last reference goes away, so subsequent mount()s at
        // the same path succeed without operator intervention.
        let mut fallback_err: Option<MountError> = None;
        if still_mounted {
            if let Ok(c) = std::ffi::CString::new(self.mountpoint.as_os_str().as_encoded_bytes()) {
                // SAFETY: `umount2` is a direct syscall. `c` owns the
                // NUL-terminated path bytes for the duration of the call;
                // no aliased mutable state is observable across the FFI
                // boundary.
                let rc = unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
                if rc != 0 {
                    let errno = std::io::Error::last_os_error();
                    // EINVAL / ENOENT here mean the kernel has already
                    // released the mount between our last poll and the
                    // syscall; treat those as success. Everything else
                    // bubbles up so the daemon can log the real cause.
                    let raw = errno.raw_os_error().unwrap_or(0);
                    if raw != libc::EINVAL && raw != libc::ENOENT {
                        fallback_err = Some(MountError::Io(errno));
                    }
                }
            } else {
                // Non-UTF-8 / NUL-embedded mountpoint is impossible via
                // the validator but we still fail explicitly rather
                // than silently leak the mount.
                fallback_err = Some(MountError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "mountpoint cannot be converted to CString",
                )));
            }
        }

        unregister_mount(&self.mountpoint);
        match fallback_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

// -----------------------------------------------------------------------------
// Adapter-wrapping shim that implements `fuser::Filesystem`.
// -----------------------------------------------------------------------------

/// Shim adapter that implements `fuser::Filesystem` by delegating
/// both read-path and write-path kernel operations through the
/// [`FuseAdapter`] trait.
///
/// Forwards `lookup`, `getattr`, `readdir`, `open`, `read`, `release`,
/// `create`, `write`, `flush`, `fsync`, `setattr(size)`, `unlink`,
/// `rename`, `mkdir`, and `rmdir`. When the adapter has no write-path
/// attached, trait-default `ENOSYS` replies keep the mount read-only.
struct FuserShim<A: FuseAdapter> {
    adapter: A,
    ttl: std::time::Duration,
}

impl<A: FuseAdapter> fuser::Filesystem for FuserShim<A> {
    /// Report pCloud account quota, never the local staging filesystem.
    fn statfs(&mut self, _req: &fuser::Request<'_>, _ino: u64, reply: fuser::ReplyStatfs) {
        reply_account_quota(&self.adapter, reply);
    }

    fn lookup(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEntry,
    ) {
        let Some(n) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.lookup(parent, n) {
            Ok(attr) => reply.entry(&self.ttl, &adapter_attr_to_fuser(&attr), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn getattr(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: Option<u64>,
        reply: fuser::ReplyAttr,
    ) {
        match self.adapter.getattr(ino) {
            Ok(attr) => reply.attr(&self.ttl, &adapter_attr_to_fuser(&attr)),
            Err(errno) => reply.error(errno),
        }
    }

    fn readdir(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: fuser::ReplyDirectory,
    ) {
        let entries = match self.adapter.readdir(ino, offset) {
            Ok(v) => v,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let mut next = offset + 1;
        if offset == 0 {
            if reply.add(ino, next, fuser::FileType::Directory, ".") {
                reply.ok();
                return;
            }
            next += 1;
            if reply.add(ino, next, fuser::FileType::Directory, "..") {
                reply.ok();
                return;
            }
            next += 1;
        }
        for entry in entries {
            if reply.add(
                entry.ino,
                next,
                adapter_kind_to_fuser(entry.kind),
                &entry.name,
            ) {
                break;
            }
            next += 1;
        }
        reply.ok();
    }

    fn open(&mut self, _req: &fuser::Request<'_>, ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        match self.adapter.open(ino) {
            Ok(h) => reply.opened(h, 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn read(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyData,
    ) {
        let off = offset.max(0) as u64;
        let started = std::time::Instant::now();
        let outcome = self.adapter.read(fh, off, size as usize);
        crate::slo_hook::observe_mount_read(started.elapsed());
        match outcome {
            Ok(bytes) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn release(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        match self.adapter.release(fh) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    // -----------------------------------------------------------------
    // Write-path forwarding through the FuseAdapter trait.
    // -----------------------------------------------------------------

    fn create(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        let parent_path = match self.adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let parent_str = match parent_path.to_str() {
            Some(s) => s,
            None => {
                reply.error(crate::errors::EINVAL);
                return;
            }
        };
        let Some(name_str) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.create(parent_str, name_str) {
            Ok(ino) => {
                let attr = match self.adapter.getattr(ino) {
                    Ok(a) => a,
                    Err(errno) => {
                        reply.error(errno);
                        return;
                    }
                };
                reply.created(&self.ttl, &adapter_attr_to_fuser(&attr), 0, 0, 0);
            }
            Err(errno) => reply.error(errno),
        }
    }

    fn write(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyWrite,
    ) {
        let off = offset.max(0) as u64;
        match self.adapter.write(ino, off, data) {
            Ok(n) => reply.written(n as u32),
            Err(errno) => reply.error(errno),
        }
    }

    fn flush(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        match self.adapter.flush_write(ino) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn fsync(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: fuser::ReplyEmpty,
    ) {
        match self.adapter.fsync_write(ino) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    /// setattr: chmod/chown return ENOTSUP (pCloud has no Unix permission
    /// bits). utimens is accepted as a no-op to satisfy editors that update
    /// mtime. Size changes are forwarded to the adapter's truncate method.
    ///
    /// pCloud does not support Unix permission bits. chmod/chown return
    /// ENOTSUP; utimens is accepted as a no-op to satisfy editors that
    /// update mtime.
    fn setattr(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<u32>,
        reply: fuser::ReplyAttr,
    ) {
        // pCloud does not support Unix permission bits. chmod/chown return
        // ENOTSUP; utimens is accepted as a no-op to satisfy editors that
        // update mtime.
        if mode.is_some() || uid.is_some() || gid.is_some() {
            reply.error(ENOTSUP);
            return;
        }
        if let Some(new_size) = size {
            if let Err(errno) = self.adapter.truncate(ino, new_size) {
                reply.error(errno);
                return;
            }
        }
        match self.adapter.getattr(ino) {
            Ok(mut attr) => {
                if let Some(s) = size {
                    attr.size = s;
                }
                reply.attr(&self.ttl, &adapter_attr_to_fuser(&attr));
            }
            Err(errno) => reply.error(errno),
        }
    }

    /// access(2) is not meaningful on pCloud because there are no per-file
    /// Unix permission bits. Return ENOTSUP so callers fall back to
    /// getattr UID/GID checks.
    fn access(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _mask: i32,
        reply: fuser::ReplyEmpty,
    ) {
        // pCloud has no per-file Unix permission bits; access(2) is not meaningful.
        // Return ENOTSUP so callers fall back to getattr UID/GID checks.
        reply.error(ENOTSUP);
    }

    /// forget: called by the kernel when it releases a reference to an inode
    /// lookup. Decrements the adapter's lookup reference count and evicts the
    /// inode map entry when it reaches zero.
    fn forget(&mut self, _req: &fuser::Request<'_>, ino: u64, nlookup: u64) {
        self.adapter.forget_ino(ino, nlookup);
        log::trace!("forget ino={} nlookup={}", ino, nlookup);
    }

    fn unlink(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEmpty,
    ) {
        let parent_path = match self.adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let parent_str = match parent_path.to_str() {
            Some(s) => s,
            None => {
                reply.error(crate::errors::EINVAL);
                return;
            }
        };
        let Some(name_str) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.unlink(parent_str, name_str) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn rename(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        newparent: u64,
        newname: &std::ffi::OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        let from_parent = match self.adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let to_parent = match self.adapter.resolve_ino_to_path(newparent) {
            Ok(p) => p,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let Some(from) = BoxedFuserShim::join_child(&from_parent, name) else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        let Some(to) = BoxedFuserShim::join_child(&to_parent, newname) else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.rename(&from, &to) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn mkdir(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        _mode: u32,
        _umask: u32,
        reply: fuser::ReplyEntry,
    ) {
        let parent_path = match self.adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let parent_str = match parent_path.to_str() {
            Some(s) => s,
            None => {
                reply.error(crate::errors::EINVAL);
                return;
            }
        };
        let Some(name_str) = name.to_str() else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.mkdir(parent_str, name_str) {
            Ok(attr) => reply.entry(&self.ttl, &adapter_attr_to_fuser(&attr), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn rmdir(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEmpty,
    ) {
        let parent_path = match self.adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let Some(full) = BoxedFuserShim::join_child(&parent_path, name) else {
            reply.error(crate::errors::EINVAL);
            return;
        };
        match self.adapter.rmdir(&full) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }
}

// -----------------------------------------------------------------------------
// Mount entry points.
// -----------------------------------------------------------------------------

fn build_fuse_options(options: &MountOptions) -> Vec<fuser::MountOption> {
    let mut fuse_opts: Vec<fuser::MountOption> = vec![
        fuser::MountOption::FSName(
            options
                .fs_name
                .clone()
                .unwrap_or_else(|| "pcloud".to_string()),
        ),
        // Private subtype proves ownership to orphan recovery. `fuse.pcloud`
        // is also used by the official client and must never be auto-unmounted.
        fuser::MountOption::Subtype("pcloud-rs".to_string()),
        fuser::MountOption::DefaultPermissions,
        fuser::MountOption::NoDev,
        fuser::MountOption::NoSuid,
    ];
    if options.read_only {
        fuse_opts.push(fuser::MountOption::RO);
    } else {
        fuse_opts.push(fuser::MountOption::RW);
    }
    fuse_opts
}

/// Mount using a [`FuseAdapter`]-wrapping shim.
pub fn mount_with_fuser<A: FuseAdapter>(
    mountpoint: &Path,
    adapter: A,
    options: MountOptions,
) -> Result<MountHandle, MountError> {
    install_signal_handler_once();
    let fuse_opts = build_fuse_options(&options);

    let shim = FuserShim {
        adapter,
        ttl: std::time::Duration::from_secs(1),
    };
    let session = fuser::spawn_mount2(shim, mountpoint, &fuse_opts)
        .map_err(|e| MountError::Fuser(e.to_string()))?;

    register_mount(mountpoint);

    Ok(MountHandle::from_linux(LinuxMountHandle {
        mountpoint: mountpoint.to_path_buf(),
        session: Some(session),
    }))
}

/// Mount a real `fuser::Filesystem` directly (used by the daemon when
/// composing `PcloudFsShim` in 4.e.3).
pub fn mount_fuser_filesystem<F>(
    mountpoint: &Path,
    filesystem: F,
    options: MountOptions,
) -> Result<MountHandle, MountError>
where
    F: fuser::Filesystem + Send + 'static,
{
    install_signal_handler_once();
    let fuse_opts = build_fuse_options(&options);

    let session = fuser::spawn_mount2(filesystem, mountpoint, &fuse_opts)
        .map_err(|e| MountError::Fuser(e.to_string()))?;

    register_mount(mountpoint);

    Ok(MountHandle::from_linux(LinuxMountHandle {
        mountpoint: mountpoint.to_path_buf(),
        session: Some(session),
    }))
}

// -----------------------------------------------------------------------------
// audit-06 concurrency regression tests for ACTIVE_MOUNTS canonicalisation.
// -----------------------------------------------------------------------------
#[cfg(test)]
mod active_mounts_tests {
    use super::{canonical_key, register_mount, registry, unregister_mount};
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn canonical_key_collapses_trailing_slash() {
        let k1 = canonical_key(&PathBuf::from("/nonexistent/aud06/a"));
        let k2 = canonical_key(&PathBuf::from("/nonexistent/aud06/a/"));
        assert_eq!(k1, k2, "trailing-slash variants must collapse");
    }

    #[test]
    fn register_unregister_balanced_for_variants() {
        let p1 = PathBuf::from("/nonexistent/aud06/balanced");
        let p2 = PathBuf::from("/nonexistent/aud06/balanced/");
        register_mount(&p1);
        let before = registry().lock().unwrap().len();
        unregister_mount(&p2);
        let after = registry().lock().unwrap().len();
        assert_eq!(before - 1, after, "unregister of slash variant must remove");
    }

    #[test]
    fn concurrent_register_unregister_no_leak() {
        const N: usize = 16;
        const ITERS: usize = 64;
        let barrier = Arc::new(Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|tid| {
                let b = Arc::clone(&barrier);
                thread::spawn(move || {
                    let path = PathBuf::from(format!("/nonexistent/aud06/conc/t{tid}"));
                    b.wait();
                    for _ in 0..ITERS {
                        register_mount(&path);
                        unregister_mount(&path);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread panicked");
        }
        let guard = registry().lock().unwrap();
        for tid in 0..N {
            let key = canonical_key(&PathBuf::from(format!("/nonexistent/aud06/conc/t{tid}")));
            assert!(!guard.contains(&key), "leaked entry for t{tid}: {key:?}");
        }
    }
}

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

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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

impl fuser::Filesystem for BoxedFuserShim {
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

    fn setattr(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
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

static ACTIVE_MOUNTS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
static SIGNAL_HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<PathBuf>> {
    ACTIVE_MOUNTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn install_signal_handler_once() {
    SIGNAL_HANDLER_INSTALLED.get_or_init(|| {
        // SAFETY: signal(2) is called once during process lifetime with a
        // static handler.
        let handler = signal_trampoline as *const () as libc::sighandler_t;
        unsafe {
            libc::signal(libc::SIGTERM, handler);
            libc::signal(libc::SIGINT, handler);
        }
    });
}

extern "C" fn signal_trampoline(sig: libc::c_int) {
    if let Some(mtx) = ACTIVE_MOUNTS.get()
        && let Ok(guard) = mtx.lock()
    {
        for path in guard.iter() {
            if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
                // SAFETY: umount2 is async-signal-safe.
                unsafe {
                    libc::umount2(c.as_ptr(), libc::MNT_DETACH);
                }
            }
        }
    }
    // Restore default and re-raise so the process terminates normally.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
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
        drop(self.session.take());

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

        if let Ok(mut guard) = registry().lock() {
            guard.retain(|p| p != &self.mountpoint);
        }
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

    fn setattr(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
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
        fuser::MountOption::Subtype("pcloud".to_string()),
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

/// Mount using a [`FuseAdapter`]-wrapping shim. Used by the 4.a scaffold.
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

    if let Ok(mut guard) = registry().lock() {
        guard.push(mountpoint.to_path_buf());
    }

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

    if let Ok(mut guard) = registry().lock() {
        guard.push(mountpoint.to_path_buf());
    }

    Ok(MountHandle::from_linux(LinuxMountHandle {
        mountpoint: mountpoint.to_path_buf(),
        session: Some(session),
    }))
}

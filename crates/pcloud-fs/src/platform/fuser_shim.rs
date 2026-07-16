//! **PLATFORM: FreeBSD, NetBSD, OpenBSD, and DragonFlyBSD.**
//! **GATING:** the BSD `cfg` at the `mod fuser_shim;` line in
//! `platform/mod.rs`.
//!
//! Shared `fuser::Filesystem` shims and adapter-type conversions used by
//! the BSD libfuse/refuse mount back-ends. The
//! `fuser` crate exposes the same `Filesystem` trait on both platforms;
//! only the underlying native library differs (selected via `fuser` cargo
//! features at build time). The shim code itself is byte-identical, so
//! keeping a single authoritative copy here prevents the Linux and BSD
//! paths from drifting.

use crate::fuse_adapter::FuseAdapter;

// ENOTSUP / EOPNOTSUPP: "operation not supported". Used instead of ENOSYS
// ("no such syscall") when the operation is understood but inapplicable to
// this filesystem (e.g. access(2) and chmod/chown on a filesystem with no
// Unix permission bits).
//
// On Linux/glibc, ENOTSUP == EOPNOTSUPP == 95.
// On FreeBSD, ENOTSUP == 45 and EOPNOTSUPP == 45. We use the libc constant
// to stay portable.
const ENOTSUP: i32 = libc::EOPNOTSUPP;

/// Convert the adapter-level `FsEntryKind` to `fuser::FileType`.
pub(crate) fn adapter_kind_to_fuser(k: crate::fuse_adapter::FsEntryKind) -> fuser::FileType {
    use crate::fuse_adapter::FsEntryKind;
    match k {
        FsEntryKind::Directory => fuser::FileType::Directory,
        FsEntryKind::RegularFile => fuser::FileType::RegularFile,
        FsEntryKind::Symlink => fuser::FileType::Symlink,
    }
}

/// Convert the adapter-level `EntryAttr` to `fuser::FileAttr`.
pub(crate) fn adapter_attr_to_fuser(a: &crate::fuse_adapter::EntryAttr) -> fuser::FileAttr {
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

// -----------------------------------------------------------------------------
// BoxedFuserShim: wraps `Box<dyn FuseAdapter>` for the dyn dispatch path.
// -----------------------------------------------------------------------------

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
pub(crate) struct BoxedFuserShim {
    adapter: Box<dyn FuseAdapter>,
    ttl: std::time::Duration,
}

impl BoxedFuserShim {
    pub(crate) fn new(adapter: Box<dyn FuseAdapter>) -> Self {
        Self {
            adapter,
            ttl: std::time::Duration::from_secs(1),
        }
    }

    fn path_for(&self, ino: u64) -> Option<std::path::PathBuf> {
        self.adapter.resolve_ino_to_path(ino).ok()
    }

    pub(crate) fn join_child(parent: &std::path::Path, name: &std::ffi::OsStr) -> Option<String> {
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

    /// access(2) is not meaningful on pCloud because there are no per-file
    /// Unix permission bits. Return ENOTSUP so callers fall back to
    /// getattr UID/GID checks via the kernel's default permission logic.
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

    /// setattr: chmod/chown return ENOTSUP (pCloud has no Unix permission
    /// bits). utimens is accepted as a no-op to satisfy editors that update
    /// mtime. Size changes are forwarded to the adapter's truncate method.
    ///
    /// // pCloud does not support Unix permission bits. chmod/chown return
    /// // ENOTSUP; utimens is accepted as a no-op to satisfy editors that
    /// // update mtime.
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

    /// forget: called by the kernel when it releases its reference to an inode
    /// lookup. Decrements the lookup reference count via the adapter and evicts
    /// the inode map entry when it reaches zero, preventing unbounded growth.
    fn forget(&mut self, _req: &fuser::Request<'_>, ino: u64, nlookup: u64) {
        self.adapter.forget_ino(ino, nlookup);
        log::trace!("forget ino={} nlookup={}", ino, nlookup);
    }

    // pCloud does not support extended attributes; return ENOTSUP (not ENOSYS)
    // for better desktop-env compatibility. ENOTSUP signals that xattr is
    // understood but unsupported by this filesystem.
    fn getxattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _name: &std::ffi::OsStr,
        _size: u32,
        reply: fuser::ReplyXattr,
    ) {
        reply.error(ENOTSUP);
    }

    fn setxattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _name: &std::ffi::OsStr,
        _value: &[u8],
        _flags: i32,
        _position: u32,
        reply: fuser::ReplyEmpty,
    ) {
        reply.error(ENOTSUP);
    }

    fn listxattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _size: u32,
        reply: fuser::ReplyXattr,
    ) {
        reply.error(ENOTSUP);
    }

    fn removexattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _name: &std::ffi::OsStr,
        reply: fuser::ReplyEmpty,
    ) {
        reply.error(ENOTSUP);
    }

    // pCloud has no symlink, hard-link, or fallocate support; return ENOTSUP
    // for desktop-env compatibility rather than ENOSYS.
    fn readlink(&mut self, _req: &fuser::Request<'_>, _ino: u64, reply: fuser::ReplyData) {
        reply.error(ENOTSUP);
    }

    fn symlink(
        &mut self,
        _req: &fuser::Request<'_>,
        _parent: u64,
        _name: &std::ffi::OsStr,
        _link: &std::path::Path,
        reply: fuser::ReplyEntry,
    ) {
        reply.error(ENOTSUP);
    }

    fn link(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _newparent: u64,
        _newname: &std::ffi::OsStr,
        reply: fuser::ReplyEntry,
    ) {
        reply.error(ENOTSUP);
    }

    fn fallocate(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _length: i64,
        _mode: i32,
        reply: fuser::ReplyEmpty,
    ) {
        reply.error(ENOTSUP);
    }
}

// -----------------------------------------------------------------------------
// FuserShim<A>: monomorphized generic variant for the typed dispatch path.
// -----------------------------------------------------------------------------

/// Generic shim that implements `fuser::Filesystem` by delegating through
/// the [`FuseAdapter`] trait. Used by [`mount_with_fuser`]-style entry
/// points where the adapter type is known at compile time (avoids the
/// `Box<dyn>` indirection used by [`BoxedFuserShim`]).
#[allow(dead_code)] // retained for typed third-party adapters; daemon uses PcloudFsShim
pub(crate) struct FuserShim<A: FuseAdapter> {
    pub(crate) adapter: A,
    pub(crate) ttl: std::time::Duration,
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

    /// access(2) is not meaningful on pCloud because there are no per-file
    /// Unix permission bits. Return ENOTSUP so callers fall back to
    /// getattr UID/GID checks via the kernel's default permission logic.
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

    /// forget: called by the kernel when it releases its reference to an inode
    /// lookup. Decrements the lookup reference count via the adapter and evicts
    /// the inode map entry when it reaches zero, preventing unbounded growth.
    fn forget(&mut self, _req: &fuser::Request<'_>, ino: u64, nlookup: u64) {
        self.adapter.forget_ino(ino, nlookup);
        log::trace!("forget ino={} nlookup={}", ino, nlookup);
    }

    // pCloud does not support extended attributes; return ENOTSUP (not ENOSYS)
    // for better desktop-env compatibility.
    fn getxattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _name: &std::ffi::OsStr,
        _size: u32,
        reply: fuser::ReplyXattr,
    ) {
        reply.error(ENOTSUP);
    }

    fn setxattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _name: &std::ffi::OsStr,
        _value: &[u8],
        _flags: i32,
        _position: u32,
        reply: fuser::ReplyEmpty,
    ) {
        reply.error(ENOTSUP);
    }

    fn listxattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _size: u32,
        reply: fuser::ReplyXattr,
    ) {
        reply.error(ENOTSUP);
    }

    fn removexattr(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _name: &std::ffi::OsStr,
        reply: fuser::ReplyEmpty,
    ) {
        reply.error(ENOTSUP);
    }

    // pCloud has no symlink, hard-link, or fallocate support; return ENOTSUP
    // for desktop-env compatibility rather than ENOSYS.
    fn readlink(&mut self, _req: &fuser::Request<'_>, _ino: u64, reply: fuser::ReplyData) {
        reply.error(ENOTSUP);
    }

    fn symlink(
        &mut self,
        _req: &fuser::Request<'_>,
        _parent: u64,
        _name: &std::ffi::OsStr,
        _link: &std::path::Path,
        reply: fuser::ReplyEntry,
    ) {
        reply.error(ENOTSUP);
    }

    fn link(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _newparent: u64,
        _newname: &std::ffi::OsStr,
        reply: fuser::ReplyEntry,
    ) {
        reply.error(ENOTSUP);
    }

    fn fallocate(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _length: i64,
        _mode: i32,
        reply: fuser::ReplyEmpty,
    ) {
        reply.error(ENOTSUP);
    }
}

/// Drop a `fuser::BackgroundSession` with a bounded timeout.
///
/// `fuser::BackgroundSession::drop` joins the internal dispatcher thread.
/// If the kernel has stalled the FUSE session (e.g. a blocked syscall inside
/// the mount, or a kernel bug), the join would block forever. This helper
/// spawns a scoped thread to do the drop and waits at most `timeout` for it
/// to complete. If the timeout expires, a warning is logged and the thread is
/// left to finish in the background (the handle is intentionally leaked so
/// the OS reclaims it at process exit).
///
/// Call sites in `platform/linux.rs` should prefer `drop_session_bounded`
/// over a bare `drop(session)` to avoid hang-on-unmount scenarios.
#[allow(dead_code)]
pub(crate) fn drop_session_bounded(
    session: fuser::BackgroundSession,
    timeout: std::time::Duration,
) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        drop(session);
        let _ = tx.send(());
    });
    match rx.recv_timeout(timeout) {
        Ok(_) => {}
        Err(_) => {
            log::warn!(
                "FUSE dispatcher thread did not exit within {}s; possible resource leak",
                timeout.as_secs()
            );
        }
    }
}

/// Build the shared `fuser::MountOption` list for both the Linux and
/// FreeBSD mount paths. The options set is identical: `FSName`/`Subtype`
/// for identification in `/proc/mounts` or `getmntinfo(3)`,
/// `DefaultPermissions` so the kernel enforces mode checks,
/// `NoDev`/`NoSuid` for hardening, and `RO`/`RW` per the caller's
/// `MountOptions::read_only`.
pub(crate) fn build_fuse_options(
    options: &crate::mount_service::MountOptions,
) -> Vec<fuser::MountOption> {
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
        // Wire max_readahead so the kernel respects the caller's readahead
        // budget. The fuser crate exposes this as a custom option string.
        fuser::MountOption::CUSTOM(format!("max_readahead={}", options.max_readahead)),
    ];
    if options.read_only {
        fuse_opts.push(fuser::MountOption::RO);
    } else {
        fuse_opts.push(fuser::MountOption::RW);
    }
    fuse_opts
}

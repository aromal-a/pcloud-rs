//! `fuser::Filesystem` shim (bd-1du.4.e, sub-task 1).
//!
//! Delegates every FUSE kernel op to the already-landed services:
//! - `lookup` / `getattr` / `readdir` → [`FuseAdapter`] (4.b)
//! - `open` / `read` / `release`      → [`FuseAdapter`] handle table (4.c)
//! - `create` / `write` / `flush` / `fsync` / `setattr(size)`
//!   / `unlink` / `rename`            → [`WritePathService`]      (4.d)
//!
//! This module is **Linux-only** (the `fuser` crate itself is). It does not
//! spawn any FUSE session; wiring it to a real kernel mount is the job of
//! sub-task 2 in `pcloud-daemon`. Tests here only exercise the trait
//! boundary with mock backends.

#![cfg(target_os = "linux")]

// **PLATFORM:** Linux
// **GATING:** none (portable; uses Linux-only idioms — see TODO(bd-xplat)).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

// TODO(bd-xplat): Linux-only — needs cfg gate or platform trait abstraction. See PLAN_CROSSPLATFORM.md §2.
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};

use crate::backend::{FileBackend, FolderBackend};
use crate::errors::{EINVAL, ENOENT};
use crate::fuse_adapter::{
    DirEntry, EntryAttr, FileHandleId, FsEntryKind, FuseAdapter, ProtoFuseAdapter,
};
use crate::inode::InodeTable;
use crate::write_path::{FileUploadBackend, WritePathService};

/// Default TTL for cached `lookup` / `getattr` replies handed to the kernel.
const DEFAULT_TTL: Duration = Duration::from_secs(1);

/// Entry in the shim's fh → state table.
#[derive(Debug)]
struct FhEntry {
    ino: u64,
    /// The read-side handle id returned by [`ProtoFuseAdapter::open`], if
    /// the file was opened for reading.
    read_handle: Option<FileHandleId>,
    /// `true` if the [`WritePathService`] has an open write handle for
    /// `ino`. `release` tears this down.
    write_open: bool,
}

/// Generic Fh → FhEntry table with atomic id allocation.
#[derive(Debug, Default)]
struct FhTable {
    entries: Mutex<HashMap<u64, FhEntry>>,
    next: AtomicU64,
}

impl FhTable {
    fn allocate(&self, e: FhEntry) -> Option<u64> {
        // Start at 1; 0 is reserved (fuser uses 0 as "no fh").
        let mut id = self.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if id == 0 {
            id = self.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        }
        self.entries.lock().ok()?.insert(id, e);
        Some(id)
    }

    fn get_snapshot(&self, fh: u64) -> Option<(u64, Option<FileHandleId>, bool)> {
        self.entries
            .lock()
            .ok()?
            .get(&fh)
            .map(|e| (e.ino, e.read_handle, e.write_open))
    }

    fn remove(&self, fh: u64) -> Option<FhEntry> {
        self.entries.lock().ok()?.remove(&fh)
    }
}

/// Composite FUSE filesystem implementation. Holds an [`Arc`] to each
/// delegate so the shim is cheaply cloneable for alternative mount
/// harnesses and for testing.
pub struct PcloudFsShim<B, F, U>
where
    B: FolderBackend,
    F: FileBackend,
    U: FileUploadBackend,
{
    adapter: Arc<ProtoFuseAdapter<B, F>>,
    writer: Arc<WritePathService<U>>,
    inodes: Arc<InodeTable>,
    fhs: Arc<FhTable>,
    ttl: Duration,
}

impl<B, F, U> PcloudFsShim<B, F, U>
where
    B: FolderBackend,
    F: FileBackend,
    U: FileUploadBackend,
{
    /// Construct a shim from the three already-wired services. Inodes are
    /// taken from the adapter's shared [`InodeTable`] so that write-side
    /// path lookups agree with read-side lookups.
    pub fn new(adapter: Arc<ProtoFuseAdapter<B, F>>, writer: Arc<WritePathService<U>>) -> Self {
        let inodes = adapter.inode_table();
        Self {
            adapter,
            writer,
            inodes,
            fhs: Arc::new(FhTable::default()),
            ttl: DEFAULT_TTL,
        }
    }

    /// Override the lookup/attribute TTL passed to the kernel.
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Number of currently-registered file handles. Test helper.
    #[must_use]
    pub fn open_fh_count(&self) -> usize {
        self.fhs.entries.lock().map(|t| t.len()).unwrap_or(0)
    }

    fn path_for(&self, ino: u64) -> Option<String> {
        self.inodes.resolve(ino).map(|(p, _, _)| p)
    }

    fn join_child(parent: &str, name: &OsStr) -> Option<String> {
        let n = name.to_str()?;
        if n.is_empty() || n.contains('/') || n.contains('\0') {
            return None;
        }
        Some(if parent == "/" {
            format!("/{n}")
        } else {
            format!("{parent}/{n}")
        })
    }

    /// Read the entire contents of an opened file via the adapter's read
    /// path. Short reads mean EOF; the loop stops then. Returns a single
    /// `errno` on the first non-EOF error. Called by `open` when seeding
    /// the staging blob for a writable open on an existing remote file.
    fn read_whole_file(&self, handle: FileHandleId) -> Result<Vec<u8>, i32> {
        const CHUNK: usize = 1024 * 1024;
        let mut out = Vec::new();
        let mut offset: u64 = 0;
        loop {
            let bytes = self.adapter.read(handle, offset, CHUNK)?;
            if bytes.is_empty() {
                break;
            }
            let n = bytes.len() as u64;
            out.extend_from_slice(&bytes);
            offset = offset.saturating_add(n);
            if bytes.len() < CHUNK {
                break;
            }
        }
        Ok(out)
    }
}

fn map_kind(kind: FsEntryKind) -> FileType {
    match kind {
        FsEntryKind::Directory => FileType::Directory,
        FsEntryKind::RegularFile => FileType::RegularFile,
        FsEntryKind::Symlink => FileType::Symlink,
    }
}

fn file_attr_from(entry: &EntryAttr) -> FileAttr {
    let now = SystemTime::now();
    let backend_mtime = entry
        .mtime_epoch
        .map(|secs| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
    let mtime = backend_mtime.unwrap_or(now);
    FileAttr {
        ino: entry.ino,
        size: entry.size,
        blocks: entry.size.div_ceil(512),
        atime: mtime,
        mtime,
        ctime: mtime,
        crtime: mtime,
        kind: map_kind(entry.kind),
        perm: entry.mode,
        nlink: 1,
        uid: entry.uid,
        gid: entry.gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

impl<B, F, U> Filesystem for PcloudFsShim<B, F, U>
where
    B: FolderBackend,
    F: FileBackend,
    U: FileUploadBackend,
{
    /// Called by fuser once the FUSE session is established, before any kernel
    /// requests are dispatched. We replay the on-disk write journal here so
    /// that any writes acknowledged before a crash are recovered before new
    /// kernel ops arrive. A failed replay is logged but does not abort the
    /// mount — the operator can recover by inspecting the staging directory.
    fn init(
        &mut self,
        _req: &Request<'_>,
        _config: &mut fuser::KernelConfig,
    ) -> std::result::Result<(), libc::c_int> {
        match self.writer.replay_journal() {
            Ok(records) if !records.is_empty() => {
                log::info!(
                    "pcloud-fs: journal replay recovered {} record(s) on mount",
                    records.len()
                );
            }
            Ok(_) => {}
            Err(e) => {
                log::error!(
                    "pcloud-fs: journal replay failed on startup: {e} — data may be inconsistent"
                );
                // Do not abort mount; log and continue so the user can recover.
            }
        }
        Ok(())
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let Some(name_str) = name.to_str() else {
            reply.error(EINVAL);
            return;
        };
        match self.adapter.lookup(parent, name_str) {
            Ok(attr) => reply.entry(&self.ttl, &file_attr_from(&attr), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        match self.adapter.getattr(ino) {
            Ok(attr) => reply.attr(&self.ttl, &file_attr_from(&attr)),
            Err(errno) => reply.error(errno),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let entries: Vec<DirEntry> = match self.adapter.readdir(ino, offset) {
            Ok(v) => v,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        // Synthesise `.` and `..` only when offset == 0 so repeated calls
        // with non-zero offset do not duplicate them.
        let mut next_off = offset + 1;
        if offset == 0 {
            if reply.add(ino, next_off, FileType::Directory, ".") {
                reply.ok();
                return;
            }
            next_off += 1;
            let parent = parent_ino_of(&self.inodes, ino).unwrap_or(ino);
            if reply.add(parent, next_off, FileType::Directory, "..") {
                reply.ok();
                return;
            }
            next_off += 1;
        }
        for entry in entries {
            if reply.add(entry.ino, next_off, map_kind(entry.kind), &entry.name) {
                break;
            }
            next_off += 1;
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        // Decode access mode + O_APPEND / O_TRUNC. For read-only opens the
        // path is unchanged. For writable opens we also create a write
        // handle pre-seeded with the current remote bytes so append/rw on
        // existing files behaves correctly.
        let acc = flags & libc::O_ACCMODE;
        let want_write = acc == libc::O_WRONLY || acc == libc::O_RDWR;
        let append_mode = (flags & libc::O_APPEND) != 0;
        let trunc = (flags & libc::O_TRUNC) != 0;

        // Read handle (every open gets one; pure-O_WRONLY still returns it
        // so the fh is valid for operations the kernel may issue). Pure
        // O_WRONLY is allowed to fail only if the file genuinely can't be
        // opened for read — we treat that as non-fatal for writable opens.
        let read_handle = match self.adapter.open(ino) {
            Ok(h) => Some(h),
            Err(errno) if !want_write => {
                reply.error(errno);
                return;
            }
            Err(_) => None,
        };

        if want_write {
            // Seed the staging blob with current remote content (skipped
            // when O_TRUNC is set — caller explicitly asked for empty).
            let existing_bytes = if trunc {
                Vec::new()
            } else if let Some(rh) = read_handle {
                // Pull the whole remote file into memory. Small files are
                // fine; for huge files this is a known limitation — the
                // correct long-term fix is a lazy copy-on-write that only
                // downloads pages that get modified.
                match self.read_whole_file(rh) {
                    Ok(b) => b,
                    Err(errno) => {
                        reply.error(errno);
                        return;
                    }
                }
            } else {
                Vec::new()
            };

            let Some(path) = self.path_for(ino) else {
                reply.error(ENOENT);
                return;
            };
            if let Err(e) = self.writer.seed_blob(ino, &existing_bytes) {
                reply.error(e.to_errno());
                return;
            }
            if let Err(e) = self.writer.open_for_write(ino, path, append_mode, trunc) {
                reply.error(e.to_errno());
                return;
            }
            let Some(fh) = self.fhs.allocate(FhEntry {
                ino,
                read_handle,
                write_open: true,
            }) else {
                reply.error(libc::EIO);
                return;
            };
            reply.opened(fh, 0);
        } else {
            let Some(fh) = self.fhs.allocate(FhEntry {
                ino,
                read_handle,
                write_open: false,
            }) else {
                reply.error(libc::EIO);
                return;
            };
            reply.opened(fh, 0);
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let Some((_ino, Some(read_h), _)) = self.fhs.get_snapshot(fh) else {
            reply.error(crate::fuse_adapter::EBADF);
            return;
        };
        let off = offset.max(0) as u64;
        match self.adapter.read(read_h, off, size as usize) {
            Ok(bytes) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let Some(entry) = self.fhs.remove(fh) else {
            reply.error(crate::fuse_adapter::EBADF);
            return;
        };
        if let Some(h) = entry.read_handle {
            let _ = self.adapter.release(h);
        }
        if entry.write_open {
            self.writer.release(ino);
            // After a write-handle close, the server now holds a canonical
            // copy of the file with its own remote file_id. Our local inode
            // has no file_id registered, so subsequent kernel reads would
            // fail to open. Invalidate the parent dir cache so the next
            // readdir/lookup re-fetches from the backend and adopts the
            // server-assigned file_id.
            if let Some(path) = self.path_for(ino) {
                if let Some(slash) = path.rfind('/') {
                    let parent = if slash == 0 { "/" } else { &path[..slash] };
                    self.adapter.invalidate_cache(parent);
                    self.adapter.invalidate_cache(&path);
                }
            }
        }
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(ENOENT);
            return;
        };
        let Some(full_path) = Self::join_child(&parent_path, name) else {
            reply.error(EINVAL);
            return;
        };
        // Allocate an inode for the new file deterministically via the
        // shared inode table.
        let (ino, _gen) = match self
            .inodes
            .insert_or_get(&full_path, FsEntryKind::RegularFile)
        {
            Ok(pair) => pair,
            Err(e) => {
                reply.error(e.to_errno());
                return;
            }
        };
        let name_str = name.to_str().unwrap_or("");
        if let Err(e) = self.writer.create(ino, &parent_path, name_str) {
            reply.error(e.to_errno());
            return;
        }
        let attr = EntryAttr {
            ino,
            kind: FsEntryKind::RegularFile,
            size: 0,
            mode: self.adapter.options().file_mode,
            uid: self.adapter.options().uid,
            gid: self.adapter.options().gid,
            mtime_epoch: None,
            mtime_nsec: 0,
        };
        // Publish to the adapter's metadata cache so subsequent
        // lookup/getattr/readdir calls can resolve the locally-created
        // file without hitting the backend (which has no knowledge of
        // pending writes).
        self.adapter
            .publish_local_entry(&parent_path, name_str, attr.clone());
        let Some(fh) = self.fhs.allocate(FhEntry {
            ino,
            read_handle: None,
            write_open: true,
        }) else {
            reply.error(libc::EIO);
            return;
        };
        reply.created(&self.ttl, &file_attr_from(&attr), 0, fh, 0);
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        // The ino parameter is authoritative for FUSE; the fh just has to
        // be a valid registered handle.
        if self.fhs.get_snapshot(fh).is_none() {
            reply.error(crate::fuse_adapter::EBADF);
            return;
        }
        let off = offset.max(0) as u64;
        match self.writer.write(ino, off, data) {
            Ok(n) => {
                // Publish new length so getattr reflects the pending write
                // before the final save-to-pcloud is done. Use `off + n`
                // since writes may be non-contiguous (sparse) and this is
                // the furthest byte written so far.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .ok();
                self.adapter.publish_local_size(ino, off + n as u64, now);
                reply.written(n as u32);
            }
            Err(e) => reply.error(e.to_errno()),
        }
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        // `flush` may arrive on a read-only fh too; only drive the writer
        // if this fh actually has a write handle.
        let is_write = self
            .fhs
            .get_snapshot(fh)
            .map(|(_, _, w)| w)
            .unwrap_or(false);
        if !is_write {
            reply.ok();
            return;
        }
        match self.writer.flush(ino) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e.to_errno()),
        }
    }

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        match self.writer.fsync(ino) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e.to_errno()),
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        if let Some(new_size) = size {
            if let Err(e) = self.writer.truncate(ino, new_size) {
                reply.error(e.to_errno());
                return;
            }
        }
        match self.adapter.getattr(ino) {
            Ok(mut attr) => {
                if let Some(s) = size {
                    attr.size = s;
                }
                reply.attr(&self.ttl, &file_attr_from(&attr));
            }
            Err(errno) => reply.error(errno),
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(ENOENT);
            return;
        };
        let Some(name_str) = name.to_str() else {
            reply.error(EINVAL);
            return;
        };
        if name_str.is_empty() || name_str.contains('/') || name_str.contains('\0') {
            reply.error(EINVAL);
            return;
        }
        match self.adapter.mkdir(&parent_path, name_str) {
            Ok(attr) => reply.entry(&self.ttl, &file_attr_from(&attr), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(ENOENT);
            return;
        };
        let Some(full) = Self::join_child(&parent_path, name) else {
            reply.error(EINVAL);
            return;
        };
        match self.adapter.rmdir(&full) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(ENOENT);
            return;
        };
        let Some(full) = Self::join_child(&parent_path, name) else {
            reply.error(EINVAL);
            return;
        };
        let ino = self.inodes.ino_for_path(&full);
        let name_str = name.to_str().unwrap_or("");
        match self.writer.unlink(ino, &full) {
            Ok(()) => {
                self.inodes.invalidate_path(&full);
                self.adapter.forget_local_entry(&parent_path, name_str);
                reply.ok();
            }
            Err(e) => reply.error(e.to_errno()),
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let Some(from_parent) = self.path_for(parent) else {
            reply.error(ENOENT);
            return;
        };
        let Some(to_parent) = self.path_for(newparent) else {
            reply.error(ENOENT);
            return;
        };
        let Some(from) = Self::join_child(&from_parent, name) else {
            reply.error(EINVAL);
            return;
        };
        let Some(to) = Self::join_child(&to_parent, newname) else {
            reply.error(EINVAL);
            return;
        };
        match self.writer.rename(&from, &to) {
            Ok(()) => {
                let from_name = name.to_str().unwrap_or("");
                let new_name = newname.to_str().unwrap_or("");
                self.adapter.forget_local_entry(&from_parent, from_name);
                // Best-effort: if the source path had a cached attr, carry
                // it over as the destination's cached attr. Otherwise the
                // next lookup falls through to the backend.
                if let Some(attr) = self.adapter.cached_attr(&from) {
                    let moved = EntryAttr {
                        ino: self.inodes.ino_for_path(&to).unwrap_or(attr.ino),
                        ..attr
                    };
                    self.adapter
                        .publish_local_entry(&to_parent, new_name, moved);
                }
                reply.ok();
            }
            Err(e) => reply.error(e.to_errno()),
        }
    }
}

/// Best-effort parent inode resolver. Returns the inode of the parent path
/// if it is registered; otherwise returns [`None`].
fn parent_ino_of(inodes: &InodeTable, ino: u64) -> Option<u64> {
    let (path, _, _) = inodes.resolve(ino)?;
    if path == "/" {
        return Some(ino);
    }
    let idx = path.rfind('/')?;
    let parent = if idx == 0 {
        "/".to_owned()
    } else {
        path[..idx].to_owned()
    };
    inodes.ino_for_path(&parent)
}

#[cfg(test)]
mod tests {
    //! Tests exercise the trait-boundary only: we construct the shim with
    //! mock folder/file/upload backends and drive a few representative
    //! FUSE paths indirectly through the adapter's typed API. We do not
    //! spin up a real kernel mount — the `fuser::Filesystem` impl is only
    //! required to compile cleanly and hold the delegate invariants.

    use super::*;
    use crate::backend::mock::{MockFileBackend, MockFolderBackend};
    use crate::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
    use crate::page_cache::PageCacheConfig;
    use crate::staging::StagingDir;
    use crate::write_journal::WriteJournal;
    use crate::write_path::{WritePathOptions, WritePathService, mock::MockUploadBackend};
    use std::sync::Arc;
    use std::time::Duration as StdDuration;
    use tempfile::tempdir;

    fn build_shim() -> (
        PcloudFsShim<MockFolderBackend, MockFileBackend, MockUploadBackend>,
        Arc<MockFolderBackend>,
        Arc<MockFileBackend>,
        Arc<MockUploadBackend>,
        tempfile::TempDir,
    ) {
        let folder = Arc::new(MockFolderBackend::new());
        folder.insert_dir(
            "/",
            1,
            vec![
                ("docs", true, Some(2), None),
                ("data.bin", false, None, Some(100)),
            ],
        );
        folder.insert_dir("/docs", 2, vec![("note.md", false, None, Some(101))]);
        let files = Arc::new(MockFileBackend::new());
        files.insert_file(100, (0..64u8).collect());
        files.insert_file(101, b"hello".to_vec());

        let adapter = Arc::new(ProtoFuseAdapter::with_file_backend(
            Arc::clone(&folder),
            Arc::clone(&files),
            AdapterOptions {
                page_cache: PageCacheConfig {
                    page_size: 16,
                    max_bytes: 1024,
                },
                ..AdapterOptions::default()
            },
        ));

        let tmp = tempdir().unwrap();
        let stage = StagingDir::open(tmp.path().join("stage")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let upload = Arc::new(MockUploadBackend::new());
        let writer = Arc::new(WritePathService::new(
            stage,
            journal,
            Arc::clone(&upload),
            WritePathOptions {
                flush_threshold_bytes: 1024 * 1024,
                flush_interval: StdDuration::from_secs(3600),
                ..WritePathOptions::default()
            },
        ));

        let shim = PcloudFsShim::new(Arc::clone(&adapter), Arc::clone(&writer));
        (shim, folder, files, upload, tmp)
    }

    #[test]
    fn shim_constructs_and_implements_filesystem_trait() {
        // Compile-time check: PcloudFsShim implements fuser::Filesystem.
        fn assert_fs<T: Filesystem>(_t: &T) {}
        let (shim, _, _, _, _tmp) = build_shim();
        assert_fs(&shim);
        assert_eq!(shim.open_fh_count(), 0);
    }

    #[test]
    fn fh_table_allocates_unique_ids_and_skips_zero() {
        let tbl = FhTable::default();
        // Force wrap-around edge: set next to u64::MAX so the +1 wraps to 0.
        tbl.next.store(u64::MAX, Ordering::Relaxed);
        let fh = tbl
            .allocate(FhEntry {
                ino: 1,
                read_handle: None,
                write_open: false,
            })
            .expect("allocate fh");
        assert_ne!(fh, 0, "fh=0 must never be handed out");
    }

    #[test]
    fn fh_table_snapshot_and_remove_roundtrip() {
        let tbl = FhTable::default();
        let fh = tbl
            .allocate(FhEntry {
                ino: 7,
                read_handle: Some(42),
                write_open: true,
            })
            .expect("allocate fh");
        let snap = tbl.get_snapshot(fh).unwrap();
        assert_eq!(snap, (7u64, Some(42u64), true));
        let removed = tbl.remove(fh).unwrap();
        assert_eq!(removed.ino, 7);
        assert!(tbl.get_snapshot(fh).is_none());
    }

    #[test]
    fn join_child_rejects_empty_and_slashes_and_nul() {
        assert!(
            PcloudFsShim::<MockFolderBackend, MockFileBackend, MockUploadBackend>::join_child(
                "/",
                OsStr::new(""),
            )
            .is_none()
        );
        assert!(
            PcloudFsShim::<MockFolderBackend, MockFileBackend, MockUploadBackend>::join_child(
                "/",
                OsStr::new("a/b"),
            )
            .is_none()
        );
        assert!(
            PcloudFsShim::<MockFolderBackend, MockFileBackend, MockUploadBackend>::join_child(
                "/",
                OsStr::new("x\0y"),
            )
            .is_none()
        );
        assert_eq!(
            PcloudFsShim::<MockFolderBackend, MockFileBackend, MockUploadBackend>::join_child(
                "/",
                OsStr::new("f.txt"),
            ),
            Some("/f.txt".to_owned())
        );
        assert_eq!(
            PcloudFsShim::<MockFolderBackend, MockFileBackend, MockUploadBackend>::join_child(
                "/docs",
                OsStr::new("note.md"),
            ),
            Some("/docs/note.md".to_owned())
        );
    }

    #[test]
    fn file_attr_translation_maps_kinds_and_mode() {
        let attr = EntryAttr {
            ino: 3,
            kind: FsEntryKind::Directory,
            size: 0,
            mode: 0o755,
            uid: 10,
            gid: 11,
            mtime_epoch: None,
            mtime_nsec: 0,
        };
        let fa = file_attr_from(&attr);
        assert_eq!(fa.kind, FileType::Directory);
        assert_eq!(fa.perm, 0o755);
        assert_eq!(fa.uid, 10);
        assert_eq!(fa.gid, 11);
        assert_eq!(fa.ino, 3);
        assert_eq!(fa.blksize, 4096);
        assert_eq!(fa.nlink, 1);
    }

    #[test]
    fn map_kind_covers_every_variant() {
        assert_eq!(map_kind(FsEntryKind::Directory), FileType::Directory);
        assert_eq!(map_kind(FsEntryKind::RegularFile), FileType::RegularFile);
        assert_eq!(map_kind(FsEntryKind::Symlink), FileType::Symlink);
    }

    #[test]
    fn parent_ino_of_root_is_root_itself() {
        let (shim, _, _, _, _tmp) = build_shim();
        assert_eq!(
            parent_ino_of(&shim.inodes, crate::inode::ROOT_INODE),
            Some(crate::inode::ROOT_INODE)
        );
    }

    #[test]
    fn parent_ino_of_resolves_after_lookup() {
        let (shim, _, _, _, _tmp) = build_shim();
        // Populate /docs via a lookup so its ino is known.
        let attr = shim
            .adapter
            .lookup(crate::inode::ROOT_INODE, "docs")
            .unwrap();
        assert_eq!(
            parent_ino_of(&shim.inodes, attr.ino),
            Some(crate::inode::ROOT_INODE)
        );
    }

    #[test]
    fn write_without_fh_registration_is_caught_by_fh_check() {
        // This validates the defensive fh lookup path used by the `write`
        // delegate: with no registered fh, the snapshot lookup must be
        // None, so the shim would reply EBADF. We assert the invariant
        // directly on the table since we cannot construct fuser replies.
        let (shim, _, _, _, _tmp) = build_shim();
        assert!(shim.fhs.get_snapshot(12345).is_none());
    }

    #[test]
    fn write_path_create_and_write_roundtrip_through_services() {
        // Drive the underlying services the shim delegates to so we know
        // the wiring is compatible end-to-end.
        let (_shim, _, _, upload, _tmp) = build_shim();
        // Reuse the writer from the shim by pulling it back through a
        // fresh shim clone of Arcs; the writer is shared.
        // Construct a second shim over the same Arcs so we can exercise
        // the public writer directly.
        let folder = Arc::new(MockFolderBackend::new());
        folder.insert_dir("/", 1, vec![]);
        let files = Arc::new(MockFileBackend::new());
        let adapter = Arc::new(ProtoFuseAdapter::with_file_backend(
            folder,
            files,
            AdapterOptions::default(),
        ));
        let tmp2 = tempdir().unwrap();
        let stage = StagingDir::open(tmp2.path().join("s")).unwrap();
        let journal = WriteJournal::open(stage.journal_path()).unwrap();
        let writer = Arc::new(WritePathService::new(
            stage,
            journal,
            Arc::clone(&upload),
            WritePathOptions::default(),
        ));
        writer.create(42, "/", "x.txt").unwrap();
        writer.write(42, 0, b"abc").unwrap();
        writer.flush(42).unwrap();
        writer.release(42);
        let uploads = upload.uploads.lock().unwrap();
        assert_eq!(uploads.get("/x.txt").unwrap(), b"abc");
        drop(adapter);
    }
}

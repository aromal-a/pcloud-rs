//! FUSE adapter traits and the 4.b `ProtoFuseAdapter` implementation.
//!
//! The 4.a sub-bead landed the [`FuseAdapter`] trait and a [`NullFuseAdapter`]
//! that returns `ENOSYS` for everything. 4.b (this file) adds:
//!
//! - [`ProtoFuseAdapter`]: a concrete adapter wired to a [`FolderBackend`]
//!   and the [`InodeTable`]/[`MetadataCache`] from sibling modules.
//! - Implementations of `lookup`, `getattr`, and `readdir` that transform
//!   listing results into `EntryAttr`/`DirEntry` values and honour the
//!   metadata cache TTL.
//!
//! Read (`open`/`read`/`release`), write, rename, unlink, and truncate are
//! explicitly out of scope for 4.b. They remain at the trait-default
//! `ENOSYS` reply and will be wired in 4.c–4.e.

#![allow(clippy::too_many_arguments)]

// **PLATFORM:** Linux
// **GATING:** #[cfg(target_os = "linux")].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::backend::{FileBackend, FileHandle, FolderBackend};
use crate::errors::FsError;
use crate::inode::{InodeTable, ROOT_INODE};
use crate::metadata_cache::{CachedMetadata, MetadataCache, MetadataCacheConfig};
use crate::page_cache::{PageCache, PageCacheConfig, PageKey};
use crate::path_norm::{PathError, canonicalise, join_child};
use crate::write_path::{FileUploadBackend, WritePathError, WritePathService};

/// Unix `libc::ENOSYS` error code for "function not implemented".
pub const ENOSYS: i32 = 38;
/// `libc::EBADF` — bad file descriptor.
pub const EBADF: i32 = 9;
/// `libc::EROFS` — read-only file system. Mirrored from
/// [`crate::errors::EROFS`] so trait consumers can depend on a single
/// module.
pub const EROFS: i32 = crate::errors::EROFS;

/// Opaque file-handle identifier returned by [`FuseAdapter::open`] and
/// consumed by [`FuseAdapter::read`] / [`FuseAdapter::release`].
pub type FileHandleId = u64;

/// Inode number type mirrored from the FUSE kernel protocol.
pub type Ino = u64;

/// Minimal file kind classification used by the scaffold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsEntryKind {
    /// Directory entry (maps to `S_IFDIR` mode bits in FUSE replies).
    Directory,
    /// Regular file entry (maps to `S_IFREG`).
    RegularFile,
    /// Symbolic link entry (maps to `S_IFLNK`); not yet populated by the
    /// pcloud backend but reserved for future support.
    Symlink,
}

/// Attribute snapshot returned by `getattr`/`lookup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryAttr {
    /// Inode number assigned by the [`InodeTable`].
    pub ino: Ino,
    /// File-kind classification used to derive the mode bits.
    pub kind: FsEntryKind,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// POSIX mode bits (e.g. `0o644` for files, `0o755` for dirs).
    pub mode: u16,
    /// Owning user id reported to the kernel.
    pub uid: u32,
    /// Owning group id reported to the kernel.
    pub gid: u32,
    /// Last-modified epoch seconds from the backend, when known. The
    /// fuser shim uses this for `mtime`/`ctime`; `None` means "use now".
    pub mtime_epoch: Option<u64>,
}

/// One entry returned by a `readdir` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Inode number of the child entry.
    pub ino: Ino,
    /// Whether the child is a directory, regular file, or symlink.
    pub kind: FsEntryKind,
    /// Final path component of the entry (no slashes).
    pub name: String,
}

/// Adapter trait bridging pCloud runtime state to a FUSE mount.
///
/// # Read-side methods
///
/// `lookup`, `getattr`, `readdir`, `open`, `read`, `release` default to
/// [`ENOSYS`]. Implementors that only service reads (e.g. the 4.b/4.c
/// scaffold) override exactly those.
///
/// # Write-side methods (bd-1du.4.d/4.e)
///
/// `create`, `write`, `flush_write`, `fsync_write`, `truncate`, `unlink`,
/// `rename` default to [`ENOSYS`] for implementors that have no write
/// path, and to [`EROFS`] for implementors that have a write path but
/// whose transport does not yet support the individual mutation (e.g.
/// unlink/rename when `pcloud-proto` has not yet ported `deletefile` /
/// `renamefile` — see [`ProtoFuseAdapter::with_write_path`]).
///
/// Returning an integer errno keeps the trait dyn-safe and FUSE-kernel
/// friendly: callers translate directly into `reply.error(errno)`.
pub trait FuseAdapter: Send + Sync + 'static {
    /// FUSE `lookup`. Resolve the child named `_name` under `_parent` and
    /// return its attribute snapshot (including the allocated inode, kind,
    /// size, and timestamps).
    ///
    /// # Errors
    ///
    /// * `ENOENT` — no child named `_name` exists under `_parent`.
    /// * `ENOTDIR` — `_parent` resolves to a non-directory inode.
    /// * `EINVAL` — `_name` contains an embedded NUL byte, is empty, or
    ///   equals `"."` / `".."` (forbidden by the POSIX lookup contract).
    /// * `EACCES` — the caller lacks execute (`x`) on the parent directory
    ///   or the backend rejected the request with
    ///   `FsError::PermissionDenied`.
    /// * `EIO` — the transport/backend produced an irrecoverable error.
    /// * `ENOSYS` — the adapter has no lookup implementation (default).
    ///
    /// # Concurrency
    ///
    /// `lookup` is fully concurrent-safe: it only reads cached metadata
    /// and issues idempotent RPCs. Multiple FUSE worker threads may
    /// invoke it on the same `_parent`/`_name` simultaneously; the
    /// metadata cache coalesces redundant backend round-trips.
    fn lookup(&self, _parent: Ino, _name: &str) -> Result<EntryAttr, i32> {
        Err(ENOSYS)
    }

    /// FUSE `getattr`. Return the attribute snapshot for `_ino`.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — `_ino` has been evicted from the inode table or was
    ///   never allocated.
    /// * `ESTALE` — the inode is known locally but the backend reports
    ///   the underlying object no longer exists (generation mismatch).
    /// * `EIO` — transport failure while refreshing a stale cache entry.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe. Served from the TTL-bounded metadata cache in
    /// the common case and performs only a single idempotent
    /// `stat`-equivalent on miss.
    fn getattr(&self, _ino: Ino) -> Result<EntryAttr, i32> {
        Err(ENOSYS)
    }

    /// FUSE `readdir`. Return the children of `_ino` starting at
    /// `_offset` (the FUSE-kernel-supplied resume cookie).
    ///
    /// # Errors
    ///
    /// * `ENOENT` — `_ino` is unknown.
    /// * `ENOTDIR` — `_ino` resolves to a regular file or symlink.
    /// * `EACCES` — the caller lacks read (`r`) on the directory.
    /// * `EIO` — listing RPC failed after retry.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe. The listing is served from an immutable snapshot
    /// keyed on `(ino, generation)`; mutations committed after the
    /// snapshot was taken are not visible until the next invalidation
    /// tick, matching POSIX `readdir` semantics.
    fn readdir(&self, _ino: Ino, _offset: i64) -> Result<Vec<DirEntry>, i32> {
        Err(ENOSYS)
    }

    /// FUSE `open`. Allocate a per-open handle id backed by a reference
    /// counted [`FileHandle`] that pins the page cache for the life of
    /// the descriptor.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — `_ino` is unknown.
    /// * `EISDIR` — `_ino` is a directory (use `opendir`).
    /// * `EACCES` — permission denied by the backend ACL.
    /// * `EMFILE` — the adapter's handle table is full.
    /// * `EIO` — transport failure priming the page cache.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe. Handle-id allocation uses an atomic counter; the
    /// backing `FileHandle` is shared via `Arc` so multiple concurrent
    /// opens of the same inode share cached pages.
    fn open(&self, _ino: Ino) -> Result<FileHandleId, i32> {
        Err(ENOSYS)
    }

    /// FUSE `read`. Serve up to `_len` bytes starting at `_offset` for an
    /// open handle. A returned buffer shorter than `_len` indicates EOF,
    /// matching POSIX `read(2)` semantics.
    ///
    /// # Errors
    ///
    /// * `EBADF` — `_handle` is not a live handle id (never opened or
    ///   already released).
    /// * `EINVAL` — `_offset + _len` overflows `u64`.
    /// * `EIO` — transport failure populating a missing page.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe and designed for maximum parallelism. Multiple
    /// readers of the same or distinct handles do not serialise: pages
    /// are stored as `Arc<Vec<u8>>` in the page cache (see
    /// [`crate::page_cache`]), and cache hits return cheap clones
    /// without holding any mutex across the copy.
    fn read(&self, _handle: FileHandleId, _offset: u64, _len: usize) -> Result<Vec<u8>, i32> {
        Err(ENOSYS)
    }

    /// FUSE `release`. Drop one strong reference to the handle id. When
    /// the last reference is released the backend is notified so it may
    /// flush dirty pages and free server-side state.
    ///
    /// # Errors
    ///
    /// * `EBADF` — `_handle` is unknown.
    /// * `EIO` — a best-effort final flush failed; the handle is still
    ///   closed locally.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe. Ref-count decrement is atomic; only the last
    /// releaser performs backend cleanup.
    fn release(&self, _handle: FileHandleId) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// FUSE `mkdir`. Create a directory named `name` under `parent_path`
    /// and return its new attributes (including the freshly allocated
    /// inode). Adapters without a folder-create backend default to
    /// [`ENOSYS`] so the mount remains read-only.
    ///
    /// # Errors
    ///
    /// * `EEXIST` — a file or directory already exists at that path.
    /// * `ENOENT` — `parent_path` does not exist.
    /// * `ENOTDIR` — `parent_path` is a regular file.
    /// * `EACCES` — caller lacks write on the parent.
    /// * `EINVAL` — `name` contains a NUL byte or a path separator.
    /// * `ENAMETOOLONG` — `name` exceeds the backend limit.
    /// * `ENOSPC` — quota/space exhaustion reported by the backend.
    /// * `EROFS` — write path disabled for this adapter.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// May serialise against other mutating operations on `parent_path`
    /// to preserve create-vs-unlink ordering, but concurrent mkdirs on
    /// unrelated parents proceed in parallel.
    fn mkdir(&self, _parent_path: &str, _name: &str) -> Result<EntryAttr, i32> {
        Err(ENOSYS)
    }

    /// FUSE `rmdir`. Remove an empty directory by absolute remote path.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — no directory at `_path`.
    /// * `ENOTDIR` — `_path` is a regular file.
    /// * `ENOTEMPTY` — directory contains children.
    /// * `EACCES` — caller lacks write on the parent.
    /// * `EBUSY` — directory is currently mounted or open.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Serialises with concurrent mutations targeting the same path to
    /// avoid TOCTOU on the empty-directory check.
    fn rmdir(&self, _path: &str) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// FUSE `create`. Allocate an inode for a new regular file under
    /// `parent_path` with `name`, pre-registering a write-side staging
    /// slot.
    ///
    /// # Errors
    ///
    /// * `EEXIST` — entry already present at that path.
    /// * `ENOENT` — `parent_path` missing.
    /// * `ENOTDIR` — `parent_path` is not a directory.
    /// * `EACCES` — caller lacks write on the parent.
    /// * `EINVAL` — `name` contains NUL or path separator.
    /// * `ENAMETOOLONG` — `name` exceeds backend limit.
    /// * `ENOSPC` — quota exhaustion.
    /// * `EROFS` — adapter configured without a write path.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Serialises only against other create/unlink/rename operations on
    /// the same `(parent_path, name)` tuple.
    fn create(&self, _parent_path: &str, _name: &str) -> Result<Ino, i32> {
        Err(ENOSYS)
    }

    /// FUSE `write`. Stage `data` at byte `offset` for `ino`. Returns
    /// the number of bytes accepted (short writes are permitted to match
    /// POSIX `write(2)`).
    ///
    /// # Errors
    ///
    /// * `EBADF` — no open write handle for `ino`.
    /// * `EINVAL` — `offset + data.len()` overflows `u64`.
    /// * `ENOSPC` — staging tier (journal or server quota) exhausted.
    /// * `EDQUOT` — account over quota.
    /// * `EIO` — journal write failed.
    /// * `EROFS` — adapter has no write path.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// May serialise per-inode. Concurrent writers to the *same* inode
    /// contend on the staging mutex to keep the journal monotonically
    /// ordered; writers to *distinct* inodes proceed in parallel.
    fn write(&self, _ino: Ino, _offset: u64, _data: &[u8]) -> Result<usize, i32> {
        Err(ENOSYS)
    }

    /// FUSE `flush`. Trigger a best-effort writeback of any staged bytes
    /// for `ino`. Does not guarantee durability — use
    /// [`Self::fsync_write`] for that.
    ///
    /// # Errors
    ///
    /// * `EBADF` — no staging state for `ino`.
    /// * `EIO` — writeback upload failed.
    /// * `ENOSPC` / `EDQUOT` — server refused the upload.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Serialises with concurrent writes on the same inode.
    fn flush_write(&self, _ino: Ino) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// FUSE `fsync`. Strong durability barrier for `ino`: forces the
    /// upload to complete AND the journal to be `fsync(file)+fsync(dir)`
    /// synced to disk before returning.
    ///
    /// # Errors
    ///
    /// * `EBADF` — no staging state for `ino`.
    /// * `EIO` — upload or journal fsync failed.
    /// * `ENOSPC` / `EDQUOT` — server rejected the final upload.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Blocks concurrent writers on the same inode until the barrier
    /// completes to preserve ordering across the fsync boundary.
    fn fsync_write(&self, _ino: Ino) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// FUSE `setattr(ATTR_SIZE)`. Truncate the staging view of `ino` to
    /// exactly `new_size` bytes, zero-extending on growth.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — unknown inode.
    /// * `EISDIR` — `ino` is a directory.
    /// * `EINVAL` — `new_size` exceeds the backend maximum.
    /// * `ENOSPC` — growth would exceed quota.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Serialises with concurrent writes on the same inode.
    fn truncate(&self, _ino: Ino, _new_size: u64) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// FUSE `unlink`. Remove the entry named `name` under `parent_path`.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — no such entry.
    /// * `EISDIR` — entry is a directory (use `rmdir`).
    /// * `EACCES` — caller lacks write on the parent.
    /// * `EBUSY` — entry is currently open with mandatory-lock semantics.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Serialises with create/rename on the same `(parent_path, name)`.
    fn unlink(&self, _parent_path: &str, _name: &str) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// FUSE `rename`. Move `from` to `to` using absolute remote paths.
    /// Both paths must resolve under the mount root; cross-mount renames
    /// are rejected.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — `from` does not exist.
    /// * `EEXIST` — `to` exists and is non-empty (destination is a
    ///   non-empty directory).
    /// * `ENOTDIR` — `from` is a directory but `to`'s parent is not.
    /// * `EISDIR` — `to` exists as a directory and `from` is a file.
    /// * `EINVAL` — `to` is a subpath of `from` (would create a loop).
    /// * `EXDEV` — cross-device rename rejected by the backend.
    /// * `EACCES` — permission denied on either parent.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Takes a global rename lock to preserve the atomicity guarantees
    /// POSIX requires; concurrent unrelated mutations still proceed.
    fn rename(&self, _from: &str, _to: &str) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// Resolve `ino` to its full remote path.
    ///
    /// Platform shims that only receive inode numbers (FUSE `lookup`,
    /// `create`, `unlink`, `mkdir`, `rmdir`, `rename`) call this to
    /// reconstruct the absolute path argument demanded by the write-side
    /// trait methods (which operate on path strings).
    ///
    /// The default implementation returns [`ENOSYS`]; real adapters
    /// (notably [`ProtoFuseAdapter`]) walk their inode table to produce
    /// the canonical path.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — the inode has been evicted or was never allocated.
    /// * `ENOSYS` — adapter does not implement an inode table.
    fn resolve_ino_to_path(&self, _ino: Ino) -> Result<PathBuf, i32> {
        Err(ENOSYS)
    }

    // ---------------------------------------------------------------------
    // Phase-5 cross-platform write/maintenance surface.
    //
    // These operations are invoked by the macOS (U1) and Windows (U2)
    // platform shims. Linux (`ProtoFuseAdapter`) deliberately keeps them
    // at the default `ENOSYS` stub; real Linux bodies land under
    // bd-1du.4.6. Every method has a default implementation so adding a
    // new entry here does not break existing `FuseAdapter` impls.
    // ---------------------------------------------------------------------

    /// Atomic whole-file overwrite. Replace the entire content of `ino`
    /// with `data`, discarding any previously staged bytes. Used by
    /// WinFSP's `Overwrite` callback and macOS' `setattr(ATTR_SIZE=0)` +
    /// subsequent write when the platform prefers an atomic replacement.
    /// Returns the number of bytes accepted.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — inode unknown.
    /// * `EISDIR` — `ino` is a directory.
    /// * `ENOSPC` / `EDQUOT` — quota exceeded.
    /// * `EIO` — staging-journal write failed.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Exclusively locks the inode; concurrent writers block until the
    /// overwrite is durable in staging.
    fn overwrite(&self, _ino: Ino, _data: &[u8]) -> Result<usize, i32> {
        Err(ENOSYS)
    }

    /// FUSE/WinFSP `statfs` / `GetVolumeInfo`. Return `(total_bytes,
    /// free_bytes)` for the mounted volume. Platform shims translate the
    /// tuple into their native struct (`statvfs` on macOS, `FSP_FSCTL_
    /// VOLUME_INFO` on Windows).
    ///
    /// # Errors
    ///
    /// * `EIO` — quota RPC failed.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe; quota responses are memoised with a short TTL to
    /// coalesce bursts from `df(1)` / Explorer polling.
    fn statfs(&self) -> Result<(u64, u64), i32> {
        Err(ENOSYS)
    }

    /// Per-handle close notification (distinct from [`Self::release`], which
    /// drops the last strong reference). WinFSP's `Close` callback fires
    /// on every `CloseHandle` even when other duplicated handles remain
    /// open; the adapter uses this to flush dirty pages without tearing
    /// down the underlying `FileHandle`.
    ///
    /// # Errors
    ///
    /// * `EBADF` — handle id unknown.
    /// * `EIO` — best-effort flush failed (handle still closed locally).
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe; ref-count decrement is atomic.
    fn close(&self, _handle: FileHandleId) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// WinFSP `Cleanup` callback / macOS `vnop_reclaim`. Invoked when the
    /// OS is about to evict the inode from its cache. Adapters may drop
    /// read-ahead buffers, cancel background prefetch, or finalise a
    /// deferred delete (`CLEANUP_DELETE` flag on Windows). `flags` is a
    /// platform-specific bitmask; unknown bits MUST be ignored.
    ///
    /// # Errors
    ///
    /// * `EBUSY` — deferred delete requested but handles remain open.
    /// * `EIO` — backend delete failed during `CLEANUP_DELETE`.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe; always called AFTER all user handles have been
    /// closed, so no lock contention is expected in practice.
    fn cleanup(&self, _ino: Ino, _flags: u32) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// Generic flush for an open handle. Unlike [`Self::flush_write`] — which
    /// is keyed on an inode — this variant mirrors FUSE's `flush(fh)`
    /// and WinFSP's `Flush(FileContext)`. Default dispatches to
    /// [`ENOSYS`].
    ///
    /// # Errors
    ///
    /// * `EBADF` — handle id unknown.
    /// * `EIO` — writeback failed.
    /// * `ENOSPC` / `EDQUOT` — upload rejected.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Serialises against writes on the same inode; unrelated handles
    /// are unaffected.
    fn flush(&self, _handle: FileHandleId) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// Generic durability barrier for an open handle. `datasync=true`
    /// requests a data-only sync (FUSE `fdatasync`); `false` requests a
    /// full metadata+data sync. Default dispatches to [`ENOSYS`].
    ///
    /// # Errors
    ///
    /// * `EBADF` — handle id unknown.
    /// * `EIO` — upload or journal fsync failed.
    /// * `ENOSPC` / `EDQUOT` — upload rejected.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Blocks concurrent writers on the same inode until the barrier
    /// completes.
    fn fsync(&self, _handle: FileHandleId, _datasync: bool) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// FUSE `setattr` / WinFSP `SetBasicInfo` generic entry point. Any
    /// field set to `Some(_)` MUST be applied atomically; fields left as
    /// `None` MUST NOT be touched. Returns the attributes AFTER the
    /// mutation so the platform shim can reply in one round-trip.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — inode unknown.
    /// * `EPERM` — caller is not owner and not root (chmod/chown rules).
    /// * `EINVAL` — mode contains reserved bits or size overflows.
    /// * `ENOSPC` — growth would exceed quota.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// All mutations are applied under a single inode lock so observers
    /// never see a partial update.
    fn setattr(&self, _ino: Ino, _attr: SetAttr) -> Result<EntryAttr, i32> {
        Err(ENOSYS)
    }

    /// WinFSP `SetFileSize` / macOS `vnop_setattr(ATTR_SIZE)`. Resize the
    /// staging/backend file for `ino` to exactly `new_size` bytes,
    /// zero-filling on growth and truncating on shrink. Distinct from
    /// `Self::truncate` only in that WinFSP issues it on both grow and
    /// shrink paths; adapters may share an implementation (see `Self::truncate`).
    ///
    /// # Errors
    ///
    /// * `ENOENT` — inode unknown.
    /// * `EISDIR` — `ino` is a directory.
    /// * `EFBIG` — `new_size` exceeds backend maximum.
    /// * `ENOSPC` — quota would be exceeded by growth.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Serialises with concurrent writes on the same inode.
    fn set_size(&self, _ino: Ino, _new_size: u64, _set_allocation_size: bool) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// WinFSP `CanDelete` callback. Return `Ok(())` if the object at
    /// `ino` may be deleted right now (e.g. an empty directory, an
    /// unlocked file). Return `Err(errno)` with `libc::EACCES` /
    /// `ENOTEMPTY` / `EBUSY` otherwise. The platform shim will refuse
    /// the subsequent `Unlink`/`Rmdir` if this returns `Err`.
    ///
    /// # Errors
    ///
    /// * `EACCES` — permission denied.
    /// * `ENOTEMPTY` — directory still has children.
    /// * `EBUSY` — object is currently in use (open handles, mounted).
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Concurrent-safe — purely predicate evaluation, no mutation.
    fn can_delete(&self, _ino: Ino) -> Result<(), i32> {
        Err(ENOSYS)
    }

    /// WinFSP `SetBasicInfo` callback. Apply Windows-style basic
    /// metadata (file attributes bitmask + creation/last-access/
    /// last-write/change timestamps in FILETIME units). Unset fields
    /// (`None`) MUST be preserved. Timestamps equal to `0` or `u64::MAX`
    /// are reserved sentinels in WinFSP and MUST be ignored.
    ///
    /// # Errors
    ///
    /// * `ENOENT` — inode unknown.
    /// * `EPERM` — caller lacks `FILE_WRITE_ATTRIBUTES`.
    /// * `EINVAL` — reserved FILETIME sentinel supplied.
    /// * `EROFS` — write path disabled.
    /// * `ENOSYS` — default stub.
    ///
    /// # Concurrency
    ///
    /// Atomic per-inode; concurrent attribute observers see either the
    /// pre- or post-state, never a mix.
    fn set_basic_info(&self, _ino: Ino, _info: BasicInfo) -> Result<EntryAttr, i32> {
        Err(ENOSYS)
    }
}

/// Optional-field bundle for [`FuseAdapter::setattr`].
///
/// Mirrors the subset of FUSE `setattr` / WinFSP `SetBasicInfo` fields
/// actually exercised by the platform shims. Any `None` field MUST be
/// left untouched by the adapter.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SetAttr {
    /// New POSIX mode bits to apply; `None` preserves the current value.
    pub mode: Option<u16>,
    /// New owning user id; `None` preserves the current value.
    pub uid: Option<u32>,
    /// New owning group id; `None` preserves the current value.
    pub gid: Option<u32>,
    /// New file size (triggers a truncate); `None` leaves size unchanged.
    pub size: Option<u64>,
    /// Last-modified time, epoch seconds.
    pub mtime_epoch: Option<u64>,
    /// Last-accessed time, epoch seconds.
    pub atime_epoch: Option<u64>,
}

/// Windows basic-info bundle for [`FuseAdapter::set_basic_info`].
///
/// `file_attributes` is the Win32 `FILE_ATTRIBUTE_*` bitmask. Times are
/// Windows `FILETIME` 100-nanosecond ticks since 1601-01-01 UTC. Fields
/// set to `None` MUST NOT be modified by the adapter.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BasicInfo {
    /// Win32 `FILE_ATTRIBUTE_*` bitmask to apply; `None` preserves value.
    pub file_attributes: Option<u32>,
    /// Creation time in FILETIME 100-ns ticks since 1601-01-01 UTC.
    pub creation_time: Option<u64>,
    /// Last-access time in FILETIME ticks.
    pub last_access_time: Option<u64>,
    /// Last-write time in FILETIME ticks.
    pub last_write_time: Option<u64>,
    /// Attribute change time in FILETIME ticks.
    pub change_time: Option<u64>,
}

/// No-op adapter used by the 4.a scaffold tests and the mount self-check.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullFuseAdapter;

impl FuseAdapter for NullFuseAdapter {}

// -----------------------------------------------------------------------------
// 4.b ProtoFuseAdapter
// -----------------------------------------------------------------------------

/// Construction options for [`ProtoFuseAdapter`].
#[derive(Debug, Clone, Copy)]
pub struct AdapterOptions {
    /// Owning user id reported for every inode (typically the mounting user).
    pub uid: u32,
    /// Owning group id reported for every inode.
    pub gid: u32,
    /// Default POSIX mode bits for regular files (e.g. `0o644`).
    pub file_mode: u16,
    /// Default POSIX mode bits for directories (e.g. `0o755`).
    pub dir_mode: u16,
    /// Metadata-cache tuning (TTL, capacity).
    pub cache: MetadataCacheConfig,
    /// Page-cache tuning (page size, capacity).
    pub page_cache: PageCacheConfig,
}

impl Default for AdapterOptions {
    fn default() -> Self {
        // SAFETY: getuid/getgid on Linux are always-success libc calls
        // that take no arguments and never set errno; they read kernel
        // task creds. Falls back to root only if the libc binding is
        // missing on a non-linux build.
        #[cfg(target_os = "linux")]
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        #[cfg(not(target_os = "linux"))]
        let (uid, gid) = (0u32, 0u32);
        Self {
            uid,
            gid,
            file_mode: 0o644,
            dir_mode: 0o755,
            cache: MetadataCacheConfig::default(),
            page_cache: PageCacheConfig::default(),
        }
    }
}

/// Placeholder [`FileBackend`] that always returns `ENOSYS`, used when
/// [`ProtoFuseAdapter`] is constructed without a read-path backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFileBackend;

impl FileBackend for NoFileBackend {
    fn open(&self, _file_id: u64) -> Result<FileHandle, FsError> {
        Err(FsError::Invalid)
    }
    fn read(&self, _handle: &FileHandle, _offset: u64, _len: usize) -> Result<Vec<u8>, FsError> {
        Err(FsError::Invalid)
    }
}

/// Per-open-handle bookkeeping. Multiple open calls on the same inode each
/// receive a distinct [`FileHandleId`]. The reference count on the
/// underlying [`FileHandle`] is what governs the lifetime of the
/// backend-side resource — `release` drops the last reference.
#[derive(Debug)]
struct HandleSlot {
    ino: Ino,
    shared: Arc<FileHandle>,
}

#[derive(Debug, Default)]
struct HandleTable {
    by_id: HashMap<FileHandleId, HandleSlot>,
    /// Shared `FileHandle` per inode; refcounted via `Arc::strong_count`.
    by_ino: HashMap<Ino, Arc<FileHandle>>,
    next_id: FileHandleId,
}

/// Object-safe dispatcher that lets [`ProtoFuseAdapter`] hold a
/// [`WritePathService<U>`] without carrying the generic `U` on itself.
///
/// All methods forward to the corresponding [`WritePathService`] API and
/// translate [`WritePathError`] into an `errno`. The upload backend's
/// ability to satisfy `unlink_remote` / `rename_remote` is an orthogonal
/// question: when the transport layer does not yet support the
/// operation, the backend returns a transport error which this
/// dispatcher reports to the kernel as [`EROFS`] so the caller sees a
/// clear "read-only file system" signal (no silent data loss).
trait WriteDelegate: Send + Sync + 'static {
    fn create(&self, ino: u64, parent_path: &str, name: &str) -> Result<(), i32>;
    fn open_for_write(
        &self,
        ino: u64,
        path: String,
        append_mode: bool,
        o_trunc: bool,
    ) -> Result<(), i32>;
    fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, i32>;
    fn flush(&self, ino: u64) -> Result<(), i32>;
    fn fsync(&self, ino: u64) -> Result<(), i32>;
    fn truncate(&self, ino: u64, new_size: u64) -> Result<(), i32>;
    fn unlink(&self, ino: Option<u64>, path: &str) -> Result<(), i32>;
    fn rename(&self, from: &str, to: &str) -> Result<(), i32>;
}

struct WriteDelegateImpl<U: FileUploadBackend> {
    inner: Arc<WritePathService<U>>,
}

fn write_err_to_errno(e: &WritePathError) -> i32 {
    match e {
        // Transport-level failures on mutations (unlink/rename where the
        // upload backend rejects the op because deletefile/renamefile are
        // not yet ported to pcloud-proto) surface as EROFS so the kernel
        // reports "read-only file system" rather than a generic EIO.
        WritePathError::Upload(_) => crate::errors::EROFS,
        other => other.to_errno(),
    }
}

impl<U: FileUploadBackend> WriteDelegate for WriteDelegateImpl<U> {
    fn create(&self, ino: u64, parent_path: &str, name: &str) -> Result<(), i32> {
        self.inner
            .create(ino, parent_path, name)
            .map_err(|e| write_err_to_errno(&e))
    }
    fn open_for_write(
        &self,
        ino: u64,
        path: String,
        append_mode: bool,
        o_trunc: bool,
    ) -> Result<(), i32> {
        self.inner
            .open_for_write(ino, path, append_mode, o_trunc)
            .map_err(|e| write_err_to_errno(&e))
    }
    fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, i32> {
        self.inner
            .write(ino, offset, data)
            .map_err(|e| write_err_to_errno(&e))
    }
    fn flush(&self, ino: u64) -> Result<(), i32> {
        self.inner.flush(ino).map_err(|e| write_err_to_errno(&e))
    }
    fn fsync(&self, ino: u64) -> Result<(), i32> {
        self.inner.fsync(ino).map_err(|e| write_err_to_errno(&e))
    }
    fn truncate(&self, ino: u64, new_size: u64) -> Result<(), i32> {
        self.inner
            .truncate(ino, new_size)
            .map_err(|e| write_err_to_errno(&e))
    }
    fn unlink(&self, ino: Option<u64>, path: &str) -> Result<(), i32> {
        self.inner
            .unlink(ino, path)
            .map_err(|e| write_err_to_errno(&e))
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), i32> {
        self.inner
            .rename(from, to)
            .map_err(|e| write_err_to_errno(&e))
    }
}

/// FUSE adapter that resolves paths against a [`FolderBackend`] and, for
/// sub-bead 4.c, a [`FileBackend`] for content reads.
///
/// The adapter services `lookup`/`getattr`/`readdir` (4.b),
/// `open`/`read`/`release` (4.c), and — when a writer is attached via
/// [`ProtoFuseAdapter::with_write_path`] — the full write family
/// (`create`/`write`/`flush_write`/`fsync_write`/`truncate`/`unlink`/
/// `rename`) from 4.d. Without a writer attached, write-side trait
/// methods return the default [`ENOSYS`].
pub struct ProtoFuseAdapter<B: FolderBackend, F: FileBackend = NoFileBackend> {
    backend: Arc<B>,
    file_backend: Arc<F>,
    inodes: Arc<InodeTable>,
    cache: Arc<MetadataCache>,
    page_cache: Arc<PageCache>,
    /// `ino → file_id` mirror populated during directory listings so that
    /// `open(ino)` can resolve a pCloud file id without a round trip.
    file_ids: Arc<Mutex<HashMap<Ino, u64>>>,
    handles: Arc<Mutex<HandleTable>>,
    /// Optional write-path dispatcher (bd-1du.4.d/4.e). `None` means the
    /// adapter is read-only.
    writer: Option<Arc<dyn WriteDelegate>>,
    options: AdapterOptions,
}

impl<B: FolderBackend, F: FileBackend> std::fmt::Debug for ProtoFuseAdapter<B, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtoFuseAdapter")
            .field("options", &self.options)
            .field("inodes_len", &self.inodes.len())
            .field("cache_len", &self.cache.len())
            .field("page_cache_len", &self.page_cache.len())
            .finish_non_exhaustive()
    }
}

impl<B: FolderBackend> ProtoFuseAdapter<B, NoFileBackend> {
    /// Construct an adapter without a real read-path backend. Reads on any
    /// inode will return `EINVAL` via the `NoFileBackend` stub.
    pub fn new(backend: Arc<B>, options: AdapterOptions) -> Self {
        Self::with_file_backend(backend, Arc::new(NoFileBackend), options)
    }
}

impl<B: FolderBackend, F: FileBackend> ProtoFuseAdapter<B, F> {
    /// Construct an adapter with explicit folder + file backends.
    pub fn with_file_backend(
        backend: Arc<B>,
        file_backend: Arc<F>,
        options: AdapterOptions,
    ) -> Self {
        Self {
            backend,
            file_backend,
            inodes: Arc::new(InodeTable::new()),
            cache: Arc::new(MetadataCache::new(options.cache)),
            page_cache: Arc::new(PageCache::new(options.page_cache)),
            file_ids: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HandleTable::default())),
            writer: None,
            options,
        }
    }

    /// Attach a [`WritePathService`] so this adapter can service FUSE
    /// write-side operations. Without this call, the write-side trait
    /// methods return [`ENOSYS`].
    ///
    /// The writer may be shared across other subsystems: the adapter
    /// only holds a cloned [`Arc`] and never mutates the service state
    /// outside of the documented [`WritePathService`] API.
    #[must_use]
    pub fn with_write_path<U>(mut self, writer: Arc<WritePathService<U>>) -> Self
    where
        U: FileUploadBackend,
    {
        self.writer = Some(Arc::new(WriteDelegateImpl { inner: writer }));
        self
    }

    /// Whether a write-path dispatcher is currently attached.
    #[must_use]
    pub fn has_write_path(&self) -> bool {
        self.writer.is_some()
    }

    /// Return a cloned [`Arc`] handle to the inode table so callers (e.g.
    /// the fuser shim) can resolve paths without a round-trip.
    #[must_use]
    pub fn inode_table(&self) -> Arc<InodeTable> {
        Arc::clone(&self.inodes)
    }

    /// Return a cloned [`Arc`] handle to the metadata cache. Used by the
    /// fuser shim to share TTL entries across concurrent requests.
    #[must_use]
    pub fn metadata_cache(&self) -> Arc<MetadataCache> {
        Arc::clone(&self.cache)
    }

    /// Return a cloned [`Arc`] handle to the page cache. Used primarily by
    /// tests to inspect hit ratios and capacity.
    #[must_use]
    pub fn page_cache(&self) -> Arc<PageCache> {
        Arc::clone(&self.page_cache)
    }

    /// Expose the options currently in effect.
    #[must_use]
    pub fn options(&self) -> AdapterOptions {
        self.options
    }

    fn mode_for(&self, kind: FsEntryKind) -> u16 {
        match kind {
            FsEntryKind::Directory => self.options.dir_mode,
            FsEntryKind::RegularFile | FsEntryKind::Symlink => self.options.file_mode,
        }
    }

    fn build_attr(&self, ino: u64, kind: FsEntryKind, size: u64) -> EntryAttr {
        self.build_attr_with_mtime(ino, kind, size, None)
    }

    fn build_attr_with_mtime(
        &self,
        ino: u64,
        kind: FsEntryKind,
        size: u64,
        mtime_epoch: Option<u64>,
    ) -> EntryAttr {
        EntryAttr {
            ino,
            kind,
            size,
            mode: self.mode_for(kind),
            uid: self.options.uid,
            gid: self.options.gid,
            mtime_epoch,
        }
    }

    fn path_from_ino(&self, ino: u64) -> Result<String, i32> {
        match self.inodes.resolve(ino) {
            Some((p, _, _)) => Ok(p),
            None => Err(crate::errors::ENOENT),
        }
    }

    /// Fetch-or-cache the directory listing at `path`. Always re-reads the
    /// cache first. On miss, contacts the backend and populates both the
    /// inode table and the metadata cache with entries for every child.
    fn fetch_directory(&self, path: &str) -> Result<CachedMetadata, FsError> {
        if let Some(meta) = self.cache.get(path) {
            if meta.children.is_some() {
                return Ok(meta);
            }
        }
        let listing = self.backend.list_contents(path)?;
        let (dir_ino, _) = self.inodes.insert_or_get(path, FsEntryKind::Directory);
        let mut child_entries = Vec::with_capacity(listing.entries.len());
        for entry in &listing.entries {
            let child_path = join_child(path, &entry.name).map_err(path_err_to_fs)?;
            let kind = if entry.is_folder {
                FsEntryKind::Directory
            } else {
                FsEntryKind::RegularFile
            };
            let (ino, _) = self.inodes.insert_or_get(&child_path, kind);
            if !entry.is_folder {
                if let Some(file_id) = entry.file_id {
                    if let Ok(mut ids) = self.file_ids.lock() {
                        ids.insert(ino, file_id);
                    }
                }
            }
            let entry_size = if entry.is_folder {
                0
            } else {
                entry.size.unwrap_or(0)
            };
            let attr = self.build_attr_with_mtime(ino, kind, entry_size, entry.modified);
            // Cache the child as a standalone lookup target.
            self.cache.put(
                &child_path,
                CachedMetadata {
                    attr: attr.clone(),
                    children: None,
                },
            );
            child_entries.push(DirEntry {
                ino,
                kind,
                name: entry.name.clone(),
            });
        }
        let dir_attr = self.build_attr(dir_ino, FsEntryKind::Directory, 0);
        let meta = CachedMetadata {
            attr: dir_attr,
            children: Some(child_entries),
        };
        self.cache.put(path, meta.clone());
        Ok(meta)
    }

    /// Update the cached size for a locally-modified inode so subsequent
    /// `getattr` reflects the current write-path length. A no-op when the
    /// inode is not in the cache (e.g. a read-only inode whose remote
    /// metadata has not been fetched).
    pub fn publish_local_size(&self, ino: u64, new_size: u64, mtime_epoch: Option<u64>) {
        let Some((path, _, _)) = self.inodes.resolve(ino) else {
            return;
        };
        if let Some(mut meta) = self.cache.get(&path) {
            meta.attr.size = new_size;
            if mtime_epoch.is_some() {
                meta.attr.mtime_epoch = mtime_epoch;
            }
            self.cache.put(&path, meta);
        }
    }

    /// Return the cached [`EntryAttr`] for `path`, if any. Used by
    /// `rename` to carry the source attribute to the destination cache
    /// without a round-trip to the backend.
    #[must_use]
    pub fn cached_attr(&self, path: &str) -> Option<EntryAttr> {
        self.cache.get(path).map(|m| m.attr)
    }

    /// Drop a single path from the metadata cache. Used by the shim to
    /// force a re-fetch after a write release so the server's canonical
    /// `file_id` replaces the locally-synthesised entry.
    pub fn invalidate_cache(&self, path: &str) {
        self.cache.invalidate(path);
    }

    /// Remove a path from the metadata cache, e.g. after a successful
    /// `unlink` or a failed `rename` rollback. Also removes the entry from
    /// its parent's children list when cached.
    pub fn forget_local_entry(&self, parent_path: &str, name: &str) {
        let child_path = match join_child(parent_path, name) {
            Ok(p) => p,
            Err(_) => return,
        };
        self.cache.invalidate(&child_path);
        if let Some(mut parent_meta) = self.cache.get(parent_path) {
            if let Some(children) = parent_meta.children.as_mut() {
                children.retain(|e| e.name != name);
                self.cache.put(parent_path, parent_meta);
            }
        }
    }

    /// Publish a locally-created file (or directory) into the metadata
    /// cache so subsequent `lookup`/`getattr` calls can see it without
    /// hitting the backend (which does not know about pending writes
    /// yet). The parent's cached children list, if any, is extended with
    /// the new entry; otherwise it is invalidated so the next `readdir`
    /// re-fetches and merges.
    pub fn publish_local_entry(&self, parent_path: &str, name: &str, attr: EntryAttr) {
        let child_path = match join_child(parent_path, name) {
            Ok(p) => p,
            Err(_) => return,
        };
        self.cache.put(
            &child_path,
            CachedMetadata {
                attr: attr.clone(),
                children: None,
            },
        );
        // Merge into the parent's children listing if we have one cached,
        // so readdir reflects the locally-created entry immediately.
        if let Some(mut parent_meta) = self.cache.get(parent_path) {
            if let Some(children) = parent_meta.children.as_mut() {
                if !children.iter().any(|e| e.name == name) {
                    children.push(DirEntry {
                        ino: attr.ino,
                        kind: attr.kind,
                        name: name.to_owned(),
                    });
                }
                self.cache.put(parent_path, parent_meta);
            } else {
                // No cached child list — drop it so the next readdir
                // re-fetches. (Unlikely after a successful create which
                // implies we've listed the parent recently.)
                self.cache.invalidate(parent_path);
            }
        }
    }
}

fn path_err_to_fs(err: PathError) -> FsError {
    match err {
        PathError::EmbeddedNul
        | PathError::EmptyName
        | PathError::InvalidComponent(_)
        | PathError::EscapesRoot => FsError::Invalid,
    }
}

impl<B: FolderBackend, F: FileBackend> FuseAdapter for ProtoFuseAdapter<B, F> {
    fn resolve_ino_to_path(&self, ino: Ino) -> Result<PathBuf, i32> {
        // The inode table stores the canonical absolute path for every
        // allocated inode (root, and everything materialised via
        // `lookup`/`readdir`/`create`/`mkdir`). Walking parent edges is
        // therefore unnecessary: a direct resolve is both correct and
        // cheap.
        match self.inodes.resolve(ino) {
            Some((path, _, _)) => Ok(PathBuf::from(path)),
            None => Err(crate::errors::ENOENT),
        }
    }

    fn lookup(&self, parent: Ino, name: &str) -> Result<EntryAttr, i32> {
        let parent_path = self.path_from_ino(parent)?;
        let child_path =
            join_child(&parent_path, name).map_err(|e| path_err_to_fs(e).to_errno())?;

        // Cache hit?
        if let Some(meta) = self.cache.get(&child_path) {
            return Ok(meta.attr);
        }

        // Otherwise, list the parent and hope to find the child.
        let parent_meta = self
            .fetch_directory(&parent_path)
            .map_err(|e| e.to_errno())?;
        if let Some(children) = parent_meta.children.as_ref() {
            if let Some(entry) = children.iter().find(|e| e.name == name) {
                let attr = self.build_attr(entry.ino, entry.kind, 0);
                return Ok(attr);
            }
        }
        Err(crate::errors::ENOENT)
    }

    fn getattr(&self, ino: Ino) -> Result<EntryAttr, i32> {
        let path = self.path_from_ino(ino)?;
        if let Some(meta) = self.cache.get(&path) {
            return Ok(meta.attr);
        }

        // Root: synthesise attributes from a listing of `/`.
        if ino == ROOT_INODE {
            let _ = self.fetch_directory("/").map_err(|e| e.to_errno())?;
            if let Some(meta) = self.cache.get("/") {
                return Ok(meta.attr);
            }
            return Err(crate::errors::EIO);
        }

        // Resolve via parent listing.
        let canon = canonicalise(&path).map_err(|e| path_err_to_fs(e).to_errno())?;
        let (parent_path, name) = split_parent(&canon).ok_or(crate::errors::ENOENT)?;
        let parent_meta = self
            .fetch_directory(&parent_path)
            .map_err(|e| e.to_errno())?;
        if let Some(children) = parent_meta.children.as_ref() {
            if let Some(entry) = children.iter().find(|e| e.name == name) {
                return Ok(self.build_attr(entry.ino, entry.kind, 0));
            }
        }
        Err(crate::errors::ENOENT)
    }

    fn readdir(&self, ino: Ino, offset: i64) -> Result<Vec<DirEntry>, i32> {
        let path = self.path_from_ino(ino)?;
        let meta = self.fetch_directory(&path).map_err(|e| e.to_errno())?;
        let children = meta.children.ok_or(crate::errors::ENOTDIR)?;
        let start = offset.max(0) as usize;
        if start >= children.len() {
            return Ok(Vec::new());
        }
        Ok(children[start..].to_vec())
    }

    fn mkdir(&self, parent_path: &str, name: &str) -> Result<EntryAttr, i32> {
        // Call the backend to create the remote folder.
        self.backend
            .create_folder(parent_path, name)
            .map_err(|e| e.to_errno())?;
        let child_path = join_child(parent_path, name).map_err(|e| path_err_to_fs(e).to_errno())?;
        // Allocate an inode for the new folder and publish it into the
        // cache + parent's children list so kernel readdir sees it
        // immediately without another `listfolder` round-trip.
        let (ino, _) = self
            .inodes
            .insert_or_get(&child_path, FsEntryKind::Directory);
        let attr = self.build_attr(ino, FsEntryKind::Directory, 0);
        self.publish_local_entry(parent_path, name, attr.clone());
        Ok(attr)
    }

    fn rmdir(&self, path: &str) -> Result<(), i32> {
        self.backend.delete_folder(path).map_err(|e| e.to_errno())?;
        // Drop the inode + cache entry.
        let _ = self.inodes.invalidate_path(path);
        if let Some((parent, name)) = split_parent(path) {
            self.forget_local_entry(&parent, &name);
        } else {
            self.cache.invalidate(path);
        }
        Ok(())
    }

    fn open(&self, ino: Ino) -> Result<FileHandleId, i32> {
        // Resolve the pCloud file_id for this inode. If the ino was never
        // populated from a directory listing, try to resolve it on demand
        // by listing the parent and re-visiting the child entry.
        let file_id = self.resolve_file_id(ino).map_err(|e| e.to_errno())?;

        // Ref-counted per-inode shared FileHandle: multiple concurrent opens
        // on the same inode share one upstream handle. `release` drops the
        // reference; when the last reference falls the backend is notified.
        let shared = {
            let tbl = self.handles.lock().map_err(|_| crate::errors::EIO)?;
            if let Some(existing) = tbl.by_ino.get(&ino) {
                Arc::clone(existing)
            } else {
                drop(tbl);
                let handle = self.file_backend.open(file_id).map_err(|e| e.to_errno())?;
                let shared = Arc::new(handle);
                let mut tbl = self.handles.lock().map_err(|_| crate::errors::EIO)?;
                tbl.by_ino.entry(ino).or_insert_with(|| Arc::clone(&shared));
                Arc::clone(tbl.by_ino.get(&ino).expect("just-inserted"))
            }
        };

        let mut tbl = self.handles.lock().map_err(|_| crate::errors::EIO)?;
        let id = tbl.next_id.checked_add(1).unwrap_or(1);
        tbl.next_id = id;
        tbl.by_id.insert(id, HandleSlot { ino, shared });
        Ok(id)
    }

    fn read(&self, handle: FileHandleId, offset: u64, len: usize) -> Result<Vec<u8>, i32> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let shared = {
            let tbl = self.handles.lock().map_err(|_| crate::errors::EIO)?;
            tbl.by_id.get(&handle).map(|s| Arc::clone(&s.shared))
        }
        .ok_or(EBADF)?;

        let page_size = self.options.page_cache.page_size as u64;
        let mut out = Vec::with_capacity(len);
        let mut cursor = offset;
        let end = offset.saturating_add(len as u64);

        while cursor < end {
            let page_index = cursor / page_size;
            let page_start = page_index * page_size;
            let page_key = PageKey {
                file_id: shared.file_id,
                page_index,
            };
            // Load page (cache or backend).
            let page_bytes = if let Some(b) = self.page_cache.get(page_key) {
                b
            } else {
                let fetched = self
                    .file_backend
                    .read(&shared, page_start, page_size as usize)
                    .map_err(|e| e.to_errno())?;
                // End-of-file: a short or empty read means we cannot proceed
                // past this point.
                if fetched.is_empty() {
                    break;
                }
                // Only cache full pages; a short trailing page is cached
                // anyway because it represents the EOF boundary and serving
                // it from cache on repeat reads is correct.
                self.page_cache.put(page_key, fetched.clone());
                fetched
            };

            let page_off = (cursor - page_start) as usize;
            if page_off >= page_bytes.len() {
                // Requested byte lies beyond observed EOF.
                break;
            }
            let take = (end - cursor).min((page_bytes.len() - page_off) as u64) as usize;
            out.extend_from_slice(&page_bytes[page_off..page_off + take]);
            cursor = cursor.saturating_add(take as u64);
            // If the backend returned a short page (EOF), stop; further
            // pages are empty.
            if page_bytes.len() < page_size as usize {
                break;
            }
        }

        Ok(out)
    }

    fn release(&self, handle: FileHandleId) -> Result<(), i32> {
        let mut tbl = self.handles.lock().map_err(|_| crate::errors::EIO)?;
        let slot = tbl.by_id.remove(&handle).ok_or(EBADF)?;
        let ino = slot.ino;
        drop(slot);
        // If this was the last per-ino reference, drop the shared entry
        // and notify the backend.
        let maybe_final = if let Some(shared) = tbl.by_ino.get(&ino) {
            // `shared` is one ref, `tbl.by_ino` holds one more. If strong
            // count is 1 after removing the table entry, the slot above
            // was the last outstanding handle.
            if Arc::strong_count(shared) == 1 {
                tbl.by_ino.remove(&ino)
            } else {
                None
            }
        } else {
            None
        };
        drop(tbl);
        if let Some(final_handle) = maybe_final {
            let _ = self.file_backend.release(&final_handle);
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Write-side delegates. All return ENOSYS when no WritePathService is
    // attached, matching the trait-default posture. When a writer is
    // attached (via `with_write_path`) the request is forwarded.
    // -------------------------------------------------------------------------

    fn create(&self, parent_path: &str, name: &str) -> Result<Ino, i32> {
        let writer = self.writer.as_ref().ok_or(ENOSYS)?;
        if name.is_empty() || name.contains('/') || name.contains('\0') {
            return Err(crate::errors::EINVAL);
        }
        let full = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let (ino, _gen) = self.inodes.insert_or_get(&full, FsEntryKind::RegularFile);
        writer.create(ino, parent_path, name)?;
        // Invalidate any stale cached listing for the parent so a
        // subsequent readdir sees the new child.
        self.cache.invalidate(parent_path);
        Ok(ino)
    }

    fn write(&self, ino: Ino, offset: u64, data: &[u8]) -> Result<usize, i32> {
        let writer = self.writer.as_ref().ok_or(ENOSYS)?;
        writer.write(ino, offset, data)
    }

    fn flush_write(&self, ino: Ino) -> Result<(), i32> {
        let writer = self.writer.as_ref().ok_or(ENOSYS)?;
        writer.flush(ino)
    }

    fn fsync_write(&self, ino: Ino) -> Result<(), i32> {
        let writer = self.writer.as_ref().ok_or(ENOSYS)?;
        writer.fsync(ino)
    }

    fn truncate(&self, ino: Ino, new_size: u64) -> Result<(), i32> {
        let writer = self.writer.as_ref().ok_or(ENOSYS)?;
        // If no write handle exists yet (pure truncate via setattr on an
        // existing inode that was never opened for write), open one
        // best-effort using the inode's resolved path.
        if let Err(errno) = writer.truncate(ino, new_size) {
            if errno == crate::errors::EINVAL {
                let path = self
                    .inodes
                    .resolve(ino)
                    .map(|(p, _, _)| p)
                    .ok_or(crate::errors::ENOENT)?;
                writer.open_for_write(ino, path, false, false)?;
                return writer.truncate(ino, new_size);
            }
            return Err(errno);
        }
        Ok(())
    }

    fn unlink(&self, parent_path: &str, name: &str) -> Result<(), i32> {
        let writer = self.writer.as_ref().ok_or(ENOSYS)?;
        if name.is_empty() || name.contains('/') || name.contains('\0') {
            return Err(crate::errors::EINVAL);
        }
        let full = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let ino = self.inodes.ino_for_path(&full);
        writer.unlink(ino, &full)?;
        self.inodes.invalidate_path(&full);
        self.cache.invalidate(parent_path);
        self.cache.invalidate(&full);
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), i32> {
        let writer = self.writer.as_ref().ok_or(ENOSYS)?;
        writer.rename(from, to)?;
        self.cache.invalidate(from);
        self.cache.invalidate(to);
        Ok(())
    }
}

impl<B: FolderBackend, F: FileBackend> ProtoFuseAdapter<B, F> {
    fn resolve_file_id(&self, ino: Ino) -> Result<u64, FsError> {
        if let Ok(ids) = self.file_ids.lock() {
            if let Some(&fid) = ids.get(&ino) {
                return Ok(fid);
            }
        }
        // On miss, list the parent directory which will populate file_ids
        // for every child as a side-effect.
        let path = self
            .inodes
            .resolve(ino)
            .map(|(p, _, _)| p)
            .ok_or(FsError::NotFound)?;
        let canon = canonicalise(&path).map_err(path_err_to_fs)?;
        let (parent, _name) = split_parent(&canon).ok_or(FsError::NotFound)?;
        let _ = self.fetch_directory(&parent)?;
        if let Ok(ids) = self.file_ids.lock() {
            if let Some(&fid) = ids.get(&ino) {
                return Ok(fid);
            }
        }
        Err(FsError::NotFound)
    }
}

fn split_parent(path: &str) -> Option<(String, String)> {
    if path == "/" {
        return None;
    }
    let idx = path.rfind('/')?;
    let parent = if idx == 0 {
        "/".to_owned()
    } else {
        path[..idx].to_owned()
    };
    let name = path[idx + 1..].to_owned();
    if name.is_empty() {
        return None;
    }
    Some((parent, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_adapter_lookup_returns_enosys() {
        let a = NullFuseAdapter;
        assert_eq!(a.lookup(1, "anything"), Err(ENOSYS));
    }

    #[test]
    fn null_adapter_getattr_returns_enosys() {
        let a = NullFuseAdapter;
        assert_eq!(a.getattr(1), Err(ENOSYS));
    }

    #[test]
    fn null_adapter_readdir_returns_enosys() {
        let a = NullFuseAdapter;
        assert_eq!(a.readdir(1, 0), Err(ENOSYS));
    }

    #[test]
    fn split_parent_handles_root_children() {
        assert_eq!(split_parent("/"), None);
        assert_eq!(
            split_parent("/docs"),
            Some(("/".to_owned(), "docs".to_owned()))
        );
        assert_eq!(
            split_parent("/a/b"),
            Some(("/a".to_owned(), "b".to_owned()))
        );
    }

    // -------------------------------------------------------------------------
    // ProtoFuseAdapter tests against a mock backend.
    // -------------------------------------------------------------------------

    use super::super::backend::mock::MockFolderBackend;
    use std::sync::Arc;

    fn seed_root_with_two_entries() -> Arc<ProtoFuseAdapter<MockFolderBackend>> {
        let backend = Arc::new(MockFolderBackend::new());
        backend.insert_dir(
            "/",
            10,
            vec![
                ("docs", true, Some(11), None),
                ("report.txt", false, None, Some(42)),
            ],
        );
        backend.insert_dir("/docs", 11, vec![("notes.md", false, None, Some(99))]);
        Arc::new(ProtoFuseAdapter::new(backend, AdapterOptions::default()))
    }

    #[test]
    fn readdir_on_root_returns_children() {
        let a = seed_root_with_two_entries();
        let entries = a.readdir(ROOT_INODE, 0).expect("readdir root");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "docs");
        assert_eq!(entries[0].kind, FsEntryKind::Directory);
        assert_eq!(entries[1].name, "report.txt");
        assert_eq!(entries[1].kind, FsEntryKind::RegularFile);
    }

    #[test]
    fn readdir_offset_is_honoured() {
        let a = seed_root_with_two_entries();
        let entries = a.readdir(ROOT_INODE, 1).expect("readdir root offset=1");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "report.txt");
        let past_end = a.readdir(ROOT_INODE, 99).expect("past-end returns empty");
        assert!(past_end.is_empty());
    }

    #[test]
    fn lookup_child_returns_entry_attr() {
        let a = seed_root_with_two_entries();
        let attr = a.lookup(ROOT_INODE, "docs").expect("lookup docs");
        assert_eq!(attr.kind, FsEntryKind::Directory);
        assert_ne!(attr.ino, ROOT_INODE);
    }

    #[test]
    fn lookup_missing_returns_enoent() {
        let a = seed_root_with_two_entries();
        let err = a.lookup(ROOT_INODE, "nope").unwrap_err();
        assert_eq!(err, crate::errors::ENOENT);
    }

    #[test]
    fn getattr_happy_path_after_lookup() {
        let a = seed_root_with_two_entries();
        let doc_attr = a.lookup(ROOT_INODE, "docs").unwrap();
        let again = a.getattr(doc_attr.ino).unwrap();
        assert_eq!(again, doc_attr);
    }

    #[test]
    fn getattr_not_found_for_unknown_ino() {
        let a = seed_root_with_two_entries();
        // Unknown ino never registered.
        let err = a.getattr(99_999).unwrap_err();
        assert_eq!(err, crate::errors::ENOENT);
    }

    #[test]
    fn lookup_maps_permission_denied() {
        let backend = Arc::new(MockFolderBackend::new());
        backend.insert_error("/", FsError::PermissionDenied);
        let a = ProtoFuseAdapter::new(backend, AdapterOptions::default());
        let err = a.lookup(ROOT_INODE, "anything").unwrap_err();
        assert_eq!(err, crate::errors::EACCES);
    }

    #[test]
    fn lookup_rejects_embedded_nul_as_einval() {
        let a = seed_root_with_two_entries();
        let err = a.lookup(ROOT_INODE, "bad\0name").unwrap_err();
        assert_eq!(err, crate::errors::EINVAL);
    }

    #[test]
    fn readdir_on_nested_dir() {
        let a = seed_root_with_two_entries();
        // First resolve the nested dir via lookup so the ino is known.
        let docs_attr = a.lookup(ROOT_INODE, "docs").unwrap();
        let entries = a.readdir(docs_attr.ino, 0).expect("readdir /docs");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "notes.md");
    }

    // -------------------------------------------------------------------------
    // 4.c: open/read/release tests with a mock FileBackend.
    // -------------------------------------------------------------------------

    use super::super::backend::mock::MockFileBackend;

    fn seed_with_file(
        file_id: u64,
        size: usize,
    ) -> (
        Arc<ProtoFuseAdapter<MockFolderBackend, MockFileBackend>>,
        Arc<MockFileBackend>,
        Ino,
    ) {
        let folder = Arc::new(MockFolderBackend::new());
        folder.insert_dir("/", 1, vec![("data.bin", false, None, Some(file_id))]);
        let files = Arc::new(MockFileBackend::new());
        files.insert_file(file_id, (0..size).map(|i| (i % 256) as u8).collect());
        let adapter = Arc::new(ProtoFuseAdapter::with_file_backend(
            folder,
            Arc::clone(&files),
            AdapterOptions {
                page_cache: PageCacheConfig {
                    page_size: 16,
                    max_bytes: 1024,
                },
                ..AdapterOptions::default()
            },
        ));
        let attr = adapter.lookup(ROOT_INODE, "data.bin").expect("lookup");
        (adapter, files, attr.ino)
    }

    #[test]
    fn open_read_release_returns_exact_bytes() {
        let (a, files, ino) = seed_with_file(42, 100);
        let handle = a.open(ino).expect("open");
        let bytes = a.read(handle, 10, 20).expect("read");
        let expected: Vec<u8> = (10u32..30).map(|i| (i % 256) as u8).collect();
        assert_eq!(bytes, expected);
        a.release(handle).expect("release");
        assert_eq!(files.releases.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn read_past_eof_returns_short_or_empty() {
        let (a, _files, ino) = seed_with_file(42, 50);
        let handle = a.open(ino).expect("open");
        // Request crosses EOF.
        let bytes = a.read(handle, 40, 100).expect("read");
        assert_eq!(bytes.len(), 10);
        // Fully past EOF.
        let empty = a.read(handle, 60, 10).expect("read past eof");
        assert!(empty.is_empty());
        a.release(handle).expect("release");
    }

    #[test]
    fn read_on_bad_handle_returns_ebadf() {
        let (a, _files, _ino) = seed_with_file(42, 50);
        let err = a.read(9_999, 0, 4).unwrap_err();
        assert_eq!(err, EBADF);
    }

    #[test]
    fn open_on_unknown_inode_returns_enoent() {
        let (a, _files, _ino) = seed_with_file(42, 50);
        let err = a.open(987_654).unwrap_err();
        assert_eq!(err, crate::errors::ENOENT);
    }

    #[test]
    fn concurrent_opens_share_single_upstream_handle() {
        let (a, files, ino) = seed_with_file(42, 256);
        let mut handles = Vec::new();
        for _ in 0..8 {
            let a = Arc::clone(&a);
            handles.push(std::thread::spawn(move || {
                let h = a.open(ino).expect("open");
                let b = a.read(h, 0, 32).expect("read");
                assert_eq!(b.len(), 32);
                a.release(h).expect("release");
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Every open above should have reused the per-ino shared handle
        // whenever another thread still held one; so total backend opens
        // is strictly less than 8 (and ≥ 1). Ranged over many runs the
        // invariant `opens ≤ 8` is always safe to assert.
        let opens = files.opens.load(std::sync::atomic::Ordering::Relaxed);
        assert!((1..=8).contains(&opens), "unexpected backend opens={opens}");
    }

    #[test]
    fn second_read_hits_page_cache() {
        let (a, files, ino) = seed_with_file(42, 256);
        let handle = a.open(ino).expect("open");
        let _ = a.read(handle, 0, 16).unwrap();
        let first_reads = files.reads.load(std::sync::atomic::Ordering::Relaxed);
        let _ = a.read(handle, 0, 16).unwrap();
        let second_reads = files.reads.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            first_reads, second_reads,
            "second read must be served from cache"
        );
        assert!(a.page_cache().hit_ratio() > 0.0);
        a.release(handle).expect("release");
    }

    #[test]
    fn concurrent_lookups_race_on_same_path() {
        // 8 threads look up the same path concurrently. None should error,
        // and all must agree on the resolved inode number.
        let a = seed_root_with_two_entries();
        let mut handles = Vec::new();
        for _ in 0..8 {
            let a = Arc::clone(&a);
            handles.push(std::thread::spawn(move || {
                for _ in 0..32 {
                    let attr = a.lookup(ROOT_INODE, "docs").expect("lookup must succeed");
                    assert_eq!(attr.kind, FsEntryKind::Directory);
                }
                a.lookup(ROOT_INODE, "docs").unwrap().ino
            }));
        }
        let mut inos = Vec::new();
        for h in handles {
            inos.push(h.join().unwrap());
        }
        let first = inos[0];
        for ino in &inos {
            assert_eq!(*ino, first, "all threads must observe the same ino");
        }
    }

    // -------------------------------------------------------------------------
    // Write-side trait delegation tests (bd-1du.4.e / row 85).
    // -------------------------------------------------------------------------

    use super::super::staging::StagingDir;
    use super::super::write_journal::WriteJournal;
    use super::super::write_path::mock::MockUploadBackend;
    use super::super::write_path::{WritePathOptions, WritePathService};

    fn build_rw_adapter() -> (
        Arc<ProtoFuseAdapter<MockFolderBackend, MockFileBackend>>,
        Arc<MockUploadBackend>,
        tempfile::TempDir,
    ) {
        let folder = Arc::new(MockFolderBackend::new());
        folder.insert_dir("/", 1, vec![]);
        let files = Arc::new(MockFileBackend::new());
        let tmp = tempfile::tempdir().expect("tempdir");
        let stage = StagingDir::open(tmp.path().join("stage")).expect("staging");
        let journal = WriteJournal::open(stage.journal_path()).expect("journal");
        let upload = Arc::new(MockUploadBackend::new());
        let writer = Arc::new(WritePathService::new(
            stage,
            journal,
            Arc::clone(&upload),
            WritePathOptions::default(),
        ));
        let adapter = Arc::new(
            ProtoFuseAdapter::with_file_backend(folder, files, AdapterOptions::default())
                .with_write_path(writer),
        );
        (adapter, upload, tmp)
    }

    #[test]
    fn write_side_returns_enosys_without_writer() {
        let a = seed_root_with_two_entries();
        assert!(!a.has_write_path());
        assert_eq!(a.create("/", "new.txt").unwrap_err(), ENOSYS);
        assert_eq!(a.write(999, 0, b"x").unwrap_err(), ENOSYS);
        assert_eq!(a.flush_write(999).unwrap_err(), ENOSYS);
        assert_eq!(a.fsync_write(999).unwrap_err(), ENOSYS);
        assert_eq!(a.truncate(999, 0).unwrap_err(), ENOSYS);
        assert_eq!(a.unlink("/", "x").unwrap_err(), ENOSYS);
        assert_eq!(a.rename("/a", "/b").unwrap_err(), ENOSYS);
    }

    #[test]
    fn create_allocates_inode_and_delegates_to_writer() {
        let (a, upload, _tmp) = build_rw_adapter();
        assert!(a.has_write_path());
        let ino = a.create("/", "hello.txt").expect("create");
        assert_ne!(ino, 0);
        let wrote = a.write(ino, 0, b"hi").expect("write");
        assert_eq!(wrote, 2);
        a.flush_write(ino).expect("flush");
        let uploads = upload.uploads.lock().unwrap();
        assert_eq!(uploads.get("/hello.txt").unwrap(), b"hi");
    }

    #[test]
    fn create_rejects_invalid_names() {
        let (a, _upload, _tmp) = build_rw_adapter();
        assert_eq!(a.create("/", "").unwrap_err(), crate::errors::EINVAL);
        assert_eq!(a.create("/", "a/b").unwrap_err(), crate::errors::EINVAL);
        assert_eq!(a.create("/", "a\0b").unwrap_err(), crate::errors::EINVAL);
    }

    #[test]
    fn fsync_write_forces_upload() {
        let (a, upload, _tmp) = build_rw_adapter();
        let ino = a.create("/", "f.txt").expect("create");
        a.write(ino, 0, b"abc").expect("write");
        a.fsync_write(ino).expect("fsync");
        let uploads = upload.uploads.lock().unwrap();
        assert_eq!(uploads.get("/f.txt").unwrap(), b"abc");
    }

    #[test]
    fn truncate_shrinks_blob_through_trait() {
        let (a, upload, _tmp) = build_rw_adapter();
        let ino = a.create("/", "t.bin").expect("create");
        a.write(ino, 0, b"0123456789").expect("write");
        a.truncate(ino, 4).expect("truncate");
        a.flush_write(ino).expect("flush");
        let uploads = upload.uploads.lock().unwrap();
        assert_eq!(uploads.get("/t.bin").unwrap(), b"0123");
    }

    #[test]
    fn unlink_through_trait_calls_backend_and_invalidates() {
        let (a, upload, _tmp) = build_rw_adapter();
        let ino = a.create("/", "gone.txt").expect("create");
        a.write(ino, 0, b"bye").expect("write");
        a.flush_write(ino).expect("flush");
        a.unlink("/", "gone.txt").expect("unlink");
        assert!(
            upload
                .unlinks
                .lock()
                .unwrap()
                .contains(&"/gone.txt".to_owned())
        );
    }

    #[test]
    fn rename_through_trait_delegates_and_invalidates_cache() {
        let (a, upload, _tmp) = build_rw_adapter();
        let ino = a.create("/", "old.txt").expect("create");
        a.write(ino, 0, b"x").expect("write");
        a.rename("/old.txt", "/new.txt").expect("rename");
        a.write(ino, 1, b"y").expect("write");
        a.flush_write(ino).expect("flush");
        let uploads = upload.uploads.lock().unwrap();
        assert_eq!(uploads.get("/new.txt").unwrap(), b"xy");
        assert!(!uploads.contains_key("/old.txt"));
    }

    #[test]
    fn resolve_ino_to_path_round_trip_via_lookup() {
        // Seed a 3-deep tree: /a/b/c with a file /a/b/c/leaf.txt.
        let backend = Arc::new(MockFolderBackend::new());
        backend.insert_dir("/", 1, vec![("a", true, Some(2), None)]);
        backend.insert_dir("/a", 2, vec![("b", true, Some(3), None)]);
        backend.insert_dir("/a/b", 3, vec![("c", true, Some(4), None)]);
        backend.insert_dir("/a/b/c", 4, vec![("leaf.txt", false, None, Some(17))]);
        let a = ProtoFuseAdapter::new(backend, AdapterOptions::default());

        // Walk lookup top-down so each ino is materialised in the table.
        let a_ino = a.lookup(ROOT_INODE, "a").expect("lookup a").ino;
        let b_ino = a.lookup(a_ino, "b").expect("lookup b").ino;
        let c_ino = a.lookup(b_ino, "c").expect("lookup c").ino;
        let leaf_ino = a.lookup(c_ino, "leaf.txt").expect("lookup leaf").ino;

        // Round-trip every level back to a PathBuf.
        assert_eq!(
            a.resolve_ino_to_path(ROOT_INODE).unwrap(),
            PathBuf::from("/")
        );
        assert_eq!(a.resolve_ino_to_path(a_ino).unwrap(), PathBuf::from("/a"));
        assert_eq!(a.resolve_ino_to_path(b_ino).unwrap(), PathBuf::from("/a/b"));
        assert_eq!(
            a.resolve_ino_to_path(c_ino).unwrap(),
            PathBuf::from("/a/b/c")
        );
        assert_eq!(
            a.resolve_ino_to_path(leaf_ino).unwrap(),
            PathBuf::from("/a/b/c/leaf.txt")
        );

        // Unknown ino yields ENOENT.
        assert_eq!(
            a.resolve_ino_to_path(9_999_999).unwrap_err(),
            crate::errors::ENOENT
        );
    }

    #[test]
    fn write_delegate_upload_failure_maps_to_erofs() {
        // Simulate the unlink/rename proto-missing case by forcing the mock
        // backend to emit an Upload error on the next upload; the delegate
        // must surface EROFS rather than a generic EIO so the kernel sees
        // a clean read-only signal.
        let (a, upload, _tmp) = build_rw_adapter();
        let ino = a.create("/", "err.txt").expect("create");
        *upload.fail_next_upload.lock().unwrap() = true;
        a.write(ino, 0, b"z").expect("write");
        let err = a.flush_write(ino).unwrap_err();
        assert_eq!(err, crate::errors::EROFS);
    }
}

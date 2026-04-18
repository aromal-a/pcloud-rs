#![warn(unsafe_op_in_unsafe_fn)]
// FS crate requires targeted unsafe for FUSE mount helpers and
// signal-safe unmount cleanup.
//! # pcloud-fs
//!
//! Filesystem shell for the Rust pcloud-rs path: inode model, journal,
//! page cache, staging, writeback, mount service scaffolding, and a FUSE
//! adapter. The daemon composes these into a live Linux FUSE mount via
//! `pcloud-daemon::mount_runtime::pcloud_shim_adapter_factory`, which
//! binds [`write_path::WritePathService`] to the transfer backend,
//! per-mount staging directory and on-disk write journal — so
//! `create` / `write` / `flush` / `fsync` / `unlink` / `rename` are
//! serviced by the real writer rather than returning `ENOSYS`
//! (bd-1du.4.6 footnote `[fuse-wiring]`; final parity gate remains
//! `bd-1du.10`). Mid-write flushes beyond the default 64 MiB
//! `flush_threshold_bytes` stream through the chunked
//! `upload_create` + `upload_write` (4 MiB) + `upload_save` pipeline
//! wired in the `WritePathService` chunked flush path when the
//! backend implements the chunked surface; otherwise the write path
//! falls back to a single `upload_file` call. Each successful flush is
//! surfaced on the observability layer via [`slo_hook::observe_flush`].
//!
//! **Architecture:** see `docs/book/src/architecture/crate-map.md`.
//! Consumed by `pcloud-daemon::mount_runtime`; the FUSE adapter bridges
//! to the `fuser` crate on Linux via `fuser_shim`.
//!
//! **Stability:** T1 internal — API is in flux until mount parity lands.
//!
//! **MSRV:** Rust 1.82 (workspace-pinned; edition 2024).
//!
//! **Features:** none.
//!
//! **Platform:** public API type-checks on all supported targets
//! (Linux / macOS / Windows / BSD) via `platform/{linux,macos,windows,bsd}`.
//! The live FUSE runtime is Linux-only; macOS (FSKit), Windows (WinFsp),
//! and BSD paths are currently scaffolding.

#![allow(clippy::collapsible_if)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM: all** (Linux | FreeBSD | macOS | Windows).
// **GATING: none at the crate root** -- per-OS gating lives inside
// `platform/{linux,macos,windows,bsd}.rs` and a few `#[cfg(target_os =
// "linux")]` blocks in `mount_service.rs` / `mount_orphan.rs` /
// `fuser_shim.rs`. The crate's top-level public API type-checks on any
// supported target.

pub mod backend;
pub mod errors;
pub mod fs_watcher;
pub mod fuse_adapter;
// `fuser_shim` uses `fuser` (Linux/FreeBSD only) and `libc::statvfs64`
// (Linux-only). Gate the module so cross-compilation targets (macOS,
// Windows, bare-metal) do not attempt to compile it. The `fuser` dep is
// already gated to `cfg(target_os = "linux")` in Cargo.toml; the module
// gate must match so the compiler does not try to resolve the import on
// unsupported platforms (bd-xplat-$OS, Phase 0).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod fuser_shim;
pub mod inode;
pub mod integrity_sweeper;
pub mod journal;
pub mod metadata_cache;
pub mod mount;
pub mod mount_orphan;
pub mod mount_service;
pub mod page_cache;
pub mod path_norm;
pub mod platform;
pub mod read_path;
pub mod slo_hook;
pub mod staging;
pub mod write_journal;
pub mod write_path;
pub mod writeback;

use crate::journal::JournalEntry;
use crate::mount::MountPolicyError;
use crate::read_path::{ReadPathError, ReadResult};

/// Cargo crate name. Exposed for diagnostic logging and integration tests.
pub const CRATE_NAME: &str = "pcloud-fs";

/// Composable filesystem shell that wires the mount policy, read path,
/// writeback queue, and journal into a single in-memory value used by
/// tests and scaffolding. **This is not a mounted runtime** — it exists
/// so the rest of the codebase can exercise the filesystem helpers
/// without a live FUSE session. See `bd-1du.4` for the real runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemShell {
    /// Mount-policy descriptor (read-only flag, `allow_other`, etc.).
    pub mount: mount::MountService,
    /// Read path that satisfies `read(2)` requests from the staging
    /// area and the page cache.
    pub reads: read_path::ReadPathService,
    /// Writeback queue that batches staged writes before flushing them
    /// through the journal.
    pub writeback: writeback::WritebackService,
    /// Crash-safe write journal used for durability of staged writes.
    pub journal: journal::WritebackJournal,
}

impl Default for FilesystemShell {
    fn default() -> Self {
        Self {
            mount: mount::MountService {
                allow_other: false,
                read_only: false,
            },
            reads: read_path::ReadPathService::default(),
            writeback: writeback::WritebackService::default(),
            journal: journal::WritebackJournal::default(),
        }
    }
}

impl FilesystemShell {
    /// Validate the configured mount policy, returning any policy violation
    /// such as `allow_other` combined with a writable mount.
    pub fn validate_mount_policy(&self) -> Result<(), MountPolicyError> {
        self.mount.validate()
    }

    /// Seed the staging area with a file and its contents. Intended for
    /// tests and deterministic fixtures; does not invoke the journal.
    pub fn seed_staged_file(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.writeback.staging.stage(path, bytes);
    }

    /// Stage an all-zero write of `bytes` size through the journal. This is
    /// a convenience used by tests to exercise flush-threshold behaviour.
    ///
    /// # Errors
    ///
    /// Returns [`crate::journal::JournalError::Full`] when the journal is
    /// at capacity. Callers must apply back-pressure.
    pub fn journal_write(
        &mut self,
        path: impl Into<String>,
        bytes: usize,
    ) -> Result<(), crate::journal::JournalError> {
        self.writeback
            .stage_write(&mut self.journal, path, vec![0u8; bytes])
    }

    /// Read `requested_bytes` starting at `offset` from the staged `path`.
    /// The first read fills the page cache from the staging area; subsequent
    /// reads within the prefetch window are served from the cache.
    pub fn read_staged_path(
        &mut self,
        path: &str,
        offset: usize,
        requested_bytes: usize,
    ) -> Result<ReadResult, ReadPathError> {
        self.reads
            .read(&self.writeback.staging, path, offset, requested_bytes)
    }

    /// Zero-copy reference to the staged buffer for `path`.
    ///
    /// Unlike [`Self::read_staged_path`], this does not populate or
    /// consult the page cache and does not allocate a new `Vec`. Returns
    /// `None` if no buffer is currently staged for `path`.
    ///
    /// Bead `pcloud-rs-s1p.88`: used by the sync loop's upload path to
    /// stream the payload in 4 MiB chunks without buffering the whole
    /// file.
    #[must_use]
    pub fn staged_bytes(&self, path: &str) -> Option<&[u8]> {
        self.writeback.staging.get(path)
    }

    /// Byte length of the staged buffer for `path`, or `None` if absent.
    #[must_use]
    pub fn staged_len(&self, path: &str) -> Option<usize> {
        self.writeback.staging.get(path).map(<[u8]>::len)
    }

    /// Return an iterator over fixed-size chunks of the staged buffer
    /// for `path`. Each yielded slice borrows from the staging area —
    /// no per-chunk heap allocation occurs.
    ///
    /// `chunk_size` must be non-zero; callers that pass `0` get a
    /// single chunk covering the whole buffer (behaviour inherited
    /// from the underlying `slice::chunks` when guarded below).
    ///
    /// Bead `pcloud-rs-s1p.88`: pairs with [`Self::staged_bytes`] to
    /// bound sync-loop upload memory to a single chunk at a time.
    #[must_use]
    pub fn staged_chunks<'a>(
        &'a self,
        path: &str,
        chunk_size: usize,
    ) -> Option<std::slice::Chunks<'a, u8>> {
        let bytes = self.writeback.staging.get(path)?;
        let cs = if chunk_size == 0 {
            bytes.len().max(1)
        } else {
            chunk_size
        };
        Some(bytes.chunks(cs))
    }

    /// Flush up to `max_entries` pending writes from the journal, returning
    /// the drained [`JournalEntry`] values in FIFO order.
    pub fn flush_writeback(&mut self, max_entries: usize) -> Vec<JournalEntry> {
        self.writeback.flush(&mut self.journal, max_entries)
    }

    /// Produce a one-line human-readable summary of the shell's state.
    /// Used by CLI diagnostics and tests.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "fs(read_only={}, allow_other={}, flush_threshold={}KiB, journal_pending={}, staged_writes={}, completed_writes={})",
            self.mount.read_only,
            self.mount.allow_other,
            self.writeback.flush_threshold_bytes / 1024,
            self.journal.pending_count(),
            self.writeback.staged_file_count(),
            self.writeback.completed_writes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::FilesystemShell;
    use crate::mount::MountPolicyError;
    use crate::read_path::ReadSource;

    #[test]
    fn summary_reflects_pending_journal_entries() {
        let mut fs = FilesystemShell::default();
        fs.journal_write("docs/report.txt", 5).expect("stage");

        assert!(fs.summary().contains("journal_pending=1"));
        assert!(fs.summary().contains("staged_writes=1"));
    }

    #[test]
    fn flush_writeback_drains_journal_and_updates_summary() {
        let mut fs = FilesystemShell::default();
        fs.journal_write("docs/report.txt", 5).expect("stage");

        let drained = fs.flush_writeback(10);

        assert_eq!(drained.len(), 1);
        assert!(fs.summary().contains("journal_pending=0"));
        assert!(fs.summary().contains("completed_writes=1"));
    }

    #[test]
    fn staged_reads_succeed_and_hit_cache_on_second_read() {
        let mut fs = FilesystemShell::default();
        fs.writeback
            .staging
            .stage("docs/report.txt", b"draft-data".to_vec());
        fs.reads.prefetch_window_bytes = 5;

        let first = fs.read_staged_path("docs/report.txt", 1, 3).unwrap();
        let second = fs.read_staged_path("docs/report.txt", 1, 3).unwrap();

        assert_eq!(first.bytes, b"raf");
        assert_eq!(first.source, ReadSource::Stage);
        assert_eq!(second.source, ReadSource::Cache);
    }

    #[test]
    fn staged_reads_fail_for_missing_paths() {
        let mut fs = FilesystemShell::default();

        let error = fs.read_staged_path("missing.txt", 0, 8).unwrap_err();

        assert!(matches!(
            error,
            crate::read_path::ReadPathError::MissingPath { .. }
        ));
    }

    #[test]
    fn mount_policy_validation_rejects_allow_other_write_mounts() {
        let fs = FilesystemShell {
            mount: crate::mount::MountService {
                allow_other: true,
                read_only: false,
            },
            ..FilesystemShell::default()
        };

        assert_eq!(
            fs.validate_mount_policy(),
            Err(MountPolicyError::AllowOtherRequiresReadOnly)
        );
        assert!(fs.summary().contains("allow_other=true"));
    }
}

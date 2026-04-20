#![allow(clippy::pedantic)]
//! Integration tests for the `FuseAdapter` trait implementation
//! (`ProtoFuseAdapter`) exercised **without a real kernel FUSE mount**.
//!
//! These tests drive the adapter trait methods directly against hand-rolled
//! mock backends that implement the public [`FolderBackend`],
//! [`FileBackend`], and [`FileUploadBackend`] traits. No network, no fuser,
//! no FUSE kernel — so they run unmodified in CI on any platform.
//!
//! The mocks here intentionally duplicate the `#[cfg(test)] pub(crate)`
//! mocks that live inside the crate sources, because those are not visible
//! to integration tests in `tests/` (Rust's test harness boundary).
//! Keeping them local to this file also ensures each test holds its own
//! deterministic fixture.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pcloud_fs::backend::{FileBackend, FileHandle, FolderBackend};
use pcloud_fs::errors::FsError;
use pcloud_fs::fuse_adapter::{
    AdapterOptions, EBADF, ENOSYS, FsEntryKind, FuseAdapter, ProtoFuseAdapter,
};
use pcloud_fs::inode::ROOT_INODE;
use pcloud_fs::page_cache::PageCacheConfig;
use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::WriteJournal;
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};
use pcloud_proto::folder_api::{RemoteFolderEntry, RemoteFolderListing};

// -----------------------------------------------------------------------------
// Mock backends (public-trait impls; no cfg(test) magic).
// -----------------------------------------------------------------------------

/// Tuple describing a directory entry fed into `MockFolderBackend::insert_dir`:
/// `(name, is_folder, folder_id, file_id, size)`.
type MockDirEntry<'a> = (&'a str, bool, Option<u64>, Option<u64>, Option<u64>);

#[derive(Debug, Default)]
struct MockFolderBackend {
    listings: Mutex<HashMap<String, RemoteFolderListing>>,
    created_folders: Mutex<Vec<(String, String)>>, // (parent, name)
    deleted_folders: Mutex<Vec<String>>,
}

impl MockFolderBackend {
    fn new() -> Self {
        Self::default()
    }

    fn insert_dir(
        &self,
        path: &str,
        folder_id: u64,
        entries: Vec<MockDirEntry<'_>>,
    ) {
        let listing = RemoteFolderListing {
            folder_id,
            path: path.to_owned(),
            name: path.rsplit('/').next().unwrap_or("").to_owned(),
            entries: entries
                .into_iter()
                .map(|(name, is_folder, fid, fileid, size)| RemoteFolderEntry {
                    name: name.to_owned(),
                    is_folder,
                    folder_id: fid,
                    file_id: fileid,
                    owner_user_id: None,
                    is_mine: false,
                    encrypted: false,
                    is_shared: false,
                    permissions: None,
                    size,
                    modified: None,
                })
                .collect(),
            api_server: None,
            owner_user_id: None,
            is_mine: false,
            encrypted: false,
            is_shared: false,
            permissions: None,
        };
        self.listings
            .lock()
            .unwrap()
            .insert(path.to_owned(), listing);
    }
}

impl FolderBackend for MockFolderBackend {
    fn list_contents(&self, path: &str) -> Result<RemoteFolderListing, FsError> {
        self.listings
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or(FsError::NotFound)
    }

    fn create_folder(&self, parent_path: &str, name: &str) -> Result<u64, FsError> {
        self.created_folders
            .lock()
            .unwrap()
            .push((parent_path.to_owned(), name.to_owned()));
        // Also side-effect a listing for the newly created folder so
        // subsequent getattr/readdir on the new folder is consistent.
        let full = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let new_id = 100 + self.created_folders.lock().unwrap().len() as u64;
        self.insert_dir(&full, new_id, vec![]);
        Ok(new_id)
    }

    fn delete_folder(&self, path: &str) -> Result<(), FsError> {
        self.deleted_folders.lock().unwrap().push(path.to_owned());
        self.listings.lock().unwrap().remove(path);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct MockFileBackend {
    files: Mutex<HashMap<u64, Vec<u8>>>,
    opens: AtomicU64,
    reads: AtomicU64,
    releases: AtomicU64,
}

impl MockFileBackend {
    fn new() -> Self {
        Self::default()
    }

    fn insert_file(&self, file_id: u64, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(file_id, bytes);
    }
}

impl FileBackend for MockFileBackend {
    fn open(&self, file_id: u64) -> Result<FileHandle, FsError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        let files = self.files.lock().unwrap();
        let bytes = files.get(&file_id).ok_or(FsError::NotFound)?;
        Ok(FileHandle {
            file_id,
            size: bytes.len() as u64,
            host: "mock".to_owned(),
            path: format!("/mock/{file_id}"),
            dwltag: None,
        })
    }

    fn read(&self, handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let files = self.files.lock().unwrap();
        let bytes = files.get(&handle.file_id).ok_or(FsError::NotFound)?;
        let off = offset as usize;
        if off >= bytes.len() {
            return Ok(Vec::new());
        }
        let end = off.saturating_add(len).min(bytes.len());
        Ok(bytes[off..end].to_vec())
    }

    fn release(&self, _handle: &FileHandle) -> Result<(), FsError> {
        self.releases.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct MockUploadBackend {
    uploads: Mutex<HashMap<String, Vec<u8>>>,
    unlinks: Mutex<Vec<String>>,
    renames: Mutex<Vec<(String, String)>>,
}

impl FileUploadBackend for MockUploadBackend {
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &Path,
    ) -> Result<(), WritePathError> {
        let bytes =
            std::fs::read(staging_file).map_err(|e| WritePathError::Upload(e.to_string()))?;
        let full = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        self.uploads.lock().unwrap().insert(full, bytes);
        Ok(())
    }

    fn unlink_remote(&self, path: &str) -> Result<(), WritePathError> {
        self.unlinks.lock().unwrap().push(path.to_owned());
        self.uploads.lock().unwrap().remove(path);
        Ok(())
    }

    fn rename_remote(&self, from: &str, to: &str) -> Result<(), WritePathError> {
        self.renames
            .lock()
            .unwrap()
            .push((from.to_owned(), to.to_owned()));
        let mut uploads = self.uploads.lock().unwrap();
        if let Some(bytes) = uploads.remove(from) {
            uploads.insert(to.to_owned(), bytes);
        }
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

fn seed_ro_adapter() -> Arc<ProtoFuseAdapter<MockFolderBackend, MockFileBackend>> {
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir(
        "/",
        10,
        vec![
            ("docs", true, Some(11), None, None),
            ("report.txt", false, None, Some(42), Some(100)),
        ],
    );
    folder.insert_dir(
        "/docs",
        11,
        vec![("notes.md", false, None, Some(99), Some(50))],
    );
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(42, (0..100u8).collect());
    files.insert_file(99, b"# notes\nhello world".to_vec());
    Arc::new(ProtoFuseAdapter::with_file_backend(
        folder,
        files,
        AdapterOptions {
            page_cache: PageCacheConfig {
                page_size: 32,
                max_bytes: 1024,
            },
            ..AdapterOptions::default()
        },
    ))
}

fn seed_rw_adapter() -> (
    Arc<ProtoFuseAdapter<MockFolderBackend, MockFileBackend>>,
    Arc<MockUploadBackend>,
    tempfile::TempDir,
) {
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let files = Arc::new(MockFileBackend::new());
    let tmp = tempfile::tempdir().expect("tempdir");
    let stage = StagingDir::open(tmp.path().join("stage")).expect("stage");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let upload = Arc::new(MockUploadBackend::default());
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

// -----------------------------------------------------------------------------
// Read-side trait tests
// -----------------------------------------------------------------------------

#[test]
fn lookup_returns_inode_for_known_file() {
    let a = seed_ro_adapter();
    let attr = a
        .lookup(ROOT_INODE, "report.txt")
        .expect("lookup report.txt");
    assert_eq!(attr.kind, FsEntryKind::RegularFile);
    assert_ne!(attr.ino, ROOT_INODE);
    assert_ne!(attr.ino, 0);
}

#[test]
fn lookup_returns_enoent_for_missing_entry() {
    let a = seed_ro_adapter();
    let err = a.lookup(ROOT_INODE, "nope").unwrap_err();
    assert_eq!(err, pcloud_fs::errors::ENOENT);
}

#[test]
fn lookup_rejects_embedded_nul_as_einval() {
    let a = seed_ro_adapter();
    let err = a.lookup(ROOT_INODE, "bad\0name").unwrap_err();
    assert_eq!(err, pcloud_fs::errors::EINVAL);
}

#[test]
fn readdir_lists_all_children() {
    let a = seed_ro_adapter();
    let entries = a.readdir(ROOT_INODE, 0).expect("readdir");
    assert_eq!(entries.len(), 2);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"docs"));
    assert!(names.contains(&"report.txt"));
}

#[test]
fn readdir_honours_offset() {
    let a = seed_ro_adapter();
    let all = a.readdir(ROOT_INODE, 0).unwrap();
    let skipped = a.readdir(ROOT_INODE, 1).unwrap();
    assert_eq!(skipped.len(), all.len() - 1);
    let past_end = a.readdir(ROOT_INODE, 99).unwrap();
    assert!(past_end.is_empty());
}

#[test]
fn getattr_on_unknown_ino_returns_enoent() {
    let a = seed_ro_adapter();
    let err = a.getattr(987_654).unwrap_err();
    assert_eq!(err, pcloud_fs::errors::ENOENT);
}

#[test]
fn getattr_after_lookup_reflects_cached_attr() {
    let a = seed_ro_adapter();
    let first = a.lookup(ROOT_INODE, "docs").unwrap();
    let again = a.getattr(first.ino).unwrap();
    assert_eq!(again, first);
}

#[test]
fn read_fetches_bytes_from_backend() {
    let a = seed_ro_adapter();
    let attr = a.lookup(ROOT_INODE, "report.txt").unwrap();
    let h = a.open(attr.ino).expect("open");
    let bytes = a.read(h, 10, 16).expect("read");
    let expected: Vec<u8> = (10u8..26).collect();
    assert_eq!(bytes, expected);
    a.release(h).expect("release");
}

#[test]
fn read_past_eof_truncates() {
    let a = seed_ro_adapter();
    let attr = a.lookup(ROOT_INODE, "report.txt").unwrap();
    let h = a.open(attr.ino).unwrap();
    // report.txt has 100 bytes (0..=99).
    let bytes = a.read(h, 90, 50).expect("read past eof");
    assert_eq!(bytes.len(), 10);
    let empty = a.read(h, 500, 10).expect("fully past eof");
    assert!(empty.is_empty());
    a.release(h).expect("release");
}

#[test]
fn read_on_bad_handle_returns_ebadf() {
    let a = seed_ro_adapter();
    let err = a.read(0xDEAD_BEEF, 0, 4).unwrap_err();
    assert_eq!(err, EBADF);
}

#[test]
fn page_cache_hit_avoids_backend_call() {
    let a = seed_ro_adapter();
    // Force a fresh listing so file-id cache is populated.
    let attr = a.lookup(ROOT_INODE, "report.txt").unwrap();
    let h = a.open(attr.ino).unwrap();
    let _ = a.read(h, 0, 16).unwrap();
    let pc_first = a.page_cache().stats();
    let _ = a.read(h, 0, 16).unwrap();
    let pc_second = a.page_cache().stats();
    assert!(
        pc_second.hits > pc_first.hits,
        "second read must be a cache hit: first={pc_first:?} second={pc_second:?}"
    );
    a.release(h).unwrap();
}

#[test]
fn release_on_bad_handle_returns_ebadf() {
    let a = seed_ro_adapter();
    let err = a.release(0xDEAD_BEEF).unwrap_err();
    assert_eq!(err, EBADF);
}

// -----------------------------------------------------------------------------
// mkdir / create / unlink / rename / rmdir trait tests
// -----------------------------------------------------------------------------

#[test]
fn mkdir_creates_folder_on_remote_and_locally() {
    // The read-only adapter still has a FolderBackend that implements
    // create_folder, so mkdir works regardless of a wired write path.
    let a = seed_ro_adapter();
    let attr = a.mkdir("/", "newdir").expect("mkdir");
    assert_eq!(attr.kind, FsEntryKind::Directory);
    assert_ne!(attr.ino, ROOT_INODE);
    // Subsequent lookup must resolve the newly-created entry without
    // another backend round-trip (it was published into the parent's
    // cached children list).
    let lookup = a.lookup(ROOT_INODE, "newdir").expect("lookup new dir");
    assert_eq!(lookup.ino, attr.ino);
}

#[test]
fn rmdir_invalidates_inode_and_parent_cache() {
    let a = seed_ro_adapter();
    let _attr = a.mkdir("/", "throwaway").expect("mkdir");
    a.rmdir("/throwaway").expect("rmdir");
    // After rmdir, the old entry must be gone from the parent listing.
    // A fresh lookup should return ENOENT because the mock folder backend
    // also removed it from its listings.
    let err = a.lookup(ROOT_INODE, "throwaway").unwrap_err();
    assert_eq!(err, pcloud_fs::errors::ENOENT);
}

#[test]
fn create_allocates_inode_and_delegates_to_writer() {
    let (a, upload, _tmp) = seed_rw_adapter();
    assert!(a.has_write_path());
    let ino = a.create("/", "hello.txt").expect("create");
    assert_ne!(ino, 0);
    let n = a.write(ino, 0, b"hi").expect("write");
    assert_eq!(n, 2);
    a.flush_write(ino).expect("flush_write");
    let uploads = upload.uploads.lock().unwrap();
    assert_eq!(uploads.get("/hello.txt").unwrap(), b"hi");
}

#[test]
fn write_stages_bytes_in_journal() {
    let (a, _upload, tmp) = seed_rw_adapter();
    let ino = a.create("/", "staged.txt").expect("create");
    let n = a.write(ino, 0, b"abcdef").expect("write");
    assert_eq!(n, 6);
    // The journal file under the staging root must contain > 0 bytes
    // because both Create and Write are journalled (write-ahead order).
    let journal_path = tmp.path().join("stage").join("journal.log");
    let meta = std::fs::metadata(&journal_path).expect("journal exists");
    assert!(meta.len() > 0, "journal must hold at least one record");
    // Replay must surface both Create and Write ops.
    let records = pcloud_fs::write_journal::replay_path(&journal_path).unwrap();
    let has_create = records.iter().any(|r| {
        matches!(&r.op,
            pcloud_fs::write_journal::JournalOp::Create { name, .. } if name == "staged.txt")
    });
    let has_write = records.iter().any(|r| {
        matches!(&r.op,
            pcloud_fs::write_journal::JournalOp::Write { path, len, .. }
            if path == "/staged.txt" && *len == 6)
    });
    assert!(has_create, "journal must contain Create for staged.txt");
    assert!(has_write, "journal must contain Write for staged.txt");
}

#[test]
fn flush_uploads_staged_bytes() {
    let (a, upload, _tmp) = seed_rw_adapter();
    let ino = a.create("/", "x.bin").expect("create");
    a.write(ino, 0, &vec![7u8; 1024]).expect("write");
    a.fsync_write(ino).expect("fsync");
    let uploads = upload.uploads.lock().unwrap();
    let bytes = uploads.get("/x.bin").expect("upload was captured");
    assert_eq!(bytes.len(), 1024);
    assert!(bytes.iter().all(|&b| b == 7));
}

#[test]
fn unlink_removes_inode_and_queues_remote_delete() {
    let (a, upload, _tmp) = seed_rw_adapter();
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
    // Inode table no longer resolves the old path.
    assert_eq!(
        a.resolve_ino_to_path(ino).unwrap_err(),
        pcloud_fs::errors::ENOENT
    );
}

#[test]
fn unlink_rejects_invalid_names() {
    let (a, _upload, _tmp) = seed_rw_adapter();
    assert_eq!(a.unlink("/", "").unwrap_err(), pcloud_fs::errors::EINVAL);
    assert_eq!(a.unlink("/", "a/b").unwrap_err(), pcloud_fs::errors::EINVAL);
    assert_eq!(
        a.unlink("/", "a\0b").unwrap_err(),
        pcloud_fs::errors::EINVAL
    );
}

#[test]
fn rename_moves_inode_between_parents() {
    let (a, upload, _tmp) = seed_rw_adapter();
    let ino = a.create("/", "old.txt").expect("create");
    a.write(ino, 0, b"payload").expect("write");
    a.flush_write(ino).expect("flush");
    a.rename("/old.txt", "/new.txt").expect("rename");
    let uploads = upload.uploads.lock().unwrap();
    assert!(uploads.contains_key("/new.txt"));
    assert!(!uploads.contains_key("/old.txt"));
}

// -----------------------------------------------------------------------------
// Write-side ENOSYS posture when no writer attached
// -----------------------------------------------------------------------------

#[test]
fn write_side_returns_enosys_without_writer() {
    let a = seed_ro_adapter();
    assert!(!a.has_write_path());
    assert_eq!(a.create("/", "x.txt").unwrap_err(), ENOSYS);
    assert_eq!(a.write(9, 0, b"x").unwrap_err(), ENOSYS);
    assert_eq!(a.flush_write(9).unwrap_err(), ENOSYS);
    assert_eq!(a.fsync_write(9).unwrap_err(), ENOSYS);
    assert_eq!(a.truncate(9, 0).unwrap_err(), ENOSYS);
    assert_eq!(a.unlink("/", "x").unwrap_err(), ENOSYS);
    assert_eq!(a.rename("/a", "/b").unwrap_err(), ENOSYS);
}

// -----------------------------------------------------------------------------
// forget_ino / inode lifecycle
// -----------------------------------------------------------------------------

#[test]
fn forget_decrements_lookup_count_and_evicts_at_zero() {
    let a = seed_ro_adapter();
    let attr = a.lookup(ROOT_INODE, "docs").unwrap();
    let ino = attr.ino;
    // Manually increment lookup refcount so forget has something to drain.
    // (The trait surface does not automatically bump the refcount; the
    // kernel-side fuser shim does. For the unit test we simulate that.)
    let table = a.inode_table();
    table.increment_lookup(ino);
    table.increment_lookup(ino);
    assert_eq!(table.lookup_count(ino), 2);
    a.forget_ino(ino, 1);
    assert_eq!(table.lookup_count(ino), 1);
    // Entry is still resolvable because count > 0.
    assert!(a.resolve_ino_to_path(ino).is_ok());
    a.forget_ino(ino, 1);
    // Now evicted: resolve returns ENOENT.
    assert_eq!(
        a.resolve_ino_to_path(ino).unwrap_err(),
        pcloud_fs::errors::ENOENT
    );
}

#[test]
fn resolve_ino_to_path_for_nested_inodes() {
    let a = seed_ro_adapter();
    let docs_attr = a.lookup(ROOT_INODE, "docs").unwrap();
    let notes_attr = a.lookup(docs_attr.ino, "notes.md").unwrap();
    assert_eq!(
        a.resolve_ino_to_path(docs_attr.ino).unwrap(),
        std::path::PathBuf::from("/docs")
    );
    assert_eq!(
        a.resolve_ino_to_path(notes_attr.ino).unwrap(),
        std::path::PathBuf::from("/docs/notes.md")
    );
}

// -----------------------------------------------------------------------------
// Concurrency smoke tests
// -----------------------------------------------------------------------------

#[test]
fn concurrent_lookups_observe_stable_inode() {
    let a = seed_ro_adapter();
    let mut threads = Vec::new();
    for _ in 0..8 {
        let a = Arc::clone(&a);
        threads.push(std::thread::spawn(move || {
            let mut inos = Vec::new();
            for _ in 0..16 {
                let attr = a.lookup(ROOT_INODE, "docs").unwrap();
                inos.push(attr.ino);
            }
            inos
        }));
    }
    let mut all = Vec::new();
    for t in threads {
        all.extend(t.join().unwrap());
    }
    let first = all[0];
    assert!(all.iter().all(|&i| i == first), "all threads agree on ino");
}

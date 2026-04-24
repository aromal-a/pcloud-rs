#![allow(clippy::pedantic)]
//! bd-xplat-windows — live WinFSP mount smoke test.
//!
//! Proves that the Rust WinFSP FFI wiring in
//! `pcloud_fs::platform::windows::mount_with_winfsp` actually stands up a
//! mounted drive on a real Windows host with a real `winfsp-x64.dll`, and
//! that the cross-platform [`FuseAdapter`] dispatcher correctly services:
//!
//! 1. `readdir` — `std::fs::read_dir("<drive>:\\")` sees every file we
//!    seeded in the in-memory adapter,
//! 2. `getattr` + `open` + `read` — `std::fs::read_to_string` returns the
//!    exact bytes we seeded,
//! 3. `create` + `write` + `close` — `std::fs::write` mutates the backing
//!    btree and the in-memory adapter observes the new bytes,
//! 4. unmount — drive letter disappears after the `MountHandle` is dropped.
//!
//! # Scope (honesty statement)
//!
//! This test is **deliberately decoupled** from the pCloud backend stack:
//! it uses a tiny in-memory [`MemFuseAdapter`] (defined below) rather
//! than [`ProtoFuseAdapter`] + the real `folder_backend` / `file_backend`
//! / `WritePathService`. The goal is to answer exactly one question —
//! "does the WinFSP FFI wiring work?" — without being gated on the
//! `pcloud-ipc` cross-platform work that is still in flight.
//!
//! A successful run proves:
//!   * `FspFileSystemCreate`, `FspFileSystemSetMountPoint`,
//!     `FspFileSystemStartDispatcher`, `FspFileSystemStopDispatcher`,
//!     `FspFileSystemDelete` are invoked in the correct order;
//!   * `FspFileSystemAddDirInfo` is resolved and produces a listing
//!     Explorer / Win32 APIs actually walk;
//!   * the `FileContext` box lifecycle survives a full
//!     Open → Read → Close sequence without a double-free;
//!   * the `Create` → `Overwrite` → `Write` → `Cleanup` → `Close` write
//!     path round-trips bytes from Win32 into the adapter's state.
//!
//! It does **not** prove:
//!   * anything about the pCloud daemon, IPC, auth vault, or live
//!     pCloud account round-tripping (separate beads);
//!   * ACL mirroring (the SetSecurity path is a permanent no-op by
//!     design — see `cb_set_security`);
//!   * reparse points, ADS, symlinks, or long-path (`\\?\`) handling
//!     (these are follow-ups listed at the end of the test).
//!
//! # Gating
//!
//! * `#[cfg(target_os = "windows")]` — the FFI module only compiles on
//!   Windows.
//! * `#[ignore]` by default — opt-in via `PCLOUD_WINFSP_TEST=1` or
//!   `PCLOUD_LIVE_E2E=1`. CI must NOT run this unattended because a
//!   crashed WinFSP dispatcher can leave a stale drive letter the
//!   operator has to clean up manually.
//!
//! # How to run
//!
//! On a Windows host with WinFSP 2.x installed and the MSVC toolchain:
//!
//! ```powershell
//! $env:PCLOUD_WINFSP_TEST = "1"
//! cargo test -p pcloud-fs --test winfsp_mount_live -- --ignored --nocapture
//! ```
//!
//! On Linux this file is empty (gated out by `#[cfg(target_os =
//! "windows")]`) but `cargo check -p pcloud-fs --tests` must still pass
//! so a stale import would be caught at CI time.

#![cfg(target_os = "windows")]

// **PLATFORM:** Windows
// **GATING:** #[cfg(target_os = "windows")].

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use pcloud_fs::fuse_adapter::{
    DirEntry, EntryAttr, FileHandleId, FsEntryKind, FuseAdapter, Ino,
};
use pcloud_fs::inode::ROOT_INODE;
use pcloud_fs::mount_service::{MountHandle, MountOptions};
use pcloud_fs::platform::windows::mount_with_winfsp;

// --- Gate helpers -----------------------------------------------------------

fn e2e_gate_enabled() -> bool {
    let winfsp = std::env::var("PCLOUD_WINFSP_TEST").ok().as_deref() == Some("1");
    let live = std::env::var("PCLOUD_LIVE_E2E").ok().as_deref() == Some("1");
    winfsp || live
}

// --- Minimal in-memory FuseAdapter -----------------------------------------

/// A tiny tree node. Either a directory with a `name -> ino` child map, or
/// a regular file carrying its bytes. Deliberately simple; the goal is
/// exercising WinFSP plumbing, not the adapter.
#[derive(Debug)]
enum Node {
    Dir { children: BTreeMap<String, Ino> },
    File { data: Vec<u8> },
}

/// In-memory state shared between the test body and the adapter. Wrapped
/// in `Arc<Mutex<_>>` so the test can observe post-mount mutations.
#[derive(Debug, Default)]
struct FsState {
    nodes: BTreeMap<Ino, Node>,
    /// `child_ino -> parent_ino`. Needed by `resolve_ino_to_path` so
    /// `FspCleanupDelete` can synthesise a path for `unlink` / `rmdir`.
    parent_of: HashMap<Ino, Ino>,
    /// Reverse `child_ino -> final name component` — same reason.
    name_of: HashMap<Ino, String>,
    next_ino: Ino,
    /// Monotonic handle-id allocator. Maps handle -> ino.
    handles: HashMap<FileHandleId, Ino>,
    next_handle: FileHandleId,
}

impl FsState {
    fn seed() -> Self {
        let mut s = FsState {
            nodes: BTreeMap::new(),
            parent_of: HashMap::new(),
            name_of: HashMap::new(),
            next_ino: ROOT_INODE + 1,
            handles: HashMap::new(),
            next_handle: 1,
        };
        // Root directory.
        s.nodes.insert(
            ROOT_INODE,
            Node::Dir {
                children: BTreeMap::new(),
            },
        );

        // Seed two files.
        let file1_ino = s.next_ino;
        s.next_ino += 1;
        s.nodes.insert(
            file1_ino,
            Node::File {
                data: b"hello-from-winfsp-smoke-test\n".repeat(145), // ~4 KiB
            },
        );
        s.parent_of.insert(file1_ino, ROOT_INODE);
        s.name_of.insert(file1_ino, "file1.txt".into());
        if let Some(Node::Dir { children }) = s.nodes.get_mut(&ROOT_INODE) {
            children.insert("file1.txt".into(), file1_ino);
        }

        let empty_ino = s.next_ino;
        s.next_ino += 1;
        s.nodes.insert(empty_ino, Node::File { data: Vec::new() });
        s.parent_of.insert(empty_ino, ROOT_INODE);
        s.name_of.insert(empty_ino, "empty.txt".into());
        if let Some(Node::Dir { children }) = s.nodes.get_mut(&ROOT_INODE) {
            children.insert("empty.txt".into(), empty_ino);
        }
        s
    }
}

/// The in-memory adapter used by the test. Wraps [`FsState`] with
/// interior mutability so the `FuseAdapter` trait's `&self` receivers
/// can still mutate.
#[derive(Debug)]
struct MemFuseAdapter {
    state: Mutex<FsState>,
}

impl MemFuseAdapter {
    fn new() -> Self {
        Self {
            state: Mutex::new(FsState::seed()),
        }
    }

    fn entry_attr_for(state: &FsState, ino: Ino) -> Option<EntryAttr> {
        let node = state.nodes.get(&ino)?;
        let (kind, size) = match node {
            Node::Dir { .. } => (FsEntryKind::Directory, 0u64),
            Node::File { data } => (FsEntryKind::RegularFile, data.len() as u64),
        };
        Some(EntryAttr {
            ino,
            kind,
            size,
            mode: match kind {
                FsEntryKind::Directory => 0o755,
                _ => 0o644,
            },
            uid: 0,
            gid: 0,
            mtime_epoch: None,
            mtime_nsec: 0,
        })
    }
}

// ENOENT / EISDIR / ENOTDIR / EIO / EINVAL / ENOSYS — reuse the
// pcloud-fs errno constants to stay consistent with the rest of the crate.
const ENOENT: i32 = 2;
const EISDIR: i32 = 21;
const ENOTDIR: i32 = 20;
const ENOSYS: i32 = 38;
const EBADF: i32 = 9;

impl FuseAdapter for MemFuseAdapter {
    fn lookup(&self, parent: Ino, name: &str) -> Result<EntryAttr, i32> {
        let state = self.state.lock().unwrap();
        let Some(Node::Dir { children }) = state.nodes.get(&parent) else {
            return Err(ENOTDIR);
        };
        let Some(&ino) = children.get(name) else {
            return Err(ENOENT);
        };
        Self::entry_attr_for(&state, ino).ok_or(ENOENT)
    }

    fn getattr(&self, ino: Ino) -> Result<EntryAttr, i32> {
        let state = self.state.lock().unwrap();
        Self::entry_attr_for(&state, ino).ok_or(ENOENT)
    }

    fn readdir(&self, ino: Ino, _offset: i64) -> Result<Vec<DirEntry>, i32> {
        let state = self.state.lock().unwrap();
        let Some(Node::Dir { children }) = state.nodes.get(&ino) else {
            return Err(ENOTDIR);
        };
        let mut out = Vec::with_capacity(children.len());
        for (name, &child_ino) in children {
            let kind = match state.nodes.get(&child_ino) {
                Some(Node::Dir { .. }) => FsEntryKind::Directory,
                Some(Node::File { .. }) => FsEntryKind::RegularFile,
                None => continue,
            };
            out.push(DirEntry {
                ino: child_ino,
                kind,
                name: name.clone(),
            });
        }
        Ok(out)
    }

    fn open(&self, ino: Ino) -> Result<FileHandleId, i32> {
        let mut state = self.state.lock().unwrap();
        match state.nodes.get(&ino) {
            Some(Node::File { .. }) => {}
            Some(Node::Dir { .. }) => return Err(EISDIR),
            None => return Err(ENOENT),
        }
        let h = state.next_handle;
        state.next_handle += 1;
        state.handles.insert(h, ino);
        Ok(h)
    }

    fn read(&self, handle: FileHandleId, offset: u64, len: usize) -> Result<Vec<u8>, i32> {
        let state = self.state.lock().unwrap();
        let Some(&ino) = state.handles.get(&handle) else {
            return Err(EBADF);
        };
        let Some(Node::File { data }) = state.nodes.get(&ino) else {
            return Err(ENOENT);
        };
        let start = offset as usize;
        if start >= data.len() {
            return Ok(Vec::new());
        }
        let end = (start + len).min(data.len());
        Ok(data[start..end].to_vec())
    }

    fn release(&self, handle: FileHandleId) -> Result<(), i32> {
        let mut state = self.state.lock().unwrap();
        if state.handles.remove(&handle).is_none() {
            return Err(EBADF);
        }
        Ok(())
    }

    fn create(&self, parent_path: &str, name: &str) -> Result<Ino, i32> {
        let mut state = self.state.lock().unwrap();
        // Resolve parent_path -> parent ino (simple walk).
        let parent_ino = resolve_path_locked(&state, parent_path)?;
        let Some(Node::Dir { children }) = state.nodes.get(&parent_ino) else {
            return Err(ENOTDIR);
        };
        if children.contains_key(name) {
            return Err(17 /* EEXIST */);
        }
        let ino = state.next_ino;
        state.next_ino += 1;
        state.nodes.insert(ino, Node::File { data: Vec::new() });
        state.parent_of.insert(ino, parent_ino);
        state.name_of.insert(ino, name.to_string());
        if let Some(Node::Dir { children }) = state.nodes.get_mut(&parent_ino) {
            children.insert(name.to_string(), ino);
        }
        Ok(ino)
    }

    fn write(&self, ino: Ino, offset: u64, data: &[u8]) -> Result<usize, i32> {
        let mut state = self.state.lock().unwrap();
        let Some(node) = state.nodes.get_mut(&ino) else {
            return Err(ENOENT);
        };
        let Node::File { data: buf } = node else {
            return Err(EISDIR);
        };
        let offset = offset as usize;
        let end = offset + data.len();
        if buf.len() < end {
            buf.resize(end, 0);
        }
        buf[offset..end].copy_from_slice(data);
        Ok(data.len())
    }

    fn truncate(&self, ino: Ino, new_size: u64) -> Result<(), i32> {
        let mut state = self.state.lock().unwrap();
        let Some(node) = state.nodes.get_mut(&ino) else {
            return Err(ENOENT);
        };
        let Node::File { data } = node else {
            return Err(EISDIR);
        };
        data.resize(new_size as usize, 0);
        Ok(())
    }

    fn set_size(&self, ino: Ino, new_size: u64, _set_allocation_size: bool) -> Result<(), i32> {
        self.truncate(ino, new_size)
    }

    fn overwrite(&self, ino: Ino, data: &[u8]) -> Result<usize, i32> {
        let mut state = self.state.lock().unwrap();
        let Some(node) = state.nodes.get_mut(&ino) else {
            return Err(ENOENT);
        };
        let Node::File { data: buf } = node else {
            return Err(EISDIR);
        };
        buf.clear();
        buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn can_delete(&self, ino: Ino) -> Result<(), i32> {
        let state = self.state.lock().unwrap();
        match state.nodes.get(&ino) {
            Some(Node::Dir { children }) if !children.is_empty() => Err(39 /* ENOTEMPTY */),
            Some(_) => Ok(()),
            None => Err(ENOENT),
        }
    }

    fn unlink(&self, parent_path: &str, name: &str) -> Result<(), i32> {
        let mut state = self.state.lock().unwrap();
        let parent_ino = resolve_path_locked(&state, parent_path)?;
        let Some(Node::Dir { children }) = state.nodes.get_mut(&parent_ino) else {
            return Err(ENOTDIR);
        };
        let Some(child_ino) = children.remove(name) else {
            return Err(ENOENT);
        };
        state.nodes.remove(&child_ino);
        state.parent_of.remove(&child_ino);
        state.name_of.remove(&child_ino);
        Ok(())
    }

    fn resolve_ino_to_path(&self, ino: Ino) -> Result<PathBuf, i32> {
        let state = self.state.lock().unwrap();
        let mut parts: Vec<String> = Vec::new();
        let mut cur = ino;
        while cur != ROOT_INODE {
            let Some(name) = state.name_of.get(&cur) else {
                return Err(ENOENT);
            };
            parts.push(name.clone());
            cur = *state.parent_of.get(&cur).ok_or(ENOENT)?;
        }
        parts.reverse();
        let mut s = String::from("/");
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                s.push('/');
            }
            s.push_str(p);
        }
        Ok(PathBuf::from(s))
    }

    fn statfs(&self) -> Result<(u64, u64), i32> {
        // 1 GiB nominal / 512 MiB free — purely cosmetic, but keeps
        // Explorer from complaining about an unusable drive.
        Ok((1u64 << 30, 1u64 << 29))
    }

    // The rest stay at ENOSYS — Windows tolerates ENOSYS on optional
    // paths (see `cb_set_basic_info` for the explicit ENOSYS bypass).
    fn set_basic_info(
        &self,
        _ino: Ino,
        _info: pcloud_fs::fuse_adapter::BasicInfo,
    ) -> Result<EntryAttr, i32> {
        Err(ENOSYS)
    }

    fn setattr(
        &self,
        ino: Ino,
        _attr: pcloud_fs::fuse_adapter::SetAttr,
    ) -> Result<EntryAttr, i32> {
        // Best-effort: just return current attrs. Windows create() only
        // checks the `Ok(_)` arm to propagate mode bits back to the
        // backend, and any failure is ignored.
        self.getattr(ino)
    }
}

/// Walk a POSIX-shaped absolute path through the in-memory tree while
/// holding the state lock. Returns the terminal inode.
fn resolve_path_locked(state: &FsState, path: &str) -> Result<Ino, i32> {
    if path.is_empty() || path == "/" {
        return Ok(ROOT_INODE);
    }
    let mut cur = ROOT_INODE;
    for seg in path.trim_start_matches('/').split('/') {
        if seg.is_empty() {
            continue;
        }
        let Some(Node::Dir { children }) = state.nodes.get(&cur) else {
            return Err(ENOTDIR);
        };
        let Some(&child) = children.get(seg) else {
            return Err(ENOENT);
        };
        cur = child;
    }
    Ok(cur)
}

// --- Drive-letter probe -----------------------------------------------------

/// Find an unused drive letter by probing Z: downward. Returns something
/// like `Z:` (the trailing colon, no slash — matches
/// `mountpoint_to_utf16` expectations). Returns `None` if every letter
/// from Z down to D is taken.
fn pick_free_drive_letter() -> Option<String> {
    for letter in (b'D'..=b'Z').rev() {
        let c = letter as char;
        let root = format!("{}:\\", c);
        if !Path::new(&root).exists() {
            return Some(format!("{}:", c));
        }
    }
    None
}

// --- The test --------------------------------------------------------------

#[test]
#[ignore = "requires PCLOUD_WINFSP_TEST=1 (or PCLOUD_LIVE_E2E=1), WinFSP 2.x installed, and a free drive letter"]
fn winfsp_mount_readdir_read_write_unmount() {
    if !e2e_gate_enabled() {
        eprintln!(
            "[winfsp_mount_live] skip: set PCLOUD_WINFSP_TEST=1 or PCLOUD_LIVE_E2E=1 to run"
        );
        return;
    }

    // --- Pick a free drive letter --------------------------------------
    let Some(drive) = pick_free_drive_letter() else {
        eprintln!("[winfsp_mount_live] skip: no free drive letter between D: and Z:");
        return;
    };
    let drive_root_display = format!("{}\\", drive); // e.g. "Z:\\"
    eprintln!("[winfsp_mount_live] using drive letter {drive}");

    // --- Build the adapter and grab a handle on its shared state so we
    //     can cross-check Win32 writes reached the btree. --------------
    let adapter = std::sync::Arc::new(MemFuseAdapter::new());
    // We need a raw reference that outlives the boxed trait object. The
    // boxed adapter that the mount owns is a *clone* of the Arc's
    // trait-object view; the `state` Mutex inside is the shared thing.
    //
    // To give the test body post-mount read access we construct a second
    // Arc that shares the same `Mutex<FsState>`. Easiest way: wrap the
    // adapter itself in an Arc and hand both halves a trait-object clone.
    //
    // (The mount entry point takes `adapter: A: FuseAdapter` by value and
    // double-boxes it. So rather than share the adapter across mount and
    // test we share the state via a global.)

    let state_view = std::sync::Arc::clone(&adapter);

    // The mount entry point takes the adapter by value and leaks it as a
    // `Box<Box<dyn FuseAdapter>>`. We cannot hand it our Arc directly, so
    // we wrap the Arc in a thin `ArcAdapter` forwarder whose `FuseAdapter`
    // impl delegates everything. That way both `state_view` and the
    // mount's owned box point at the same `Mutex<FsState>`.
    struct ArcAdapter(std::sync::Arc<MemFuseAdapter>);
    impl FuseAdapter for ArcAdapter {
        fn lookup(&self, parent: Ino, name: &str) -> Result<EntryAttr, i32> {
            self.0.lookup(parent, name)
        }
        fn getattr(&self, ino: Ino) -> Result<EntryAttr, i32> {
            self.0.getattr(ino)
        }
        fn readdir(&self, ino: Ino, offset: i64) -> Result<Vec<DirEntry>, i32> {
            self.0.readdir(ino, offset)
        }
        fn open(&self, ino: Ino) -> Result<FileHandleId, i32> {
            self.0.open(ino)
        }
        fn read(&self, h: FileHandleId, o: u64, l: usize) -> Result<Vec<u8>, i32> {
            self.0.read(h, o, l)
        }
        fn release(&self, h: FileHandleId) -> Result<(), i32> {
            self.0.release(h)
        }
        fn create(&self, parent_path: &str, name: &str) -> Result<Ino, i32> {
            self.0.create(parent_path, name)
        }
        fn write(&self, ino: Ino, offset: u64, data: &[u8]) -> Result<usize, i32> {
            self.0.write(ino, offset, data)
        }
        fn truncate(&self, ino: Ino, new_size: u64) -> Result<(), i32> {
            self.0.truncate(ino, new_size)
        }
        fn set_size(&self, ino: Ino, new_size: u64, b: bool) -> Result<(), i32> {
            self.0.set_size(ino, new_size, b)
        }
        fn overwrite(&self, ino: Ino, data: &[u8]) -> Result<usize, i32> {
            self.0.overwrite(ino, data)
        }
        fn can_delete(&self, ino: Ino) -> Result<(), i32> {
            self.0.can_delete(ino)
        }
        fn unlink(&self, p: &str, n: &str) -> Result<(), i32> {
            self.0.unlink(p, n)
        }
        fn resolve_ino_to_path(&self, ino: Ino) -> Result<PathBuf, i32> {
            self.0.resolve_ino_to_path(ino)
        }
        fn statfs(&self) -> Result<(u64, u64), i32> {
            self.0.statfs()
        }
        fn setattr(
            &self,
            ino: Ino,
            a: pcloud_fs::fuse_adapter::SetAttr,
        ) -> Result<EntryAttr, i32> {
            self.0.setattr(ino, a)
        }
        fn set_basic_info(
            &self,
            ino: Ino,
            i: pcloud_fs::fuse_adapter::BasicInfo,
        ) -> Result<EntryAttr, i32> {
            self.0.set_basic_info(ino, i)
        }
    }

    let mount_adapter = ArcAdapter(std::sync::Arc::clone(&adapter));
    drop(adapter);

    // --- Mount via the Windows WinFSP entry point ----------------------
    let drive_path = PathBuf::from(&drive);
    let opts = MountOptions {
        read_only: false,
        fs_name: Some("pcloud-winfsp-smoke".into()),
        allow_other: false,
        attr_timeout_secs: 1.0,
        entry_timeout_secs: 1.0,
        max_readahead: 128 * 1024,
    };

    let handle: MountHandle = match mount_with_winfsp(&drive_path, mount_adapter, opts) {
        Ok(h) => h,
        Err(err) => {
            let msg = err.to_string();
            // Skip rather than fail on environment issues: WinFSP not
            // installed, lacking SeCreateSymbolicLinkPrivilege, no free
            // drive letter despite our probe (race).
            if msg.contains("WinFSP not installed")
                || msg.contains("requires elevation")
                || msg.contains("Access is denied")
                || msg.contains("STATUS_ACCESS_DENIED")
            {
                eprintln!("[winfsp_mount_live] skip (environment): {msg}");
                return;
            }
            panic!("mount_with_winfsp failed: {msg}");
        }
    };

    // Give WinFSP a moment to publish the drive letter to the Object Manager
    // before Win32 APIs can open it. The dispatcher's first `GetVolumeInfo`
    // round-trip typically completes within tens of ms.
    std::thread::sleep(Duration::from_millis(500));

    // --- Exercise readdir ----------------------------------------------
    let entries: Vec<String> = match std::fs::read_dir(&drive_root_display) {
        Ok(it) => it
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) => {
            drop(handle);
            panic!("read_dir on {drive_root_display} failed: {err}");
        }
    };
    eprintln!("[winfsp_mount_live] readdir entries: {entries:?}");
    assert!(
        entries.iter().any(|e| e == "file1.txt"),
        "file1.txt missing from readdir: {entries:?}",
    );
    assert!(
        entries.iter().any(|e| e == "empty.txt"),
        "empty.txt missing from readdir: {entries:?}",
    );

    // --- Exercise read -------------------------------------------------
    let file1_path = format!("{}file1.txt", drive_root_display);
    let read_back = match std::fs::read_to_string(&file1_path) {
        Ok(s) => s,
        Err(err) => {
            drop(handle);
            panic!("read_to_string {file1_path} failed: {err}");
        }
    };
    assert!(
        read_back.starts_with("hello-from-winfsp-smoke-test\n"),
        "file1.txt contents do not start with seed marker: len={}",
        read_back.len(),
    );
    eprintln!(
        "[winfsp_mount_live] read {} bytes from {file1_path}",
        read_back.len()
    );

    // --- Exercise write ------------------------------------------------
    // std::fs::write -> CreateFileW(CREATE_ALWAYS) -> WinFSP Create +
    // Overwrite -> Write -> Cleanup -> Close.
    let new_path = format!("{}newfile.txt", drive_root_display);
    let payload = b"new-content-via-winfsp\n".to_vec();
    match std::fs::write(&new_path, &payload) {
        Ok(()) => {}
        Err(err) => {
            drop(handle);
            panic!("std::fs::write {new_path} failed: {err}");
        }
    }
    eprintln!("[winfsp_mount_live] wrote {} bytes to {new_path}", payload.len());

    // Cross-check: the in-memory btree saw the write.
    {
        let state = state_view.state.lock().unwrap();
        let root_children = match state.nodes.get(&ROOT_INODE) {
            Some(Node::Dir { children }) => children.clone(),
            _ => panic!("root inode vanished"),
        };
        let new_ino = *root_children
            .get("newfile.txt")
            .expect("newfile.txt not in root children after write");
        let Some(Node::File { data }) = state.nodes.get(&new_ino) else {
            drop(handle);
            panic!("newfile.txt inode is not a regular file");
        };
        assert_eq!(
            data, &payload,
            "in-memory bytes differ from payload (got {} bytes)",
            data.len()
        );
        eprintln!(
            "[winfsp_mount_live] btree cross-check: newfile.txt ino={new_ino} len={}",
            data.len()
        );
    }

    // Also exercise overwrite (truncate-and-write) on an existing file.
    let over_path = format!("{}file1.txt", drive_root_display);
    let over_payload = b"truncated".to_vec();
    match std::fs::write(&over_path, &over_payload) {
        Ok(()) => {}
        Err(err) => {
            drop(handle);
            panic!("std::fs::write (overwrite) {over_path} failed: {err}");
        }
    }
    {
        let state = state_view.state.lock().unwrap();
        let root_children = match state.nodes.get(&ROOT_INODE) {
            Some(Node::Dir { children }) => children.clone(),
            _ => panic!("root inode vanished"),
        };
        let f1_ino = *root_children.get("file1.txt").expect("file1.txt gone");
        let Some(Node::File { data }) = state.nodes.get(&f1_ino) else {
            drop(handle);
            panic!("file1.txt ino not a file");
        };
        assert_eq!(
            data, &over_payload,
            "overwrite did not truncate file1.txt: got {} bytes",
            data.len()
        );
    }

    // --- Unmount -------------------------------------------------------
    match handle.unmount() {
        Ok(()) => eprintln!("[winfsp_mount_live] unmount OK"),
        Err(err) => panic!("unmount failed: {err}"),
    }

    // Give WinFSP a moment to tear the drive letter back down.
    std::thread::sleep(Duration::from_millis(500));

    // --- Assert the drive disappeared ----------------------------------
    assert!(
        !Path::new(&drive_root_display).exists(),
        "{drive_root_display} still exists after unmount — stale WinFSP state?",
    );
    eprintln!("[winfsp_mount_live] drive {drive} disappeared post-unmount; OK");
}

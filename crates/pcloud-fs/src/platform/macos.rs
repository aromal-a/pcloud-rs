//! **PLATFORM: macOS only.**
//! **GATING: `#[cfg(target_os = "macos")]`** -- the entire module file is
//! gated at the `mod macos;` line in `platform/mod.rs`.
//!
//! **NOT YET TESTED ON MACOS** — bring-up requires a real Mac with
//! fuse-t installed. Ships pending PHASE-4 live verification.
//!
//! Implementation strategy: **fuse-t** (<https://www.fuse-t.org/>) via
//! direct FFI to its shipped `libfuse.dylib`, which is ABI-compatible
//! with libfuse 2.9's low-level API. We hand-roll the binding in
//! [`macos_ffi`] instead of depending on an external crate so the
//! surface stays small, auditable, and free of transitive macOS-only
//! deps that would churn the workspace.
//!
//! **BRING-UP STATUS:** Phase 5: full read+write surface wired;
//! pending real-Mac bring-up for bd-1du.4.6. The probe path, option
//! defaults, session loop, and read+write op thunks are populated;
//! actual boot on a Mac (dylib ABI confirmation, argv option audit,
//! integration tests) is still tracked under bd-1du.4.6. The Linux
//! workspace must remain green regardless (enforced by cfg-gates).
//!
//! **WRITE SURFACE (U3):** the `create`, `unlink`, `mkdir`, `rmdir`,
//! and `rename` thunks now resolve parent inodes to remote paths via
//! `FuseAdapter::resolve_ino_to_path` and pass real paths through to
//! the adapter's write-side methods. Live-host bring-up is still
//! required to exercise the kernel-VFS integration for these calls on
//! an actual fuse-t mount (tracked under bd-1du.4.6).
//!
//! Also implemented here:
//! - `MacosMountinfoReader` wraps `getmntinfo(3)` and produces a
//!   `/proc/self/mountinfo`-shaped payload for orphan detection.

use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::fuse_adapter::{DirEntry, EntryAttr, FsEntryKind, FuseAdapter};
use crate::mount_orphan::MountinfoReader;
use crate::mount_service::{MountError, MountHandle, MountOptions};
use crate::platform::PlatformMount;

pub mod macos_ffi;

// -----------------------------------------------------------------------------
// PlatformMount (macOS): fuse-t low-level API via direct FFI.
// -----------------------------------------------------------------------------

/// macOS platform-mount implementation backed by fuse-t.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosPlatformMount;

impl PlatformMount for MacosPlatformMount {
    /// Basic validation: path must exist and be a directory. Additional
    /// checks (emptiness, ownership, world-writable) are handled by
    /// [`crate::mount_service::MountService::validate_mountpoint`] when
    /// the caller routes through `MountService`. Here we apply the
    /// minimum viable macOS-local checks so the FFI never sees an
    /// obviously broken path.
    fn validate_mountpoint(&self, mountpoint: &Path) -> Result<(), MountError> {
        let meta = match std::fs::metadata(mountpoint) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(MountError::MountpointMissing(mountpoint.to_path_buf()));
            }
            Err(e) => return Err(MountError::Io(e)),
        };
        if !meta.is_dir() {
            return Err(MountError::MountpointNotDirectory(mountpoint.to_path_buf()));
        }
        Ok(())
    }

    /// Probe fuse-t availability. We succeed when either:
    /// - the fuse-t framework bundle exists at its canonical install path, or
    /// - `dlopen("libfuse.dylib", RTLD_LAZY)` resolves (user-space
    ///   compatibility shim is discoverable via `DYLD_LIBRARY_PATH`).
    ///
    /// Returns [`MountError::Unsupported`] with an install hint when
    /// neither check passes.
    fn probe_supported(&self) -> Result<(), MountError> {
        if Path::new("/Library/Frameworks/fuse-t.framework").exists() {
            return Ok(());
        }
        if dlopen_libfuse_succeeds() {
            return Ok(());
        }
        Err(MountError::Unsupported(
            "fuse-t not installed; install from https://www.fuse-t.org/".to_string(),
        ))
    }

    /// macOS-flavored defaults: we advertise a pCloud volume name,
    /// defer permission checks to the daemon (fuse-t enforces by
    /// default in-kernel which the pCloud adapter cannot honor), and
    /// request `allow_other` for parity with the Linux path where the
    /// daemon runs as the invoking user.
    fn default_options(&self) -> MountOptions {
        let mut opts = MountOptions::default();
        // `allow_other` is vetoed by the Rust `MountService` at the
        // cross-platform layer; we still surface the intent here so
        // callers that bypass `MountService` (integration tests, raw
        // CLI) see the platform-preferred value. The macOS-specific
        // `volname` / `defer_permissions` flags are not first-class
        // fields on `MountOptions`; they will be emitted by the
        // mount-arg builder in `mount_adapter` once the session loop
        // lands.
        opts.allow_other = true;
        if opts.fs_name.is_none() {
            opts.fs_name = Some("pCloud".to_string());
        }
        opts
    }

    /// Mount a boxed [`FuseAdapter`] at `mount_point` using fuse-t.
    ///
    /// **Current bring-up gate:** this implementation validates the
    /// mountpoint, probes fuse-t, and stages the adapter for the FFI
    /// thunks declared in [`macos_ffi`]. The actual session loop
    /// (`fuse_session_loop` on a background thread, RAII-style
    /// unmount) is deferred to bd-1du.4 real-Mac bring-up. Until then
    /// we return [`MountError::Unsupported`] with a clear marker so
    /// callers do not silently believe they have a live mount.
    fn mount_adapter(
        &self,
        adapter: Box<dyn FuseAdapter>,
        mount_point: &Path,
        opts: MountOptions,
    ) -> Result<MountHandle, MountError> {
        self.validate_mountpoint(mount_point)?;
        self.probe_supported()?;
        mount_with_fuse_t(adapter, mount_point, opts)
    }
}

// -----------------------------------------------------------------------------
// fuse-t session bring-up.
//
// **NOT YET TESTED ON MACOS** — ships pending PHASE-4 live verification.
// -----------------------------------------------------------------------------

/// Mount `adapter` at `mount_point` against fuse-t's `libfuse.dylib`.
///
/// Flow:
///   1. build argv (owned `CString`s + pointer vector kept alive),
///   2. `fuse_mount(mountpoint, &args) -> *mut fuse_chan`,
///   3. `fuse_lowlevel_new(&args, &ops, size_of::<LowlevelOps>(), user_data) -> *mut fuse_session`,
///   4. `fuse_session_add_chan(session, chan)`,
///   5. spawn a thread that runs `fuse_session_loop(session)`,
///   6. return a populated `MountHandle` (RAII teardown lives in
///      `mount_service::MountHandle::teardown_macos`).
///
/// On any failure after `fuse_mount` we call `fuse_unmount` to release
/// the kernel mount and then bubble up a `MountError::Unsupported` with
/// a precise reason. The adapter `user_data` is moved into the
/// `MountHandle` on success so its address stays stable for the
/// lifetime of the session.
fn mount_with_fuse_t(
    adapter: Box<dyn FuseAdapter>,
    mount_point: &Path,
    opts: MountOptions,
) -> Result<MountHandle, MountError> {
    let mount_point_c = path_to_cstring(mount_point)?;
    let argv_owned = build_fuse_args(&opts);

    // Build argv: a vector of raw `*mut c_char` pointers into the
    // owned `CString`s. The `CString`s must outlive the FFI call
    // sequence, so we keep them alive in `argv_owned`.
    let mut argv_ptrs: Vec<*mut std::os::raw::c_char> = argv_owned
        .iter()
        .map(|cs| cs.as_ptr() as *mut std::os::raw::c_char)
        .collect();
    // libfuse convention: argv is NOT NUL-terminated; argc drives
    // iteration. We still reserve one slot defensively in case the
    // library scans past argc on a malformed input.
    argv_ptrs.push(std::ptr::null_mut());

    let mut args = macos_ffi::fuse_args {
        argc: argv_owned.len() as std::os::raw::c_int,
        argv: argv_ptrs.as_mut_ptr(),
        allocated: 0,
    };

    // Heap-stable user-data. The address we hand to fuse-t must stay
    // valid until `fuse_session_destroy` has returned and all thunks
    // have stopped firing. We move ownership into the `MountHandle`
    // on success; on failure we drop it here.
    let mut user_data: Box<Box<dyn FuseAdapter>> = Box::new(adapter);
    let user_data_ptr = (&mut *user_data) as *mut Box<dyn FuseAdapter> as *mut std::ffi::c_void;

    // SAFETY: `mount_point_c` is NUL-terminated and alive for this
    // call; `args` is populated with live argv pointers rooted in
    // `argv_owned`/`argv_ptrs` which live across this call. On
    // success fuse-t returns a non-null `*mut fuse_chan` whose
    // lifetime we now own.
    let chan = unsafe { macos_ffi::fuse_mount(mount_point_c.as_ptr(), &mut args) };
    if chan.is_null() {
        return Err(MountError::Unsupported(
            "fuse_mount failed (fuse-t kernel extension not loaded, or mountpoint rejected)"
                .to_string(),
        ));
    }

    let ops = build_lowlevel_ops();

    // SAFETY: `args` is still alive; `ops` is a `LowlevelOps` whose
    // layout mirrors `struct fuse_lowlevel_ops` up to the fields we
    // populate — all other slots are `None`, so libfuse must return
    // `ENOSYS` for them (we pass `size_of::<LowlevelOps>()` so
    // libfuse does not read past our buffer). `user_data_ptr` is a
    // heap-stable address whose target lives for the session.
    let session = unsafe {
        macos_ffi::fuse_lowlevel_new(
            &mut args,
            (&ops as *const macos_ffi::LowlevelOps) as *const std::ffi::c_void,
            std::mem::size_of::<macos_ffi::LowlevelOps>(),
            user_data_ptr,
        )
    };
    if session.is_null() {
        // SAFETY: `chan` is live; unmount releases the kernel-side
        // mount we just established.
        unsafe { macos_ffi::fuse_unmount(mount_point_c.as_ptr(), chan) };
        return Err(MountError::Unsupported(
            "fuse_lowlevel_new returned NULL (fuse-t ABI mismatch?)".to_string(),
        ));
    }

    // SAFETY: both `session` and `chan` are live owned handles.
    unsafe { macos_ffi::fuse_session_add_chan(session, chan) };

    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Wrap `session` in a Send newtype so the loop thread can hold
    // it across the FFI boundary.
    struct SessionPtr(*mut macos_ffi::fuse_session);
    // SAFETY: `fuse_session` is an opaque handle; we transfer unique
    // ownership to the loop thread which only invokes
    // `fuse_session_loop` on it. Teardown on the control thread
    // uses `fuse_session_exit` (documented safe across threads).
    unsafe impl Send for SessionPtr {}
    let session_ptr = SessionPtr(session);

    let loop_thread = std::thread::Builder::new()
        .name("pcloud-fuse-t-loop".to_string())
        .spawn(move || {
            let sp = session_ptr;
            // SAFETY: `sp.0` is a live `fuse_session` handle owned by
            // this thread until `fuse_session_loop` returns.
            // `catch_unwind` would be redundant here: `fuse_session_loop`
            // does not run user Rust panics itself — those happen in
            // the thunks, which guard their own panic boundaries.
            let _rc = unsafe { macos_ffi::fuse_session_loop(sp.0) };
        })
        .map_err(|e| {
            // SAFETY: session/chan are live; we must tear them down
            // before propagating. `user_data` is dropped at function
            // exit because we never transferred ownership.
            unsafe {
                macos_ffi::fuse_session_destroy(session);
                macos_ffi::fuse_unmount(mount_point_c.as_ptr(), chan);
            }
            MountError::Io(e)
        })?;

    Ok(MountHandle::from_macos(
        session,
        chan,
        mount_point_c,
        loop_thread,
        shutdown,
        user_data,
    ))
}

// -----------------------------------------------------------------------------
// Low-level op thunks.
//
// Each thunk:
//   * recovers `&dyn FuseAdapter` from the `user_data` pointer that
//     was installed via `fuse_lowlevel_new`,
//   * catches any Rust panic with `std::panic::catch_unwind` because
//     a panic across an FFI boundary is undefined behavior,
//   * on panic, replies `EIO` so the kernel sees a clean error
//     instead of a wedged request.
//
// Only `init`, `destroy`, `lookup`, `getattr` are wired for Phase 3.
// Other op slots remain `None` in `LowlevelOps`, which makes libfuse
// reply `ENOSYS` automatically.
// -----------------------------------------------------------------------------

/// Recover `&dyn FuseAdapter` from the `user_data` pointer.
///
/// # Safety
/// `ud` must be the pointer installed at `fuse_lowlevel_new` time,
/// which points to a `Box<dyn FuseAdapter>` living inside a
/// `Box<Box<dyn FuseAdapter>>` owned by the `MountHandle`. Lifetime
/// of the returned reference is bounded by the caller thunk.
#[allow(clippy::borrowed_box, dead_code)]
unsafe fn adapter_from_userdata<'a>(ud: *mut std::ffi::c_void) -> Option<&'a dyn FuseAdapter> {
    if ud.is_null() {
        return None;
    }
    // SAFETY: `ud` points to a `Box<dyn FuseAdapter>` whose address
    // was taken from a `Box<Box<dyn FuseAdapter>>` that lives for
    // the entire session (see `MacosMountInner::user_data`).
    let bx = unsafe { &*(ud as *const Box<dyn FuseAdapter>) };
    Some(&**bx)
}

/// Recover the adapter from a live `fuse_req_t`. Returns `None` if
/// libfuse somehow hands us a NULL userdata — in which case the
/// caller replies `EIO`.
///
/// # Safety
/// `req` must be live for the duration of the call; libfuse owns it
/// until a `fuse_reply_*` returns. The returned adapter reference is
/// rooted in the `Box<Box<dyn FuseAdapter>>` that outlives the session.
unsafe fn adapter_from_req<'a>(req: macos_ffi::fuse_req_t) -> Option<&'a dyn FuseAdapter> {
    if req.is_null() {
        return None;
    }
    // SAFETY: `req` is a live request handle owned by libfuse for this
    // callback's duration; `fuse_req_userdata` returns the pointer we
    // installed in `fuse_lowlevel_new`, which is a
    // `Box<Box<dyn FuseAdapter>>`-rooted address.
    let ud = unsafe { macos_ffi::fuse_req_userdata(req) };
    // SAFETY: forwarded to `adapter_from_userdata`; contract matches.
    unsafe { adapter_from_userdata(ud) }
}

/// Attribute timeout used for `fuse_reply_entry` / `fuse_reply_attr`.
/// Conservative default: 1 second. Real tuning lives alongside the
/// metadata-cache TTL policy (bd-1du.4.e).
const ATTR_TIMEOUT_SECS: f64 = 1.0;
/// Directory-entry timeout for negative/positive dentries.
const ENTRY_TIMEOUT_SECS: f64 = 1.0;

/// Populate a `libc::stat` from an [`EntryAttr`]. We fill only the
/// fields the kernel relies on for ls/stat semantics; the rest stays
/// zeroed (which is the libfuse convention for unused slots).
fn entry_attr_to_stat(attr: &EntryAttr) -> libc::stat {
    // SAFETY: `libc::stat` is `#[repr(C)]` with no invalid bit patterns
    // for the fields we leave at zero; `zeroed()` is the libfuse idiom.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    st.st_ino = attr.ino;
    st.st_size = attr.size as i64;
    st.st_uid = attr.uid;
    st.st_gid = attr.gid;
    let mode_type: u16 = match attr.kind {
        FsEntryKind::Directory => libc::S_IFDIR as u16,
        FsEntryKind::RegularFile => libc::S_IFREG as u16,
        FsEntryKind::Symlink => libc::S_IFLNK as u16,
    };
    st.st_mode = mode_type | (attr.mode & 0o7777);
    st.st_nlink = if matches!(attr.kind, FsEntryKind::Directory) {
        2
    } else {
        1
    };
    if let Some(mtime) = attr.mtime_epoch {
        st.st_mtime = mtime as i64;
        st.st_ctime = mtime as i64;
        st.st_atime = mtime as i64;
    }
    st
}

fn entry_attr_to_param(attr: &EntryAttr) -> macos_ffi::fuse_entry_param {
    macos_ffi::fuse_entry_param {
        ino: attr.ino,
        generation: 0,
        attr: entry_attr_to_stat(attr),
        attr_timeout: ATTR_TIMEOUT_SECS,
        entry_timeout: ENTRY_TIMEOUT_SECS,
    }
}

/// `init` thunk. Called once after `fuse_session_new` completes.
extern "C" fn thunk_init(_userdata: *mut std::ffi::c_void, _conn: *mut std::ffi::c_void) {
    let _ = std::panic::catch_unwind(|| {
        // Nothing to do: the adapter has no explicit `init` hook yet.
    });
}

/// `destroy` thunk. Called once at session teardown.
extern "C" fn thunk_destroy(_userdata: *mut std::ffi::c_void) {
    let _ = std::panic::catch_unwind(|| {
        // Adapter drop happens on the control thread in
        // `teardown_macos`; we do not free anything here.
    });
}

/// `lookup` thunk. Resolves `name` within `parent` and replies with a
/// `fuse_entry_param`. Maps `Err(ENOENT)` to a negative reply and any
/// other error to `EIO`. A name containing an interior NUL is rejected
/// as `EINVAL` before calling into the adapter.
///
/// # Safety
/// `req` is a live request handle owned by libfuse for this call;
/// `name` is a NUL-terminated C string valid until we reply.
extern "C" fn thunk_lookup(
    req: macos_ffi::fuse_req_t,
    parent: macos_ffi::fuse_ino_t,
    name: *const std::os::raw::c_char,
) {
    let _ = std::panic::catch_unwind(|| {
        if name.is_null() {
            // SAFETY: `req` is valid for this call.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: `name` is a NUL-terminated C string owned by libfuse
        // for the duration of this callback; we copy into an owned
        // Rust string before replying so no pointer escapes.
        let name_str = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
                return;
            }
        };
        // SAFETY: `req` is live for this call; userdata was installed
        // at mount time and outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        match adapter.lookup(parent, &name_str) {
            Ok(attr) => {
                let param = entry_attr_to_param(&attr);
                // SAFETY: `req` is valid; `&param` lives for this call
                // and libfuse copies out of it synchronously.
                unsafe { macos_ffi::fuse_reply_entry(req, &param) };
            }
            Err(errno) => {
                // SAFETY: `req` is valid; `errno` is a Rust-side i32
                // forwarded verbatim (already a libc errno).
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `getattr` thunk. Synchronous attribute fetch; replies with
/// `fuse_reply_attr` on success or a libc errno on failure.
///
/// # Safety
/// `req` is live for this call; `fi` is either NULL or valid for this
/// call and we do not dereference it (the low-level getattr contract
/// lets us ignore `fi` when we keep no per-handle attr state).
extern "C" fn thunk_getattr(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    _fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        match adapter.getattr(ino) {
            Ok(attr) => {
                let st = entry_attr_to_stat(&attr);
                // SAFETY: `req` is valid; `&st` lives for this call
                // and libfuse copies out of it synchronously.
                unsafe { macos_ffi::fuse_reply_attr(req, &st, ATTR_TIMEOUT_SECS) };
            }
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `open` thunk. Opens `ino` via the adapter and stashes the returned
/// [`FileHandleId`] in `fi.fh` so subsequent `read`/`release` can
/// recover it without another adapter round-trip.
///
/// # Safety
/// `req` is live; `fi` is non-NULL and writable for this call per the
/// libfuse low-level contract.
extern "C" fn thunk_open(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if fi.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
            return;
        }
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        match adapter.open(ino) {
            Ok(handle_id) => {
                // SAFETY: `fi` is writable for this callback per the
                // libfuse contract; we only store the handle id.
                unsafe { (*fi).fh = handle_id };
                // SAFETY: `req` is valid; `fi` is valid and its
                // contents are read synchronously by libfuse.
                unsafe { macos_ffi::fuse_reply_open(req, fi) };
            }
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `read` thunk. Reads up to `size` bytes starting at `off` from the
/// file handle previously stashed in `fi.fh`.
///
/// # Safety
/// `req` is live; `fi` is non-NULL and valid for this call.
extern "C" fn thunk_read(
    req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    size: usize,
    off: i64,
    fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if fi.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
            return;
        }
        // SAFETY: `fi` is non-null and readable for this callback.
        let handle_id = unsafe { (*fi).fh };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let offset = if off < 0 { 0u64 } else { off as u64 };
        match adapter.read(handle_id, offset, size) {
            Ok(bytes) => {
                // SAFETY: `req` is valid; `bytes.as_ptr()` and
                // `bytes.len()` describe a live slice that libfuse
                // copies synchronously before returning.
                unsafe { macos_ffi::fuse_reply_buf(req, bytes.as_ptr(), bytes.len()) };
            }
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `readdir` thunk. Materializes directory entries into a libfuse
/// buffer using `fuse_add_direntry` and replies via `fuse_reply_buf`.
///
/// The kernel passes `size` as the maximum bytes it's willing to
/// accept; we must stop packing once adding the next entry would
/// overflow that budget and hand back exactly the used prefix.
///
/// # Safety
/// `req` is live; `fi` is either NULL or valid for this call.
extern "C" fn thunk_readdir(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    size: usize,
    off: i64,
    _fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let entries: Vec<DirEntry> = match adapter.readdir(ino, off) {
            Ok(v) => v,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };

        // Allocate the reply buffer up-front. Cap at the kernel-
        // requested `size`; a fresh offset starts at `off + 1` for
        // each subsequent entry per the low-level readdir contract.
        let mut buf = vec![0u8; size];
        let mut used: usize = 0;
        let mut next_off: i64 = off;
        for entry in entries.into_iter() {
            next_off = next_off.saturating_add(1);
            // Build stat for the entry; fuse_add_direntry only reads
            // `st_ino` and `st_mode` from this struct.
            let stub_attr = EntryAttr {
                ino: entry.ino,
                kind: entry.kind,
                size: 0,
                mode: 0o755,
                uid: 0,
                gid: 0,
                mtime_epoch: None,
            };
            let st = entry_attr_to_stat(&stub_attr);
            let name_c = match std::ffi::CString::new(entry.name.as_bytes()) {
                Ok(c) => c,
                Err(_) => continue, // skip entries with interior NULs
            };
            let remaining = size.saturating_sub(used);
            // SAFETY: `req` is valid; `buf` has at least `remaining`
            // bytes available at offset `used`; `name_c` is a
            // NUL-terminated C string valid for this call; `&st` is a
            // live stat read synchronously. `fuse_add_direntry`
            // returns the total bytes the entry *would* consume; if
            // that exceeds `remaining`, the entry is not committed.
            let needed = unsafe {
                macos_ffi::fuse_add_direntry(
                    req,
                    buf.as_mut_ptr().add(used) as *mut std::os::raw::c_char,
                    remaining,
                    name_c.as_ptr(),
                    &st,
                    next_off,
                )
            };
            if needed > remaining {
                // Out of space; flush whatever is already packed.
                break;
            }
            used += needed;
        }
        // SAFETY: `req` is valid; `buf.as_ptr()` is live for `used`
        // bytes and libfuse copies synchronously.
        unsafe { macos_ffi::fuse_reply_buf(req, buf.as_ptr(), used) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `release` thunk. Closes the handle stashed in `fi.fh`.
///
/// # Safety
/// `req` is live; `fi` is non-NULL and valid for this call.
extern "C" fn thunk_release(
    req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if fi.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
            return;
        }
        // SAFETY: `fi` is non-null and readable for this callback.
        let handle_id = unsafe { (*fi).fh };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let rc = adapter.release(handle_id);
        // Release always replies with `fuse_reply_err(req, 0)` on
        // success per the libfuse low-level contract.
        let err = rc.err().unwrap_or(0);
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, err) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// libfuse `FUSE_SET_ATTR_SIZE` bit (from `<fuse_lowlevel.h>`). When
/// this bit is set on the `to_set` mask in `setattr`, the caller is
/// requesting a size-change (truncate). All other bits are currently
/// accepted as no-ops so the reply path can still return the refreshed
/// attributes — refining chmod/chown/utimens lands with bd-1du.4.6.
const FUSE_SET_ATTR_SIZE: std::os::raw::c_int = 1 << 3;

/// `write` thunk. Stages `size` bytes from `buf` starting at `off` into
/// the adapter's write path and replies with the number of bytes
/// accepted via `fuse_reply_write`.
///
/// # Safety
/// `req` is live; `buf` is readable for `size` bytes for this call;
/// `fi` may be NULL (ignored — the adapter keyed by ino handles its
/// own staging slot).
extern "C" fn thunk_write(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    buf: *const std::os::raw::c_char,
    size: usize,
    off: i64,
    _fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if buf.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: libfuse guarantees `buf` points to `size` readable
        // bytes for the duration of this callback. We copy into an
        // owned slice view bound to this call; no pointer escapes.
        let data = unsafe { std::slice::from_raw_parts(buf as *const u8, size) };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let offset = if off < 0 { 0u64 } else { off as u64 };
        match adapter.write(ino, offset, data) {
            Ok(count) => {
                // SAFETY: `req` is valid; `count` is the byte total
                // libfuse will pass back to the kernel.
                unsafe { macos_ffi::fuse_reply_write(req, count) };
            }
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `create` thunk. Allocates a new regular file `name` under `parent`
/// and replies with both an entry param and the file-info (so the
/// kernel can skip a follow-up `open`).
///
/// Resolves `parent` to its remote path via
/// [`FuseAdapter::resolve_ino_to_path`] before delegating to
/// `adapter.create(parent_path, name)`. Adapters with no inode table
/// (the default trait impl) return `ENOSYS`, which surfaces as a
/// clean kernel error.
///
/// # Safety
/// `req` is live; `name` is a NUL-terminated C string owned by libfuse
/// for this callback; `fi` is non-NULL and writable for this call.
/// The `fi` pointer is dereferenced as `*mut fuse_file_info` — libfuse
/// guarantees the pointee is initialised and exclusively owned by this
/// callback until `fuse_reply_create` returns.
extern "C" fn thunk_create(
    req: macos_ffi::fuse_req_t,
    parent: macos_ffi::fuse_ino_t,
    name: *const std::os::raw::c_char,
    _mode: u32,
    fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if name.is_null() || fi.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: `name` is NUL-terminated and valid for this callback.
        let name_str = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
                return;
            }
        };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        // U3: resolve parent ino -> absolute remote path via the trait.
        let parent_buf = match adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let parent_path = parent_buf.to_string_lossy();
        let new_ino = match adapter.create(&parent_path, &name_str) {
            Ok(ino) => ino,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        // Refresh attrs for the entry reply. On failure we still
        // succeeded at creation, so surface EIO rather than leaking a
        // half-created state.
        let attr = match adapter.getattr(new_ino) {
            Ok(a) => a,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let param = entry_attr_to_param(&attr);
        // SAFETY: `fi` is writable for this call; we stash the new
        // ino so a subsequent read/write can recover it without an
        // additional open round-trip. Adapters that require an
        // explicit `open` to allocate a handle id will still see one
        // on the next callback.
        unsafe { (*fi).fh = new_ino };
        // SAFETY: `req` is valid; `&param` and `fi` are live for this
        // call and libfuse copies them synchronously.
        unsafe { macos_ffi::fuse_reply_create(req, &param, fi) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `unlink` thunk. Removes a file `name` under `parent`.
///
/// Resolves `parent` to its remote path via
/// [`FuseAdapter::resolve_ino_to_path`] and delegates to
/// `adapter.unlink(parent_path, name)`.
///
/// # Safety
/// `req` is live; `name` is a NUL-terminated C string valid for this call.
extern "C" fn thunk_unlink(
    req: macos_ffi::fuse_req_t,
    parent: macos_ffi::fuse_ino_t,
    name: *const std::os::raw::c_char,
) {
    let _ = std::panic::catch_unwind(|| {
        if name.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: `name` is NUL-terminated and valid for this callback.
        let name_str = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
                return;
            }
        };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        // U3: resolve parent ino -> absolute remote path via the trait.
        let parent_buf = match adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let parent_path = parent_buf.to_string_lossy();
        let err = match adapter.unlink(&parent_path, &name_str) {
            Ok(()) => 0,
            Err(e) => e,
        };
        // SAFETY: `req` is valid; libfuse contract replies via
        // `fuse_reply_err(req, 0)` on success.
        unsafe { macos_ffi::fuse_reply_err(req, err) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `mkdir` thunk. Creates a directory `name` under `parent` and replies
/// with the new entry param.
///
/// Resolves `parent` to its remote path via
/// [`FuseAdapter::resolve_ino_to_path`] and delegates to
/// `adapter.mkdir(parent_path, name)`.
///
/// # Safety
/// `req` is live; `name` is a NUL-terminated C string valid for this call.
extern "C" fn thunk_mkdir(
    req: macos_ffi::fuse_req_t,
    parent: macos_ffi::fuse_ino_t,
    name: *const std::os::raw::c_char,
    _mode: u32,
) {
    let _ = std::panic::catch_unwind(|| {
        if name.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: `name` is NUL-terminated and valid for this callback.
        let name_str = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
                return;
            }
        };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        // U3: resolve parent ino -> absolute remote path via the trait.
        let parent_buf = match adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let parent_path = parent_buf.to_string_lossy();
        match adapter.mkdir(&parent_path, &name_str) {
            Ok(attr) => {
                let param = entry_attr_to_param(&attr);
                // SAFETY: `req` is valid; `&param` lives for this call.
                unsafe { macos_ffi::fuse_reply_entry(req, &param) };
            }
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `rmdir` thunk. Removes an empty directory `name` under `parent`.
///
/// Resolves `parent` to its remote path via
/// [`FuseAdapter::resolve_ino_to_path`], joins the child `name`, and
/// delegates to `adapter.rmdir(full_path)`.
///
/// # Safety
/// `req` is live; `name` is a NUL-terminated C string valid for this call.
extern "C" fn thunk_rmdir(
    req: macos_ffi::fuse_req_t,
    parent: macos_ffi::fuse_ino_t,
    name: *const std::os::raw::c_char,
) {
    let _ = std::panic::catch_unwind(|| {
        if name.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: `name` is NUL-terminated and valid for this callback.
        let name_str = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
                return;
            }
        };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        // U3: resolve parent ino -> absolute remote path via the trait.
        let parent_buf = match adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let parent_str = parent_buf.to_string_lossy();
        let full_path = if parent_str == "/" {
            format!("/{name_str}")
        } else {
            format!("{parent_str}/{name_str}")
        };
        let err = match adapter.rmdir(&full_path) {
            Ok(()) => 0,
            Err(e) => e,
        };
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, err) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `rename` thunk. Moves an entry across parents/names.
///
/// Resolves both `parent` and `newparent` via
/// [`FuseAdapter::resolve_ino_to_path`], joins the child names, and
/// delegates to `adapter.rename(from_path, to_path)`.
///
/// # Safety
/// `req` is live; `name`/`newname` are NUL-terminated C strings valid
/// for this call.
extern "C" fn thunk_rename(
    req: macos_ffi::fuse_req_t,
    parent: macos_ffi::fuse_ino_t,
    name: *const std::os::raw::c_char,
    newparent: macos_ffi::fuse_ino_t,
    newname: *const std::os::raw::c_char,
) {
    let _ = std::panic::catch_unwind(|| {
        if name.is_null() || newname.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: both pointers are NUL-terminated and valid for this callback.
        let from_name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
                return;
            }
        };
        // SAFETY: see above.
        let to_name = match unsafe { std::ffi::CStr::from_ptr(newname) }.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
                return;
            }
        };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        // U3: resolve both parent inos -> absolute remote paths.
        let from_parent_buf = match adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let to_parent_buf = match adapter.resolve_ino_to_path(newparent) {
            Ok(p) => p,
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let from_parent = from_parent_buf.to_string_lossy();
        let to_parent = to_parent_buf.to_string_lossy();
        let from_path = if from_parent == "/" {
            format!("/{from_name}")
        } else {
            format!("{from_parent}/{from_name}")
        };
        let to_path = if to_parent == "/" {
            format!("/{to_name}")
        } else {
            format!("{to_parent}/{to_name}")
        };
        let err = match adapter.rename(&from_path, &to_path) {
            Ok(()) => 0,
            Err(e) => e,
        };
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, err) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `flush` thunk. Triggers a best-effort writeback for `ino`.
///
/// # Safety
/// `req` is live; `fi` may be NULL (we do not dereference it).
extern "C" fn thunk_flush(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    _fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let err = match adapter.flush_write(ino) {
            Ok(()) => 0,
            Err(e) => e,
        };
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, err) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `fsync` thunk. Enforces a durability barrier for `ino`. The
/// `datasync` flag is forwarded to the adapter as context but the
/// current trait surface does not distinguish data-only from full
/// fsync — both call `fsync_write(ino)`. This matches the libfuse
/// default-mode guidance (fsync-is-fsync) and will tighten when the
/// adapter grows a dedicated datasync hook.
///
/// # Safety
/// `req` is live; `fi` may be NULL (we do not dereference it).
extern "C" fn thunk_fsync(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    _datasync: std::os::raw::c_int,
    _fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let err = match adapter.fsync_write(ino) {
            Ok(()) => 0,
            Err(e) => e,
        };
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, err) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `setattr` thunk. The only mutation currently honored is
/// `FUSE_SET_ATTR_SIZE` (truncate), delegated to
/// [`FuseAdapter::truncate`]. All other attr mutations (chmod, chown,
/// utimens) are accepted as no-ops so the caller can still receive a
/// fresh attribute reply; full coverage lands with bd-1du.4.6.
///
/// # Safety
/// `req` is live; `attr` is a readable `libc::stat` for this call;
/// `fi` is either NULL or valid for this call (not dereferenced).
extern "C" fn thunk_setattr(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    attr: *mut libc::stat,
    to_set: std::os::raw::c_int,
    _fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if attr.is_null() {
            // SAFETY: `req` is valid.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EINVAL) };
            return;
        }
        // SAFETY: `attr` is readable for this callback per the libfuse
        // setattr contract; we copy out the single field we need.
        let requested_size = unsafe { (*attr).st_size };
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(a) => a,
            None => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        if to_set & FUSE_SET_ATTR_SIZE != 0 {
            let new_size = if requested_size < 0 {
                0u64
            } else {
                requested_size as u64
            };
            if let Err(errno) = adapter.truncate(ino, new_size) {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        }
        // Reply with a refreshed attribute snapshot so the kernel's
        // attr cache stays consistent with whatever the adapter now
        // considers the truth.
        match adapter.getattr(ino) {
            Ok(refreshed) => {
                let st = entry_attr_to_stat(&refreshed);
                // SAFETY: `req` is valid; `&st` lives for this call.
                unsafe { macos_ffi::fuse_reply_attr(req, &st, ATTR_TIMEOUT_SECS) };
            }
            Err(errno) => {
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `statfs` thunk. The [`FuseAdapter`] trait does not currently expose
/// a statfs hook, so we reply with a zero-initialized `statvfs` that
/// advertises the volume as present but size-unknown. Real capacity
/// reporting (pCloud quota) is a follow-up once the adapter grows a
/// `statfs` method (tracked alongside bd-1du.4 write path).
///
/// # Safety
/// `req` is live for this call.
extern "C" fn thunk_statfs(req: macos_ffi::fuse_req_t, _ino: macos_ffi::fuse_ino_t) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `libc::statvfs` is `#[repr(C)]` with all-zero-valid
        // fields; we populate only `f_namemax` so Finder shows a sane
        // filename limit.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        st.f_namemax = 255;
        st.f_bsize = 4096;
        st.f_frsize = 4096;
        // SAFETY: `req` is valid; `&st` lives for this call.
        unsafe { macos_ffi::fuse_reply_statfs(req, &st) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

// -----------------------------------------------------------------------------
// Probe helpers.
// -----------------------------------------------------------------------------

/// Attempt `dlopen("libfuse.dylib", RTLD_LAZY)`; return whether the
/// symbol resolves. We `dlclose` immediately because we only care
/// about presence, not a live handle.
fn dlopen_libfuse_succeeds() -> bool {
    let Ok(name) = CString::new("libfuse.dylib") else {
        return false;
    };
    // SAFETY: `dlopen` accepts a NUL-terminated C string and an integer
    // flag; it is documented thread-safe. We immediately pair every
    // non-null return with `dlclose`. No pointer escapes this function.
    unsafe {
        let handle = libc::dlopen(name.as_ptr(), libc::RTLD_LAZY);
        if handle.is_null() {
            false
        } else {
            libc::dlclose(handle);
            true
        }
    }
}

// -----------------------------------------------------------------------------
// Mount-arg helpers. Currently unused pending session-loop bring-up;
// kept at module scope so the shape of the FFI call sequence is
// reviewable alongside the struct definitions in `macos_ffi`.
// -----------------------------------------------------------------------------

/// Translate a filesystem path into a `CString` suitable for the
/// fuse-t FFI. Rejects paths containing interior NULs.
fn path_to_cstring(path: &Path) -> Result<CString, MountError> {
    CString::new(OsStr::new(path).as_bytes()).map_err(|_| {
        MountError::Unsupported(format!(
            "mountpoint path contains interior NUL: {}",
            path.display()
        ))
    })
}

/// Build a placeholder `fuse_args` representation. Full argv
/// construction (volname, allow_other, defer_permissions, iosize=...)
/// lands with the session loop; today we return a placeholder.
fn build_fuse_args(_opts: &MountOptions) -> Vec<CString> {
    // The full implementation will materialize:
    //   ["pcloud-rs",
    //    "-o", format!("volname={}", volname),
    //    "-o", "allow_other",
    //    "-o", "defer_permissions",
    //    ...]
    // for now we emit just the argv[0] placeholder so callers can see
    // the shape.
    vec![CString::new("pcloud-rs").expect("literal has no NUL")]
}

/// Construct the `LowlevelOps` vtable for Phase-3 bring-up. Wires the
/// minimum four callbacks (`init`, `destroy`, `lookup`, `getattr`);
/// every other slot remains `None` so libfuse replies `ENOSYS` to the
/// kernel automatically.
fn build_lowlevel_ops() -> macos_ffi::LowlevelOps {
    let mut ops = macos_ffi::LowlevelOps::default();
    ops.init = Some(thunk_init);
    ops.destroy = Some(thunk_destroy);
    ops.lookup = Some(thunk_lookup);
    ops.getattr = Some(thunk_getattr);
    ops.open = Some(thunk_open);
    ops.read = Some(thunk_read);
    ops.readdir = Some(thunk_readdir);
    ops.release = Some(thunk_release);
    ops.statfs = Some(thunk_statfs);
    // Phase 5: write-path wiring.
    ops.write = Some(thunk_write);
    ops.create = Some(thunk_create);
    ops.unlink = Some(thunk_unlink);
    ops.mkdir = Some(thunk_mkdir);
    ops.rmdir = Some(thunk_rmdir);
    ops.rename = Some(thunk_rename);
    ops.flush = Some(thunk_flush);
    ops.fsync = Some(thunk_fsync);
    ops.setattr = Some(thunk_setattr);
    ops
    // Wiring plan (bd-1du.4):
    //   ops.init     = Some(thunk_init);
    //   ops.destroy  = Some(thunk_destroy);
    //   ops.lookup   = Some(thunk_lookup);
    //   ops.getattr  = Some(thunk_getattr);
    //   ops.readdir  = Some(thunk_readdir);
    //   ops.open     = Some(thunk_open);
    //   ops.read     = Some(thunk_read);
    //   ops.write    = Some(thunk_write);
    //   ops.flush    = Some(thunk_flush);
    //   ops.release  = Some(thunk_release);
    //   ops.create   = Some(thunk_create);
    //   ops.unlink   = Some(thunk_unlink);
    //   ops.mkdir    = Some(thunk_mkdir);
    //   ops.rmdir    = Some(thunk_rmdir);
    //   ops.rename   = Some(thunk_rename);
    //   ops.setattr  = Some(thunk_setattr);
    //   ops.fsync    = Some(thunk_fsync);
    // Each thunk:
    //   extern "C" fn thunk_<op>(req, ..., user_data-bearing path) {
    //       // SAFETY: user_data was installed via fuse_lowlevel_new
    //       // from a Box<Box<dyn FuseAdapter>> and lives for the
    //       // duration of the session.
    //       let adapter: &dyn FuseAdapter =
    //           &**(user_data as *const Box<dyn FuseAdapter>);
    //       match adapter.<op>(...) {
    //           Ok(v) => fuse_reply_<op>(req, ...),
    //           Err(errno) => { fuse_reply_err(req, errno); }
    //       }
    //   }
}

// -----------------------------------------------------------------------------
// MountinfoReader (macOS): enumerates live FUSE mounts for orphan
// detection via `getmntinfo(3)`.
// -----------------------------------------------------------------------------

/// macOS mountinfo reader backed by `getmntinfo(3)`.
///
/// Enumerates the kernel mount table and emits a
/// `/proc/self/mountinfo`-compatible payload containing only FUSE-backed
/// entries (those whose `f_fstypename` contains `"fuse"`, which covers
/// both `macfuse` and `fuse-t`). Emitted lines advertise `fuse.pcloud`
/// as the filesystem type so the shared parser in
/// [`crate::mount_orphan::parse_pcloud_mounts`] classifies them as
/// candidate pCloud mounts; the daemon then reconciles against its own
/// known-mount set.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosMountinfoReader;

impl MountinfoReader for MacosMountinfoReader {
    fn read(&self) -> io::Result<String> {
        read_getmntinfo()
    }
}

fn read_getmntinfo() -> io::Result<String> {
    // SAFETY: `getmntinfo` accepts a non-null out-pointer in which the
    // kernel stores a pointer to a libc-owned statically-allocated
    // array. On success it returns the number of entries (>0); on
    // failure it returns 0 and sets `errno`. We never free the returned
    // buffer and do not retain the pointer beyond this function.
    let mut mntbuf: *mut libc::statfs = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut mntbuf, libc::MNT_NOWAIT) };
    if count <= 0 || mntbuf.is_null() {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `mntbuf` points to `count` initialized `statfs` structs
    // owned by libc. The slice lives only within this function scope
    // and we only read through it. `count` is checked positive above.
    let entries = unsafe { std::slice::from_raw_parts(mntbuf, count as usize) };

    let mut out = String::new();
    for entry in entries {
        let fstype = cstr_to_string(entry.f_fstypename.as_ptr());
        if !fstype.contains("fuse") {
            continue;
        }
        let mountpoint = cstr_to_string(entry.f_mntonname.as_ptr());
        if mountpoint.is_empty() {
            continue;
        }

        // Emit a minimal `/proc/self/mountinfo`-shaped line. The parser
        // requires five space-delimited fields on the left of " - "
        // (mountpoint at field index 4) and a matching fstype on the
        // right.
        //
        // Fields: id parent_id major:minor root mountpoint - fstype src opts
        out.push_str("0 0 0:0 / ");
        out.push_str(&escape_mountinfo(&mountpoint));
        out.push_str(" - fuse.pcloud ");
        let src = cstr_to_string(entry.f_mntfromname.as_ptr());
        if src.is_empty() {
            out.push_str("pcloud");
        } else {
            out.push_str(&escape_mountinfo(&src));
        }
        out.push_str(" rw\n");
    }
    Ok(out)
}

/// Convert a NUL-terminated C string to an owned Rust `String`, lossily.
/// Returns an empty string when the pointer is null.
fn cstr_to_string(ptr: *const libc::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: `ptr` points into a libc-owned `statfs` whose character
    // arrays are NUL-terminated per statfs(2). We do not retain the
    // CStr reference beyond this scope.
    let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
    cstr.to_string_lossy().into_owned()
}

/// Escape whitespace and backslash per `proc(5)` mountinfo rules so the
/// shared parser can recover the original path bytes.
fn escape_mountinfo(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            ' ' => out.push_str("\\040"),
            '\t' => out.push_str("\\011"),
            '\n' => out.push_str("\\012"),
            '\\' => out.push_str("\\134"),
            other => out.push(other),
        }
    }
    out
}

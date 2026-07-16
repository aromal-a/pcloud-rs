//! **PLATFORM: macOS only.**
//! **GATING: `#[cfg(target_os = "macos")]`** -- the entire module file is
//! gated at the `mod macos;` line in `platform/mod.rs`.
//!
//! **Running on a real Mac.** fuse-t must be installed. The strict native
//! workflow exercises readdir, create/write/fsync, and unmount on a labelled
//! fuse-t runner; a workflow definition is not a claim that a given release
//! commit passed it.
//!
//! Implementation strategy: **fuse-t** (<https://www.fuse-t.org/>) via
//! direct FFI to its shipped `libfuse.dylib`, which is ABI-compatible
//! with libfuse 2.9's low-level API. We hand-roll the binding in
//! [`macos_ffi`] instead of depending on an external crate so the
//! surface stays small, auditable, and free of transitive macOS-only
//! deps that would churn the workspace.
//!
//! The probe path, option defaults, session loop, read/write thunks, and RAII
//! teardown are populated. Native CI owns dylib ABI and VFS qualification;
//! Linux builds remain isolated through target cfgs.
//!
//! **WRITE SURFACE (U3):** the `create`, `unlink`, `mkdir`, `rmdir`,
//! and `rename` thunks now resolve parent inodes to remote paths via
//! `FuseAdapter::resolve_ino_to_path` and pass real paths through to
//! the adapter's write-side methods. Live-host bring-up is still
//! required to exercise the kernel-VFS integration for these calls on
//! an actual fuse-t mount in the native gate.
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

#[path = "macos_ffi.rs"]
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
        let backend = MacFuseBackend::from_env();
        if find_libfuse_install_path(backend).is_some() {
            return Ok(());
        }
        Err(MountError::Unsupported(install_hint(backend)))
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
        // allow_other is disabled by default — setting it enables world-readable mounts
        opts.allow_other = false;
        if opts.fs_name.is_none() {
            opts.fs_name = Some("pCloud".to_string());
        }
        opts
    }

    /// Mount a boxed [`FuseAdapter`] at `mount_point` using fuse-t.
    ///
    /// The implementation validates the mountpoint, probes fuse-t, starts
    /// `fuse_session_loop` on a background thread, and returns an RAII mount
    /// handle. Missing or incompatible fuse-t fails as a surfaced
    /// [`MountError::Unsupported`].
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
// Native verification is owned by the strict fuse-t workflow.
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
    // Publish fuse-t's `libfuse.dylib` symbols into the process's flat
    // namespace before the first call to any `fuse_*` extern. Without
    // this the `-undefined,dynamic_lookup` linker flag will defer the
    // symbols forever and dyld will abort on first call.
    ensure_libfuse_loaded()?;

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

    // Full-size backing buffer sized to the upstream libfuse 2.9
    // `struct fuse_lowlevel_ops` layout (see `LowlevelOpsCompat`). We
    // zero-initialize the tail (slots our adapter does not populate
    // such as getlk/setlk/bmap/ioctl/poll/flock/fallocate/...) so
    // libfuse observes `NULL` function pointers and returns `ENOSYS`
    // for those ops. The prefix bytes are copied from our populated
    // `LowlevelOps`; the compile-time assertion in `macos_ffi.rs`
    // guarantees `size_of::<LowlevelOps>() <= LOWLEVEL_OPS_SIZE`.
    //
    // Audit (bd-1du.4 / §5-opus C-2 / §5-sonnet C-2): previously we
    // passed `size_of::<LowlevelOps>()` to `fuse_lowlevel_new`, which
    // is strictly smaller than `sizeof(struct fuse_lowlevel_ops)` for
    // the libfuse 2.9 ABI. That made libfuse read uninitialized memory
    // past our buffer (UB) or, equivalently, install thunks at wrong
    // offsets. Always use `LOWLEVEL_OPS_SIZE` with a matching-size
    // zero-initialized buffer.
    let mut ops_buf = vec![0u8; macos_ffi::LOWLEVEL_OPS_SIZE];
    // SAFETY: destination `ops_buf` is a heap allocation of exactly
    // `LOWLEVEL_OPS_SIZE` bytes (>= `size_of::<LowlevelOps>()` by the
    // const-assert in macos_ffi.rs). Source `ops` is a valid
    // `LowlevelOps`. Regions do not overlap. `LowlevelOps` is `#[repr(C)]`
    // and contains only `Option<extern "C" fn(...)>` slots, which are
    // plain-old-data.
    // SAFETY: see paragraph above.
    unsafe {
        std::ptr::copy_nonoverlapping(
            (&ops as *const macos_ffi::LowlevelOps) as *const u8,
            ops_buf.as_mut_ptr(),
            std::mem::size_of::<macos_ffi::LowlevelOps>(),
        );
    }

    // SAFETY: `args` is still alive; `ops_buf` is a zero-initialized
    // byte buffer of exact upstream size with our populated op
    // pointers copied into its prefix. `user_data_ptr` is a
    // heap-stable address whose target lives for the session.
    let session = unsafe {
        macos_ffi::fuse_lowlevel_new(
            &mut args,
            ops_buf.as_ptr() as *const std::ffi::c_void,
            macos_ffi::LOWLEVEL_OPS_SIZE,
            user_data_ptr,
        )
    };
    // libfuse copies the ops table internally during
    // `fuse_lowlevel_new`, so `ops_buf` can be dropped here.
    drop(ops_buf);
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

    // SIGTERM / SIGINT trampoline (bd-1du.4 / §5-sonnet C-1). Install a
    // `sigaction`-based handler that flips an AtomicBool, register this
    // session with a process-wide registry, and spawn a reaper thread
    // that blocks on a Condvar bound to the flag. On wake the reaper
    // walks the registry and calls `fuse_session_exit` on every live
    // session, which breaks the loop and lets `teardown_macos` run
    // `fuse_unmount` / `fuse_session_destroy`. Without this handler a
    // SIGTERM would terminate the process and leave a stale kernel
    // mount requiring a manual `umount -f` to clear.
    install_signal_handler_once();
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    register_active_session(session, shutdown.clone());

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
// SAFETY: see "# Safety" doc comment above.
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
// SAFETY: see "# Safety" doc comment above.
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
    // Report st_blocks as 512-byte units so that `du -sh` reports correct
    // disk usage on fuse-t mounts. Without this field, `du` shows 0 for
    // every file. Ceiling division: (size + 511) / 512.
    st.st_blksize = 512;
    st.st_blocks = ((attr.size + 511) / 512) as i64;
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
    // Reasonable block size for FUSE (standard 4 KiB)
    st.st_blksize = 4096;
    // 512-byte block count consistent with st_size (macOS stat convention)
    st.st_blocks = (attr.size as i64 + 511) / 512;
    // Set birthtime to mtime if we don't have a separate birthtime.
    // Use mtime_nsec for sub-second precision (APFS supports nanosecond timestamps).
    if let Some(mtime) = attr.mtime_epoch {
        st.st_mtime = mtime as i64;
        st.st_mtime_nsec = attr.mtime_nsec as libc::c_long;
        st.st_ctime = mtime as i64;
        st.st_ctime_nsec = attr.mtime_nsec as libc::c_long;
        st.st_atime = mtime as i64;
        st.st_atime_nsec = 0;
        st.st_birthtime = mtime as i64; // macOS-specific
        st.st_birthtime_nsec = 0; // macOS-specific
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
                log::debug!(
                    "[pcloud-fuse-t] lookup parent={parent} name={name_str} FAILED errno={errno}"
                );
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
                log::debug!("[pcloud-fuse-t] getattr ino={ino} FAILED errno={errno}");
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
                log::debug!("[pcloud-fuse-t] open ino={ino} -> handle={handle_id}");
                // SAFETY: `fi` is writable for this callback per the
                // libfuse contract; we only store the handle id.
                unsafe { (*fi).fh = handle_id };
                // SAFETY: `req` is valid; `fi` is valid and its
                // contents are read synchronously by libfuse.
                unsafe { macos_ffi::fuse_reply_open(req, fi) };
            }
            Err(errno) => {
                log::debug!("[pcloud-fuse-t] open ino={ino} FAILED errno={errno}");
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        log::debug!("[pcloud-fuse-t] open PANIC ino={ino}");
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
    ino: macos_ffi::fuse_ino_t,
    size: usize,
    off: i64,
    fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if fi.is_null() {
            log::debug!(
                "[pcloud-fuse-t] read ino={ino} off={off} size={size} fi=NULL"
            );
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
                log::debug!(
                    "[pcloud-fuse-t] read ino={ino} fh={handle_id} adapter=NULL"
                );
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let offset = if off < 0 { 0u64 } else { off as u64 };
        // If fuse-t's NFS bridge did not carry an `fh` across from
        // `open` (observed on some NFSv4 client flows), fall back to
        // opening the file on demand keyed on the inode.
        let effective = if handle_id == 0 {
            log::debug!(
                "[pcloud-fuse-t] read ino={ino} off={off} size={size} fh=0 — opening on demand"
            );
            match adapter.open(ino) {
                Ok(h) => {
                    log::debug!(
                        "[pcloud-fuse-t] read ino={ino} on-demand handle={h}"
                    );
                    // Store so subsequent reads on this `fi` skip the
                    // fallback. fuse-t-maintained state is best-effort
                    // and may still be reset per NFS request.
                    // SAFETY: `fi` is writable for this callback.
                    unsafe { (*fi).fh = h };
                    h
                }
                Err(errno) => {
                    log::debug!(
                        "[pcloud-fuse-t] read ino={ino} on-demand OPEN FAILED errno={errno}"
                    );
                    // SAFETY: `req` is valid.
                    unsafe { macos_ffi::fuse_reply_err(req, errno) };
                    return;
                }
            }
        } else {
            handle_id
        };
        match adapter.read(effective, offset, size) {
            Ok(bytes) => {
                log::debug!(
                    "[pcloud-fuse-t] read ino={ino} fh={effective} off={off} req={size} got={}",
                    bytes.len()
                );
                // SAFETY: `req` is valid; `bytes.as_ptr()` and
                // `bytes.len()` describe a live slice that libfuse
                // copies synchronously before returning.
                unsafe { macos_ffi::fuse_reply_buf(req, bytes.as_ptr(), bytes.len()) };
            }
            Err(errno) => {
                log::debug!(
                    "[pcloud-fuse-t] read ino={ino} fh={effective} off={off} size={size} FAILED errno={errno}"
                );
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
            }
        }
    })
    .unwrap_or_else(|_| {
        log::debug!("[pcloud-fuse-t] read PANIC ino={ino} off={off} size={size}");
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
                mtime_nsec: 0,
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
        log::debug!("[pcloud-fuse-t] write ino={ino} off={offset} size={size}");
        match adapter.write(ino, offset, data) {
            Ok(count) => {
                log::debug!("[pcloud-fuse-t] write ino={ino} off={offset} -> {count} bytes");
                // SAFETY: `req` is valid; `count` is the byte total
                // libfuse will pass back to the kernel.
                unsafe { macos_ffi::fuse_reply_write(req, count) };
            }
            Err(errno) => {
                log::debug!("[pcloud-fuse-t] write ino={ino} off={offset} FAILED errno={errno}");
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
        log::debug!(
            "[pcloud-fuse-t] create parent={parent} name={name_str}"
        );
        // U3: resolve parent ino -> absolute remote path via the trait.
        let parent_buf = match adapter.resolve_ino_to_path(parent) {
            Ok(p) => p,
            Err(errno) => {
                log::debug!(
                    "[pcloud-fuse-t] create parent={parent} resolve_ino_to_path FAILED errno={errno}"
                );
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let parent_path = parent_buf.to_string_lossy();
        let new_ino = match adapter.create(&parent_path, &name_str) {
            Ok(ino) => ino,
            Err(errno) => {
                log::debug!(
                    "[pcloud-fuse-t] create parent_path={parent_path} name={name_str} FAILED errno={errno}"
                );
                // SAFETY: `req` is valid.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        log::debug!(
            "[pcloud-fuse-t] create parent_path={parent_path} name={name_str} ok new_ino={new_ino}"
        );
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
        // SAFETY: `fi` is a libfuse-allocated `fuse_file_info*` valid for
        // the duration of this callback; writing the `fh` field is the
        // documented way to publish a handle id back to libfuse.
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

/// `statfs` thunk. Account quota comes from the canonical backend through
/// [`FuseAdapter::statfs`]; local disk capacity and synthetic defaults are
/// deliberately never reported as remote capacity.
///
/// # Safety
/// `req` is live for this call.
extern "C" fn thunk_statfs(req: macos_ffi::fuse_req_t, _ino: macos_ffi::fuse_ino_t) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is live; userdata outlives the session.
        let adapter = match unsafe { adapter_from_req(req) } {
            Some(adapter) => adapter,
            None => {
                // SAFETY: `req` is valid and must receive exactly one reply.
                unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
                return;
            }
        };
        let (total_bytes, free_bytes) = match adapter.statfs() {
            Ok(quota) => quota,
            Err(errno) => {
                // SAFETY: `req` is valid and must receive exactly one reply.
                unsafe { macos_ffi::fuse_reply_err(req, errno) };
                return;
            }
        };
        let (blocks, free_blocks) = crate::fuse_adapter::statfs_blocks(total_bytes, free_bytes);
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        st.f_namemax = 255;
        st.f_bsize = u64::from(crate::fuse_adapter::STATFS_BLOCK_SIZE);
        st.f_frsize = u64::from(crate::fuse_adapter::STATFS_BLOCK_SIZE);
        // macOS libc::statvfs uses u32 for block counts; clamp to u32::MAX
        // on overflow so the reply remains ABI-correct.
        st.f_blocks = blocks.min(u32::MAX as u64) as u32;
        st.f_bfree = free_blocks.min(u32::MAX as u64) as u32;
        st.f_bavail = free_blocks.min(u32::MAX as u64) as u32;
        // pCloud does not publish inode quotas.
        st.f_files = 0;
        st.f_ffree = 0;
        // SAFETY: `req` is a valid libfuse request handle; `&st` is a
        // fully-initialised `statvfs` on the stack, alive for this call.
        unsafe { macos_ffi::fuse_reply_statfs(req, &st) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: panic recovery; `req` is still valid and must be replied to.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

// -----------------------------------------------------------------------------
// SIGTERM / SIGINT trampoline (bd-1du.4 / §5-sonnet C-1).
//
// Mirrors the Linux sigaction+reaper pattern: the signal handler only
// flips an `AtomicBool` (async-signal-safe) and notifies a `Condvar`.
// A dedicated reaper thread blocks on that Condvar and, on wake, walks
// a registry of live `fuse_session` pointers calling
// `fuse_session_exit` on each, which unblocks `fuse_session_loop`.
// The cooperating `teardown_macos` path then runs `fuse_unmount` /
// `fuse_session_destroy` under normal Drop.
//
// This matches the behavior users expect when SIGTERM/SIGINT lands on
// the daemon: the mount is released cleanly instead of the kernel
// retaining a stale fuse-t mount that requires manual `umount -f`.
// -----------------------------------------------------------------------------

/// Async-signal-safe flag set by [`signal_trampoline`]. The reaper
/// thread reads this under its Condvar guard.
static SHUTDOWN_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Ensures the signal handler and reaper thread are installed exactly
/// once per process.
static SIGNAL_HANDLER_INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Opaque registry entry tracking a live fuse-t session whose loop
/// should be broken on SIGTERM/SIGINT.
struct RegisteredSession {
    session: *mut macos_ffi::fuse_session,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

// SAFETY: the `fuse_session` pointer is an opaque kernel handle owned
// by the `MountHandle` for the duration of the mount. The reaper only
// ever calls `fuse_session_exit` on it, which libfuse documents as
// safe across threads. We remove the entry from the registry before
// `fuse_session_destroy` runs in `teardown_macos`.
// SAFETY: see block above.
unsafe impl Send for RegisteredSession {}
// SAFETY: see block above.
unsafe impl Sync for RegisteredSession {}

/// Mutex + Condvar pair used by the reaper thread. The `bool` in the
/// Mutex is the same signal observed by [`SHUTDOWN_REQUESTED`]; we
/// duplicate it here so `Condvar::wait_while` has something to read
/// under the guard.
fn reaper_state() -> &'static (std::sync::Mutex<bool>, std::sync::Condvar) {
    static STATE: std::sync::OnceLock<(std::sync::Mutex<bool>, std::sync::Condvar)> =
        std::sync::OnceLock::new();
    STATE.get_or_init(|| (std::sync::Mutex::new(false), std::sync::Condvar::new()))
}

/// Registry of live sessions keyed by raw pointer. Iterated only from
/// non-signal contexts (the reaper thread and `register`/`deregister`
/// callers). We never lock this Mutex from the signal handler.
fn session_registry() -> &'static std::sync::Mutex<Vec<RegisteredSession>> {
    static REG: std::sync::OnceLock<std::sync::Mutex<Vec<RegisteredSession>>> =
        std::sync::OnceLock::new();
    REG.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Install the SIGTERM/SIGINT handler exactly once per process, and
/// spawn the reaper thread.
fn install_signal_handler_once() {
    SIGNAL_HANDLER_INSTALLED.get_or_init(|| {
        // SAFETY: `sigaction(2)` is invoked once per process with a
        // static extern-C handler and a zero-initialized `sigaction`
        // struct whose `sa_flags` sets `SA_RESTART` (so restartable
        // syscalls resume cleanly). The handler body only stores to
        // an `AtomicBool` and calls `pthread_cond_signal`, both of
        // which are async-signal-safe on Darwin.
        // SAFETY: see paragraph above.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = signal_trampoline as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            // Ignore any pre-existing handler; this is process-wide
            // install and we accept the (very small) race where a
            // prior handler is clobbered — the daemon owns its own
            // signal policy and tests are single-process.
            libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
            libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        }

        // Reaper thread. Blocks on the Condvar until the signal
        // handler flips the flag, then walks the registry and calls
        // `fuse_session_exit` on each live session. The exit call
        // unblocks `fuse_session_loop` on the loop thread, which then
        // returns and lets `teardown_macos` complete cleanly.
        let _ = std::thread::Builder::new()
            .name("pcloud-fuse-t-reaper".to_string())
            .spawn(|| {
                let (lock, cvar) = reaper_state();
                loop {
                    let mut guard = match lock.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    guard = cvar
                        .wait_while(guard, |triggered| !*triggered)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    // Drain the trigger and drop the guard before
                    // touching the session registry to avoid holding
                    // unrelated locks across FFI.
                    *guard = false;
                    drop(guard);

                    if !SHUTDOWN_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
                        continue;
                    }

                    // Snapshot the registry under its own lock.
                    let sessions: Vec<(*mut macos_ffi::fuse_session, _)> = {
                        match session_registry().lock() {
                            Ok(g) => g.iter().map(|e| (e.session, e.shutdown.clone())).collect(),
                            Err(_) => Vec::new(),
                        }
                    };

                    for (session, shutdown) in sessions {
                        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                        if !session.is_null() {
                            // SAFETY: session pointer was registered
                            // from `mount_with_fuse_t` and is live
                            // until `teardown_macos` deregisters it
                            // (before `fuse_session_destroy`).
                            unsafe { macos_ffi::fuse_session_exit(session) };
                        }
                    }
                }
            });
    });
}

/// Async-signal-safe signal trampoline. Flips the atomic flag and
/// wakes the reaper via `pthread_cond_signal`, which is documented
/// async-signal-safe on Darwin. We deliberately do NOT lock any
/// Mutex here.
extern "C" fn signal_trampoline(_sig: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
    let (lock, cvar) = reaper_state();
    // Best-effort: flip the duplicated bool under the guard so
    // `wait_while` re-evaluates. Using `try_lock` avoids deadlock if
    // the reaper happens to hold it — the Condvar notify below still
    // wakes the waiter.
    if let Ok(mut guard) = lock.try_lock() {
        *guard = true;
    }
    cvar.notify_all();
}

/// Register a live fuse-t session with the signal reaper. Call after
/// `fuse_lowlevel_new` succeeds. The matching deregistration happens
/// in [`deregister_active_session`], which must be called before
/// `fuse_session_destroy` runs to ensure the reaper cannot call
/// `fuse_session_exit` on a destroyed session.
pub(crate) fn register_active_session(
    session: *mut macos_ffi::fuse_session,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    if let Ok(mut guard) = session_registry().lock() {
        guard.push(RegisteredSession { session, shutdown });
    }
}

/// Remove a session from the signal reaper registry. Must be called
/// by `teardown_macos` before `fuse_session_destroy` so the reaper
/// does not race with session destruction on a delayed signal.
///
/// audit-06 closure (2026-04-18): `mount_service::MountHandle::teardown_macos`
/// (see `mount_service.rs:556`) now invokes this helper BEFORE the
/// `fuse_session_destroy` call (and before the loop-thread join), closing
/// the prior destroy-then-signal UAF window. The stale audit-04 TODO
/// previously living on this function has been removed because the
/// teardown path is now correct.
pub(crate) fn deregister_active_session(session: *mut macos_ffi::fuse_session) {
    if let Ok(mut guard) = session_registry().lock() {
        guard.retain(|e| e.session != session);
    }
}

// -----------------------------------------------------------------------------
// Probe helpers.
// -----------------------------------------------------------------------------

/// Which userspace libfuse implementation pcloud-rs should bind at
/// mount time.
///
/// Both fuse-t and macFUSE export the libfuse 2.9 low-level ABI, so
/// our FFI is agnostic once the right dylib is loaded. The default is
/// [`Self::FuseT`] because it needs no kernel extension and no SIP
/// carve-out; [`Self::MacFuse`] is available for callers who already
/// rely on macFUSE. [`Self::Auto`] probes fuse-t first, then macFUSE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacFuseBackend {
    FuseT,
    MacFuse,
    Auto,
}

impl MacFuseBackend {
    /// Read the selector from `PCLOUD_MACOS_FUSE_BACKEND`. Unknown or
    /// empty values fall back to the default ([`Self::FuseT`]).
    fn from_env() -> Self {
        match std::env::var("PCLOUD_MACOS_FUSE_BACKEND")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("fuse-t") | Some("fuset") | Some("fuse_t") => Self::FuseT,
            Some("macfuse") | Some("mac-fuse") | Some("mac_fuse") | Some("osxfuse") => {
                Self::MacFuse
            }
            Some("auto") => Self::Auto,
            _ => Self::FuseT,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FuseT => "fuse-t",
            Self::MacFuse => "macFUSE",
            Self::Auto => "auto",
        }
    }
}

/// Known absolute install paths for **fuse-t**'s userspace libfuse
/// shim. Separate from [`MACFUSE_CANDIDATES`] because the two stacks
/// ship under distinct file names so they can coexist on the same
/// machine. macOS dyld does not search `/usr/local/lib` or
/// `/opt/homebrew/lib` for bare dlopen names on modern SIP-enforced
/// systems, so we probe absolute paths.
const FUSET_CANDIDATES: &[&str] = &[
    "/usr/local/lib/libfuse-t.dylib",
    "/opt/homebrew/lib/libfuse-t.dylib",
    "/Library/Application Support/fuse-t/lib/libfuse-t.dylib",
];

/// Known absolute install paths for **macFUSE**'s kext-backed libfuse
/// 2.9-compat shim. macFUSE requires its kernel extension to be
/// approved in System Settings → Privacy & Security; without that
/// `fuse_mount` returns `mount_macfuse: the file system is not
/// available`. This module does not attempt to detect kext-approval
/// state — the daemon surfaces the libfuse error verbatim.
const MACFUSE_CANDIDATES: &[&str] = &[
    "/usr/local/lib/libfuse.dylib",
    "/opt/homebrew/lib/libfuse.dylib",
];

/// Attempt to `dlopen` `path` and confirm that the `fuse_mount` symbol
/// resolves. Returns `true` only when both succeed.
///
/// The handle is closed immediately after the symbol check — `dlopen` is
/// idempotent on macOS (the kernel reference-counts dylibs), so the
/// subsequent call in [`ensure_libfuse_loaded`] with `RTLD_GLOBAL` will
/// return the already-loaded image without re-parsing the dylib.
///
/// # Safety
/// `dlopen`/`dlsym`/`dlclose` are POSIX; we hold the handle only for the
/// probe duration and release it unconditionally before returning.
fn probe_with_dlopen(path: &str) -> bool {
    use std::ffi::CString;
    let path_c = match CString::new(path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // SAFETY: dlopen/dlclose are standard POSIX; we hold the handle only
    // briefly for the probe and close it immediately.
    let handle = unsafe { libc::dlopen(path_c.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return false;
    }
    let sym_name = match CString::new("fuse_mount") {
        Ok(s) => s,
        Err(_) => {
            // SAFETY: `handle` was successfully returned by `dlopen` above
            // and has not been closed yet. `dlclose` on a valid handle is safe.
            unsafe { libc::dlclose(handle) };
            return false;
        }
    };
    let sym = unsafe { libc::dlsym(handle, sym_name.as_ptr()) };
    // SAFETY: `handle` is a valid, non-null `dlopen` result; closing it here
    // releases our brief probe hold. No Rust reference to the library remains.
    unsafe { libc::dlclose(handle) };
    !sym.is_null()
}

/// Return the first loadable (and fuse_mount-bearing) install candidate
/// for `backend`, if any.
///
/// Uses [`probe_with_dlopen`] rather than a plain `Path::exists()` check
/// so we confirm the library is both present and ABI-functional before
/// advertising support.
fn find_libfuse_install_path(backend: MacFuseBackend) -> Option<&'static str> {
    let probe = |candidates: &[&'static str]| -> Option<&'static str> {
        candidates.iter().copied().find(|p| probe_with_dlopen(p))
    };
    match backend {
        MacFuseBackend::FuseT => probe(FUSET_CANDIDATES),
        MacFuseBackend::MacFuse => probe(MACFUSE_CANDIDATES),
        MacFuseBackend::Auto => probe(FUSET_CANDIDATES).or_else(|| probe(MACFUSE_CANDIDATES)),
    }
}

/// Human-readable install hint used by `probe_supported` / mount
/// bring-up when no dylib matching the requested backend is present.
fn install_hint(backend: MacFuseBackend) -> String {
    match backend {
        MacFuseBackend::FuseT => "fuse-t not installed; install from https://www.fuse-t.org/ \
             (or set PCLOUD_MACOS_FUSE_BACKEND=macfuse to use macFUSE)"
            .to_string(),
        MacFuseBackend::MacFuse => {
            "macFUSE not installed; install from https://macfuse.github.io/ \
             (or unset PCLOUD_MACOS_FUSE_BACKEND to use fuse-t)"
                .to_string()
        }
        MacFuseBackend::Auto => {
            "no macOS FUSE backend found; install fuse-t from https://www.fuse-t.org/ \
             or macFUSE from https://macfuse.github.io/"
                .to_string()
        }
    }
}

/// Ensure the selected backend's `libfuse*.dylib` is loaded into the
/// process with `RTLD_GLOBAL` so the `-undefined,dynamic_lookup` extern
/// symbols declared in [`macos_ffi`] can resolve on first call via
/// dyld's flat-namespace lookup.
///
/// The handle is intentionally leaked (never `dlclose`d) — the mount
/// session references `fuse_*` symbols for as long as the daemon is
/// alive, and a `dlclose` would invalidate them. Called once at the
/// start of every `mount_with_fuse_t`; `dlopen` is idempotent so
/// repeated calls return the same underlying library.
fn ensure_libfuse_loaded() -> Result<(), MountError> {
    let backend = MacFuseBackend::from_env();
    let path = find_libfuse_install_path(backend)
        .ok_or_else(|| MountError::Unsupported(install_hint(backend)))?;
    let Ok(path_c) = CString::new(path) else {
        return Err(MountError::Unsupported(format!(
            "libfuse path contains NUL: {path}"
        )));
    };
    // SAFETY: `dlopen` accepts a NUL-terminated C string and an integer
    // flag; it is documented thread-safe. `RTLD_GLOBAL` publishes the
    // library's symbols so subsequent flat-namespace lookups (triggered
    // by our `extern "C"` call sites) can find them. We intentionally
    // do not call `dlclose`: the library must outlive the mount session.
    let handle = unsafe { libc::dlopen(path_c.as_ptr(), libc::RTLD_LAZY | libc::RTLD_GLOBAL) };
    if handle.is_null() {
        // SAFETY: `dlerror` returns a pointer to a thread-local NUL-
        // terminated error string owned by libdl; valid until the next
        // dlopen/dlsym/dlclose call on this thread.
        let err = unsafe {
            let e = libc::dlerror();
            if e.is_null() {
                "dlopen returned NULL".to_string()
            } else {
                std::ffi::CStr::from_ptr(e).to_string_lossy().into_owned()
            }
        };
        return Err(MountError::Unsupported(format!(
            "failed to load {} backend ({path}): {err}",
            backend.label()
        )));
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Mount-argument helpers used by the live session bring-up.
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

/// `forget` thunk. Called when the kernel drops its reference count on
/// an inode. We do not maintain a ref-counted inode table yet, so this
/// is a no-op — it must exist so libfuse does not default to an error.
extern "C" fn thunk_forget(
    _req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    _nlookup: u64,
) {
    // No kernel reply expected for forget — it is a one-way notification.
}

/// `opendir` thunk. Returns the directory handle immediately; we don't
/// maintain per-dir handles so we use the inode as the handle id.
extern "C" fn thunk_opendir(
    req: macos_ffi::fuse_req_t,
    ino: macos_ffi::fuse_ino_t,
    fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        if fi.is_null() {
            // SAFETY: `req` is a valid libfuse request handle for the
            // duration of this callback. `fuse_reply_err` consumes it.
            unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
            return;
        }
        // SAFETY: `fi` is non-null and libfuse-owned for this callback;
        // writing `fh` is the documented way to set the file handle.
        unsafe { (*fi).fh = ino };
        // SAFETY: `req`, `fi` are libfuse-owned and valid for this call.
        unsafe { macos_ffi::fuse_reply_open(req, fi) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: `req` is a valid libfuse request handle; panic recovery
        // path must still reply to avoid a hung kernel operation.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `releasedir` thunk. Paired with `opendir`; no-op since we have no
/// per-dir handle state to release.
extern "C" fn thunk_releasedir(
    req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    _fi: *mut macos_ffi::fuse_file_info,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is a valid libfuse request handle for the
        // duration of this callback; 0 means success (no error).
        unsafe { macos_ffi::fuse_reply_err(req, 0) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: panic recovery path; `req` is still valid and must
        // be replied to so the kernel request does not hang.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `getxattr` thunk. Returns `ENOATTR` for all extended attribute requests.
/// macOS Finder and system daemons probe for xattrs on every file;
/// returning `ENOSYS` causes Finder to treat the volume as broken.
/// `ENOATTR` (= `ENODATA` on Linux, but macOS uses its own constant) is
/// the correct "attribute does not exist" response.
extern "C" fn thunk_getxattr(
    req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    _name: *const std::os::raw::c_char,
    _size: usize,
) {
    let _ = std::panic::catch_unwind(|| {
        // ENOATTR is not in libc for all platforms, but on macOS it is
        // defined as 93. Use the raw constant so we don't depend on a
        // non-portable libc symbol.
        const ENOATTR: i32 = 93;
        // SAFETY: `req` is a valid libfuse request handle for the
        // duration of this callback; replying with ENOATTR is the
        // documented response for "attribute does not exist".
        unsafe { macos_ffi::fuse_reply_err(req, ENOATTR) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: panic recovery; `req` is still valid and must be
        // replied to so the kernel request does not hang.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `listxattr` thunk. Returns empty xattr list.
extern "C" fn thunk_listxattr(
    req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    size: usize,
) {
    let _ = std::panic::catch_unwind(|| {
        if size == 0 {
            // Query: return total size needed (0 = no xattrs).
            // SAFETY: `req` is a valid libfuse request handle.
            unsafe { macos_ffi::fuse_reply_xattr(req, 0) };
        } else {
            // Read: return empty buffer.
            // SAFETY: `req` is valid; null buf + 0 size is documented
            // by libfuse as "return empty data", not a null deref.
            unsafe { macos_ffi::fuse_reply_buf(req, std::ptr::null(), 0) };
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: panic recovery; `req` is still valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `setxattr` thunk. Rejects all xattr writes with `ENOTSUP`.
extern "C" fn thunk_setxattr_op(
    req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    _name: *const std::os::raw::c_char,
    _value: *const std::os::raw::c_char,
    _size: usize,
    _flags: i32,
    _position: u32,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is a valid libfuse request handle; ENOTSUP is
        // the correct response for unsupported xattr writes on macOS.
        unsafe { macos_ffi::fuse_reply_err(req, libc::ENOTSUP) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: panic recovery; `req` is still valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `removexattr` thunk. Returns `ENOATTR` since we have no xattrs.
extern "C" fn thunk_removexattr(
    req: macos_ffi::fuse_req_t,
    _ino: macos_ffi::fuse_ino_t,
    _name: *const std::os::raw::c_char,
) {
    let _ = std::panic::catch_unwind(|| {
        const ENOATTR: i32 = 93;
        // SAFETY: `req` is a valid libfuse request handle; ENOATTR
        // indicates "no such attribute" as required by the protocol.
        unsafe { macos_ffi::fuse_reply_err(req, ENOATTR) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: panic recovery; `req` is still valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// `access` thunk. Permits all access checks — the kernel enforces
/// uid/gid/mode bits independently; our access thunk returning 0
/// defers to kernel-side permission checking.
extern "C" fn thunk_access(req: macos_ffi::fuse_req_t, _ino: macos_ffi::fuse_ino_t, _mask: i32) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `req` is a valid libfuse request handle; replying
        // with 0 grants access and is async-signal-safe from a thunk.
        unsafe { macos_ffi::fuse_reply_err(req, 0) };
    })
    .unwrap_or_else(|_| {
        // SAFETY: panic recovery; `req` is still valid.
        unsafe { macos_ffi::fuse_reply_err(req, libc::EIO) };
    });
}

/// Build the `fuse_args` argv for a fuse-t mount.
///
/// fuse-t's `fuse_mount` parses these options and forwards them to the
/// embedded NFS server that macOS mounts as the backing filesystem.
/// Without at least `-o rw` the NFS server exports read-only and every
/// create/write fails client-side with `EACCES` before the FUSE thunks
/// see the request (observed on macOS 14+ where `touch` / `echo >` in
/// the mountpoint fail with `Permission denied` and no `create` thunk
/// fires).
///
/// Options we emit:
/// - `rw`              — read-write export,
/// - `allow_other`     — fuse-t ignores this on the kernel layer
///                       because NFS already routes by uid, but the
///                       option is required to make the fuse layer
///                       accept cross-user ops triggered by
///                       macOS indexers (Spotlight/mds),
/// - `defer_permissions` — let FUSE mode/uid/gid bits govern access
///                         rather than the NFS client's cached perms,
/// - `fsname=pcloud-rs` — private source identity used by native orphan and
///                       sync-root discovery (never labels foreign FUSE),
/// - `volname=<fs_name>` — macOS Finder display name (falls back to
///                         "pCloud" if the caller did not set one).
fn build_fuse_args(opts: &MountOptions) -> Vec<CString> {
    let raw_volname = opts.fs_name.as_deref().unwrap_or("pCloud");
    // macOS fuse-t / macFUSE require volname ≤ 127 bytes. Clamp at a valid
    // UTF-8 char boundary to avoid splitting a multibyte sequence.
    let volname = if raw_volname.len() > 127 {
        log::warn!("FUSE volname truncated to 127 bytes");
        // `str::get(..127)` returns None if byte 127 is mid-char; fall back
        // to the full string only if the slice is valid (which it always is
        // when len > 127 and the first 127 bytes happen to be a valid UTF-8
        // prefix). Walk back to find the last char boundary ≤ 127.
        let mut end = 127;
        while end > 0 && !raw_volname.is_char_boundary(end) {
            end -= 1;
        }
        &raw_volname[..end]
    } else {
        raw_volname
    };
    let mut argv: Vec<CString> = Vec::with_capacity(8);
    // SAFETY: All `CString::new` call sites in this `argv` builder receive
    // hard-coded ASCII string literals (e.g. "pcloud-rs", "-o", "ro",
    // "rw", "allow_other", "defer_permissions"). None of these contain
    // interior NUL bytes, so `CString::new` is infallible here. Any
    // variable-length inputs (e.g. `volname_opt`) use `if let Ok` and
    // fall back safely.
    argv.push(CString::new("pcloud-rs").expect("literal has no NUL"));
    // Honour the read_only flag: pass `ro` or `rw` accordingly.
    argv.push(CString::new("-o").expect("literal has no NUL"));
    argv.push(CString::new(if opts.read_only { "ro" } else { "rw" }).expect("literal has no NUL"));
    // Only pass allow_other when the caller explicitly requests it.
    // MountService::validate_mountpoint rejects allow_other=true by default;
    // we must not hard-code it or we bypass that policy gate.
    if opts.allow_other {
        argv.push(CString::new("-o").expect("literal has no NUL"));
        argv.push(CString::new("allow_other").expect("literal has no NUL"));
    }
    argv.push(CString::new("-o").expect("literal has no NUL"));
    argv.push(CString::new("defer_permissions").expect("literal has no NUL"));
    argv.push(CString::new("-o").expect("literal has no NUL"));
    argv.push(CString::new("fsname=pcloud-rs").expect("literal has no NUL"));
    // `volname=…` may contain arbitrary user-supplied characters;
    // build through CString which will reject interior NULs (falling
    // back to the safe default).
    let volname_opt = format!("volname={volname}");
    if let Ok(s) = CString::new(volname_opt) {
        argv.push(CString::new("-o").expect("literal has no NUL"));
        argv.push(s);
    }
    argv
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
    ops.getxattr = Some(thunk_getxattr);
    ops.listxattr = Some(thunk_listxattr);
    ops.setxattr = Some(thunk_setxattr_op);
    ops.removexattr = Some(thunk_removexattr);
    ops.access = Some(thunk_access);
    ops.forget = Some(thunk_forget);
    ops.opendir = Some(thunk_opendir);
    ops.releasedir = Some(thunk_releasedir);
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
/// genuine pCloud entries. Generic FUSE mounts are filtered by both their
/// filesystem type and source identity so orphan cleanup cannot claim an
/// unrelated sshfs, rclone, or macFUSE volume. Emitted lines advertise the
/// private `fuse.pcloud-rs` marker consumed by the shared parser.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacosMountinfoReader;

impl MountinfoReader for MacosMountinfoReader {
    fn read(&self) -> io::Result<String> {
        read_getmntinfo()
    }
}

fn read_getmntinfo() -> io::Result<String> {
    // getmntinfo(3) is documented as not thread-safe: it returns a pointer
    // into a static internal buffer. Serialise all callers with a process-
    // wide mutex so concurrent test threads and runtime probes cannot race.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());

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
        let src = cstr_to_string(entry.f_mntfromname.as_ptr());
        let identity = format!("{fstype} {src}").to_ascii_lowercase();
        let is_supported_backend = [
            "fuse", "macfuse", "osxfuse", "fuse-t", "nfs", "smb", "fskit",
        ]
        .iter()
        .any(|marker| identity.contains(marker));
        if !is_supported_backend || !identity.contains("pcloud-rs") {
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
        out.push_str(" - fuse.pcloud-rs ");
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

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuse_adapter::{EntryAttr, FsEntryKind};
    use crate::mount_service::MountOptions;

    // -------------------------------------------------------------------------
    // escape_mountinfo
    // -------------------------------------------------------------------------

    #[test]
    fn escape_mountinfo_plain_path_unchanged() {
        assert_eq!(escape_mountinfo("/home/user/pcloud"), "/home/user/pcloud");
    }

    #[test]
    fn escape_mountinfo_space_becomes_040() {
        assert_eq!(escape_mountinfo("/mnt/my drive"), "/mnt/my\\040drive");
    }

    #[test]
    fn escape_mountinfo_tab_becomes_011() {
        assert_eq!(escape_mountinfo("/mnt/ta\tb"), "/mnt/ta\\011b");
    }

    #[test]
    fn escape_mountinfo_newline_becomes_012() {
        assert_eq!(escape_mountinfo("/mnt/new\nline"), "/mnt/new\\012line");
    }

    #[test]
    fn escape_mountinfo_backslash_becomes_134() {
        assert_eq!(escape_mountinfo("/mnt/back\\slash"), "/mnt/back\\134slash");
    }

    #[test]
    fn escape_mountinfo_multiple_specials_all_escaped() {
        let input = "/mnt/a b\tc\nd\\e";
        let got = escape_mountinfo(input);
        assert!(got.contains("\\040"), "space");
        assert!(got.contains("\\011"), "tab");
        assert!(got.contains("\\012"), "newline");
        assert!(got.contains("\\134"), "backslash");
    }

    #[test]
    fn escape_mountinfo_empty_string_stays_empty() {
        assert_eq!(escape_mountinfo(""), "");
    }

    #[test]
    fn escape_mountinfo_roundtrip_via_mountinfo_parser() {
        // Escape a path with a space, then feed to parse_pcloud_mounts.
        // The parser must recover the original path.
        use crate::mount_orphan::parse_pcloud_mounts;
        let raw = "/home/user/pCloud Drive";
        let escaped = escape_mountinfo(raw);
        // Construct a synthetic mountinfo line for the parser.
        let line = format!("0 0 0:0 / {escaped} - fuse.pcloud-rs pcloud-rs rw\n");
        let entries = parse_pcloud_mounts(&line);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].mount_point.to_str().unwrap(),
            raw,
            "unescape should recover original path"
        );
    }

    // -------------------------------------------------------------------------
    // build_fuse_args
    // -------------------------------------------------------------------------

    fn collect_args(opts: &MountOptions) -> Vec<String> {
        build_fuse_args(opts)
            .into_iter()
            .map(|cs| cs.into_string().expect("argv must be valid UTF-8"))
            .collect()
    }

    #[test]
    fn build_fuse_args_first_arg_is_program_name() {
        let args = collect_args(&MountOptions::default());
        assert_eq!(args[0], "pcloud-rs", "argv[0] must be the program name");
    }

    #[test]
    fn build_fuse_args_read_only_default_emits_ro() {
        let args = collect_args(&MountOptions {
            read_only: true,
            ..MountOptions::default()
        });
        let ro_pos = args.iter().position(|a| a == "ro");
        assert!(ro_pos.is_some(), "read-only must include 'ro' option");
        // Immediately preceded by -o
        let idx = ro_pos.unwrap();
        assert_eq!(args[idx - 1], "-o", "ro must follow -o");
    }

    #[test]
    fn build_fuse_args_read_write_emits_rw() {
        let args = collect_args(&MountOptions {
            read_only: false,
            ..MountOptions::default()
        });
        let rw_pos = args.iter().position(|a| a == "rw");
        assert!(rw_pos.is_some(), "read-write must include 'rw' option");
        let idx = rw_pos.unwrap();
        assert_eq!(args[idx - 1], "-o", "rw must follow -o");
    }

    #[test]
    fn build_fuse_args_ro_absent_when_read_write() {
        let args = collect_args(&MountOptions {
            read_only: false,
            ..MountOptions::default()
        });
        assert!(
            !args.contains(&"ro".to_string()),
            "'ro' must not appear in rw args"
        );
    }

    #[test]
    fn build_fuse_args_rw_absent_when_read_only() {
        let args = collect_args(&MountOptions {
            read_only: true,
            ..MountOptions::default()
        });
        assert!(
            !args.contains(&"rw".to_string()),
            "'rw' must not appear in ro args"
        );
    }

    #[test]
    fn build_fuse_args_allow_other_absent_by_default() {
        let args = collect_args(&MountOptions {
            allow_other: false,
            ..MountOptions::default()
        });
        assert!(
            !args.contains(&"allow_other".to_string()),
            "allow_other must not appear when not requested"
        );
    }

    #[test]
    fn build_fuse_args_allow_other_present_when_set() {
        let args = collect_args(&MountOptions {
            allow_other: true,
            ..MountOptions::default()
        });
        let ao_pos = args.iter().position(|a| a == "allow_other");
        assert!(ao_pos.is_some(), "allow_other must appear when requested");
        let idx = ao_pos.unwrap();
        assert_eq!(args[idx - 1], "-o", "allow_other must follow -o");
    }

    #[test]
    fn build_fuse_args_defer_permissions_always_present() {
        let args = collect_args(&MountOptions::default());
        assert!(
            args.contains(&"defer_permissions".to_string()),
            "defer_permissions must always be present"
        );
    }

    #[test]
    fn build_fuse_args_has_private_mount_identity() {
        let args = collect_args(&MountOptions::default());
        let position = args
            .iter()
            .position(|arg| arg == "fsname=pcloud-rs")
            .expect("private filesystem identity must be present");
        assert_eq!(args[position - 1], "-o");
    }

    #[test]
    fn build_fuse_args_volname_defaults_to_pcloud() {
        let args = collect_args(&MountOptions {
            fs_name: None,
            ..MountOptions::default()
        });
        assert!(
            args.iter().any(|a| a.starts_with("volname=")),
            "volname= must be present"
        );
        assert!(
            args.contains(&"volname=pCloud".to_string()),
            "default volname must be 'pCloud'"
        );
    }

    #[test]
    fn build_fuse_args_volname_uses_custom_fs_name() {
        let args = collect_args(&MountOptions {
            fs_name: Some("MyVolume".to_string()),
            ..MountOptions::default()
        });
        assert!(
            args.contains(&"volname=MyVolume".to_string()),
            "custom volname must be used when fs_name is set"
        );
    }

    #[test]
    fn build_fuse_args_every_option_preceded_by_dash_o() {
        let args = collect_args(&MountOptions {
            allow_other: true,
            fs_name: Some("Test".to_string()),
            read_only: false,
            ..MountOptions::default()
        });
        // Every option value (not -o itself, not argv[0]) must be preceded by -o
        let mut i = 1;
        while i < args.len() {
            if args[i] == "-o" {
                assert!(i + 1 < args.len(), "-o must be followed by an option value");
                i += 2;
            } else {
                panic!("unexpected top-level arg {:?} at position {i}", args[i]);
            }
        }
    }

    // -------------------------------------------------------------------------
    // path_to_cstring
    // -------------------------------------------------------------------------

    #[test]
    fn path_to_cstring_valid_path_succeeds() {
        let path = std::path::Path::new("/Volumes/pCloud");
        let result = path_to_cstring(path);
        assert!(result.is_ok(), "valid path must succeed");
        let cs = result.unwrap();
        assert_eq!(cs.to_str().unwrap(), "/Volumes/pCloud");
    }

    #[test]
    fn path_to_cstring_path_with_nul_byte_fails() {
        use std::os::unix::ffi::OsStrExt;
        let bad = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/mnt/bad\0path"));
        let result = path_to_cstring(&bad);
        assert!(result.is_err(), "path containing NUL must fail");
        match result.unwrap_err() {
            MountError::Unsupported(msg) => {
                assert!(msg.contains("NUL"), "error must mention NUL: {msg}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // MacFuseBackend::from_env
    // -------------------------------------------------------------------------

    #[test]
    fn macfuse_backend_from_env_default_when_unset_is_fuset() {
        // SAFETY: test-only env mutation; tests using this env var must
        // be run with --test-threads=1 if called concurrently.
        unsafe { std::env::remove_var("PCLOUD_MACOS_FUSE_BACKEND") };
        assert_eq!(MacFuseBackend::from_env(), MacFuseBackend::FuseT);
    }

    #[test]
    fn macfuse_backend_from_env_fuse_t_spellings() {
        for val in ["fuse-t", "fuset", "fuse_t", "FUSE-T", "FUSE_T"] {
            // SAFETY: test-only env mutation.
            unsafe { std::env::set_var("PCLOUD_MACOS_FUSE_BACKEND", val) };
            assert_eq!(
                MacFuseBackend::from_env(),
                MacFuseBackend::FuseT,
                "spelling '{val}' should map to FuseT"
            );
        }
        // SAFETY: test-only cleanup; no concurrent readers of this var.
        unsafe { std::env::remove_var("PCLOUD_MACOS_FUSE_BACKEND") };
    }

    #[test]
    fn macfuse_backend_from_env_macfuse_spellings() {
        for val in ["macfuse", "mac-fuse", "mac_fuse", "osxfuse", "MACFUSE"] {
            // SAFETY: test-only env mutation.
            unsafe { std::env::set_var("PCLOUD_MACOS_FUSE_BACKEND", val) };
            assert_eq!(
                MacFuseBackend::from_env(),
                MacFuseBackend::MacFuse,
                "spelling '{val}' should map to MacFuse"
            );
        }
        // SAFETY: test-only cleanup; no concurrent readers of this var.
        unsafe { std::env::remove_var("PCLOUD_MACOS_FUSE_BACKEND") };
    }

    #[test]
    fn macfuse_backend_from_env_auto() {
        // SAFETY: test-only env mutation.
        unsafe { std::env::set_var("PCLOUD_MACOS_FUSE_BACKEND", "auto") };
        assert_eq!(MacFuseBackend::from_env(), MacFuseBackend::Auto);
        // SAFETY: test-only cleanup.
        unsafe { std::env::remove_var("PCLOUD_MACOS_FUSE_BACKEND") };
    }

    #[test]
    fn macfuse_backend_from_env_unknown_value_falls_back_to_fuset() {
        // SAFETY: test-only env mutation.
        unsafe { std::env::set_var("PCLOUD_MACOS_FUSE_BACKEND", "doris") };
        assert_eq!(MacFuseBackend::from_env(), MacFuseBackend::FuseT);
        // SAFETY: test-only cleanup.
        unsafe { std::env::remove_var("PCLOUD_MACOS_FUSE_BACKEND") };
    }

    // -------------------------------------------------------------------------
    // MacFuseBackend::label
    // -------------------------------------------------------------------------

    #[test]
    fn macfuse_backend_labels_are_stable() {
        assert_eq!(MacFuseBackend::FuseT.label(), "fuse-t");
        assert_eq!(MacFuseBackend::MacFuse.label(), "macFUSE");
        assert_eq!(MacFuseBackend::Auto.label(), "auto");
    }

    // -------------------------------------------------------------------------
    // install_hint
    // -------------------------------------------------------------------------

    #[test]
    fn install_hint_fuset_mentions_url_and_macfuse_alternative() {
        let hint = install_hint(MacFuseBackend::FuseT);
        assert!(hint.contains("fuse-t"), "hint must mention fuse-t");
        assert!(hint.contains("fuse-t.org"), "hint must contain fuse-t URL");
        assert!(
            hint.contains("macfuse"),
            "hint must suggest macFUSE as alternative"
        );
    }

    #[test]
    fn install_hint_macfuse_mentions_url_and_fuset_alternative() {
        let hint = install_hint(MacFuseBackend::MacFuse);
        assert!(hint.contains("macFUSE"), "hint must mention macFUSE");
        assert!(
            hint.contains("macfuse.github.io"),
            "hint must contain macFUSE URL"
        );
        assert!(
            hint.contains("fuse-t"),
            "hint must suggest fuse-t as alternative"
        );
    }

    #[test]
    fn install_hint_auto_mentions_both_backends() {
        let hint = install_hint(MacFuseBackend::Auto);
        assert!(hint.contains("fuse-t"), "auto hint must mention fuse-t");
        assert!(hint.contains("macFUSE"), "auto hint must mention macFUSE");
    }

    // -------------------------------------------------------------------------
    // entry_attr_to_stat
    // -------------------------------------------------------------------------

    fn dir_attr(ino: u64) -> EntryAttr {
        EntryAttr {
            ino,
            kind: FsEntryKind::Directory,
            size: 0,
            mode: 0o755,
            uid: 501,
            gid: 20,
            mtime_epoch: None,
            mtime_nsec: 0,
        }
    }

    fn file_attr(ino: u64, size: u64) -> EntryAttr {
        EntryAttr {
            ino,
            kind: FsEntryKind::RegularFile,
            size,
            mode: 0o644,
            uid: 501,
            gid: 20,
            mtime_epoch: None,
            mtime_nsec: 0,
        }
    }

    #[test]
    fn entry_attr_to_stat_directory_sets_ifdir_bit() {
        let st = entry_attr_to_stat(&dir_attr(1));
        assert_eq!(
            st.st_mode & libc::S_IFMT,
            libc::S_IFDIR,
            "directory must have S_IFDIR mode type"
        );
    }

    #[test]
    fn entry_attr_to_stat_regular_file_sets_ifreg_bit() {
        let st = entry_attr_to_stat(&file_attr(2, 1024));
        assert_eq!(
            st.st_mode & libc::S_IFMT,
            libc::S_IFREG,
            "regular file must have S_IFREG mode type"
        );
    }

    #[test]
    fn entry_attr_to_stat_symlink_sets_iflnk_bit() {
        let attr = EntryAttr {
            kind: FsEntryKind::Symlink,
            ino: 3,
            size: 0,
            mode: 0o777,
            uid: 0,
            gid: 0,
            mtime_epoch: None,
            mtime_nsec: 0,
        };
        let st = entry_attr_to_stat(&attr);
        assert_eq!(
            st.st_mode & libc::S_IFMT,
            libc::S_IFLNK,
            "symlink must have S_IFLNK mode type"
        );
    }

    #[test]
    fn entry_attr_to_stat_ino_transferred() {
        let st = entry_attr_to_stat(&file_attr(42, 0));
        assert_eq!(st.st_ino, 42, "inode number must be transferred");
    }

    #[test]
    fn entry_attr_to_stat_uid_and_gid_transferred() {
        let st = entry_attr_to_stat(&file_attr(1, 0));
        assert_eq!(st.st_uid, 501);
        assert_eq!(st.st_gid, 20);
    }

    #[test]
    fn entry_attr_to_stat_size_transferred() {
        let st = entry_attr_to_stat(&file_attr(1, 4096));
        assert_eq!(st.st_size, 4096);
    }

    #[test]
    fn entry_attr_to_stat_mode_bits_preserved() {
        let attr = file_attr(1, 0); // mode 0o644
        let st = entry_attr_to_stat(&attr);
        // st_mode includes the type bits; mask to permission bits only
        assert_eq!(st.st_mode & 0o7777, 0o644);
    }

    #[test]
    fn entry_attr_to_stat_directory_has_nlink_2() {
        let st = entry_attr_to_stat(&dir_attr(1));
        assert_eq!(st.st_nlink, 2, "directories must report nlink=2");
    }

    #[test]
    fn entry_attr_to_stat_file_has_nlink_1() {
        let st = entry_attr_to_stat(&file_attr(2, 0));
        assert_eq!(st.st_nlink, 1, "regular files must report nlink=1");
    }

    #[test]
    fn entry_attr_to_stat_mtime_set_when_present() {
        let mut attr = file_attr(1, 0);
        attr.mtime_epoch = Some(1_700_000_000);
        let st = entry_attr_to_stat(&attr);
        assert_eq!(st.st_mtime, 1_700_000_000i64, "mtime must be set");
        assert_eq!(st.st_ctime, 1_700_000_000i64, "ctime must match mtime");
        assert_eq!(st.st_atime, 1_700_000_000i64, "atime must match mtime");
        assert_eq!(
            st.st_birthtime, 1_700_000_000i64,
            "birthtime must match mtime"
        );
    }

    #[test]
    fn entry_attr_to_stat_mtime_zero_when_absent() {
        let st = entry_attr_to_stat(&file_attr(1, 0));
        assert_eq!(st.st_mtime, 0, "mtime must be 0 when not provided");
    }

    #[test]
    fn entry_attr_to_stat_block_count_consistent_with_size() {
        // 4096 bytes -> ceiling(4096/512) = 8 blocks
        let st = entry_attr_to_stat(&file_attr(1, 4096));
        assert_eq!(st.st_blocks, 8, "block count must be size/512 rounded up");
        // 1 byte -> 1 block
        let st2 = entry_attr_to_stat(&file_attr(1, 1));
        assert_eq!(st2.st_blocks, 1);
    }

    #[test]
    fn entry_attr_to_stat_blksize_is_4096() {
        let st = entry_attr_to_stat(&file_attr(1, 0));
        assert_eq!(st.st_blksize, 4096, "block size must be 4096");
    }

    // -------------------------------------------------------------------------
    // entry_attr_to_param
    // -------------------------------------------------------------------------

    #[test]
    fn entry_attr_to_param_uses_attr_timeout() {
        let param = entry_attr_to_param(&file_attr(1, 0));
        assert_eq!(param.attr_timeout, ATTR_TIMEOUT_SECS);
    }

    #[test]
    fn entry_attr_to_param_uses_entry_timeout() {
        let param = entry_attr_to_param(&file_attr(1, 0));
        assert_eq!(param.entry_timeout, ENTRY_TIMEOUT_SECS);
    }

    #[test]
    fn entry_attr_to_param_ino_matches_attr() {
        let param = entry_attr_to_param(&file_attr(99, 0));
        assert_eq!(param.ino, 99);
    }

    #[test]
    fn entry_attr_to_param_generation_is_zero() {
        let param = entry_attr_to_param(&file_attr(1, 0));
        assert_eq!(
            param.generation, 0,
            "generation must be 0 (no inode versioning yet)"
        );
    }

    // -------------------------------------------------------------------------
    // MacosPlatformMount public API
    // -------------------------------------------------------------------------

    #[test]
    fn validate_mountpoint_returns_error_for_missing_path() {
        let mount = MacosPlatformMount;
        let result = mount.validate_mountpoint(std::path::Path::new("/nonexistent/path/xyz"));
        assert!(result.is_err());
        match result.unwrap_err() {
            MountError::MountpointMissing(_) => {}
            other => panic!("expected MountpointMissing, got {other:?}"),
        }
    }

    #[test]
    fn validate_mountpoint_returns_error_for_file_not_dir() {
        let file = tempfile::NamedTempFile::new().expect("tempfile");
        let mount = MacosPlatformMount;
        let result = mount.validate_mountpoint(file.path());
        assert!(result.is_err());
        match result.unwrap_err() {
            MountError::MountpointNotDirectory(_) => {}
            other => panic!("expected MountpointNotDirectory, got {other:?}"),
        }
    }

    #[test]
    fn validate_mountpoint_returns_ok_for_empty_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mount = MacosPlatformMount;
        assert!(
            mount.validate_mountpoint(dir.path()).is_ok(),
            "empty directory must be a valid mountpoint"
        );
    }

    #[test]
    fn default_options_fs_name_is_pcloud() {
        let mount = MacosPlatformMount;
        let opts = mount.default_options();
        assert_eq!(
            opts.fs_name.as_deref(),
            Some("pCloud"),
            "default fs_name must be 'pCloud'"
        );
    }

    #[test]
    fn default_options_allow_other_is_false() {
        let mount = MacosPlatformMount;
        let opts = mount.default_options();
        assert!(
            !opts.allow_other,
            "default options on macOS must NOT request allow_other \
             (security: enabling it would make the mount world-readable; \
             see fn default_options for rationale)"
        );
    }

    // -------------------------------------------------------------------------
    // MacosMountinfoReader shape
    // -------------------------------------------------------------------------

    #[test]
    fn macos_mountinfo_reader_read_returns_ok_or_is_empty() {
        let reader = MacosMountinfoReader;
        let result = reader.read();
        // On a live Mac this either returns Ok (possibly empty if no FUSE mounts)
        // or an error from getmntinfo. We only assert it doesn't panic.
        match result {
            Ok(payload) => {
                // Every line must end with \n if non-empty
                for line in payload.lines() {
                    assert!(
                        !line.trim().is_empty() || payload.is_empty(),
                        "non-empty payload lines must not be blank"
                    );
                }
            }
            Err(_) => {
                // Acceptable: getmntinfo failed (unusual but not panicking)
            }
        }
    }

    #[test]
    fn macos_mountinfo_reader_output_parses_via_parse_pcloud_mounts() {
        use crate::mount_orphan::parse_pcloud_mounts;
        let reader = MacosMountinfoReader;
        if let Ok(payload) = reader.read() {
            // Must not panic, must return well-formed entries.
            let entries = parse_pcloud_mounts(&payload);
            for entry in &entries {
                assert!(
                    !entry.fs_type.is_empty(),
                    "parsed entry must have non-empty fs_type"
                );
                assert!(
                    entry.mount_point.is_absolute(),
                    "mount point must be absolute: {:?}",
                    entry.mount_point
                );
            }
        }
    }

    // -------------------------------------------------------------------------
    // probe_supported behavior
    // -------------------------------------------------------------------------

    #[test]
    fn probe_supported_returns_ok_or_unsupported_with_hint() {
        let mount = MacosPlatformMount;
        match mount.probe_supported() {
            Ok(()) => {
                // fuse-t or macFUSE is installed — nothing more to assert.
            }
            Err(MountError::Unsupported(hint)) => {
                assert!(
                    !hint.is_empty(),
                    "unsupported error must carry a non-empty install hint"
                );
                // Hint must mention at least one URL.
                assert!(
                    hint.contains("fuse-t.org") || hint.contains("macfuse.github.io"),
                    "hint must contain an install URL: {hint}"
                );
            }
            Err(other) => {
                panic!("probe_supported must return Ok or Unsupported, got {other:?}");
            }
        }
    }

    // -------------------------------------------------------------------------
    // probe_with_dlopen
    // -------------------------------------------------------------------------

    #[test]
    fn probe_with_dlopen_returns_false_for_nonexistent_path() {
        assert!(
            !probe_with_dlopen("/nonexistent/libfuse.dylib"),
            "probing a nonexistent path must return false"
        );
    }

    #[test]
    fn probe_with_dlopen_returns_false_for_path_with_nul() {
        assert!(
            !probe_with_dlopen("/bad\0path"),
            "path with NUL must return false gracefully"
        );
    }

    // -------------------------------------------------------------------------
    // find_libfuse_install_path
    // -------------------------------------------------------------------------

    #[test]
    fn find_libfuse_install_path_auto_returns_same_or_subset_of_explicit() {
        // Auto should find at least as many candidates as explicit FuseT or
        // MacFuse alone (union, not intersection). If FuseT finds something,
        // Auto must find the same or more.
        let fuset = find_libfuse_install_path(MacFuseBackend::FuseT);
        let macfuse = find_libfuse_install_path(MacFuseBackend::MacFuse);
        let auto = find_libfuse_install_path(MacFuseBackend::Auto);

        if fuset.is_some() || macfuse.is_some() {
            assert!(
                auto.is_some(),
                "Auto must find something when FuseT or MacFuse succeeds individually"
            );
        }
    }

    // -------------------------------------------------------------------------
    // FUSET_CANDIDATES and MACFUSE_CANDIDATES constants
    // -------------------------------------------------------------------------

    #[test]
    fn fuset_candidates_are_absolute_dylib_paths() {
        for path in FUSET_CANDIDATES {
            assert!(
                path.starts_with('/'),
                "fuse-t candidate must be absolute: {path}"
            );
            assert!(
                path.ends_with(".dylib"),
                "fuse-t candidate must end with .dylib: {path}"
            );
        }
    }

    #[test]
    fn macfuse_candidates_are_absolute_dylib_paths() {
        for path in MACFUSE_CANDIDATES {
            assert!(
                path.starts_with('/'),
                "macFUSE candidate must be absolute: {path}"
            );
            assert!(
                path.ends_with(".dylib"),
                "macFUSE candidate must end with .dylib: {path}"
            );
        }
    }

    // -------------------------------------------------------------------------
    // ATTR_TIMEOUT_SECS / ENTRY_TIMEOUT_SECS sanity
    // -------------------------------------------------------------------------

    #[test]
    fn timeout_constants_are_positive() {
        assert!(
            ATTR_TIMEOUT_SECS > 0.0,
            "ATTR_TIMEOUT_SECS must be positive"
        );
        assert!(
            ENTRY_TIMEOUT_SECS > 0.0,
            "ENTRY_TIMEOUT_SECS must be positive"
        );
    }
}

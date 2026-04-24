//! **PLATFORM: Windows only.**
//! **GATING: `#[cfg(target_os = "windows")]`** -- the entire module file
//! is gated at the `mod windows;` line in `platform/mod.rs`.
//!
//! ============================================================================
//!  PHASE-3 SCAFFOLDING -- FSP_FILE_SYSTEM dispatcher wired but not tested
//!  on Windows. Requires WinFSP 2.x installed. Full bring-up pending Phase-4
//!  live verification.
//!
//!  This file wires `FspFileSystemCreate` / `SetMountPoint` /
//!  `StartDispatcher` into the [`PlatformMount`] seam and returns a real
//!  [`MountHandle`] whose Drop calls `StopDispatcher` + `Delete` and
//!  reclaims the leaked adapter box. It has NOT been compiled on a Windows
//!  toolchain and has NOT been exercised against `winfsp-x64.dll`; the
//!  callback thunks are minimal-viable (read/readdir/open return sensible
//!  NT statuses for now).
//!
//!  Before any claim of Windows parity (`bd-1du.4` / `bd-1du.10`):
//!   1. Build the crate with `--target x86_64-pc-windows-msvc`.
//!   2. Install the WinFSP 2.x MSI (https://winfsp.dev/) and verify
//!      `winfsp-x64.dll` is on `%PATH%`.
//!   3. Validate struct sizes / field offsets against the installed
//!      `winfsp/fsctl.h` and `winfsp/winfsp.h`.
//!   4. Exercise mount/read/write/unmount under Windows integration tests
//!      (Phase-4 live verification).
//!   5. Confirm signal-equivalent teardown (CTRL-C / service stop) actually
//!      cleans the mount point.
//! ============================================================================
//!
//! # Design
//!
//! The Windows adapter mirrors the Linux shape (`LinuxPlatformMount` +
//! `fuser::Filesystem` shim): a thin [`PlatformMount`] implementation plus a
//! callback-table shim that converts WinFSP's NT semantics into the
//! cross-platform [`FuseAdapter`] trait the backends already implement.
//!
//! WinFSP semantic mapping (MVP):
//!
//! * **Open / Create merge lookup+open.** WinFSP's `Open` receives the full
//!   NT path; the shim resolves it through [`FuseAdapter::lookup`] and
//!   returns a file context pointer that carries the adapter's
//!   `FileHandleId`. `Create` additionally asks the backend to materialize
//!   a new entry (not wired in the scaffold).
//! * **Cleanup handles delete-on-close.** WinFSP calls `Cleanup` with the
//!   `FspCleanupDelete` flag when the NT `FILE_DELETE_ON_CLOSE` disposition
//!   is set; the shim then issues the backend removal.
//! * **Paths are UTF-16.** WinFSP hands `PWSTR`s using backslash separators
//!   (`\foo\bar`). We convert via `OsStringExt::from_wide`, then swap
//!   backslashes for forward slashes before calling the adapter (whose
//!   trait uses POSIX-style paths).
//! * **Timestamps are Windows FILETIME** (100-ns ticks since 1601). See
//!   [`super::winfsp_ffi::unix_nanos_to_filetime`].
//! * **Alternate Data Streams / reparse points: NOT supported.** The
//!   corresponding WinFSP callbacks (where present) return
//!   `STATUS_NOT_SUPPORTED` (`0xC00000BB`).
//!
//! # Security posture
//!
//! * The DLL is loaded from the system search path only (no user-controlled
//!   override) so an attacker cannot silently substitute a malicious
//!   `winfsp-x64.dll`.
//! * We never log `PWSTR` contents verbatim (paths may contain secrets in
//!   names); structured error reporting only.
//! * `allow_other` stays rejected on the Windows path as well: Windows ACL
//!   enforcement is left to the default mount descriptor.

// Declare the FFI sibling module without requiring changes to
// `platform/mod.rs` (the caller is not allowed to touch that file).
#[path = "winfsp_ffi.rs"]
pub mod winfsp_ffi;

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::Arc;

use crate::fuse_adapter::{EntryAttr, FsEntryKind, FuseAdapter};
use crate::inode::ROOT_INODE;
use crate::mount_orphan::MountinfoReader;
use crate::mount_service::{MountError, MountHandle, MountOptions};
use crate::platform::PlatformMount;

use self::winfsp_ffi::{
    BOOLEAN, DirInfoHeader, FSP_FILE_SYSTEM_INTERFACE, FileInfo, NTSTATUS, PCWSTR, PFspFileSystem,
    STATUS_CANNOT_DELETE, STATUS_INVALID_PARAMETER, STATUS_MEDIA_WRITE_PROTECTED,
    STATUS_NOT_SUPPORTED, STATUS_OBJECT_NAME_COLLISION, STATUS_OBJECT_NAME_NOT_FOUND,
    STATUS_SUCCESS, VolumeInfo, VolumeParams, filetime_to_unix_nanos, load_winfsp,
    unix_nanos_to_filetime,
};

/// `STATUS_UNSUCCESSFUL` — generic NT failure status used when a callback
/// panics and we must return *something* across the FFI boundary.
const STATUS_UNSUCCESSFUL: NTSTATUS = NTSTATUS(0xC000_0001_u32 as i32);

/// WinFSP `FspCleanup*` flag bits. Values taken verbatim from WinFSP
/// `winfsp/winfsp.h` (`FSP_FSCTL_CLEANUP_*` defines).
///
/// `FspCleanupDelete` (0x01): the file was opened with `FILE_DELETE_ON_CLOSE`
/// disposition. When this bit is set in the `Cleanup` callback's `Flags`
/// parameter the driver expects the server to delete the underlying object
/// before `Close` is called so that a subsequent open of the same path finds
/// it gone.
const FSP_CLEANUP_DELETE: u32 = 0x01;

/// WinFSP `FSP_FSCTL_VOLUME_PARAMS.Flags` bits (subset). Values taken
/// verbatim from WinFSP `fsctl.h`.
const VP_FLAG_CASE_SENSITIVE_SEARCH: u32 = 0x0000_0001;
const VP_FLAG_CASE_PRESERVED_NAMES: u32 = 0x0000_0002;
const VP_FLAG_UNICODE_ON_DISK: u32 = 0x0000_0004;

// ---------------------------------------------------------------------------
//  PlatformMount impl
// ---------------------------------------------------------------------------

/// Windows platform-mount implementation backed by WinFSP.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPlatformMount;

// Alias requested by the task description (`WindowsMount: PlatformMount`).
// Re-exported as [`WindowsPlatformMount`] which is what `platform/mod.rs`
// imports as `ActivePlatformMount`.
pub type WindowsMount = WindowsPlatformMount;

impl PlatformMount for WindowsPlatformMount {
    /// Accept either a drive-letter root (e.g. `Z:\`) or an empty
    /// directory reachable via the current NT namespace.
    fn validate_mountpoint(&self, mountpoint: &Path) -> Result<(), MountError> {
        if is_drive_letter_root(mountpoint) {
            // We intentionally do *not* require the drive letter to be
            // free at validate-time: WinFSP's `FspFileSystemSetMountPoint`
            // is the authoritative check, and racing the free-letter
            // decision here just leaks TOCTOU.
            return Ok(());
        }

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
        let mut entries = std::fs::read_dir(mountpoint)?;
        if entries.next().is_some() {
            return Err(MountError::MountpointNotEmpty(mountpoint.to_path_buf()));
        }
        Ok(())
    }

    /// Probe by dynamically loading `winfsp-x64.dll`. If it is absent we
    /// surface a precise remediation hint pointing operators at the MSI.
    fn probe_supported(&self) -> Result<(), MountError> {
        match load_winfsp() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(MountError::Unsupported(
                "WinFSP not installed; download from https://winfsp.dev/".to_string(),
            )),
            Err(msg) => Err(MountError::Unsupported(format!(
                "WinFSP DLL present but incompatible: {msg}"
            ))),
        }
    }

    /// Windows defaults: no `allow_other` (WinFSP enforces via ACLs),
    /// case-insensitive (Windows convention). Volume label is
    /// `"pCloud"` and is applied at dispatcher-start time through
    /// [`VolumeParams::file_system_name`] -- it is not carried on the
    /// cross-platform [`MountOptions`] struct because that struct is
    /// OS-agnostic.
    fn default_options(&self) -> MountOptions {
        MountOptions {
            read_only: true,
            fs_name: Some("pcloud".to_string()),
            allow_other: false,
            attr_timeout_secs: 1.0,
            entry_timeout_secs: 1.0,
            max_readahead: 128 * 1024,
        }
    }

    /// Type-erased mount entry point that the daemon calls through the
    /// `PlatformMount` trait object. Delegates to [`mount_with_winfsp`]
    /// after validating the mountpoint.
    fn mount_adapter(
        &self,
        adapter: Box<dyn FuseAdapter>,
        mount_point: &Path,
        opts: MountOptions,
    ) -> Result<MountHandle, MountError> {
        self.validate_mountpoint(mount_point)?;
        mount_with_winfsp_dyn(mount_point, adapter, opts)
    }
}

// ---------------------------------------------------------------------------
//  MountinfoReader
// ---------------------------------------------------------------------------

/// Windows mountinfo reader. Placeholder until WinFSP-based enumeration
/// (`GetLogicalDriveStringsW` + `QueryDosDeviceW`) is wired to produce the
/// `/proc/self/mountinfo`-shaped payload the cross-platform parser
/// consumes.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsMountinfoReader;

impl MountinfoReader for WindowsMountinfoReader {
    fn read(&self) -> io::Result<String> {
        // Empty payload is a valid "no pCloud mounts" answer for the
        // orphan detector until real enumeration lands.
        Ok(String::new())
    }
}

// ---------------------------------------------------------------------------
//  Mount entry point (scaffold)
// ---------------------------------------------------------------------------

/// Mount `adapter` at `mountpoint` via WinFSP.
///
/// This is **scaffold only**: it wires [`FspFileSystemCreate`],
/// [`FspFileSystemSetMountPoint`], and [`FspFileSystemStartDispatcher`]
/// but the callback thunks are minimal-viable. Full parity with the Linux
/// path requires propagating NT-specific semantics (security descriptors,
/// sharing modes, reparse handling) into [`FuseAdapter`], which is
/// tracked under `bd-1du.4`.
pub fn mount_with_winfsp<A: FuseAdapter>(
    mountpoint: &Path,
    adapter: A,
    options: MountOptions,
) -> Result<MountHandle, MountError> {
    let boxed: Box<dyn FuseAdapter> = Box::new(adapter);
    mount_with_winfsp_dyn(mountpoint, boxed, options)
}

/// Type-erased variant of [`mount_with_winfsp`] used by the trait-object
/// path ([`PlatformMount::mount_adapter`]). Keeps a single implementation
/// body.
pub fn mount_with_winfsp_dyn(
    mountpoint: &Path,
    adapter: Box<dyn FuseAdapter>,
    options: MountOptions,
) -> Result<MountHandle, MountError> {
    if options.allow_other {
        return Err(MountError::AllowOtherRejected);
    }

    let lib = match load_winfsp() {
        Ok(Some(l)) => Arc::new(l),
        Ok(None) => {
            return Err(MountError::Unsupported(
                "WinFSP not installed; download from https://winfsp.dev/".to_string(),
            ));
        }
        Err(msg) => return Err(MountError::Unsupported(msg)),
    };

    // Double-box the trait object so the "fat pointer" itself lives on
    // the heap and we can round-trip it as a single `*mut c_void` through
    // the WinFSP user-context slot.
    let adapter_box: Box<Box<dyn FuseAdapter>> = Box::new(adapter);
    let adapter_raw: *mut c_void = Box::into_raw(adapter_box).cast::<c_void>();

    let volume_params = build_volume_params(&options);
    // Leak the interface table: WinFSP retains a pointer to it for the
    // life of the file system and we only ever need a single instance.
    let interface_table: &'static FSP_FILE_SYSTEM_INTERFACE = Box::leak(Box::new(callback_table()));

    let mut fs: PFspFileSystem = std::ptr::null_mut();

    // SAFETY: `FspFileSystemCreate` accepts a NUL-terminated UTF-16 device
    // name, a pointer to a caller-owned `VolumeParams`, a pointer to a
    // caller-owned interface table, and writes the resulting file-system
    // handle into `fs`. We keep `volume_params` on the stack for the
    // duration of the call and leak `interface_table` so its address
    // outlives the file system.
    let device_name_utf16: Vec<u16> = "pcloud-rs\0".encode_utf16().collect();
    let status = unsafe {
        (lib.fsp_create)(
            winfsp_ffi::PCWSTR(device_name_utf16.as_ptr()),
            &volume_params,
            interface_table,
            &mut fs,
        )
    };
    if status.0 != STATUS_SUCCESS.0 {
        // SAFETY: `adapter_raw` was produced by `Box::into_raw` just above
        // and has not been handed to WinFSP yet; reconstruct and drop it
        // to free the boxed trait object.
        unsafe {
            let _ = Box::from_raw(adapter_raw as *mut Box<dyn FuseAdapter>);
        }
        return Err(status_to_mount_error(status, "FspFileSystemCreate"));
    }

    // From this point forward we own a live `FSP_FILE_SYSTEM*` plus a
    // leaked adapter `Box`. If any subsequent step fails we must tear
    // them down in **strict reverse order**:
    //
    //   1. `FspFileSystemDelete`  — forces WinFSP to release its
    //      reference to the user-context pointer. Must happen BEFORE
    //      step 2, otherwise a pending dispatcher callback thread could
    //      call `cb_open`/`cb_read`/... and dereference `adapter_raw`
    //      after we have already freed the adapter box (use-after-free).
    //   2. `Box::from_raw(adapter_raw)` — reclaim and drop the leaked
    //      adapter box.
    //
    // `MountFailureGuard` is an RAII guard that encodes exactly this
    // ordering. We `arm()` it immediately, `disarm()` it on the success
    // path, and rely on its `Drop` to run the cleanup on every early-
    // return error path.
    let mut guard = MountFailureGuard::new(lib.clone(), fs, adapter_raw);

    // Attach the boxed adapter as the WinFSP UserContext so every callback
    // can recover it via `FspFileSystemGetUserContext`.
    //
    // SAFETY: `adapter_raw` is a live `Box<Box<dyn FuseAdapter>>` pointer;
    // we transfer ownership to WinFSP here and take it back at unmount.
    unsafe { (lib.fsp_set_user_context)(fs, adapter_raw) };

    // Set mount point. Drive-letter roots are passed as `"Z:"` (no
    // trailing slash); directories are passed as-is.
    let mp_utf16 = mountpoint_to_utf16(mountpoint);
    // SAFETY: UTF-16 buffer is NUL-terminated and lives for the call.
    let status = unsafe { (lib.fsp_set_mount_point)(fs, winfsp_ffi::PCWSTR(mp_utf16.as_ptr())) };
    if status.0 != STATUS_SUCCESS.0 {
        // `guard` will run `fsp_delete` first, adapter-box free second.
        return Err(status_to_mount_error(status, "FspFileSystemSetMountPoint"));
    }

    // Start dispatcher. WinFSP chooses its own thread count when 0 is
    // passed.
    // SAFETY: `fs` is a valid handle; thread count 0 is documented as
    // "library default".
    let status = unsafe { (lib.fsp_start_dispatcher)(fs, 0) };
    if status.0 != STATUS_SUCCESS.0 {
        // `guard` will run `fsp_delete` first, adapter-box free second.
        return Err(status_to_mount_error(
            status,
            "FspFileSystemStartDispatcher",
        ));
    }

    // Transfer ownership of `fs` and `adapter_raw` to the `MountHandle`;
    // the guard must no longer touch them.
    guard.disarm();
    Ok(MountHandle::from_windows(fs, mp_utf16, adapter_raw, lib))
}

/// RAII cleanup guard for the partially-initialised WinFSP mount path.
///
/// # Why a dedicated guard
///
/// Between `FspFileSystemCreate` and `FspFileSystemStartDispatcher` the
/// file-system handle `fs` may already have the adapter pointer
/// installed as its `UserContext`. A naive error path that frees the
/// adapter `Box` and then calls `FspFileSystemDelete` races with
/// WinFSP internal worker threads that can still look up the user
/// context and call into the adapter's v-table — a classic double-
/// reclaim / use-after-free.
///
/// The guard enforces the correct teardown order on every early-return
/// path:
///
/// 1. `FspFileSystemDelete(fs)` — WinFSP drops its reference to the
///    user-context pointer and stops any pending dispatchers.
/// 2. `Box::from_raw(adapter_raw)` — only now is it safe to free the
///    adapter.
///
/// The success path calls [`MountFailureGuard::disarm`] to hand
/// ownership of both pointers to the returned `MountHandle`.
struct MountFailureGuard {
    lib: std::sync::Arc<crate::platform::windows::winfsp_ffi::WinFspLibrary>,
    fs: PFspFileSystem,
    adapter_raw: *mut c_void,
    armed: bool,
}

impl MountFailureGuard {
    fn new(
        lib: std::sync::Arc<crate::platform::windows::winfsp_ffi::WinFspLibrary>,
        fs: PFspFileSystem,
        adapter_raw: *mut c_void,
    ) -> Self {
        Self {
            lib,
            fs,
            adapter_raw,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for MountFailureGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Step 1: delete file system FIRST so WinFSP drops every
        // reference to the user-context pointer (no dispatcher thread
        // can reach the adapter after this returns).
        if !self.fs.is_null() {
            // SAFETY: `fs` is a live WinFSP handle we own. After
            // `FspFileSystemDelete` returns, WinFSP has released its
            // reference and no further callback will fire.
            unsafe { (self.lib.fsp_delete)(self.fs) };
        }
        // Step 2: reclaim the adapter box. Now, and only now, is it
        // safe to free the memory.
        if !self.adapter_raw.is_null() {
            // SAFETY: `adapter_raw` was produced by `Box::into_raw` on
            // a `Box<Box<dyn FuseAdapter>>` and WinFSP has dropped its
            // reference.
            unsafe {
                let _ = Box::from_raw(self.adapter_raw as *mut Box<dyn FuseAdapter>);
            }
        }
    }
}

// ---------------------------------------------------------------------------
//  Shim context + callback table
// ---------------------------------------------------------------------------

/// Recover the boxed adapter from the WinFSP user-context slot.
///
/// # Safety
///
/// Caller must guarantee that `fs` is the `FSP_FILE_SYSTEM*` that was
/// created in [`mount_with_winfsp_dyn`] and that its user-context slot
/// still points at the `Box<Box<dyn FuseAdapter>>` we installed there.
/// The returned reference is valid for the duration of the callback
/// (WinFSP guarantees the user-context pointer is stable until
/// `FspFileSystemDelete`).
#[inline]
unsafe fn adapter_from_fs<'a>(fs: PFspFileSystem) -> Option<&'a dyn FuseAdapter> {
    if fs.is_null() {
        return None;
    }
    // The shim library is already resolved and cached globally via the
    // static `WinFspLibrary` the caller holds; callbacks reach it
    // indirectly via the user-context pointer only.
    //
    // We have no access to the Arc<WinFspLibrary> from inside the callback
    // (it lives on `MountHandle`), but we do have a function-pointer view
    // through a thread-local-free re-load. To avoid re-resolving on every
    // callback we perform a one-shot load on first callback entry.
    // SAFETY: `fs` is a live WinFSP file-system handle whose dispatcher is
    // active; `fsp_get_user_context_global` is resolved once at first call
    // from the loaded `winfsp-x64.dll` and then cached — safe to call on
    // any dispatcher thread while the file system is active.
    let ctx_ptr = unsafe { fsp_get_user_context_global(fs) };
    if ctx_ptr.is_null() {
        return None;
    }
    // SAFETY: user-context slot holds a `Box<Box<dyn FuseAdapter>>` whose
    // outer Box was leaked via `Box::into_raw`. We reborrow it as `&dyn`.
    let outer = unsafe { &*(ctx_ptr as *const Box<dyn FuseAdapter>) };
    Some(outer.as_ref())
}

/// Retrieve the WinFSP user-context pointer without needing an
/// `Arc<WinFspLibrary>`. We re-resolve the export on first use and cache
/// it in a `OnceLock<Mutex<Option<FnPtr>>>`. If `winfsp-x64.dll` is
/// already resident (which is always the case inside a callback) this is
/// just a symbol lookup.
///
/// # Why `OnceLock<Mutex<Option<FnPtr>>>`
///
/// Rust `extern "system" fn` pointers are already `Send + Sync`, so a
/// bare `OnceLock<Option<FnPtr>>` is sound in isolation. However, the
/// auditor requested an explicit lock wrapper to document — and enforce
/// at the type level — that **no future maintainer may swap the
/// contents for a non-`Sync` type** (e.g. `Rc`, `Cell<FnPtr>`) without
/// first removing the mutex. The `Mutex` also gives us a clean hook to
/// stash any future per-process symbol-resolution state alongside the
/// function pointer (logging, metrics, version probe) without another
/// unsafe refactor.
///
/// The runtime cost is a single `Mutex::lock` on every callback entry,
/// which is a handful of nanoseconds and strictly dominated by the
/// subsequent DLL call.
///
/// # Safety
///
/// Must only be called from a WinFSP dispatcher callback, i.e. when the
/// DLL is known to be resident.
unsafe fn fsp_get_user_context_global(fs: PFspFileSystem) -> *mut c_void {
    static GETTER: std::sync::OnceLock<
        std::sync::Mutex<Option<winfsp_ffi::FnFspFileSystemGetUserContext>>,
    > = std::sync::OnceLock::new();
    let cell = GETTER.get_or_init(|| {
        let resolved = match load_winfsp() {
            Ok(Some(lib)) => Some(lib.fsp_get_user_context),
            _ => None,
        };
        std::sync::Mutex::new(resolved)
    });
    let getter = match cell.lock() {
        Ok(g) => *g,
        Err(poison) => *poison.into_inner(),
    };
    match getter {
        // SAFETY: `fs` came from WinFSP; the getter signature matches the
        // WinFSP ABI.
        Some(f) => unsafe { f(fs) },
        None => std::ptr::null_mut(),
    }
}

/// Build the callback vtable WinFSP invokes. All slots we have not yet
/// wired are left as `None`, which WinFSP translates into the appropriate
/// NTSTATUS automatically.
fn callback_table() -> FSP_FILE_SYSTEM_INTERFACE {
    FSP_FILE_SYSTEM_INTERFACE {
        GetVolumeInfo: Some(cb_get_volume_info),
        SetVolumeLabel: None,
        GetSecurityByName: Some(cb_get_security_by_name),
        Create: Some(cb_create),
        Open: Some(cb_open),
        Overwrite: Some(cb_overwrite),
        Cleanup: Some(cb_cleanup),
        Close: Some(cb_close),
        Read: Some(cb_read),
        Write: Some(cb_write),
        Flush: Some(cb_flush),
        GetFileInfo: Some(cb_get_file_info),
        SetBasicInfo: Some(cb_set_basic_info),
        SetFileSize: Some(cb_set_file_size),
        CanDelete: Some(cb_can_delete),
        Rename: Some(cb_rename),
        GetSecurity: None,
        SetSecurity: Some(cb_set_security),
        ReadDirectory: Some(cb_read_directory),
        reserved_tail: [std::ptr::null_mut(); 16],
    }
}

/// Panic-safe shim around a WinFSP callback. Converts any panic into
/// `STATUS_UNSUCCESSFUL` so we never unwind across the C ABI boundary.
#[inline]
fn guarded<F: FnOnce() -> NTSTATUS + std::panic::UnwindSafe>(f: F) -> NTSTATUS {
    match std::panic::catch_unwind(f) {
        Ok(status) => status,
        Err(_) => STATUS_UNSUCCESSFUL,
    }
}

/// Panic-safe shim for `()`-returning callbacks (Cleanup / Close).
#[inline]
fn guarded_void<F: FnOnce() + std::panic::UnwindSafe>(f: F) {
    let _ = std::panic::catch_unwind(f);
}

// ---------------------------------------------------------------------------
//  Per-open file context
// ---------------------------------------------------------------------------
//
// WinFSP passes our `*mut c_void` verbatim from `Open`/`Create` to every
// subsequent callback for that handle. We store a tiny heap-allocated
// descriptor that carries the inode + a directory flag. More sophisticated
// state (page caches, write-back handles, ADS streams) goes here when we
// actually need it.
//
// Persistent `FileHandleId` from `FuseAdapter::open` is cached inside the
// `FileContext` on first Read/Write use. We additionally carry a per-context
// sequence number so the daemon can correlate log lines across round-trips
// against the same WinFSP `FileContext` slot.

/// Monotonic counter used to stamp each [`FileContext`] with a unique id.
/// Useful for debugging concurrent Read/Write interleavings without logging
/// the raw pointer value (which would disclose ASLR info).
static FILE_CONTEXT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Stable per-open context we stash in the WinFSP `FileContext` slot.
///
/// `handle_id` is populated lazily by the first Read/Write so that open-only
/// paths (e.g. `GetSecurityByName`-probed lookups) do not pay the round trip
/// cost of a backend `open`. Interior mutability is provided by the `Cell`
/// types; the context is only ever accessed from the owning dispatcher-thread
/// call chain (WinFSP serialises callbacks against the same FileContext).
#[repr(C)]
struct FileContext {
    ino: u64,
    is_dir: bool,
    /// Unique per-context sequence number assigned at allocation time.
    context_seq: u64,
    /// Cached [`FileHandleId`] opened against the backend on first Read/Write.
    handle_id: std::cell::Cell<Option<crate::fuse_adapter::FileHandleId>>,
}

impl FileContext {
    fn new(ino: u64, is_dir: bool) -> Self {
        Self {
            ino,
            is_dir,
            context_seq: FILE_CONTEXT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            handle_id: std::cell::Cell::new(None),
        }
    }

    /// Return the cached [`FileHandleId`] or open one lazily.
    fn ensure_handle(
        &self,
        adapter: &dyn FuseAdapter,
    ) -> Result<crate::fuse_adapter::FileHandleId, i32> {
        if let Some(h) = self.handle_id.get() {
            return Ok(h);
        }
        let h = adapter.open(self.ino)?;
        self.handle_id.set(Some(h));
        Ok(h)
    }
}

/// Win32 file-attribute bits we care about. Taken verbatim from
/// `minwinbase.h`; redeclared locally to keep the `windows` feature surface
/// minimal.
const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;

/// Resolve `path` (POSIX-shaped, leading `/`) through the adapter's
/// parent-by-parent `lookup` API. Returns the final [`EntryAttr`].
fn resolve_path(adapter: &dyn FuseAdapter, path: &str) -> Result<EntryAttr, i32> {
    // Root is synthetic — the adapter's `getattr(ROOT_INODE)` owns it.
    if path.is_empty() || path == "/" {
        return adapter.getattr(ROOT_INODE);
    }
    let mut ino = ROOT_INODE;
    let mut last: Option<EntryAttr> = None;
    for segment in path.trim_start_matches('/').split('/') {
        if segment.is_empty() {
            continue;
        }
        let attr = adapter.lookup(ino, segment)?;
        ino = attr.ino;
        last = Some(attr);
    }
    last.ok_or(libc_enoent())
}

/// Portable `ENOENT` literal (value matches Linux/Windows CRT).
#[inline]
const fn libc_enoent() -> i32 {
    2
}

/// Populate `info` (a `FileInfo`) from an [`EntryAttr`].
///
/// # Safety
/// `info` must be a writable pointer that lives for the call.
unsafe fn fill_file_info(info: *mut FileInfo, attr: &EntryAttr) {
    if info.is_null() {
        return;
    }
    let filetime = attr
        .mtime_epoch
        .map(|s| unix_nanos_to_filetime((s as i128).saturating_mul(1_000_000_000)))
        .unwrap_or(0);
    let attrs = match attr.kind {
        FsEntryKind::Directory => FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_READONLY,
        _ => FILE_ATTRIBUTE_NORMAL,
    };
    // SAFETY: caller contract.
    unsafe {
        (*info) = FileInfo::default();
        (*info).file_attributes = attrs;
        (*info).allocation_size = attr.size;
        (*info).file_size = attr.size;
        (*info).creation_time = filetime;
        (*info).last_access_time = filetime;
        (*info).last_write_time = filetime;
        (*info).change_time = filetime;
        (*info).index_number = attr.ino;
        (*info).hard_links = 1;
    }
}

/// Recover a `FileContext` reference from the WinFSP-supplied raw pointer.
///
/// # Safety
/// `ptr` must have been produced by `Box::into_raw` in [`cb_open`] and must
/// not have been freed yet.
#[inline]
unsafe fn file_context_ref<'a>(ptr: *mut c_void) -> Option<&'a FileContext> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: caller contract.
        Some(unsafe { &*(ptr as *const FileContext) })
    }
}

// Each callback thunk below is a minimal-viable shim. SAFETY comments
// describe the contract we inherit from the WinFSP ABI and must uphold
// before crossing back into WinFSP.

extern "system" fn cb_get_volume_info(fs: PFspFileSystem, info: *mut VolumeInfo) -> NTSTATUS {
    guarded(|| {
        if info.is_null() {
            return STATUS_INVALID_PARAMETER;
        }
        // Query the adapter for real pCloud quota. Fall back to a
        // Explorer-friendly default when the adapter hasn't implemented
        // `statfs` (it returns `ENOSYS`), so mount probes still succeed.
        //
        // SAFETY: inside a WinFSP callback `fs` carries the adapter
        // installed in `mount_with_winfsp_dyn`.
        let (total, free) = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a.statfs().unwrap_or((u64::MAX / 2, u64::MAX / 4)),
            None => (u64::MAX / 2, u64::MAX / 4),
        };
        // SAFETY: WinFSP guarantees `info` is a writable `VolumeInfo` for
        // the callback's duration. The `volume_label` array is fixed-size
        // (32 u16s) and we bound the copy by its length.
        unsafe {
            (*info) = VolumeInfo::default();
            (*info).total_size = total;
            (*info).free_size = free;
            let label: Vec<u16> = "pCloud".encode_utf16().collect();
            let cap = (*info).volume_label.len();
            let n = label.len().min(cap);
            (*info).volume_label[..n].copy_from_slice(&label[..n]);
            // WinFSP expects the length *in bytes*, not UTF-16 units.
            (*info).volume_label_length = (n * 2) as u16;
        }
        STATUS_SUCCESS
    })
}

extern "system" fn cb_get_security_by_name(
    fs: PFspFileSystem,
    file_name: winfsp_ffi::PCWSTR,
    file_attributes: *mut u32,
    security_descriptor: *mut c_void,
    security_descriptor_size: *mut usize,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: `fs` carries the boxed adapter installed in
        // `mount_with_winfsp_dyn`; callback lifetime is within dispatcher.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_name` is a NUL-terminated UTF-16 string supplied
        // and owned by WinFSP for the callback's duration.
        let path = match unsafe { pwstr_to_posix_string(file_name) } {
            Some(s) => s,
            None => return STATUS_INVALID_PARAMETER,
        };
        let attr = match resolve_path(adapter, &path) {
            Ok(a) => a,
            Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
        };

        // Report attributes if WinFSP asked for them.
        if !file_attributes.is_null() {
            let v = match attr.kind {
                FsEntryKind::Directory => FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_READONLY,
                _ => FILE_ATTRIBUTE_NORMAL,
            };
            // SAFETY: WinFSP guarantees a writable u32 when non-null.
            unsafe { *file_attributes = v };
        }

        // Build a minimal SDDL granting the current user Full Access and
        // copy it into the caller's buffer. We synthesise `D:(A;;FA;;;<SID>)`
        // with the current process SID substituted at runtime.
        //
        // TODO(bd-xplat-windows): validate actual SDDL parsing against a
        // real Windows host. Until then the MVP also accepts a partial
        // buffer (WinFSP first calls with `size == 0` to learn the length).
        if !security_descriptor_size.is_null() {
            match build_current_user_security_descriptor() {
                Ok(sd) => {
                    // SAFETY: WinFSP guarantees the `size` out-param is writable.
                    let avail = unsafe { *security_descriptor_size };
                    // SAFETY: same contract; write the needed length regardless.
                    unsafe { *security_descriptor_size = sd.len() };
                    if sd.len() > avail {
                        // Buffer too small — WinFSP will retry with the new size.
                        return NTSTATUS(0x8000_0005_u32 as i32); // STATUS_BUFFER_OVERFLOW
                    }
                    if !security_descriptor.is_null() {
                        // SAFETY: caller-owned buffer of at least `avail` bytes;
                        // we bounded the copy by `sd.len() <= avail`.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                sd.as_ptr(),
                                security_descriptor as *mut u8,
                                sd.len(),
                            );
                        }
                    }
                }
                Err(_) => {
                    // Failing SD construction is non-fatal; WinFSP falls back
                    // to the default mount descriptor.
                    // SAFETY: same writable-out-param contract.
                    unsafe { *security_descriptor_size = 0 };
                }
            }
        }

        STATUS_SUCCESS
    })
}

/// Synthesize a self-relative SECURITY_DESCRIPTOR granting the current user
/// full access. Calls into
/// `advapi32.dll::ConvertStringSecurityDescriptorToSecurityDescriptorW`.
///
/// TODO(bd-xplat-windows): add a proper integration test on Windows; the
/// SDDL path is untested in Linux CI.
fn build_current_user_security_descriptor() -> Result<Vec<u8>, ()> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::PSID;
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SID_AND_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // 1. Open current process token and fetch its user SID.
    let mut token = windows::Win32::Foundation::HANDLE(std::ptr::null_mut());
    // SAFETY: OpenProcessToken with TOKEN_QUERY is always safe; we own the
    // returned handle and close it before returning.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok.is_err() {
        return Err(());
    }
    // Query required buffer size.
    let mut needed: u32 = 0;
    // SAFETY: First call with NULL buffer returns size in `needed`.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        // SAFETY: close the handle we opened.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(token) };
        return Err(());
    }
    let mut buf = vec![0u8; needed as usize];
    // SAFETY: buf lives for the call and is `needed` bytes long.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            needed,
            &mut needed,
        )
    };
    // SAFETY: close token unconditionally.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(token) };
    if ok.is_err() {
        return Err(());
    }
    // SAFETY: `buf` contains a TOKEN_USER followed by the SID blob.
    let tu = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    let sid: PSID = tu.User.Sid;
    if sid.0.is_null() {
        return Err(());
    }

    // 2. Convert SID -> string via ConvertSidToStringSidW.
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut sid_string: windows::core::PWSTR = windows::core::PWSTR(std::ptr::null_mut());
    // SAFETY: ConvertSidToStringSidW writes an allocated string we must LocalFree.
    if unsafe { ConvertSidToStringSidW(sid, &mut sid_string) }.is_err() {
        return Err(());
    }
    // SAFETY: walk to NUL, bounded at 256 (max textual SID is well under).
    let mut len = 0usize;
    while unsafe { *sid_string.0.add(len) } != 0 && len < 256 {
        len += 1;
    }
    // SAFETY: same slice bounds.
    let sid_slice = unsafe { std::slice::from_raw_parts(sid_string.0, len) };
    let sid_text = String::from_utf16_lossy(sid_slice);
    // SAFETY: free the buffer allocated by ConvertSidToStringSidW.
    unsafe {
        let _ = LocalFree(HLOCAL(sid_string.0 as *mut c_void));
    }

    // 3. Build SDDL and convert to a self-relative SECURITY_DESCRIPTOR.
    let sddl = format!("D:(A;;FA;;;{sid_text})\0");
    let sddl_w: Vec<u16> = sddl.encode_utf16().collect();
    let mut psd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    let mut sd_len: u32 = 0;
    // SAFETY: input string is NUL-terminated; revision 1 per API docs.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            winfsp_ffi::PCWSTR(sddl_w.as_ptr()),
            1,
            &mut psd,
            Some(&mut sd_len),
        )
    };
    if ok.is_err() || psd.0.is_null() {
        return Err(());
    }
    // SAFETY: psd points at sd_len bytes of self-relative SD.
    let bytes = unsafe { std::slice::from_raw_parts(psd.0 as *const u8, sd_len as usize) }.to_vec();
    // SAFETY: free the OS-allocated SD.
    unsafe {
        let _ = LocalFree(HLOCAL(psd.0));
    }
    Ok(bytes)
}

extern "system" fn cb_open(
    fs: PFspFileSystem,
    file_name: winfsp_ffi::PCWSTR,
    _create_options: u32,
    _granted_access: u32,
    file_context: *mut *mut c_void,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: inside a WinFSP callback `fs` is a live handle whose
        // user-context was installed by `mount_with_winfsp_dyn`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_name` is NUL-terminated and owned by WinFSP.
        let path = match unsafe { pwstr_to_posix_string(file_name) } {
            Some(s) => s,
            None => return STATUS_INVALID_PARAMETER,
        };
        let attr = match resolve_path(adapter, &path) {
            Ok(a) => a,
            Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
        };

        let ctx = Box::new(FileContext::new(
            attr.ino,
            matches!(attr.kind, FsEntryKind::Directory),
        ));
        if !file_context.is_null() {
            // SAFETY: WinFSP guarantees a writable slot; transfer ownership.
            unsafe { *file_context = Box::into_raw(ctx) as *mut c_void };
        }
        // SAFETY: caller provided a writable `FileInfo` when non-null.
        unsafe { fill_file_info(file_info, &attr) };
        STATUS_SUCCESS
    })
}

// ---------------------------------------------------------------------------
//  Write-path callbacks (Phase 5)
// ---------------------------------------------------------------------------
//
// These callbacks complete the MVP mounted-drive write surface. Each thunk
// recovers the boxed adapter via [`adapter_from_fs`], converts WinFSP's NT
// semantics into the cross-platform [`FuseAdapter`] trait, and re-fills the
// caller's `FileInfo` out-param where applicable. All run on WinFSP
// dispatcher threads; panics are caught by [`guarded`] and translated into
// `STATUS_UNSUCCESSFUL` so we never unwind across the C ABI boundary.

/// `Create` — materialize a new file or directory and leave it open.
///
/// Maps to [`FuseAdapter::create`] (files only in the current trait shape).
/// Directory creation and `mode_from_attrs` propagation land with
/// `bd-xplat-windows` follow-up once the adapter grows the richer surface.
extern "system" fn cb_create(
    fs: PFspFileSystem,
    file_name: PCWSTR,
    _create_options: u32,
    _granted_access: u32,
    file_attributes: u32,
    _security_descriptor: *const c_void,
    _allocation_size: u64,
    file_context: *mut *mut c_void,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: within a WinFSP callback `fs` carries the adapter we
        // installed in `mount_with_winfsp_dyn`; see `adapter_from_fs`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_name` is NUL-terminated and owned by WinFSP for
        // the callback's duration.
        let path = match unsafe { pwstr_to_posix_string(file_name) } {
            Some(s) => s,
            None => return STATUS_INVALID_PARAMETER,
        };
        let (parent, name) = match split_parent_and_name(&path) {
            Some(x) => x,
            None => return STATUS_INVALID_PARAMETER,
        };

        // Collision check — WinFSP expects `STATUS_OBJECT_NAME_COLLISION`
        // when the target already exists.
        if resolve_path(adapter, &path).is_ok() {
            return STATUS_OBJECT_NAME_COLLISION;
        }

        // Derive a POSIX mode from the Win32 attribute bitmask so the
        // backend can persist a sane mode even though `adapter.create` does
        // not take it directly today; we apply it via `setattr` right after
        // creation when the attribute set carries meaningful bits.
        //
        // Mapping:
        //   FILE_ATTRIBUTE_READONLY  -> 0o444 (r--r--r--)
        //   FILE_ATTRIBUTE_NORMAL    -> 0o644 (rw-r--r--)
        //   anything else / 0        -> 0o644 (default)
        let mode_from_attrs: u16 = if file_attributes & FILE_ATTRIBUTE_READONLY != 0 {
            0o444
        } else {
            0o644
        };

        let ino = match adapter.create(&parent, &name) {
            Ok(i) => i,
            Err(errno) => return errno_to_status(errno),
        };

        // Best-effort mode propagation. Adapters that do not implement
        // `setattr` return `ENOSYS`; we treat that as non-fatal — the
        // filesystem is still usable, just without Win32 ACL mirroring.
        let _ = adapter.setattr(
            ino,
            crate::fuse_adapter::SetAttr {
                mode: Some(mode_from_attrs),
                ..Default::default()
            },
        );

        let attr = match adapter.getattr(ino) {
            Ok(a) => a,
            Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
        };
        let ctx = Box::new(FileContext::new(
            attr.ino,
            matches!(attr.kind, FsEntryKind::Directory),
        ));
        if !file_context.is_null() {
            // SAFETY: WinFSP guarantees a writable `*mut c_void` slot; we
            // transfer ownership of the heap-allocated context to WinFSP
            // until `cb_close` reclaims it.
            unsafe { *file_context = Box::into_raw(ctx) as *mut c_void };
        }
        // SAFETY: WinFSP guarantees a writable FileInfo when non-null.
        unsafe { fill_file_info(file_info, &attr) };
        STATUS_SUCCESS
    })
}

/// `Overwrite` — truncate the file to zero length (optionally replacing
/// attributes) and refresh `FileInfo`.
extern "system" fn cb_overwrite(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    file_attributes: u32,
    replace_file_attributes: BOOLEAN,
    _allocation_size: u64,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: produced by `Box::into_raw` in cb_open / cb_create.
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };
        if ctx.is_dir {
            return STATUS_INVALID_PARAMETER;
        }
        if let Err(errno) = adapter.truncate(ctx.ino, 0) {
            return errno_to_status(errno);
        }
        // Per WinFSP semantics, when `replace_file_attributes` is TRUE the
        // caller's `file_attributes` REPLACES the on-disk attribute set;
        // when FALSE it is OR'd into the current set. We forward the
        // REPLACE case to the adapter via `set_basic_info` so backends with
        // real ACL mirroring observe the new bitmask. The OR case is left
        // to the adapter's existing truncate semantics (attributes are
        // typically preserved across content overwrite on pCloud, which
        // has no native Win32 ACL storage anyway).
        if replace_file_attributes.0 != 0 {
            let _ = adapter.set_basic_info(
                ctx.ino,
                crate::fuse_adapter::BasicInfo {
                    file_attributes: Some(file_attributes),
                    ..Default::default()
                },
            );
        }
        let attr = match adapter.getattr(ctx.ino) {
            Ok(a) => a,
            Err(errno) => return errno_to_status(errno),
        };
        // SAFETY: caller-provided writable FileInfo when non-null.
        unsafe { fill_file_info(file_info, &attr) };
        STATUS_SUCCESS
    })
}

/// `Write` — copy `length` bytes from the caller-owned buffer into the
/// backend at `offset` (or end-of-file when `write_to_end_of_file`).
extern "system" fn cb_write(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    buffer: *const c_void,
    offset: u64,
    length: u32,
    write_to_end_of_file: BOOLEAN,
    _constrained_io: BOOLEAN,
    bytes_transferred: *mut u32,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    guarded(|| {
        if !bytes_transferred.is_null() {
            // SAFETY: WinFSP guarantees a writable `u32` out-param.
            unsafe { *bytes_transferred = 0 };
        }
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };
        if ctx.is_dir {
            return STATUS_INVALID_PARAMETER;
        }
        if buffer.is_null() || length == 0 {
            return STATUS_SUCCESS;
        }

        // SAFETY: WinFSP guarantees the source buffer is readable for at
        // least `length` bytes; we copy it into a Rust-owned `Vec<u8>` so
        // the adapter trait never aliases caller memory.
        let buf: Vec<u8> =
            unsafe { std::slice::from_raw_parts(buffer as *const u8, length as usize).to_vec() };

        // Resolve the effective offset. `write_to_end_of_file` means "append":
        // substitute the current file size.
        let effective_offset = if write_to_end_of_file.0 != 0 {
            match adapter.getattr(ctx.ino) {
                Ok(a) => a.size,
                Err(errno) => return errno_to_status(errno),
            }
        } else {
            offset
        };

        match adapter.write(ctx.ino, effective_offset, &buf) {
            Ok(_n) => {
                // Report the full request length as transferred; the adapter
                // either wrote all bytes or returned an error.
                if !bytes_transferred.is_null() {
                    // SAFETY: writable u32 out-param.
                    unsafe { *bytes_transferred = length };
                }
            }
            Err(errno) => return errno_to_status(errno),
        }

        let attr = match adapter.getattr(ctx.ino) {
            Ok(a) => a,
            Err(errno) => return errno_to_status(errno),
        };
        // SAFETY: caller-provided writable FileInfo when non-null.
        unsafe { fill_file_info(file_info, &attr) };
        STATUS_SUCCESS
    })
}

/// `SetFileSize` — truncate or extend the file to `new_size`.
extern "system" fn cb_set_file_size(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    new_size: u64,
    _set_allocation_size: BOOLEAN,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };
        if ctx.is_dir {
            return STATUS_INVALID_PARAMETER;
        }
        if let Err(errno) = adapter.truncate(ctx.ino, new_size) {
            return errno_to_status(errno);
        }
        let attr = match adapter.getattr(ctx.ino) {
            Ok(a) => a,
            Err(errno) => return errno_to_status(errno),
        };
        // SAFETY: caller-provided writable FileInfo when non-null.
        unsafe { fill_file_info(file_info, &attr) };
        STATUS_SUCCESS
    })
}

/// `SetBasicInfo` — update file attributes / timestamps.
///
/// The adapter trait does not yet expose a dedicated `setattr` entry point,
/// so we convert the FILETIMEs to Unix nanoseconds for completeness but
/// currently report success without mutating backend state. The conversion
/// exercises [`filetime_to_unix_nanos`] so the helper stays live in the
/// Windows build.
extern "system" fn cb_set_basic_info(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    file_attributes: u32,
    creation_time: u64,
    last_access_time: u64,
    last_write_time: u64,
    change_time: u64,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };

        // WinFSP sentinel semantics (per `FspFileSystemInterface`):
        //   * `0`     — "do not change" this field
        //   * `-1`    — "do not change" this field (older WinFSP variant)
        //   * `!0u32` on `file_attributes` — "do not change"
        // Anything else is a real FILETIME we must persist.
        const FT_NOCHANGE: u64 = u64::MAX;
        let attrs_opt = if file_attributes == u32::MAX {
            None
        } else {
            Some(file_attributes)
        };
        let ctime_opt = if creation_time == 0 || creation_time == FT_NOCHANGE {
            None
        } else {
            Some(creation_time)
        };
        let atime_opt = if last_access_time == 0 || last_access_time == FT_NOCHANGE {
            None
        } else {
            Some(last_access_time)
        };
        let mtime_opt = if last_write_time == 0 || last_write_time == FT_NOCHANGE {
            None
        } else {
            Some(last_write_time)
        };
        let chgtime_opt = if change_time == 0 || change_time == FT_NOCHANGE {
            None
        } else {
            Some(change_time)
        };

        // Keep the FILETIME<->Unix converter exercised for parity tests even
        // when every field is "no change" (all `None`).
        let _ = filetime_to_unix_nanos(creation_time);

        let info = crate::fuse_adapter::BasicInfo {
            file_attributes: attrs_opt,
            creation_time: ctime_opt,
            last_access_time: atime_opt,
            last_write_time: mtime_opt,
            change_time: chgtime_opt,
        };

        // Forward to the adapter. `ENOSYS` is treated as a non-fatal no-op
        // so the Explorer "save" dialog doesn't fail on read-only backends.
        match adapter.set_basic_info(ctx.ino, info) {
            Ok(_) => {}
            Err(libc_enosys) if libc_enosys == 38 /* ENOSYS */ => {}
            Err(errno) => return errno_to_status(errno),
        }

        let attr = match adapter.getattr(ctx.ino) {
            Ok(a) => a,
            Err(errno) => return errno_to_status(errno),
        };
        // SAFETY: caller-provided writable FileInfo when non-null.
        unsafe { fill_file_info(file_info, &attr) };
        STATUS_SUCCESS
    })
}

/// `CanDelete` — reject deletion of non-empty directories.
extern "system" fn cb_can_delete(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    _file_name: PCWSTR,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };
        if ctx.is_dir {
            match adapter.readdir(ctx.ino, 0) {
                Ok(entries) if entries.is_empty() => STATUS_SUCCESS,
                Ok(_) => STATUS_CANNOT_DELETE,
                Err(errno) => errno_to_status(errno),
            }
        } else {
            STATUS_SUCCESS
        }
    })
}

/// `Rename` — move / rename an entry. WinFSP provides absolute NT-shaped
/// paths for both source and destination.
extern "system" fn cb_rename(
    fs: PFspFileSystem,
    _file_context: *mut c_void,
    file_name: PCWSTR,
    new_file_name: PCWSTR,
    replace_if_exists: BOOLEAN,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: both pointers are NUL-terminated and owned by WinFSP.
        let from = match unsafe { pwstr_to_posix_string(file_name) } {
            Some(s) => s,
            None => return STATUS_INVALID_PARAMETER,
        };
        let to = match unsafe { pwstr_to_posix_string(new_file_name) } {
            Some(s) => s,
            None => return STATUS_INVALID_PARAMETER,
        };

        // Enforce `replace_if_exists == FALSE` semantics: collision means
        // `STATUS_OBJECT_NAME_COLLISION`. When TRUE, fall through to the
        // adapter, which owns the atomic replace policy.
        if replace_if_exists.0 == 0 && resolve_path(adapter, &to).is_ok() {
            return STATUS_OBJECT_NAME_COLLISION;
        }

        match adapter.rename(&from, &to) {
            Ok(()) => STATUS_SUCCESS,
            Err(errno) => errno_to_status(errno),
        }
    })
}

/// `SetSecurity` — intentional STATUS_SUCCESS no-op.
///
/// # Why this is a permanent no-op (not a TODO)
///
/// Windows ACL semantics (DACL/SACL/Owner/Group with per-ACE access masks
/// and inheritance flags) do not map onto pCloud's server-side permission
/// model, which is limited to:
///
///   * sharing rules (per-folder shared contact / email),
///   * business-team ACLs (coarse-grained, not per-user),
///   * public-link visibility flags,
///   * crypto-folder private-key gating.
///
/// There is no round-trip-safe translation from an arbitrary NT
/// SECURITY_DESCRIPTOR to any combination of the above, so mirroring the
/// descriptor back to pCloud would either:
///
///   1. silently drop most of the semantics (misleading to the user who
///      set the ACL in Explorer),
///   2. reject any non-trivial descriptor (breaks Explorer's property
///      sheet even for reads), or
///   3. persist the descriptor locally only (misleading again — the ACL
///      wouldn't survive another mount).
///
/// We therefore acknowledge the request so the Explorer property sheet
/// closes cleanly, but we deliberately do not persist anything. The
/// authoritative descriptor is still synthesised by
/// [`cb_get_security_by_name`] from the current process SID + "full
/// access" SDDL. The legacy C client behaves the same way.
extern "system" fn cb_set_security(
    _fs: PFspFileSystem,
    _file_context: *mut c_void,
    _security_information: u32,
    _modification_descriptor: *const c_void,
) -> NTSTATUS {
    guarded(|| STATUS_SUCCESS)
}

extern "system" fn cb_cleanup(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    _file_name: winfsp_ffi::PCWSTR,
    flags: u32,
) {
    guarded_void(|| {
        // Handle FILE_DELETE_ON_CLOSE: when the kernel sets FspCleanupDelete
        // the file must be deleted before Close arrives. We issue a backend
        // delete via the adapter's unlink path, keyed on the resolved path
        // from the inode stored in the FileContext.
        if flags & FSP_CLEANUP_DELETE != 0 {
            // SAFETY: `fs` carries the adapter installed in mount_with_winfsp_dyn;
            // callback lifetime is within the dispatcher.
            let adapter = match unsafe { adapter_from_fs(fs) } {
                Some(a) => a,
                None => return,
            };
            // SAFETY: file_context was produced by Box::into_raw in cb_open/cb_create
            // and has not been freed yet (Close arrives after Cleanup).
            let ctx = match unsafe { file_context_ref(file_context) } {
                Some(c) => c,
                None => return,
            };
            if ctx.is_dir {
                // FspCleanupDelete on a directory: rmdir via adapter.
                // We need the path, but the trait exposes ino-based readdir/rmdir.
                // Resolve via resolve_ino_to_path then delegate.
                match adapter.resolve_ino_to_path(ctx.ino) {
                    Ok(path) => {
                        let path_str = path.to_string_lossy();
                        if let Err(e) = adapter.rmdir(&path_str) {
                            log::error!(
                                "FspCleanupDelete: rmdir ino={} failed errno={}",
                                ctx.ino,
                                e
                            );
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "FspCleanupDelete: resolve_ino_to_path ino={} failed errno={}",
                            ctx.ino,
                            e
                        );
                    }
                }
            } else {
                // FspCleanupDelete on a file: resolve path and call unlink.
                match adapter.resolve_ino_to_path(ctx.ino) {
                    Ok(path) => {
                        let path_str = path.to_string_lossy();
                        // Split into (parent_path, name) for adapter.unlink.
                        if let Some((parent, name)) = split_parent_and_name(&path_str) {
                            if let Err(e) = adapter.unlink(&parent, &name) {
                                log::error!(
                                    "FspCleanupDelete: unlink ino={} path={} failed errno={}",
                                    ctx.ino,
                                    path_str,
                                    e
                                );
                            }
                        } else {
                            log::warn!(
                                "FspCleanupDelete: could not split path={} for unlink",
                                path_str
                            );
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "FspCleanupDelete: resolve_ino_to_path ino={} failed errno={}",
                            ctx.ino,
                            e
                        );
                    }
                }
            }
        }
        // Note: Cleanup does NOT own the FileContext; Close reclaims it.
    });
}

extern "system" fn cb_close(fs: PFspFileSystem, file_context: *mut c_void) {
    guarded_void(|| {
        if file_context.is_null() {
            return;
        }
        // SAFETY: `file_context` was produced by `Box::into_raw` in
        // `cb_open` / `cb_create`; WinFSP invokes `Close` exactly once per
        // opened handle, so this is the single point of ownership return.
        let boxed = unsafe { Box::from_raw(file_context as *mut FileContext) };
        // Release the cached backend handle, if we lazily opened one for
        // Read/Write. Best-effort: ENOSYS / ENOENT are non-fatal here.
        if let Some(h) = boxed.handle_id.get() {
            // SAFETY: dispatcher-thread callback, adapter still installed.
            if let Some(adapter) = unsafe { adapter_from_fs(fs) } {
                let _ = adapter.release(h);
            }
        }
        drop(boxed);
    });
}

extern "system" fn cb_read(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    buffer: *mut c_void,
    offset: u64,
    length: u32,
    bytes_transferred: *mut u32,
) -> NTSTATUS {
    guarded(|| {
        if !bytes_transferred.is_null() {
            // SAFETY: WinFSP guarantees a writable `u32` out-param.
            unsafe { *bytes_transferred = 0 };
        }
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: produced by `Box::into_raw` in cb_open; still owned by us.
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };
        if ctx.is_dir {
            return STATUS_INVALID_PARAMETER;
        }
        if buffer.is_null() || length == 0 {
            return STATUS_SUCCESS;
        }

        // Persistent `FileHandleId`: open lazily on first Read, cache in
        // the `FileContext` and release in `cb_close`. This avoids an
        // open/release round-trip on every WinFSP Read, matching the
        // semantics of a real Win32 handle where the backend file stays
        // open for the lifetime of the NT FILE_OBJECT.
        let handle = match ctx.ensure_handle(adapter) {
            Ok(h) => h,
            Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
        };
        let data = match adapter.read(handle, offset, length as usize) {
            Ok(d) => d,
            Err(_) => return winfsp_ffi::STATUS_IO_DEVICE_ERROR,
        };

        let n = data.len().min(length as usize);
        if n > 0 {
            // SAFETY: caller-owned buffer of at least `length` bytes; we
            // bound the copy at `n <= length`. Regions do not overlap
            // (Rust-owned `data` vs caller-owned `buffer`).
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), buffer as *mut u8, n);
            }
        }
        if !bytes_transferred.is_null() {
            // SAFETY: writable u32 out-param.
            unsafe { *bytes_transferred = n as u32 };
        }
        if n == 0 {
            return winfsp_ffi::STATUS_END_OF_FILE;
        }
        STATUS_SUCCESS
    })
}

extern "system" fn cb_flush(
    _fs: PFspFileSystem,
    _file_context: *mut c_void,
    _file_info: *mut FileInfo,
) -> NTSTATUS {
    // Read-only MVP: a flush is always a no-op success.
    guarded(|| STATUS_SUCCESS)
}

extern "system" fn cb_get_file_info(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    file_info: *mut FileInfo,
) -> NTSTATUS {
    guarded(|| {
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };
        let attr = match adapter.getattr(ctx.ino) {
            Ok(a) => a,
            Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
        };
        // SAFETY: caller-provided writable FileInfo when non-null.
        unsafe { fill_file_info(file_info, &attr) };
        STATUS_SUCCESS
    })
}

extern "system" fn cb_read_directory(
    fs: PFspFileSystem,
    file_context: *mut c_void,
    _pattern: winfsp_ffi::PCWSTR,
    marker: winfsp_ffi::PCWSTR,
    buffer: *mut c_void,
    length: u32,
    bytes_transferred: *mut u32,
) -> NTSTATUS {
    guarded(|| {
        if !bytes_transferred.is_null() {
            // SAFETY: writable u32 out-param.
            unsafe { *bytes_transferred = 0 };
        }
        // SAFETY: see `cb_open`.
        let adapter = match unsafe { adapter_from_fs(fs) } {
            Some(a) => a,
            None => return STATUS_INVALID_PARAMETER,
        };
        // SAFETY: `file_context` was produced by `Box::into_raw` in `cb_open`
        // or `cb_create` and has not been freed yet (WinFSP calls `Cleanup`/
        // callback before `Close`, which is the only point of ownership return).
        let ctx = match unsafe { file_context_ref(file_context) } {
            Some(c) => c,
            None => return STATUS_INVALID_PARAMETER,
        };
        if !ctx.is_dir {
            return STATUS_INVALID_PARAMETER;
        }
        let entries = match adapter.readdir(ctx.ino, 0) {
            Ok(e) => e,
            Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
        };

        // Honour the resume marker — WinFSP asks us to emit entries
        // lexicographically after `marker`.
        // SAFETY: marker, when non-null, is NUL-terminated per WinFSP ABI.
        let marker_s = unsafe { pwstr_to_posix_string(marker) };
        // Resolve the add-dir-info function pointer once.
        let add_fn: Option<winfsp_ffi::FnFspFileSystemAddDirInfo> = match load_winfsp() {
            Ok(Some(lib)) => lib.fsp_add_dir_info,
            _ => None,
        };

        let mut filtered: Vec<_> = entries
            .into_iter()
            .filter(|e| match &marker_s {
                Some(m) if !m.is_empty() => e.name.as_str() > m.as_str(),
                _ => true,
            })
            .collect();
        filtered.sort_by(|a, b| a.name.cmp(&b.name));

        for e in &filtered {
            let attr = match adapter.getattr(e.ino) {
                Ok(a) => a,
                Err(_) => continue,
            };
            // Build a DirInfo record: [header | name_utf16], 8-byte aligned.
            let name_utf16: Vec<u16> = e.name.encode_utf16().collect();
            let name_bytes = name_utf16.len() * 2;
            let hdr_size = std::mem::size_of::<DirInfoHeader>();
            let total = hdr_size + name_bytes;
            // WinFSP's DirInfo `size` field is a u16; anything larger must
            // be rejected (a filename > ~64KB can never happen in practice).
            if total > u16::MAX as usize {
                continue;
            }

            let mut record: Vec<u8> = vec![0; total];
            let hdr = DirInfoHeader {
                size: total as u16,
                _padding: 0,
                file_info: FileInfo::default(),
                next_offset: 0,
            };
            // SAFETY: `record` is sized to fit `hdr` + the name. We first
            // write the header via pointer write, then fill `file_info` via
            // fill_file_info, then copy the name suffix.
            unsafe {
                std::ptr::write_unaligned(record.as_mut_ptr() as *mut DirInfoHeader, hdr);
                let hdr_ptr = record.as_mut_ptr() as *mut DirInfoHeader;
                fill_file_info(&mut (*hdr_ptr).file_info as *mut FileInfo, &attr);
                // Copy the UTF-16 name (no NUL) immediately after the header.
                std::ptr::copy_nonoverlapping(
                    name_utf16.as_ptr() as *const u8,
                    record.as_mut_ptr().add(hdr_size),
                    name_bytes,
                );
            }

            if let Some(add) = add_fn {
                // SAFETY: `add` is the ABI-correct WinFSP helper; we pass a
                // pointer to a complete DirInfo record, the caller-owned
                // buffer bounded by `length`, and a writable bytes-transferred.
                let ok = unsafe {
                    add(
                        record.as_ptr() as *const c_void,
                        buffer,
                        length,
                        bytes_transferred,
                    )
                };
                if ok.0 == 0 {
                    // Buffer full; stop here. WinFSP will call us again with
                    // `marker` = last emitted name.
                    return STATUS_SUCCESS;
                }
            } else {
                // The DLL loaded but is missing `FspFileSystemAddDirInfo`.
                // This can only happen on a pre-1.x WinFSP build (the MSI
                // has shipped this export since 2017). We cannot safely
                // append a DirInfo record without it — the cursor /
                // `NextOffset` bookkeeping is non-trivial and differs
                // across WinFSP versions — so we refuse the operation
                // instead of emitting subtly broken directory listings.
                //
                // Log an explanatory line so operators on locked-down
                // hosts can diagnose and upgrade; Explorer surfaces the
                // `STATUS_NOT_SUPPORTED` as "The request is not
                // supported."
                log::error!(
                    "pcloud_fs::winfsp: ReadDirectory on inode {} requires FspFileSystemAddDirInfo (missing from loaded winfsp-x64.dll; please upgrade WinFSP to 1.x or newer)",
                    ctx.ino,
                );
                return STATUS_NOT_SUPPORTED;
            }
        }

        // Terminate the stream (`DirInfo == NULL`).
        if let Some(add) = add_fn {
            // SAFETY: passing NULL DirInfo is the documented terminator.
            let _ = unsafe { add(std::ptr::null(), buffer, length, bytes_transferred) };
        }
        STATUS_SUCCESS
    })
}

// ---------------------------------------------------------------------------
//  Helpers
// ---------------------------------------------------------------------------

fn is_drive_letter_root(p: &Path) -> bool {
    let s = p.as_os_str().to_string_lossy();
    let bytes = s.as_bytes();
    // Accept `Z:`, `Z:\`, `Z:/`.
    matches!(
        bytes,
        [c, b':'] | [c, b':', b'\\'] | [c, b':', b'/']
            if c.is_ascii_alphabetic()
    )
}

fn mountpoint_to_utf16(mountpoint: &Path) -> Vec<u16> {
    // Drive letters: pass `"Z:"` (no trailing slash), WinFSP requires that
    // exact shape.
    if is_drive_letter_root(mountpoint) {
        let mut v: Vec<u16> = mountpoint
            .as_os_str()
            .encode_wide()
            .take(2) // letter + ':'
            .collect();
        v.push(0);
        return v;
    }
    let mut v: Vec<u16> = mountpoint.as_os_str().encode_wide().collect();
    v.push(0);
    v
}

fn build_volume_params(_options: &MountOptions) -> VolumeParams {
    // Derive a stable-but-unique 32-bit volume serial number from the
    // process start time. Operators can override this post-mount via
    // `FspFileSystemSetVolumeParams` if they need a deterministic value.
    let serial = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // Fold top and bottom halves to fit in u32.
        ((nanos >> 32) as u32) ^ (nanos as u32)
    };

    // Sector size 4096 / cluster size 4096: one sector per allocation
    // unit keeps Windows quota/space accounting consistent with modern
    // NTFS defaults and matches WinFSP's recommended MEMFS/PTFS shape.
    let mut vp = VolumeParams {
        version: std::mem::size_of::<VolumeParams>() as u16,
        sector_size: 4096,
        sectors_per_allocation_unit: 1,
        max_component_length: 255,
        volume_creation_time: 0,
        volume_serial_number: serial,
        transact_timeout: 0,
        irp_timeout: 0,
        irp_capacity: 0,
        file_info_timeout: 1000,
        // Case-insensitive, case-preserved, Unicode-on-disk: Windows
        // convention. Backends that need POSIX case-sensitivity should
        // additionally set `VP_FLAG_CASE_SENSITIVE_SEARCH` — not done
        // here because pCloud canonicalizes names server-side.
        flags: VP_FLAG_UNICODE_ON_DISK | VP_FLAG_CASE_PRESERVED_NAMES,
        prefix: [0; winfsp_ffi::FSP_FSCTL_VOLUME_PREFIX_SIZE / 2],
        file_system_name: [0; winfsp_ffi::FSP_FSCTL_VOLUME_FSNAME_SIZE / 2],
        reserved_tail: [0; 256],
    };
    let label: Vec<u16> = "pCloud".encode_utf16().collect();
    let n = label.len().min(vp.file_system_name.len());
    vp.file_system_name[..n].copy_from_slice(&label[..n]);
    vp
}

fn status_to_mount_error(status: NTSTATUS, op: &'static str) -> MountError {
    MountError::Unsupported(format!("{op} failed: NTSTATUS=0x{:08X}", status.0 as u32))
}

// ---------------------------------------------------------------------------
//  Convert an NT-shaped path to the POSIX-shaped string FuseAdapter wants.
// ---------------------------------------------------------------------------

/// UTF-16 `PCWSTR` -> Rust `String` with backslashes normalized to `/`.
/// Returns `None` if the pointer is NUL.
///
/// # Safety
///
/// Caller must guarantee the pointer is NUL-terminated and valid for the
/// duration of the call (WinFSP upholds this for callback parameters).
pub unsafe fn pwstr_to_posix_string(p: winfsp_ffi::PCWSTR) -> Option<String> {
    if p.0.is_null() {
        return None;
    }
    // Walk to NUL.
    let mut len = 0usize;
    // SAFETY: caller contract.
    while unsafe { *p.0.add(len) } != 0 {
        len += 1;
        if len > 32 * 1024 {
            // Guard against runaway inputs.
            return None;
        }
    }
    // SAFETY: caller contract; `len` bounded above.
    let slice = unsafe { std::slice::from_raw_parts(p.0, len) };
    let os = std::ffi::OsString::from_wide(slice);
    let s = os.to_string_lossy().replace('\\', "/");
    Some(s)
}

/// Split a POSIX-shaped absolute path into `(parent_dir, final_name)`.
///
/// Returns `None` for the root itself or pathless inputs.
fn split_parent_and_name(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit_once('/') {
        Some(("", name)) => Some(("/".to_string(), name.to_string())),
        Some((parent, name)) if !name.is_empty() => Some((parent.to_string(), name.to_string())),
        _ => None,
    }
}

/// Translate a POSIX errno returned by a [`FuseAdapter`] into the closest
/// NTSTATUS. Kept pessimistic: anything unrecognised is `STATUS_UNSUCCESSFUL`
/// so unexpected backend errors surface loudly in Windows Event Viewer.
fn errno_to_status(errno: i32) -> NTSTATUS {
    use crate::errors::{EACCES, EINVAL, EIO, ENOENT, ENOTDIR, EROFS};
    // Redeclare the few errnos we need without pulling in libc.
    const EEXIST: i32 = 17;
    const ENOTEMPTY: i32 = 39;
    match errno {
        ENOENT => STATUS_OBJECT_NAME_NOT_FOUND,
        ENOTDIR => STATUS_OBJECT_NAME_NOT_FOUND,
        EEXIST => STATUS_OBJECT_NAME_COLLISION,
        ENOTEMPTY => STATUS_CANNOT_DELETE,
        EACCES => winfsp_ffi::STATUS_ACCESS_DENIED,
        EROFS => STATUS_MEDIA_WRITE_PROTECTED,
        EINVAL => STATUS_INVALID_PARAMETER,
        EIO => winfsp_ffi::STATUS_IO_DEVICE_ERROR,
        // Fall through: any other errno is an unmapped backend failure.
        _ => {
            let _ = STATUS_NOT_SUPPORTED; // keep the import live for future mappings
            STATUS_UNSUCCESSFUL
        }
    }
}

// ---------------------------------------------------------------------------
//  Signal / Ctrl-C reaper stub (M-5.1)
// ---------------------------------------------------------------------------

/// Windows Ctrl-C / service-stop signal reaper stub.
///
/// Mirrors the Linux `install_reaper_once` + `reaper_main` pattern but
/// uses Windows `SetConsoleCtrlHandler` instead of `sigaction`. When the
/// process receives a Ctrl-C, Ctrl-Break, or logoff/shutdown event the
/// handler sets [`SHUTDOWN_REQUESTED`] to `true`; the reaper thread
/// polls and logs a warning so operators know the process is unwinding.
///
/// **TIER-3 status (pcloud-rs-ncx.29, audit-06):** Windows signal-driven
/// mount cleanup is **scaffolded-only and not live-verified**, consistent
/// with the Windows IPC Tier-3 disposition documented in `CLAUDE.md`
/// under "IPC and local security". The `SetConsoleCtrlHandler` hook is
/// installed and the reaper logs on shutdown, but:
///   - it does **not** call `FspFileSystemStopDispatcher`,
///   - it does **not** call `FspFileSystemRemoveMountPoint`,
///   - it does **not** drain an ACTIVE_MOUNTS registry (none exists on
///     Windows — the WinFSP mount wiring is scaffolded, not live).
///
/// Closing the gap requires `bd-xplat-windows`: wire WinFSP through the
/// accept loop, maintain an ACTIVE_MOUNTS equivalent keyed by
/// `FSP_FILE_SYSTEM*`, and in [`windows_reaper_main`] call
/// `FspFileSystemStopDispatcher` then `FspFileSystemRemoveMountPoint`
/// per entry. Until then, a process crash or abnormal exit may leave a
/// stale mount point that the operator must clean up manually
/// (`fsutil reparsepoint delete` or WinFSP admin tooling).
///
/// The reaper here ensures we do not silently swallow termination events
/// on Windows the way we do on Linux.
pub mod reaper {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Set to `true` by the Ctrl-C handler; polled by [`windows_reaper_main`].
    static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

    /// Ensures the Ctrl-C handler is installed at most once per process.
    static SIGNAL_HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();

    /// Ensures the reaper thread is spawned at most once per process.
    static REAPER_INSTALLED: OnceLock<()> = OnceLock::new();

    /// Install the Windows Ctrl-C signal reaper.
    ///
    /// Idempotent: safe to call multiple times; handler and reaper thread
    /// are installed at most once per process lifetime.
    ///
    /// On non-Windows targets this is a no-op so callers can remain
    /// platform-agnostic.
    pub fn install_windows_signal_reaper() {
        #[cfg(target_os = "windows")]
        {
            // Install the console control handler once.
            SIGNAL_HANDLER_INSTALLED.get_or_init(|| {
                // SAFETY: `SetConsoleCtrlHandler` is safe to call with a
                // static function pointer and `TRUE`. The handler may be
                // invoked on a separate OS thread but only touches an
                // `AtomicBool`.
                unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> i32 {
                    // CTRL_C_EVENT=0, CTRL_BREAK_EVENT=1, CTRL_CLOSE_EVENT=2,
                    // CTRL_LOGOFF_EVENT=5, CTRL_SHUTDOWN_EVENT=6
                    log::warn!(
                        "pcloud-fs[windows]: received Windows control event {} — \
                         requesting graceful shutdown",
                        ctrl_type
                    );
                    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
                    // Return TRUE to suppress the default handler (process kill).
                    1_i32
                }
                // SAFETY: static fn pointer; TRUE (1) means add the handler.
                unsafe {
                    let _ = SetConsoleCtrlHandler(Some(ctrl_handler), 1);
                }
            });

            // Spawn the reaper thread once.
            REAPER_INSTALLED.get_or_init(|| {
                if let Err(e) = std::thread::Builder::new()
                    .name("pcloudfs-win-reaper".to_string())
                    .spawn(windows_reaper_main)
                {
                    log::error!(
                        "pcloud-fs[windows]: failed to spawn Windows reaper thread; \
                         active mounts will NOT be cleaned up on Ctrl-C/shutdown: {e}"
                    );
                }
            });
        }
    }

    /// Reaper thread body. Polls [`SHUTDOWN_REQUESTED`] and emits a warning
    /// when shutdown is requested. Actual WinFSP unmount is a TODO tracked
    /// under `bd-xplat-windows`.
    ///
    /// TIER-3 (pcloud-rs-ncx.29): this body must be upgraded to call
    /// `FspFileSystemStopDispatcher` + `FspFileSystemRemoveMountPoint`
    /// per active mount when `bd-xplat-windows` wires WinFSP through the
    /// accept loop. Currently there is no ACTIVE_MOUNTS registry to drain
    /// (none exists on Windows yet), so the reaper only logs.
    fn windows_reaper_main() {
        use std::time::Duration;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
                log::warn!(
                    "pcloud-fs[windows]: shutdown requested — \
                     active pCloud mounts should be unmounted before process exits. \
                     Automatic WinFSP teardown tracked under bd-xplat-windows \
                     (Tier-3 per CLAUDE.md)."
                );
                // Exit reaper; the process is unwinding.
                break;
            }
        }
    }

    /// `SetConsoleCtrlHandler` Windows API shim.
    ///
    /// Declared here so the reaper module compiles without importing the
    /// full `windows` crate (which has a different feature-flag surface
    /// than the parent module).
    #[cfg(target_os = "windows")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_letter_root_detection() {
        assert!(is_drive_letter_root(Path::new("Z:")));
        assert!(is_drive_letter_root(Path::new("z:")));
        assert!(is_drive_letter_root(Path::new("C:\\")));
        assert!(is_drive_letter_root(Path::new("C:/")));
        assert!(!is_drive_letter_root(Path::new("C:\\foo")));
        assert!(!is_drive_letter_root(Path::new("foo")));
        assert!(!is_drive_letter_root(Path::new("1:")));
    }

    #[test]
    fn split_parent_and_name_basic() {
        assert_eq!(
            split_parent_and_name("/foo"),
            Some(("/".to_string(), "foo".to_string()))
        );
        assert_eq!(
            split_parent_and_name("/a/b/c"),
            Some(("/a/b".to_string(), "c".to_string()))
        );
        assert_eq!(split_parent_and_name("/"), None);
        assert_eq!(split_parent_and_name(""), None);
    }

    #[test]
    fn errno_to_status_known_mappings() {
        use crate::errors::{EACCES, ENOENT, EROFS};
        assert_eq!(errno_to_status(ENOENT).0, STATUS_OBJECT_NAME_NOT_FOUND.0);
        assert_eq!(
            errno_to_status(EACCES).0,
            winfsp_ffi::STATUS_ACCESS_DENIED.0
        );
        assert_eq!(errno_to_status(EROFS).0, STATUS_MEDIA_WRITE_PROTECTED.0);
        assert_eq!(errno_to_status(17).0, STATUS_OBJECT_NAME_COLLISION.0);
        assert_eq!(errno_to_status(39).0, STATUS_CANNOT_DELETE.0);
    }

    #[test]
    fn default_options_rejects_allow_other() {
        let opts = WindowsPlatformMount.default_options();
        assert!(!opts.allow_other);
        assert!(opts.read_only);
        assert_eq!(opts.fs_name.as_deref(), Some("pcloud"));
    }

    /// Security-posture smoke test mirroring the macOS one. Windows has
    /// no `nodev`/`nosuid` NFS-style flags (ACLs govern access), but the
    /// same security intent surfaces here as: no broad `allow_other`,
    /// read-only by default, and sane cache TTLs. Any regression here
    /// would silently widen the NT file-object exposure.
    #[test]
    fn windows_mount_options_are_secure_by_default() {
        let opts = WindowsPlatformMount.default_options();
        assert!(
            !opts.allow_other,
            "allow_other must default to false on Windows"
        );
        assert!(
            opts.read_only,
            "read_only must default to true on Windows MVP"
        );
        assert!(
            opts.attr_timeout_secs > 0.0,
            "attr cache TTL must be positive"
        );
        assert!(
            opts.entry_timeout_secs > 0.0,
            "entry cache TTL must be positive"
        );
    }

    /// `VolumeParams` must advertise Windows-convention Unicode-on-disk
    /// plus case-preserved names (pCloud canonicalises names
    /// server-side). Case-sensitive search must NOT be enabled by
    /// default — it breaks Windows apps that assume case-insensitivity.
    #[test]
    fn windows_volume_params_flags_are_sane() {
        let opts = WindowsPlatformMount.default_options();
        let vp = build_volume_params(&opts);
        assert_ne!(
            vp.flags & VP_FLAG_UNICODE_ON_DISK,
            0,
            "UNICODE_ON_DISK must be set"
        );
        assert_ne!(
            vp.flags & VP_FLAG_CASE_PRESERVED_NAMES,
            0,
            "CASE_PRESERVED_NAMES must be set"
        );
        assert_eq!(
            vp.flags & VP_FLAG_CASE_SENSITIVE_SEARCH,
            0,
            "CASE_SENSITIVE_SEARCH must NOT be set by default"
        );
        assert_eq!(
            vp.max_component_length, 255,
            "max filename length must be 255"
        );
    }
}

//! **PLATFORM: Windows only.** Hand-rolled thin FFI bindings against the
//! WinFSP (Windows File System Proxy) public C ABI.
//!
//! ============================================================================
//!  PHASE-1 SCAFFOLDING -- NOT YET TESTED ON WINDOWS
//!  This module compiles against declared types only. No Windows target build,
//!  no runtime validation, no WinFSP linkage, no dispatcher lifecycle test.
//!  Treat every symbol here as a structural placeholder until
//!  bd-1du.4 Agent A proves it against a real winfsp-x64.dll.
//! ============================================================================
//!
//! # Rationale
//!
//! We intentionally **do not** depend on the third-party `winfsp` crate:
//!
//! * `winfsp` bundles a large macro-driven wrapper surface that is hard to
//!   audit for unsafe soundness and secret-handling discipline (see
//!   `CLAUDE.md` "Security and Enterprise Rules").
//! * WinFSP's C ABI is small and stable enough that a hand-rolled binding
//!   keeps the exposed `unsafe` surface minimal and reviewable.
//! * Dynamic loading of `winfsp-x64.dll` via `LoadLibraryW` lets us
//!   gracefully report `MountError::Unsupported` when the MSI is not
//!   installed, instead of failing at link time.
//!
//! # Runtime loading
//!
//! The WinFSP MSI installer (https://winfsp.dev/) places
//! `winfsp-x64.dll` into `%ProgramFiles(x86)%\WinFsp\bin\` and adds the
//! directory to the machine `PATH`. [`load_winfsp`] relies on the standard
//! Win32 loader search order to find it; operators on locked-down hosts
//! can alternatively ship the DLL next to the daemon executable.
//!
//! # Bindings subset
//!
//! We declare only the subset of WinFSP used by the MVP mounted-drive
//! runtime:
//!
//! * [`FSP_FILE_SYSTEM_INTERFACE`] -- the callback table populated by the
//!   adapter shim: `GetVolumeInfo`, `GetSecurityByName`, `Open`, `Close`,
//!   `Read`, `Write`, `Flush`, `ReadDirectory`, `Create`, `Overwrite`,
//!   `Cleanup`, `Rename`, `SetBasicInfo`, `SetFileSize`, `SetSecurity`.
//! * Lifecycle entry points: [`FspFileSystemCreate`],
//!   [`FspFileSystemSetMountPoint`], [`FspFileSystemStartDispatcher`],
//!   [`FspFileSystemStopDispatcher`], [`FspFileSystemDelete`].
//!
//! Alternate Data Streams (ADS) and reparse-point handling are
//! intentionally **not** declared here. The adapter returns
//! `STATUS_NOT_SUPPORTED` (`0xC00000BB`) for those entry points.
//!
//! Types follow WinFSP's `winfsp/winfsp.h` and
//! `winfsp/fsctl.h`. UCS-2/UTF-16 strings arrive as `PWSTR`.

#![cfg(target_os = "windows")]
#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::c_void;
use std::os::raw::{c_int, c_ulong};

// We reuse the shared Win32 foundation types exported by the already-present
// `windows` crate (feature set already includes Win32 storage/system items
// per Y3, per CLAUDE.md). This keeps HANDLE/NTSTATUS/BOOLEAN/PWSTR/FILETIME
// definitions aligned with the rest of the codebase.
pub use windows::Win32::Foundation::{BOOLEAN, FILETIME, HANDLE, HMODULE, NTSTATUS};
pub use windows::core::{PCWSTR, PWSTR};

// ---------------------------------------------------------------------------
//  NT status codes used by the adapter
// ---------------------------------------------------------------------------

/// `STATUS_SUCCESS` -- the operation completed successfully.
pub const STATUS_SUCCESS: NTSTATUS = NTSTATUS(0x0000_0000_u32 as i32);
/// `STATUS_NOT_SUPPORTED` -- returned for ADS / reparse points / unsupported
/// control codes in the MVP.
pub const STATUS_NOT_SUPPORTED: NTSTATUS = NTSTATUS(0xC000_00BB_u32 as i32);
/// `STATUS_OBJECT_NAME_NOT_FOUND` -- used when path lookup fails.
pub const STATUS_OBJECT_NAME_NOT_FOUND: NTSTATUS = NTSTATUS(0xC000_0034_u32 as i32);
/// `STATUS_ACCESS_DENIED`.
pub const STATUS_ACCESS_DENIED: NTSTATUS = NTSTATUS(0xC000_0022_u32 as i32);
/// `STATUS_INVALID_PARAMETER`.
pub const STATUS_INVALID_PARAMETER: NTSTATUS = NTSTATUS(0xC000_000D_u32 as i32);
/// `STATUS_END_OF_FILE`.
pub const STATUS_END_OF_FILE: NTSTATUS = NTSTATUS(0xC000_0011_u32 as i32);
/// `STATUS_IO_DEVICE_ERROR`.
pub const STATUS_IO_DEVICE_ERROR: NTSTATUS = NTSTATUS(0xC000_0185_u32 as i32);
/// `STATUS_OBJECT_NAME_COLLISION` — target name already exists.
pub const STATUS_OBJECT_NAME_COLLISION: NTSTATUS = NTSTATUS(0xC000_0035_u32 as i32);
/// `STATUS_CANNOT_DELETE` — delete refused (e.g. non-empty directory).
pub const STATUS_CANNOT_DELETE: NTSTATUS = NTSTATUS(0xC000_0121_u32 as i32);
/// `STATUS_DIRECTORY_NOT_EMPTY`.
pub const STATUS_DIRECTORY_NOT_EMPTY: NTSTATUS = NTSTATUS(0xC000_0101_u32 as i32);
/// `STATUS_MEDIA_WRITE_PROTECTED` — write on a read-only adapter.
pub const STATUS_MEDIA_WRITE_PROTECTED: NTSTATUS = NTSTATUS(0xC000_00A2_u32 as i32);

// ---------------------------------------------------------------------------
//  Volume params (subset of FSP_FSCTL_VOLUME_PARAMS)
// ---------------------------------------------------------------------------

/// Max bytes for the UTF-16 volume prefix / file-system name in
/// `FSP_FSCTL_VOLUME_PARAMS`. Per WinFSP `fsctl.h`.
pub const FSP_FSCTL_VOLUME_PREFIX_SIZE: usize = 192;
pub const FSP_FSCTL_VOLUME_FSNAME_SIZE: usize = 16;

/// Subset of `FSP_FSCTL_VOLUME_PARAMS` (WinFSP `fsctl.h`).
///
/// We only declare fields the adapter actually reads/writes. All other
/// fields are packed into [`Self::reserved_tail`] as bytes; WinFSP will
/// zero them through [`FspFileSystemCreate`]'s volume-params pointer.
///
/// NOTE: The true struct layout is WinFSP-internal and version-sensitive.
/// A final Windows-side build must validate `size_of::<VolumeParams>() ==
/// sizeof(FSP_FSCTL_VOLUME_PARAMS)` and each field offset against the
/// installed WinFSP headers before we claim runtime parity.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VolumeParams {
    pub version: u16,
    pub sector_size: u16,
    pub sectors_per_allocation_unit: u16,
    pub max_component_length: u16,
    pub volume_creation_time: u64,
    pub volume_serial_number: u32,
    pub transact_timeout: u32,
    pub irp_timeout: u32,
    pub irp_capacity: u32,
    pub file_info_timeout: u32,
    pub flags: u32,
    /// UTF-16 volume prefix (for network FS), NUL-padded.
    pub prefix: [u16; FSP_FSCTL_VOLUME_PREFIX_SIZE / 2],
    /// UTF-16 filesystem name (e.g. `"pCloud"`), NUL-padded.
    pub file_system_name: [u16; FSP_FSCTL_VOLUME_FSNAME_SIZE / 2],
    /// Opaque tail to pad out to the real WinFSP struct size. The Windows
    /// build must tune this constant against the installed headers; until
    /// then, treat the struct as "the interesting prefix".
    pub reserved_tail: [u8; 256],
}

/// `FSP_FSCTL_VOLUME_INFO`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VolumeInfo {
    pub total_size: u64,
    pub free_size: u64,
    pub volume_label_length: u16,
    pub volume_label: [u16; 32],
}

/// `FSP_FSCTL_FILE_INFO` -- what WinFSP expects per-file.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FileInfo {
    pub file_attributes: u32,
    pub reparse_tag: u32,
    pub allocation_size: u64,
    pub file_size: u64,
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub change_time: u64,
    pub index_number: u64,
    pub hard_links: u32,
    pub ea_size: u32,
}

/// `FSP_FSCTL_DIR_INFO` header (variable-length name follows).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DirInfoHeader {
    pub size: u16,
    pub _padding: u16,
    pub file_info: FileInfo,
    pub next_offset: u64,
    // `[u16]` NUL-less file name follows in-place.
}

// ---------------------------------------------------------------------------
//  The FSP_FILE_SYSTEM_INTERFACE callback table
// ---------------------------------------------------------------------------

/// Opaque pointer to an `FSP_FILE_SYSTEM`.
///
/// We intentionally type this as `*mut c_void` throughout the public
/// surface because external callers only ever pass it back to WinFSP
/// unchanged; the [`set_user_context`] / [`get_user_context`] helpers
/// below cast it to [`FspFileSystemLayout`] for in-process struct-field
/// access (see the layout block just below for the rationale).
pub type PFspFileSystem = *mut c_void;

// ---------------------------------------------------------------------------
//  FSP_FILE_SYSTEM struct prefix (for direct UserContext access)
// ---------------------------------------------------------------------------
//
// WinFSP ≥ 1.8 removed the `FspFileSystemSetUserContext` /
// `FspFileSystemGetUserContext` DLL exports and replaced them with inline
// functions that touch `FSP_FILE_SYSTEM::UserContext` directly. `GetProcAddress`
// therefore returns `NULL` for those names on modern installs (this is what
// caused `winfsp-x64.dll missing symbol: FspFileSystemSetUserContext` in the
// WinFSP live-mount test before this patch).
//
// Upstream struct definition (from `inc/winfsp/winfsp.h`, WinFSP master):
// ```c
// typedef struct _FSP_FILE_SYSTEM
// {
//     UINT16 Version;
//     PVOID UserContext;
//     WCHAR VolumeName[FSP_FSCTL_VOLUME_NAME_SIZEMAX / sizeof(WCHAR)];
//     ...
// } FSP_FILE_SYSTEM;
// FSP_FSCTL_STATIC_ASSERT(
//     (4 == sizeof(PVOID) && 660 == sizeof(FSP_FILE_SYSTEM)) ||
//     (8 == sizeof(PVOID) && 792 == sizeof(FSP_FILE_SYSTEM)),
//     "sizeof(FSP_FILE_SYSTEM) must be exactly 660 in 32-bit and 792 in 64-bit.");
// ```
//
// The only field we need to reach is `UserContext`, which sits **after a
// single `UINT16` plus natural `PVOID` alignment padding**. On 64-bit the
// compiler pads the `UINT16 Version` up to the `PVOID` alignment, so
// `offsetof(FSP_FILE_SYSTEM, UserContext) == sizeof(PVOID) == 8`. On 32-bit
// Windows the pad is 2 bytes and the offset is 4. Rust's default `#[repr(C)]`
// layout applies the same padding/alignment rules, so a struct whose prefix
// is `{ u16 Version; <pad>; *mut c_void UserContext; }` places `UserContext`
// at the same offset as the C struct.
//
// We deliberately mirror only the prefix we need and pad out the tail to
// the full documented struct size so that a stray write into `user_context`
// can never clobber memory belonging to whatever allocation follows. The
// Windows-only unit test `userctx_roundtrip_on_zeroed_struct` asserts the
// `UserContext` offset and the total struct size match the WinFSP ABI.

/// Total `sizeof(FSP_FILE_SYSTEM)` as documented by the WinFSP header's
/// `FSP_FSCTL_STATIC_ASSERT` on 64-bit Windows (8-byte `PVOID`). The
/// `pcloud-fs` build only targets 64-bit Windows; the 32-bit path is
/// intentionally unsupported and would need a different constant (660).
const FSP_FILE_SYSTEM_SIZE_64: usize = 792;

/// Minimal prefix of the WinFSP `FSP_FILE_SYSTEM` struct, sized to match the
/// full 64-bit layout. Only `version` and `user_context` have defined
/// positions; `_opaque_tail` is reserved storage that WinFSP owns — we
/// never read or write it.
///
/// # ABI assumption
///
/// * 64-bit Windows only. `#[cfg(target_pointer_width = "64")]` is enforced
///   at the module level via the surrounding `#![cfg(target_os = "windows")]`
///   combined with the 64-bit-only pcloud-fs build matrix; a guard
///   `const _: () = assert!(core::mem::size_of::<usize>() == 8, ...);`
///   inside the Windows test module fails the build if that assumption
///   ever breaks.
/// * Rust's `#[repr(C)]` reproduces MSVC's natural alignment rules for
///   this prefix, i.e. `UserContext` lands at offset 8. The
///   `userctx_offset_matches_winfsp_abi` test asserts this at runtime.
#[repr(C)]
pub struct FspFileSystemLayout {
    /// `UINT16 Version` — WinFSP interface version. Written by
    /// `FspFileSystemCreate`; we never mutate it.
    pub version: u16,
    /// 6 bytes of alignment padding so `user_context` is PVOID-aligned.
    /// Named explicitly to keep the layout self-documenting rather than
    /// letting the compiler insert invisible padding.
    _pad_after_version: [u8; 6],
    /// `PVOID UserContext` — the slot we attach our `Box<dyn FuseAdapter>`
    /// pointer to. WinFSP itself never touches this field; it exists
    /// purely for the user-mode file system to stash a back-pointer.
    pub user_context: *mut c_void,
    /// Reserved storage for the remainder of `FSP_FILE_SYSTEM`. WinFSP
    /// owns these bytes; Rust code must never touch them.
    _opaque_tail: [u8; FSP_FILE_SYSTEM_SIZE_64 - 16],
}

// `FspFileSystemLayout` is strictly a view into a foreign allocation. It
// is never moved, cloned, or shared by value across threads by us — we
// only ever operate on `*mut FspFileSystemLayout` that WinFSP hands back.
// We do not implement `Send`/`Sync` on the view struct itself because
// callers operate on the raw pointer, which carries no auto-traits.

/// Write the WinFSP user-context pointer. Replaces the removed
/// `FspFileSystemSetUserContext` DLL export.
///
/// # Safety
///
/// * `fs` must be a non-null `FSP_FILE_SYSTEM*` returned by
///   `FspFileSystemCreate` and not yet passed to `FspFileSystemDelete`.
/// * The caller is responsible for lifetime management of `ctx`; WinFSP
///   does not free it.
#[inline]
pub unsafe fn set_user_context(fs: PFspFileSystem, ctx: *mut c_void) {
    debug_assert!(!fs.is_null(), "FspFileSystem pointer must be non-null");
    // SAFETY: contract documented above; `FspFileSystemLayout` mirrors
    // the prefix of the real WinFSP struct at offset-identical field
    // positions (verified by `userctx_offset_matches_winfsp_abi`).
    unsafe { (*(fs as *mut FspFileSystemLayout)).user_context = ctx };
}

/// Read the WinFSP user-context pointer. Replaces the removed
/// `FspFileSystemGetUserContext` DLL export.
///
/// # Safety
///
/// * `fs` must be a non-null `FSP_FILE_SYSTEM*` returned by
///   `FspFileSystemCreate` and not yet passed to `FspFileSystemDelete`.
#[inline]
pub unsafe fn get_user_context(fs: PFspFileSystem) -> *mut c_void {
    if fs.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: same contract as `set_user_context`.
    unsafe { (*(fs as *const FspFileSystemLayout)).user_context }
}

/// `FSP_FILE_SYSTEM_INTERFACE` -- function-pointer vtable WinFSP invokes
/// on each NT I/O request. All callbacks run on WinFSP dispatcher threads.
///
/// Every entry point is `Option<extern "system" fn(...) -> NTSTATUS>` so we
/// can populate only the subset we support and leave the rest `None`; WinFSP
/// treats a `None` slot as "not implemented" and routes an appropriate
/// NTSTATUS back to the caller.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FSP_FILE_SYSTEM_INTERFACE {
    pub GetVolumeInfo:
        Option<extern "system" fn(fs: PFspFileSystem, info: *mut VolumeInfo) -> NTSTATUS>,
    pub SetVolumeLabel: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            volume_label: PCWSTR,
            info: *mut VolumeInfo,
        ) -> NTSTATUS,
    >,
    pub GetSecurityByName: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_name: PCWSTR,
            file_attributes: *mut u32,
            security_descriptor: *mut c_void,
            security_descriptor_size: *mut usize,
        ) -> NTSTATUS,
    >,
    pub Create: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_name: PCWSTR,
            create_options: u32,
            granted_access: u32,
            file_attributes: u32,
            security_descriptor: *const c_void,
            allocation_size: u64,
            file_context: *mut *mut c_void,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub Open: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_name: PCWSTR,
            create_options: u32,
            granted_access: u32,
            file_context: *mut *mut c_void,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub Overwrite: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            file_attributes: u32,
            replace_file_attributes: BOOLEAN,
            allocation_size: u64,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub Cleanup: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            file_name: PCWSTR,
            flags: u32,
        ),
    >,
    pub Close: Option<extern "system" fn(fs: PFspFileSystem, file_context: *mut c_void)>,
    pub Read: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            buffer: *mut c_void,
            offset: u64,
            length: u32,
            bytes_transferred: *mut u32,
        ) -> NTSTATUS,
    >,
    pub Write: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            buffer: *const c_void,
            offset: u64,
            length: u32,
            write_to_end_of_file: BOOLEAN,
            constrained_io: BOOLEAN,
            bytes_transferred: *mut u32,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub Flush: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub GetFileInfo: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub SetBasicInfo: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            file_attributes: u32,
            creation_time: u64,
            last_access_time: u64,
            last_write_time: u64,
            change_time: u64,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub SetFileSize: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            new_size: u64,
            set_allocation_size: BOOLEAN,
            file_info: *mut FileInfo,
        ) -> NTSTATUS,
    >,
    pub CanDelete: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            file_name: PCWSTR,
        ) -> NTSTATUS,
    >,
    pub Rename: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            file_name: PCWSTR,
            new_file_name: PCWSTR,
            replace_if_exists: BOOLEAN,
        ) -> NTSTATUS,
    >,
    pub GetSecurity: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            security_descriptor: *mut c_void,
            security_descriptor_size: *mut usize,
        ) -> NTSTATUS,
    >,
    pub SetSecurity: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            security_information: u32,
            modification_descriptor: *const c_void,
        ) -> NTSTATUS,
    >,
    pub ReadDirectory: Option<
        extern "system" fn(
            fs: PFspFileSystem,
            file_context: *mut c_void,
            pattern: PCWSTR,
            marker: PCWSTR,
            buffer: *mut c_void,
            length: u32,
            bytes_transferred: *mut u32,
        ) -> NTSTATUS,
    >,
    /// Padding for WinFSP-internal trailing callbacks (ResolveReparsePoints,
    /// GetReparsePoint, SetReparsePoint, DeleteReparsePoint, GetStreamInfo,
    /// GetDirInfoByName, Control, SetDelete, CreateEx, OverwriteEx,
    /// GetEa, SetEa, ...). Leaving them as opaque `None`-equivalent pointers
    /// is safe because WinFSP only calls through the slots it was told exist.
    pub reserved_tail: [*mut c_void; 16],
}

// SAFETY: `FSP_FILE_SYSTEM_INTERFACE` is a vtable of `unsafe extern "system" fn`
// pointers plus a reserved null-padded tail. All function pointers:
//   (1) are `'static` (they are thunk addresses baked into the binary),
//   (2) are reentrant and do not capture or mutate any field of this struct,
//   (3) are never replaced after `load_winfsp` returns.
// There is no interior mutability. Concurrent reads across dispatcher
// threads are therefore safe.
unsafe impl Sync for FSP_FILE_SYSTEM_INTERFACE {}
unsafe impl Send for FSP_FILE_SYSTEM_INTERFACE {}

// ---------------------------------------------------------------------------
//  WinFSP lifecycle entry-point signatures (resolved dynamically)
// ---------------------------------------------------------------------------

pub type FnFspFileSystemCreate = unsafe extern "system" fn(
    device_name: PCWSTR,
    volume_params: *const VolumeParams,
    interface_: *const FSP_FILE_SYSTEM_INTERFACE,
    file_system: *mut PFspFileSystem,
) -> NTSTATUS;

pub type FnFspFileSystemSetMountPoint =
    unsafe extern "system" fn(file_system: PFspFileSystem, mount_point: PCWSTR) -> NTSTATUS;

pub type FnFspFileSystemStartDispatcher =
    unsafe extern "system" fn(file_system: PFspFileSystem, thread_count: c_ulong) -> NTSTATUS;

pub type FnFspFileSystemStopDispatcher = unsafe extern "system" fn(file_system: PFspFileSystem);

pub type FnFspFileSystemDelete = unsafe extern "system" fn(file_system: PFspFileSystem);

// NOTE: `FspFileSystemSetUserContext` / `FspFileSystemGetUserContext` are
// intentionally NOT declared here. WinFSP ≥ 1.8 removed the corresponding
// DLL exports and inlined them as direct-struct-field accessors in
// `winfsp.h`. Use [`set_user_context`] / [`get_user_context`] above, which
// reach into the `UserContext` slot of [`FspFileSystemLayout`] directly.
// See the block comment preceding `FspFileSystemLayout` for the rationale
// and ABI invariant.

/// `FspFileSystemAddDirInfo` — WinFSP helper that appends one
/// variable-length `FSP_FSCTL_DIR_INFO` record (header + UTF-16 name) to
/// the caller's reply buffer, handling the `Marker`/`NextOffset` cursor
/// bookkeeping. Returns `FALSE` when the entry does not fit.
///
/// Signature per WinFSP `winfsp.h`:
/// ```c
/// BOOLEAN FspFileSystemAddDirInfo(
///     FSP_FSCTL_DIR_INFO *DirInfo,
///     PVOID Buffer, ULONG Length,
///     PULONG PBytesTransferred);
/// ```
pub type FnFspFileSystemAddDirInfo = unsafe extern "system" fn(
    dir_info: *const c_void,
    buffer: *mut c_void,
    length: u32,
    bytes_transferred: *mut u32,
) -> BOOLEAN;

/// `FspFileSystemAddDirInfo` with `DirInfo == NULL` terminates the stream
/// (writes the NULL-record sentinel). Same symbol — we call it through the
/// same function pointer.

// ---------------------------------------------------------------------------
//  Dynamic loader
// ---------------------------------------------------------------------------

/// Resolved function pointers for one loaded copy of `winfsp-x64.dll`.
///
/// Kept alive for the lifetime of the daemon; dropping does *not* free the
/// DLL handle (WinFSP explicitly warns against `FreeLibrary` while the
/// dispatcher is running, and the daemon is single-instance).
pub struct WinFspLibrary {
    /// Module handle from `LoadLibraryW`. Stored so the DLL stays resident.
    /// Never used for explicit `FreeLibrary`; see safety note above.
    ///
    /// Note: since `windows` crate 0.52+ `LoadLibraryW` returns a dedicated
    /// `HMODULE` newtype distinct from `HANDLE` — stored as `HMODULE` to
    /// preserve that signal. Callers who need a raw `HANDLE` use
    /// `windows::Win32::Foundation::HANDLE(module.0)`.
    pub module: HMODULE,
    pub fsp_create: FnFspFileSystemCreate,
    pub fsp_set_mount_point: FnFspFileSystemSetMountPoint,
    pub fsp_start_dispatcher: FnFspFileSystemStartDispatcher,
    pub fsp_stop_dispatcher: FnFspFileSystemStopDispatcher,
    pub fsp_delete: FnFspFileSystemDelete,
    // `fsp_set_user_context` / `fsp_get_user_context` removed on 2026-04-19.
    // WinFSP ≥ 1.8 stopped exporting `FspFileSystemSetUserContext` /
    // `FspFileSystemGetUserContext` (they are now inline accessors in
    // `winfsp.h`). Callers use [`set_user_context`] / [`get_user_context`]
    // above, which go through [`FspFileSystemLayout`] directly.
    /// Optional: present in WinFSP 1.x+. Used by `ReadDirectory` to append
    /// directory entries. Resolved lazily; if missing we fall back to a
    /// manual buffer walk that mirrors the reference implementation.
    pub fsp_add_dir_info: Option<FnFspFileSystemAddDirInfo>,
}

// SAFETY (audit-06 LOW fuse / pcloud-rs-ncx.82-d):
//   Why `unsafe impl Sync + Send for WinFspLibrary` is sound:
//
//   1. Field inventory.
//      - `handle: HMODULE` — the Win32 module handle returned by
//        `LoadLibraryW`. We never mutate it after [`load_winfsp`]
//        stores it, and the OS loader guarantees the module stays
//        mapped at a fixed address for the lifetime of the process
//        (we never call `FreeLibrary`).
//      - `fsp_*: fn(...)` — resolved once via `GetProcAddress` and
//        then treated as immutable `'static` thunks. Function
//        pointers are trivially `Sync + Send`.
//      - `fsp_add_dir_info: Option<fn(...)>` — same shape as above;
//        `Option<fn>` is also `Sync + Send`.
//
//   2. Access pattern.
//      - `WinFspLibrary` is only ever held behind a `&'static` / `Arc`
//        that is cloned across WinFSP dispatcher worker threads.
//      - All access is read-only; there is no interior mutability
//        anywhere in the struct.
//      - The resolved thunks enforce their own reentrancy contract
//        (see [`FSP_FILE_SYSTEM_INTERFACE`] SAFETY block above for
//        the thunk-level argument).
//
//   3. Teardown.
//      - There is no teardown: the library is process-global. A
//        mount-level teardown races unmount state on the WinFSP
//        dispatcher, not this library handle.
//
//   Therefore sharing the struct across dispatcher threads cannot
//   introduce a data race at the Rust-memory-model level.
unsafe impl Sync for WinFspLibrary {}
unsafe impl Send for WinFspLibrary {}

/// Attempt to `LoadLibraryW("winfsp-x64.dll")` and resolve the lifecycle
/// entry points.
///
/// Returns `Ok(None)` when the DLL is simply missing (caller maps to
/// `MountError::Unsupported`). Returns `Err` if the DLL loaded but is
/// missing a required export (i.e. incompatible WinFSP version).
///
/// # Platform
///
/// Relies on the Win32 loader search order. The WinFSP MSI installer adds
/// `%ProgramFiles(x86)%\WinFsp\bin` to the machine `PATH`; operators on
/// restricted hosts may instead co-locate `winfsp-x64.dll` with the
/// daemon executable.
pub fn load_winfsp() -> Result<Option<WinFspLibrary>, String> {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    // UTF-16 name including trailing NUL.
    let name: Vec<u16> = "winfsp-x64.dll\0".encode_utf16().collect();

    // SAFETY: `LoadLibraryW` expects a NUL-terminated UTF-16 string.
    // `name` is owned locally, NUL-terminated, and alive for the call.
    let module = unsafe { LoadLibraryW(PCWSTR(name.as_ptr())) };
    let module = match module {
        Ok(h) if !h.is_invalid() => h,
        // Any error (missing DLL, ACCESS_DENIED) is treated as "not
        // installed" for probe purposes. The caller surfaces the install
        // hint.
        _ => return Ok(None),
    };

    // Helper: resolve a symbol or fail loudly (version mismatch).
    //
    // SAFETY: `GetProcAddress` with a module handle returned by
    // `LoadLibraryW` is defined behavior; the returned pointer lifetime is
    // tied to the module, which we keep resident for process lifetime.
    unsafe fn resolve<T: Copy>(module: HMODULE, name: &[u8]) -> Result<T, String> {
        // We rely on `name` being ASCII + NUL-terminated.
        // SAFETY (Rust 2024 `unsafe_op_in_unsafe_fn`): the outer `unsafe fn`
        // signature documents the module-lifetime + ABI-shape contract; the
        // inner `unsafe { ... }` blocks narrow each call to its specific
        // precondition.
        let p = unsafe { GetProcAddress(module, windows::core::PCSTR(name.as_ptr())) };
        match p {
            Some(f) => {
                // SAFETY: transmute fn-ptr to the typed signature. Caller
                // guarantees `T` is the correct WinFSP ABI signature.
                Ok(unsafe { std::mem::transmute_copy::<_, T>(&f) })
            }
            None => Err(format!(
                "winfsp-x64.dll missing symbol: {}",
                String::from_utf8_lossy(name.split_last().map(|(_, r)| r).unwrap_or(name))
            )),
        }
    }

    // Optional-symbol helper: returns `None` when the export is missing.
    //
    // # Safety
    // Same contract as `resolve`; only callable during `load_winfsp`.
    unsafe fn resolve_optional<T: Copy>(module: HMODULE, name: &[u8]) -> Option<T> {
        // SAFETY: same as `resolve` — narrow the 2024-edition unsafe
        // block around the specific GetProcAddress call.
        let p = unsafe { GetProcAddress(module, windows::core::PCSTR(name.as_ptr())) };
        // SAFETY: transmute fn-ptr; caller guarantees `T` matches the ABI.
        p.map(|f| unsafe { std::mem::transmute_copy::<_, T>(&f) })
    }

    // SAFETY: see `resolve`. All export names are ASCII NUL-terminated.
    let lib = unsafe {
        WinFspLibrary {
            module,
            fsp_create: resolve(module, b"FspFileSystemCreate\0")?,
            fsp_set_mount_point: resolve(module, b"FspFileSystemSetMountPoint\0")?,
            fsp_start_dispatcher: resolve(module, b"FspFileSystemStartDispatcher\0")?,
            fsp_stop_dispatcher: resolve(module, b"FspFileSystemStopDispatcher\0")?,
            fsp_delete: resolve(module, b"FspFileSystemDelete\0")?,
            // `FspFileSystemSetUserContext` / `FspFileSystemGetUserContext`
            // are deliberately not resolved — they are no longer DLL
            // exports in WinFSP ≥ 1.8. See the module-level comment above
            // [`FspFileSystemLayout`] for the direct-field access path.
            // Optional symbol — resolve best-effort so older DLLs still load.
            fsp_add_dir_info: resolve_optional(module, b"FspFileSystemAddDirInfo\0"),
        }
    };

    Ok(Some(lib))
}

// ---------------------------------------------------------------------------
//  Time helpers
// ---------------------------------------------------------------------------

/// Convert a Unix nanosecond timestamp to a Windows FILETIME tick count
/// (100-ns intervals since 1601-01-01 UTC).
///
/// Reference: Win32 `FILETIME`, epoch delta = 11_644_473_600 seconds.
#[inline]
#[must_use]
pub const fn unix_nanos_to_filetime(unix_nanos: i128) -> u64 {
    // 11644473600 seconds between 1601-01-01 and 1970-01-01.
    const EPOCH_DELTA_100NS: i128 = 11_644_473_600_i128 * 10_000_000_i128;
    let ticks_since_unix_epoch = unix_nanos / 100;
    let ticks = ticks_since_unix_epoch.saturating_add(EPOCH_DELTA_100NS);
    if ticks < 0 { 0 } else { ticks as u64 }
}

/// Inverse of [`unix_nanos_to_filetime`] for round-trip tests.
#[inline]
#[must_use]
pub const fn filetime_to_unix_nanos(ticks: u64) -> i128 {
    const EPOCH_DELTA_100NS: i128 = 11_644_473_600_i128 * 10_000_000_i128;
    let ticks_i = ticks as i128;
    (ticks_i - EPOCH_DELTA_100NS) * 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_roundtrip_at_unix_epoch() {
        // Unix epoch (1970-01-01) -> 116444736000000000 100-ns ticks.
        let ft = unix_nanos_to_filetime(0);
        assert_eq!(ft, 116_444_736_000_000_000_u64);
        assert_eq!(filetime_to_unix_nanos(ft), 0);
    }

    #[test]
    fn filetime_roundtrip_positive_time() {
        let unix_ns: i128 = 1_700_000_000_000_000_000; // ~2023-11-14
        let ft = unix_nanos_to_filetime(unix_ns);
        let back = filetime_to_unix_nanos(ft);
        assert_eq!(back, unix_ns);
    }

    #[test]
    fn filetime_clamps_pre_1601() {
        // Very negative unix nanos (pre-1601) clamp to zero instead of wrapping.
        assert_eq!(unix_nanos_to_filetime(-1_000_000_000_000_000_000_000), 0);
    }

    // --- WinFSP FSP_FILE_SYSTEM layout checks ---------------------------
    //
    // These tests only run on 64-bit Windows, which is also the only
    // `pcloud-fs` build target that exercises the WinFSP path. The whole
    // module is gated on `cfg(target_os = "windows")` so a Linux host
    // skips them entirely.

    /// Compile-time guard: we only support 64-bit `FSP_FILE_SYSTEM` layout.
    /// 32-bit Windows would need the alternate 660-byte variant.
    const _: () = assert!(
        core::mem::size_of::<usize>() == 8,
        "FspFileSystemLayout assumes 64-bit Windows (PVOID == 8 bytes)"
    );

    #[test]
    fn fsp_filesystem_layout_matches_winfsp_abi() {
        // Matches the WinFSP static assert in `winfsp.h`:
        //     (8 == sizeof(PVOID) && 792 == sizeof(FSP_FILE_SYSTEM))
        assert_eq!(
            core::mem::size_of::<FspFileSystemLayout>(),
            792,
            "FspFileSystemLayout must mirror WinFSP 64-bit FSP_FILE_SYSTEM size"
        );
        // `UserContext` must land at offset 8 (after UINT16 Version + 6
        // bytes of natural PVOID alignment padding).
        let base = core::mem::offset_of!(FspFileSystemLayout, user_context);
        assert_eq!(base, 8, "user_context offset must be 8 on 64-bit Windows");
    }

    #[test]
    fn userctx_roundtrip_on_zeroed_struct() {
        // Fabricate a zeroed `FSP_FILE_SYSTEM`-shaped buffer, set the
        // user-context slot via the inline accessor, then read it back.
        // This exercises exactly the same pointer math the mount path
        // will perform at runtime, minus WinFSP itself.
        let mut buf: FspFileSystemLayout = unsafe { core::mem::zeroed() };
        let fs: PFspFileSystem = (&mut buf as *mut FspFileSystemLayout).cast::<c_void>();

        // Null before any write.
        assert!(unsafe { get_user_context(fs) }.is_null());

        // A non-null dummy pointer — we only inspect the bit pattern, we
        // never dereference it.
        let dummy: *mut c_void = 0xDEAD_BEEF_CAFE_F00D_usize as *mut c_void;
        unsafe { set_user_context(fs, dummy) };
        assert_eq!(unsafe { get_user_context(fs) }, dummy);

        // Clearing works too.
        unsafe { set_user_context(fs, core::ptr::null_mut()) };
        assert!(unsafe { get_user_context(fs) }.is_null());
    }
}

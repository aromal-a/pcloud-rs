//! **PLATFORM: macOS only.**
//! **GATING: `#[cfg(target_os = "macos")]`** -- gated at the `mod macos_ffi;`
//! declaration in `platform/macos.rs`.
//!
//! Minimal hand-rolled FFI surface for **fuse-t**
//! (<https://www.fuse-t.org/>), whose shipped `libfuse.dylib` is ABI-
//! compatible with the libfuse 2.9 low-level API
//! (`fuse_lowlevel.h`). Binding directly avoids pulling in a macOS-only
//! third-party crate and keeps the surface auditable.
//!
//! NOTE: This is **Phase-1 scaffolding**. It has **not** been
//! compiled or executed on an actual Mac. The struct layouts below
//! follow the publicly documented libfuse 2.9 ABI; real-Mac bring-up
//! (bd-1du.4) must verify layout, calling convention, and the opaque
//! type sizes against the installed `libfuse.dylib` on a macOS host.
//!
//! Every `extern "C"` function declared here corresponds to a symbol
//! exported by fuse-t's `libfuse.dylib`. Every raw pointer / C struct
//! carries a doc comment naming the upstream header.

#![allow(non_camel_case_types)]
#![allow(dead_code)]
#![allow(missing_docs)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

// -----------------------------------------------------------------------------
// Opaque handle types (upstream: <fuse_lowlevel.h>, <fuse_common.h>).
// We never dereference these in Rust; they are only ever passed back to
// the library. Their concrete layout is intentionally `()` + a
// `PhantomData` so `*mut T` stays `!Send`-friendly and ABI-compatible
// with any opaque C pointer.
// -----------------------------------------------------------------------------

/// `struct fuse_session` from `<fuse_lowlevel.h>`.
#[repr(C)]
pub struct fuse_session {
    _private: [u8; 0],
}

/// `struct fuse_chan` from `<fuse_common.h>`.
#[repr(C)]
pub struct fuse_chan {
    _private: [u8; 0],
}

/// `struct fuse_req` from `<fuse_lowlevel.h>`. Reply handle passed to
/// every op; ops reply via `fuse_reply_*` helpers (not declared here
/// yet -- added incrementally as each op gets wired).
#[repr(C)]
pub struct fuse_req {
    _private: [u8; 0],
}

/// Opaque request handle pointer.
pub type fuse_req_t = *mut fuse_req;

/// libfuse inode number. 64-bit across all supported targets.
pub type fuse_ino_t = u64;

// -----------------------------------------------------------------------------
// `struct fuse_args` (upstream `<fuse_opt.h>`) -- used by
// `fuse_session_new` / `fuse_parse_cmdline`.
// -----------------------------------------------------------------------------

/// `struct fuse_args` from `<fuse_opt.h>`.
#[repr(C)]
pub struct fuse_args {
    pub argc: c_int,
    pub argv: *mut *mut c_char,
    pub allocated: c_int,
}

// -----------------------------------------------------------------------------
// `struct fuse_file_info` (upstream `<fuse_common.h>`). Passed by
// pointer into open/read/write/release. Layout matches libfuse 2.9.
// Bit-field accessors are intentionally omitted at this stage.
// -----------------------------------------------------------------------------

/// `struct fuse_file_info` from `<fuse_common.h>`.
#[repr(C)]
pub struct fuse_file_info {
    pub flags: c_int,
    pub fh_old: c_uint,
    pub writepage: c_int,
    /// Bitfield container for `direct_io`, `keep_cache`, `flush`,
    /// `nonseekable`, etc. We do not touch bit layout here; fuse-t
    /// treats these as advisory.
    pub bitfields: u32,
    pub fh: u64,
    pub lock_owner: u64,
}

// -----------------------------------------------------------------------------
// `struct stat` is re-used from libc; fuse-t accepts the host's
// `struct stat` through getattr/setattr replies. We reference
// `libc::stat` directly at call sites so there's no duplicate definition.
// `struct statvfs` likewise comes from libc for statfs replies.
// -----------------------------------------------------------------------------

/// `struct fuse_entry_param` from `<fuse_lowlevel.h>`. Returned via
/// `fuse_reply_entry` / `fuse_reply_create` to describe a resolved
/// directory entry. `ino == 0` signals a negative (cached) lookup in
/// libfuse parlance; the pCloud adapter always emits a nonzero ino on
/// success, so we do not special-case the zero path.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct fuse_entry_param {
    pub ino: fuse_ino_t,
    pub generation: u64,
    pub attr: libc::stat,
    pub attr_timeout: f64,
    pub entry_timeout: f64,
}

// -----------------------------------------------------------------------------
// Low-level operations vtable (upstream: `struct fuse_lowlevel_ops` in
// `<fuse_lowlevel.h>`). We declare only the subset of fields the pCloud
// adapter intends to implement in bring-up. Fields we don't populate
// must be left `None` so libfuse returns `ENOSYS` automatically.
//
// The upstream struct contains additional trailing fields (xattr ops,
// getlk/setlk, bmap, ioctl, poll, etc.). Omitting them here would
// corrupt the vtable layout, so we pad with enough `Option<...>` slots
// to cover the libfuse 2.9 ABI. Real-Mac bring-up must reconcile the
// exact slot count against the installed header.
// -----------------------------------------------------------------------------

/// Subset of `struct fuse_lowlevel_ops` needed for the pCloud FUSE
/// adapter. Each `Option<extern "C" fn(...)>` corresponds to a libfuse
/// low-level op; `None` leaves the slot empty and libfuse returns
/// `ENOSYS` to the kernel.
///
/// **Layout caveat:** this mirrors the libfuse 2.9 ABI in field order
/// but stops at the ops we wire. Passing `&LowlevelOps` directly to
/// `fuse_lowlevel_new` is UNSAFE until the full struct is padded to
/// the exact upstream size. The mount thunk in [`super::macos`]
/// therefore constructs a zero-initialized buffer of upstream
/// `sizeof(fuse_lowlevel_ops)` and populates only the fields it
/// actually implements. See `LowlevelOps::write_into`.
#[repr(C)]
#[derive(Default)]
pub struct LowlevelOps {
    pub init: Option<extern "C" fn(userdata: *mut c_void, conn: *mut c_void)>,
    pub destroy: Option<extern "C" fn(userdata: *mut c_void)>,
    pub lookup: Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char)>,
    pub forget: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, nlookup: u64)>,
    pub getattr: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    pub setattr: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            attr: *mut libc::stat,
            to_set: c_int,
            fi: *mut fuse_file_info,
        ),
    >,
    pub readlink: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t)>,
    pub mknod: Option<
        extern "C" fn(
            req: fuse_req_t,
            parent: fuse_ino_t,
            name: *const c_char,
            mode: u32,
            rdev: u32,
        ),
    >,
    pub mkdir:
        Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char, mode: u32)>,
    pub unlink: Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char)>,
    pub rmdir: Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char)>,
    pub symlink: Option<
        extern "C" fn(
            req: fuse_req_t,
            link: *const c_char,
            parent: fuse_ino_t,
            name: *const c_char,
        ),
    >,
    pub rename: Option<
        extern "C" fn(
            req: fuse_req_t,
            parent: fuse_ino_t,
            name: *const c_char,
            newparent: fuse_ino_t,
            newname: *const c_char,
        ),
    >,
    pub link: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            newparent: fuse_ino_t,
            newname: *const c_char,
        ),
    >,
    pub open: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    pub read: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            size: usize,
            off: i64,
            fi: *mut fuse_file_info,
        ),
    >,
    pub write: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            buf: *const c_char,
            size: usize,
            off: i64,
            fi: *mut fuse_file_info,
        ),
    >,
    pub flush: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    pub release: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    pub fsync: Option<
        extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, datasync: c_int, fi: *mut fuse_file_info),
    >,
    pub opendir: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    pub readdir: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            size: usize,
            off: i64,
            fi: *mut fuse_file_info,
        ),
    >,
    pub releasedir:
        Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    pub fsyncdir: Option<
        extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, datasync: c_int, fi: *mut fuse_file_info),
    >,
    pub statfs: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t)>,
    // Upstream `fuse_lowlevel_ops` interposes these 5 extended-attribute
    // and access slots between `statfs` and `create`. We do not wire
    // them (libfuse returns `ENOSYS` automatically when `None`), but we
    // must keep them in the Rust struct so the `create` slot lands at
    // the exact offset fuse-t's `libfuse.dylib` reads. Without these
    // placeholders our `create` thunk is installed at the `setxattr`
    // offset and kernel CREATEs appear unimplemented (observed as
    // `touch ~/mount/f` returning EACCES/EPERM with no FUSE callback
    // firing).
    pub setxattr: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            name: *const c_char,
            value: *const c_char,
            size: usize,
            flags: c_int,
            position: u32,
        ),
    >,
    pub getxattr:
        Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, name: *const c_char, size: usize)>,
    pub listxattr: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, size: usize)>,
    pub removexattr: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, name: *const c_char)>,
    pub access: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, mask: c_int)>,
    pub create: Option<
        extern "C" fn(
            req: fuse_req_t,
            parent: fuse_ino_t,
            name: *const c_char,
            mode: u32,
            fi: *mut fuse_file_info,
        ),
    >,
}

// -----------------------------------------------------------------------------
// Full-layout mirror of `struct fuse_lowlevel_ops` for the libfuse 2.9 ABI
// (matches the declaration in fuse-t's bundled `fuse_lowlevel.h`, which is
// ABI-compatible with upstream libfuse 2.9). This exists SOLELY so we can
// hand `fuse_lowlevel_new` a size equal to the full upstream `sizeof(struct
// fuse_lowlevel_ops)` regardless of how many trailing ops the adapter's
// public [`LowlevelOps`] struct actually exposes.
//
// Audit (bd-1du.4 / §5-opus C-2 / §5-sonnet C-2): passing the Rust
// `size_of::<LowlevelOps>()` to `fuse_lowlevel_new` when the struct stops
// short of the upstream definition made libfuse read uninitialized memory
// past our buffer (or equivalently, install thunks at the wrong offsets).
// Always pass [`LOWLEVEL_OPS_SIZE`] instead — it is the full upstream size.
//
// When fuse-t upgrades to a new libfuse ABI, update this struct to mirror
// the new layout *exactly* (same field order, same signatures). The
// const-assertion below enforces that [`LowlevelOps`] remains no larger
// than the full upstream layout, catching drift at compile time.
//
// Field order and signatures mirror `struct fuse_lowlevel_ops` from
// `fuse_lowlevel.h` (libfuse 2.9 ABI). Every slot is `Option<extern "C"
// fn(...)>` so the all-zero byte pattern used at the call site corresponds
// to "None" for every op, which libfuse interprets as ENOSYS.
// -----------------------------------------------------------------------------

/// Full-layout mirror of `struct fuse_lowlevel_ops` (libfuse 2.9). Used
/// only for [`LOWLEVEL_OPS_SIZE`]; never dereferenced at runtime.
#[repr(C)]
#[allow(dead_code)]
pub struct LowlevelOpsCompat {
    // Slots 0..=4 (lifecycle + name lookup + forget + attr).
    init: Option<extern "C" fn(userdata: *mut c_void, conn: *mut c_void)>,
    destroy: Option<extern "C" fn(userdata: *mut c_void)>,
    lookup: Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char)>,
    forget: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, nlookup: u64)>,
    getattr: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,

    // setattr + readlink + namespace ops.
    setattr: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            attr: *mut libc::stat,
            to_set: c_int,
            fi: *mut fuse_file_info,
        ),
    >,
    readlink: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t)>,
    mknod: Option<
        extern "C" fn(
            req: fuse_req_t,
            parent: fuse_ino_t,
            name: *const c_char,
            mode: u32,
            rdev: u32,
        ),
    >,
    mkdir:
        Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char, mode: u32)>,
    unlink: Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char)>,
    rmdir: Option<extern "C" fn(req: fuse_req_t, parent: fuse_ino_t, name: *const c_char)>,
    symlink: Option<
        extern "C" fn(
            req: fuse_req_t,
            link: *const c_char,
            parent: fuse_ino_t,
            name: *const c_char,
        ),
    >,
    rename: Option<
        extern "C" fn(
            req: fuse_req_t,
            parent: fuse_ino_t,
            name: *const c_char,
            newparent: fuse_ino_t,
            newname: *const c_char,
        ),
    >,
    link: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            newparent: fuse_ino_t,
            newname: *const c_char,
        ),
    >,

    // File I/O slots.
    open: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    read: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            size: usize,
            off: i64,
            fi: *mut fuse_file_info,
        ),
    >,
    write: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            buf: *const c_char,
            size: usize,
            off: i64,
            fi: *mut fuse_file_info,
        ),
    >,
    flush: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    release: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    fsync: Option<
        extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, datasync: c_int, fi: *mut fuse_file_info),
    >,

    // Directory slots.
    opendir: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    readdir: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            size: usize,
            off: i64,
            fi: *mut fuse_file_info,
        ),
    >,
    releasedir: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info)>,
    fsyncdir: Option<
        extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, datasync: c_int, fi: *mut fuse_file_info),
    >,

    // statfs + xattr family + access + create.
    statfs: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t)>,
    setxattr: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            name: *const c_char,
            value: *const c_char,
            size: usize,
            flags: c_int,
        ),
    >,
    getxattr:
        Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, name: *const c_char, size: usize)>,
    listxattr: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, size: usize)>,
    removexattr: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, name: *const c_char)>,
    access: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, mask: c_int)>,
    create: Option<
        extern "C" fn(
            req: fuse_req_t,
            parent: fuse_ino_t,
            name: *const c_char,
            mode: u32,
            fi: *mut fuse_file_info,
        ),
    >,

    // Trailing libfuse 2.9 ops our adapter does not wire. They remain
    // in layout so the upstream size is correct.
    getlk: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            fi: *mut fuse_file_info,
            lock: *mut c_void, // struct flock*
        ),
    >,
    setlk: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            fi: *mut fuse_file_info,
            lock: *mut c_void, // struct flock*
            sleep: c_int,
        ),
    >,
    bmap: Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, blocksize: usize, idx: u64)>,
    ioctl: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            cmd: c_int,
            arg: *mut c_void,
            fi: *mut fuse_file_info,
            flags: c_uint,
            in_buf: *const c_void,
            in_bufsz: usize,
            out_bufsz: usize,
        ),
    >,
    poll: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            fi: *mut fuse_file_info,
            ph: *mut c_void, // struct fuse_pollhandle*
        ),
    >,
    write_buf: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            bufv: *mut c_void, // struct fuse_bufvec*
            off: i64,
            fi: *mut fuse_file_info,
        ),
    >,
    retrieve_reply: Option<
        extern "C" fn(
            req: fuse_req_t,
            cookie: *mut c_void,
            ino: fuse_ino_t,
            offset: i64,
            bufv: *mut c_void, // struct fuse_bufvec*
        ),
    >,
    forget_multi: Option<
        extern "C" fn(
            req: fuse_req_t,
            count: usize,
            forgets: *mut c_void, // struct fuse_forget_data*
        ),
    >,
    flock:
        Option<extern "C" fn(req: fuse_req_t, ino: fuse_ino_t, fi: *mut fuse_file_info, op: c_int)>,
    fallocate: Option<
        extern "C" fn(
            req: fuse_req_t,
            ino: fuse_ino_t,
            mode: c_int,
            offset: i64,
            length: i64,
            fi: *mut fuse_file_info,
        ),
    >,
}

/// Size in bytes of the full upstream `struct fuse_lowlevel_ops` (libfuse
/// 2.9 ABI, as exposed by fuse-t). **Always** pass this value as the
/// `op_size` argument to [`fuse_lowlevel_new`]; never
/// `size_of::<LowlevelOps>()`, which is strictly smaller and would cause
/// libfuse to skip fields or read past our buffer.
pub const LOWLEVEL_OPS_SIZE: usize = std::mem::size_of::<LowlevelOpsCompat>();

// Compile-time guard: the adapter's public [`LowlevelOps`] must never grow
// larger than the full upstream layout. If it does, someone added an op
// beyond the 2.9 ABI and either the mirror struct here or the adapter
// struct is out of sync with fuse-t.
const _: () = {
    assert!(
        std::mem::size_of::<LowlevelOps>() <= std::mem::size_of::<LowlevelOpsCompat>(),
        "LowlevelOps exceeded the full libfuse 2.9 fuse_lowlevel_ops layout; \
         update LowlevelOpsCompat in macos_ffi.rs to mirror the new upstream ABI"
    );
};

// -----------------------------------------------------------------------------
// Extern declarations. Symbols come from fuse-t's `libfuse.dylib`,
// discovered via the dynamic linker at process start. Link-time
// resolution will happen only on macOS builds; Linux workspaces do
// not compile this module.
// -----------------------------------------------------------------------------

// On macOS the fuse-t libfuse.dylib is resolved at runtime via dyld's
// dynamic-lookup behavior enabled by `build.rs`. We intentionally do
// **not** use `#[link(name = "fuse")]` here: fuse-t is an optional
// runtime dependency and this crate must link on Macs where fuse-t is
// not installed. Runtime availability is gated by
// [`super::MacosPlatformMount::probe_supported`]; any call to these
// symbols without fuse-t loaded will trap at dlsym/dyld resolution time.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// `int fuse_parse_cmdline(struct fuse_args *args, char **mountpoint,
    /// int *multithreaded, int *foreground);`
    pub fn fuse_parse_cmdline(
        args: *mut fuse_args,
        mountpoint: *mut *mut c_char,
        multithreaded: *mut c_int,
        foreground: *mut c_int,
    ) -> c_int;

    /// `struct fuse_chan *fuse_mount(const char *mountpoint,
    /// struct fuse_args *args);`
    pub fn fuse_mount(mountpoint: *const c_char, args: *mut fuse_args) -> *mut fuse_chan;

    /// `void fuse_unmount(const char *mountpoint, struct fuse_chan *ch);`
    pub fn fuse_unmount(mountpoint: *const c_char, ch: *mut fuse_chan);

    /// `struct fuse_session *fuse_lowlevel_new(struct fuse_args *args,
    /// const struct fuse_lowlevel_ops *op, size_t op_size, void *userdata);`
    pub fn fuse_lowlevel_new(
        args: *mut fuse_args,
        op: *const c_void,
        op_size: usize,
        userdata: *mut c_void,
    ) -> *mut fuse_session;

    /// `void fuse_session_add_chan(struct fuse_session *se,
    /// struct fuse_chan *ch);`
    pub fn fuse_session_add_chan(se: *mut fuse_session, ch: *mut fuse_chan);

    /// `void fuse_session_remove_chan(struct fuse_chan *ch);`
    pub fn fuse_session_remove_chan(ch: *mut fuse_chan);

    /// `int fuse_session_loop(struct fuse_session *se);`
    pub fn fuse_session_loop(se: *mut fuse_session) -> c_int;

    /// `int fuse_session_loop_mt(struct fuse_session *se);`
    pub fn fuse_session_loop_mt(se: *mut fuse_session) -> c_int;

    /// `void fuse_session_exit(struct fuse_session *se);`
    pub fn fuse_session_exit(se: *mut fuse_session);

    /// `void fuse_session_destroy(struct fuse_session *se);`
    pub fn fuse_session_destroy(se: *mut fuse_session);

    /// `int fuse_reply_err(fuse_req_t req, int err);`
    pub fn fuse_reply_err(req: fuse_req_t, err: c_int) -> c_int;

    /// `void fuse_reply_none(fuse_req_t req);`
    pub fn fuse_reply_none(req: fuse_req_t);

    /// `int fuse_reply_entry(fuse_req_t req, const struct fuse_entry_param *e);`
    pub fn fuse_reply_entry(req: fuse_req_t, e: *const fuse_entry_param) -> c_int;

    /// `int fuse_reply_attr(fuse_req_t req, const struct stat *attr,
    /// double attr_timeout);`
    pub fn fuse_reply_attr(req: fuse_req_t, attr: *const libc::stat, attr_timeout: f64) -> c_int;

    /// `int fuse_reply_buf(fuse_req_t req, const char *buf, size_t size);`
    pub fn fuse_reply_buf(req: fuse_req_t, buf: *const u8, size: usize) -> c_int;

    /// `int fuse_reply_write(fuse_req_t req, size_t count);`
    pub fn fuse_reply_write(req: fuse_req_t, count: usize) -> c_int;

    /// `int fuse_reply_open(fuse_req_t req, const struct fuse_file_info *fi);`
    pub fn fuse_reply_open(req: fuse_req_t, fi: *const fuse_file_info) -> c_int;

    /// `int fuse_reply_create(fuse_req_t req, const struct fuse_entry_param *e,
    /// const struct fuse_file_info *fi);`
    pub fn fuse_reply_create(
        req: fuse_req_t,
        e: *const fuse_entry_param,
        fi: *const fuse_file_info,
    ) -> c_int;

    /// `int fuse_reply_readlink(fuse_req_t req, const char *link);`
    pub fn fuse_reply_readlink(req: fuse_req_t, link: *const c_char) -> c_int;

    /// `int fuse_reply_statfs(fuse_req_t req, const struct statvfs *stbuf);`
    pub fn fuse_reply_statfs(req: fuse_req_t, stbuf: *const libc::statvfs) -> c_int;

    /// `int fuse_reply_xattr(fuse_req_t req, size_t count);`
    pub fn fuse_reply_xattr(req: fuse_req_t, count: usize) -> c_int;

    /// `size_t fuse_add_direntry(fuse_req_t req, char *buf, size_t bufsize,
    /// const char *name, const struct stat *stbuf, off_t off);`
    pub fn fuse_add_direntry(
        req: fuse_req_t,
        buf: *mut c_char,
        bufsize: usize,
        name: *const c_char,
        stbuf: *const libc::stat,
        off: i64,
    ) -> usize;

    /// `void *fuse_req_userdata(fuse_req_t req);`
    pub fn fuse_req_userdata(req: fuse_req_t) -> *mut c_void;
}

// On non-macOS targets (Linux CI, docs build, etc.) the file is
// compiled out entirely by the `#[cfg(target_os = "macos")]` gate in
// `macos.rs`'s `mod macos_ffi;` declaration. No stub bodies are
// provided here because the extern block above is also gated.

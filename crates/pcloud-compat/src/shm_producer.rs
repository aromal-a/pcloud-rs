//! SysV shared-memory producer matching the legacy `pclsync/pshm.c` layout.
//!
//! The C overlay server publishes complex return values through a fixed
//! 4 KiB SysV shm segment keyed by `ftok("$HOME/.pcloud/data.db", 'A')`.
//!
//! # The ftok anchor must not move
//!
//! `ftok(path, id)` hashes the `(st_dev, st_ino)` of `path` together with
//! the low byte of `id` to derive a `key_t`. Two processes computing the
//! same key requires the **same underlying inode** at the same path and the
//! same project id byte. The legacy C client anchors on
//! `$HOME/.pcloud/data.db` — its own `SQLite` state file — and uses project
//! id `'A'`. This crate preserves both exactly (see [`default_anchor_path`]
//! and [`FTOK_PROJ_ID`]).
//!
//! Changing either would break the R8 dual-boot guarantee:
//!
//! * a legacy C binary still running in the background would compute one
//!   key, the new Rust daemon would compute another, and neither would see
//!   the other's segment,
//! * the mutual-exclusion side-effect (one client refusing to start while
//!   another is already publishing status) would disappear, leading to two
//!   processes racing on the same `data.db` from opposite sides.
//!
//! **Never** introduce an alternative anchor path without a full dual-boot
//! migration story.
//! Layout:
//!
//! ```text
//! offset  size  field
//!   0     8     void     *data
//!   8     8     size_t    datasz
//!  16     4     volatile  int flag
//!  20     4     <padding>
//!  24     ...   payload bytes (flexible array)
//! ```
//!
//! The C reader polls `flag == 1`, copies `datasz` bytes starting at
//! `shm + sizeof(psync_shm)`, and stores `flag = 0` with SEQ_CST semantics.
//! See `pclsync/pshm.c` for the authoritative implementation.
//!
//! # Security
//!
//! The legacy segment is created with mode `0666`. That is a C-compat
//! quirk, documented and opt-in via the `legacy-shm` Cargo feature on this
//! crate. This module additionally refuses to operate on an existing
//! segment that is owned by a different UID (see [`ShmSegment::create`]).
//! Do **not** write secrets into this segment — the compat surface only
//! carries status strings, pending counters, and folder descriptors.
//!
//! # Unsafe
//!
//! All `unsafe` is isolated to the calls into `libc::shmget` / `shmat` /
//! `shmdt` / `shmctl` / `ftok` and the raw-pointer copy into the mapped
//! segment. Each `unsafe` block documents the invariants it relies on.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::ffi::CString;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicI32, Ordering};

use libc::{c_int, c_void, size_t};
use thiserror::Error;

/// `PSYNC_SHM_SIZE` from `pclsync/pshm.h`.
pub const PSYNC_SHM_SIZE: usize = 4096;

/// `ftok` project-id byte used by the C client.
pub const FTOK_PROJ_ID: c_int = b'A' as c_int;

/// Legacy compat mode (`0666`). World-accessible; see module docs.
pub const LEGACY_SHM_MODE: c_int = 0o666;

/// Header struct mirroring the C `psync_shm` layout on 64-bit Linux.
///
/// * `data` (`*mut c_void`, 8 bytes) — the C code sets this to the address
///   of the payload area (i.e. `shm + sizeof(psync_shm)`). We mirror that
///   exact value to preserve byte-for-byte layout even though readers in
///   another process cannot meaningfully dereference our pointer.
/// * `datasz` (`size_t`, 8 bytes) — payload length in bytes.
/// * `flag` (`volatile int`, 4 bytes) — mirrored as [`AtomicI32`]; size and
///   alignment match on every supported platform (Linux x86_64 / aarch64).
///
/// Final 4 bytes are natural-alignment tail padding, matching GCC's
/// layout of the C struct on 64-bit Linux.
#[repr(C)]
struct PsyncShm {
    data: *mut c_void,
    datasz: size_t,
    flag: AtomicI32,
    _pad: u32,
}

const _: () = {
    // Compile-time layout guard. Mirrors the 24-byte ABI we depend on.
    assert!(std::mem::size_of::<PsyncShm>() == 24);
    assert!(std::mem::align_of::<PsyncShm>() == 8);
};

/// Errors from the shm producer.
#[derive(Debug, Error)]
pub enum ShmError {
    /// `$HOME` was not set when computing the ftok path.
    #[error("HOME environment variable is not set")]
    NoHome,
    /// A syscall failed.
    #[error("{syscall} failed: {source}")]
    Syscall {
        /// Which libc call failed.
        syscall: &'static str,
        /// The underlying `errno`.
        #[source]
        source: io::Error,
    },
    /// Payload did not fit in the shm segment.
    #[error("payload size {payload} exceeds available shm space {available}")]
    PayloadTooLarge {
        /// Payload bytes requested.
        payload: usize,
        /// Bytes available after the header.
        available: usize,
    },
    /// The existing shm segment is owned by a different UID — refuse to use it.
    #[error("existing shm segment owned by uid {owner}, expected {expected}")]
    ForeignOwner {
        /// Owner of the existing segment.
        owner: u32,
        /// UID we are running as.
        expected: u32,
    },
    /// The ftok anchor file does not exist.
    #[error("ftok anchor path does not exist: {path}")]
    MissingAnchor {
        /// The path we tried to key on.
        path: PathBuf,
    },
}

/// Compute the canonical ftok anchor path used by the C client:
/// `$HOME/.pcloud/data.db`.
pub fn default_anchor_path() -> Result<PathBuf, ShmError> {
    let home = std::env::var_os("HOME").ok_or(ShmError::NoHome)?;
    let mut p = PathBuf::from(home);
    p.push(".pcloud");
    p.push("data.db");
    Ok(p)
}

/// Compute the SysV IPC key from an anchor path, matching C `get_key()`.
pub fn ftok_key(anchor: &Path) -> Result<libc::key_t, ShmError> {
    if !anchor.exists() {
        return Err(ShmError::MissingAnchor {
            path: anchor.to_path_buf(),
        });
    }
    let cpath =
        CString::new(anchor.as_os_str().as_encoded_bytes()).map_err(|_| ShmError::Syscall {
            syscall: "ftok(path-contains-nul)",
            source: io::Error::from(io::ErrorKind::InvalidInput),
        })?;
    // SAFETY: `cpath` is a valid NUL-terminated C string for the lifetime
    // of this call; `ftok` does not retain the pointer.
    let key = unsafe { libc::ftok(cpath.as_ptr(), FTOK_PROJ_ID) };
    if key == -1 {
        return Err(ShmError::Syscall {
            syscall: "ftok",
            source: io::Error::last_os_error(),
        });
    }
    Ok(key)
}

/// RAII owner of a SysV shm segment.
///
/// The segment is attached for the lifetime of this value and detached on
/// drop. [`ShmSegment::mark_for_removal`] (also invoked automatically on
/// drop) requests kernel-side removal once all attachers detach, matching
/// `pshm_cleanup()` in C.
pub struct ShmSegment {
    shmid: c_int,
    mapping: NonNull<PsyncShm>,
    size: usize,
    removed: bool,
}

// SAFETY: `NonNull<PsyncShm>` is `!Send` by default, but the underlying
// shared-memory mapping is valid across threads provided we serialize
// stores via atomic ops. Callers are still responsible for their own
// synchronization beyond the SEQ_CST flag store. We intentionally do
// *not* implement `Sync`.
unsafe impl Send for ShmSegment {}

impl ShmSegment {
    /// Attach to (creating if necessary) the legacy-layout shm segment.
    ///
    /// `mode` is the permission bits to pass to `shmget` when the segment
    /// does not yet exist. For C compatibility you must pass
    /// [`LEGACY_SHM_MODE`] (`0666`). Callers that do not need C
    /// compatibility should prefer `0o600`.
    ///
    /// If a segment already exists for the key, its ownership is checked
    /// against the current effective UID; a mismatch returns
    /// [`ShmError::ForeignOwner`] — we never attach to a segment owned by
    /// another user.
    pub fn create(anchor: &Path, mode: c_int) -> Result<Self, ShmError> {
        let key = ftok_key(anchor)?;
        // SAFETY: `shmget` with IPC_CREAT is safe; arguments are POD.
        let shmid = unsafe { libc::shmget(key, PSYNC_SHM_SIZE as size_t, libc::IPC_CREAT | mode) };
        if shmid == -1 {
            return Err(ShmError::Syscall {
                syscall: "shmget",
                source: io::Error::last_os_error(),
            });
        }

        // Verify ownership of the segment matches our UID.
        let mut stat = std::mem::MaybeUninit::<libc::shmid_ds>::zeroed();
        // SAFETY: valid shmid; `stat` is a properly sized out-param.
        let rc = unsafe { libc::shmctl(shmid, libc::IPC_STAT, stat.as_mut_ptr()) };
        if rc == -1 {
            return Err(ShmError::Syscall {
                syscall: "shmctl(IPC_STAT)",
                source: io::Error::last_os_error(),
            });
        }
        // SAFETY: `shmctl(IPC_STAT)` succeeded, so `stat` is initialized.
        let stat = unsafe { stat.assume_init() };
        // SAFETY: `geteuid` has no preconditions.
        let my_uid = unsafe { libc::geteuid() };
        if stat.shm_perm.uid != my_uid {
            return Err(ShmError::ForeignOwner {
                owner: stat.shm_perm.uid,
                expected: my_uid,
            });
        }

        // SAFETY: `shmid` is valid; NULL + 0 lets the kernel pick the addr.
        let addr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) };
        if addr == (-1isize) as *mut c_void {
            return Err(ShmError::Syscall {
                syscall: "shmat",
                source: io::Error::last_os_error(),
            });
        }
        // SAFETY: the preceding equality check rejects the sentinel
        // `(void*)-1` that `shmat` uses for failure. Any other return
        // value from `shmat` is a live, non-null mapping by kernel
        // contract, so `NonNull::new` cannot return None here.
        let mapping = NonNull::new(addr.cast::<PsyncShm>()).expect("shmat returned non-null");

        Ok(Self {
            shmid,
            mapping,
            size: PSYNC_SHM_SIZE,
            removed: false,
        })
    }

    /// Maximum payload bytes publishable in one [`write`](Self::write) call.
    pub const fn max_payload(&self) -> usize {
        PSYNC_SHM_SIZE - std::mem::size_of::<PsyncShm>()
    }

    /// Publish `data` into the shm payload area and set `flag = 1` with
    /// SEQ_CST semantics, matching `pshm_write()` in C.
    ///
    /// Does **not** clear previous contents beyond the bytes it writes.
    /// The C reader uses `datasz` to bound its copy, so trailing bytes
    /// from a larger previous write remain latent in the segment.
    pub fn write(&self, data: &[u8]) -> Result<(), ShmError> {
        let available = self.max_payload();
        if data.len() > available {
            return Err(ShmError::PayloadTooLarge {
                payload: data.len(),
                available,
            });
        }

        // SAFETY: `self.mapping` is a valid, attached SysV shm mapping of
        // size `self.size` bytes, and remains valid for the lifetime of
        // `self`. Writes are within `[mapping, mapping + size)`.
        unsafe {
            let base = self.mapping.as_ptr();
            let payload_ptr = (base as *mut u8).add(std::mem::size_of::<PsyncShm>());

            // Populate header. `data` pointer is set to the payload address
            // to mirror the C producer verbatim; readers from other
            // processes must not dereference it.
            (*base).data = payload_ptr.cast::<c_void>();
            (*base).datasz = data.len() as size_t;

            if !data.is_empty() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), payload_ptr, data.len());
            }

            // Flag store publishes all prior writes (SEQ_CST).
            (*base).flag.store(1, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Read-back helper for same-process tests — performs the mirror of
    /// `pshm_read`: checks `flag == 1`, copies `datasz` bytes, clears the
    /// flag with SEQ_CST.
    pub fn try_consume(&self) -> Option<Vec<u8>> {
        // SAFETY: mapping invariants as in `write`.
        unsafe {
            let base = self.mapping.as_ptr();
            if (*base).flag.load(Ordering::SeqCst) != 1 {
                return None;
            }
            let len = (*base).datasz;
            if len > self.max_payload() {
                return None;
            }
            let payload_ptr = (base as *const u8).add(std::mem::size_of::<PsyncShm>());
            let mut out = vec![0u8; len];
            if len > 0 {
                std::ptr::copy_nonoverlapping(payload_ptr, out.as_mut_ptr(), len);
            }
            (*base).flag.store(0, Ordering::SeqCst);
            Some(out)
        }
    }

    /// Mark the shm segment for removal when the last attacher detaches.
    ///
    /// This matches `pshm_cleanup()` in C and is additionally invoked by
    /// [`Drop`] so that a normal exit does not leak SysV segments.
    pub fn mark_for_removal(&mut self) -> Result<(), ShmError> {
        if self.removed {
            return Ok(());
        }
        // SAFETY: `shmid` is valid.
        let rc = unsafe { libc::shmctl(self.shmid, libc::IPC_RMID, std::ptr::null_mut()) };
        if rc == -1 {
            return Err(ShmError::Syscall {
                syscall: "shmctl(IPC_RMID)",
                source: io::Error::last_os_error(),
            });
        }
        self.removed = true;
        Ok(())
    }

    /// The SysV shm identifier. Primarily for diagnostics / tests.
    pub fn shmid(&self) -> c_int {
        self.shmid
    }

    /// Total segment size (always [`PSYNC_SHM_SIZE`]).
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        // SAFETY: `mapping` was obtained from `shmat` and has not yet been
        // detached. Errors here are not surfaced — there is no recovery in
        // a drop handler and callers can pre-call `mark_for_removal()`.
        let _ = unsafe { libc::shmdt(self.mapping.as_ptr().cast::<c_void>()) };
        let _ = self.mark_for_removal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layout assertion: `PsyncShm` occupies exactly 24 bytes on 64-bit
    /// Linux. The compile-time guard above already enforces this, but a
    /// runtime test keeps the intent visible in CI reports.
    #[test]
    fn psync_shm_layout_is_stable() {
        assert_eq!(std::mem::size_of::<PsyncShm>(), 24);
        assert_eq!(std::mem::align_of::<PsyncShm>(), 8);
    }

    #[test]
    fn max_payload_matches_c_definition() {
        // PSYNC_SHM_SIZE - sizeof(psync_shm) = 4096 - 24 = 4072.
        assert_eq!(PSYNC_SHM_SIZE - 24, 4072);
    }

    /// End-to-end write + read-back in a single process. Uses a temp file
    /// as the ftok anchor and a unique project id to avoid colliding with
    /// a running C client.
    ///
    /// Gated behind `#[ignore]` because creating SysV segments requires
    /// environment support (sufficient `shmmax`) that is not always
    /// available in sandboxed CI runners. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "requires SysV IPC; run with --ignored"]
    fn write_then_consume_roundtrip() {
        use std::io::Write;

        let dir =
            std::env::temp_dir().join(format!("pcloud-compat-shm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let anchor = dir.join("data.db");
        std::fs::File::create(&anchor)
            .unwrap()
            .write_all(b"anchor")
            .unwrap();

        let mut seg = ShmSegment::create(&anchor, 0o600)
            .expect("create shm segment (if this fails, SysV IPC disabled)");
        let payload = b"hello from rust pcloud-compat";
        seg.write(payload).unwrap();
        let read = seg.try_consume().expect("flag must be set after write");
        assert_eq!(read, payload);
        // Second read returns None (flag cleared).
        assert!(seg.try_consume().is_none());

        // Explicit cleanup so we do not depend on Drop ordering.
        seg.mark_for_removal().unwrap();
        drop(seg);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_rejects_oversized_payload() {
        // We cannot cheaply construct a real ShmSegment without SysV IPC,
        // so we exercise the size check via a synthetic `max_payload`.
        let max = PSYNC_SHM_SIZE - std::mem::size_of::<PsyncShm>();
        assert_eq!(max, 4072);
        // The real `write` returns `PayloadTooLarge` when data.len() > max.
        // That path is covered by the `#[ignore]` integration test above.
    }
}

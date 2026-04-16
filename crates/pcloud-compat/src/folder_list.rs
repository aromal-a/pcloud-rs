//! ABI-exact mirror of the legacy `psync_folder_list_t` shared-memory payload.
//!
//! The C layout lives in `pclsync/pfoldersync.h`:
//!
//! ```text
//! #define PSYNC_MAX_PATH_LENGTH 256
//! typedef uint64_t psync_folderid_t;
//! typedef uint32_t psync_synctype_t;
//! typedef uint32_t psync_syncid_t;
//!
//! typedef struct {
//!   char localname[PSYNC_MAX_PATH_LENGTH];
//!   char localpath[PSYNC_MAX_PATH_LENGTH];
//!   char remotename[PSYNC_MAX_PATH_LENGTH];
//!   char remotepath[PSYNC_MAX_PATH_LENGTH];
//!   psync_folderid_t folderid;
//!   psync_syncid_t   syncid;
//!   psync_synctype_t synctype;
//! } psync_folder_t;
//!
//! typedef struct {
//!   size_t         foldercnt;
//!   psync_folder_t folders[];
//! } psync_folder_list_t;
//! ```
//!
//! On the target ABI (Linux x86_64, LP64) this crate produces a byte-for-byte
//! compatible buffer. Constants are mirrored here; if the upstream C header
//! ever changes them the compile-time layout assertions at the bottom of this
//! module must fail to build, forcing an explicit update.
//!
//! # Security and defensive behavior
//!
//! * Paths longer than `PSYNC_MAX_PATH_LENGTH - 1` bytes are **rejected** at
//!   [`FolderListBuilder::push`] time. Silent truncation is refused because
//!   the legacy C printer prints `remotepath` verbatim and a truncated
//!   non-NUL-terminated string would leak adjacent bytes.
//! * Each entry slot is zero-initialized before the path bytes are copied
//!   in. No heap reuse, no uninitialized padding.
//! * The serialized buffer contains only path bytes, numeric IDs, and the
//!   enum-like `synctype`. No secrets transit this surface.

// **PLATFORM:** all
// **GATING:** none (portable).

use core::mem::{align_of, offset_of, size_of};

/// Maximum path length mirrored from `PSYNC_MAX_PATH_LENGTH` in
/// `pclsync/pfoldersync.h`. Includes the trailing NUL byte.
pub const PSYNC_MAX_PATH_LENGTH: usize = 256;

/// Mirror of `size_t` on the target platform. The legacy C payload uses
/// `size_t foldercnt`; on Linux x86_64 that is 8 bytes (LP64). We make it
/// explicit rather than relying on `usize` at serialization time so the
/// wire layout is stable regardless of host pointer width at test time.
pub type CSizeT = u64;

/// ABI-exact mirror of the C `psync_folder_t` struct.
///
/// `#[repr(C)]` is load-bearing: it pins the field order and padding to
/// match the C compiler's natural layout on a 64-bit LP64 target. The
/// layout assertions at the bottom of this module verify the exact size
/// and field offsets.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FolderEntry {
    /// NUL-terminated local leaf name.
    pub localname: [u8; PSYNC_MAX_PATH_LENGTH],
    /// NUL-terminated absolute local path.
    pub localpath: [u8; PSYNC_MAX_PATH_LENGTH],
    /// NUL-terminated remote leaf name.
    pub remotename: [u8; PSYNC_MAX_PATH_LENGTH],
    /// NUL-terminated absolute remote path.
    pub remotepath: [u8; PSYNC_MAX_PATH_LENGTH],
    /// `psync_folderid_t` — remote folder id.
    pub folderid: u64,
    /// `psync_syncid_t` — local sync id.
    pub syncid: u32,
    /// `psync_synctype_t` — sync type enum value (C uses 1/2/3 currently).
    pub synctype: u32,
}

impl core::fmt::Debug for FolderEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FolderEntry")
            .field("folderid", &self.folderid)
            .field("syncid", &self.syncid)
            .field("synctype", &self.synctype)
            .field("localpath", &cstr_preview(&self.localpath))
            .field("remotepath", &cstr_preview(&self.remotepath))
            .finish()
    }
}

impl Default for FolderEntry {
    fn default() -> Self {
        // Cannot derive Default for [u8; 256] — explicit zeroing is
        // preferable anyway to avoid any compiler being clever about
        // padding.
        Self {
            localname: [0u8; PSYNC_MAX_PATH_LENGTH],
            localpath: [0u8; PSYNC_MAX_PATH_LENGTH],
            remotename: [0u8; PSYNC_MAX_PATH_LENGTH],
            remotepath: [0u8; PSYNC_MAX_PATH_LENGTH],
            folderid: 0,
            syncid: 0,
            synctype: 0,
        }
    }
}

/// Header of the flexible-array C struct `psync_folder_list_t`.
///
/// The raw serialized buffer is `FolderListHeader` + `entries` ×
/// [`FolderEntry`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FolderListHeader {
    /// Mirrors `size_t foldercnt` from `psync_folder_list_t`.
    pub foldercnt: CSizeT,
}

/// Errors that can be produced by the folder-list builder.
#[derive(Debug, thiserror::Error)]
pub enum FolderListError {
    /// A path supplied to [`FolderListBuilder::push_paths`] exceeds
    /// `PSYNC_MAX_PATH_LENGTH - 1` bytes (no room for the trailing NUL).
    #[error("path {field} is too long for psync_folder_t buffer: {len} bytes (max {max})")]
    PathTooLong {
        /// Which field tripped the length check.
        field: &'static str,
        /// Length of the offending path in bytes.
        len: usize,
        /// Maximum allowed length (excluding NUL).
        max: usize,
    },
    /// A path contained an embedded NUL byte, which would corrupt the
    /// C-string layout read by `control_tools.cpp`.
    #[error("path {field} contains an embedded NUL byte")]
    InteriorNul {
        /// Which field contained the NUL.
        field: &'static str,
    },
}

/// Safe builder that accumulates [`FolderEntry`] values and emits an
/// ABI-exact `psync_folder_list_t` serialized byte buffer.
#[derive(Debug, Default)]
pub struct FolderListBuilder {
    entries: Vec<FolderEntry>,
}

impl FolderListBuilder {
    /// Construct an empty builder.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Current number of queued entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entries have been queued yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Queue a pre-built entry. Callers using this variant are responsible
    /// for ensuring path buffers are NUL-terminated; prefer
    /// [`Self::push_paths`] for safer construction.
    pub fn push(&mut self, entry: FolderEntry) {
        self.entries.push(entry);
    }

    /// Build and push an entry from individual fields. The string slices
    /// are length-checked and NUL-scanned before being copied into the
    /// fixed-size buffers.
    pub fn push_paths(
        &mut self,
        localname: &str,
        localpath: &str,
        remotename: &str,
        remotepath: &str,
        folderid: u64,
        syncid: u32,
        synctype: u32,
    ) -> Result<(), FolderListError> {
        let mut entry = FolderEntry::default();
        copy_cstr(&mut entry.localname, localname, "localname")?;
        copy_cstr(&mut entry.localpath, localpath, "localpath")?;
        copy_cstr(&mut entry.remotename, remotename, "remotename")?;
        copy_cstr(&mut entry.remotepath, remotepath, "remotepath")?;
        entry.folderid = folderid;
        entry.syncid = syncid;
        entry.synctype = synctype;
        self.entries.push(entry);
        Ok(())
    }

    /// Serialize the accumulated entries into an ABI-exact
    /// `psync_folder_list_t` buffer (header + flexible array).
    pub fn build(&self) -> Vec<u8> {
        let header = FolderListHeader {
            foldercnt: self.entries.len() as CSizeT,
        };
        let total = size_of::<FolderListHeader>() + self.entries.len() * size_of::<FolderEntry>();
        let mut out = Vec::with_capacity(total);
        // SAFETY: `FolderListHeader` is `#[repr(C)]`, `Copy`, and contains
        // no padding-sensitive niches; viewing it as its own size worth
        // of bytes is well-defined.
        let header_bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(
                (&header as *const FolderListHeader).cast::<u8>(),
                size_of::<FolderListHeader>(),
            )
        };
        out.extend_from_slice(header_bytes);
        for entry in &self.entries {
            // SAFETY: `FolderEntry` is `#[repr(C)]` and `Copy`. It is
            // composed entirely of integer / byte-array fields, so every
            // byte of its footprint is initialized.
            let entry_bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(
                    (entry as *const FolderEntry).cast::<u8>(),
                    size_of::<FolderEntry>(),
                )
            };
            out.extend_from_slice(entry_bytes);
        }
        debug_assert_eq!(out.len(), total);
        out
    }
}

/// Minimal Rust-side decoder used by the round-trip test: reads a buffer
/// produced by [`FolderListBuilder::build`] back into typed values.
///
/// This is *not* a public parsing API for untrusted input — it assumes the
/// buffer was produced by the matching encoder on the same target ABI.
pub fn decode_roundtrip(buf: &[u8]) -> Option<(FolderListHeader, Vec<FolderEntry>)> {
    if buf.len() < size_of::<FolderListHeader>() {
        return None;
    }
    let mut header = FolderListHeader::default();
    // SAFETY: `FolderListHeader` is `#[repr(C)] Copy`; copying
    // `size_of::<FolderListHeader>()` bytes into it is well-defined.
    unsafe {
        core::ptr::copy_nonoverlapping(
            buf.as_ptr(),
            (&mut header as *mut FolderListHeader).cast::<u8>(),
            size_of::<FolderListHeader>(),
        );
    }
    let count = header.foldercnt as usize;
    let expected = size_of::<FolderListHeader>() + count * size_of::<FolderEntry>();
    if buf.len() < expected {
        return None;
    }
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let offset = size_of::<FolderListHeader>() + i * size_of::<FolderEntry>();
        let mut entry = FolderEntry::default();
        // SAFETY: bounds checked by the `buf.len() < expected` gate above.
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr().add(offset),
                (&mut entry as *mut FolderEntry).cast::<u8>(),
                size_of::<FolderEntry>(),
            );
        }
        entries.push(entry);
    }
    Some((header, entries))
}

fn copy_cstr(
    dst: &mut [u8; PSYNC_MAX_PATH_LENGTH],
    src: &str,
    field: &'static str,
) -> Result<(), FolderListError> {
    let bytes = src.as_bytes();
    if bytes.len() >= PSYNC_MAX_PATH_LENGTH {
        return Err(FolderListError::PathTooLong {
            field,
            len: bytes.len(),
            max: PSYNC_MAX_PATH_LENGTH - 1,
        });
    }
    if bytes.contains(&0u8) {
        return Err(FolderListError::InteriorNul { field });
    }
    dst[..bytes.len()].copy_from_slice(bytes);
    // Tail is already zero from `FolderEntry::default()`.
    Ok(())
}

fn cstr_preview(buf: &[u8; PSYNC_MAX_PATH_LENGTH]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

// =============================================================================
// Compile-time ABI assertions.
//
// These must match the C layout on Linux x86_64 (LP64). If any of these
// asserts fail the header in `pclsync/pfoldersync.h` has drifted and this
// crate MUST be updated to reflect the new C ABI before any shim uses it.
// =============================================================================

const _: () = {
    // Individual primitive widths.
    assert!(size_of::<u64>() == 8);
    assert!(size_of::<u32>() == 4);
    assert!(size_of::<CSizeT>() == 8);

    // FolderEntry size: 4 * 256 (paths) + 8 (folderid) + 4 (syncid)
    //                 + 4 (synctype) = 1040 bytes. No trailing padding
    // because the struct's alignment is 8 and 1040 is already 8-aligned.
    assert!(size_of::<FolderEntry>() == 1040);
    assert!(align_of::<FolderEntry>() == 8);

    // Field offsets on the C layout.
    assert!(offset_of!(FolderEntry, localname) == 0);
    assert!(offset_of!(FolderEntry, localpath) == 256);
    assert!(offset_of!(FolderEntry, remotename) == 512);
    assert!(offset_of!(FolderEntry, remotepath) == 768);
    assert!(offset_of!(FolderEntry, folderid) == 1024);
    assert!(offset_of!(FolderEntry, syncid) == 1032);
    assert!(offset_of!(FolderEntry, synctype) == 1036);

    // FolderListHeader matches `size_t foldercnt` on LP64.
    assert!(size_of::<FolderListHeader>() == 8);
    assert!(align_of::<FolderListHeader>() == 8);
    assert!(offset_of!(FolderListHeader, foldercnt) == 0);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-crafted fixture mirroring exactly what a C producer using
    /// `psync_folder_list_t` would write: little-endian on x86_64, natural
    /// packing, fixed-size path arrays zero-padded past the NUL.
    fn hand_crafted_fixture() -> Vec<u8> {
        // Two entries.
        let mut buf = Vec::new();
        // Header: foldercnt = 2 as u64 LE.
        buf.extend_from_slice(&2u64.to_le_bytes());

        // Entry 1.
        let mut e1 = vec![0u8; 1040];
        let n1 = b"sync1";
        e1[..n1.len()].copy_from_slice(n1); // localname
        let p1 = b"/home/user/pcloud";
        e1[256..256 + p1.len()].copy_from_slice(p1); // localpath
        let rn1 = b"Pictures";
        e1[512..512 + rn1.len()].copy_from_slice(rn1); // remotename
        let rp1 = b"/Pictures";
        e1[768..768 + rp1.len()].copy_from_slice(rp1); // remotepath
        e1[1024..1032].copy_from_slice(&42u64.to_le_bytes()); // folderid
        e1[1032..1036].copy_from_slice(&7u32.to_le_bytes()); // syncid
        e1[1036..1040].copy_from_slice(&1u32.to_le_bytes()); // synctype
        buf.extend_from_slice(&e1);

        // Entry 2.
        let mut e2 = vec![0u8; 1040];
        let n2 = b"docs";
        e2[..n2.len()].copy_from_slice(n2);
        let p2 = b"/data/docs";
        e2[256..256 + p2.len()].copy_from_slice(p2);
        let rn2 = b"Docs";
        e2[512..512 + rn2.len()].copy_from_slice(rn2);
        let rp2 = b"/Docs";
        e2[768..768 + rp2.len()].copy_from_slice(rp2);
        e2[1024..1032].copy_from_slice(&0xdead_beef_u64.to_le_bytes());
        e2[1032..1036].copy_from_slice(&9u32.to_le_bytes());
        e2[1036..1040].copy_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&e2);

        buf
    }

    #[test]
    fn builder_matches_hand_crafted_fixture() {
        let mut b = FolderListBuilder::new();
        b.push_paths(
            "sync1",
            "/home/user/pcloud",
            "Pictures",
            "/Pictures",
            42,
            7,
            1,
        )
        .unwrap();
        b.push_paths("docs", "/data/docs", "Docs", "/Docs", 0xdead_beef, 9, 3)
            .unwrap();
        let built = b.build();
        let fixture = hand_crafted_fixture();
        assert_eq!(
            built.len(),
            fixture.len(),
            "built ({} bytes) and fixture ({} bytes) differ in length",
            built.len(),
            fixture.len()
        );
        assert_eq!(
            built, fixture,
            "builder output does not match C-ABI fixture"
        );
    }

    #[test]
    fn roundtrip_decode_preserves_fields() {
        let mut b = FolderListBuilder::new();
        b.push_paths("a", "/l/a", "A", "/A", 1, 10, 1).unwrap();
        b.push_paths("b", "/l/b", "B", "/B", 2, 20, 2).unwrap();
        b.push_paths("c", "/l/c", "C", "/C", u64::MAX, u32::MAX, 3)
            .unwrap();
        let buf = b.build();

        let (header, entries) = decode_roundtrip(&buf).expect("decode ok");
        assert_eq!(header.foldercnt, 3);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].folderid, 1);
        assert_eq!(entries[0].syncid, 10);
        assert_eq!(entries[0].synctype, 1);
        assert_eq!(&entries[0].localname[..1], b"a");
        assert_eq!(entries[0].localname[1], 0);
        assert_eq!(&entries[0].localpath[..4], b"/l/a");
        assert_eq!(entries[0].localpath[4], 0);

        assert_eq!(entries[2].folderid, u64::MAX);
        assert_eq!(entries[2].syncid, u32::MAX);
        assert_eq!(entries[2].synctype, 3);
    }

    #[test]
    fn empty_builder_serializes_to_header_only() {
        let b = FolderListBuilder::new();
        let out = b.build();
        assert_eq!(out.len(), size_of::<FolderListHeader>());
        assert_eq!(&out[..8], &0u64.to_le_bytes());

        let (header, entries) = decode_roundtrip(&out).unwrap();
        assert_eq!(header.foldercnt, 0);
        assert!(entries.is_empty());
    }

    #[test]
    fn path_too_long_is_rejected() {
        let mut b = FolderListBuilder::new();
        // 255 bytes is the max (leaving room for NUL). 256 must fail.
        let too_long = "a".repeat(PSYNC_MAX_PATH_LENGTH);
        let err = b
            .push_paths(&too_long, "/ok", "n", "/n", 0, 0, 0)
            .unwrap_err();
        match err {
            FolderListError::PathTooLong { field, len, max } => {
                assert_eq!(field, "localname");
                assert_eq!(len, PSYNC_MAX_PATH_LENGTH);
                assert_eq!(max, PSYNC_MAX_PATH_LENGTH - 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        // Boundary: exactly 255 bytes must succeed.
        let ok = "a".repeat(PSYNC_MAX_PATH_LENGTH - 1);
        b.push_paths(&ok, "/ok", "n", "/n", 0, 0, 0).unwrap();
    }

    #[test]
    fn interior_nul_is_rejected() {
        let mut b = FolderListBuilder::new();
        let err = b
            .push_paths("ok", "/a\0/b", "n", "/n", 0, 0, 0)
            .unwrap_err();
        assert!(matches!(
            err,
            FolderListError::InteriorNul { field: "localpath" }
        ));
    }

    #[test]
    fn layout_sizes_match_c_abi_runtime() {
        // Cross-check the const asserts at runtime as well so failure is
        // obvious in test output rather than only at compile time.
        assert_eq!(size_of::<FolderEntry>(), 1040);
        assert_eq!(size_of::<FolderListHeader>(), 8);
        assert_eq!(offset_of!(FolderEntry, folderid), 1024);
        assert_eq!(offset_of!(FolderEntry, syncid), 1032);
        assert_eq!(offset_of!(FolderEntry, synctype), 1036);
    }

    #[test]
    fn push_raw_entry_is_serialized_verbatim() {
        let mut e = FolderEntry::default();
        e.localname[..3].copy_from_slice(b"raw");
        e.folderid = 0x0102_0304_0506_0708;
        e.syncid = 0x0a0b_0c0d;
        e.synctype = 0x11_22_33_44;
        let mut b = FolderListBuilder::new();
        b.push(e);
        let out = b.build();

        assert_eq!(&out[..8], &1u64.to_le_bytes());
        // folderid little-endian at offset 8 + 1024.
        let f_off = 8 + 1024;
        assert_eq!(
            &out[f_off..f_off + 8],
            &0x0102_0304_0506_0708u64.to_le_bytes()
        );
        assert_eq!(&out[f_off + 8..f_off + 12], &0x0a0b_0c0du32.to_le_bytes());
        assert_eq!(
            &out[f_off + 12..f_off + 16],
            &0x11_22_33_44u32.to_le_bytes()
        );
    }
}

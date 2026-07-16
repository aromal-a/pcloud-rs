//! Error taxonomy for the FUSE adapter layer.
//!
//! FUSE operations ultimately return an `errno` integer to the kernel. This
//! module centralises the mapping from structured errors to `errno` so that
//! backend implementations produce consistent kernel-visible behaviour.

// **PLATFORM:** all
// **GATING:** none (portable).

use thiserror::Error;

/// `libc::ENOENT` — no such file or directory.
pub const ENOENT: i32 = 2;
/// `libc::EIO` — input/output error.
pub const EIO: i32 = 5;
/// `libc::EACCES` — permission denied.
pub const EACCES: i32 = 13;
/// `libc::ENOTDIR` — not a directory.
pub const ENOTDIR: i32 = 20;
/// `libc::EINVAL` — invalid argument.
pub const EINVAL: i32 = 22;
/// `libc::ENOSPC` — no space left on device or configured staging budget.
pub const ENOSPC: i32 = 28;
/// `libc::EROFS` — read-only file system. Used by write-op trait stubs
/// whose backing transport does not (yet) support the mutation, so the
/// kernel reports the filesystem as read-only rather than hanging or
/// silently dropping writes.
pub const EROFS: i32 = 30;

/// High-level failure modes the FUSE adapter distinguishes. Transport-level
/// errors are coarsened because they are reported to the kernel as `EIO`;
/// richer detail is preserved in log surfaces outside this crate.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FsError {
    /// No such file or directory; maps to [`ENOENT`].
    #[error("no such file or directory")]
    NotFound,
    /// Permission denied; maps to [`EACCES`].
    #[error("permission denied")]
    PermissionDenied,
    /// Target exists but is not a directory; maps to [`ENOTDIR`].
    #[error("not a directory")]
    NotDirectory,
    /// Invalid argument supplied by the caller; maps to [`EINVAL`].
    #[error("invalid argument")]
    Invalid,
    /// Generic input/output failure; maps to [`EIO`].
    #[error("i/o error")]
    Io,
    /// The 64-bit inode number space is exhausted. This should never occur in
    /// practice (would require 2^64 allocations), but must not panic the daemon.
    /// Maps to [`EIO`].
    #[error("inode number space exhausted")]
    InodeSpaceExhausted,
    /// Transport-level failure with a diagnostic message. Reported to the
    /// kernel as [`EIO`]; the string is kept for logging only and is not
    /// surfaced via the errno surface.
    #[error("transport failure: {0}")]
    Transport(String),
}

impl FsError {
    /// Construct a [`FsError::Transport`] wrapping a caller-supplied
    /// diagnostic message. The message is only used for logging; it never
    /// reaches the kernel.
    pub fn transport(msg: impl Into<String>) -> Self {
        Self::Transport(msg.into())
    }

    /// Convert a pCloud protocol result code into an [`FsError`].
    ///
    /// Mapping chosen to match POSIX expectations for a FUSE filesystem:
    /// - 2002 / 2005 / 2009 → `NotFound` (ENOENT)
    /// - 1004 / 1027 / 2003 / 2004 / 2014 → `PermissionDenied` (EACCES)
    /// - anything else → `Io` (EIO)
    pub fn from_pcloud_result(result: u64, _message: Option<String>) -> Self {
        match result {
            // Common pCloud "path not found" family.
            2002 | 2005 | 2009 | 2010 => Self::NotFound,
            // Permission and auth-related rejections.
            1004 | 1027 | 2003 | 2004 | 2014 => Self::PermissionDenied,
            _ => Self::Io,
        }
    }

    /// Map this error to the POSIX `errno` value reported to the FUSE
    /// kernel layer. Transport errors are coarsened to [`EIO`] because the
    /// richer context is not meaningful to userspace callers going through
    /// the kernel interface.
    #[must_use]
    pub fn to_errno(&self) -> i32 {
        match self {
            Self::NotFound => ENOENT,
            Self::PermissionDenied => EACCES,
            Self::NotDirectory => ENOTDIR,
            Self::Invalid => EINVAL,
            Self::Io | Self::Transport(_) | Self::InodeSpaceExhausted => EIO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_not_found_result_codes() {
        for code in [2002u64, 2005, 2009, 2010] {
            assert_eq!(
                FsError::from_pcloud_result(code, None).to_errno(),
                ENOENT,
                "code {code} must map to ENOENT"
            );
        }
    }

    #[test]
    fn maps_permission_result_codes() {
        for code in [1004u64, 1027, 2003, 2004, 2014] {
            assert_eq!(
                FsError::from_pcloud_result(code, None).to_errno(),
                EACCES,
                "code {code} must map to EACCES"
            );
        }
    }

    #[test]
    fn unknown_result_code_falls_back_to_io() {
        assert_eq!(FsError::from_pcloud_result(9999, None).to_errno(), EIO);
    }

    #[test]
    fn transport_is_eio() {
        assert_eq!(FsError::transport("boom").to_errno(), EIO);
    }

    #[test]
    fn enospc_constant_matches_posix() {
        assert_eq!(ENOSPC, libc::ENOSPC);
    }
}

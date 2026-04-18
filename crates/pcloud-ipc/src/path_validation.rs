//! Local sync-root path validation helpers.
//!
//! Provides a single entry point — [`validate_local_sync_path`] — that
//! enforces the security invariants required before a path is accepted as
//! a sync root:
//!
//! - No NUL bytes (would split the path at the OS level differently from
//!   how Rust displays it, enabling path-confusion attacks).
//! - No `..` components (parent-directory traversal escape).
//! - Not a symlink at the root level (symlink-following would let an
//!   unprivileged user redirect the sync root to an arbitrary directory
//!   after the validation check, a classic TOCTOU pattern).
//! - Must be valid UTF-8 (the IPC wire format and all downstream
//!   operations assume UTF-8 paths; non-UTF-8 input is rejected early
//!   rather than silently mangled).
//!
//! ## Security rationale
//!
//! Without these checks a hostile or malformed IPC client can submit
//! `local_path` values like `/home/user/../../etc/passwd` or paths
//! containing embedded NUL bytes to confuse `canonicalize` and the store
//! layer. The daemon canonicalizes paths *after* validation, so the
//! sequence is: validate → canonicalize → duplicate/nested-root check →
//! persist. Any deviation that passes `validate_local_sync_path` but
//! fails the OS-level `canonicalize` is caught by the caller.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;
use std::path::{Component, Path};

/// Maximum accepted byte length for a local sync-root path.
///
/// 4 096 bytes is a conservative cross-platform ceiling (Linux `PATH_MAX`
/// is 4 096; macOS is 1 024; Windows is 260 without the long-path opt-in).
/// Paths longer than this are rejected early so that later OS syscalls do
/// not silently truncate or fail with an opaque error.
pub const MAX_SYNC_PATH_LEN: usize = 4096;

/// Validates that a path is safe to use as a local sync root.
///
/// # Errors
///
/// Returns a [`PathValidationError`] describing the first constraint
/// violation found. The order of checks is:
///
/// 1. Non-UTF-8 encoding
/// 2. Path length exceeds [`MAX_SYNC_PATH_LEN`]
/// 3. NUL byte
/// 4. `..` component
/// 5. Symlink at the root level (checked only when the path already exists)
pub fn validate_local_sync_path(path: &Path) -> Result<(), PathValidationError> {
    // 1. Reject non-UTF-8 paths: the IPC wire and all downstream helpers
    //    assume UTF-8; we surface the error early rather than mangling.
    let s = path.to_str().ok_or(PathValidationError::NonUtf8)?;

    // 2. Reject paths that exceed the OS-agnostic maximum length. Very long
    //    paths can cause OS syscalls (open, stat, canonicalize) to fail with
    //    opaque ENAMETOOLONG errors or, on some implementations, silently
    //    truncate the path to a different file. We reject early with a clear
    //    message instead.
    if s.len() > MAX_SYNC_PATH_LEN {
        return Err(PathValidationError::TooLong);
    }

    // 2. NUL bytes: a path with an embedded NUL would be silently
    //    truncated by many C APIs, allowing a mismatched view of the
    //    path between the Rust layer and any FFI/syscall boundary.
    if s.contains('\0') {
        return Err(PathValidationError::NulByte);
    }

    // 3. `..` components: parent-directory traversal could escape the
    //    intended subtree. Canonicalization happens later; we catch the
    //    raw input here before any resolution occurs.
    for component in path.components() {
        if component == Component::ParentDir {
            return Err(PathValidationError::DotDot);
        }
    }

    // 4. Symlink at the root level: if the path already exists on disk and
    //    is a symlink, the underlying target could be swapped after
    //    validation (TOCTOU). Reject it and require the caller to supply a
    //    real directory. Only the path itself is checked — symlinks inside
    //    the tree are handled by the walker with follow-symlink limits.
    if path.exists() {
        let meta = std::fs::symlink_metadata(path).map_err(PathValidationError::Io)?;
        if meta.file_type().is_symlink() {
            return Err(PathValidationError::Symlink);
        }
    }

    Ok(())
}

/// Error variants returned by [`validate_local_sync_path`].
#[derive(Debug, thiserror::Error)]
pub enum PathValidationError {
    /// The path contains a NUL byte (`\0`), which would be silently
    /// truncated by C-library functions.
    #[error("path contains a NUL byte")]
    NulByte,
    /// The path contains a `..` component, which could escape the
    /// intended sync-root subtree.
    #[error("path contains a `..` component")]
    DotDot,
    /// The path is a symlink at the root level; sync roots must be real
    /// directories to prevent TOCTOU attacks after validation.
    #[error("path is a symlink — sync roots must be real directories")]
    Symlink,
    /// The path bytes are not valid UTF-8.
    #[error("path is not valid UTF-8")]
    NonUtf8,
    /// The path exceeds [`MAX_SYNC_PATH_LEN`] bytes.
    ///
    /// Very long paths can cause OS syscalls to fail with opaque
    /// `ENAMETOOLONG` errors or silently truncate to a different target.
    /// We reject them early with a clear, actionable message.
    #[error("path too long (max {MAX_SYNC_PATH_LEN} bytes)")]
    TooLong,
    /// An I/O error occurred while inspecting the path metadata.
    #[error("I/O error checking path: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn valid_absolute_path_accepted() {
        // An absolute path that does not exist on disk (existence check is
        // skipped) and has no unsafe components must be accepted.
        let p = PathBuf::from("/home/user/sync-root");
        assert!(validate_local_sync_path(&p).is_ok());
    }

    #[test]
    fn relative_path_without_dotdot_accepted() {
        // A relative path with no `..` is valid at this layer (callers
        // are expected to canonicalize afterwards).
        let p = PathBuf::from("relative/path");
        assert!(validate_local_sync_path(&p).is_ok());
    }

    #[test]
    fn rejects_path_exceeding_max_length() {
        // Build a path that is exactly one byte longer than the maximum.
        let long_segment = "a".repeat(MAX_SYNC_PATH_LEN + 1);
        let p = PathBuf::from(format!("/{long_segment}"));
        assert!(
            matches!(
                validate_local_sync_path(&p),
                Err(PathValidationError::TooLong)
            ),
            "expected TooLong for a {}-byte path",
            p.to_str().unwrap().len()
        );
    }

    #[test]
    fn accepts_path_at_max_length() {
        // A path of exactly MAX_SYNC_PATH_LEN bytes must be accepted by
        // the length check (it will fail the existence check on disk, but
        // that is a different error path).
        let segment = "a".repeat(MAX_SYNC_PATH_LEN - 1); // subtract 1 for leading "/"
        let p = PathBuf::from(format!("/{segment}"));
        // Length check should pass (existence check skipped for non-existent paths).
        assert!(
            p.to_str().unwrap().len() <= MAX_SYNC_PATH_LEN,
            "test setup error: path should be within limit"
        );
        // The path does not exist so symlink check is skipped; no TooLong.
        let result = validate_local_sync_path(&p);
        assert!(
            !matches!(result, Err(PathValidationError::TooLong)),
            "a path within the limit must not produce TooLong"
        );
    }

    #[test]
    fn rejects_dotdot_component() {
        let cases = [
            PathBuf::from("/home/user/../etc"),
            PathBuf::from("../escape"),
            PathBuf::from("/a/b/../../c"),
        ];
        for p in &cases {
            assert!(
                matches!(
                    validate_local_sync_path(p),
                    Err(PathValidationError::DotDot)
                ),
                "expected DotDot for {p:?}"
            );
        }
    }

    #[test]
    fn rejects_nul_byte() {
        // Paths with embedded NUL are not representable via `PathBuf::from`
        // on most platforms, but we can test the UTF-8 guard indirectly —
        // the NUL check requires UTF-8 to succeed first.
        // Build a path that contains a NUL via raw OsStr:
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let raw = b"/home/user/\0evil";
        let p = PathBuf::from(OsStr::from_bytes(raw));
        // to_str() may succeed (the bytes are otherwise UTF-8 except for NUL).
        if let Some(s) = p.to_str() {
            assert!(s.contains('\0'));
            let result = validate_local_sync_path(&p);
            assert!(
                matches!(result, Err(PathValidationError::NulByte)),
                "expected NulByte, got {result:?}"
            );
        }
        // If to_str() returns None (platform rejects NUL in strings),
        // NonUtf8 is also an acceptable error.
    }

    #[test]
    fn rejects_symlink_when_path_exists() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real_dir");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link_to_dir");
        symlink(&target, &link).unwrap();

        let result = validate_local_sync_path(&link);
        assert!(
            matches!(result, Err(PathValidationError::Symlink)),
            "expected Symlink error for a symlink path, got {result:?}"
        );
    }

    #[test]
    fn accepts_real_directory_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let result = validate_local_sync_path(dir.path());
        assert!(
            result.is_ok(),
            "a real, existing directory must pass validation, got {result:?}"
        );
    }
}

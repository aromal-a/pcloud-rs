//! Backwards-compatibility shim for the legacy file-based auth vault API.
//!
//! The real implementation now lives in [`crate::vault::file`]. This
//! module re-exports the historical free-function surface (`load_token`,
//! `store_token`, `clear_token`) plus the [`AuthVaultError`] type so the
//! existing call sites in `bootstrap.rs`, `runtime.rs`, and
//! `refresh_loop.rs` keep compiling unchanged while higher layers migrate
//! to the cross-platform [`crate::vault::PlatformVault`] trait.
//!
//! # Deprecation
//!
//! New code should depend on [`crate::vault::PlatformVault`] and
//! [`crate::vault::FileVault`] directly instead of these free functions.
//! Once every internal caller has been ported the free-function surface
//! will be removed.

use std::io;

use thiserror::Error;

/// Errors surfaced by the file-based auth vault.
///
/// Re-exported from [`crate::vault`] via [`crate::vault::VaultError`].
#[derive(Debug, Error)]
pub enum AuthVaultError {
    /// Underlying filesystem I/O failed.
    #[error("vault io failed: {0}")]
    Io(#[from] io::Error),
    /// The vault file exists but its metadata failed the security check
    /// (wrong owner, world/group-accessible mode, non-regular file, or
    /// non-UTF8 contents).
    #[error("vault file metadata was insecure: {0}")]
    InsecureMetadata(&'static str),
    /// The file vault backend is not supported on this platform.
    ///
    /// On Windows, NTFS ACLs cannot be applied portably without calling
    /// into the Win32 security API. Use the DPAPI backend instead
    /// (`PCLOUD_VAULT=dpapi`). This error is returned at runtime by
    /// `store_token` on Windows so callers get a clear diagnostic rather
    /// than silently writing a world-accessible file.
    #[error("file vault not supported on this platform: {0}")]
    UnsupportedPlatform(String),
}

// Re-export the real file-vault free functions under their historical
// paths so existing `use crate::auth_vault::{load_token, store_token,
// clear_token};` imports keep resolving.
//
// Audit 05 §2 LOW-2.6 confirmation: `vault/file.rs` enforces 0600 file mode,
// 0700 parent dir mode, and UNIX owner-equality in `validate_vault_file` (lines
// 215-245 of that file). The shim here does not weaken any of those checks.
pub use crate::vault::file::{clear_token, load_token, store_token};

//! Runtime directory permission policy attached to a
//! [`crate::ConfigProfile`].
//!
//! Every field is the Unix mode (octal, e.g. `0o700`) that the daemon will
//! enforce with `chmod` on the matching managed directory before using it.
//! [`RuntimePolicy::validate`] rejects any mode with group/other bits set
//! — `0o700` or stricter is the only acceptable posture.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// Unix modes applied to each managed directory.
///
/// Persists in the envelope's `profile.runtime` object. All four fields are
/// required by the schema; there are no env-var overrides.
/// [`crate::ConfigProfile::secure_defaults`] sets every mode to `0o700`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePolicy {
    /// Unix mode for
    /// [`paths::ManagedPaths::config_dir`](crate::paths::ManagedPaths::config_dir).
    /// Default: `0o700`. Valid values: any mode with `mode & 0o077 == 0`
    /// (owner-only). **Security:** this directory holds the profile
    /// envelope and the auth-token vault — any group/other bit is
    /// rejected by [`Self::validate`]. Example: `config_dir_mode = 0o700`.
    pub config_dir_mode: u32,
    /// Unix mode for the runtime dir parent of the IPC socket
    /// ([`paths::ManagedPaths::runtime_dir`](crate::paths::ManagedPaths::runtime_dir)).
    /// Default: `0o700`. Valid values: any mode with `mode & 0o077 == 0`.
    /// **Security:** loosening this exposes the local IPC control socket
    /// to other local users, letting them speak the daemon protocol.
    /// Example: `socket_dir_mode = 0o700`.
    pub socket_dir_mode: u32,
    /// Unix mode for
    /// [`paths::ManagedPaths::state_dir`](crate::paths::ManagedPaths::state_dir).
    /// Default: `0o700`. Valid values: any mode with `mode & 0o077 == 0`.
    /// **Security:** houses the SQLite store, audit log, and sync DB —
    /// world/group reads would leak sync metadata and tokens. Example:
    /// `state_dir_mode = 0o700`.
    pub state_dir_mode: u32,
    /// Unix mode for
    /// [`paths::ManagedPaths::cache_dir`](crate::paths::ManagedPaths::cache_dir).
    /// Default: `0o700`. Valid values: any mode with `mode & 0o077 == 0`.
    /// **Security:** FUSE staging and thumbnails live here; allowing
    /// group/other reads exposes plaintext of encrypted content that has
    /// transited staging. Example: `cache_dir_mode = 0o700`.
    pub cache_dir_mode: u32,
}

impl RuntimePolicy {
    /// Reject any mode with group/other permission bits set.
    ///
    /// Returns [`ConfigError::InsecureMode`] on the first offending field.
    /// Managed directories hold auth tokens, crypto material, and IPC
    /// sockets — there is no legitimate reason for them to be group- or
    /// world-accessible.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, mode) in [
            ("config_dir_mode", self.config_dir_mode),
            ("socket_dir_mode", self.socket_dir_mode),
            ("state_dir_mode", self.state_dir_mode),
            ("cache_dir_mode", self.cache_dir_mode),
        ] {
            if mode & 0o077 != 0 {
                return Err(ConfigError::InsecureMode { field, mode });
            }
        }

        Ok(())
    }
}

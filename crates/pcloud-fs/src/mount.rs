//! Mount policy types: options (`allow_other`, `read_only`) and their
//! validation rules that gate whether a mount configuration is safe to
//! pass to the platform-specific mount service. Consumed by
//! `FilesystemShell::validate_mount_policy` and `mount_service`.
//!
//! Portable types; platform coupling lives in `mount_service` and
//! `platform/*`.

use serde::{Deserialize, Serialize};

/// Errors raised by [`MountService::validate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MountPolicyError {
    /// `allow_other=true` is only permitted when the mount is also
    /// `read_only=true`. Allowing arbitrary local users to write through
    /// a pCloud mount is considered unsafe and is rejected here.
    AllowOtherRequiresReadOnly,
}

/// Declarative mount policy for the FUSE adapter. This type is a pure
/// data record; it does not perform any syscalls. The platform-specific
/// mount drivers consume it after [`validate`](Self::validate) succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountService {
    /// When `true`, the kernel exposes the mount to other local users in
    /// addition to the user that performed the mount. Implies the mount
    /// must be read-only.
    pub allow_other: bool,
    /// When `true`, the mount refuses all write operations at the adapter
    /// layer regardless of backend state.
    pub read_only: bool,
}

impl MountService {
    /// Apply the mount-policy rules:
    ///
    /// - `allow_other && !read_only` is rejected with
    ///   [`MountPolicyError::AllowOtherRequiresReadOnly`].
    ///
    /// Every other combination is accepted.
    pub fn validate(&self) -> Result<(), MountPolicyError> {
        if self.allow_other && !self.read_only {
            return Err(MountPolicyError::AllowOtherRequiresReadOnly);
        }
        Ok(())
    }

    /// Human-readable summary of the effective mode (`"read-only"` or
    /// `"read-write"`), for logging and diagnostics.
    #[must_use]
    pub fn effective_mode(&self) -> &'static str {
        if self.read_only {
            "read-only"
        } else {
            "read-write"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MountPolicyError, MountService};

    #[test]
    fn rejects_allow_other_writeable_mounts() {
        let mount = MountService {
            allow_other: true,
            read_only: false,
        };

        assert_eq!(
            mount.validate(),
            Err(MountPolicyError::AllowOtherRequiresReadOnly)
        );
    }

    #[test]
    fn allows_private_writeable_mounts() {
        let mount = MountService {
            allow_other: false,
            read_only: false,
        };

        assert_eq!(mount.validate(), Ok(()));
        assert_eq!(mount.effective_mode(), "read-write");
    }
}

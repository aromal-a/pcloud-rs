//! # IPC peer identity
//!
//! **PLATFORM: all.**
//! **GATING: none — this module defines the portable [`PeerIdentity`]
//! value type and the `current_effective_uid()` helper. Peer-credential
//! *recovery* is platform-specific and lives in [`crate::platform`]:
//! Linux → `SO_PEERCRED`, BSD/macOS → `getpeereid`, Windows → named pipe
//! SID check (stub).**
//!
//! `current_effective_uid()` uses `libc::geteuid` and compiles on any
//! Unix target; on Windows a different notion of "current user identity"
//! (SID) is required and will be added alongside the named-pipe
//! backend.

use serde::{Deserialize, Serialize};

/// Identity of the peer at the other end of an IPC connection, recovered
/// from `SO_PEERCRED` at accept time. The daemon uses `uid` to enforce
/// owner-only access and `pid` for correlated audit logging.
///
/// ```
/// use pcloud_ipc::auth::PeerIdentity;
/// let peer = PeerIdentity { uid: 1000, pid: 4242 };
/// assert!(peer.matches_owner(1000));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerIdentity {
    /// UNIX user id of the peer process as observed by
    /// `SO_PEERCRED` / `getpeereid(3)` at connection-accept time. Used
    /// for the owner-only authorization decision performed by
    /// [`Self::matches_owner`].
    pub uid: u32,
    /// Process id of the peer as reported by the platform. On Linux this
    /// comes from `SO_PEERCRED`; on BSD/macOS it is synthesized as `0`
    /// because `getpeereid(3)` does not expose it; on Windows it is
    /// recovered via `GetNamedPipeClientProcessId`. Carried for audit
    /// correlation only — never used for authorization.
    pub pid: u32,
}

impl PeerIdentity {
    /// Returns `true` when the peer uid matches the daemon owner uid —
    /// the only authorization check performed by the IPC layer.
    ///
    /// ```
    /// use pcloud_ipc::auth::PeerIdentity;
    /// let peer = PeerIdentity { uid: 1000, pid: 1 };
    /// assert!(peer.matches_owner(1000));
    /// assert!(!peer.matches_owner(0));
    /// ```
    #[must_use]
    pub fn matches_owner(&self, owner_uid: u32) -> bool {
        self.uid == owner_uid
    }
}

/// Returns the process's current effective UID. Used by the daemon and
/// CLI to pin IPC ownership to the invoking user.
///
/// ```
/// // Value depends on the runner, so we just assert the call succeeds.
/// let _uid = pcloud_ipc::auth::current_effective_uid();
/// ```
#[must_use]
pub fn current_effective_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and simply returns the
        // effective uid.
        unsafe { libc::geteuid() }
    }
    #[cfg(windows)]
    {
        // Windows has no Unix-style uid. The caller's security principal
        // is identified by SID, not uid; peer authentication on Windows
        // goes through `platform::windows::peer_uid` which returns a
        // stable hash of the TokenUser SID. Returning 0 here is a
        // placeholder for code paths that haven't yet been refactored
        // off this function on Windows (tracked under bd-xplat-windows).
        0
    }
}

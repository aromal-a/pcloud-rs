// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        pub struct $name(
            /// Raw `u64` value. Exposed as `pub` for ergonomic pattern
            /// matching; prefer [`Self::new`] / [`Self::get`] in new code.
            pub u64,
        );

        impl $name {
            /// Build a new id from a raw `u64`. `const` so ids can be
            /// used in constant contexts (e.g. test tables).
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// Extract the underlying `u64`. `const` to avoid forcing
            /// callers to match on the tuple struct.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }
    };
}

define_id!(
    /// Stable per-user identifier returned by the pCloud API. Used as
    /// the cache partition key on the daemon.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::UserId;
    /// let u = UserId::new(42);
    /// assert_eq!(u.get(), 42);
    /// ```
    UserId
);
define_id!(
    /// Sync-root identifier. Allocated locally by the daemon when a
    /// new sync pair is registered; persists across restarts via the
    /// store. This is the primary routing key for
    /// [`crate::sync::SyncCandidate`] and
    /// [`crate::sync::PlannedOperation`].
    ///
    /// # Serde invariant
    ///
    /// Serialized transparently as a `u64`; roundtrips losslessly
    /// through `serde_json` and any numeric-preserving format.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::SyncId;
    /// let a = SyncId::new(1);
    /// let b = SyncId::new(2);
    /// assert!(a < b);
    /// let json = serde_json::to_string(&a).unwrap();
    /// let back: SyncId = serde_json::from_str(&json).unwrap();
    /// assert_eq!(a, back);
    /// ```
    SyncId
);
define_id!(
    /// Server-side file id (`fileid`) from the binprotocol. Unique per
    /// account; survives renames but not delete/recreate.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::RemoteFileId;
    /// const EXPECTED: RemoteFileId = RemoteFileId::new(9000);
    /// assert_eq!(EXPECTED.get(), 9000);
    /// ```
    RemoteFileId
);
define_id!(
    /// Server-side folder id (`folderid`). `0` is the account root on
    /// the pCloud API; ids above `0` are user-visible folders.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::RemoteFolderId;
    /// // Root folder on pCloud is always id 0.
    /// assert_eq!(RemoteFolderId::new(0).get(), 0);
    /// ```
    RemoteFolderId
);
define_id!(
    /// Resumable upload-session id returned by `upload_create`. Feeds
    /// `upload_write` / `upload_save` to build a file in multiple
    /// round-trips without holding the whole payload in memory.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::UploadSessionId;
    /// let s = UploadSessionId::new(12345);
    /// let j = serde_json::to_string(&s).unwrap();
    /// let back: UploadSessionId = serde_json::from_str(&j).unwrap();
    /// assert_eq!(s, back);
    /// ```
    UploadSessionId
);
define_id!(
    /// Opaque `diff` stream cursor used to resume incremental diff
    /// polling after a disconnect. Always monotonic for a given user.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::DiffCursor;
    /// let prev = DiffCursor::new(100);
    /// let next = DiffCursor::new(101);
    /// assert!(next > prev);
    /// ```
    DiffCursor
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_and_get_roundtrip() {
        let u = UserId::new(42);
        assert_eq!(u.get(), 42);
    }

    #[test]
    fn zero_boundary() {
        assert_eq!(SyncId::new(0).get(), 0);
    }

    #[test]
    fn u64_max_boundary() {
        assert_eq!(RemoteFileId::new(u64::MAX).get(), u64::MAX);
    }

    #[test]
    fn ids_are_ordered() {
        let a = RemoteFolderId::new(1);
        let b = RemoteFolderId::new(2);
        assert!(a < b);
    }

    #[test]
    fn ids_serde_roundtrip() {
        let id = UploadSessionId::new(12345);
        let j = serde_json::to_string(&id).unwrap();
        let back: UploadSessionId = serde_json::from_str(&j).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn distinct_id_types_same_value_equal_inside_type() {
        assert_eq!(DiffCursor::new(7), DiffCursor::new(7));
        assert_ne!(DiffCursor::new(7), DiffCursor::new(8));
    }
}

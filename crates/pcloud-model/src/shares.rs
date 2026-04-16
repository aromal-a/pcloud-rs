// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Bitwise permission flags mirroring the legacy C `PSYNC_PERM_*`
/// constants.
///
/// The wire representation is a bitmask over
/// [`Self::READ`] | [`Self::CREATE`] | [`Self::MODIFY`] | [`Self::DELETE`] |
/// [`Self::MANAGE`]. [`Self::READ`] is always implicitly set on a valid
/// share (matching the C behavior); [`Self::from_bits`] preserves that
/// invariant even when given `0`.
///
/// # Serde invariant
///
/// Serialized as a struct of five booleans (not as a bitmask). The
/// bitmask encoding is a separate schema concern exposed via
/// [`Self::to_bits`]/[`Self::from_bits`]; the two representations are
/// not interchangeable over the serde layer. Roundtrips losslessly
/// through `serde_json` for every combination of flags.
///
/// # Example
///
/// ```
/// use pcloud_model::shares::SharePermissions;
/// // Default is read-only.
/// let p = SharePermissions::default();
/// assert_eq!(p.to_bits(), SharePermissions::READ);
/// // Constants match the C-side bitmask.
/// assert_eq!(SharePermissions::CREATE, 2);
/// assert_eq!(SharePermissions::MODIFY, 4);
/// assert_eq!(SharePermissions::DELETE, 8);
/// assert_eq!(SharePermissions::MANAGE, 16);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SharePermissions {
    /// Recipient may list and download content. Always `true` on a
    /// valid share.
    pub read: bool,
    /// Recipient may create new files/folders inside the shared folder.
    pub create: bool,
    /// Recipient may modify existing files inside the shared folder.
    pub modify: bool,
    /// Recipient may delete files/folders inside the shared folder.
    pub delete: bool,
    /// Recipient may change the share's permission set and re-share it.
    pub manage: bool,
}

impl SharePermissions {
    /// Bitmask value for the implicit "read" flag.
    pub const READ: u32 = 1;
    /// Bitmask value for the "create" flag.
    pub const CREATE: u32 = 2;
    /// Bitmask value for the "modify" flag.
    pub const MODIFY: u32 = 4;
    /// Bitmask value for the "delete" flag.
    pub const DELETE: u32 = 8;
    /// Bitmask value for the "manage" flag.
    pub const MANAGE: u32 = 16;

    /// Decode a C bitmask into a typed permission set.
    ///
    /// Unknown upper bits are ignored; `read` is always set on the
    /// returned value to match the C invariant that a share without
    /// read access is not representable.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::shares::SharePermissions;
    /// // READ|CREATE
    /// let p = SharePermissions::from_bits(SharePermissions::READ | SharePermissions::CREATE);
    /// assert!(p.read);
    /// assert!(p.create);
    /// assert!(!p.modify);
    /// // Even a 0 mask preserves read.
    /// assert!(SharePermissions::from_bits(0).read);
    /// ```
    #[must_use]
    pub fn from_bits(bits: u32) -> Self {
        Self {
            // READ is implicit on C side; preserve that behavior.
            read: true,
            create: bits & Self::CREATE != 0,
            modify: bits & Self::MODIFY != 0,
            delete: bits & Self::DELETE != 0,
            manage: bits & Self::MANAGE != 0,
        }
    }

    /// Encode a typed permission set into the C bitmask form. `read`
    /// is always OR-ed in so the encoding roundtrips with
    /// [`Self::from_bits`].
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::shares::SharePermissions;
    /// let all = SharePermissions {
    ///     read: true, create: true, modify: true, delete: true, manage: true,
    /// };
    /// // 1|2|4|8|16 = 31
    /// assert_eq!(all.to_bits(), 31);
    /// // roundtrip
    /// assert_eq!(SharePermissions::from_bits(all.to_bits()).to_bits(), 31);
    /// ```
    #[must_use]
    pub fn to_bits(self) -> u32 {
        let mut bits = Self::READ;
        if self.create {
            bits |= Self::CREATE;
        }
        if self.modify {
            bits |= Self::MODIFY;
        }
        if self.delete {
            bits |= Self::DELETE;
        }
        if self.manage {
            bits |= Self::MANAGE;
        }
        bits
    }
}

/// Direction of a share relative to the currently authenticated user.
///
/// Used to partition UI surfaces (incoming inbox vs. outgoing
/// management) and to route diff events from [`crate::sync`] into the
/// correct backend collection.
///
/// # Serde invariant
///
/// Roundtrips losslessly through `serde_json` using the variant name
/// as tag (`"Incoming"` / `"Outgoing"`).
///
/// # Example
///
/// ```
/// use pcloud_model::shares::ShareDirection;
/// let d = ShareDirection::Incoming;
/// let j = serde_json::to_string(&d).unwrap();
/// assert!(j.contains("Incoming"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShareDirection {
    /// Share was created by somebody else and received by this user.
    Incoming,
    /// Share was created by this user and given out to somebody else.
    Outgoing,
}

/// Established share entry. Mirrors the retained subset of `psync_share_t`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareEntry {
    /// Server-assigned share id.
    pub share_id: u64,
    /// Folder id of the shared folder.
    pub folder_id: u64,
    /// Display name of the shared folder.
    pub share_name: String,
    /// User id of the share creator.
    pub from_user_id: u64,
    /// Email of the share creator.
    pub from_email: String,
    /// User id of the share recipient.
    pub to_user_id: u64,
    /// Email of the share recipient.
    pub to_email: String,
    /// Permission set granted to the recipient.
    pub permissions: SharePermissions,
    /// Creation time (unix seconds).
    pub created: u64,
    /// Direction of the share relative to the authenticated user.
    pub direction: ShareDirection,
    /// `true` if this is a business team-share (not a direct user-to-
    /// user share).
    pub is_team: bool,
    /// Team id when [`Self::is_team`] is `true`; `None` otherwise.
    pub team_id: Option<u64>,
}

/// Pending share request. Mirrors the retained subset of `psync_sharerequest_t`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRequestEntry {
    /// Server-assigned share-request id.
    pub share_request_id: u64,
    /// Folder id the request would share.
    pub folder_id: u64,
    /// Display name of the folder.
    pub share_name: String,
    /// User id of the requester.
    pub from_user_id: u64,
    /// Email of the requester.
    pub from_email: String,
    /// Email of the intended recipient.
    pub to_email: String,
    /// Proposed permission set.
    pub permissions: SharePermissions,
    /// Request creation time (unix seconds).
    pub created: u64,
    /// Optional free-form message from the requester.
    pub message: Option<String>,
    /// Direction relative to the authenticated user.
    pub direction: ShareDirection,
}

/// Business contact entry (`type==1`) or team (`type==3`) cache row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactEntry {
    /// pCloud contact-type discriminator (`1` = user, `3` = team).
    pub contact_type: u32,
    /// Server-side contact id (user id or team id depending on kind).
    pub contact_id: u64,
    /// Display name.
    pub name: String,
    /// Email, when the entry is a user rather than a team.
    pub email: Option<String>,
    /// Team id, when the entry represents a team.
    pub team_id: Option<u64>,
}

/// Result of a share mutation call (`sharefolder`, `acceptshare`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareMutationResult {
    /// The new share-request id when the mutation produced one; `None`
    /// for mutations that operate on an existing share without
    /// creating a new request (e.g. accept/decline).
    pub share_request_id: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_roundtrip_matches_c_bits() {
        let perms = SharePermissions {
            read: true,
            create: true,
            modify: false,
            delete: true,
            manage: true,
        };
        let bits = perms.to_bits();
        // READ|CREATE|DELETE|MANAGE = 1|2|8|16 = 27
        assert_eq!(bits, 1 | 2 | 8 | 16);
        let round = SharePermissions::from_bits(bits);
        assert!(round.read && round.create && !round.modify && round.delete && round.manage);
    }

    #[test]
    fn permissions_from_zero_bits_still_has_read() {
        let perms = SharePermissions::from_bits(0);
        assert!(perms.read);
        assert!(!perms.create && !perms.modify && !perms.delete && !perms.manage);
    }

    #[test]
    fn permissions_to_bits_default_is_read_only() {
        let perms = SharePermissions::default();
        assert_eq!(perms.to_bits(), SharePermissions::READ);
    }

    #[test]
    fn permissions_all_set_roundtrip() {
        let all = SharePermissions {
            read: true,
            create: true,
            modify: true,
            delete: true,
            manage: true,
        };
        assert_eq!(all.to_bits(), 1 | 2 | 4 | 8 | 16);
    }

    #[test]
    fn permissions_ignores_unknown_bits() {
        // Unknown upper bits should not flip known fields.
        let perms = SharePermissions::from_bits(0xFFFF_0000);
        assert!(perms.read);
        assert!(!perms.create && !perms.modify && !perms.delete && !perms.manage);
    }

    #[test]
    fn share_direction_serde_roundtrip() {
        let d = ShareDirection::Outgoing;
        let j = serde_json::to_string(&d).unwrap();
        let back: ShareDirection = serde_json::from_str(&j).unwrap();
        assert_eq!(d, back);
    }
}

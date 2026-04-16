// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Summary row for an existing public link, as returned by
/// `listpublinks` / `showpublink`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLinkSummary {
    /// Server-assigned link identifier.
    pub link_id: u64,
    /// Short code embedded in the shareable URL.
    pub code: String,
    /// Display name of the target file or folder.
    pub name: String,
    /// Fully-qualified public URL.
    pub link: String,
    /// Creation time (unix seconds).
    pub created: u64,
    /// Last-modified time of the underlying item (unix seconds).
    pub modified: u64,
    /// `true` if the link points at a folder.
    pub is_folder: bool,
    /// Id of the file or folder being shared.
    pub item_id: u64,
    /// Parent folder id of the shared item.
    pub parent_folder_id: u64,
    /// `true` if this is an upload-link (recipients can upload into
    /// the target folder) rather than a regular download link.
    pub is_upload: bool,
    /// `true` if the link is password-protected.
    pub has_password: bool,
    /// Number of recorded views.
    pub views: u64,
    /// Optional expiry (unix seconds); `None` means no expiry.
    pub expire: Option<u64>,
}

/// Single entry inside a public folder-link listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLinkContentsEntry {
    /// Display name of the entry.
    pub name: String,
    /// Creation time (unix seconds).
    pub created: u64,
    /// Last-modified time (unix seconds).
    pub modified: u64,
    /// `true` if this entry is a subfolder.
    pub is_folder: bool,
    /// Server-side file or folder id.
    pub item_id: u64,
    /// pCloud icon enum value.
    pub icon: u64,
}

/// Contents of a public folder-link — the short code and the list of
/// entries currently visible through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLinkContents {
    /// Short code from the public URL.
    pub code: String,
    /// Entries listed under the shared folder.
    pub entries: Vec<PublicLinkContentsEntry>,
}

/// Result of creating a public link (file or folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedPublicLink {
    /// Newly-assigned link id.
    pub link_id: u64,
    /// Newly-generated public URL.
    pub link: String,
    /// `true` if the target is a folder.
    pub is_folder: bool,
}

/// Summary row for an upload-link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadLinkSummary {
    /// Server-assigned upload-link id.
    pub upload_link_id: u64,
    /// Short code embedded in the shareable URL.
    pub code: String,
    /// Display name of the target folder.
    pub name: String,
    /// Fully-qualified public URL.
    pub link: String,
    /// Owner-provided description/comment shown to uploaders.
    pub comment: String,
    /// Bytes already uploaded through this link.
    pub space: u64,
    /// Optional upper bound on total bytes; `None` means unlimited.
    pub maxspace: Option<u64>,
    /// File count uploaded through this link.
    pub files: u64,
    /// Creation time (unix seconds).
    pub created: u64,
    /// Last-modified time (unix seconds).
    pub modified: u64,
    /// `true` if the target item is a folder (always true for upload
    /// links in practice; retained for API shape parity).
    pub is_folder: bool,
    /// Target folder id.
    pub item_id: u64,
    /// Parent folder id.
    pub parent_folder_id: u64,
    /// pCloud icon enum value.
    pub icon: u64,
}

/// Result of creating an upload-link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedUploadLink {
    /// Newly-assigned upload-link id.
    pub upload_link_id: u64,
    /// Newly-generated public URL.
    pub link: String,
}

/// Result of creating a "tree" public link that bundles several files
/// or folders into a single share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedTreePublicLink {
    /// Newly-assigned link id.
    pub link_id: u64,
    /// Display name of the tree-link.
    pub name: String,
    /// Fully-qualified public URL.
    pub link: String,
}

/// Access-control entry for a public link: a specific recipient email
/// and receiver id allowed to access the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLinkAccessEntry {
    /// Recipient email address.
    pub email: String,
    /// Server-side receiver id.
    pub receiver_id: u64,
}

/// Bookmarked (pinned) public link as stored by the owner for quick
/// reuse from the client UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLinkBookmark {
    /// Fully-qualified public URL.
    pub link: String,
    /// Display name.
    pub name: String,
    /// Short code from the URL.
    pub code: String,
    /// Optional description provided by the owner.
    pub description: String,
    /// Creation time (unix seconds).
    pub created: u64,
    /// pCloud location id (regional cluster).
    pub location_id: u64,
}

/// Upload-policy for an existing public folder-link (controls who may
/// upload into the shared folder through the link).
///
/// # Example
///
/// ```
/// use pcloud_model::public_links::PublicLinkUploadPolicy;
/// let p = PublicLinkUploadPolicy::ChosenUsers;
/// let j = serde_json::to_string(&p).unwrap();
/// let back: PublicLinkUploadPolicy = serde_json::from_str(&j).unwrap();
/// assert_eq!(p, back);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicLinkUploadPolicy {
    /// Uploads are disabled — link is read-only.
    Disabled,
    /// Anyone with the link may upload.
    Everyone,
    /// Only explicitly listed recipients may upload.
    ChosenUsers,
}

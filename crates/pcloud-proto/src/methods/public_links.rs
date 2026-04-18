//! Wire-level method builders for public-link operations (file/folder
//! links, upload-links, tree-links, changepublink). Consumed by
//! `public_links_api`.

use crate::binary_api::{BinaryParam, BinaryParamValue};
use crate::redacted::RedactedProtoString;
use pcloud_model::public_links::PublicLinkUploadPolicy;

use super::ProtocolMethod;

/// `ListPublicLinksRequest` — list public links request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPublicLinksRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
}

impl ProtocolMethod for ListPublicLinksRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "listpublinks"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params.push(BinaryParam::string("iconformat", "id"));
        params
    }
}

/// `ShowPublicLinkRequest` — show public link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowPublicLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `code` field (code).
    pub code: String,
}

/// `DeletePublicLinkRequest` — delete public link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletePublicLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `link_id` field (link id).
    pub link_id: u64,
}

impl ProtocolMethod for DeletePublicLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "deletepublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("linkid", self.link_id));
        params
    }
}

impl ProtocolMethod for ShowPublicLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "showpublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(4);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params.push(BinaryParam::string("iconformat", "id"));
        params.push(BinaryParam::string("code", self.code.as_str()));
        params
    }
}

/// `CreateFilePublicLinkRequest` — create file public link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFilePublicLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `path` field (path).
    pub path: String,
}

impl ProtocolMethod for CreateFilePublicLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "getfilepublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("path", self.path.as_str()));
        params
    }
}

/// `CreateFolderPublicLinkRequest` — create folder public link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFolderPublicLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `path` field (path).
    pub path: String,
}

impl ProtocolMethod for CreateFolderPublicLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "getfolderpublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("path", self.path.as_str()));
        params
    }
}

/// `ChangePublicLinkExpireRequest` — change public link expire request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePublicLinkExpireRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `link_id` field (link id).
    pub link_id: u64,
    /// The `expire` field (expire).
    pub expire: Option<u64>,
}

impl ProtocolMethod for ChangePublicLinkExpireRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "changepublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("linkid", self.link_id));
        match self.expire {
            Some(expire) => params.push(BinaryParam::number("expire", expire)),
            None => params.push(BinaryParam::number("deleteexpire", 1)),
        }
        params
    }
}

/// `ChangePublicLinkPasswordRequest` — change public link password request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePublicLinkPasswordRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `link_id` field (link id).
    pub link_id: u64,
    /// The `password` field (password).
    pub password: Option<RedactedProtoString>,
}

impl ProtocolMethod for ChangePublicLinkPasswordRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "changepublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("linkid", self.link_id));
        match self.password.as_ref().map(|p| p.expose_secret()) {
            Some(password) => params.push(BinaryParam::string("linkpassword", password)),
            None => params.push(BinaryParam::number("deletepassword", 1)),
        }
        params
    }
}

/// `ChangePublicLinkUploadRequest` — change public link upload request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePublicLinkUploadRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `link_id` field (link id).
    pub link_id: u64,
    /// The `policy` field (policy).
    pub policy: PublicLinkUploadPolicy,
}

impl ProtocolMethod for ChangePublicLinkUploadRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "changepublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let cap = match self.policy {
            PublicLinkUploadPolicy::Disabled => 3,
            _ => 4,
        };
        let mut params = Vec::with_capacity(cap);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("linkid", self.link_id));
        match self.policy {
            PublicLinkUploadPolicy::Everyone => {
                params.push(BinaryParam::number("enableuploadforeveryone", 1));
                params.push(BinaryParam::number("enableuploadforchosenusers", 0));
            }
            PublicLinkUploadPolicy::ChosenUsers => {
                params.push(BinaryParam::number("enableuploadforeveryone", 0));
                params.push(BinaryParam::number("enableuploadforchosenusers", 1));
            }
            PublicLinkUploadPolicy::Disabled => {
                params.push(BinaryParam::number("disableupload", 1));
            }
        }
        params
    }
}

/// `ListUploadLinksRequest` — list upload links request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListUploadLinksRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
}

impl ProtocolMethod for ListUploadLinksRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "listuploadlinks"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params.push(BinaryParam::string("iconformat", "id"));
        params
    }
}

/// `CreateUploadLinkRequest` — create upload link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateUploadLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `path` field (path).
    pub path: String,
    /// The `comment` field (comment).
    pub comment: String,
    /// The `expire` field (expire).
    pub expire: Option<u64>,
    /// The `maxspace` field (maxspace).
    pub maxspace: Option<u64>,
    /// The `maxfiles` field (maxfiles).
    pub maxfiles: Option<u64>,
}

impl ProtocolMethod for CreateUploadLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "createuploadlink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let cap = 3
            + usize::from(self.expire.is_some())
            + usize::from(self.maxspace.is_some())
            + usize::from(self.maxfiles.is_some());
        let mut params = Vec::with_capacity(cap);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("path", self.path.as_str()));
        params.push(BinaryParam::string("comment", self.comment.as_str()));
        if let Some(expire) = self.expire {
            params.push(BinaryParam::number("expire", expire));
        }
        if let Some(maxspace) = self.maxspace {
            params.push(BinaryParam::number("maxspace", maxspace));
        }
        if let Some(maxfiles) = self.maxfiles {
            params.push(BinaryParam::number("maxfiles", maxfiles));
        }
        params
    }
}

/// `DeleteUploadLinkRequest` — delete upload link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteUploadLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `upload_link_id` field (upload link id).
    pub upload_link_id: u64,
}

impl ProtocolMethod for DeleteUploadLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "deleteuploadlink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("uploadlinkid", self.upload_link_id));
        params
    }
}

/// `CreateTreePublicLinkRequest` — create tree public link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTreePublicLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `name` field (name).
    pub name: String,
    /// The `root_folder_id` field (root folder id).
    pub root_folder_id: Option<u64>,
    /// The `folder_ids_csv` field (folder ids csv).
    pub folder_ids_csv: Option<String>,
    /// The `file_ids_csv` field (file ids csv).
    pub file_ids_csv: Option<String>,
    /// The `expire` field (expire).
    pub expire: Option<u64>,
    /// The `maxdownloads` field (maxdownloads).
    pub maxdownloads: Option<u64>,
    /// The `maxtraffic` field (maxtraffic).
    pub maxtraffic: Option<u64>,
}

impl ProtocolMethod for CreateTreePublicLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "gettreepublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let cap = 2
            + usize::from(self.root_folder_id.is_some())
            + usize::from(self.folder_ids_csv.is_some())
            + usize::from(self.file_ids_csv.is_some())
            + usize::from(self.expire.is_some())
            + usize::from(self.maxdownloads.is_some())
            + usize::from(self.maxtraffic.is_some());
        let mut params = Vec::with_capacity(cap);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("name", self.name.as_str()));
        if let Some(root_folder_id) = self.root_folder_id {
            // Preserve original wire shape: the `folderid` field was historically
            // encoded as a string for this request.
            params.push(BinaryParam {
                name: "folderid".to_owned(),
                value: BinaryParamValue::String(root_folder_id.to_string()),
            });
        }
        if let Some(folder_ids_csv) = self.folder_ids_csv.as_deref() {
            params.push(BinaryParam::string("folderids", folder_ids_csv));
        }
        if let Some(file_ids_csv) = self.file_ids_csv.as_deref() {
            params.push(BinaryParam::string("fileids", file_ids_csv));
        }
        if let Some(expire) = self.expire {
            params.push(BinaryParam::number("expire", expire));
        }
        if let Some(maxdownloads) = self.maxdownloads {
            params.push(BinaryParam::number("maxdownloads", maxdownloads));
        }
        if let Some(maxtraffic) = self.maxtraffic {
            params.push(BinaryParam::number("maxtraffic", maxtraffic));
        }
        params
    }
}

/// `ListPublicLinkAccessRequest` — list public link access request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPublicLinkAccessRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `link_id` field (link id).
    pub link_id: u64,
}

impl ProtocolMethod for ListPublicLinkAccessRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "publink/listemailswithaccess"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("linkid", self.link_id));
        params
    }
}

/// `AddPublicLinkAccessRequest` — add public link access request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPublicLinkAccessRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `link_id` field (link id).
    pub link_id: u64,
    /// The `email` field (email).
    pub email: String,
}

impl ProtocolMethod for AddPublicLinkAccessRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "publink/addaccess"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("linkid", self.link_id));
        params.push(BinaryParam::string("mail", self.email.as_str()));
        params
    }
}

/// `RemovePublicLinkAccessRequest` — remove public link access request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePublicLinkAccessRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `link_id` field (link id).
    pub link_id: u64,
    /// The `receiver_id` field (receiver id).
    pub receiver_id: u64,
}

impl ProtocolMethod for RemovePublicLinkAccessRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "publink/removeaccess"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("linkid", self.link_id));
        params.push(BinaryParam::number("receiverid", self.receiver_id));
        params
    }
}

/// `ListBookmarksRequest` — list bookmarks request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListBookmarksRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
}

impl ProtocolMethod for ListBookmarksRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "publink/listpins"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params
    }
}

/// `RemoveBookmarkRequest` — remove bookmark request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveBookmarkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `code` field (code).
    pub code: String,
    /// The `location_id` field (location id).
    pub location_id: u64,
}

impl ProtocolMethod for RemoveBookmarkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "publink/unpin"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("locationid", self.location_id));
        params.push(BinaryParam::string("code", self.code.as_str()));
        params
    }
}

/// Mirrors the C `do_psync_file_public_link` helper, which can optionally send
/// `expire`, `maxdownloads`, and `maxtraffic` parameters alongside the path.
///
/// The zero-argument variant is kept as [`CreateFilePublicLinkRequest`] so the
/// bare parity helper (`psync_file_public_link`) stays one-to-one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFilePublicLinkOptionsRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `path` field (path).
    pub path: String,
    /// The `expire` field (expire).
    pub expire: Option<u64>,
    /// The `maxdownloads` field (maxdownloads).
    pub maxdownloads: Option<u64>,
    /// The `maxtraffic` field (maxtraffic).
    pub maxtraffic: Option<u64>,
}

impl ProtocolMethod for CreateFilePublicLinkOptionsRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "getfilepublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let cap = 2
            + usize::from(self.expire.is_some())
            + usize::from(self.maxdownloads.is_some())
            + usize::from(self.maxtraffic.is_some());
        let mut params = Vec::with_capacity(cap);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("path", self.path.as_str()));
        if let Some(expire) = self.expire {
            params.push(BinaryParam::number("expire", expire));
        }
        if let Some(maxdownloads) = self.maxdownloads {
            params.push(BinaryParam::number("maxdownloads", maxdownloads));
        }
        if let Some(maxtraffic) = self.maxtraffic {
            params.push(BinaryParam::number("maxtraffic", maxtraffic));
        }
        params
    }
}

/// Mirrors the C `do_psync_folder_public_link_full` helper, which adds an
/// optional password alongside `expire`, `maxdownloads`, and `maxtraffic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFolderPublicLinkOptionsRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `path` field (path).
    pub path: String,
    /// The `expire` field (expire).
    pub expire: Option<u64>,
    /// The `maxdownloads` field (maxdownloads).
    pub maxdownloads: Option<u64>,
    /// The `maxtraffic` field (maxtraffic).
    pub maxtraffic: Option<u64>,
    /// The `password` field (password).
    pub password: Option<RedactedProtoString>,
}

impl ProtocolMethod for CreateFolderPublicLinkOptionsRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "getfolderpublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let cap = 2
            + usize::from(self.password.is_some())
            + usize::from(self.expire.is_some())
            + usize::from(self.maxdownloads.is_some())
            + usize::from(self.maxtraffic.is_some());
        let mut params = Vec::with_capacity(cap);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("path", self.path.as_str()));
        if let Some(password) = self.password.as_ref().map(|p| p.expose_secret()) {
            params.push(BinaryParam::string("linkpassword", password));
        }
        if let Some(expire) = self.expire {
            params.push(BinaryParam::number("expire", expire));
        }
        if let Some(maxdownloads) = self.maxdownloads {
            params.push(BinaryParam::number("maxdownloads", maxdownloads));
        }
        if let Some(maxtraffic) = self.maxtraffic {
            params.push(BinaryParam::number("maxtraffic", maxtraffic));
        }
        params
    }
}

/// Mirrors the C `do_psync_folder_updownlink_link` helper, which sends
/// `publink/createfolderlinkandsend` to mail a download/upload link to a
/// recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFolderUpDownLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
    /// The `mail` field (mail).
    pub mail: String,
    /// The `can_upload` field (can upload).
    pub can_upload: bool,
}

impl ProtocolMethod for CreateFolderUpDownLinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "publink/createfolderlinkandsend"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(4);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("folderid", self.folder_id));
        params.push(BinaryParam::string("mail", self.mail.as_str()));
        params.push(BinaryParam::number("canupload", u64::from(self.can_upload)));
        params
    }
}

/// Mirrors the C `psync_send_publink` helper (`pclsync/psynclib.c:2217`),
/// which posts `sendpublink` with an existing public-link `code`, a comma-
/// separated `mails` list, an optional `message`, and a fixed
/// `source=1` discriminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPublinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `code` field (code).
    pub code: String,
    /// The `mails` field (mails).
    pub mails: String,
    /// The `message` field (message).
    pub message: String,
}

impl ProtocolMethod for SendPublinkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "sendpublink"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(5);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("code", self.code.as_str()));
        params.push(BinaryParam::string("mails", self.mails.as_str()));
        params.push(BinaryParam::string("message", self.message.as_str()));
        params.push(BinaryParam::number("source", 1));
        params
    }
}

/// `ChangeBookmarkRequest` — change bookmark request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeBookmarkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `code` field (code).
    pub code: String,
    /// The `location_id` field (location id).
    pub location_id: u64,
    /// The `name` field (name).
    pub name: String,
    /// The `description` field (description).
    pub description: String,
}

impl ProtocolMethod for ChangeBookmarkRequest {
    #[inline]
    fn command_name(&self) -> &'static str {
        "publink/changepin"
    }

    fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(5);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("code", self.code.as_str()));
        params.push(BinaryParam::number("locationid", self.location_id));
        params.push(BinaryParam::string("name", self.name.as_str()));
        params.push(BinaryParam::string(
            "description",
            self.description.as_str(),
        ));
        params
    }
}

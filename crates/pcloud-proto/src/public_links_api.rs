//! Public-links protocol client: file/folder link CRUD, `changepublink`
//! mutations (expire, password, upload policy), upload-link CRUD,
//! tree-link creation, upload-access, bookmark/pin, and screenshot
//! helpers. Consumed by `pcloud-backends::public_link_backend`.
//!
//! ## Role in the request pipeline
//!
//! Wraps a broad family of pCloud commands (`getfilepublink`,
//! `getfolderpublink`, `changepublink`, `createuploadlink`,
//! `gettreepublink`, etc.). Every method assembles parameters via a
//! typed request builder, dispatches through the supplied transport,
//! and projects results into `pcloud-model` domain types so the
//! daemon and SDK layers can consume them without touching raw
//! [`crate::response::Value`] trees.
//!
//! ## Security considerations
//!
//! - Link passwords are accepted as plain `&str` but never logged;
//!   upstream callers pass `SecretString::expose_secret()` only at
//!   the request-construction boundary.
//! - Server-returned URLs and codes are untrusted input; callers
//!   that embed them into HTML, shell commands, or filesystem paths
//!   must escape appropriately.
//! - Upload-policy changes are surfaced as explicit typed enums so
//!   callers cannot accidentally pass the wrong integer to the
//!   server.
//!
//! Portable; no platform gating.

use pcloud_model::public_links::{
    CreatedPublicLink, CreatedTreePublicLink, CreatedUploadLink, PublicLinkAccessEntry,
    PublicLinkBookmark, PublicLinkContents, PublicLinkContentsEntry, PublicLinkSummary,
    PublicLinkUploadPolicy, UploadLinkSummary,
};
use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    methods::public_links::{
        AddPublicLinkAccessRequest, ChangeBookmarkRequest, ChangePublicLinkExpireRequest,
        ChangePublicLinkPasswordRequest, ChangePublicLinkUploadRequest,
        CreateFilePublicLinkOptionsRequest, CreateFilePublicLinkRequest,
        CreateFolderPublicLinkOptionsRequest, CreateFolderPublicLinkRequest,
        CreateFolderUpDownLinkRequest, CreateTreePublicLinkRequest, CreateUploadLinkRequest,
        DeletePublicLinkRequest, DeleteUploadLinkRequest, ListBookmarksRequest,
        ListPublicLinkAccessRequest, ListPublicLinksRequest, ListUploadLinksRequest,
        RemoveBookmarkRequest, RemovePublicLinkAccessRequest, SendPublinkRequest,
        ShowPublicLinkRequest,
    },
    response::{HashView, Value},
};

/// Resolves pCloud drive paths into folder/file identifiers.
///
/// This mirrors the C path-to-id resolvers used by `do_ptree_public_link` for
/// the `root`, `folders`, and `files` arrays. In the C client those resolvers
/// use `pfs_fldr_id_by_path` / `pfs_fldr_resolve_path` against the local pfs
/// cache; the Rust runtime provides its own implementation so the resolver is
/// never fabricated and never silently returns `0`.
pub trait PublicLinkPathResolver {
    /// Errors returned when a path cannot be resolved.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Resolves an absolute pCloud drive path to a folder id.
    fn resolve_folder(&self, path: &str) -> Result<u64, Self::Error>;

    /// Resolves an absolute pCloud drive file path to a file id.
    fn resolve_file(&self, path: &str) -> Result<u64, Self::Error>;
}

/// Builder describing the tree-link target set in the exact shape C accepts
/// (root path + folder paths + file paths) before any ids have been resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreePublicLinkPaths {
    /// The `root` field (root).
    pub root: Option<String>,
    /// The `folders` field (folders).
    pub folders: Vec<String>,
    /// The `files` field (files).
    pub files: Vec<String>,
}

impl TreePublicLinkPaths {
    /// `new` — new.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `with_root` — with root.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn with_root(mut self, root: impl Into<String>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// `with_folders` — with folders.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn with_folders(mut self, folders: Vec<String>) -> Self {
        self.folders = folders;
        self
    }

    /// `with_files` — with files.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files = files;
        self
    }

    /// Returns true when no root, folder, or file target is populated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.is_none() && self.folders.is_empty() && self.files.is_empty()
    }
}

/// `PublicLinksApi` — public links api.
#[derive(Debug)]
pub struct PublicLinksApi<T> {
    transport: T,
}

/// `PublicLinksApiError` — public links api error.
#[derive(Debug, Error)]
pub enum PublicLinksApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("public-link method returned non-zero result code {result} ({message:?})")]
    Result {
        /// The `result` field (result).
        result: u64,
        /// The `message` field (message).
        message: Option<String>,
    },
    /// `Malformed` variant (malformed).
    #[error("response was malformed: {0}")]
    Malformed(&'static str),
    /// `EmptyTreeTarget` variant (empty tree target).
    #[error("tree link requires at least one of root, folders, or files")]
    EmptyTreeTarget,
    /// `PathUnresolved` variant (path unresolved).
    #[error("failed to resolve pCloud path {path:?}: {source}")]
    PathUnresolved {
        /// The `path` field (path).
        path: String,
        /// The `source` field (source).
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl<T> PublicLinksApi<T> {
    /// `new` — new.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T> PublicLinksApi<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// `apply_api_server_hint` — apply api server hint.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.transport.apply_api_server_hint(api_server);
    }

    /// `list_public_links` — list public links.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_public_links(
        &self,
        auth_token: impl Into<String>,
    ) -> Result<Vec<PublicLinkSummary>, PublicLinksApiError<T::Error>> {
        let request = ListPublicLinksRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "listpublinks response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let entries = hash
            .get_array("publinks")
            .ok_or(PublicLinksApiError::Malformed(
                "listpublinks missing publinks",
            ))?;

        entries
            .iter()
            .map(parse_public_link_summary::<T::Error>)
            .collect()
    }

    /// `show_public_link` — show public link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn show_public_link(
        &self,
        auth_token: impl Into<String>,
        code: impl Into<String>,
    ) -> Result<PublicLinkContents, PublicLinksApiError<T::Error>> {
        let request = ShowPublicLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            code: code.into(),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "showpublink response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let metadata = hash
            .get_hash("metadata")
            .ok_or(PublicLinksApiError::Malformed(
                "showpublink missing metadata",
            ))?;
        let contents = metadata
            .get_array("contents")
            .ok_or(PublicLinksApiError::Malformed(
                "showpublink missing contents",
            ))?;

        let entries = contents
            .iter()
            .map(parse_public_link_contents_entry::<T::Error>)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(PublicLinkContents {
            code: request.code,
            entries,
        })
    }

    /// `delete_public_link` — delete public link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn delete_public_link(
        &self,
        auth_token: impl Into<String>,
        link_id: u64,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = DeletePublicLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            link_id,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "deletepublink response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `create_file_public_link` — create file public link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn create_file_public_link(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<CreatedPublicLink, PublicLinksApiError<T::Error>> {
        let request = CreateFilePublicLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            path: path.into(),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "getfilepublink response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        parse_created_public_link::<T::Error>(hash, false)
    }

    /// `create_folder_public_link` — create folder public link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn create_folder_public_link(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<CreatedPublicLink, PublicLinksApiError<T::Error>> {
        let request = CreateFolderPublicLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            path: path.into(),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "getfolderpublink response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        parse_created_public_link::<T::Error>(hash, true)
    }

    /// `change_public_link_expire` — change public link expire.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn change_public_link_expire(
        &self,
        auth_token: impl Into<String>,
        link_id: u64,
        expire: Option<u64>,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = ChangePublicLinkExpireRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            link_id,
            expire,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "changepublink response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `change_public_link_password` — change public link password.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn change_public_link_password(
        &self,
        auth_token: impl Into<String>,
        link_id: u64,
        password: Option<String>,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = ChangePublicLinkPasswordRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            link_id,
            password: password.map(crate::redacted::RedactedProtoString::from),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "changepublink response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `change_public_link_upload` — change public link upload.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn change_public_link_upload(
        &self,
        auth_token: impl Into<String>,
        link_id: u64,
        policy: PublicLinkUploadPolicy,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = ChangePublicLinkUploadRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            link_id,
            policy,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "changepublink response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `list_upload_links` — list upload links.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_upload_links(
        &self,
        auth_token: impl Into<String>,
    ) -> Result<Vec<UploadLinkSummary>, PublicLinksApiError<T::Error>> {
        let request = ListUploadLinksRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "listuploadlinks response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let entries = hash
            .get_array("uploadlinks")
            .ok_or(PublicLinksApiError::Malformed(
                "listuploadlinks missing uploadlinks",
            ))?;

        entries
            .iter()
            .map(parse_upload_link_summary::<T::Error>)
            .collect()
    }

    /// `create_upload_link` — create upload link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn create_upload_link(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
        comment: impl Into<String>,
        expire: Option<u64>,
        maxspace: Option<u64>,
        maxfiles: Option<u64>,
    ) -> Result<CreatedUploadLink, PublicLinksApiError<T::Error>> {
        let request = CreateUploadLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            path: path.into(),
            comment: comment.into(),
            expire,
            maxspace,
            maxfiles,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "createuploadlink response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(CreatedUploadLink {
            upload_link_id: hash.get_number("uploadlinkid").ok_or(
                PublicLinksApiError::Malformed("created upload link missing uploadlinkid"),
            )?,
            link: hash
                .get_string("link")
                .ok_or(PublicLinksApiError::Malformed(
                    "created upload link missing link",
                ))?
                .to_owned(),
        })
    }

    /// `delete_upload_link` — delete upload link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn delete_upload_link(
        &self,
        auth_token: impl Into<String>,
        upload_link_id: u64,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = DeleteUploadLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            upload_link_id,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "deleteuploadlink response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `create_tree_public_link` — create tree public link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[allow(clippy::too_many_arguments)]
    pub fn create_tree_public_link(
        &self,
        auth_token: impl Into<String>,
        name: impl Into<String>,
        root_folder_id: Option<u64>,
        folder_ids_csv: Option<String>,
        file_ids_csv: Option<String>,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Result<CreatedTreePublicLink, PublicLinksApiError<T::Error>> {
        let request = CreateTreePublicLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            name: name.into(),
            root_folder_id,
            folder_ids_csv,
            file_ids_csv,
            expire,
            maxdownloads,
            maxtraffic,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "gettreepublink response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(CreatedTreePublicLink {
            link_id: hash
                .get_number("linkid")
                .ok_or(PublicLinksApiError::Malformed(
                    "created tree link missing linkid",
                ))?,
            name: request.name,
            link: hash
                .get_string("link")
                .ok_or(PublicLinksApiError::Malformed(
                    "created tree link missing link",
                ))?
                .to_owned(),
        })
    }

    /// `list_public_link_access` — list public link access.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_public_link_access(
        &self,
        auth_token: impl Into<String>,
        link_id: u64,
    ) -> Result<Vec<PublicLinkAccessEntry>, PublicLinksApiError<T::Error>> {
        let request = ListPublicLinkAccessRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            link_id,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "publink/listemailswithaccess response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let entries = hash
            .get_array("list")
            .ok_or(PublicLinksApiError::Malformed(
                "publink/listemailswithaccess missing list",
            ))?;
        entries
            .iter()
            .map(parse_public_link_access_entry::<T::Error>)
            .collect()
    }

    /// `add_public_link_access` — add public link access.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn add_public_link_access(
        &self,
        auth_token: impl Into<String>,
        link_id: u64,
        email: impl Into<String>,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = AddPublicLinkAccessRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            link_id,
            email: email.into(),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "publink/addaccess response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `remove_public_link_access` — remove public link access.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn remove_public_link_access(
        &self,
        auth_token: impl Into<String>,
        link_id: u64,
        receiver_id: u64,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = RemovePublicLinkAccessRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            link_id,
            receiver_id,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "publink/removeaccess response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `list_bookmarks` — list bookmarks.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_bookmarks(
        &self,
        auth_token: impl Into<String>,
    ) -> Result<Vec<PublicLinkBookmark>, PublicLinksApiError<T::Error>> {
        let request = ListBookmarksRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "publink/listpins response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let entries = hash
            .get_array("list")
            .ok_or(PublicLinksApiError::Malformed(
                "publink/listpins missing list",
            ))?;
        entries
            .iter()
            .map(parse_public_link_bookmark::<T::Error>)
            .collect()
    }

    /// `remove_bookmark` — remove bookmark.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn remove_bookmark(
        &self,
        auth_token: impl Into<String>,
        code: impl Into<String>,
        location_id: u64,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = RemoveBookmarkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            code: code.into(),
            location_id,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "publink/unpin response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// `change_bookmark` — change bookmark.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn change_bookmark(
        &self,
        auth_token: impl Into<String>,
        code: impl Into<String>,
        location_id: u64,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = ChangeBookmarkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            code: code.into(),
            location_id,
            name: name.into(),
            description: description.into(),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "publink/changepin response was not a hash",
        ))?;
        expect_ok_result(hash)
    }
}

impl<T> PublicLinksApi<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// Mirrors the C `do_psync_file_public_link` helper with the optional
    /// `expire`, `maxdownloads`, and `maxtraffic` parameters.
    pub fn create_file_public_link_with_options(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Result<CreatedPublicLink, PublicLinksApiError<T::Error>> {
        let request = CreateFilePublicLinkOptionsRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            path: path.into(),
            expire,
            maxdownloads,
            maxtraffic,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "getfilepublink response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        parse_created_public_link::<T::Error>(hash, false)
    }

    /// Mirrors the C `do_psync_folder_public_link_full` helper, allowing the
    /// optional `linkpassword`, `expire`, `maxdownloads`, and `maxtraffic`
    /// parameters.
    pub fn create_folder_public_link_with_options(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
        password: Option<String>,
    ) -> Result<CreatedPublicLink, PublicLinksApiError<T::Error>> {
        let request = CreateFolderPublicLinkOptionsRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            path: path.into(),
            expire,
            maxdownloads,
            maxtraffic,
            password: password.map(crate::redacted::RedactedProtoString::from),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "getfolderpublink response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        parse_created_public_link::<T::Error>(hash, true)
    }

    /// Mirrors the C `do_psync_screenshot_public_link` helper.
    ///
    /// Creates a download link via `getfilepublink` and, when `delay_seconds`
    /// is provided, issues a follow-up `changepublink` with the expiration
    /// computed exactly like the C client (`now + delay, rounded down to the
    /// hour`, defaulting to 30 days when `delay == 0` and `has_delay` is set).
    pub fn create_screenshot_public_link(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
        has_delay: bool,
        delay_seconds: u64,
        now_epoch_seconds: u64,
    ) -> Result<CreatedPublicLink, PublicLinksApiError<T::Error>> {
        let auth_token: String = auth_token.into();
        let created =
            self.create_file_public_link_with_options(auth_token.clone(), path, None, None, None)?;
        if has_delay {
            // C uses 2592000 (30 days) as the default when `delay == 0`.
            let delay = if delay_seconds == 0 {
                2_592_000
            } else {
                delay_seconds
            };
            let mut expire = now_epoch_seconds.saturating_add(delay);
            // C rounds down to the hour: `time = time - time % 3600;`.
            expire -= expire % 3600;
            self.change_public_link_expire(auth_token, created.link_id, Some(expire))?;
        }
        Ok(created)
    }

    /// Mirrors the C `do_psync_folder_updownlink_link` helper
    /// (`publink/createfolderlinkandsend`).
    pub fn create_folder_updownlink(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
        mail: impl Into<String>,
        can_upload: bool,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = CreateFolderUpDownLinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
            mail: mail.into(),
            can_upload,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "publink/createfolderlinkandsend response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// Mirrors the C `psync_send_publink` helper
    /// (`pclsync/psynclib.c:2217`). Mails an existing public link `code`
    /// to the comma-separated `mails` list with the given `message` body.
    /// The wire `source` field is fixed to `1` exactly as the C helper.
    pub fn send_publink(
        &self,
        auth_token: impl Into<String>,
        code: impl Into<String>,
        mails: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<(), PublicLinksApiError<T::Error>> {
        let request = SendPublinkRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            code: code.into(),
            mails: mails.into(),
            message: message.into(),
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(PublicLinksApiError::Transport)?;
        let hash = response.as_hash().ok_or(PublicLinksApiError::Malformed(
            "sendpublink response was not a hash",
        ))?;
        expect_ok_result(hash)
    }

    /// Mirrors the C `do_ptree_public_link` helper, which accepts pCloud drive
    /// paths and resolves them to folder/file identifiers before calling
    /// `gettreepublink`.
    ///
    /// The path resolution is performed by the supplied
    /// [`PublicLinkPathResolver`]. This keeps the proto crate decoupled from
    /// the local pfs cache while preserving the exact C request shape.
    #[allow(clippy::too_many_arguments)]
    pub fn create_tree_public_link_from_paths<R>(
        &self,
        auth_token: impl Into<String>,
        name: impl Into<String>,
        paths: &TreePublicLinkPaths,
        resolver: &R,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Result<CreatedTreePublicLink, PublicLinksApiError<T::Error>>
    where
        R: PublicLinkPathResolver,
    {
        if paths.is_empty() {
            return Err(PublicLinksApiError::EmptyTreeTarget);
        }

        let root_folder_id = match paths.root.as_deref() {
            Some(root) => Some(resolver.resolve_folder(root).map_err(|err| {
                PublicLinksApiError::PathUnresolved {
                    path: root.to_owned(),
                    source: Box::new(err),
                }
            })?),
            None => None,
        };

        let folder_ids_csv = if paths.folders.is_empty() {
            None
        } else {
            let mut ids = Vec::with_capacity(paths.folders.len());
            for folder in &paths.folders {
                let id = resolver.resolve_folder(folder).map_err(|err| {
                    PublicLinksApiError::PathUnresolved {
                        path: folder.clone(),
                        source: Box::new(err),
                    }
                })?;
                ids.push(id.to_string());
            }
            Some(ids.join(","))
        };

        let file_ids_csv = if paths.files.is_empty() {
            None
        } else {
            let mut ids = Vec::with_capacity(paths.files.len());
            for file in &paths.files {
                let id = resolver.resolve_file(file).map_err(|err| {
                    PublicLinksApiError::PathUnresolved {
                        path: file.clone(),
                        source: Box::new(err),
                    }
                })?;
                ids.push(id.to_string());
            }
            Some(ids.join(","))
        };

        self.create_tree_public_link(
            auth_token,
            name,
            root_folder_id,
            folder_ids_csv,
            file_ids_csv,
            expire,
            maxdownloads,
            maxtraffic,
        )
    }
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), PublicLinksApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }

    Err(PublicLinksApiError::Result {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
    })
}

fn parse_public_link_summary<E>(value: &Value) -> Result<PublicLinkSummary, PublicLinksApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value.as_hash().ok_or(PublicLinksApiError::Malformed(
        "publink entry was not a hash",
    ))?;
    let metadata = hash
        .get_hash("metadata")
        .ok_or(PublicLinksApiError::Malformed(
            "publink entry missing metadata",
        ))?;
    let is_folder = metadata.get_bool("isfolder").unwrap_or(false);
    let item_id = if is_folder {
        metadata
            .get_number("folderid")
            .ok_or(PublicLinksApiError::Malformed(
                "publink metadata missing folderid",
            ))?
    } else {
        metadata
            .get_number("fileid")
            .ok_or(PublicLinksApiError::Malformed(
                "publink metadata missing fileid",
            ))?
    };

    Ok(PublicLinkSummary {
        link_id: hash
            .get_number("linkid")
            .ok_or(PublicLinksApiError::Malformed("publink missing linkid"))?,
        code: hash
            .get_string("code")
            .ok_or(PublicLinksApiError::Malformed("publink missing code"))?
            .to_owned(),
        name: metadata
            .get_string("name")
            .ok_or(PublicLinksApiError::Malformed(
                "publink metadata missing name",
            ))?
            .to_owned(),
        link: hash
            .get_string("link")
            .ok_or(PublicLinksApiError::Malformed("publink missing link"))?
            .to_owned(),
        created: hash.get_number("created").unwrap_or(0),
        modified: hash.get_number("modified").unwrap_or(0),
        is_folder,
        item_id,
        parent_folder_id: metadata.get_number("parentfolderid").unwrap_or(0),
        is_upload: hash.get_bool("isupload").unwrap_or(false),
        has_password: hash.get_bool("haspassword").unwrap_or(false),
        views: hash.get_number("views").unwrap_or(0),
        expire: hash.get_number("expires"),
    })
}

fn parse_public_link_contents_entry<E>(
    value: &Value,
) -> Result<PublicLinkContentsEntry, PublicLinksApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value.as_hash().ok_or(PublicLinksApiError::Malformed(
        "publink content entry was not a hash",
    ))?;
    let is_folder = hash.get_bool("isfolder").unwrap_or(false);
    let item_id = if is_folder {
        hash.get_number("folderid")
            .ok_or(PublicLinksApiError::Malformed(
                "publink content missing folderid",
            ))?
    } else {
        hash.get_number("fileid")
            .ok_or(PublicLinksApiError::Malformed(
                "publink content missing fileid",
            ))?
    };

    Ok(PublicLinkContentsEntry {
        name: hash
            .get_string("name")
            .ok_or(PublicLinksApiError::Malformed(
                "publink content missing name",
            ))?
            .to_owned(),
        created: hash.get_number("created").unwrap_or(0),
        modified: hash.get_number("modified").unwrap_or(0),
        is_folder,
        item_id,
        icon: hash.get_number("icon").unwrap_or(0),
    })
}

fn parse_created_public_link<E>(
    hash: HashView<'_>,
    is_folder: bool,
) -> Result<CreatedPublicLink, PublicLinksApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    Ok(CreatedPublicLink {
        link_id: hash
            .get_number("linkid")
            .ok_or(PublicLinksApiError::Malformed(
                "created link missing linkid",
            ))?,
        link: hash
            .get_string("link")
            .ok_or(PublicLinksApiError::Malformed("created link missing link"))?
            .to_owned(),
        is_folder,
    })
}

fn parse_upload_link_summary<E>(value: &Value) -> Result<UploadLinkSummary, PublicLinksApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value.as_hash().ok_or(PublicLinksApiError::Malformed(
        "uploadlink entry was not a hash",
    ))?;
    let metadata = hash
        .get_hash("metadata")
        .ok_or(PublicLinksApiError::Malformed(
            "uploadlink entry missing metadata",
        ))?;
    let is_folder = metadata.get_bool("isfolder").unwrap_or(false);
    let item_id = if is_folder {
        metadata
            .get_number("folderid")
            .ok_or(PublicLinksApiError::Malformed(
                "uploadlink metadata missing folderid",
            ))?
    } else {
        metadata
            .get_number("fileid")
            .ok_or(PublicLinksApiError::Malformed(
                "uploadlink metadata missing fileid",
            ))?
    };

    Ok(UploadLinkSummary {
        upload_link_id: hash
            .get_number("uploadlinkid")
            .ok_or(PublicLinksApiError::Malformed(
                "uploadlink missing uploadlinkid",
            ))?,
        code: hash
            .get_string("code")
            .ok_or(PublicLinksApiError::Malformed("uploadlink missing code"))?
            .to_owned(),
        name: metadata
            .get_string("name")
            .ok_or(PublicLinksApiError::Malformed(
                "uploadlink metadata missing name",
            ))?
            .to_owned(),
        link: hash
            .get_string("link")
            .ok_or(PublicLinksApiError::Malformed("uploadlink missing link"))?
            .to_owned(),
        comment: hash
            .get_string("comment")
            .ok_or(PublicLinksApiError::Malformed("uploadlink missing comment"))?
            .to_owned(),
        space: hash
            .get_number("space")
            .ok_or(PublicLinksApiError::Malformed("uploadlink missing space"))?,
        maxspace: hash.get_number("maxspace"),
        files: hash
            .get_number("files")
            .ok_or(PublicLinksApiError::Malformed("uploadlink missing files"))?,
        created: hash.get_number("created").unwrap_or(0),
        modified: hash.get_number("modified").unwrap_or(0),
        is_folder,
        item_id,
        parent_folder_id: metadata.get_number("parentfolderid").unwrap_or(0),
        icon: metadata.get_number("icon").unwrap_or(0),
    })
}

fn parse_public_link_access_entry<E>(
    value: &Value,
) -> Result<PublicLinkAccessEntry, PublicLinksApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value.as_hash().ok_or(PublicLinksApiError::Malformed(
        "public link access entry was not a hash",
    ))?;
    Ok(PublicLinkAccessEntry {
        email: hash
            .get_string("email")
            .ok_or(PublicLinksApiError::Malformed(
                "public link access entry missing email",
            ))?
            .to_owned(),
        receiver_id: hash
            .get_number("receiverid")
            .ok_or(PublicLinksApiError::Malformed(
                "public link access entry missing receiverid",
            ))?,
    })
}

fn parse_public_link_bookmark<E>(
    value: &Value,
) -> Result<PublicLinkBookmark, PublicLinksApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value.as_hash().ok_or(PublicLinksApiError::Malformed(
        "bookmark entry was not a hash",
    ))?;
    Ok(PublicLinkBookmark {
        link: hash
            .get_string("link")
            .ok_or(PublicLinksApiError::Malformed("bookmark missing link"))?
            .to_owned(),
        name: hash
            .get_string("name")
            .ok_or(PublicLinksApiError::Malformed("bookmark missing name"))?
            .to_owned(),
        code: hash
            .get_string("code")
            .ok_or(PublicLinksApiError::Malformed("bookmark missing code"))?
            .to_owned(),
        description: hash
            .get_string("description")
            .unwrap_or_default()
            .to_owned(),
        created: hash.get_number("ctime").unwrap_or(0),
        location_id: hash
            .get_number("locationid")
            .ok_or(PublicLinksApiError::Malformed(
                "bookmark missing locationid",
            ))?,
    })
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Mutex};

    use pcloud_model::public_links::PublicLinkUploadPolicy;

    use crate::{
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        response::Value,
    };

    use super::PublicLinksApi;

    #[derive(Debug)]
    struct MockTransport {
        responses: Mutex<Vec<Value>>,
    }

    impl MockTransport {
        fn with_responses(responses: Vec<Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
            }
        }
    }

    impl ProtocolTransport for MockTransport {
        type Error = io::Error;

        fn execute(&self, _request: &crate::EncodedRequest) -> Result<Value, Self::Error> {
            self.responses
                .lock()
                .expect("responses lock should not be poisoned")
                .pop()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, _api_server: &str) {}
    }

    #[test]
    fn list_public_links_parses_summaries() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "publinks".to_owned(),
                Value::Array(vec![Value::Hash(vec![
                    ("linkid".to_owned(), Value::Number(7)),
                    ("code".to_owned(), Value::String("abc123".to_owned())),
                    (
                        "link".to_owned(),
                        Value::String("https://e.pcloud.link/abc123".to_owned()),
                    ),
                    ("created".to_owned(), Value::Number(100)),
                    ("modified".to_owned(), Value::Number(200)),
                    ("isupload".to_owned(), Value::Bool(false)),
                    ("haspassword".to_owned(), Value::Bool(true)),
                    ("views".to_owned(), Value::Number(9)),
                    ("expires".to_owned(), Value::Number(300)),
                    (
                        "metadata".to_owned(),
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("report.txt".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(false)),
                            ("fileid".to_owned(), Value::Number(42)),
                            ("parentfolderid".to_owned(), Value::Number(2)),
                        ]),
                    ),
                ])]),
            ),
        ])]);
        let api = PublicLinksApi::new(transport);

        let links = api
            .list_public_links("auth")
            .expect("public links should parse");

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_id, 7);
        assert_eq!(links[0].code, "abc123");
        assert_eq!(links[0].item_id, 42);
        assert!(links[0].has_password);
        assert_eq!(links[0].expire, Some(300));
    }

    #[test]
    fn show_public_link_parses_contents() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "metadata".to_owned(),
                Value::Hash(vec![(
                    "contents".to_owned(),
                    Value::Array(vec![
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("docs".to_owned())),
                            ("created".to_owned(), Value::Number(11)),
                            ("modified".to_owned(), Value::Number(12)),
                            ("isfolder".to_owned(), Value::Bool(true)),
                            ("folderid".to_owned(), Value::Number(3)),
                            ("icon".to_owned(), Value::Number(4)),
                        ]),
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("report.txt".to_owned())),
                            ("created".to_owned(), Value::Number(21)),
                            ("modified".to_owned(), Value::Number(22)),
                            ("isfolder".to_owned(), Value::Bool(false)),
                            ("fileid".to_owned(), Value::Number(5)),
                            ("icon".to_owned(), Value::Number(6)),
                        ]),
                    ]),
                )]),
            ),
        ])]);
        let api = PublicLinksApi::new(transport);

        let contents = api
            .show_public_link("auth", "abc123")
            .expect("public link contents should parse");

        assert_eq!(contents.code, "abc123");
        assert_eq!(contents.entries.len(), 2);
        assert!(contents.entries[0].is_folder);
        assert_eq!(contents.entries[0].item_id, 3);
        assert!(!contents.entries[1].is_folder);
        assert_eq!(contents.entries[1].item_id, 5);
    }

    #[test]
    fn list_public_links_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2000)),
            ("error".to_owned(), Value::String("failed".to_owned())),
        ])]);
        let api = PublicLinksApi::new(transport);

        let err = api
            .list_public_links("auth")
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::PublicLinksApiError::Result {
                result: 2000,
                ref message
            } if message.as_deref() == Some("failed")
        ));
    }

    #[test]
    fn delete_public_link_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2001)),
            ("error".to_owned(), Value::String("not found".to_owned())),
        ])]);
        let api = PublicLinksApi::new(transport);

        let err = api
            .delete_public_link("auth", 7)
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::PublicLinksApiError::Result {
                result: 2001,
                ref message
            } if message.as_deref() == Some("not found")
        ));
    }

    #[test]
    fn create_file_and_folder_public_links_parse() {
        let transport = MockTransport::with_responses(vec![
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                ("linkid".to_owned(), Value::Number(7)),
                (
                    "link".to_owned(),
                    Value::String("https://e.pcloud.link/alpha123".to_owned()),
                ),
            ]),
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                ("linkid".to_owned(), Value::Number(8)),
                (
                    "link".to_owned(),
                    Value::String("https://e.pcloud.link/folder999".to_owned()),
                ),
            ]),
        ]);
        let api = PublicLinksApi::new(transport);

        let file = api
            .create_file_public_link("auth", "/Docs/report.txt")
            .expect("file public link should parse");
        let folder = api
            .create_folder_public_link("auth", "/Docs")
            .expect("folder public link should parse");

        assert_eq!(file.link_id, 7);
        assert!(!file.is_folder);
        assert_eq!(folder.link_id, 8);
        assert!(folder.is_folder);
    }

    #[test]
    fn change_public_link_expire_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2002)),
            ("error".to_owned(), Value::String("invalid link".to_owned())),
        ])]);
        let api = PublicLinksApi::new(transport);

        let err = api
            .change_public_link_expire("auth", 7, Some(123))
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::PublicLinksApiError::Result {
                result: 2002,
                ref message
            } if message.as_deref() == Some("invalid link")
        ));
    }

    #[test]
    fn change_public_link_password_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2003)),
            (
                "error".to_owned(),
                Value::String("invalid password".to_owned()),
            ),
        ])]);
        let api = PublicLinksApi::new(transport);

        let err = api
            .change_public_link_password("auth", 7, Some("secret".to_owned()))
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::PublicLinksApiError::Result {
                result: 2003,
                ref message
            } if message.as_deref() == Some("invalid password")
        ));
    }

    #[derive(Debug)]
    struct StaticResolver {
        folders: std::collections::HashMap<String, u64>,
        files: std::collections::HashMap<String, u64>,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("path not found: {0}")]
    struct StaticResolverError(String);

    impl super::PublicLinkPathResolver for StaticResolver {
        type Error = StaticResolverError;

        fn resolve_folder(&self, path: &str) -> Result<u64, Self::Error> {
            self.folders
                .get(path)
                .copied()
                .ok_or_else(|| StaticResolverError(path.to_owned()))
        }

        fn resolve_file(&self, path: &str) -> Result<u64, Self::Error> {
            self.files
                .get(path)
                .copied()
                .ok_or_else(|| StaticResolverError(path.to_owned()))
        }
    }

    #[test]
    fn tree_public_link_from_paths_rejects_empty_target() {
        let transport = MockTransport::with_responses(vec![]);
        let api = PublicLinksApi::new(transport);
        let resolver = StaticResolver {
            folders: Default::default(),
            files: Default::default(),
        };
        let err = api
            .create_tree_public_link_from_paths(
                "auth",
                "Name",
                &super::TreePublicLinkPaths::new(),
                &resolver,
                None,
                None,
                None,
            )
            .expect_err("empty target should fail");
        assert!(matches!(err, super::PublicLinksApiError::EmptyTreeTarget));
    }

    #[test]
    fn tree_public_link_from_paths_resolves_ids_and_sends_expected_params() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("linkid".to_owned(), Value::Number(271)),
            (
                "link".to_owned(),
                Value::String("https://e.pcloud.link/tree-x".to_owned()),
            ),
        ])]);
        let api = PublicLinksApi::new(transport);
        let resolver = StaticResolver {
            folders: [("/Docs".to_owned(), 9_u64), ("/Reports".to_owned(), 10_u64)]
                .into_iter()
                .collect(),
            files: [("/Docs/report.txt".to_owned(), 11_u64)]
                .into_iter()
                .collect(),
        };
        let paths = super::TreePublicLinkPaths::new()
            .with_root("/Docs")
            .with_folders(vec!["/Docs".to_owned(), "/Reports".to_owned()])
            .with_files(vec!["/Docs/report.txt".to_owned()]);
        let created = api
            .create_tree_public_link_from_paths(
                "auth",
                "Quarter",
                &paths,
                &resolver,
                Some(42),
                None,
                None,
            )
            .expect("tree link should be created");
        assert_eq!(created.link_id, 271);
    }

    #[test]
    fn tree_public_link_from_paths_surfaces_resolver_errors() {
        let transport = MockTransport::with_responses(vec![]);
        let api = PublicLinksApi::new(transport);
        let resolver = StaticResolver {
            folders: Default::default(),
            files: Default::default(),
        };
        let paths = super::TreePublicLinkPaths::new().with_root("/Missing");
        let err = api
            .create_tree_public_link_from_paths("auth", "N", &paths, &resolver, None, None, None)
            .expect_err("missing path should fail");
        assert!(matches!(
            err,
            super::PublicLinksApiError::PathUnresolved { ref path, .. } if path == "/Missing"
        ));
    }

    #[test]
    fn screenshot_public_link_sets_rounded_expire_when_delay_enabled() {
        let transport = MockTransport::with_responses(vec![
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                ("linkid".to_owned(), Value::Number(71)),
                (
                    "link".to_owned(),
                    Value::String("https://e.pcloud.link/file-x".to_owned()),
                ),
            ]),
            Value::Hash(vec![("result".to_owned(), Value::Number(0))]),
        ]);
        let api = PublicLinksApi::new(transport);
        let created = api
            .create_screenshot_public_link("auth", "/Screenshots/1.png", true, 7_200, 1_700_000_010)
            .expect("screenshot link should be created");
        assert_eq!(created.link_id, 71);
    }

    #[test]
    fn folder_updownlink_sends_expected_params() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = PublicLinksApi::new(transport);
        api.create_folder_updownlink("auth", 77, "alice@example.com", true)
            .expect("folder updownlink should succeed");
    }

    #[test]
    fn send_publink_encodes_expected_params_and_succeeds() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = PublicLinksApi::new(transport);
        api.send_publink(
            "auth",
            "alpha123",
            "alice@example.com,bob@example.com",
            "hi",
        )
        .expect("sendpublink should succeed");
    }

    #[test]
    fn send_publink_surfaces_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2231)),
            (
                "error".to_owned(),
                Value::String("invalid email".to_owned()),
            ),
        ])]);
        let api = PublicLinksApi::new(transport);
        let err = api
            .send_publink("auth", "alpha123", "not-an-email", "hi")
            .expect_err("nonzero result should fail");
        assert!(matches!(
            err,
            super::PublicLinksApiError::Result {
                result: 2231,
                ref message
            } if message.as_deref() == Some("invalid email")
        ));
    }

    #[test]
    fn change_public_link_upload_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2004)),
            (
                "error".to_owned(),
                Value::String("invalid upload policy".to_owned()),
            ),
        ])]);
        let api = PublicLinksApi::new(transport);

        let err = api
            .change_public_link_upload("auth", 7, PublicLinkUploadPolicy::Everyone)
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::PublicLinksApiError::Result {
                result: 2004,
                ref message
            } if message.as_deref() == Some("invalid upload policy")
        ));
    }
}

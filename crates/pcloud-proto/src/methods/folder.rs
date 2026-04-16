//! Wire-level method builders for folder operations (list, create,
//! rename, delete, copy). Consumed by `folder_api`.

use crate::binary_api::BinaryParam;
use crate::methods::ProtocolMethod;

/// `ListFolderByPathRequest` — list folder by path request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListFolderByPathRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `path` field (path).
    pub path: String,
}

impl ListFolderByPathRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "listfolder"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        params.push(BinaryParam::string("path", self.path.as_str()));
        params
    }
}

impl ProtocolMethod for ListFolderByPathRequest {
    fn command_name(&self) -> &'static str {
        ListFolderByPathRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        ListFolderByPathRequest::params(self)
    }
}

/// Parameters for the pCloud `createfolder` (and
/// `createfolderifnotexists`-flavored) endpoint, mirroring the shape used
/// by the C `psync_create_remote_folder` and
/// `psync_create_remote_folder_by_path` calls in `pclsync/psynclib.c`.
///
/// When `parent_folder_id` is `Some`, the request emits a `folderid` +
/// `name` pair (parent-id + leaf-name form). When it is `None`, the
/// request emits a single absolute `path`. `folder_exists_ok = true`
/// switches the wire command to `createfolderifnotexists`, which the
/// pCloud backend treats as idempotent on conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFolderRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// `Some(parent_id)` selects the `folderid` + `name` shape;
    /// `None` selects the absolute-`path` shape.
    pub parent_folder_id: Option<u64>,
    /// Leaf folder name (used with the `folderid` shape) or empty when
    /// `path` is set.
    pub name: String,
    /// Absolute remote path (used when `parent_folder_id` is `None`).
    pub path: String,
    /// When `true` the request uses `createfolderifnotexists` and
    /// returns the existing folder on conflict.
    pub folder_exists_ok: bool,
}

impl CreateFolderRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        if self.folder_exists_ok {
            "createfolderifnotexists"
        } else {
            "createfolder"
        }
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        // auth + (folderid + name | path) + timeformat
        let cap = 2 + if self.parent_folder_id.is_some() {
            2
        } else {
            1
        };
        let mut params = Vec::with_capacity(cap);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        match self.parent_folder_id {
            Some(parent) => {
                params.push(BinaryParam::number("folderid", parent));
                params.push(BinaryParam::string("name", self.name.as_str()));
            }
            None => {
                params.push(BinaryParam::string("path", self.path.as_str()));
            }
        }
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params
    }
}

impl ProtocolMethod for CreateFolderRequest {
    fn command_name(&self) -> &'static str {
        CreateFolderRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        CreateFolderRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// Delete / rename requests used by the FUSE unlink/rename forwarding path.
// Wire shapes mirror:
// - `pclsync/pfsupload.c:1327-1338` (`deletefile`)
// - `pclsync/pfsupload_send.c:60-72` (`deletefolder`)
// - `pclsync/pupload.c:1663-1675` (`deletefolderrecursive`)
// - `pclsync/pupload.c:276-291`, `pclsync/pfsupload.c:1438-1447` (`renamefile`)
// - `pclsync/pupload.c:388-438`, `pclsync/pfsupload.c:1449-1459` (`renamefolder`)
// -----------------------------------------------------------------------------

/// `deletefile` request. Mirrors `psync_send_task_unlink`
/// (`pclsync/pfsupload.c:1327-1338`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteFileRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `file_id` field (file id).
    pub file_id: u64,
}

impl DeleteFileRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "deletefile"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        params.push(BinaryParam::number("fileid", self.file_id));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params
    }
}

impl ProtocolMethod for DeleteFileRequest {
    fn command_name(&self) -> &'static str {
        DeleteFileRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        DeleteFileRequest::params(self)
    }
}

/// `renamefile` request. Mirrors `task_renameremotefile`
/// (`pclsync/pupload.c:276-291`) and `psync_send_task_rename_file`
/// (`pclsync/pfsupload.c:1438-1447`). A rename into the same parent is
/// expressed by passing the existing parent folder id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameFileRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `file_id` field (file id).
    pub file_id: u64,
    /// The `to_folder_id` field (to folder id).
    pub to_folder_id: u64,
    /// The `to_name` field (to name).
    pub to_name: String,
}

impl RenameFileRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "renamefile"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(5);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        params.push(BinaryParam::number("fileid", self.file_id));
        params.push(BinaryParam::number("tofolderid", self.to_folder_id));
        params.push(BinaryParam::string("toname", self.to_name.as_str()));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params
    }
}

impl ProtocolMethod for RenameFileRequest {
    fn command_name(&self) -> &'static str {
        RenameFileRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        RenameFileRequest::params(self)
    }
}

/// `deletefolder` request (non-recursive). Mirrors
/// `psync_send_task_rmdir` (`pclsync/pfsupload_send.c:60-72`). The pCloud
/// backend rejects this if the folder is non-empty, matching POSIX
/// `rmdir` semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteFolderRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
}

impl DeleteFolderRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "deletefolder"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        params.push(BinaryParam::number("folderid", self.folder_id));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params
    }
}

impl ProtocolMethod for DeleteFolderRequest {
    fn command_name(&self) -> &'static str {
        DeleteFolderRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        DeleteFolderRequest::params(self)
    }
}

/// `deletefolderrecursive` request. Mirrors `task_deletefolderrec`
/// (`pclsync/pupload.c:1663-1675`). Note the C call intentionally does
/// not send `timeformat`; we mirror that to match wire shape exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteFolderRecursiveRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
}

impl DeleteFolderRecursiveRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "deletefolderrecursive"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        params.push(BinaryParam::number("folderid", self.folder_id));
        params
    }
}

impl ProtocolMethod for DeleteFolderRecursiveRequest {
    fn command_name(&self) -> &'static str {
        DeleteFolderRecursiveRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        DeleteFolderRecursiveRequest::params(self)
    }
}

/// `renamefolder` request. Mirrors `task_renameremotefolder`
/// (`pclsync/pupload.c:388-438`) and `psync_send_task_rename_folder`
/// (`pclsync/pfsupload.c:1449-1459`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameFolderRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
    /// The `to_folder_id` field (to folder id).
    pub to_folder_id: u64,
    /// The `to_name` field (to name).
    pub to_name: String,
}

impl RenameFolderRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "renamefolder"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(5);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        params.push(BinaryParam::number("folderid", self.folder_id));
        params.push(BinaryParam::number("tofolderid", self.to_folder_id));
        params.push(BinaryParam::string("toname", self.to_name.as_str()));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        params
    }
}

impl ProtocolMethod for RenameFolderRequest {
    fn command_name(&self) -> &'static str {
        RenameFolderRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        RenameFolderRequest::params(self)
    }
}

/// Classifier for result codes returned by the delete/rename endpoints.
/// Mirrors the `UploadErrorClass` pattern (`methods/upload.rs:487-513`),
/// which keys off `psync_handle_api_result` (`pnetlibs.c:341-354`). The
/// same C dispatch handles both upload and fs-mutation tasks, so the
/// classification is intentionally identical:
///
/// - `2000` -> `Auth`: bad/expired login.
/// - `2003 / 2005 / 2007 / 2009 / 2029 / 2067 / 5002` -> `PermFail`: do
///   not retry (e.g. not found, no access, parent deleted).
/// - everything else nonzero -> `TempFail`: retryable (network, rate
///   limits, transient backend failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsMutationErrorClass {
    /// `Auth` variant (auth).
    Auth,
    /// `PermFail` variant (perm fail).
    PermFail,
    /// `TempFail` variant (temp fail).
    TempFail,
}

impl FsMutationErrorClass {
    /// Classify a pCloud `result` number. `0` means success and returns
    /// `None`.
    #[must_use]
    pub fn classify(result: u64) -> Option<Self> {
        match result {
            0 => None,
            2000 => Some(Self::Auth),
            2003 | 2005 | 2007 | 2009 | 2029 | 2067 | 5002 => Some(Self::PermFail),
            _ => Some(Self::TempFail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_folder_by_parent_emits_folderid_name() {
        let request = CreateFolderRequest {
            auth_token: "token".to_owned(),
            parent_folder_id: Some(11),
            name: "Reports".to_owned(),
            path: String::new(),
            folder_exists_ok: false,
        };
        let encoded = request.encode().expect("create-folder should encode");
        assert_eq!(encoded.frame.command, "createfolder");
        assert_eq!(encoded.frame.parameter_count, 4);
    }

    #[test]
    fn create_folder_by_path_emits_path() {
        let request = CreateFolderRequest {
            auth_token: "token".to_owned(),
            parent_folder_id: None,
            name: String::new(),
            path: "/Docs/Reports".to_owned(),
            folder_exists_ok: false,
        };
        let encoded = request.encode().expect("create-folder should encode");
        assert_eq!(encoded.frame.command, "createfolder");
        assert_eq!(encoded.frame.parameter_count, 3);
    }

    #[test]
    fn create_folder_if_not_exists_uses_idempotent_command() {
        let request = CreateFolderRequest {
            auth_token: "token".to_owned(),
            parent_folder_id: Some(0),
            name: "Reports".to_owned(),
            path: String::new(),
            folder_exists_ok: true,
        };
        let encoded = request.encode().expect("create-folder should encode");
        assert_eq!(encoded.frame.command, "createfolderifnotexists");
    }

    // ----- Delete / rename wire-shape tests --------------------------------

    #[test]
    fn delete_file_request_emits_fileid_and_timeformat() {
        let request = DeleteFileRequest {
            auth_token: "token".to_owned(),
            file_id: 42,
        };
        let encoded = request.encode().expect("deletefile should encode");
        assert_eq!(encoded.frame.command, "deletefile");
        // auth + fileid + timeformat
        assert_eq!(encoded.frame.parameter_count, 3);
    }

    #[test]
    fn rename_file_request_emits_target_parent_and_name() {
        let request = RenameFileRequest {
            auth_token: "token".to_owned(),
            file_id: 42,
            to_folder_id: 7,
            to_name: "renamed.txt".to_owned(),
        };
        let encoded = request.encode().expect("renamefile should encode");
        assert_eq!(encoded.frame.command, "renamefile");
        // auth + fileid + tofolderid + toname + timeformat
        assert_eq!(encoded.frame.parameter_count, 5);
    }

    #[test]
    fn delete_folder_request_emits_folderid_and_timeformat() {
        let request = DeleteFolderRequest {
            auth_token: "token".to_owned(),
            folder_id: 11,
        };
        let encoded = request.encode().expect("deletefolder should encode");
        assert_eq!(encoded.frame.command, "deletefolder");
        // auth + folderid + timeformat
        assert_eq!(encoded.frame.parameter_count, 3);
    }

    #[test]
    fn delete_folder_recursive_matches_c_wire_shape() {
        // `task_deletefolderrec` (`pclsync/pupload.c:1663-1675`) sends
        // only `auth` + `folderid`. No `timeformat`.
        let request = DeleteFolderRecursiveRequest {
            auth_token: "token".to_owned(),
            folder_id: 11,
        };
        let encoded = request
            .encode()
            .expect("deletefolderrecursive should encode");
        assert_eq!(encoded.frame.command, "deletefolderrecursive");
        assert_eq!(encoded.frame.parameter_count, 2);
    }

    #[test]
    fn rename_folder_request_emits_target_parent_and_name() {
        let request = RenameFolderRequest {
            auth_token: "token".to_owned(),
            folder_id: 11,
            to_folder_id: 3,
            to_name: "Renamed".to_owned(),
        };
        let encoded = request.encode().expect("renamefolder should encode");
        assert_eq!(encoded.frame.command, "renamefolder");
        // auth + folderid + tofolderid + toname + timeformat
        assert_eq!(encoded.frame.parameter_count, 5);
    }

    #[test]
    fn fs_mutation_error_class_classifies_codes() {
        assert_eq!(FsMutationErrorClass::classify(0), None);
        assert_eq!(
            FsMutationErrorClass::classify(2000),
            Some(FsMutationErrorClass::Auth)
        );
        for code in [2003u64, 2005, 2007, 2009, 2029, 2067, 5002] {
            assert_eq!(
                FsMutationErrorClass::classify(code),
                Some(FsMutationErrorClass::PermFail),
                "code {code} should be PermFail"
            );
        }
        for code in [1u64, 2001, 4000, 5000, 5001] {
            assert_eq!(
                FsMutationErrorClass::classify(code),
                Some(FsMutationErrorClass::TempFail),
                "code {code} should be TempFail"
            );
        }
    }
}

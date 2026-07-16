//! Stable, filesystem-focused SDK facade.
//!
//! This module deliberately owns every public type it exposes. The daemon's
//! IPC schema and backend runtimes remain implementation details, so their
//! evolution does not force application authors to track workspace-internal
//! crates.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use pcloud_ipc::{Request, Response, ResponseStatus};
use thiserror::Error;

use crate::EmbeddedDaemon;

/// Stable, kind-carrying identifier for a remote drive entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteEntryId {
    /// A remote folder id.
    Folder(u64),
    /// A remote file id.
    File(u64),
}

impl RemoteEntryId {
    /// Return the numeric pCloud id.
    #[must_use]
    pub const fn value(self) -> u64 {
        match self {
            Self::Folder(id) | Self::File(id) => id,
        }
    }

    /// Return whether this identifies a folder.
    #[must_use]
    pub const fn is_folder(self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

/// Owned metadata for one remote drive entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoteEntry {
    /// Kind-carrying remote id.
    pub id: RemoteEntryId,
    /// Direct parent folder id, when known. The drive root has no parent.
    pub parent_folder_id: Option<u64>,
    /// Leaf name. The drive root uses `/`.
    pub name: String,
    /// File size, or `None` for folders/unknown metadata.
    pub size: Option<u64>,
    /// Last modification time in Unix seconds, when supplied.
    pub modified: Option<u64>,
    /// Creation time in Unix seconds, when supplied.
    pub created: Option<u64>,
    /// Whether the current account owns the entry.
    pub is_mine: bool,
    /// Whether the entry is shared.
    pub is_shared: bool,
    /// Whether the entry is inside pCloud Crypto.
    pub encrypted: bool,
    /// Effective pCloud permission bitmap, when supplied.
    pub permissions: Option<u32>,
}

/// An authoritative folder listing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoteListing {
    /// Metadata for the listed folder.
    pub folder: RemoteEntry,
    /// Immediate children in server order.
    pub entries: Vec<RemoteEntry>,
}

/// A bounded range read and its EOF metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoteRead {
    /// Bytes returned for this range.
    pub data: Vec<u8>,
    /// Full remote file size.
    pub total_size: u64,
    /// Whether this read reached the remote EOF.
    pub eof: bool,
}

/// Aggregate counters from a remote recursive copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoteCopyResult {
    /// Files copied.
    pub files: u64,
    /// Folders created.
    pub folders: u64,
    /// File bytes copied.
    pub bytes: u64,
}

/// Receipt for a streamed local-to-remote upload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoteUploadResult {
    /// Server upload-session id used for the committed write.
    pub upload_id: u64,
    /// File id supplied by pCloud, when available.
    pub file_id: Option<u64>,
    /// Number of source bytes acknowledged before commit.
    pub bytes: u64,
    /// Lowercase SHA-1 verified before publication.
    pub sha1_hex: String,
    /// Durable offset reused from an interrupted attempt.
    pub resumed_from: u64,
}

/// Receipt for a streamed remote-to-local download.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoteDownloadResult {
    /// Published local destination.
    pub path: PathBuf,
    /// Bytes present after the crash-safe publication step.
    pub bytes: u64,
    /// Lowercase SHA-256 of the published local file.
    pub sha256_hex: String,
    /// Durable offset reused from an interrupted attempt.
    pub resumed_from: u64,
}

/// Errors returned by the focused remote-drive SDK.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RemoteDriveError {
    /// The request was malformed or violated an operation precondition.
    #[error("invalid remote-drive request: {0}")]
    InvalidRequest(String),
    /// No authenticated session is active.
    #[error("remote-drive operation requires an authenticated session: {0}")]
    Unauthorized(String),
    /// The request conflicts with current remote or local state.
    #[error("remote-drive conflict: {0}")]
    Conflict(String),
    /// A transport or remote subsystem is temporarily unavailable.
    #[error("remote-drive service unavailable: {0}")]
    Unavailable(String),
    /// A configured policy refused the request.
    #[error("remote-drive policy {kind:?} refused the request: {message}")]
    Policy {
        /// Stable policy discriminator.
        kind: String,
        /// Human-readable refusal reason.
        message: String,
    },
    /// The daemon returned an unexpected backend failure.
    #[error("remote-drive backend failure: {0}")]
    Backend(String),
    /// A successful daemon response did not match the documented payload.
    #[error("invalid remote-drive response: {0}")]
    Protocol(String),
    /// Local filesystem inspection failed.
    #[error(transparent)]
    LocalIo(#[from] std::io::Error),
}

/// Borrowed, focused view of an [`EmbeddedDaemon`]'s remote drive.
///
/// Obtain this with [`EmbeddedDaemon::remote`]. All operations route through
/// the same canonical, live, ID-first service used by the CLI and daemon.
pub struct RemoteDrive<'a> {
    daemon: &'a mut EmbeddedDaemon,
}

impl std::fmt::Debug for RemoteDrive<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteDrive")
            .finish_non_exhaustive()
    }
}

impl EmbeddedDaemon {
    /// Borrow the stable remote-drive API.
    ///
    /// The borrow prevents interleaving mutable raw-dispatch calls with one
    /// high-level operation and is released when the returned value drops.
    pub fn remote(&mut self) -> RemoteDrive<'_> {
        RemoteDrive { daemon: self }
    }
}

impl RemoteDrive<'_> {
    /// Resolve authoritative metadata for an absolute remote path.
    pub fn stat(&mut self, path: &str) -> Result<RemoteEntry, RemoteDriveError> {
        validate_remote_path(path)?;
        let response = self.daemon.dispatch(Request::StatPath {
            path: path.to_owned(),
        });
        let body = successful_body(response)?;
        let payload: pcloud_ipc::StatPathPayload = serde_json::from_str(&body)
            .map_err(|error| RemoteDriveError::Protocol(error.to_string()))?;
        Ok(entry_from_stat(payload))
    }

    /// List the immediate children of an absolute remote folder path.
    pub fn list(&mut self, path: &str) -> Result<RemoteListing, RemoteDriveError> {
        validate_remote_path(path)?;
        let folder = self.stat(path)?;
        if !folder.id.is_folder() {
            return Err(RemoteDriveError::Conflict(format!(
                "expected folder at {path}, found file"
            )));
        }
        let response = self.daemon.dispatch(Request::ListFolderByPath {
            path: path.to_owned(),
        });
        let body = successful_body(response)?;
        let payload: Vec<pcloud_ipc::ListFolderEntry> = serde_json::from_str(&body)
            .map_err(|error| RemoteDriveError::Protocol(error.to_string()))?;
        let parent_id = Some(folder.id.value());
        let entries = payload
            .into_iter()
            .map(|entry| entry_from_listing(entry, parent_id))
            .collect();
        Ok(RemoteListing { folder, entries })
    }

    /// Read a bounded byte range from a remote file.
    ///
    /// One call is capped by the daemon at 8 MiB. Use consecutive ranges for
    /// larger streams, or [`Self::download`] for a local file destination.
    pub fn read_range(
        &mut self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> Result<RemoteRead, RemoteDriveError> {
        validate_remote_path(path)?;
        if length == 0 {
            return Err(RemoteDriveError::InvalidRequest(
                "range length must be greater than zero".to_owned(),
            ));
        }
        let response = self.daemon.dispatch(Request::ReadFileRange {
            path: path.to_owned(),
            offset,
            length,
        });
        let body = successful_body(response)?;
        let payload: pcloud_ipc::ReadRangePayload = serde_json::from_str(&body)
            .map_err(|error| RemoteDriveError::Protocol(error.to_string()))?;
        let data = B64
            .decode(payload.data_b64.as_bytes())
            .map_err(|error| RemoteDriveError::Protocol(error.to_string()))?;
        if data.len() as u64 != payload.bytes_returned {
            return Err(RemoteDriveError::Protocol(format!(
                "range payload declared {} bytes but decoded {}",
                payload.bytes_returned,
                data.len()
            )));
        }
        Ok(RemoteRead {
            data,
            total_size: payload.total_size,
            eof: payload.eof,
        })
    }

    /// Stream a local regular file to an absolute remote destination.
    pub fn upload(
        &mut self,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<RemoteUploadResult, RemoteDriveError> {
        validate_remote_path(remote_path)?;
        let response = self.daemon.dispatch(Request::UploadFileByPath {
            local_path: local_path.to_owned(),
            remote_path: remote_path.to_owned(),
        });
        let body = successful_body(response)?;
        let payload: pcloud_ipc::RemoteUploadPayload = serde_json::from_str(&body)
            .map_err(|error| RemoteDriveError::Protocol(error.to_string()))?;
        Ok(RemoteUploadResult {
            upload_id: payload.upload_id,
            file_id: payload.file_id,
            bytes: payload.bytes,
            sha1_hex: payload.sha1_hex,
            resumed_from: payload.resumed_from,
        })
    }

    /// Stream a remote file into a crash-safe local destination.
    pub fn download(
        &mut self,
        remote_path: &str,
        local_path: &Path,
        overwrite: bool,
    ) -> Result<RemoteDownloadResult, RemoteDriveError> {
        validate_remote_path(remote_path)?;
        let response = self.daemon.dispatch(Request::DownloadFileByPath {
            remote_path: remote_path.to_owned(),
            local_path: local_path.to_owned(),
            overwrite,
        });
        let body = successful_body(response)?;
        let payload: pcloud_ipc::RemoteDownloadPayload = serde_json::from_str(&body)
            .map_err(|error| RemoteDriveError::Protocol(error.to_string()))?;
        Ok(RemoteDownloadResult {
            path: payload.path,
            bytes: payload.bytes,
            sha256_hex: payload.sha256_hex,
            resumed_from: payload.resumed_from,
        })
    }

    /// Recursively copy a remote file or folder tree.
    pub fn copy(&mut self, from: &str, to: &str) -> Result<RemoteCopyResult, RemoteDriveError> {
        validate_remote_path(from)?;
        validate_remote_path(to)?;
        let response = self.daemon.dispatch(Request::CopyPath {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        let body = successful_body(response)?;
        let payload: pcloud_ipc::RemoteCopyPayload = serde_json::from_str(&body)
            .map_err(|error| RemoteDriveError::Protocol(error.to_string()))?;
        Ok(RemoteCopyResult {
            files: payload.files,
            folders: payload.folders,
            bytes: payload.bytes,
        })
    }

    /// Rename or move a remote file or folder.
    pub fn move_path(&mut self, from: &str, to: &str) -> Result<(), RemoteDriveError> {
        validate_remote_path(from)?;
        validate_remote_path(to)?;
        let response = self.daemon.dispatch(Request::RenamePath {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        successful_body(response).map(|_| ())
    }

    /// Idempotently delete a remote file or folder.
    pub fn delete(&mut self, path: &str, recursive: bool) -> Result<(), RemoteDriveError> {
        validate_remote_path(path)?;
        let response = self.daemon.dispatch(Request::DeletePath {
            path: path.to_owned(),
            recursive,
        });
        successful_body(response).map(|_| ())
    }

    /// Create one remote folder.
    pub fn mkdir(&mut self, path: &str) -> Result<(), RemoteDriveError> {
        validate_remote_path(path)?;
        let response = self.daemon.dispatch(Request::CreateFolderByPath {
            path: path.to_owned(),
        });
        successful_body(response).map(|_| ())
    }

    /// Share a remote folder with an email recipient.
    #[allow(clippy::too_many_arguments)]
    pub fn share_folder(
        &mut self,
        path: &str,
        mail: &str,
        message: &str,
        permissions_bits: u32,
        hint: Option<String>,
    ) -> Result<(), RemoteDriveError> {
        let folder = self.stat(path)?;
        let RemoteEntryId::Folder(folder_id) = folder.id else {
            return Err(RemoteDriveError::Conflict(format!(
                "expected folder at {path}, found file"
            )));
        };
        let response = self.daemon.dispatch(Request::ShareFolder {
            folder_id,
            name: folder.name,
            mail: mail.to_owned(),
            message: message.to_owned(),
            permissions_bits,
            hint,
        });
        successful_body(response).map(|_| ())
    }
}

fn validate_remote_path(path: &str) -> Result<(), RemoteDriveError> {
    if path.is_empty() || !path.starts_with('/') || path.as_bytes().contains(&0) {
        return Err(RemoteDriveError::InvalidRequest(
            "remote paths must be absolute, non-empty, and contain no NUL".to_owned(),
        ));
    }
    Ok(())
}

fn successful_body(response: Response) -> Result<String, RemoteDriveError> {
    match response.status {
        ResponseStatus::Ok => Ok(response.message),
        ResponseStatus::InvalidRequest => Err(RemoteDriveError::InvalidRequest(response.message)),
        ResponseStatus::Unauthorized => Err(RemoteDriveError::Unauthorized(response.message)),
        ResponseStatus::Conflict => Err(RemoteDriveError::Conflict(response.message)),
        ResponseStatus::Unavailable => Err(RemoteDriveError::Unavailable(response.message)),
        ResponseStatus::InternalError => Err(RemoteDriveError::Backend(response.message)),
        ResponseStatus::PolicyViolation { kind } => Err(RemoteDriveError::Policy {
            kind,
            message: response.message,
        }),
        _ => Err(RemoteDriveError::Backend(response.message)),
    }
}

fn entry_from_stat(payload: pcloud_ipc::StatPathPayload) -> RemoteEntry {
    let is_folder = payload.is_folder;
    RemoteEntry {
        id: if is_folder {
            RemoteEntryId::Folder(payload.file_id)
        } else {
            RemoteEntryId::File(payload.file_id)
        },
        parent_folder_id: if payload.name == "/" {
            None
        } else {
            Some(payload.parent_folder_id)
        },
        name: payload.name,
        size: (!is_folder).then_some(payload.size),
        modified: (payload.modified > 0).then_some(payload.modified as u64),
        created: (payload.created > 0).then_some(payload.created as u64),
        is_mine: payload.is_mine,
        is_shared: payload.is_shared,
        encrypted: payload.encrypted,
        permissions: payload.permissions,
    }
}

fn entry_from_listing(
    payload: pcloud_ipc::ListFolderEntry,
    parent_folder_id: Option<u64>,
) -> RemoteEntry {
    let is_folder = payload.is_folder;
    RemoteEntry {
        id: if is_folder {
            RemoteEntryId::Folder(payload.file_id)
        } else {
            RemoteEntryId::File(payload.file_id)
        },
        parent_folder_id,
        name: payload.name,
        size: (!is_folder).then_some(payload.size),
        modified: (payload.modified > 0).then_some(payload.modified as u64),
        created: (payload.created > 0).then_some(payload.created as u64),
        is_mine: payload.is_mine,
        is_shared: payload.is_shared,
        encrypted: payload.encrypted,
        permissions: payload.permissions,
    }
}

#[cfg(test)]
mod tests {
    use pcloud_config::Environment;
    use pcloud_ipc::{Request, ResponseStatus};

    use super::*;

    fn authenticated_daemon() -> (tempfile::TempDir, EmbeddedDaemon) {
        let root = tempfile::tempdir().expect("temporary SDK root");
        let mut daemon = EmbeddedDaemon::builder(root.path().to_owned())
            .environment(Environment::Development)
            .build()
            .expect("development daemon");
        let response = daemon.dispatch(Request::AuthTokenSubmission {
            value: "remote-sdk-test-token".to_owned().into(),
        });
        assert_eq!(response.status, ResponseStatus::Ok, "{}", response.message);
        (root, daemon)
    }

    #[test]
    fn focused_sdk_uses_live_remote_contract_from_a_fresh_root() {
        let (_root, mut daemon) = authenticated_daemon();
        let mut remote = daemon.remote();

        let notes = remote.stat("/notes.txt").expect("live stat");
        assert_eq!(notes.id, RemoteEntryId::File(20));
        assert_eq!(notes.size, Some(1024));

        let root = remote.list("/").expect("live list");
        assert!(root.entries.iter().any(|entry| entry.name == "notes.txt"));

        let range = remote
            .read_range("/notes.txt", 0, 7)
            .expect("bounded range read");
        assert_eq!(range.data, b"downloa");
        assert!(!range.eof);
    }

    #[test]
    fn focused_sdk_streams_upload_download_and_mkdir() {
        let (root, mut daemon) = authenticated_daemon();
        let source = root.path().join("source.bin");
        std::fs::write(&source, b"streamed SDK fixture").unwrap();
        let destination = root.path().join("notes.bin");
        let mut remote = daemon.remote();

        let upload = remote
            .upload(&source, "/sdk-upload.bin")
            .expect("streaming upload");
        assert_eq!(upload.bytes, 20);
        assert_eq!(upload.upload_id, 77);

        remote.mkdir("/SdkFolder").expect("remote mkdir");
        let download = remote
            .download("/notes.txt", &destination, false)
            .expect("crash-safe download");
        assert_eq!(download.bytes, 30);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"downloaded:/get/abc/report.txt"
        );
    }
}

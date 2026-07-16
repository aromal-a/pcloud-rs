#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
//! Stable, filesystem-focused pCloud SDK.
//!
//! This crate is a blocking client for the owner-authenticated local
//! `pcloudd` endpoint. Its public contract contains only SDK-owned types; the
//! daemon IPC schema and backend crates remain implementation details. Every
//! operation reaches the daemon's canonical, live, ID-first `RemoteFs`
//! service.
//!
//! The daemon must already be running and authenticated. Construct a
//! [`Client`] with the socket path from the active pcloud-rs configuration,
//! then borrow its [`RemoteDrive`] view.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use pcloud_ipc::{IpcClient, Request, Response, ResponseStatus};
use thiserror::Error;

type RequestSender = dyn Fn(&Path, &Request) -> Result<Response, String> + Send + Sync;

/// Package version of the stable SDK contract.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Blocking client for an owner-authenticated `pcloudd` endpoint.
///
/// Clones share the immutable transport callback and endpoint configuration;
/// each operation opens its own IPC connection, so no connection or response
/// state is shared between calls.
#[derive(Clone)]
pub struct Client {
    socket_path: PathBuf,
    sender: Arc<RequestSender>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Configure a client for the daemon endpoint at `socket_path`.
    ///
    /// Construction performs no I/O. On Unix this is the AF_UNIX socket path.
    /// On Windows the argument is retained for cross-platform configuration
    /// symmetry while the IPC layer derives the named-pipe endpoint from the
    /// current user's SID.
    #[must_use]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            sender: Arc::new(|path, request| {
                IpcClient
                    .send(path, request)
                    .map_err(|error| error.to_string())
            }),
        }
    }

    /// Borrow the stable remote-drive API.
    #[must_use]
    pub const fn remote(&self) -> RemoteDrive<'_> {
        RemoteDrive { client: self }
    }

    fn dispatch(&self, request: &Request) -> Result<Response, Error> {
        (self.sender)(&self.socket_path, request).map_err(Error::Transport)
    }

    #[cfg(test)]
    fn with_sender<F>(sender: F) -> Self
    where
        F: Fn(&Path, &Request) -> Result<Response, String> + Send + Sync + 'static,
    {
        Self {
            socket_path: PathBuf::from("test-endpoint"),
            sender: Arc::new(sender),
        }
    }
}

/// Stable, kind-carrying identifier for a remote drive entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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

/// Typed permissions for a folder-share invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SharePermissions {
    /// Recipient may create new entries.
    pub create: bool,
    /// Recipient may modify existing entries.
    pub modify: bool,
    /// Recipient may delete entries.
    pub delete: bool,
    /// Recipient may administer and re-share the folder.
    pub manage: bool,
}

impl SharePermissions {
    /// Read-only permissions.
    pub const READ_ONLY: Self = Self {
        create: false,
        modify: false,
        delete: false,
        manage: false,
    };

    /// Read/write permissions without share administration.
    pub const READ_WRITE: Self = Self {
        create: true,
        modify: true,
        delete: true,
        manage: false,
    };

    const fn to_bits(self) -> u32 {
        1 | (self.create as u32) << 1
            | (self.modify as u32) << 2
            | (self.delete as u32) << 3
            | (self.manage as u32) << 4
    }
}

impl Default for SharePermissions {
    fn default() -> Self {
        Self::READ_ONLY
    }
}

/// Options for sharing one remote folder.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShareOptions {
    recipient: String,
    message: String,
    permissions: SharePermissions,
    hint: Option<String>,
}

impl ShareOptions {
    /// Create a read-only invitation for `recipient`.
    #[must_use]
    pub fn new(recipient: impl Into<String>) -> Self {
        Self {
            recipient: recipient.into(),
            message: String::new(),
            permissions: SharePermissions::READ_ONLY,
            hint: None,
        }
    }

    /// Attach a human-readable invitation message.
    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// Set the recipient's permissions.
    #[must_use]
    pub const fn permissions(mut self, permissions: SharePermissions) -> Self {
        self.permissions = permissions;
        self
    }

    /// Attach an optional pCloud share hint.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Errors returned by the stable SDK.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The local owner-authenticated IPC endpoint could not be reached.
    #[error("pcloudd transport failed: {0}")]
    Transport(String),
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
}

/// Borrowed, focused view of a [`Client`]'s remote drive.
#[derive(Debug, Clone, Copy)]
pub struct RemoteDrive<'a> {
    client: &'a Client,
}

impl RemoteDrive<'_> {
    /// Resolve authoritative metadata for an absolute remote path.
    pub fn stat(&self, path: &str) -> Result<RemoteEntry, Error> {
        validate_remote_path(path)?;
        let body = self.send(Request::StatPath {
            path: path.to_owned(),
        })?;
        let payload: pcloud_ipc::StatPathPayload = decode_payload(&body, "stat")?;
        Ok(entry_from_stat(payload))
    }

    /// List the immediate children of an absolute remote folder path.
    pub fn list(&self, path: &str) -> Result<RemoteListing, Error> {
        validate_remote_path(path)?;
        let folder = self.stat(path)?;
        if !folder.id.is_folder() {
            return Err(Error::Conflict(format!(
                "expected folder at {path}, found file"
            )));
        }
        let body = self.send(Request::ListFolderByPath {
            path: path.to_owned(),
        })?;
        let payload: Vec<pcloud_ipc::ListFolderEntry> = decode_payload(&body, "list")?;
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
    pub fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<RemoteRead, Error> {
        validate_remote_path(path)?;
        if length == 0 {
            return Err(Error::InvalidRequest(
                "range length must be greater than zero".to_owned(),
            ));
        }
        let body = self.send(Request::ReadFileRange {
            path: path.to_owned(),
            offset,
            length,
        })?;
        let payload: pcloud_ipc::ReadRangePayload = decode_payload(&body, "range read")?;
        let data = B64
            .decode(payload.data_b64.as_bytes())
            .map_err(|error| Error::Protocol(format!("range read base64: {error}")))?;
        if data.len() as u64 != payload.bytes_returned {
            return Err(Error::Protocol(format!(
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
        &self,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<RemoteUploadResult, Error> {
        validate_remote_path(remote_path)?;
        let body = self.send(Request::UploadFileByPath {
            local_path: local_path.to_owned(),
            remote_path: remote_path.to_owned(),
        })?;
        let payload: pcloud_ipc::RemoteUploadPayload = decode_payload(&body, "upload")?;
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
        &self,
        remote_path: &str,
        local_path: &Path,
        overwrite: bool,
    ) -> Result<RemoteDownloadResult, Error> {
        validate_remote_path(remote_path)?;
        let body = self.send(Request::DownloadFileByPath {
            remote_path: remote_path.to_owned(),
            local_path: local_path.to_owned(),
            overwrite,
        })?;
        let payload: pcloud_ipc::RemoteDownloadPayload = decode_payload(&body, "download")?;
        Ok(RemoteDownloadResult {
            path: payload.path,
            bytes: payload.bytes,
            sha256_hex: payload.sha256_hex,
            resumed_from: payload.resumed_from,
        })
    }

    /// Recursively copy a remote file or folder tree.
    pub fn copy(&self, from: &str, to: &str) -> Result<RemoteCopyResult, Error> {
        validate_remote_path(from)?;
        validate_remote_path(to)?;
        let body = self.send(Request::CopyPath {
            from: from.to_owned(),
            to: to.to_owned(),
        })?;
        let payload: pcloud_ipc::RemoteCopyPayload = decode_payload(&body, "copy")?;
        Ok(RemoteCopyResult {
            files: payload.files,
            folders: payload.folders,
            bytes: payload.bytes,
        })
    }

    /// Rename or move a remote file or folder.
    pub fn move_path(&self, from: &str, to: &str) -> Result<(), Error> {
        validate_remote_path(from)?;
        validate_remote_path(to)?;
        self.send(Request::RenamePath {
            from: from.to_owned(),
            to: to.to_owned(),
        })?;
        Ok(())
    }

    /// Idempotently delete a remote file or folder.
    pub fn delete(&self, path: &str, recursive: bool) -> Result<(), Error> {
        validate_remote_path(path)?;
        self.send(Request::DeletePath {
            path: path.to_owned(),
            recursive,
        })?;
        Ok(())
    }

    /// Create one remote folder.
    pub fn mkdir(&self, path: &str) -> Result<(), Error> {
        validate_remote_path(path)?;
        self.send(Request::CreateFolderByPath {
            path: path.to_owned(),
        })?;
        Ok(())
    }

    /// Share a remote folder with an email recipient.
    pub fn share_folder(&self, path: &str, options: &ShareOptions) -> Result<(), Error> {
        let folder = self.stat(path)?;
        let RemoteEntryId::Folder(folder_id) = folder.id else {
            return Err(Error::Conflict(format!(
                "expected folder at {path}, found file"
            )));
        };
        self.send(Request::ShareFolder {
            folder_id,
            name: folder.name,
            mail: options.recipient.clone(),
            message: options.message.clone(),
            permissions_bits: options.permissions.to_bits(),
            hint: options.hint.clone(),
        })?;
        Ok(())
    }

    fn send(&self, request: Request) -> Result<String, Error> {
        successful_body(self.client.dispatch(&request)?)
    }
}

fn validate_remote_path(path: &str) -> Result<(), Error> {
    if path.is_empty() || !path.starts_with('/') || path.as_bytes().contains(&0) {
        return Err(Error::InvalidRequest(
            "remote paths must be absolute, non-empty, and contain no NUL".to_owned(),
        ));
    }
    Ok(())
}

fn successful_body(response: Response) -> Result<String, Error> {
    match response.status {
        ResponseStatus::Ok => Ok(response.message),
        ResponseStatus::InvalidRequest => Err(Error::InvalidRequest(response.message)),
        ResponseStatus::Unauthorized => Err(Error::Unauthorized(response.message)),
        ResponseStatus::Conflict => Err(Error::Conflict(response.message)),
        ResponseStatus::Unavailable => Err(Error::Unavailable(response.message)),
        ResponseStatus::InternalError => Err(Error::Backend(response.message)),
        ResponseStatus::PolicyViolation { kind } => Err(Error::Policy {
            kind,
            message: response.message,
        }),
        _ => Err(Error::Backend(response.message)),
    }
}

fn decode_payload<T: serde::de::DeserializeOwned>(body: &str, operation: &str) -> Result<T, Error> {
    serde_json::from_str(body)
        .map_err(|error| Error::Protocol(format!("{operation} response: {error}")))
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
    use std::sync::Mutex;

    use pcloud_ipc::{
        ListFolderEntry, ReadRangePayload, RemoteCopyPayload, RemoteDownloadPayload,
        RemoteUploadPayload, StatPathPayload,
    };

    use super::*;

    fn ok<T: serde::Serialize>(payload: &T) -> Result<Response, String> {
        Ok(Response {
            status: ResponseStatus::Ok,
            message: serde_json::to_string(payload).map_err(|error| error.to_string())?,
        })
    }

    fn stat(path: &str, folder: bool) -> StatPathPayload {
        StatPathPayload {
            file_id: if folder { 1 } else { 42 },
            parent_folder_id: 1,
            name: if path == "/" {
                "/".to_owned()
            } else {
                path.rsplit('/').next().unwrap_or(path).to_owned()
            },
            size: if folder { 0 } else { 7 },
            hash: String::new(),
            modified: 5,
            created: 4,
            is_folder: folder,
            is_mine: true,
            is_shared: false,
            encrypted: false,
            permissions: Some(31),
            source: "api".to_owned(),
        }
    }

    #[test]
    fn stat_list_and_range_use_only_canonical_requests() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let client = Client::with_sender(move |_path, request| {
            captured.lock().unwrap().push(format!("{request:?}"));
            match request {
                Request::StatPath { path } => ok(&stat(path, path == "/")),
                Request::ListFolderByPath { .. } => ok(&vec![ListFolderEntry {
                    file_id: 42,
                    name: "a.txt".to_owned(),
                    size: 7,
                    hash: String::new(),
                    modified: 5,
                    created: 4,
                    is_folder: false,
                    is_mine: true,
                    is_shared: false,
                    encrypted: false,
                    permissions: Some(1),
                }]),
                Request::ReadFileRange { .. } => ok(&ReadRangePayload {
                    data_b64: B64.encode(b"payload"),
                    bytes_returned: 7,
                    total_size: 7,
                    eof: true,
                }),
                _ => Err("unexpected request".to_owned()),
            }
        });

        let remote = client.remote();
        assert_eq!(remote.stat("/a.txt").unwrap().size, Some(7));
        assert_eq!(remote.list("/").unwrap().entries[0].name, "a.txt");
        assert_eq!(remote.read_range("/a.txt", 0, 7).unwrap().data, b"payload");
        assert_eq!(requests.lock().unwrap().len(), 4);
    }

    #[test]
    fn mutation_and_transfer_payloads_are_sdk_owned() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let client = Client::with_sender(move |_path, request| {
            captured.lock().unwrap().push(request.clone());
            match request {
                Request::StatPath { path } => ok(&stat(path, true)),
                Request::UploadFileByPath { .. } => ok(&RemoteUploadPayload {
                    upload_id: 7,
                    file_id: Some(42),
                    bytes: 9,
                    sha1_hex: "abc".to_owned(),
                    resumed_from: 3,
                }),
                Request::DownloadFileByPath { local_path, .. } => ok(&RemoteDownloadPayload {
                    path: local_path.clone(),
                    bytes: 9,
                    sha256_hex: "def".to_owned(),
                    resumed_from: 3,
                }),
                Request::CopyPath { .. } => ok(&RemoteCopyPayload {
                    files: 1,
                    folders: 2,
                    bytes: 9,
                }),
                Request::RenamePath { .. }
                | Request::DeletePath { .. }
                | Request::CreateFolderByPath { .. }
                | Request::ShareFolder { .. } => Ok(Response {
                    status: ResponseStatus::Ok,
                    message: "ok".to_owned(),
                }),
                _ => Err("unexpected request".to_owned()),
            }
        });

        let remote = client.remote();
        assert_eq!(remote.upload(Path::new("local"), "/a").unwrap().bytes, 9);
        assert_eq!(
            remote
                .download("/a", Path::new("dest"), false)
                .unwrap()
                .bytes,
            9
        );
        assert_eq!(remote.copy("/a", "/b").unwrap().folders, 2);
        remote.move_path("/a", "/b").unwrap();
        remote.delete("/b", false).unwrap();
        remote.mkdir("/folder").unwrap();
        remote
            .share_folder(
                "/folder",
                &ShareOptions::new("person@example.test").permissions(SharePermissions::READ_WRITE),
            )
            .unwrap();

        let requests = requests.lock().unwrap();
        let share = requests
            .iter()
            .find(|request| matches!(request, Request::ShareFolder { .. }))
            .expect("share request");
        let Request::ShareFolder {
            permissions_bits, ..
        } = share
        else {
            unreachable!()
        };
        assert_eq!(*permissions_bits, 15);
    }

    #[test]
    fn errors_and_paths_do_not_leak_ipc_types() {
        let client = Client::with_sender(|_, _| {
            Ok(Response {
                status: ResponseStatus::Unauthorized,
                message: "login required".to_owned(),
            })
        });
        assert!(matches!(
            client.remote().stat("/"),
            Err(Error::Unauthorized(_))
        ));
        assert!(matches!(
            client.remote().stat("relative"),
            Err(Error::InvalidRequest(_))
        ));

        let transport = Client::with_sender(|_, _| Err("endpoint gone".to_owned()));
        assert!(matches!(
            transport.remote().stat("/"),
            Err(Error::Transport(_))
        ));
    }
}

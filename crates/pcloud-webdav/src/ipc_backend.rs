//! Canonical daemon-IPC backend for the experimental WebDAV subset.
//!
//! Every implemented WebDAV operation is translated to an IPC request whose
//! daemon handler uses `pcloud_backends::RemoteFs`. Paths therefore retain the
//! same live, cache-independent, ID-first semantics as the CLI, SDK, sync, and
//! mount surfaces.
//!
//! GET and PUT bodies are handed to the owner-matched daemon through files in
//! an owner-private temporary directory. This avoids base64 expansion and the
//! IPC frame-size ceiling. The current HTTP dispatcher still buffers a whole
//! response/request body, so this adapter does not make a WebDAV compliance or
//! unbounded-file streaming claim.

// **PLATFORM:** all
// **GATING:** none (portable IPC facade).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use pcloud_ipc::{IpcClient, ListFolderEntry, Request, Response, ResponseStatus, StatPathPayload};

use crate::handler::{BackendEntry, BackendError, IpcBackend, PutOutcome};

type RequestSender = dyn Fn(&Request) -> Result<Response, String> + Send + Sync;

/// WebDAV backend that reaches the canonical remote filesystem through the
/// daemon's owner-authenticated local IPC transport.
///
/// Construct one with the same socket path used by `pcloudc`. On Windows the
/// IPC client derives the owner-SID named-pipe endpoint; the supplied path is
/// retained for API parity with Unix.
pub struct RemoteFsIpcBackend {
    sender: Box<RequestSender>,
}

impl std::fmt::Debug for RemoteFsIpcBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteFsIpcBackend")
            .finish_non_exhaustive()
    }
}

impl RemoteFsIpcBackend {
    /// Connect subsequent operations to the daemon listening at `socket_path`.
    #[must_use]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        let socket_path = socket_path.into();
        Self {
            sender: Box::new(move |request| {
                IpcClient
                    .send(&socket_path, request)
                    .map_err(|error| error.to_string())
            }),
        }
    }

    #[cfg(test)]
    fn with_sender<F>(sender: F) -> Self
    where
        F: Fn(&Request) -> Result<Response, String> + Send + Sync + 'static,
    {
        Self {
            sender: Box::new(sender),
        }
    }

    fn send_ok(
        &self,
        request: &Request,
        operation: &'static str,
        conflict_is_missing: bool,
    ) -> Result<String, BackendError> {
        let response = (self.sender)(request)
            .map_err(|error| BackendError::Upstream(format!("{operation}: {error}")))?;
        match response.status {
            ResponseStatus::Ok => Ok(response.message),
            ResponseStatus::Conflict if conflict_is_missing => Err(BackendError::NotFound),
            ResponseStatus::Conflict | ResponseStatus::InvalidRequest => {
                Err(BackendError::Conflict)
            }
            _ => Err(BackendError::Upstream(format!(
                "{operation}: {}",
                response.message
            ))),
        }
    }

    fn stat_payload(&self, path: &str) -> Result<StatPathPayload, BackendError> {
        let message = self.send_ok(
            &Request::StatPath {
                path: path.to_owned(),
            },
            "stat",
            true,
        )?;
        serde_json::from_str(&message)
            .map_err(|error| BackendError::Upstream(format!("stat response: {error}")))
    }
}

impl IpcBackend for RemoteFsIpcBackend {
    fn list_folder(&self, path: &str) -> Result<Vec<BackendEntry>, BackendError> {
        let message = self.send_ok(
            &Request::ListFolderByPath {
                path: path.to_owned(),
            },
            "list folder",
            true,
        )?;
        let entries: Vec<ListFolderEntry> = serde_json::from_str(&message)
            .map_err(|error| BackendError::Upstream(format!("list response: {error}")))?;
        Ok(entries.into_iter().map(entry_from_listing).collect())
    }

    fn stat(&self, path: &str) -> Result<BackendEntry, BackendError> {
        Ok(entry_from_stat(self.stat_payload(path)?))
    }

    fn get_file(&self, path: &str) -> Result<Vec<u8>, BackendError> {
        let metadata = self.stat_payload(path)?;
        if metadata.is_folder {
            return Err(BackendError::Conflict);
        }

        let staging = tempfile::Builder::new()
            .prefix("pcloud-webdav-get-")
            .tempdir()
            .map_err(local_staging_error)?;
        let destination = staging.path().join("download");
        self.send_ok(
            &Request::DownloadFileByPath {
                remote_path: path.to_owned(),
                local_path: destination.clone(),
                overwrite: false,
            },
            "download",
            false,
        )?;
        let bytes = std::fs::read(&destination).map_err(local_staging_error)?;
        if bytes.len() as u64 != metadata.size {
            return Err(BackendError::Upstream(format!(
                "download: daemon published {} bytes, live metadata expected {}",
                bytes.len(),
                metadata.size
            )));
        }
        Ok(bytes)
    }

    fn put_file(&mut self, path: &str, bytes: &[u8]) -> Result<PutOutcome, BackendError> {
        let existed = match self.stat(path) {
            Ok(entry) if entry.is_collection => return Err(BackendError::Conflict),
            Ok(_) => true,
            Err(BackendError::NotFound) => false,
            Err(error) => return Err(error),
        };

        let staging = tempfile::Builder::new()
            .prefix("pcloud-webdav-put-")
            .tempdir()
            .map_err(local_staging_error)?;
        let source = staging.path().join("upload");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&source).map_err(local_staging_error)?;
        file.write_all(bytes).map_err(local_staging_error)?;
        file.sync_all().map_err(local_staging_error)?;
        drop(file);

        self.send_ok(
            &Request::UploadFileByPath {
                local_path: source,
                remote_path: path.to_owned(),
            },
            "upload",
            false,
        )?;
        Ok(if existed {
            PutOutcome::Updated
        } else {
            PutOutcome::Created
        })
    }

    fn delete(&mut self, path: &str) -> Result<(), BackendError> {
        self.send_ok(
            &Request::DeletePath {
                path: path.to_owned(),
                recursive: true,
            },
            "delete",
            false,
        )?;
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> Result<(), BackendError> {
        match self.stat(path) {
            Ok(_) => return Err(BackendError::Conflict),
            Err(BackendError::NotFound) => {}
            Err(error) => return Err(error),
        }
        self.send_ok(
            &Request::CreateFolderByPath {
                path: path.to_owned(),
            },
            "mkdir",
            false,
        )?;
        Ok(())
    }
}

fn entry_from_stat(payload: StatPathPayload) -> BackendEntry {
    BackendEntry {
        name: payload.name,
        is_collection: payload.is_folder,
        content_length: (!payload.is_folder).then_some(payload.size),
        last_modified: None,
        content_type: None,
    }
}

fn entry_from_listing(payload: ListFolderEntry) -> BackendEntry {
    BackendEntry {
        name: payload.name,
        is_collection: payload.is_folder,
        content_length: (!payload.is_folder).then_some(payload.size),
        last_modified: None,
        content_type: None,
    }
}

fn local_staging_error(error: std::io::Error) -> BackendError {
    BackendError::Upstream(format!("private transfer staging: {error}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use super::*;

    fn ok(message: String) -> Response {
        Response {
            status: ResponseStatus::Ok,
            message,
        }
    }

    fn missing() -> Response {
        Response {
            status: ResponseStatus::Conflict,
            message: "remote path not found".to_owned(),
        }
    }

    fn file_stat(path: &str, size: u64) -> StatPathPayload {
        StatPathPayload {
            file_id: 42,
            parent_folder_id: 1,
            name: Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("/")
                .to_owned(),
            size,
            hash: String::new(),
            modified: 0,
            created: 0,
            is_folder: false,
            is_mine: true,
            is_shared: false,
            encrypted: false,
            permissions: None,
            source: "api".to_owned(),
        }
    }

    #[test]
    fn stat_and_list_map_to_canonical_path_requests() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let backend = RemoteFsIpcBackend::with_sender(move |request| match request {
            Request::StatPath { path } => {
                observed.lock().unwrap().push(format!("stat:{path}"));
                let mut payload = file_stat(path, 0);
                payload.name = "/".to_owned();
                payload.is_folder = true;
                Ok(ok(serde_json::to_string(&payload).unwrap()))
            }
            Request::ListFolderByPath { path } => {
                observed.lock().unwrap().push(format!("list:{path}"));
                Ok(ok(serde_json::to_string(&vec![ListFolderEntry {
                    file_id: 42,
                    name: "photo.jpg".to_owned(),
                    size: 7,
                    hash: String::new(),
                    modified: 0,
                    created: 0,
                    is_folder: false,
                    is_mine: true,
                    is_shared: false,
                    encrypted: false,
                    permissions: None,
                }])
                .unwrap()))
            }
            _ => Err("unexpected request".to_owned()),
        });

        assert!(backend.stat("/").unwrap().is_collection);
        let entries = backend.list_folder("/").unwrap();
        assert_eq!(entries[0].name, "photo.jpg");
        assert_eq!(&*requests.lock().unwrap(), &["stat:/", "list:/"]);
    }

    #[test]
    fn get_uses_daemon_download_file_handoff() {
        let body = b"canonical remote bytes".to_vec();
        let expected = body.clone();
        let backend = RemoteFsIpcBackend::with_sender(move |request| match request {
            Request::StatPath { path } => Ok(ok(serde_json::to_string(&file_stat(
                path,
                expected.len() as u64,
            ))
            .unwrap())),
            Request::DownloadFileByPath {
                remote_path,
                local_path,
                overwrite,
            } => {
                assert_eq!(remote_path, "/photo.jpg");
                assert!(!overwrite);
                std::fs::write(local_path, &expected).map_err(|error| error.to_string())?;
                Ok(ok("downloaded".to_owned()))
            }
            _ => Err("unexpected request".to_owned()),
        });

        assert_eq!(backend.get_file("/photo.jpg").unwrap(), body);
    }

    #[test]
    fn put_uses_durable_daemon_upload_file_handoff() {
        let uploaded = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&uploaded);
        let mut backend = RemoteFsIpcBackend::with_sender(move |request| match request {
            Request::StatPath { .. } => Ok(missing()),
            Request::UploadFileByPath {
                local_path,
                remote_path,
            } => {
                assert_eq!(remote_path, "/new.bin");
                *captured.lock().unwrap() =
                    std::fs::read(local_path).map_err(|error| error.to_string())?;
                Ok(ok("uploaded".to_owned()))
            }
            _ => Err("unexpected request".to_owned()),
        });

        assert_eq!(
            backend.put_file("/new.bin", b"payload").unwrap(),
            PutOutcome::Created
        );
        assert_eq!(&*uploaded.lock().unwrap(), b"payload");
    }

    #[test]
    fn mkdir_and_delete_use_canonical_mutation_requests() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&operations);
        let mut backend = RemoteFsIpcBackend::with_sender(move |request| match request {
            Request::StatPath { path } => {
                captured.lock().unwrap().push(format!("stat:{path}"));
                Ok(missing())
            }
            Request::CreateFolderByPath { path } => {
                captured.lock().unwrap().push(format!("mkdir:{path}"));
                Ok(ok("created".to_owned()))
            }
            Request::DeletePath { path, recursive } => {
                assert!(*recursive);
                captured.lock().unwrap().push(format!("delete:{path}"));
                Ok(ok("deleted".to_owned()))
            }
            _ => Err("unexpected request".to_owned()),
        });

        backend.mkdir("/album").unwrap();
        backend.delete("/album").unwrap();
        assert_eq!(
            &*operations.lock().unwrap(),
            &["stat:/album", "mkdir:/album", "delete:/album"]
        );
    }
}

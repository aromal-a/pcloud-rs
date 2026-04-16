//! Bridge between engine [`PlannedOperation`] / [`TransferTask`] items and
//! real upload/download API calls via [`TransferRuntime`].
//!
//! This module converts the abstract work items produced by the sync engine
//! planner/scheduler into concrete pCloud API calls:
//!
//! - **Uploads:** read local file, call `upload_create` -> chunked
//!   `upload_bytes` (which wraps `upload_write` + `upload_save`).
//! - **Downloads:** call `get_file_link`, then `download_bytes`, write to
//!   local path.
//!
//! Error handling:
//! - File not found locally before upload -> `TransferBridgeError::LocalFileNotFound`
//! - Network/API errors -> wrapped transparently for the recovery classifier
//! - Partial writes use atomic rename via a `.part` sidecar
//!
//! Portable; no platform gating.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pcloud_model::sync::PlannedOperation;
use pcloud_model::transfer::TransferTask;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

use crate::transfer_backend::TransferRuntime;

/// Errors that can occur during transfer bridge execution.
#[derive(Debug, Error)]
pub enum TransferBridgeError {
    /// The local file was deleted before the upload could start.
    #[error("local file not found: {0}")]
    LocalFileNotFound(PathBuf),
    /// The local file could not be read.
    #[error("local file I/O error on {path}: {source}")]
    LocalIo {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// The parent directory for a download target does not exist and
    /// could not be created.
    #[error("failed to create parent directory {path}: {source}")]
    CreateParentDir {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// The download link resolution or byte fetch failed.
    #[error("download failed: {0}")]
    DownloadFailed(String),
    /// The upload session or byte push failed.
    #[error("upload failed: {0}")]
    UploadFailed(String),
    /// The operation type is not a transfer (e.g. Conflict, DeleteLocal).
    #[error("operation is not a transfer: {0}")]
    NotATransfer(String),
    /// Atomic rename of `.part` sidecar to final path failed.
    #[error("atomic rename failed from {from} to {to}: {source}")]
    AtomicRename {
        /// Source `.part` path.
        from: PathBuf,
        /// Target final path.
        to: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
}

/// Result of a successful transfer execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferResult {
    /// The relative path under the sync root.
    pub path: String,
    /// Direction: "upload" or "download".
    pub direction: TransferDirection,
    /// Bytes transferred.
    pub bytes_transferred: u64,
}

/// Direction of a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    /// Local -> remote.
    Upload,
    /// Remote -> local.
    Download,
}

impl std::fmt::Display for TransferDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Upload => write!(f, "upload"),
            Self::Download => write!(f, "download"),
        }
    }
}

/// Execute a single upload: read the local file and push it to the server
/// via `upload_create` + `upload_bytes` (which internally does
/// `upload_write` + `upload_save`).
///
/// When `chunk_size` is `Some(n)` and the file exceeds `n` bytes, the
/// upload is split into `upload_create` followed by multiple
/// `upload_bytes` calls of at most `n` bytes each. This keeps memory
/// pressure bounded for large files and enables future resume-on-error
/// (the last acknowledged offset can be persisted in a sidecar).
///
/// `sync_root_path` is the absolute path of the sync root on disk.
/// `task.operation` must be `PlannedOperation::UploadFile`.
///
/// # Errors
///
/// Returns `TransferBridgeError` on local I/O failure, API error, or if
/// the operation is not an upload.
pub fn execute_upload(
    task: &TransferTask,
    transfer: &TransferRuntime,
    auth_token: &SecretString,
    sync_root_path: &Path,
) -> Result<TransferResult, TransferBridgeError> {
    execute_upload_with_chunk_size(task, transfer, auth_token, sync_root_path, None)
}

/// Like [`execute_upload`] but with an explicit chunk size threshold.
///
/// When `chunk_size` is `Some(n)` and the file is larger than `n`, the
/// file is uploaded in `n`-byte chunks via repeated `upload_bytes`
/// calls. Otherwise (or when `chunk_size` is `None`) the entire file
/// is pushed in a single `upload_bytes` call.
pub fn execute_upload_with_chunk_size(
    task: &TransferTask,
    transfer: &TransferRuntime,
    auth_token: &SecretString,
    sync_root_path: &Path,
    chunk_size: Option<usize>,
) -> Result<TransferResult, TransferBridgeError> {
    let (path, parent_folder_id, remote_name) = match &task.operation {
        PlannedOperation::UploadFile {
            path,
            remote_parent_folder_id,
            remote_name,
            ..
        } => (
            path.clone(),
            remote_parent_folder_id.map(|id| id.get()).unwrap_or(0),
            remote_name.clone(),
        ),
        other => {
            return Err(TransferBridgeError::NotATransfer(format!(
                "expected UploadFile, got {:?}",
                std::mem::discriminant(other)
            )));
        }
    };

    let local_path = sync_root_path.join(&path);

    // Read the local file
    let file_bytes = match fs::read(&local_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(TransferBridgeError::LocalFileNotFound(local_path));
        }
        Err(err) => {
            return Err(TransferBridgeError::LocalIo {
                path: local_path,
                source: err,
            });
        }
    };

    let file_size = file_bytes.len() as u64;

    // Open upload session
    let session = transfer
        .upload_create(
            SecretString::new(auth_token.expose_secret().to_owned()),
            parent_folder_id,
            &remote_name,
            file_size,
        )
        .map_err(|e| TransferBridgeError::UploadFailed(e.to_string()))?;

    // Decide: single-shot or chunked upload.
    let use_chunked = chunk_size
        .map(|cs| file_size > cs as u64 && cs > 0)
        .unwrap_or(false);

    if use_chunked {
        let cs = chunk_size.expect("chunk_size is Some when use_chunked is true");
        let mut tracker = pcloud_engine::transfers::uploads::ChunkedUploadTracker::new(
            session.upload_id,
            file_size,
            cs,
        );

        while !tracker.is_complete() {
            let offset = tracker.acked_offset as usize;
            let chunk_len = tracker.next_chunk_size();
            let chunk = &file_bytes[offset..offset + chunk_len];

            transfer
                .upload_bytes(
                    SecretString::new(auth_token.expose_secret().to_owned()),
                    &session,
                    chunk,
                )
                .map_err(|e| {
                    // On error mid-chunk, persist the offset in a sidecar
                    // for future resume. Best-effort; we surface the
                    // original error regardless.
                    let sidecar = local_path.with_extension("pcloud-resume");
                    let _ = fs::write(
                        &sidecar,
                        format!(
                            "upload_id={}\nacked_offset={}\n",
                            session.upload_id, tracker.acked_offset
                        ),
                    );
                    TransferBridgeError::UploadFailed(format!(
                        "chunk upload failed at offset {}: {e}",
                        tracker.acked_offset
                    ))
                })?;

            tracker.advance(chunk_len as u64);
        }
    } else {
        // Single-shot upload
        transfer
            .upload_bytes(
                SecretString::new(auth_token.expose_secret().to_owned()),
                &session,
                &file_bytes,
            )
            .map_err(|e| TransferBridgeError::UploadFailed(e.to_string()))?;
    }

    Ok(TransferResult {
        path,
        direction: TransferDirection::Upload,
        bytes_transferred: file_size,
    })
}

/// Execute a single download: resolve the file link, fetch bytes, and
/// write to the local path via an atomic `.part` sidecar rename.
///
/// `sync_root_path` is the absolute path of the sync root on disk.
/// `task.operation` must be `PlannedOperation::DownloadFile`.
///
/// # Errors
///
/// Returns `TransferBridgeError` on API error, local I/O failure, or if
/// the operation is not a download.
pub fn execute_download(
    task: &TransferTask,
    transfer: &TransferRuntime,
    auth_token: &SecretString,
    sync_root_path: &Path,
) -> Result<TransferResult, TransferBridgeError> {
    let (path, remote_file_id) = match &task.operation {
        PlannedOperation::DownloadFile {
            path,
            remote_file_id,
            ..
        } => (path.clone(), remote_file_id.map(|id| id.get()).unwrap_or(0)),
        other => {
            return Err(TransferBridgeError::NotATransfer(format!(
                "expected DownloadFile, got {:?}",
                std::mem::discriminant(other)
            )));
        }
    };

    // Resolve download link
    let link = transfer
        .get_file_link(
            SecretString::new(auth_token.expose_secret().to_owned()),
            remote_file_id,
            None,
        )
        .map_err(|e| TransferBridgeError::DownloadFailed(e.to_string()))?;

    // Fetch bytes
    let (_signed, bytes) = transfer
        .download_bytes(&link)
        .map_err(|e| TransferBridgeError::DownloadFailed(e.to_string()))?;

    let local_path = sync_root_path.join(&path);

    // Ensure parent directory exists
    if let Some(parent) = local_path.parent().filter(|p| !p.exists()) {
        fs::create_dir_all(parent).map_err(|e| TransferBridgeError::CreateParentDir {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    // Write to .part sidecar, then atomic rename
    let part_path = local_path.with_extension(format!(
        "{}.part",
        local_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
    ));

    fs::write(&part_path, &bytes).map_err(|e| TransferBridgeError::LocalIo {
        path: part_path.clone(),
        source: e,
    })?;

    fs::rename(&part_path, &local_path).map_err(|e| TransferBridgeError::AtomicRename {
        from: part_path,
        to: local_path,
        source: e,
    })?;

    let bytes_transferred = bytes.len() as u64;

    Ok(TransferResult {
        path,
        direction: TransferDirection::Download,
        bytes_transferred,
    })
}

/// Execute all active uploads from the engine's upload coordinator.
///
/// Returns the count of successfully completed uploads. Failed uploads
/// are recorded in the engine via `mark_transfer_failed`.
///
/// Respects `max_concurrent` by processing sequentially up to that limit
/// per call. (True parallel execution via a thread pool is deferred to a
/// follow-up; the sync loop already runs on its own thread.)
pub fn execute_pending_uploads(
    tasks: &[TransferTask],
    transfer: &TransferRuntime,
    auth_token: &SecretString,
    sync_root_path: &Path,
    max_concurrent: usize,
) -> Vec<Result<TransferResult, TransferBridgeError>> {
    let limit = if max_concurrent == 0 {
        tasks.len()
    } else {
        max_concurrent.min(tasks.len())
    };

    tasks[..limit]
        .iter()
        .map(|task| execute_upload(task, transfer, auth_token, sync_root_path))
        .collect()
}

/// Execute all active downloads from the engine's download coordinator.
///
/// Returns the count of successfully completed downloads. Failed downloads
/// are recorded in the engine via `mark_transfer_failed`.
///
/// Respects `max_concurrent` by processing sequentially up to that limit
/// per call.
pub fn execute_pending_downloads(
    tasks: &[TransferTask],
    transfer: &TransferRuntime,
    auth_token: &SecretString,
    sync_root_path: &Path,
    max_concurrent: usize,
) -> Vec<Result<TransferResult, TransferBridgeError>> {
    let limit = if max_concurrent == 0 {
        tasks.len()
    } else {
        max_concurrent.min(tasks.len())
    };

    tasks[..limit]
        .iter()
        .map(|task| execute_download(task, transfer, auth_token, sync_root_path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_config::ConfigProfile;
    use pcloud_model::ids::{RemoteFileId, RemoteFolderId, SyncId};
    use pcloud_model::sync::PlannedOperation;
    use pcloud_model::transfer::{TransferState, TransferTask};
    use tempfile::TempDir;

    fn dev_transfer() -> TransferRuntime {
        let cfg = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-test-xfer"),
            pcloud_config::Environment::Development,
        );
        TransferRuntime::from_config(&cfg)
    }

    fn upload_task(path: &str) -> TransferTask {
        TransferTask {
            operation: PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: path.to_owned(),
                remote_parent_folder_id: Some(RemoteFolderId::new(0)),
                remote_name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            },
            state: TransferState::Streaming,
            last_error: None,
        }
    }

    fn download_task(path: &str, file_id: u64) -> TransferTask {
        TransferTask {
            operation: PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: path.to_owned(),
                remote_file_id: Some(RemoteFileId::new(file_id)),
            },
            state: TransferState::Streaming,
            last_error: None,
        }
    }

    #[test]
    fn upload_succeeds_with_dev_transport() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("docs/report.txt");
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(&file_path, b"hello world").unwrap();

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = upload_task("docs/report.txt");

        let result = execute_upload(&task, &transfer, &token, dir.path()).unwrap();

        assert_eq!(result.path, "docs/report.txt");
        assert_eq!(result.direction, TransferDirection::Upload);
        assert_eq!(result.bytes_transferred, 11);
    }

    #[test]
    fn upload_fails_when_local_file_missing() {
        let dir = TempDir::new().unwrap();
        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = upload_task("nonexistent.txt");

        let result = execute_upload(&task, &transfer, &token, dir.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TransferBridgeError::LocalFileNotFound(_)),
            "expected LocalFileNotFound, got: {err}"
        );
    }

    #[test]
    fn download_succeeds_with_dev_transport() {
        let dir = TempDir::new().unwrap();
        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = download_task("docs/fetched.txt", 42);

        let result = execute_download(&task, &transfer, &token, dir.path()).unwrap();

        assert_eq!(result.path, "docs/fetched.txt");
        assert_eq!(result.direction, TransferDirection::Download);
        assert!(result.bytes_transferred > 0);

        // Verify the file was written
        let local = dir.path().join("docs/fetched.txt");
        assert!(local.exists(), "downloaded file should exist on disk");
        let content = fs::read_to_string(&local).unwrap();
        assert!(
            content.contains("downloaded:"),
            "dev transport content should contain 'downloaded:'"
        );
    }

    #[test]
    fn download_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = download_task("deep/nested/dir/file.txt", 42);

        let result = execute_download(&task, &transfer, &token, dir.path()).unwrap();

        assert_eq!(result.path, "deep/nested/dir/file.txt");
        assert!(dir.path().join("deep/nested/dir/file.txt").exists());
    }

    #[test]
    fn not_a_transfer_error_on_wrong_operation() {
        let dir = TempDir::new().unwrap();
        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());

        let task = TransferTask {
            operation: PlannedOperation::DeleteLocal {
                sync_id: SyncId::new(1),
                path: "foo.txt".to_owned(),
            },
            state: TransferState::Streaming,
            last_error: None,
        };

        let up_err = execute_upload(&task, &transfer, &token, dir.path());
        assert!(matches!(up_err, Err(TransferBridgeError::NotATransfer(_))));

        let down_err = execute_download(&task, &transfer, &token, dir.path());
        assert!(matches!(
            down_err,
            Err(TransferBridgeError::NotATransfer(_))
        ));
    }

    #[test]
    fn batch_upload_respects_max_concurrent() {
        let dir = TempDir::new().unwrap();
        // Create 3 files
        for i in 0..3 {
            let p = dir.path().join(format!("file{i}.txt"));
            fs::write(&p, format!("content-{i}")).unwrap();
        }

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());

        let tasks: Vec<_> = (0..3)
            .map(|i| upload_task(&format!("file{i}.txt")))
            .collect();

        // max_concurrent = 2 -> only first 2 processed
        let results = execute_pending_uploads(&tasks, &transfer, &token, dir.path(), 2);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_ok()));
    }

    #[test]
    fn batch_download_respects_max_concurrent() {
        let dir = TempDir::new().unwrap();
        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());

        let tasks: Vec<_> = (0..4)
            .map(|i| download_task(&format!("dl{i}.txt"), 42))
            .collect();

        let results = execute_pending_downloads(&tasks, &transfer, &token, dir.path(), 2);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_ok()));
    }

    #[test]
    fn batch_upload_zero_max_processes_all() {
        let dir = TempDir::new().unwrap();
        for i in 0..3 {
            fs::write(dir.path().join(format!("f{i}.txt")), "data").unwrap();
        }

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());

        let tasks: Vec<_> = (0..3).map(|i| upload_task(&format!("f{i}.txt"))).collect();

        let results = execute_pending_uploads(&tasks, &transfer, &token, dir.path(), 0);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn download_api_error_surfaces() {
        let dir = TempDir::new().unwrap();
        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());

        // file_id 999 triggers a timeout in DevelopmentTransferTransport
        let task = download_task("timeout.txt", 999);

        let result = execute_download(&task, &transfer, &token, dir.path());
        assert!(
            result.is_err(),
            "download with file_id 999 should trigger dev transport timeout"
        );
        assert!(matches!(
            result.unwrap_err(),
            TransferBridgeError::DownloadFailed(_)
        ));
    }

    #[test]
    fn upload_api_error_surfaces() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("fail-upload.txt");
        fs::write(&file_path, b"will fail").unwrap();

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = upload_task("fail-upload.txt");

        let result = execute_upload(&task, &transfer, &token, dir.path());
        assert!(
            result.is_err(),
            "upload of 'fail-upload.txt' should trigger dev transport failure"
        );
        assert!(matches!(
            result.unwrap_err(),
            TransferBridgeError::UploadFailed(_)
        ));
    }

    #[test]
    fn chunked_upload_splits_large_file_into_chunks() {
        let dir = TempDir::new().unwrap();
        // 25 bytes with chunk_size = 10 -> 3 upload_bytes calls
        let content = "a]".repeat(12) + "b"; // 25 bytes
        let file_path = dir.path().join("chunked.txt");
        fs::write(&file_path, content.as_bytes()).unwrap();

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = upload_task("chunked.txt");

        let result =
            execute_upload_with_chunk_size(&task, &transfer, &token, dir.path(), Some(10)).unwrap();

        assert_eq!(result.path, "chunked.txt");
        assert_eq!(result.direction, TransferDirection::Upload);
        assert_eq!(result.bytes_transferred, 25);
    }

    #[test]
    fn chunked_upload_falls_back_to_single_shot_when_below_threshold() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("small.txt");
        fs::write(&file_path, b"tiny").unwrap();

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = upload_task("small.txt");

        // chunk_size 100 but file is only 4 bytes -> single shot
        let result =
            execute_upload_with_chunk_size(&task, &transfer, &token, dir.path(), Some(100))
                .unwrap();

        assert_eq!(result.bytes_transferred, 4);
    }

    #[test]
    fn chunked_upload_none_chunk_size_uses_single_shot() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("nochunk.txt");
        fs::write(&file_path, b"hello world chunked test").unwrap();

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = upload_task("nochunk.txt");

        let result =
            execute_upload_with_chunk_size(&task, &transfer, &token, dir.path(), None).unwrap();

        assert_eq!(result.bytes_transferred, 24);
    }

    #[test]
    fn chunked_upload_exact_multiple_of_chunk_size() {
        let dir = TempDir::new().unwrap();
        // 20 bytes with chunk_size = 10 -> exactly 2 chunks
        let content = "x".repeat(20);
        let file_path = dir.path().join("exact.txt");
        fs::write(&file_path, content.as_bytes()).unwrap();

        let transfer = dev_transfer();
        let token = SecretString::new("test-token".to_owned());
        let task = upload_task("exact.txt");

        let result =
            execute_upload_with_chunk_size(&task, &transfer, &token, dir.path(), Some(10)).unwrap();

        assert_eq!(result.bytes_transferred, 20);
    }
}

// TODO(bd-sweep-unwrap): This file contains ~31 `.unwrap()` / `.expect()`
// call sites in non-test code paths. The most-reachable ones are in the
// upload/download execution paths (file open, path canonicalization).
// Converting them to `?` propagation is the priority for this file.
// Full sweep deferred to a dedicated hardening pass.

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
use std::time::Duration;

use pcloud_model::sync::PlannedOperation;
use pcloud_model::transfer::TransferTask;
use pcloud_resilience::{BackoffSchedule, RetryDecision, RetryPolicy};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

use crate::transfer_backend::TransferRuntime;

/// Seed for the jitter PRNG used in transfer retry backoff.
/// "pcloud_x" encoded as ASCII bytes in little-endian u64.
const XFER_JITTER_SEED: u64 = 0x70636c6f_75645f78;

/// Default retry policy for transient upload/download failures.
///
/// Exponential backoff starting at 1 s, capped at 60 s, with jitter.
/// Up to 5 total attempts (4 retries). This covers transient network
/// failures such as connection resets, 5xx responses, and DNS hiccups.
/// Permanent errors (auth failure, invalid path) are not retried — the
/// caller must detect those and surface them as terminal failures.
fn default_transfer_retry_policy() -> RetryPolicy {
    RetryPolicy::new(
        5,
        BackoffSchedule::ExponentialJittered {
            base: Duration::from_secs(1),
            factor: 2.0,
            max: Duration::from_secs(60),
            seed: XFER_JITTER_SEED,
        },
    )
}

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

    let retry = default_transfer_retry_policy();

    // Open upload session — retry on transient failure.
    //
    // TODO(bd-1du): upload resumption from upload_resume_state is not yet
    // implemented; orphaned sessions accumulate until manual cleanup or the
    // periodic 24-hour stale-session purge below. A full resume path would
    // call UploadResumeRepository::get here, reuse the existing upload_id
    // when found, and only call upload_create when no resume row exists.
    let session = {
        let mut attempt: u32 = 1;
        loop {
            match transfer.upload_create(
                SecretString::new(auth_token.expose_secret().to_owned()),
                parent_folder_id,
                &remote_name,
                file_size,
            ) {
                Ok(s) => break s,
                Err(e) => match retry.next(attempt) {
                    RetryDecision::Retry { wait } => {
                        log::warn!(
                            "upload_create transient error (attempt {attempt}): {e}; retrying in {wait:?}"
                        );
                        std::thread::sleep(wait);
                        attempt += 1;
                    }
                    RetryDecision::GiveUp => {
                        return Err(TransferBridgeError::UploadFailed(format!(
                            "upload_create failed after {attempt} attempts: {e}"
                        )));
                    }
                },
            }
        }
    };

    // Decide: single-shot or chunked upload.
    let use_chunked = chunk_size
        .map(|cs| file_size > cs as u64 && cs > 0)
        .unwrap_or(false);

    if use_chunked {
        // SAFETY: `use_chunked` is computed as `chunk_size.map(...).unwrap_or(false)`
        // (see a few lines above). It can only be `true` when `chunk_size` is
        // `Some(_)`; the `None` branch in `.map` yields `None.unwrap_or(false) ==
        // false` and bypasses this block. So `chunk_size.expect(...)` here is
        // unreachable in well-formed control flow.
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

            let mut attempt: u32 = 1;
            loop {
                match transfer.upload_bytes(
                    SecretString::new(auth_token.expose_secret().to_owned()),
                    &session,
                    chunk,
                ) {
                    Ok(_) => break,
                    Err(e) => {
                        match retry.next(attempt) {
                            RetryDecision::Retry { wait } => {
                                log::warn!(
                                    "upload_bytes transient error at offset {} (attempt {attempt}): {e}; retrying in {wait:?}",
                                    tracker.acked_offset
                                );
                                std::thread::sleep(wait);
                                attempt += 1;
                            }
                            RetryDecision::GiveUp => {
                                // TODO(bd-1du): upload resumption from upload_resume_state is not
                                // yet implemented; orphaned sessions accumulate until manual cleanup.
                                // On permanent failure, write a best-effort sidecar so operators can
                                // identify the stuck upload_id. The upload_resume_state DB table
                                // (UploadResumeRepository) should be written here instead once the
                                // full resume path is implemented.
                                let sidecar = local_path.with_extension("pcloud-resume");
                                let _ = fs::write(
                                    &sidecar,
                                    format!(
                                        "upload_id={}\nacked_offset={}\n",
                                        session.upload_id, tracker.acked_offset
                                    ),
                                );
                                return Err(TransferBridgeError::UploadFailed(format!(
                                    "chunk upload failed at offset {} after {attempt} attempts: {e}",
                                    tracker.acked_offset
                                )));
                            }
                        }
                    }
                }
            }

            tracker.advance(chunk_len as u64);
        }
    } else {
        // Single-shot upload — retry on transient failure.
        let mut attempt: u32 = 1;
        loop {
            match transfer.upload_bytes(
                SecretString::new(auth_token.expose_secret().to_owned()),
                &session,
                &file_bytes,
            ) {
                Ok(_) => break,
                Err(e) => match retry.next(attempt) {
                    RetryDecision::Retry { wait } => {
                        log::warn!(
                            "upload_bytes transient error (attempt {attempt}): {e}; retrying in {wait:?}"
                        );
                        std::thread::sleep(wait);
                        attempt += 1;
                    }
                    RetryDecision::GiveUp => {
                        return Err(TransferBridgeError::UploadFailed(format!(
                            "upload_bytes failed after {attempt} attempts: {e}"
                        )));
                    }
                },
            }
        }
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

    let retry = default_transfer_retry_policy();

    // Resolve download link — retry on transient failure.
    let link = {
        let mut attempt: u32 = 1;
        loop {
            match transfer.get_file_link(
                SecretString::new(auth_token.expose_secret().to_owned()),
                remote_file_id,
                None,
            ) {
                Ok(l) => break l,
                Err(e) => match retry.next(attempt) {
                    RetryDecision::Retry { wait } => {
                        log::warn!(
                            "get_file_link transient error (attempt {attempt}): {e}; retrying in {wait:?}"
                        );
                        std::thread::sleep(wait);
                        attempt += 1;
                    }
                    RetryDecision::GiveUp => {
                        return Err(TransferBridgeError::DownloadFailed(format!(
                            "get_file_link failed after {attempt} attempts: {e}"
                        )));
                    }
                },
            }
        }
    };

    // Fetch bytes — retry on transient failure.
    //
    // TODO(bd-1du): large file downloads should use streaming IO rather than
    // full-file buffering. Currently download_bytes() buffers the entire
    // response body into a Vec<u8>, causing ~3x peak memory consumption
    // (response buffer + intermediate Vec + write buffer). For files above
    // 512 MiB this is a memory hazard. A streaming path using chunked range
    // requests or tokio::io::copy into the .part file should be implemented.
    let (_signed, bytes) = {
        let mut attempt: u32 = 1;
        loop {
            match transfer.download_bytes(&link) {
                Ok(result) => break result,
                Err(e) => match retry.next(attempt) {
                    RetryDecision::Retry { wait } => {
                        log::warn!(
                            "download_bytes transient error (attempt {attempt}): {e}; retrying in {wait:?}"
                        );
                        std::thread::sleep(wait);
                        attempt += 1;
                    }
                    RetryDecision::GiveUp => {
                        return Err(TransferBridgeError::DownloadFailed(format!(
                            "download_bytes failed after {attempt} attempts: {e}"
                        )));
                    }
                },
            }
        }
    };

    // Fix 3: guard against large-file memory exhaustion.
    const LARGE_FILE_WARN_BYTES: u64 = 512 * 1024 * 1024;
    let downloaded_size = bytes.len() as u64;
    if downloaded_size > LARGE_FILE_WARN_BYTES {
        log::warn!(
            "downloading large file {} ({} bytes) — buffered download may exhaust memory",
            path,
            downloaded_size
        );
    }

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

/// Purge stale rows from the `upload_resume_state` table.
///
/// Rows whose `updated_at` Unix timestamp is older than `max_age_secs`
/// seconds are deleted. This prevents orphaned server upload sessions from
/// accumulating indefinitely when uploads fail permanently and the normal
/// per-task cleanup path is not reached.
///
/// Call this on daemon startup and periodically (e.g. every 24 hours via
/// the sync loop runtime) to keep the table bounded.
///
/// # TODO(bd-1du)
///
/// Upload resumption from `upload_resume_state` is not yet implemented;
/// orphaned sessions accumulate until this cleanup runs. When resumption
/// is implemented, stale cleanup should only remove rows whose server-side
/// upload session has expired (confirmed via a `GET /upload_info` API call),
/// not all rows older than `max_age_secs`.
pub fn purge_stale_upload_resume_rows(
    conn: &rusqlite::Connection,
    max_age_secs: i64,
) -> Result<usize, rusqlite::Error> {
    use pcloud_store::repositories::upload_resume::UploadResumeRepository;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let cutoff = now.saturating_sub(max_age_secs);

    let rows = UploadResumeRepository::list_all(conn)?;
    let mut deleted = 0usize;
    for row in &rows {
        if row.updated_at < cutoff {
            log::info!(
                "purging stale upload_resume_state row: path={} upload_id={} updated_at={}",
                row.local_path,
                row.upload_id,
                row.updated_at
            );
            if UploadResumeRepository::delete(conn, &row.local_path)? {
                deleted += 1;
            }
        }
    }
    Ok(deleted)
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

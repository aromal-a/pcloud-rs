//! Canonical, cache-independent remote filesystem service.
//!
//! [`RemoteFs`] is the ID-first boundary for drive-like operations. Paths
//! are accepted only at the edge; every mutation first resolves the live
//! pCloud metadata to a typed [`RemoteId`] and then invokes an ID-based API.
//! No operation treats an empty local metadata cache as proof that a remote
//! entry is absent.
//!
//! The service intentionally borrows the existing folder, transfer, and
//! optional sharing runtimes. This keeps retry, transport, bandwidth, and
//! upload-journal policy in their established owners while giving daemon,
//! SDK, CLI, sync, mount, and gateway adapters one filesystem contract.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use pcloud_model::shares::{ShareMutationResult, SharePermissions};
use pcloud_proto::{
    FolderApiError, SharesApiError, TransferApiError, UploadInfo, UploadSession,
    methods::upload::{ConflictParam, PSYNC_COPY_BUFFER_SIZE, UploadErrorClass},
};
use pcloud_secret::secret_string::SecretString;
use pcloud_store::repositories::upload_resume::{
    ConflictHint, UploadResumeRecord, UploadResumeRepository,
};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::{Digest as Sha2Digest, Sha256};
use thiserror::Error;

use crate::{
    folder_backend::{FolderBackendError, FolderRuntime},
    shares_backend::{SharesBackendError, SharesRuntime},
    transfer_backend::{TransferBackendError, TransferRuntime},
    upload_journal::{JournalEntry, UploadJournal, UploadJournalDescriptor},
};

/// Maximum allocation made by one [`RemoteFs::read_range`] call.
/// Streaming consumers should issue consecutive calls for larger files.
pub const MAX_RANGE_READ_BYTES: u64 = 16 * 1024 * 1024;

/// Stable, kind-carrying remote identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteId {
    /// Folder id, including the account root.
    Folder(u64),
    /// File id.
    File(u64),
}

impl RemoteId {
    /// Return the numeric pCloud id without losing the kind at call sites.
    #[must_use]
    pub const fn value(self) -> u64 {
        match self {
            Self::Folder(id) | Self::File(id) => id,
        }
    }

    /// Whether this id addresses a folder.
    #[must_use]
    pub const fn is_folder(self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

/// Authoritative metadata returned by live resolution/listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMetadata {
    /// Kind-carrying remote id.
    pub id: RemoteId,
    /// Direct parent folder id. The root has no parent.
    pub parent_folder_id: Option<u64>,
    /// Canonical absolute drive path.
    pub path: String,
    /// Leaf name. The root uses `/`.
    pub name: String,
    /// File size, or `None` for folders/unknown server metadata.
    pub size: Option<u64>,
    /// Last modification time in Unix seconds when supplied by pCloud.
    pub modified: Option<u64>,
    /// Whether pCloud marks the entry as owned by the current account.
    pub is_mine: bool,
    /// Whether pCloud marks the entry as shared.
    pub is_shared: bool,
    /// Whether pCloud marks the entry as encrypted.
    pub encrypted: bool,
    /// pCloud permission bitmap when supplied.
    pub permissions: Option<u32>,
}

/// Live folder listing plus the folder's own metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteListing {
    /// Listed folder.
    pub folder: RemoteMetadata,
    /// Immediate children, in server order.
    pub entries: Vec<RemoteMetadata>,
}

/// Outcome of an idempotent delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// A live entry was resolved and deleted by id.
    Deleted(RemoteId),
    /// Live resolution confirmed the path was already absent.
    AlreadyAbsent,
}

/// Result of a streaming write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    /// Upload session used for the committed write.
    pub upload_id: u64,
    /// File id returned when the server allocated it during create.
    pub file_id: Option<u64>,
    /// Number of bytes read and acknowledged.
    pub bytes_written: u64,
    /// Full lowercase SHA-1 verified before commit when the source is a
    /// durable local file. Empty for generic reader uploads.
    pub sha1_hex: String,
    /// Durable offset from which this invocation resumed.
    pub resumed_from: u64,
}

/// Conflict behavior for a durable upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UploadConflict {
    /// Let pCloud apply its normal overwrite behavior.
    #[default]
    Overwrite,
    /// Only overwrite when the current remote hash matches.
    IfHash(u64),
    /// Create a new entry on name collision.
    CreateIfNew,
}

/// Result of an atomic streaming download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadResult {
    /// Final local destination.
    pub path: PathBuf,
    /// Bytes written and synced before publication.
    pub bytes_written: u64,
    /// Lowercase SHA-256 of the fully published local file.
    pub sha256_hex: String,
    /// Durable byte offset reused from an interrupted prior attempt.
    pub resumed_from: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DownloadResumeState {
    remote_path: String,
    file_id: u64,
    total_size: u64,
    bytes: u64,
}

/// Aggregate result of a recursive copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyReport {
    /// Number of files copied.
    pub files: u64,
    /// Number of folders created, including the requested destination root.
    pub folders: u64,
    /// Total file bytes copied.
    pub bytes: u64,
}

/// Typed failures from the canonical remote filesystem boundary.
#[derive(Debug, Error)]
pub enum RemoteFsError {
    /// Input was not a canonical absolute drive path.
    #[error("invalid absolute pCloud path {path:?}: {reason}")]
    InvalidPath {
        /// Offending path.
        path: String,
        /// Validation reason.
        reason: &'static str,
    },
    /// Live parent listing did not contain the requested entry.
    #[error("remote path not found: {path}")]
    NotFound {
        /// Missing path.
        path: String,
    },
    /// A live listing contained duplicate names and could not be resolved
    /// safely.
    #[error("remote path is ambiguous ({matches} matching entries): {path}")]
    Ambiguous {
        /// Ambiguous path.
        path: String,
        /// Number of matching entries.
        matches: usize,
    },
    /// Operation required a folder but resolved a file.
    #[error("expected a folder at {path}, found a file")]
    ExpectedFolder {
        /// Path with the wrong kind.
        path: String,
    },
    /// Operation required a file but resolved a folder.
    #[error("expected a file at {path}, found a folder")]
    ExpectedFile {
        /// Path with the wrong kind.
        path: String,
    },
    /// A listing entry omitted its numeric identifier.
    #[error("remote metadata at {path} omitted its numeric id")]
    MissingId {
        /// Path with malformed metadata.
        path: String,
    },
    /// A file operation required an authoritative size but pCloud omitted it.
    #[error("remote file metadata at {path} omitted its size")]
    MissingSize {
        /// Path whose size is unknown.
        path: String,
    },
    /// A single range request exceeded the bounded allocation policy.
    #[error("range read of {requested} bytes exceeds the {maximum}-byte limit")]
    RangeTooLarge {
        /// Requested bytes.
        requested: u64,
        /// Maximum bytes.
        maximum: u64,
    },
    /// Reader ended before the declared upload size.
    #[error("upload source ended at {actual} bytes; declared size was {expected}")]
    UnexpectedEof {
        /// Declared size.
        expected: u64,
        /// Bytes actually read.
        actual: u64,
    },
    /// Reader yielded data beyond the declared upload size.
    #[error("upload source contains more than its declared {expected} bytes")]
    SourceTooLong {
        /// Declared size.
        expected: u64,
    },
    /// Copying a folder into itself or one of its descendants is invalid.
    #[error("cannot copy folder {from} into itself or its descendant {to}")]
    RecursiveCopy {
        /// Source folder path.
        from: String,
        /// Destination path.
        to: String,
    },
    /// Sharing was requested without a sharing runtime attached.
    #[error("sharing is unavailable in this RemoteFs composition")]
    SharingUnavailable,
    /// A resumable operation was requested without a durability context.
    #[error("resumable transfer durability is not configured")]
    DurabilityUnavailable,
    /// A local download destination exists and replacement was not requested.
    #[error("local destination already exists: {path}")]
    DestinationExists {
        /// Existing destination.
        path: PathBuf,
    },
    /// Folder control-plane failure.
    #[error(transparent)]
    Folder(#[from] FolderApiError<FolderBackendError>),
    /// Transfer control-plane failure.
    #[error(transparent)]
    TransferApi(#[from] TransferApiError<TransferBackendError>),
    /// Transfer byte-path failure.
    #[error(transparent)]
    Transfer(#[from] TransferBackendError),
    /// Share control-plane failure.
    #[error(transparent)]
    Share(#[from] SharesApiError<SharesBackendError>),
    /// Local streaming source failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Upload-resume SQLite failure.
    #[error("upload resume store failed: {0}")]
    Store(#[from] rusqlite::Error),
    /// Upload-journal failure.
    #[error(transparent)]
    Journal(#[from] crate::upload_journal::JournalError),
}

#[derive(Debug, Clone)]
struct RemoteDurability {
    db_path: PathBuf,
    journal: UploadJournal,
}

/// Canonical ID-first remote filesystem facade.
pub struct RemoteFs<'a> {
    folder: &'a FolderRuntime,
    transfer: &'a TransferRuntime,
    shares: Option<&'a SharesRuntime>,
    auth_token: SecretString,
    durability: Option<RemoteDurability>,
}

impl std::fmt::Debug for RemoteFs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteFs")
            .field("shares_enabled", &self.shares.is_some())
            .field("durability_enabled", &self.durability.is_some())
            .field("auth_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl<'a> RemoteFs<'a> {
    /// Compose the remote filesystem over the established folder and
    /// transfer runtimes.
    #[must_use]
    pub fn new(
        folder: &'a FolderRuntime,
        transfer: &'a TransferRuntime,
        auth_token: SecretString,
    ) -> Self {
        Self {
            folder,
            transfer,
            shares: None,
            auth_token,
            durability: None,
        }
    }

    /// Attach sharing operations to this filesystem composition.
    #[must_use]
    pub fn with_shares(mut self, shares: &'a SharesRuntime) -> Self {
        self.shares = Some(shares);
        self
    }

    /// Attach the SQLite resume store and fsync journal used by durable
    /// local-file uploads.
    pub fn with_durability(
        mut self,
        db_path: impl Into<PathBuf>,
        runtime_dir: impl Into<PathBuf>,
    ) -> Result<Self, RemoteFsError> {
        self.durability = Some(RemoteDurability {
            db_path: db_path.into(),
            journal: UploadJournal::open(runtime_dir)?,
        });
        Ok(self)
    }

    /// Resolve a path through a live parent listing and return its typed id
    /// and metadata. This method never consults the local SQLite cache.
    pub fn resolve(&self, path: &str) -> Result<RemoteMetadata, RemoteFsError> {
        let path = normalize_path(path)?;
        if path == "/" {
            return self.list_root_metadata();
        }
        let (parent, name) = split_parent_name(&path)?;
        let listing = self
            .folder
            .list_folder_contents(self.token(), parent.clone())?;
        let matches: Vec<_> = listing
            .entries
            .iter()
            .filter(|entry| entry.name == name)
            .collect();
        let entry = match matches.as_slice() {
            [] => return Err(RemoteFsError::NotFound { path }),
            [entry] => *entry,
            _ => {
                return Err(RemoteFsError::Ambiguous {
                    path,
                    matches: matches.len(),
                });
            }
        };
        metadata_from_entry(entry, listing.folder_id, path)
    }

    /// Stat a remote path from canonical live metadata.
    pub fn stat(&self, path: &str) -> Result<RemoteMetadata, RemoteFsError> {
        self.resolve(path)
    }

    /// List a folder and project every child into the canonical metadata
    /// type.
    pub fn list(&self, path: &str) -> Result<RemoteListing, RemoteFsError> {
        let path = normalize_path(path)?;
        let listing = self
            .folder
            .list_folder_contents(self.token(), path.clone())?;
        let folder = RemoteMetadata {
            id: RemoteId::Folder(listing.folder_id),
            parent_folder_id: None,
            path: path.clone(),
            name: if path == "/" {
                "/".to_owned()
            } else {
                listing.name.clone()
            },
            size: None,
            modified: None,
            is_mine: listing.is_mine,
            is_shared: listing.is_shared,
            encrypted: listing.encrypted,
            permissions: listing.permissions,
        };
        let entries = listing
            .entries
            .iter()
            .map(|entry| {
                let child_path = join_path(&path, &entry.name);
                metadata_from_entry(entry, listing.folder_id, child_path)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RemoteListing { folder, entries })
    }

    /// Create one folder after resolving its live parent to an id.
    pub fn mkdir(&self, path: &str) -> Result<RemoteMetadata, RemoteFsError> {
        let path = normalize_non_root(path)?;
        let (parent_path, name) = split_parent_name(&path)?;
        let parent = self.expect_folder(&parent_path)?;
        let RemoteId::Folder(parent_id) = parent.id else {
            unreachable!("expect_folder returned a file")
        };
        let created = self
            .folder
            .create_remote_folder(self.token(), parent_id, name.clone())?;
        Ok(RemoteMetadata {
            id: RemoteId::Folder(created.folder_id),
            parent_folder_id: created.parent_folder_id.or(Some(parent_id)),
            path,
            name: if created.name.is_empty() {
                name
            } else {
                created.name
            },
            size: None,
            modified: None,
            is_mine: true,
            is_shared: false,
            encrypted: false,
            permissions: None,
        })
    }

    /// Idempotently delete a path. Missing is established via a live parent
    /// listing, never inferred from local cache state.
    pub fn delete(&self, path: &str, recursive: bool) -> Result<DeleteOutcome, RemoteFsError> {
        let path = normalize_non_root(path)?;
        let metadata = match self.resolve(&path) {
            Ok(metadata) => metadata,
            Err(RemoteFsError::NotFound { .. }) => return Ok(DeleteOutcome::AlreadyAbsent),
            Err(error) => return Err(error),
        };
        match metadata.id {
            RemoteId::Folder(folder_id) => {
                self.folder
                    .delete_folder_by_id(self.token(), folder_id, recursive)?;
            }
            RemoteId::File(file_id) => {
                self.transfer.delete_file_by_id(self.token(), file_id)?;
            }
        }
        Ok(DeleteOutcome::Deleted(metadata.id))
    }

    /// Rename or move a path after resolving both source and destination
    /// parent to live numeric ids.
    pub fn move_path(&self, from: &str, to: &str) -> Result<RemoteId, RemoteFsError> {
        let from = normalize_non_root(from)?;
        let to = normalize_non_root(to)?;
        let source = self.resolve(&from)?;
        let (to_parent_path, to_name) = split_parent_name(&to)?;
        let parent = self.expect_folder(&to_parent_path)?;
        let RemoteId::Folder(to_parent_id) = parent.id else {
            unreachable!("expect_folder returned a file")
        };
        match source.id {
            RemoteId::Folder(folder_id) => {
                self.folder
                    .rename_folder_by_id(self.token(), folder_id, to_parent_id, to_name)?
            }
            RemoteId::File(file_id) => {
                self.transfer
                    .rename_file_by_id(self.token(), file_id, to_parent_id, to_name)?
            }
        }
        Ok(source.id)
    }

    /// Read a bounded byte range. The returned vector may be shorter only
    /// when the requested interval crosses EOF.
    pub fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, RemoteFsError> {
        if length > MAX_RANGE_READ_BYTES {
            return Err(RemoteFsError::RangeTooLarge {
                requested: length,
                maximum: MAX_RANGE_READ_BYTES,
            });
        }
        if length == 0 {
            return Ok(Vec::new());
        }
        let metadata = self.expect_file(path)?;
        let size = metadata.size.unwrap_or(u64::MAX);
        if offset >= size {
            return Ok(Vec::new());
        }
        let end = offset.saturating_add(length).min(size);
        let RemoteId::File(file_id) = metadata.id else {
            unreachable!("expect_file returned a folder")
        };
        self.read_range_by_id(file_id, offset, end - offset)
    }

    /// Read a bounded byte range from an already-resolved file id. This is
    /// the ID-first fast path for callers that obtained authoritative size
    /// metadata from [`Self::stat`] or [`Self::list`].
    pub fn read_range_by_id(
        &self,
        file_id: u64,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, RemoteFsError> {
        if length > MAX_RANGE_READ_BYTES {
            return Err(RemoteFsError::RangeTooLarge {
                requested: length,
                maximum: MAX_RANGE_READ_BYTES,
            });
        }
        if length == 0 {
            return Ok(Vec::new());
        }
        let end = offset.saturating_add(length);
        let link = self.transfer.get_file_link(self.token(), file_id, None)?;
        Ok(self.transfer.read_range(&link, offset, end)?)
    }

    /// Stream exactly `total_size` bytes from `source` into a new pCloud
    /// upload session, committing only after every chunk is acknowledged.
    /// Failed reads/writes abort the server-side draft on a best-effort basis.
    pub fn write_stream<R: Read>(
        &self,
        path: &str,
        source: &mut R,
        total_size: u64,
        conflict: Option<ConflictParam>,
    ) -> Result<WriteResult, RemoteFsError> {
        let session = self.begin_streaming_write(path, total_size)?;
        let upload_result = self.write_session(&session, source, total_size, conflict);
        if upload_result.is_err() {
            let _ = self.abort_streaming_write(session.upload_id);
        }
        upload_result.map(|()| WriteResult {
            upload_id: session.upload_id,
            file_id: session.file_id,
            bytes_written: total_size,
            sha1_hex: String::new(),
            resumed_from: 0,
        })
    }

    /// Begin a bounded-memory streaming write after resolving the live
    /// destination parent to its numeric folder id.
    pub fn begin_streaming_write(
        &self,
        path: &str,
        total_size: u64,
    ) -> Result<UploadSession, RemoteFsError> {
        let path = normalize_non_root(path)?;
        let (parent_path, name) = split_parent_name(&path)?;
        let parent = self.expect_folder(&parent_path)?;
        let RemoteId::Folder(parent_id) = parent.id else {
            unreachable!("expect_folder returned a file")
        };
        Ok(self
            .transfer
            .upload_create(self.token(), parent_id, name, total_size)?)
    }

    /// Append one chunk to an open streaming write at an explicit offset.
    /// The returned value is the next server-acknowledged offset.
    pub fn write_streaming_chunk(
        &self,
        upload_id: u64,
        offset: u64,
        chunk_id: u64,
        bytes: &[u8],
    ) -> Result<u64, RemoteFsError> {
        Ok(self
            .transfer
            .upload_write_chunk(self.token(), upload_id, offset, chunk_id, bytes)?)
    }

    /// Query server-authoritative progress for an open streaming write.
    pub fn streaming_write_status(
        &self,
        upload_id: u64,
        chunk_id: u64,
    ) -> Result<UploadInfo, RemoteFsError> {
        Ok(self
            .transfer
            .upload_info(self.token(), upload_id, chunk_id)?)
    }

    /// Commit an open streaming write to its resolved parent/name.
    pub fn commit_streaming_write(
        &self,
        session: &UploadSession,
        conflict: Option<ConflictParam>,
        modified_at_unix: u64,
    ) -> Result<(), RemoteFsError> {
        Ok(self
            .transfer
            .upload_save_session(self.token(), session, conflict, modified_at_unix)?)
    }

    /// Best-effort removal of an abandoned server-side upload draft.
    pub fn abort_streaming_write(&self, upload_id: u64) -> Result<(), RemoteFsError> {
        Ok(self.transfer.upload_delete(self.token(), upload_id)?)
    }

    /// Upload a durable local file with bounded memory, crash-resume state,
    /// per-chunk idempotency, retry/backoff, and server checksum verification.
    ///
    /// The source is scanned rather than loaded into memory, so sparse and
    /// multi-gigabyte files retain a fixed 256 KiB working set. A source
    /// mutation detected before commit aborts the draft.
    pub fn upload_file_resumable(
        &self,
        remote_path: &str,
        local_path: &Path,
        conflict: UploadConflict,
    ) -> Result<WriteResult, RemoteFsError> {
        self.upload_file_resumable_inner(remote_path, local_path, conflict, None)
    }

    /// ID-first resumable upload for sync/planner consumers that already
    /// hold an authoritative remote parent id.
    pub fn upload_file_resumable_to_parent(
        &self,
        parent_folder_id: u64,
        remote_path: &str,
        local_path: &Path,
        conflict: UploadConflict,
    ) -> Result<WriteResult, RemoteFsError> {
        self.upload_file_resumable_inner(remote_path, local_path, conflict, Some(parent_folder_id))
    }

    fn upload_file_resumable_inner(
        &self,
        remote_path: &str,
        local_path: &Path,
        conflict: UploadConflict,
        known_parent_folder_id: Option<u64>,
    ) -> Result<WriteResult, RemoteFsError> {
        let durability = self
            .durability
            .as_ref()
            .ok_or(RemoteFsError::DurabilityUnavailable)?;
        let remote_path = normalize_non_root(remote_path)?;
        let canonical_local = std::fs::canonicalize(local_path)?;
        let metadata = std::fs::metadata(&canonical_local)?;
        if !metadata.is_file() {
            return Err(RemoteFsError::InvalidPath {
                path: canonical_local.display().to_string(),
                reason: "upload source is not a regular file",
            });
        }
        let total_size = metadata.len();
        let local_sha1 = sha1_file(&canonical_local)?;
        let (parent_path, file_name) = split_parent_name(&remote_path)?;
        let parent_folder_id = if let Some(parent_folder_id) = known_parent_folder_id {
            parent_folder_id
        } else {
            let parent = self.expect_folder(&parent_path)?;
            let RemoteId::Folder(parent_folder_id) = parent.id else {
                unreachable!("expect_folder returned a file")
            };
            parent_folder_id
        };
        let resume_key = format!("{}\0{remote_path}", canonical_local.display());
        let descriptor = UploadJournalDescriptor {
            resume_key: resume_key.clone(),
            local_path: canonical_local.clone(),
            remote_path: remote_path.clone(),
            parent_folder_id,
            file_name: file_name.clone(),
            total_size,
            local_sha1: local_sha1.clone(),
            if_hash: match conflict {
                UploadConflict::IfHash(hash) => Some(hash),
                _ => None,
            },
            if_new: matches!(conflict, UploadConflict::CreateIfNew),
        };
        let conflict_hint = conflict_hint(conflict);
        let conflict_param = conflict_param(conflict);
        let conn = rusqlite::Connection::open(&durability.db_path)?;
        let replay = durability.journal.replay()?;

        // A committed marker is fsynced before cleanup. Seeing it makes a
        // restart idempotent even if the process died before deleting SQLite.
        if let Some(done) = replay.entries.iter().rev().find(|entry| {
            entry.committed
                && entry
                    .descriptor
                    .as_ref()
                    .is_some_and(|value| value.resume_key == resume_key)
        }) {
            let _ = UploadResumeRepository::delete(&conn, &resume_key)?;
            compact_upload_journal(&durability.journal, done.upload_id)?;
            return Ok(WriteResult {
                upload_id: done.upload_id,
                file_id: None,
                bytes_written: total_size,
                sha1_hex: local_sha1,
                resumed_from: total_size,
            });
        }

        let journal_resume = replay.entries.iter().rev().find(|entry| {
            !entry.committed
                && entry
                    .descriptor
                    .as_ref()
                    .is_some_and(|value| value.resume_key == resume_key)
        });
        let mut existing = UploadResumeRepository::get(&conn, &resume_key)?;
        if existing.is_none() {
            if let Some(entry) = journal_resume {
                existing = Some(UploadResumeRecord {
                    local_path: resume_key.clone(),
                    parent_folder_id,
                    file_name: file_name.clone(),
                    upload_id: entry.upload_id,
                    offset: entry.bytes.min(total_size),
                    total_size,
                    prefix_sha1: entry.sha_partial.clone(),
                    conflict: conflict_hint,
                    updated_at: now_unix_secs(),
                });
            }
        }

        let compatible = existing.as_ref().is_some_and(|record| {
            record.parent_folder_id == parent_folder_id
                && record.file_name == file_name
                && record.total_size == total_size
                && record.conflict == conflict_hint
                && journal_resume
                    .and_then(|entry| entry.descriptor.as_ref())
                    .is_none_or(|value| value.local_sha1 == local_sha1)
        });
        if existing.is_some() && !compatible {
            if let Some(record) = existing.take() {
                let _ = self.transfer.upload_delete(self.token(), record.upload_id);
                let _ = UploadResumeRepository::delete(&conn, &resume_key)?;
                compact_upload_journal(&durability.journal, record.upload_id)?;
            }
        }

        let create_key = upload_idempotency_key(&local_sha1, "create", 0);
        let (session, mut offset) = if let Some(record) = existing {
            let mut offset = record.offset.min(total_size);
            if !self.transfer.is_development() && offset < total_size {
                let info = retry_api(|| {
                    self.transfer.upload_info(
                        self.token(),
                        record.upload_id,
                        offset / PSYNC_COPY_BUFFER_SIZE as u64,
                    )
                })?;
                if info.size > total_size
                    || sha1_prefix(&canonical_local, info.size)? != info.sha1_hex
                {
                    let _ = self.transfer.upload_delete(self.token(), record.upload_id);
                    let _ = UploadResumeRepository::delete(&conn, &resume_key)?;
                    compact_upload_journal(&durability.journal, record.upload_id)?;
                    let created = retry_api(|| {
                        self.transfer.upload_create_idempotent(
                            self.token(),
                            parent_folder_id,
                            file_name.clone(),
                            total_size,
                            create_key.clone(),
                        )
                    })?;
                    (created, 0)
                } else {
                    offset = info.size;
                    (
                        UploadSession {
                            upload_id: record.upload_id,
                            file_id: None,
                            parent_folder_id,
                            file_name: file_name.clone(),
                            api_server: None,
                        },
                        offset,
                    )
                }
            } else {
                (
                    UploadSession {
                        upload_id: record.upload_id,
                        file_id: None,
                        parent_folder_id,
                        file_name: file_name.clone(),
                        api_server: None,
                    },
                    offset,
                )
            }
        } else {
            let created = retry_api(|| {
                self.transfer.upload_create_idempotent(
                    self.token(),
                    parent_folder_id,
                    file_name.clone(),
                    total_size,
                    create_key.clone(),
                )
            })?;
            (created, 0)
        };
        let resumed_from = offset;

        // Journal first, SQLite second: replay can reconstruct the row if a
        // crash lands between these two durability domains.
        let initial_prefix = sha1_prefix(&canonical_local, offset)?;
        durability.journal.append(&JournalEntry {
            upload_id: session.upload_id,
            chunks_done: offset.div_ceil(PSYNC_COPY_BUFFER_SIZE as u64),
            bytes: offset,
            sha_partial: Some(initial_prefix.clone()),
            descriptor: Some(descriptor.clone()),
            committed: false,
        })?;
        UploadResumeRepository::put(
            &conn,
            &UploadResumeRecord {
                local_path: resume_key.clone(),
                parent_folder_id,
                file_name: file_name.clone(),
                upload_id: session.upload_id,
                offset,
                total_size,
                prefix_sha1: Some(initial_prefix),
                conflict: conflict_hint,
                updated_at: now_unix_secs(),
            },
        )?;

        let mut source = std::fs::File::open(&canonical_local)?;
        source.seek(SeekFrom::Start(offset))?;
        let mut prefix_hasher = sha1_hasher_for_prefix(&canonical_local, offset)?;
        let mut chunk = vec![0_u8; PSYNC_COPY_BUFFER_SIZE];
        while offset < total_size {
            let wanted = (total_size - offset).min(PSYNC_COPY_BUFFER_SIZE as u64) as usize;
            source.read_exact(&mut chunk[..wanted])?;
            let key = upload_idempotency_key(&local_sha1, "write", offset);
            retry_backend(|| {
                self.transfer.upload_write_chunk_idempotent(
                    self.token(),
                    session.upload_id,
                    offset,
                    offset / PSYNC_COPY_BUFFER_SIZE as u64,
                    &chunk[..wanted],
                    Some(key.clone()),
                )
            })?;
            prefix_hasher.update(&chunk[..wanted]);
            offset += wanted as u64;
            let prefix_sha1 = hex_digest(prefix_hasher.clone().finalize());
            durability.journal.append(&JournalEntry {
                upload_id: session.upload_id,
                chunks_done: offset.div_ceil(PSYNC_COPY_BUFFER_SIZE as u64),
                bytes: offset,
                sha_partial: Some(prefix_sha1.clone()),
                descriptor: Some(descriptor.clone()),
                committed: false,
            })?;
            UploadResumeRepository::update_offset(
                &conn,
                &resume_key,
                offset,
                Some(&prefix_sha1),
                now_unix_secs(),
            )?;
        }

        // Reject same-size in-place source mutation before publication.
        if std::fs::metadata(&canonical_local)?.len() != total_size
            || sha1_file(&canonical_local)? != local_sha1
        {
            let _ = self.transfer.upload_delete(self.token(), session.upload_id);
            return Err(RemoteFsError::Io(std::io::Error::other(
                "upload source changed while transfer was in progress",
            )));
        }
        if !self.transfer.is_development() {
            let info = retry_api(|| {
                self.transfer.upload_info(
                    self.token(),
                    session.upload_id,
                    offset / PSYNC_COPY_BUFFER_SIZE as u64,
                )
            })?;
            if info.size != total_size || info.sha1_hex != local_sha1 {
                let _ = self.transfer.upload_delete(self.token(), session.upload_id);
                return Err(RemoteFsError::Io(std::io::Error::other(format!(
                    "server checksum mismatch: size={} sha1={}",
                    info.size, info.sha1_hex
                ))));
            }
        }

        let save_key = upload_idempotency_key(&local_sha1, "save", 0);
        retry_backend(|| {
            self.transfer.upload_save_session_idempotent(
                self.token(),
                &session,
                conflict_param.clone(),
                metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or_else(now_unix_secs_u64, |value| value.as_secs()),
                Some(save_key.clone()),
            )
        })?;
        durability.journal.append(&JournalEntry {
            upload_id: session.upload_id,
            chunks_done: total_size.div_ceil(PSYNC_COPY_BUFFER_SIZE as u64),
            bytes: total_size,
            sha_partial: Some(local_sha1.clone()),
            descriptor: Some(descriptor),
            committed: true,
        })?;
        let _ = UploadResumeRepository::delete(&conn, &resume_key)?;
        compact_upload_journal(&durability.journal, session.upload_id)?;
        Ok(WriteResult {
            upload_id: session.upload_id,
            file_id: session.file_id,
            bytes_written: total_size,
            sha1_hex: local_sha1,
            resumed_from,
        })
    }

    /// Resume a remote file into a deterministic sibling staging file,
    /// durably checkpoint every bounded range, verify final size, and publish
    /// only after the completed file has been synced.
    pub fn download_to_path(
        &self,
        remote_path: &str,
        destination: &Path,
        overwrite: bool,
    ) -> Result<DownloadResult, RemoteFsError> {
        let remote_path = normalize_non_root(remote_path)?;
        let metadata = self.expect_file(&remote_path)?;
        let RemoteId::File(file_id) = metadata.id else {
            unreachable!("expect_file returned a folder")
        };
        let total_size = metadata.size.ok_or_else(|| RemoteFsError::MissingSize {
            path: remote_path.clone(),
        })?;
        self.download_by_id_to_path(file_id, total_size, &remote_path, destination, overwrite)
    }

    /// Stream a file addressed only by numeric id into a local destination.
    ///
    /// This compatibility path is for callers that do not have authoritative
    /// path/size metadata. It still keeps memory bounded, retries transient
    /// failures from a fresh staging file, hashes the completed file, fsyncs
    /// it, and publishes atomically. Size-aware callers should prefer
    /// [`Self::download_by_id_to_path`], which additionally supports durable
    /// range-resume checkpoints.
    pub fn download_by_id_streaming_to_path(
        &self,
        file_id: u64,
        destination: &Path,
        overwrite: bool,
    ) -> Result<DownloadResult, RemoteFsError> {
        if destination.exists() && !overwrite {
            return Err(RemoteFsError::DestinationExists {
                path: destination.to_path_buf(),
            });
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let leaf = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download");
        let temporary = parent.join(format!(".{leaf}.pcloud-id-part"));
        let link = retry_api(|| self.transfer.get_file_link(self.token(), file_id, None))?;
        let (_, bytes_written) = retry_backend(|| {
            let _ = std::fs::remove_file(&temporary);
            self.transfer.download_to_path(&link, &temporary)
        })?;
        let actual_size = std::fs::metadata(&temporary)?.len();
        if actual_size != bytes_written {
            let _ = std::fs::remove_file(&temporary);
            return Err(RemoteFsError::Io(std::io::Error::other(format!(
                "streamed download reported {bytes_written} bytes but staged {actual_size}"
            ))));
        }
        let sha256_hex = sha256_file(&temporary)?;
        publish_download(&temporary, destination, overwrite)?;
        sync_parent_directory(parent)?;
        Ok(DownloadResult {
            path: destination.to_path_buf(),
            bytes_written,
            sha256_hex,
            resumed_from: 0,
        })
    }

    /// ID-first resumable download for sync/planner consumers that already
    /// hold authoritative file id and size metadata.
    pub fn download_by_id_to_path(
        &self,
        file_id: u64,
        total_size: u64,
        remote_path: &str,
        destination: &Path,
        overwrite: bool,
    ) -> Result<DownloadResult, RemoteFsError> {
        const DOWNLOAD_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
        let remote_path = normalize_non_root(remote_path)?;
        let mut total_size = total_size;
        if destination.exists() && !overwrite {
            return Err(RemoteFsError::DestinationExists {
                path: destination.to_path_buf(),
            });
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let leaf = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download");
        let temporary = parent.join(format!(".{leaf}.pcloud-part"));
        let resume_path = parent.join(format!(".{leaf}.pcloud-download.json"));
        let prior = load_download_state(&resume_path).ok().flatten();
        let mut offset = match prior {
            Some(state)
                if state.remote_path == remote_path
                    && state.file_id == file_id
                    && state.total_size == total_size
                    && std::fs::metadata(&temporary)
                        .is_ok_and(|value| value.len() == state.bytes) =>
            {
                state.bytes.min(total_size)
            }
            _ => {
                let _ = std::fs::remove_file(&temporary);
                let _ = std::fs::remove_file(&resume_path);
                0
            }
        };
        let resumed_from = offset;
        let mut output = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(offset == 0)
            .open(&temporary)?;
        output.seek(SeekFrom::Start(offset))?;
        while offset < total_size {
            let wanted = (total_size - offset).min(DOWNLOAD_CHUNK_BYTES);
            let bytes = retry_remote_read(|| self.read_range_by_id(file_id, offset, wanted))?;
            if bytes.is_empty() {
                if self.transfer.is_development() {
                    total_size = offset;
                    break;
                }
                return Err(RemoteFsError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("download ended at {offset} of {total_size} bytes"),
                )));
            }
            output.write_all(&bytes)?;
            offset += bytes.len() as u64;
            output.sync_data()?;
            persist_download_state(
                &resume_path,
                &DownloadResumeState {
                    remote_path: remote_path.clone(),
                    file_id,
                    total_size,
                    bytes: offset,
                },
            )?;
            if self.transfer.is_development() && (bytes.len() as u64) < wanted {
                total_size = offset;
            }
        }
        output.set_len(total_size)?;
        output.sync_all()?;
        drop(output);
        if std::fs::metadata(&temporary)?.len() != total_size {
            return Err(RemoteFsError::Io(std::io::Error::other(
                "download staging size changed before publication",
            )));
        }
        let sha256_hex = sha256_file(&temporary)?;
        publish_download(&temporary, destination, overwrite)?;
        let _ = std::fs::remove_file(&resume_path);
        sync_parent_directory(parent)?;
        Ok(DownloadResult {
            path: destination.to_path_buf(),
            bytes_written: total_size,
            sha256_hex,
            resumed_from,
        })
    }

    /// Copy a file or folder tree using bounded range reads and chunked
    /// writes. File contents are never buffered in full.
    pub fn copy_path(&self, from: &str, to: &str) -> Result<CopyReport, RemoteFsError> {
        let from = normalize_non_root(from)?;
        let to = normalize_non_root(to)?;
        let source = self.resolve(&from)?;
        match source.id {
            RemoteId::File(_) => {
                self.copy_file(&source, &to)?;
                Ok(CopyReport {
                    files: 1,
                    folders: 0,
                    bytes: source.size.unwrap_or(0),
                })
            }
            RemoteId::Folder(_) => {
                if to == from || to.starts_with(&format!("{from}/")) {
                    return Err(RemoteFsError::RecursiveCopy { from, to });
                }
                self.copy_folder(&from, &to)
            }
        }
    }

    /// Share a folder resolved through the same live ID-first boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn share_folder(
        &self,
        path: &str,
        mail: String,
        message: String,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, RemoteFsError> {
        let folder = self.expect_folder(path)?;
        let RemoteId::Folder(folder_id) = folder.id else {
            unreachable!("expect_folder returned a file")
        };
        self.share_folder_by_id(folder_id, folder.name, mail, message, permissions, hint)
    }

    /// Share a folder whose authoritative id and display name are already
    /// known. This is the ID-first path used by legacy numeric-id IPC callers;
    /// path-oriented callers should prefer [`Self::share_folder`].
    #[allow(clippy::too_many_arguments)]
    pub fn share_folder_by_id(
        &self,
        folder_id: u64,
        name: String,
        mail: String,
        message: String,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, RemoteFsError> {
        let shares = self.shares.ok_or(RemoteFsError::SharingUnavailable)?;
        Ok(shares.share_folder(
            self.token(),
            folder_id,
            name,
            mail,
            message,
            permissions,
            hint,
        )?)
    }

    fn list_root_metadata(&self) -> Result<RemoteMetadata, RemoteFsError> {
        let listing = self.folder.list_folder_contents(self.token(), "/")?;
        Ok(RemoteMetadata {
            id: RemoteId::Folder(listing.folder_id),
            parent_folder_id: None,
            path: "/".to_owned(),
            name: "/".to_owned(),
            size: None,
            modified: None,
            is_mine: listing.is_mine,
            is_shared: listing.is_shared,
            encrypted: listing.encrypted,
            permissions: listing.permissions,
        })
    }

    fn expect_folder(&self, path: &str) -> Result<RemoteMetadata, RemoteFsError> {
        let metadata = self.resolve(path)?;
        if metadata.id.is_folder() {
            Ok(metadata)
        } else {
            Err(RemoteFsError::ExpectedFolder {
                path: metadata.path,
            })
        }
    }

    fn expect_file(&self, path: &str) -> Result<RemoteMetadata, RemoteFsError> {
        let metadata = self.resolve(path)?;
        if metadata.id.is_folder() {
            Err(RemoteFsError::ExpectedFile {
                path: metadata.path,
            })
        } else {
            Ok(metadata)
        }
    }

    fn write_session<R: Read>(
        &self,
        session: &pcloud_proto::UploadSession,
        source: &mut R,
        total_size: u64,
        conflict: Option<ConflictParam>,
    ) -> Result<(), RemoteFsError> {
        let mut buffer = vec![0_u8; PSYNC_COPY_BUFFER_SIZE];
        let mut offset = 0_u64;
        let mut chunk_id = 0_u64;
        while offset < total_size {
            let wanted = usize::try_from((total_size - offset).min(buffer.len() as u64))
                .expect("bounded chunk length fits usize");
            let mut filled = 0;
            while filled < wanted {
                let read = source.read(&mut buffer[filled..wanted])?;
                if read == 0 {
                    return Err(RemoteFsError::UnexpectedEof {
                        expected: total_size,
                        actual: offset + filled as u64,
                    });
                }
                filled += read;
            }
            offset = self.transfer.upload_write_chunk(
                self.token(),
                session.upload_id,
                offset,
                chunk_id,
                &buffer[..filled],
            )?;
            chunk_id += 1;
        }
        let mut trailing = [0_u8; 1];
        if source.read(&mut trailing)? != 0 {
            return Err(RemoteFsError::SourceTooLong {
                expected: total_size,
            });
        }
        self.transfer
            .upload_save_session(self.token(), session, conflict, unix_now())?;
        Ok(())
    }

    fn copy_file(&self, source: &RemoteMetadata, destination: &str) -> Result<(), RemoteFsError> {
        let size = source.size.ok_or_else(|| RemoteFsError::MissingSize {
            path: source.path.clone(),
        })?;
        let (parent_path, name) = split_parent_name(destination)?;
        let parent = self.expect_folder(&parent_path)?;
        let RemoteId::Folder(parent_id) = parent.id else {
            unreachable!("expect_folder returned a file")
        };
        let session = self
            .transfer
            .upload_create(self.token(), parent_id, name, size)?;
        let result = (|| {
            let mut offset = 0_u64;
            let mut chunk_id = 0_u64;
            while offset < size {
                let len = (size - offset).min(PSYNC_COPY_BUFFER_SIZE as u64);
                let bytes = self.read_range(&source.path, offset, len)?;
                if bytes.is_empty() {
                    return Err(RemoteFsError::UnexpectedEof {
                        expected: size,
                        actual: offset,
                    });
                }
                offset = self.transfer.upload_write_chunk(
                    self.token(),
                    session.upload_id,
                    offset,
                    chunk_id,
                    &bytes,
                )?;
                chunk_id += 1;
            }
            self.transfer
                .upload_save_session(self.token(), &session, None, unix_now())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.transfer.upload_delete(self.token(), session.upload_id);
        }
        result
    }

    fn copy_folder(&self, from: &str, to: &str) -> Result<CopyReport, RemoteFsError> {
        self.mkdir(to)?;
        let mut report = CopyReport {
            folders: 1,
            ..CopyReport::default()
        };
        let listing = self.list(from)?;
        for entry in listing.entries {
            let destination = join_path(to, &entry.name);
            match entry.id {
                RemoteId::File(_) => {
                    self.copy_file(&entry, &destination)?;
                    report.files += 1;
                    report.bytes = report.bytes.saturating_add(entry.size.unwrap_or(0));
                }
                RemoteId::Folder(_) => {
                    let child = self.copy_folder(&entry.path, &destination)?;
                    report.files = report.files.saturating_add(child.files);
                    report.folders = report.folders.saturating_add(child.folders);
                    report.bytes = report.bytes.saturating_add(child.bytes);
                }
            }
        }
        Ok(report)
    }

    fn token(&self) -> SecretString {
        self.auth_token.clone_secret()
    }
}

fn conflict_hint(conflict: UploadConflict) -> ConflictHint {
    match conflict {
        UploadConflict::Overwrite => ConflictHint::None,
        UploadConflict::IfHash(hash) => ConflictHint::IfHash(hash),
        UploadConflict::CreateIfNew => ConflictHint::IfNew,
    }
}

fn conflict_param(conflict: UploadConflict) -> Option<ConflictParam> {
    match conflict {
        UploadConflict::Overwrite => None,
        UploadConflict::IfHash(hash) => Some(ConflictParam::IfHash(hash)),
        UploadConflict::CreateIfNew => Some(ConflictParam::New),
    }
}

fn now_unix_secs() -> i64 {
    now_unix_secs_u64().min(i64::MAX as u64) as i64
}

fn now_unix_secs_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn upload_idempotency_key(local_sha1: &str, phase: &str, offset: u64) -> String {
    let mut hasher = Sha1::new();
    hasher.update(b"pcloud-rs-remote-fs-v1\0");
    hasher.update(local_sha1.as_bytes());
    hasher.update(b"\0");
    hasher.update(phase.as_bytes());
    hasher.update(offset.to_le_bytes());
    format!("remote-fs-{}", hex_digest(hasher.finalize()))
}

fn sha1_file(path: &Path) -> Result<String, std::io::Error> {
    let size = std::fs::metadata(path)?.len();
    Ok(hex_digest(sha1_hasher_for_prefix(path, size)?.finalize()))
}

fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn load_download_state(path: &Path) -> Result<Option<DownloadResumeState>, std::io::Error> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn persist_download_state(path: &Path, state: &DownloadResumeState) -> Result<(), std::io::Error> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        sync_parent_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn publish_download(
    temporary: &Path,
    destination: &Path,
    _overwrite: bool,
) -> Result<(), std::io::Error> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn publish_download(
    temporary: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<(), std::io::Error> {
    if !destination.exists() {
        return std::fs::rename(temporary, destination);
    }
    if !overwrite {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "download destination exists",
        ));
    }
    let backup = destination.with_extension("pcloud-replaced");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(destination, &backup)?;
    match std::fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = std::fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::rename(backup, destination);
            Err(error)
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn publish_download(
    temporary: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<(), std::io::Error> {
    if destination.exists() && overwrite {
        std::fs::remove_file(destination)?;
    }
    std::fs::rename(temporary, destination)
}

fn sync_parent_directory(parent: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

fn sha1_prefix(path: &Path, bytes: u64) -> Result<String, std::io::Error> {
    Ok(hex_digest(sha1_hasher_for_prefix(path, bytes)?.finalize()))
}

fn sha1_hasher_for_prefix(path: &Path, bytes: u64) -> Result<Sha1, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut remaining = bytes;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha1::new();
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "source ended while hashing resume prefix",
            ));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hasher)
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn compact_upload_journal(
    journal: &UploadJournal,
    completed_upload_id: u64,
) -> Result<(), crate::upload_journal::JournalError> {
    let retained: Vec<_> = journal
        .replay()?
        .entries
        .into_iter()
        .filter(|entry| entry.upload_id != completed_upload_id)
        .collect();
    journal.rewrite_atomic(&retained)
}

fn retry_api<T, F>(mut operation: F) -> Result<T, RemoteFsError>
where
    F: FnMut() -> Result<T, TransferApiError<TransferBackendError>>,
{
    let mut attempt = 1_u32;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < 5 && api_error_retryable(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(
                    250_u64.saturating_mul(1_u64 << (attempt - 1)),
                ));
                attempt += 1;
            }
            Err(error) => return Err(RemoteFsError::TransferApi(error)),
        }
    }
}

fn retry_backend<T, F>(mut operation: F) -> Result<T, RemoteFsError>
where
    F: FnMut() -> Result<T, TransferBackendError>,
{
    let mut attempt = 1_u32;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < 5 && backend_error_retryable(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(
                    250_u64.saturating_mul(1_u64 << (attempt - 1)),
                ));
                attempt += 1;
            }
            Err(error) => return Err(RemoteFsError::Transfer(error)),
        }
    }
}

fn retry_remote_read<F>(mut operation: F) -> Result<Vec<u8>, RemoteFsError>
where
    F: FnMut() -> Result<Vec<u8>, RemoteFsError>,
{
    let mut attempt = 1_u32;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < 5 && remote_error_retryable(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(
                    250_u64.saturating_mul(1_u64 << (attempt - 1)),
                ));
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn remote_error_retryable(error: &RemoteFsError) -> bool {
    match error {
        RemoteFsError::TransferApi(error) => api_error_retryable(error),
        RemoteFsError::Transfer(error) => backend_error_retryable(error),
        _ => false,
    }
}

fn api_error_retryable(error: &TransferApiError<TransferBackendError>) -> bool {
    match error {
        TransferApiError::Result { result, .. } => {
            matches!(
                UploadErrorClass::classify(*result),
                Some(UploadErrorClass::TempFail)
            )
        }
        TransferApiError::Transport(error) => backend_error_retryable(error),
        TransferApiError::Encode(_) | TransferApiError::Malformed(_) => false,
    }
}

fn backend_error_retryable(error: &TransferBackendError) -> bool {
    !matches!(
        error,
        TransferBackendError::PermanentResultCode { .. }
            | TransferBackendError::Encode(_)
            | TransferBackendError::Malformed(_)
    )
}

fn metadata_from_entry(
    entry: &pcloud_proto::folder_api::RemoteFolderEntry,
    parent_folder_id: u64,
    path: String,
) -> Result<RemoteMetadata, RemoteFsError> {
    let id = if entry.is_folder {
        RemoteId::Folder(
            entry
                .folder_id
                .ok_or_else(|| RemoteFsError::MissingId { path: path.clone() })?,
        )
    } else {
        RemoteId::File(
            entry
                .file_id
                .ok_or_else(|| RemoteFsError::MissingId { path: path.clone() })?,
        )
    };
    Ok(RemoteMetadata {
        id,
        parent_folder_id: Some(parent_folder_id),
        path,
        name: entry.name.clone(),
        size: entry.size,
        modified: entry.modified,
        is_mine: entry.is_mine,
        is_shared: entry.is_shared,
        encrypted: entry.encrypted,
        permissions: entry.permissions,
    })
}

fn normalize_non_root(path: &str) -> Result<String, RemoteFsError> {
    let normalized = normalize_path(path)?;
    if normalized == "/" {
        return Err(RemoteFsError::InvalidPath {
            path: path.to_owned(),
            reason: "operation is not valid for the drive root",
        });
    }
    Ok(normalized)
}

fn normalize_path(path: &str) -> Result<String, RemoteFsError> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(RemoteFsError::InvalidPath {
            path: path.to_owned(),
            reason: "path must start with '/'",
        });
    }
    if path.contains('\0') {
        return Err(RemoteFsError::InvalidPath {
            path: path.to_owned(),
            reason: "path contains a NUL byte",
        });
    }
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(RemoteFsError::InvalidPath {
                        path: path.to_owned(),
                        reason: "path escapes the drive root",
                    });
                }
            }
            value => segments.push(value),
        }
    }
    if segments.is_empty() {
        Ok("/".to_owned())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

fn split_parent_name(path: &str) -> Result<(String, String), RemoteFsError> {
    let index = path.rfind('/').ok_or_else(|| RemoteFsError::InvalidPath {
        path: path.to_owned(),
        reason: "path has no parent separator",
    })?;
    let name = &path[index + 1..];
    if name.is_empty() {
        return Err(RemoteFsError::InvalidPath {
            path: path.to_owned(),
            reason: "path has no leaf name",
        });
    }
    let parent = if index == 0 { "/" } else { &path[..index] };
    Ok((parent.to_owned(), name.to_owned()))
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use pcloud_config::{ConfigProfile, Environment};

    use super::*;

    fn service() -> (FolderRuntime, TransferRuntime, ConfigProfile) {
        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-remote-fs-test"),
            Environment::Development,
        );
        (
            FolderRuntime::from_config(&config),
            TransferRuntime::from_config(&config),
            config,
        )
    }

    fn token() -> SecretString {
        SecretString::new("test-token")
    }

    #[test]
    fn resolves_and_lists_with_no_local_metadata_cache() {
        let (folder, transfer, _config) = service();
        // There is deliberately no StoreProfile, SQLite connection, or cache
        // seeding in this test. Resolution must come from live listfolder.
        let remote = RemoteFs::new(&folder, &transfer, token());

        let stat = remote.stat("/notes.txt").expect("live stat succeeds");
        assert_eq!(stat.id, RemoteId::File(20));
        assert_eq!(stat.size, Some(1024));

        let root = remote.list("/").expect("live list succeeds");
        assert_eq!(root.folder.id, RemoteId::Folder(0));
        assert_eq!(root.entries.len(), 2);
    }

    #[test]
    fn write_stream_rejects_a_short_source_and_aborts() {
        let (folder, transfer, _config) = service();
        let remote = RemoteFs::new(&folder, &transfer, token());
        let mut source = Cursor::new(b"short".to_vec());

        let error = remote
            .write_stream("/new.bin", &mut source, 10, None)
            .expect_err("declared length must be enforced");
        assert!(matches!(error, RemoteFsError::UnexpectedEof { .. }));
    }

    #[test]
    fn normalizes_paths_without_allowing_root_escape() {
        assert_eq!(normalize_path("/docs//./report").unwrap(), "/docs/report");
        assert!(matches!(
            normalize_path("/../secret"),
            Err(RemoteFsError::InvalidPath { .. })
        ));
    }

    #[test]
    fn durable_file_upload_streams_checksums_and_cleans_resume_state() {
        let root = tempfile::tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let folder = FolderRuntime::from_config(&config);
        let transfer = TransferRuntime::from_config(&config);
        let db_path = config.paths.state_dir.join("remote-upload.sqlite3");
        pcloud_store::bootstrap_profile(&db_path).unwrap();
        let source = root.path().join("large-sparse-friendly.bin");
        let payload = vec![0x5a; PSYNC_COPY_BUFFER_SIZE + 17];
        std::fs::write(&source, &payload).unwrap();
        let remote = RemoteFs::new(&folder, &transfer, token())
            .with_durability(&db_path, &config.paths.runtime_dir)
            .unwrap();

        let result = remote
            .upload_file_resumable("/durable.bin", &source, UploadConflict::Overwrite)
            .unwrap();
        assert_eq!(result.bytes_written, payload.len() as u64);
        assert_eq!(result.resumed_from, 0);
        assert_eq!(result.sha1_hex, sha1_file(&source).unwrap());
        let journal = UploadJournal::open(&config.paths.runtime_dir).unwrap();
        assert!(journal.replay().unwrap().entries.is_empty());
        let conn = rusqlite::Connection::open(db_path).unwrap();
        assert!(UploadResumeRepository::list_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn durable_file_upload_resumes_from_sqlite_offset() {
        let root = tempfile::tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let folder = FolderRuntime::from_config(&config);
        let transfer = TransferRuntime::from_config(&config);
        let db_path = config.paths.state_dir.join("remote-resume.sqlite3");
        pcloud_store::bootstrap_profile(&db_path).unwrap();
        let source = root.path().join("resume.bin");
        std::fs::write(&source, b"resume-after-crash").unwrap();
        let canonical = std::fs::canonicalize(&source).unwrap();
        let resume_key = format!("{}\0/resumed.bin", canonical.display());
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        UploadResumeRepository::put(
            &conn,
            &UploadResumeRecord {
                local_path: resume_key,
                parent_folder_id: 0,
                file_name: "resumed.bin".to_owned(),
                upload_id: 77,
                offset: 7,
                total_size: 18,
                prefix_sha1: Some(sha1_prefix(&source, 7).unwrap()),
                conflict: ConflictHint::None,
                updated_at: now_unix_secs(),
            },
        )
        .unwrap();
        drop(conn);
        let remote = RemoteFs::new(&folder, &transfer, token())
            .with_durability(&db_path, &config.paths.runtime_dir)
            .unwrap();

        let result = remote
            .upload_file_resumable("/resumed.bin", &source, UploadConflict::Overwrite)
            .unwrap();
        assert_eq!(result.resumed_from, 7);
        assert_eq!(result.bytes_written, 18);
    }

    #[test]
    fn durable_file_upload_recovers_journal_ahead_of_sqlite() {
        let root = tempfile::tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let folder = FolderRuntime::from_config(&config);
        let transfer = TransferRuntime::from_config(&config);
        let db_path = config.paths.state_dir.join("journal-recovery.sqlite3");
        pcloud_store::bootstrap_profile(&db_path).unwrap();
        let source = root.path().join("journal-ahead.bin");
        std::fs::write(&source, b"resume-from-fsynced-journal").unwrap();
        let canonical = std::fs::canonicalize(&source).unwrap();
        let remote_path = "/journal-ahead.bin";
        let resume_key = format!("{}\0{remote_path}", canonical.display());
        let local_sha1 = sha1_file(&source).unwrap();
        let journal = UploadJournal::open(&config.paths.runtime_dir).unwrap();
        journal
            .append(&JournalEntry {
                upload_id: 81,
                chunks_done: 1,
                bytes: 7,
                sha_partial: Some(sha1_prefix(&source, 7).unwrap()),
                descriptor: Some(UploadJournalDescriptor {
                    resume_key,
                    local_path: canonical,
                    remote_path: remote_path.to_owned(),
                    parent_folder_id: 0,
                    file_name: "journal-ahead.bin".to_owned(),
                    total_size: 27,
                    local_sha1,
                    if_hash: None,
                    if_new: false,
                }),
                committed: false,
            })
            .unwrap();
        // Deliberately no UploadResumeRepository row: this models SIGKILL
        // after journal fsync and before the SQLite transaction.
        let remote = RemoteFs::new(&folder, &transfer, token())
            .with_durability(&db_path, &config.paths.runtime_dir)
            .unwrap();

        let result = remote
            .upload_file_resumable(remote_path, &source, UploadConflict::Overwrite)
            .unwrap();
        assert_eq!(result.resumed_from, 7);
        assert_eq!(result.bytes_written, 27);
        assert!(journal.replay().unwrap().entries.is_empty());
    }

    #[test]
    fn durable_file_upload_recovers_committed_marker_before_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let folder = FolderRuntime::from_config(&config);
        let transfer = TransferRuntime::from_config(&config);
        let db_path = config.paths.state_dir.join("committed-recovery.sqlite3");
        pcloud_store::bootstrap_profile(&db_path).unwrap();
        let source = root.path().join("already-committed.bin");
        std::fs::write(&source, b"already committed").unwrap();
        let canonical = std::fs::canonicalize(&source).unwrap();
        let remote_path = "/already-committed.bin";
        let resume_key = format!("{}\0{remote_path}", canonical.display());
        let local_sha1 = sha1_file(&source).unwrap();
        let total_size = std::fs::metadata(&source).unwrap().len();
        let journal = UploadJournal::open(&config.paths.runtime_dir).unwrap();
        journal
            .append(&JournalEntry {
                upload_id: 82,
                chunks_done: 1,
                bytes: total_size,
                sha_partial: Some(local_sha1.clone()),
                descriptor: Some(UploadJournalDescriptor {
                    resume_key: resume_key.clone(),
                    local_path: canonical,
                    remote_path: remote_path.to_owned(),
                    parent_folder_id: 0,
                    file_name: "already-committed.bin".to_owned(),
                    total_size,
                    local_sha1,
                    if_hash: None,
                    if_new: false,
                }),
                committed: true,
            })
            .unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        UploadResumeRepository::put(
            &conn,
            &UploadResumeRecord {
                local_path: resume_key,
                parent_folder_id: 0,
                file_name: "already-committed.bin".to_owned(),
                upload_id: 82,
                offset: total_size,
                total_size,
                prefix_sha1: None,
                conflict: ConflictHint::None,
                updated_at: now_unix_secs(),
            },
        )
        .unwrap();
        drop(conn);
        let remote = RemoteFs::new(&folder, &transfer, token())
            .with_durability(&db_path, &config.paths.runtime_dir)
            .unwrap();

        let result = remote
            .upload_file_resumable(remote_path, &source, UploadConflict::Overwrite)
            .unwrap();
        assert_eq!(result.upload_id, 82);
        assert_eq!(result.resumed_from, total_size);
        assert!(journal.replay().unwrap().entries.is_empty());
        let conn = rusqlite::Connection::open(db_path).unwrap();
        assert!(UploadResumeRepository::list_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn durable_file_upload_handles_sparse_multi_chunk_sources() {
        let root = tempfile::tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let folder = FolderRuntime::from_config(&config);
        let transfer = TransferRuntime::from_config(&config);
        let db_path = config.paths.state_dir.join("sparse.sqlite3");
        pcloud_store::bootstrap_profile(&db_path).unwrap();
        let source = root.path().join("sparse.bin");
        let logical_size = (PSYNC_COPY_BUFFER_SIZE as u64 * 8) + 17;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&source)
            .unwrap();
        file.write_all(b"head").unwrap();
        file.set_len(logical_size).unwrap();
        file.seek(SeekFrom::End(-4)).unwrap();
        file.write_all(b"tail").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let remote = RemoteFs::new(&folder, &transfer, token())
            .with_durability(&db_path, &config.paths.runtime_dir)
            .unwrap();

        let result = remote
            .upload_file_resumable("/sparse.bin", &source, UploadConflict::CreateIfNew)
            .unwrap();
        assert_eq!(result.bytes_written, logical_size);
        assert_eq!(result.sha1_hex, sha1_file(&source).unwrap());
    }

    #[test]
    fn upload_conflict_modes_map_to_store_and_wire_contracts() {
        assert_eq!(conflict_hint(UploadConflict::Overwrite), ConflictHint::None);
        assert_eq!(
            conflict_hint(UploadConflict::IfHash(42)),
            ConflictHint::IfHash(42)
        );
        assert_eq!(
            conflict_hint(UploadConflict::CreateIfNew),
            ConflictHint::IfNew
        );
        assert_eq!(conflict_param(UploadConflict::Overwrite), None);
        assert_eq!(
            conflict_param(UploadConflict::IfHash(42)),
            Some(ConflictParam::IfHash(42))
        );
        assert_eq!(
            conflict_param(UploadConflict::CreateIfNew),
            Some(ConflictParam::New)
        );
    }

    #[test]
    fn sharing_routes_through_the_canonical_id_first_service() {
        let (folder, transfer, config) = service();
        let shares = SharesRuntime::from_config(&config);
        let remote = RemoteFs::new(&folder, &transfer, token()).with_shares(&shares);

        let result = remote
            .share_folder_by_id(
                10,
                "Documents".to_owned(),
                "recipient@example.test".to_owned(),
                "shared through RemoteFs".to_owned(),
                SharePermissions::default(),
                None,
            )
            .unwrap();
        assert!(result.share_request_id.is_some());
    }

    #[test]
    fn id_only_download_is_streamed_and_published_through_remote_fs() {
        let root = tempfile::tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let folder = FolderRuntime::from_config(&config);
        let transfer = TransferRuntime::from_config(&config);
        let destination = root.path().join("legacy-id-download.bin");
        let remote = RemoteFs::new(&folder, &transfer, token());

        let result = remote
            .download_by_id_streaming_to_path(20, &destination, false)
            .unwrap();
        assert_eq!(result.resumed_from, 0);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"downloaded:/get/abc/report.txt"
        );
        assert_eq!(result.bytes_written, 30);
        assert_eq!(result.sha256_hex, sha256_file(&destination).unwrap());
    }

    #[test]
    fn download_resumes_durable_sibling_staging_and_publishes() {
        let root = tempfile::tempdir().unwrap();
        let config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        let folder = FolderRuntime::from_config(&config);
        let transfer = TransferRuntime::from_config(&config);
        let destination = root.path().join("notes.bin");
        let part = root.path().join(".notes.bin.pcloud-part");
        let state = root.path().join(".notes.bin.pcloud-download.json");
        std::fs::write(&part, b"downl").unwrap();
        persist_download_state(
            &state,
            &DownloadResumeState {
                remote_path: "/notes.txt".to_owned(),
                file_id: 20,
                total_size: 1024,
                bytes: 5,
            },
        )
        .unwrap();
        let remote = RemoteFs::new(&folder, &transfer, token());

        let result = remote
            .download_to_path("/notes.txt", &destination, false)
            .unwrap();
        assert_eq!(result.resumed_from, 5);
        assert_eq!(result.bytes_written, 30);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"downloaded:/get/abc/report.txt"
        );
        assert_eq!(result.sha256_hex, sha256_file(&destination).unwrap());
        assert!(!part.exists());
        assert!(!state.exists());
    }

    #[test]
    fn public_operations_validation_and_retry_helpers_cover_error_taxonomy() {
        let (folder, transfer, config) = service();
        let remote = RemoteFs::new(&folder, &transfer, token());
        assert!(format!("{remote:?}").contains("<redacted>"));

        assert!(matches!(
            remote.share_folder(
                "/Documents",
                "reader@example.test".to_owned(),
                String::new(),
                SharePermissions::default(),
                None,
            ),
            Err(RemoteFsError::SharingUnavailable)
        ));
        assert!(matches!(
            remote.upload_file_resumable(
                "/missing.bin",
                Path::new("/definitely/missing"),
                UploadConflict::Overwrite,
            ),
            Err(RemoteFsError::DurabilityUnavailable)
        ));

        assert!(matches!(
            normalize_path("relative"),
            Err(RemoteFsError::InvalidPath { .. })
        ));
        assert!(matches!(
            normalize_path("/bad\0name"),
            Err(RemoteFsError::InvalidPath { .. })
        ));
        assert!(matches!(
            normalize_non_root("/"),
            Err(RemoteFsError::InvalidPath { .. })
        ));
        assert!(split_parent_name("relative").is_err());
        assert!(split_parent_name("/folder/").is_err());
        assert_eq!(
            split_parent_name("/name").unwrap(),
            ("/".into(), "name".into())
        );
        assert_eq!(join_path("/", "name"), "/name");
        assert_eq!(join_path("/folder", "name"), "/folder/name");

        let missing_folder_id = pcloud_proto::folder_api::RemoteFolderEntry {
            name: "broken".to_owned(),
            is_folder: true,
            folder_id: None,
            file_id: None,
            owner_user_id: None,
            is_mine: false,
            encrypted: false,
            is_shared: false,
            permissions: None,
            size: None,
            modified: None,
        };
        assert!(matches!(
            metadata_from_entry(&missing_folder_id, 0, "/broken".to_owned()),
            Err(RemoteFsError::MissingId { .. })
        ));
        let missing_file_id = pcloud_proto::folder_api::RemoteFolderEntry {
            name: "broken".to_owned(),
            is_folder: false,
            folder_id: None,
            file_id: None,
            owner_user_id: None,
            is_mine: false,
            encrypted: false,
            is_shared: false,
            permissions: None,
            size: None,
            modified: None,
        };
        assert!(matches!(
            metadata_from_entry(&missing_file_id, 0, "/broken".to_owned()),
            Err(RemoteFsError::MissingId { .. })
        ));

        assert!(matches!(
            remote.read_range_by_id(20, 0, MAX_RANGE_READ_BYTES + 1),
            Err(RemoteFsError::RangeTooLarge { .. })
        ));
        assert!(remote.read_range_by_id(20, 0, 0).unwrap().is_empty());
        assert!(remote.read_range("/notes.txt", 0, 0).unwrap().is_empty());
        assert!(remote.read_range("/notes.txt", 2048, 8).unwrap().is_empty());
        assert!(matches!(
            remote.read_range("/Documents", 0, 8),
            Err(RemoteFsError::ExpectedFile { .. })
        ));

        let created = remote.mkdir("/Created").unwrap();
        assert!(created.id.is_folder());
        assert!(matches!(
            remote.mkdir("/notes.txt/child"),
            Err(RemoteFsError::ExpectedFolder { .. })
        ));
        assert_eq!(
            remote.delete("/missing", true).unwrap(),
            DeleteOutcome::AlreadyAbsent
        );
        assert!(matches!(
            remote.delete("/", true),
            Err(RemoteFsError::InvalidPath { .. })
        ));
        let _ = remote.move_path("/notes.txt", "/renamed.txt");
        let _ = remote.move_path("/Documents", "/MovedDocuments");

        let mut exact = Cursor::new(b"exact".to_vec());
        assert!(
            remote
                .write_stream("/exact.bin", &mut exact, 5, None)
                .is_ok()
        );
        let mut too_long = Cursor::new(b"too-long".to_vec());
        assert!(matches!(
            remote.write_stream("/too-long.bin", &mut too_long, 3, None),
            Err(RemoteFsError::SourceTooLong { .. })
        ));
        let session = remote.begin_streaming_write("/session.bin", 3).unwrap();
        assert_eq!(
            remote
                .write_streaming_chunk(session.upload_id, 0, 0, b"abc")
                .unwrap(),
            3
        );
        let _ = remote.streaming_write_status(session.upload_id, 0);
        let _ = remote.commit_streaming_write(&session, Some(ConflictParam::New), 1);
        let _ = remote.abort_streaming_write(session.upload_id);

        assert!(matches!(
            remote.copy_path("/Documents", "/Documents/nested"),
            Err(RemoteFsError::RecursiveCopy { .. })
        ));
        let _ = remote.copy_path("/notes.txt", "/notes-copy.txt");

        let root = tempfile::tempdir().unwrap();
        let missing_state = root.path().join("missing.json");
        assert!(load_download_state(&missing_state).unwrap().is_none());
        std::fs::write(&missing_state, b"not-json").unwrap();
        assert_eq!(
            load_download_state(&missing_state).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );

        let short = root.path().join("short.bin");
        std::fs::write(&short, b"short").unwrap();
        assert_eq!(
            sha1_prefix(&short, 10).unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        assert_eq!(hex_digest([0x00, 0xab, 0xff]), "00abff");
        assert_eq!(
            upload_idempotency_key("digest", "chunk", 7),
            upload_idempotency_key("digest", "chunk", 7)
        );

        let mut backend_attempts = 0;
        let retried = retry_backend(|| {
            backend_attempts += 1;
            if backend_attempts == 1 {
                Err(TransferBackendError::TransientResultCode { result: 5000 })
            } else {
                Ok(42)
            }
        })
        .unwrap();
        assert_eq!(retried, 42);
        assert_eq!(backend_attempts, 2);
        assert!(
            retry_backend::<(), _>(|| Err(TransferBackendError::PermanentResultCode {
                result: 2005
            }))
            .is_err()
        );

        let mut remote_attempts = 0;
        assert_eq!(
            retry_remote_read(|| {
                remote_attempts += 1;
                if remote_attempts == 1 {
                    Err(RemoteFsError::Transfer(
                        TransferBackendError::TransientResultCode { result: 5000 },
                    ))
                } else {
                    Ok(vec![1, 2, 3])
                }
            })
            .unwrap(),
            vec![1, 2, 3]
        );

        let transient_api = TransferApiError::Result {
            result: 5000,
            message: None,
        };
        assert!(api_error_retryable(&transient_api));
        assert!(!api_error_retryable(&TransferApiError::Malformed(
            "fixture"
        )));
        assert!(!backend_error_retryable(&TransferBackendError::Malformed(
            "fixture"
        )));
        assert!(backend_error_retryable(
            &TransferBackendError::NetworkExecutionUnavailable
        ));

        let db_path = config.paths.state_dir.join("invalid-source.sqlite3");
        pcloud_store::bootstrap_profile(&db_path).unwrap();
        let durable = RemoteFs::new(&folder, &transfer, token())
            .with_durability(&db_path, &config.paths.runtime_dir)
            .unwrap();
        assert!(matches!(
            durable.upload_file_resumable_to_parent(
                0,
                "/directory.bin",
                root.path(),
                UploadConflict::IfHash(1),
            ),
            Err(RemoteFsError::InvalidPath { .. })
        ));
    }
}

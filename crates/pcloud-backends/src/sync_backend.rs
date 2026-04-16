//! Sync-root lifecycle backend: authenticated add with remote-folder
//! validation and local path canonicalization, duplicate/nested-root
//! rejection, persisted list, remove with queued-work eviction, sync
//! suggestions, and syncability classification. Called from
//! `pcloud-daemon::dispatch`; drives the `pcloud-engine` runtime.
//!
//! Portable API; live sync execution is simplified relative to the C
//! daemon and mounted-drive coupling is tracked under `bd-1du.4`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::mount_discovery::{MountDiscovery, default_ignore_patterns, is_ignored_under};

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_engine::diff_events::{DiffEventDispatcher, dispatch_diff_batch};
use pcloud_engine::diff_poller::{RemoteDiffBatch, RemoteDiffEntry};
use pcloud_model::ids::{RemoteFolderId, SyncId};
use pcloud_model::sync::{ChangeKind, EntryKind};
use pcloud_proto::BinaryParamValue;
use pcloud_proto::{
    BinaryApiTransport, DiffBatch, EncodedRequest, ParseLimits, RemoteFolderInfo,
    ResponseParseError, SyncApi, SyncApiError, TransportConfig, TransportError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    folder_api::{FolderApi, FolderApiError},
    parse_response_frame,
    response::Value,
};
use pcloud_resilience::clock::{Clock, SystemClock};
use pcloud_resilience::retry::{BackoffSchedule, RetryDecision, RetryPolicy};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use pcloud_store::DiffStateRepository;
use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Clone, Default)]
/// `DevelopmentSyncTransport` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct DevelopmentSyncTransport;

impl ProtocolTransport for DevelopmentSyncTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "diff" => encode_hash_response(&[
                ("diffid", EncodedValue::Number(1)),
                ("hasmore", EncodedValue::Bool(false)),
                (
                    "entries",
                    EncodedValue::Array(vec![EncodedValue::Hash(vec![
                        ("event", EncodedValue::Number(1)),
                        (
                            "metadata",
                            EncodedValue::Hash(vec![
                                ("name", EncodedValue::String("report.txt")),
                                ("isfolder", EncodedValue::Bool(false)),
                                ("fileid", EncodedValue::Number(9)),
                                ("parentfolderid", EncodedValue::Number(2)),
                            ]),
                        ),
                    ])]),
                ),
            ]),
            "listfolder" if request.params.iter().any(|param| {
                param.name == "path"
                    && matches!(&param.value, BinaryParamValue::String(path) if path == "/remote-sync")
            }) => encode_hash_response(&[(
                "metadata",
                EncodedValue::Hash(vec![
                    ("folderid", EncodedValue::Number(17)),
                    ("name", EncodedValue::String("remote-sync")),
                ]),
            )]),
            "listfolder" => encode_hash_response(&[
                ("result", EncodedValue::Number(2005)),
                (
                    "error",
                    EncodedValue::String("Directory does not exist."),
                ),
            ]),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported command: {}", request.frame.command),
            )),
        }?;

        parse_response_frame(&frame, &ParseLimits::default()).map_err(map_response_parse_err)
    }
}

impl ApiServerHintConsumer for DevelopmentSyncTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
}

#[derive(Debug, Error)]
/// `SyncBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum SyncBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// `ValidatedRemoteFolder` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct ValidatedRemoteFolder {
    /// `folder_id` field.
    pub folder_id: RemoteFolderId,
    /// `path` field.
    pub path: String,
    /// `name` field.
    pub name: String,
}

/// One entry returned by [`suggest_sync_folders`].
///
/// Mirrors the shape of the C `psuggested_folder_t` in
/// `pclsync/pfoldersync.h`: a canonical local path, a display name, and a
/// short description. The underlying scorer is the extension-weighted
/// implementation in [`crate::sync_suggest`], which is a port of
/// `pclsync/psuggest.c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestedSyncFolder {
    /// `local_path` field.
    pub local_path: String,
    /// `name` field.
    pub name: String,
    /// `description` field.
    pub description: String,
    /// `file_count` field.
    pub file_count: u64,
}

/// Reasons a candidate local folder cannot be used as a sync root.
/// These mirror the `PERROR_*` codes returned by the C
/// `psync_is_folder_syncable` implementation in `pclsync/psynclib.c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderSyncabilityIssue {
    /// `PathDoesNotExist` variant.
    PathDoesNotExist,
    /// `PathIsNotADirectory` variant.
    PathIsNotADirectory,
    /// `AlreadyTrackedAsSyncRoot` variant.
    AlreadyTrackedAsSyncRoot,
    /// Candidate path is a parent, child, or equal to an existing sync root.
    OverlapsExistingSyncRoot {
        /// Canonicalized path of the existing sync root that overlaps.
        existing_local_path: String,
    },
    /// Candidate path lives inside a currently mounted pCloud drive.
    InsideMountedPCloudDrive {
        /// Mount point detected as a pCloud drive on this host.
        mount_point: String,
    },
    /// Candidate path is within a folder excluded by the ignore list.
    InsideIgnoredFolder {
        /// Absolute path of the ignored ancestor folder.
        ignored_path: String,
    },
}

impl FolderSyncabilityIssue {
    #[must_use]
    /// Invoke `message` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn message(&self) -> String {
        match self {
            Self::PathDoesNotExist => "local path does not exist".to_owned(),
            Self::PathIsNotADirectory => "local path is not a directory".to_owned(),
            Self::AlreadyTrackedAsSyncRoot => {
                "there is already an active sync or backup for this folder".to_owned()
            }
            Self::OverlapsExistingSyncRoot {
                existing_local_path,
            } => format!(
                "there is already an active sync or backup for a parent or child of this folder: {existing_local_path}"
            ),
            Self::InsideMountedPCloudDrive { mount_point } => {
                format!("folder is located on pCloud drive (mount point: {mount_point})")
            }
            Self::InsideIgnoredFolder { ignored_path } => format!(
                "this folder is a child of a folder in your ignore folders list: {ignored_path}"
            ),
        }
    }
}

/// Caller overrides applied on top of auto-discovered mount and ignore
/// lists when classifying sync candidates.
///
/// When any field is `Some`, the auto-discovered value is suppressed and
/// only the override is consulted. This mirrors the C call shape where
/// test harnesses can substitute explicit mount / ignore inputs while
/// real daemon code relies on the live system view.
#[derive(Debug, Default, Clone)]
pub struct FolderSyncabilityOverrides<'a> {
    /// `drive_mount_points` field.
    pub drive_mount_points: Option<&'a [&'a str]>,
    /// `virtual_mount_points` field.
    pub virtual_mount_points: Option<&'a [&'a str]>,
    /// `ignored_paths` field.
    pub ignored_paths: Option<&'a [&'a str]>,
    /// `default_ignore_patterns` field.
    pub default_ignore_patterns: Option<&'a [&'a str]>,
}

/// Classify whether `candidate` can be adopted as a new sync root, with
/// explicit mount and ignore lists supplied by the caller. This is the
/// low-level entry point used by tests.
///
/// Prefer [`classify_folder_syncability`] in daemon code: it auto-fills
/// the `drive_mount_points`, virtual filesystem mounts, and ignore paths
/// from the live mount table and the built-in defaults.
pub fn classify_folder_syncability_with_lists(
    candidate: &Path,
    existing_sync_roots: &[&str],
    drive_mount_points: &[&str],
    ignored_paths: &[&str],
) -> Result<PathBuf, FolderSyncabilityIssue> {
    let canonical = match std::fs::canonicalize(candidate) {
        Ok(path) => path,
        Err(_) => return Err(FolderSyncabilityIssue::PathDoesNotExist),
    };
    if !canonical.is_dir() {
        return Err(FolderSyncabilityIssue::PathIsNotADirectory);
    }

    for existing in existing_sync_roots {
        let existing_path = PathBuf::from(existing);
        if canonical == existing_path {
            return Err(FolderSyncabilityIssue::AlreadyTrackedAsSyncRoot);
        }
        if canonical.starts_with(&existing_path) || existing_path.starts_with(&canonical) {
            return Err(FolderSyncabilityIssue::OverlapsExistingSyncRoot {
                existing_local_path: existing_path.display().to_string(),
            });
        }
    }

    for mount in drive_mount_points {
        let mount_path = PathBuf::from(mount);
        if canonical == mount_path || canonical.starts_with(&mount_path) {
            return Err(FolderSyncabilityIssue::InsideMountedPCloudDrive {
                mount_point: mount_path.display().to_string(),
            });
        }
    }

    for ignored in ignored_paths {
        if is_ignored_under(&canonical, ignored) {
            return Err(FolderSyncabilityIssue::InsideIgnoredFolder {
                ignored_path: ignored.to_string(),
            });
        }
    }

    Ok(canonical)
}

/// Classify whether `candidate` can be adopted as a new sync root.
///
/// Auto-discovers live pCloud-drive mounts, virtual filesystem mounts,
/// and the default ignore-path list from [`crate::mount_discovery`]. The
/// C analogue in `pclsync/psynclib.c` pulls the same information from
/// `pfs_getmountpoint()` and the `ignorepaths` setting; the Rust path
/// additionally rejects pseudo filesystems (`/proc`, `/sys`, cgroup,
/// tmpfs special cases) and snap/flatpak runtime trees up front.
///
/// Pass [`FolderSyncabilityOverrides`] to substitute explicit values
/// (useful in tests and development runtimes).
pub fn classify_folder_syncability(
    candidate: &Path,
    existing_sync_roots: &[&str],
    discovery: &MountDiscovery,
    overrides: &FolderSyncabilityOverrides<'_>,
) -> Result<PathBuf, FolderSyncabilityIssue> {
    // Build mount list: either override, or discovery + virtual mounts.
    let auto_drive = discovery.pcloud_mount_points();
    let auto_virtual = discovery.virtual_mount_points();

    let drive_owned: Vec<String> = match overrides.drive_mount_points {
        Some(list) => list.iter().map(|s| (*s).to_string()).collect(),
        None => {
            let mut v: Vec<String> = auto_drive.iter().map(|p| p.display().to_string()).collect();
            if overrides.virtual_mount_points.is_none() {
                v.extend(auto_virtual.iter().map(|p| p.display().to_string()));
            }
            v
        }
    };
    let mut drive_refs: Vec<&str> = drive_owned.iter().map(String::as_str).collect();
    let virt_owned: Vec<String> = match overrides.virtual_mount_points {
        Some(list) => list.iter().map(|s| (*s).to_string()).collect(),
        None => Vec::new(),
    };
    drive_refs.extend(virt_owned.iter().map(String::as_str));

    // Build ignore list: override, or default + configured.
    let ignore_owned: Vec<String> = match overrides.ignored_paths {
        Some(list) => list.iter().map(|s| (*s).to_string()).collect(),
        None => {
            let defaults: Vec<String> = match overrides.default_ignore_patterns {
                Some(list) => list.iter().map(|s| (*s).to_string()).collect(),
                None => default_ignore_patterns(),
            };
            defaults
        }
    };
    let ignore_refs: Vec<&str> = ignore_owned.iter().map(String::as_str).collect();

    classify_folder_syncability_with_lists(
        candidate,
        existing_sync_roots,
        &drive_refs,
        &ignore_refs,
    )
}

/// Scan `root` and return a ranked list of candidate sync folders using
/// the extension-weighted scorer ported from `pclsync/psuggest.c`.
///
/// The scorer walks the tree depth-first (bounded by the traversal caps
/// in [`crate::sync_suggest`]), classifies each file by extension into
/// one of five buckets (other, pictures, videos, music, documents), and
/// emits any folder whose non-"other" files dominate its contents. See
/// [`crate::sync_suggest::scan_folder_with_limit`] for the full
/// algorithm and bounded-traversal guarantees.
pub fn suggest_sync_folders(
    root: &Path,
    max: usize,
) -> Result<Vec<SuggestedSyncFolder>, io::Error> {
    let raw = crate::sync_suggest::scan_folder_with_limit(root, max)?;
    Ok(raw
        .into_iter()
        .map(|s| SuggestedSyncFolder {
            local_path: s.local_path,
            name: s.name,
            description: s.description,
            file_count: u64::from(s.file_count),
        })
        .collect())
}

#[derive(Debug, Clone)]
enum SyncTransportMode {
    Development(DevelopmentSyncTransport),
    Network(BinaryApiTransport),
}

impl ProtocolTransport for SyncTransportMode {
    type Error = SyncBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(transport) => {
                transport.execute(request).map_err(SyncBackendError::from)
            }
            Self::Network(transport) => transport.execute(request).map_err(SyncBackendError::from),
        }
    }
}

impl ApiServerHintConsumer for SyncTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(transport) => transport.apply_api_server_hint(api_server),
            Self::Network(transport) => transport.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the sync-root lifecycle backend.
///
/// # Architecture role
///
/// - Dispatches `SyncAdd`, `SyncList`, `SyncRemove`, `SyncSuggest`, and
///   `SyncClassify` IPC request frames from `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `listfolder` (for remote-root
///   validation on add) and `diff` (driven by the embedded [`DiffWorker`]
///   for the retained engine path). Wire encoding uses the crate-level
///   `EncodedValue` pattern.
/// - Emits audit events for sync-root add, remove, and diff-tick failures.
///   Engine-queue eviction on remove is logged in the audit trail.
/// - Persists to `pcloud-store` tables `sync_roots` (local path
///   canonicalized, remote folder id validated) and `diff_state`
///   (per-sync diffid checkpoints). On `SyncRemove`, engine queue entries
///   are evicted **before** the API call so a partial failure is safe to
///   retry idempotently (caller must retry until the backend reports the
///   root absent).
/// - Error taxonomy: see [`SyncBackendError`]. Diff-loop errors surface
///   via [`DiffWorkerError`] / [`DiffTickOutcome`].
pub struct SyncRuntime {
    api: SyncApi<SyncTransportMode>,
    folder_api: FolderApi<SyncTransportMode>,
}

impl SyncRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => SyncTransportMode::Development(DevelopmentSyncTransport),
            ApiMode::Plaintext | ApiMode::Tls => {
                SyncTransportMode::Network(BinaryApiTransport::new(TransportConfig {
                    host: config.api.host.clone(),
                    port: config.api.port,
                    server_name: config.api.server_name.clone(),
                    use_tls: matches!(config.api.mode, ApiMode::Tls),
                    connect_timeout: std::time::Duration::from_millis(
                        config.api.connect_timeout_ms,
                    ),
                    read_timeout: std::time::Duration::from_millis(config.api.read_timeout_ms),
                }))
            }
        };

        Self {
            api: SyncApi::new(transport.clone()),
            folder_api: FolderApi::new(transport),
        }
    }

    /// Invoke `diff` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn diff(
        &self,
        auth_token: SecretString,
        cursor: u64,
        limit: u64,
    ) -> Result<RemoteDiffBatch, SyncApiError<SyncBackendError>> {
        let batch = self.api.diff(auth_token.expose_secret(), cursor, limit)?;
        Ok(convert_diff_batch(batch))
    }

    /// Invoke `validate_remote_folder` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn validate_remote_folder(
        &self,
        auth_token: SecretString,
        path: &str,
    ) -> Result<ValidatedRemoteFolder, FolderApiError<SyncBackendError>> {
        let normalized_path = normalize_remote_path(path);
        let folder = self
            .folder_api
            .list_folder_by_path(auth_token.expose_secret(), normalized_path.clone())?;
        Ok(convert_folder(folder, normalized_path))
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
        self.folder_api.apply_api_server_hint(api_server);
    }
}

/// Enforce the data-residency policy at the `sync-root add` call site.
///
/// Given the remote folder's API-server hint (typically carried on the
/// `listfolder` response used by [`SyncRuntime::validate_remote_folder`]),
/// this helper consults [`crate::residency::enforce`] and returns the
/// decision plus a [`crate::residency::ResidencyAuditEvent`] the caller
/// must persist to the audit sink. Under strict mode with a non-matching
/// region the call site should refuse with
/// [`pcloud_ipc::ResponseStatus::PolicyViolation`] (`kind =
/// "data_residency"`); under non-strict mode the event carries
/// `warned = true` and the sync root is still created.
#[must_use]
pub fn enforce_sync_root_add_residency(
    policy: &pcloud_config::data_residency::DataResidencyPolicy,
    cache: &crate::residency::RegionCache,
    metadata: &crate::residency::FolderMetadataHint,
) -> (
    crate::residency::ResidencyDecision,
    crate::residency::ResidencyAuditEvent,
) {
    let region = cache.resolve_or_insert_with(metadata.folder_id, || {
        crate::residency::resolve_region(metadata)
    });
    crate::residency::enforce(policy, region, crate::residency::ACTION_SYNC_ROOT_ADD)
}

fn convert_diff_batch(batch: DiffBatch) -> RemoteDiffBatch {
    RemoteDiffBatch {
        sync_id: pcloud_model::ids::SyncId::new(1),
        cursor: batch.diff_id,
        has_more: batch.has_more,
        entries: batch
            .entries
            .into_iter()
            .map(|entry| RemoteDiffEntry {
                path: path_from_metadata(&entry.metadata),
                entry_kind: if entry.metadata.is_folder {
                    EntryKind::Folder
                } else {
                    EntryKind::File
                },
                change_kind: if entry.metadata.deleted {
                    ChangeKind::Delete
                } else {
                    ChangeKind::Upsert
                },
                remote_file_id: entry
                    .metadata
                    .file_id
                    .map(pcloud_model::ids::RemoteFileId::new),
                remote_folder_id: entry
                    .metadata
                    .folder_id
                    .map(pcloud_model::ids::RemoteFolderId::new),
                event: entry.event,
            })
            .collect(),
    }
}

fn path_from_metadata(metadata: &pcloud_proto::DiffEntryMetadata) -> String {
    metadata.name.clone()
}

fn convert_folder(folder: RemoteFolderInfo, normalized_path: String) -> ValidatedRemoteFolder {
    ValidatedRemoteFolder {
        folder_id: RemoteFolderId::new(folder.folder_id),
        path: normalized_path,
        name: folder.name,
    }
}

fn normalize_remote_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "/" {
        return "/".to_owned();
    }

    let segments = trimmed
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

enum EncodedValue<'a> {
    Bool(bool),
    Number(u64),
    String(&'a str),
    Array(Vec<EncodedValue<'a>>),
    Hash(Vec<(&'a str, EncodedValue<'a>)>),
}

fn encode_hash_response(entries: &[(&str, EncodedValue<'_>)]) -> Result<Vec<u8>, io::Error> {
    const RPARAM_NUM8: u8 = 15;
    const RPARAM_HASH: u8 = 16;
    const RPARAM_ARRAY: u8 = 17;
    const RPARAM_BFALSE: u8 = 18;
    const RPARAM_BTRUE: u8 = 19;
    const RPARAM_SMALL_NUM_BASE: u8 = 200;
    const RPARAM_END: u8 = 255;

    fn encode_value(payload: &mut Vec<u8>, value: &EncodedValue<'_>) -> Result<(), io::Error> {
        match value {
            EncodedValue::Bool(false) => payload.push(RPARAM_BFALSE),
            EncodedValue::Bool(true) => payload.push(RPARAM_BTRUE),
            EncodedValue::Number(number) if *number < 20 => {
                payload.push(RPARAM_SMALL_NUM_BASE + (*number as u8));
            }
            EncodedValue::Number(number) => {
                payload.push(RPARAM_NUM8);
                payload.extend_from_slice(&number.to_le_bytes());
            }
            EncodedValue::String(value) => encode_string(payload, value)?,
            EncodedValue::Array(values) => {
                payload.push(RPARAM_ARRAY);
                for value in values {
                    encode_value(payload, value)?;
                }
                payload.push(RPARAM_END);
            }
            EncodedValue::Hash(entries) => {
                payload.push(RPARAM_HASH);
                for (key, value) in entries {
                    encode_string(payload, key)?;
                    encode_value(payload, value)?;
                }
                payload.push(RPARAM_END);
            }
        }
        Ok(())
    }

    let mut payload = vec![RPARAM_HASH];
    for (key, value) in entries {
        encode_string(&mut payload, key)?;
        encode_value(&mut payload, value)?;
    }
    payload.push(RPARAM_END);

    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn encode_string(payload: &mut Vec<u8>, value: &str) -> Result<(), io::Error> {
    const RPARAM_SHORT_STR_BASE: u8 = 100;
    if value.len() > 49 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "development response encoder only supports short strings",
        ));
    }
    payload.push(RPARAM_SHORT_STR_BASE + value.len() as u8);
    payload.extend_from_slice(value.as_bytes());
    Ok(())
}

// =====================================================================
// DiffWorker — continuous remote-diff worker (sync row 75: diff polling).
// =====================================================================

/// Initial inter-poll wait when the server has nothing to deliver. Mirrors
/// the C `pdiff` long-poll start cadence (the C code uses a true subscribe
/// long-poll; the Rust path falls back to adaptive single-shot polling
/// until a real subscribe transport is wired in).
pub const PDIFF_INITIAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on the inter-poll wait. Matches the C
/// `PSYNC_SLEEP_BEFORE_RECONNECT * 12 = 60s` ceiling.
pub const PDIFF_MAX_TIMEOUT: Duration = Duration::from_secs(60);
/// Diff fetch limit (`difflimit`). Mirrors C `PSYNC_DIFF_LIMIT`. A smaller
/// default avoids over-fetching during bootstrap; the C value (500_000)
/// was tuned for very large initial syncs and is overkill for steady-state.
pub const PDIFF_DEFAULT_LIMIT: u64 = 1024;

/// Errors surfaced by [`DiffWorker::tick`].
#[derive(Debug, Error)]
pub enum DiffWorkerError {
    #[error("diff transport failed: {0}")]
    /// `Diff` variant.
    Diff(String),
    #[error("diff state persistence failed: {0}")]
    /// `Persist` variant.
    Persist(#[from] rusqlite::Error),
    /// Retry budget was exhausted without a successful diff tick; the
    /// worker has surfaced the last transport error and stopped.
    #[error("diff worker exhausted retry budget after {attempts} attempts: {last_error}")]
    GiveUp {
        /// Number of retry attempts consumed before giving up.
        attempts: u32,
        /// Redacted representation of the last underlying error.
        last_error: String,
    },
}

/// Outcome of one [`DiffWorker::tick`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffTickOutcome {
    /// Server returned no entries. The worker idled and is waiting for
    /// the negotiated timeout to elapse before the next call.
    Idle {
        /// Current diffid cursor (unchanged this tick).
        diffid: u64,
        /// Inter-poll wait the worker will use next.
        next_wait: Duration,
    },
    /// One batch was processed and the diffid advanced. Returns the new
    /// cursor and the count of dispatched events.
    Processed {
        /// `diffid` field.
        diffid: u64,
        /// `dispatched` field.
        dispatched: usize,
        /// `more_available` field.
        more_available: bool,
    },
    /// A retryable error occurred; the worker has computed a backoff
    /// wait and will retry on the next tick.
    BackoffPending {
        /// `wait` field.
        wait: Duration,
        /// `attempt` field.
        attempt: u32,
        /// `error` field.
        error: String,
    },
}

/// Continuous remote-diff worker for one sync root.
///
/// Mirrors the responsibilities of the C `psync_diff_thread`
/// (`pclsync/pdiff.c:2931`): drive `diff` calls, process the entry
/// batch, advance the cursor, persist it across restart, and back off
/// on errors. Long-poll timeout negotiation is approximated via an
/// adaptive inter-poll wait that grows on idle replies and resets on
/// productive replies.
///
/// Auth tokens passed in via [`DiffWorker::tick`] are zeroized on drop
/// (they are `SecretString`).
pub struct DiffWorker {
    sync_id: SyncId,
    cursor: u64,
    next_wait: Duration,
    last_attempt: Option<Instant>,
    retry_policy: RetryPolicy,
    retry_attempt: u32,
    clock: Arc<dyn Clock>,
    limit: u64,
}

impl std::fmt::Debug for DiffWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiffWorker")
            .field("sync_id", &self.sync_id)
            .field("cursor", &self.cursor)
            .field("next_wait", &self.next_wait)
            .field("retry_attempt", &self.retry_attempt)
            .field("limit", &self.limit)
            .finish()
    }
}

impl DiffWorker {
    /// Build a worker with system clock and a default jittered
    /// exponential backoff (matching the C path's `psys_sleep_milliseconds(
    /// PSYNC_SLEEP_BEFORE_RECONNECT)` after a failure but bounded so it
    /// never sleeps longer than `PDIFF_MAX_TIMEOUT`).
    #[must_use]
    pub fn new(sync_id: SyncId, initial_cursor: u64) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        Self::with_clock(sync_id, initial_cursor, clock)
    }

    /// Build a worker with an injected clock (deterministic tests).
    pub fn with_clock(sync_id: SyncId, initial_cursor: u64, clock: Arc<dyn Clock>) -> Self {
        let retry_policy = RetryPolicy::with_clock(
            6,
            BackoffSchedule::ExponentialJittered {
                base: Duration::from_millis(500),
                factor: 2.0,
                max: PDIFF_MAX_TIMEOUT,
                seed: 0x70C1_0DDD_DFFF_70AB,
            },
            clock.clone(),
        );
        Self {
            sync_id,
            cursor: initial_cursor,
            next_wait: PDIFF_INITIAL_TIMEOUT,
            last_attempt: None,
            retry_policy,
            retry_attempt: 0,
            clock,
            limit: PDIFF_DEFAULT_LIMIT,
        }
    }

    /// Restore a worker's cursor from the `sync_diff_state` table.
    pub fn restore_cursor(sync_id: SyncId, conn: &Connection) -> Result<u64, rusqlite::Error> {
        Ok(DiffStateRepository::load(conn, sync_id)?.map_or(0, |r| r.diffid))
    }

    /// Sync root this worker is bound to.
    #[must_use]
    pub fn sync_id(&self) -> SyncId {
        self.sync_id
    }

    /// Last persisted diffid the worker is operating against.
    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Run one iteration. Performs at most one `diff` call. On success,
    /// dispatches each event entry through `dispatcher` and persists the
    /// new cursor via `conn`. On error, returns a `BackoffPending`
    /// outcome with the wait the runtime should observe before the next
    /// tick (do **not** sleep here; the caller owns the runtime).
    pub fn tick<D: DiffEventDispatcher>(
        &mut self,
        runtime: &SyncRuntime,
        auth_token: SecretString,
        conn: &Connection,
        dispatcher: &mut D,
    ) -> Result<DiffTickOutcome, DiffWorkerError> {
        // Honor the negotiated wait between calls.
        let now = self.clock.now();
        if let Some(last) = self.last_attempt
            && now.duration_since(last) < self.next_wait
        {
            return Ok(DiffTickOutcome::Idle {
                diffid: self.cursor,
                next_wait: self.next_wait,
            });
        }
        self.last_attempt = Some(now);

        let result = runtime.diff(auth_token, self.cursor, self.limit);
        match result {
            Ok(batch) => {
                // On success: reset retry counter and adapt the timeout.
                self.retry_attempt = 0;
                let entries = batch.entries.clone();
                let event_tags: Vec<Option<u64>> = entries.iter().map(|e| e.event).collect();
                let dispatched =
                    dispatch_diff_batch(self.sync_id, &entries, &event_tags, dispatcher);

                if entries.is_empty() {
                    // No work — grow the negotiated timeout up to PDIFF_MAX_TIMEOUT.
                    self.next_wait = (self.next_wait.saturating_mul(2)).min(PDIFF_MAX_TIMEOUT);
                    return Ok(DiffTickOutcome::Idle {
                        diffid: self.cursor,
                        next_wait: self.next_wait,
                    });
                }

                // Productive reply — shrink the wait and advance + persist cursor.
                self.next_wait = PDIFF_INITIAL_TIMEOUT;
                self.cursor = batch.cursor;
                let updated_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                DiffStateRepository::save(conn, self.sync_id, self.cursor, updated_at)?;

                Ok(DiffTickOutcome::Processed {
                    diffid: self.cursor,
                    dispatched,
                    more_available: batch.has_more,
                })
            }
            Err(err) => {
                let msg = err.to_string();
                self.retry_attempt = self.retry_attempt.saturating_add(1);
                match self.retry_policy.next(self.retry_attempt) {
                    RetryDecision::Retry { wait } => {
                        // Force a wait at least this long before the next attempt.
                        self.next_wait = wait;
                        Ok(DiffTickOutcome::BackoffPending {
                            wait,
                            attempt: self.retry_attempt,
                            error: msg,
                        })
                    }
                    RetryDecision::GiveUp => Err(DiffWorkerError::GiveUp {
                        attempts: self.retry_attempt,
                        last_error: msg,
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
mod diff_worker_tests {
    use super::*;
    use pcloud_engine::diff_events::ClassifiedDiffEvent;
    use pcloud_resilience::clock::ManualClock;
    use std::path::PathBuf;

    fn temp_db(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-diff-worker-{}-{}-{}.sqlite3",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn dev_runtime() -> SyncRuntime {
        let root =
            std::env::temp_dir().join(format!("pcloud-diff-worker-runtime-{}", std::process::id()));
        let cfg = pcloud_config::ConfigProfile::secure_defaults(
            root,
            pcloud_config::Environment::Development,
        );
        SyncRuntime::from_config(&cfg)
    }

    #[derive(Default, Debug)]
    struct RecordingDispatcher {
        fs: Vec<String>,
        share: Vec<String>,
        crypto: usize,
        account: usize,
        unknown: usize,
    }

    impl DiffEventDispatcher for RecordingDispatcher {
        fn handle_filesystem(&mut self, ev: &ClassifiedDiffEvent) {
            self.fs.push(format!("{:?}:{}", ev.kind, ev.entry.path));
        }
        fn handle_share(&mut self, ev: &ClassifiedDiffEvent) {
            self.share.push(format!("{:?}", ev.kind));
        }
        fn handle_crypto(&mut self, _ev: &ClassifiedDiffEvent) {
            self.crypto += 1;
        }
        fn handle_account(&mut self, _ev: &ClassifiedDiffEvent) {
            self.account += 1;
        }
        fn handle_unknown(&mut self, _ev: &ClassifiedDiffEvent) {
            self.unknown += 1;
        }
    }

    #[test]
    fn first_tick_processes_dev_batch_and_advances_diffid() {
        let path = temp_db("first");
        let _ = std::fs::remove_file(&path);
        let _ = pcloud_store::bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("conn");

        let clock = ManualClock::new();
        let arc: Arc<dyn Clock> = Arc::new(clock.clone());
        let mut worker = DiffWorker::with_clock(SyncId::new(1), 0, arc);

        let runtime = dev_runtime();
        let mut disp = RecordingDispatcher::default();

        // The dev transport returns one entry with event=1 (createfolder).
        // Wait `next_wait` is 30s default; advance past it so the tick fires.
        clock.advance(Duration::from_secs(31));

        let outcome = worker
            .tick(&runtime, SecretString::new("token"), &conn, &mut disp)
            .expect("first tick should succeed");
        match outcome {
            DiffTickOutcome::Processed {
                diffid, dispatched, ..
            } => {
                assert_eq!(diffid, 1);
                assert_eq!(dispatched, 1);
            }
            other => panic!("expected Processed, got {other:?}"),
        }

        // Cursor persisted to sync_diff_state.
        let row = DiffStateRepository::load(&conn, SyncId::new(1))
            .unwrap()
            .expect("row");
        assert_eq!(row.diffid, 1);

        // event=1 → CreateFolder → filesystem family.
        assert_eq!(disp.fs.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn restart_picks_up_persisted_diffid() {
        let path = temp_db("restart");
        let _ = std::fs::remove_file(&path);
        let _ = pcloud_store::bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("conn");

        // Pre-populate the persisted cursor as if a previous daemon run
        // already advanced to diffid=42.
        DiffStateRepository::save(&conn, SyncId::new(7), 42, 1_700_000_000).unwrap();

        let restored = DiffWorker::restore_cursor(SyncId::new(7), &conn).expect("restore");
        assert_eq!(restored, 42);

        let worker = DiffWorker::new(SyncId::new(7), restored);
        assert_eq!(worker.cursor(), 42);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn worker_idles_within_negotiated_timeout() {
        let path = temp_db("idle");
        let _ = std::fs::remove_file(&path);
        let _ = pcloud_store::bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("conn");

        let clock = ManualClock::new();
        let arc: Arc<dyn Clock> = Arc::new(clock.clone());
        let mut worker = DiffWorker::with_clock(SyncId::new(1), 0, arc);
        let runtime = dev_runtime();
        let mut disp = RecordingDispatcher::default();

        // First tick fires (no last_attempt yet).
        let _ = worker
            .tick(&runtime, SecretString::new("token"), &conn, &mut disp)
            .unwrap();

        // Without advancing the clock, the second tick must idle.
        let outcome = worker
            .tick(&runtime, SecretString::new("token"), &conn, &mut disp)
            .unwrap();
        assert!(matches!(outcome, DiffTickOutcome::Idle { .. }));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dispatcher_routes_share_request_to_share_handler() {
        // Synthesize a batch with explicit event tags, bypassing the
        // dev transport which only emits event=1. We exercise the
        // dispatcher contract directly to prove share/crypto routing.
        let mut disp = RecordingDispatcher::default();
        let entries = vec![
            RemoteDiffEntry {
                path: "docs/x".to_owned(),
                entry_kind: EntryKind::File,
                change_kind: ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
                event: Some(8), // ShareRequestIn
            },
            RemoteDiffEntry {
                path: "docs/y".to_owned(),
                entry_kind: EntryKind::File,
                change_kind: ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
                event: Some(26), // CryptoPassChange
            },
        ];
        let tags: Vec<Option<u64>> = entries.iter().map(|e| e.event).collect();
        let n = dispatch_diff_batch(SyncId::new(1), &entries, &tags, &mut disp);
        assert_eq!(n, 2);
        assert_eq!(disp.share.len(), 1);
        assert_eq!(disp.crypto, 1);
        assert!(disp.fs.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use pcloud_config::{ConfigProfile, Environment};
    use pcloud_secret::secret_string::SecretString;

    use super::{SyncRuntime, normalize_remote_path};

    #[test]
    fn normalize_remote_path_collapses_empty_segments() {
        assert_eq!(normalize_remote_path("remote-sync"), "/remote-sync");
        assert_eq!(normalize_remote_path("//alpha///beta/"), "/alpha/beta");
        assert_eq!(normalize_remote_path("/"), "/");
    }

    #[test]
    fn development_runtime_validates_known_remote_folder() {
        let root =
            std::env::temp_dir().join(format!("pcloud-sync-runtime-test-{}", std::process::id()));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        let runtime = SyncRuntime::from_config(&config);

        let validated = runtime
            .validate_remote_folder(SecretString::new("token"), "//remote-sync/")
            .expect("known development folder should validate");

        assert_eq!(validated.folder_id.get(), 17);
        assert_eq!(validated.path, "/remote-sync");
        assert_eq!(validated.name, "remote-sync");
    }

    #[test]
    fn classify_folder_syncability_detects_nested_roots() {
        use super::{FolderSyncabilityIssue, classify_folder_syncability_with_lists};
        let tmp = std::env::temp_dir().join(format!(
            "pcloud-sync-classify-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let canonical_tmp = std::fs::canonicalize(&tmp).unwrap();
        let existing = canonical_tmp.display().to_string();

        let nested = canonical_tmp.join("inside");
        std::fs::create_dir_all(&nested).unwrap();
        let result =
            classify_folder_syncability_with_lists(&nested, &[existing.as_str()], &[], &[])
                .unwrap_err();
        assert!(matches!(
            result,
            FolderSyncabilityIssue::OverlapsExistingSyncRoot { .. }
        ));

        let ignored = canonical_tmp.display().to_string();
        let err = classify_folder_syncability_with_lists(&nested, &[], &[], &[ignored.as_str()])
            .unwrap_err();
        assert!(matches!(
            err,
            FolderSyncabilityIssue::InsideIgnoredFolder { .. }
        ));

        let mount = canonical_tmp.display().to_string();
        let err = classify_folder_syncability_with_lists(&nested, &[], &[mount.as_str()], &[])
            .unwrap_err();
        assert!(matches!(
            err,
            FolderSyncabilityIssue::InsideMountedPCloudDrive { .. }
        ));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn auto_classify_applies_overrides() {
        use super::{
            FolderSyncabilityIssue, FolderSyncabilityOverrides, classify_folder_syncability,
        };
        use crate::mount_discovery::MountDiscovery;

        let tmp = std::env::temp_dir().join(format!(
            "pcloud-sync-auto-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let canonical = std::fs::canonicalize(&tmp).unwrap();
        let cstr = canonical.display().to_string();

        // Happy path with empty overrides (real discovery).
        let discovery = MountDiscovery::default();
        let ok = classify_folder_syncability(
            &canonical,
            &[],
            &discovery,
            &FolderSyncabilityOverrides::default(),
        );
        assert!(ok.is_ok(), "temp dir should be syncable: {:?}", ok);

        // Mount override rejects.
        let mounts = [cstr.as_str()];
        let err = classify_folder_syncability(
            &canonical,
            &[],
            &discovery,
            &FolderSyncabilityOverrides {
                drive_mount_points: Some(&mounts),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            FolderSyncabilityIssue::InsideMountedPCloudDrive { .. }
        ));

        // Ignore override rejects.
        let ignores = [cstr.as_str()];
        let err = classify_folder_syncability(
            &canonical,
            &[],
            &discovery,
            &FolderSyncabilityOverrides {
                ignored_paths: Some(&ignores),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            FolderSyncabilityIssue::InsideIgnoredFolder { .. }
        ));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn auto_classify_rejects_proc_when_mounted() {
        use super::{FolderSyncabilityIssue, classify_folder_syncability};
        use crate::mount_discovery::MountDiscovery;
        // /proc is covered either by virtual-mount discovery on Linux or
        // by the default ignore patterns on every platform.
        let discovery = MountDiscovery::default();
        let res = classify_folder_syncability(
            std::path::Path::new("/proc"),
            &[],
            &discovery,
            &super::FolderSyncabilityOverrides::default(),
        );
        match res {
            Err(FolderSyncabilityIssue::InsideMountedPCloudDrive { .. })
            | Err(FolderSyncabilityIssue::InsideIgnoredFolder { .. })
            | Err(FolderSyncabilityIssue::PathDoesNotExist) => {}
            other => panic!("expected rejection for /proc, got {:?}", other),
        }
    }

    #[test]
    fn suggest_sync_folders_uses_extension_scorer() {
        use super::suggest_sync_folders;
        let tmp = std::env::temp_dir().join(format!(
            "pcloud-sync-suggest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let docs = tmp.join("docs");
        let hidden = tmp.join(".hidden");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(&hidden).unwrap();
        // 30 real documents -> clears the extension-scorer threshold.
        for i in 0..30 {
            std::fs::write(docs.join(format!("a{i}.txt")), b"hi").unwrap();
        }
        // Hidden dirs are ignored regardless of their contents.
        for i in 0..30 {
            std::fs::write(hidden.join(format!("h{i}.txt")), b"hi").unwrap();
        }
        // Plant "other" noise at the root so the scorer must descend
        // into `docs` instead of suggesting the parent.
        for i in 0..500 {
            std::fs::write(tmp.join(format!("n{i}.dat")), b"hi").unwrap();
        }

        let suggestions = suggest_sync_folders(&tmp, 10).unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "docs");
        assert_eq!(suggestions[0].file_count, 30);
        assert!(suggestions[0].description.contains("documents"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn development_runtime_rejects_unknown_remote_folder() {
        let root = std::env::temp_dir().join(format!(
            "pcloud-sync-runtime-test-{}-missing",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        let runtime = SyncRuntime::from_config(&config);

        let err = runtime
            .validate_remote_folder(SecretString::new("token"), "/missing")
            .expect_err("missing development folder should fail");

        assert!(err.to_string().contains("2005"));
    }
}

/// Test-only mock fixture for the `sync_backend` subsystem.
///
/// Promoted from the `pcloud-fs` mock-backend pattern (R18 wave-01
/// audit ask) so this backend can be driven by integration tests
/// without a live transport or store. The fixture wraps the shared
/// [`crate::mock::MockFixture`] recorders and exposes a representative
/// call helper that records the canonical protocol command this
/// backend issues on its happy path.
///
/// The fixture is `Send + Sync`, deterministic (no sleeps or clocks),
/// and cheap to construct via [`Default`].
pub mod mock {
    use crate::mock::{MockEvent, MockFixture};

    /// Canonical protocol command exercised by [`Fixture::record_representative_call`].
    pub const REPRESENTATIVE_COMMAND: &str = "listfolder";

    /// Thin wrapper around [`MockFixture`] specialised for this backend.
    #[derive(Debug, Default)]
    pub struct Fixture {
        /// Underlying shared recorders.
        pub fixture: MockFixture,
    }

    impl Fixture {
        /// Construct a new mock fixture for this backend.
        pub fn new() -> Self {
            Self::default()
        }

        /// Record the representative sync runtime call (listfolder for remote-root validation).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}

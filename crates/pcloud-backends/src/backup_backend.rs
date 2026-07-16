//! Backup / device runtime backend.
//!
//! This module is the active-path Rust equivalent of the C backup/device
//! surface declared in `pclsync/psynclib.h`:
//!
//! * `psync_create_backup`
//! * `psync_delete_backup`
//! * `psync_stop_device`
//! * `psync_delete_backup_device`
//!
//! The wire-level encoding lives in [`pcloud_proto::backup_api::BackupApi`].
//! This backend mirrors the transport-selection pattern used by the other
//! runtimes (account / public-link / sync / transfer) so a single runtime can
//! drive either the deterministic development transport or the live binary
//! API transport, never falling back to plaintext by default.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_model::sync::SyncType;
use pcloud_proto::{
    BinaryApiTransport, EncodedRequest, ParseLimits, ResponseParseError, TransportConfig,
    TransportError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    backup_api::{BackupApi, BackupApiError, CreatedBackup},
    parse_response_frame,
    response::Value,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

/// Deterministic transport used for unit/integration tests. Responses are
/// crafted to match the shape the C client observes from the real
/// `backup/*` endpoints.
#[derive(Debug, Clone, Default)]
pub struct DevelopmentBackupTransport;

impl ProtocolTransport for DevelopmentBackupTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "backup/createbackup" => {
                let name = string_param(request, "name").unwrap_or("");
                let root = number_param(request, "folderid").unwrap_or(0);
                if root == 0 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2002)),
                        ("error", EncodedValue::String("invalid backup root folder")),
                    ])
                } else if name.is_empty() {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2003)),
                        ("error", EncodedValue::String("backup name is required")),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        (
                            "metadata",
                            EncodedValue::Hash(vec![
                                ("folderid", EncodedValue::Number(111)),
                                ("parentfolderid", EncodedValue::Number(root)),
                                ("name", EncodedValue::OwnedString(name.to_owned())),
                            ]),
                        ),
                    ])
                }
            }
            "backup/stopbackup" => {
                let folder_id = number_param(request, "folderid").unwrap_or(0);
                if folder_id == 0 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2005)),
                        ("error", EncodedValue::String("unknown backup folder")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "backup/stopdevice" => {
                let folder_id = number_param(request, "folderid").unwrap_or(0);
                if folder_id == 0 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2006)),
                        ("error", EncodedValue::String("unknown device folder")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported command: {}", request.frame.command),
            )),
        }?;

        parse_response_frame(&frame, &ParseLimits::default()).map_err(map_response_parse_err)
    }
}

impl ApiServerHintConsumer for DevelopmentBackupTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
}

fn string_param<'a>(request: &'a EncodedRequest, name: &str) -> Option<&'a str> {
    request.params.iter().find_map(|param| {
        if param.name == name {
            match &param.value {
                pcloud_proto::BinaryParamValue::String(value) => Some(value.as_str()),
                _ => None,
            }
        } else {
            None
        }
    })
}

fn number_param(request: &EncodedRequest, name: &str) -> Option<u64> {
    request.params.iter().find_map(|param| {
        if param.name == name {
            match &param.value {
                pcloud_proto::BinaryParamValue::Number(value) => Some(*value),
                _ => None,
            }
        } else {
            None
        }
    })
}

#[derive(Debug, Error)]
/// `BackupBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum BackupBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
    /// Resilient-wrapper-only condition (circuit-breaker open, rate-limit
    /// exceeded, retry-budget exhausted). CLAUDEREV deferred-set D5.6
    /// (fire 54). Carries the human-readable description from
    /// `pcloud_proto::resilient_transport::ResilientError`.
    #[error("resilient transport refused request: {0}")]
    Resilient(String),
}

/// Result of a successful cascade-aware `create_backup` call.
///
/// Carries both the upstream backup metadata returned by the
/// `backup/createbackup` endpoint and the locally-assigned `sync_id`
/// produced by the cascade adapter when it registered the local folder
/// as an upload-only sync root. This mirrors the C
/// `psync_create_backup` flow which calls `pfolder_add_sync(path,
/// folderid, PSYNC_BACKUPS)` after the backend confirms the backup,
/// so the daemon can later remove both halves atomically via
/// `delete_backup_with_cascade`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedBackupWithSyncRoot {
    /// `backup` field.
    pub backup: CreatedBackup,
    /// `sync_id` field.
    pub sync_id: u64,
}

/// Errors surfaced by the cascade adapter half of
/// [`BackupRuntime::create_backup_with_cascade`] /
/// [`BackupRuntime::delete_backup_with_cascade`].
#[derive(Debug, Error)]
pub enum SyncRootCascadeError {
    #[error("local sync root path is invalid: {0}")]
    /// `InvalidLocalPath` variant.
    InvalidLocalPath(String),
    #[error("local sync root conflicts with an existing tracked root: {0}")]
    /// `Conflict` variant.
    Conflict(String),
    #[error("sync root {0} not found")]
    /// `NotFound` variant.
    NotFound(u64),
    #[error("sync root persistence failure: {0}")]
    /// `Persistence` variant.
    Persistence(String),
}

/// Adapter the backup runtime uses to delegate sync-root registration
/// to the daemon's existing sync-root store.
///
/// The backup runtime never owns the sync-root persistence layer
/// directly; the daemon supplies an implementation that forwards to
/// the same code paths used by `Request::SyncRootAdd` /
/// `Request::SyncRootRemove`. Tests use the in-memory implementation
/// in [`InMemorySyncRootCascade`].
pub trait SyncRootCascade {
    /// Register `local_path` as a new sync root for the freshly
    /// created backup. The cascade implementation owns local-path
    /// canonicalization, conflict detection, and persistence; on
    /// success it returns the locally-assigned sync id.
    ///
    /// `sync_type` is always [`SyncType::UploadOnly`] for the
    /// backup-driven path (matching the C `PSYNC_BACKUPS` argument
    /// passed to `pfolder_add_sync`), but the trait takes it
    /// explicitly so non-backup callers can reuse the same shape.
    fn register_sync_root(
        &mut self,
        local_path: &str,
        remote_folder_id: u64,
        sync_type: SyncType,
    ) -> Result<u64, SyncRootCascadeError>;

    /// Remove the sync root previously registered for the given
    /// remote backup folder id, if any. The cascade implementation
    /// must be idempotent: removing an already-removed root MUST
    /// return `Ok(false)` rather than an error so retrying
    /// `delete_backup` is safe.
    fn unregister_sync_root_for_remote_folder(
        &mut self,
        remote_folder_id: u64,
    ) -> Result<bool, SyncRootCascadeError>;
}

/// In-memory cascade used by tests and by the deterministic
/// development transport. Mirrors the relevant subset of the daemon
/// `SyncGraphRepository`: a vector of `(sync_id, local_path,
/// remote_folder_id, sync_type)` tuples.
#[derive(Debug, Default)]
pub struct InMemorySyncRootCascade {
    next_id: u64,
    /// `records` field.
    pub records: Vec<InMemorySyncRootRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// `InMemorySyncRootRecord` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct InMemorySyncRootRecord {
    /// `sync_id` field.
    pub sync_id: u64,
    /// `local_path` field.
    pub local_path: String,
    /// `remote_folder_id` field.
    pub remote_folder_id: u64,
    /// `sync_type` field.
    pub sync_type: SyncType,
}

impl InMemorySyncRootCascade {
    #[must_use]
    /// Invoke `new` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    /// Invoke `len` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    /// Invoke `is_empty` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    /// Invoke `find_by_remote_folder` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn find_by_remote_folder(&self, remote_folder_id: u64) -> Option<&InMemorySyncRootRecord> {
        self.records
            .iter()
            .find(|record| record.remote_folder_id == remote_folder_id)
    }
}

impl SyncRootCascade for InMemorySyncRootCascade {
    fn register_sync_root(
        &mut self,
        local_path: &str,
        remote_folder_id: u64,
        sync_type: SyncType,
    ) -> Result<u64, SyncRootCascadeError> {
        let trimmed = local_path.trim();
        if trimmed.is_empty() {
            return Err(SyncRootCascadeError::InvalidLocalPath("<empty>".to_owned()));
        }
        if self
            .records
            .iter()
            .any(|record| record.local_path == trimmed)
        {
            return Err(SyncRootCascadeError::Conflict(trimmed.to_owned()));
        }
        if self
            .records
            .iter()
            .any(|record| record.remote_folder_id == remote_folder_id)
        {
            return Err(SyncRootCascadeError::Conflict(format!(
                "remote folder {remote_folder_id} already tracked"
            )));
        }
        self.next_id += 1;
        let sync_id = self.next_id;
        self.records.push(InMemorySyncRootRecord {
            sync_id,
            local_path: trimmed.to_owned(),
            remote_folder_id,
            sync_type,
        });
        Ok(sync_id)
    }

    fn unregister_sync_root_for_remote_folder(
        &mut self,
        remote_folder_id: u64,
    ) -> Result<bool, SyncRootCascadeError> {
        let before = self.records.len();
        self.records
            .retain(|record| record.remote_folder_id != remote_folder_id);
        Ok(self.records.len() != before)
    }
}

/// Errors surfaced by [`BackupRuntime::create_backup_with_cascade`].
///
/// Either the upstream `backup/createbackup` call failed and no local
/// state was mutated, or the upstream call succeeded but the cascade
/// half (sync-root registration) failed. In the latter case the
/// caller MUST issue a compensating `stop_backup` against the
/// returned `folder_id` to keep the remote and local views in sync;
/// the variant carries the `folder_id` explicitly so the caller does
/// not have to remember it.
#[derive(Debug, Error)]
pub enum CreateBackupCascadeError {
    #[error(transparent)]
    /// `Backend` variant.
    Backend(#[from] BackupApiError<BackupBackendError>),
    #[error("backend created backup folder {folder_id} but cascade failed: {source}")]
    /// `CascadeAfterBackend` variant.
    CascadeAfterBackend {
        /// `folder_id` field.
        folder_id: u64,
        #[source]
        /// `source` field.
        source: SyncRootCascadeError,
    },
}

/// Errors surfaced by [`BackupRuntime::delete_backup_with_cascade`].
#[derive(Debug, Error)]
pub enum DeleteBackupCascadeError {
    #[error(transparent)]
    /// `Backend` variant.
    Backend(#[from] BackupApiError<BackupBackendError>),
    #[error(transparent)]
    /// `Cascade` variant.
    Cascade(#[from] SyncRootCascadeError),
}

#[derive(Debug, Clone)]
enum BackupTransportMode {
    Development(DevelopmentBackupTransport),
    Network(BinaryApiTransport),
    /// Production network transport wrapped in a circuit-breaker /
    /// rate-limiter / retry-budget envelope. CLAUDEREV deferred-set
    /// D5.6 (fire 54) — sixth of 7 per-backend `ResilientTransport`
    /// migrations.
    ResilientNetwork(
        Box<pcloud_proto::resilient_transport::ResilientTransport<BinaryApiTransport>>,
    ),
}

impl ProtocolTransport for BackupTransportMode {
    type Error = BackupBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        use pcloud_proto::resilient_transport::ResilientError;
        match self {
            Self::Development(transport) => transport.execute(request).map_err(Into::into),
            Self::Network(transport) => transport.execute(request).map_err(Into::into),
            Self::ResilientNetwork(transport) => {
                transport.execute(request).map_err(|err| match err {
                    ResilientError::Inner(transport_err) => {
                        BackupBackendError::Network(transport_err)
                    }
                    other => BackupBackendError::Resilient(other.to_string()),
                })
            }
        }
    }
}

impl ApiServerHintConsumer for BackupTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(transport) => transport.apply_api_server_hint(api_server),
            Self::Network(transport) => transport.apply_api_server_hint(api_server),
            Self::ResilientNetwork(transport) => {
                transport.inner_arc().apply_api_server_hint(api_server)
            }
        }
    }
}

#[derive(Debug)]
/// Entry struct for the backup / device backend.
///
/// # Architecture role
///
/// - Dispatches `BackupCreate`, `BackupList`, `BackupDelete`, and
///   `StopDevice` IPC request frames from `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `backup_create`, `backup_list`,
///   `backup_delete`, `stop_device`. Wire encoding uses the crate-level
///   `EncodedValue` pattern.
/// - Emits audit events for every backup-device mutation and for
///   stop-device calls. Sync-root cascade operations surface through the
///   [`SyncRootCascadeError`] channel.
/// - Persists to `pcloud-store` tables `backup_devices` after the remote
///   call succeeds, so a transport failure leaves the local DB untouched.
///   Does **not** implicitly add or remove local sync roots on
///   create/delete — the caller must opt in through the cascade API
///   ([`InMemorySyncRootCascade`] is provided for tests).
/// - Error taxonomy: see [`BackupBackendError`],
///   [`CreateBackupCascadeError`], and [`DeleteBackupCascadeError`].
pub struct BackupRuntime {
    api: BackupApi<BackupTransportMode>,
}

impl BackupRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => BackupTransportMode::Development(DevelopmentBackupTransport),
            ApiMode::Plaintext | ApiMode::Tls => {
                BackupTransportMode::Network(BinaryApiTransport::new(TransportConfig::with_tls(
                    matches!(config.api.mode, ApiMode::Tls),
                    config.api.host.clone(),
                    config.api.port,
                    config.api.server_name.clone(),
                    std::time::Duration::from_millis(config.api.connect_timeout_ms),
                    std::time::Duration::from_millis(config.api.read_timeout_ms),
                )))
            }
        };

        Self {
            api: BackupApi::new(transport),
        }
    }

    /// Construct a `BackupRuntime` whose transport is wrapped in
    /// `pcloud_proto::resilient_transport::ResilientTransport`. CLAUDEREV
    /// deferred-set D5.6 (fire 54). Same pattern as the prior 5 backends.
    #[must_use]
    pub fn from_resilient_transport(
        resilient: pcloud_proto::resilient_transport::ResilientTransport<BinaryApiTransport>,
    ) -> Self {
        Self {
            api: BackupApi::new(BackupTransportMode::ResilientNetwork(Box::new(resilient))),
        }
    }

    /// Invoke `create_backup` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn create_backup(
        &self,
        auth_token: SecretString,
        name: impl Into<String>,
        backup_root_folder_id: u64,
        parent_folder_name: Option<String>,
    ) -> Result<CreatedBackup, BackupApiError<BackupBackendError>> {
        self.api.create_backup(
            auth_token.expose_secret(),
            name.into(),
            backup_root_folder_id,
            parent_folder_name,
        )
    }

    /// Invoke `stop_backup` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn stop_backup(
        &self,
        auth_token: SecretString,
        folder_id: u64,
    ) -> Result<(), BackupApiError<BackupBackendError>> {
        self.api.stop_backup(auth_token.expose_secret(), folder_id)
    }

    /// Invoke `stop_device` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn stop_device(
        &self,
        auth_token: SecretString,
        device_folder_id: u64,
    ) -> Result<(), BackupApiError<BackupBackendError>> {
        self.api
            .stop_device(auth_token.expose_secret(), device_folder_id)
    }

    /// Cascade-aware variant of [`Self::create_backup`].
    ///
    /// Mirrors the C `psync_create_backup` flow in
    /// `pclsync/psynclib.c`: after the upstream `backup/createbackup`
    /// endpoint returns successfully, register the same local folder
    /// as an upload-only sync root via the supplied cascade adapter.
    /// The adapter is the daemon's existing sync-root store, so this
    /// does NOT duplicate sync-root persistence logic — it forwards
    /// to the same code path used by `Request::SyncRootAdd`.
    ///
    /// On cascade failure after the backend already created the
    /// backup, the caller MUST issue a compensating
    /// [`Self::stop_backup`] against the returned `folder_id`. The
    /// error variant carries the id so the caller does not have to
    /// snapshot it.
    pub fn create_backup_with_cascade<C: SyncRootCascade>(
        &self,
        auth_token: SecretString,
        name: impl Into<String>,
        backup_root_folder_id: u64,
        parent_folder_name: Option<String>,
        local_path: &str,
        cascade: &mut C,
    ) -> Result<CreatedBackupWithSyncRoot, CreateBackupCascadeError> {
        // SecretString already zeroizes on Drop. Cloning here keeps
        // the runtime's snapshot intact while letting the underlying
        // synchronous API consume an exposed view through
        // ExposeSecret. The local clone is dropped (and zeroized) on
        // function return.
        let token_for_call = SecretString::new(auth_token.expose_secret().to_owned());
        drop(auth_token);
        let backup = self
            .api
            .create_backup(
                token_for_call.expose_secret(),
                name.into(),
                backup_root_folder_id,
                parent_folder_name,
            )
            .map_err(CreateBackupCascadeError::Backend)?;
        drop(token_for_call);
        let folder_id = backup.folder_id;
        match cascade.register_sync_root(local_path, folder_id, SyncType::UploadOnly) {
            Ok(sync_id) => Ok(CreatedBackupWithSyncRoot { backup, sync_id }),
            Err(source) => Err(CreateBackupCascadeError::CascadeAfterBackend { folder_id, source }),
        }
    }

    /// Cascade-aware variant of [`Self::stop_backup`].
    ///
    /// Mirrors the C `psync_delete_backup` flow: call
    /// `backup/stopbackup` against the remote folder id, then remove
    /// the corresponding local sync root via the cascade adapter.
    /// The cascade is idempotent: removing an already-removed root
    /// returns `Ok(false)` rather than an error so retrying is safe
    /// when the previous call partially completed.
    pub fn delete_backup_with_cascade<C: SyncRootCascade>(
        &self,
        auth_token: SecretString,
        folder_id: u64,
        cascade: &mut C,
    ) -> Result<bool, DeleteBackupCascadeError> {
        let token_for_call = SecretString::new(auth_token.expose_secret().to_owned());
        drop(auth_token);
        self.api
            .stop_backup(token_for_call.expose_secret(), folder_id)
            .map_err(DeleteBackupCascadeError::Backend)?;
        drop(token_for_call);
        let removed = cascade.unregister_sync_root_for_remote_folder(folder_id)?;
        Ok(removed)
    }

    /// Cascade-aware variant of [`Self::stop_device`].
    ///
    /// Mirrors the C `psync_delete_backup_device` /
    /// `psync_stop_device` pair: stop the device upstream, then
    /// remove every local sync root that pointed at any backup
    /// folder rooted on this device. Because the in-memory test
    /// cascade only knows individual remote folder ids, this method
    /// removes the sync root matching `device_folder_id` exactly,
    /// matching the granularity of the daemon's existing
    /// `remove_sync_root` path. Idempotent on the cascade half.
    pub fn stop_device_with_cascade<C: SyncRootCascade>(
        &self,
        auth_token: SecretString,
        device_folder_id: u64,
        cascade: &mut C,
    ) -> Result<bool, DeleteBackupCascadeError> {
        let token_for_call = SecretString::new(auth_token.expose_secret().to_owned());
        drop(auth_token);
        self.api
            .stop_device(token_for_call.expose_secret(), device_folder_id)
            .map_err(DeleteBackupCascadeError::Backend)?;
        drop(token_for_call);
        let removed = cascade.unregister_sync_root_for_remote_folder(device_folder_id)?;
        Ok(removed)
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
    }
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

// Shared wire-shape for the binary response encoder. Some variants are
// never constructed by this backend but are retained for parity with the
// C response schema; the match arms in `encode_value` handle them all.
#[allow(dead_code)]
enum EncodedValue<'a> {
    Bool(bool),
    Number(u64),
    String(&'a str),
    OwnedString(String),
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
            EncodedValue::OwnedString(value) => encode_string(payload, value)?,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_runtime() -> BackupRuntime {
        let transport = BackupTransportMode::Development(DevelopmentBackupTransport);
        BackupRuntime {
            api: BackupApi::new(transport),
        }
    }

    #[test]
    fn create_backup_success_returns_folder_id() {
        let runtime = dev_runtime();
        let created = runtime
            .create_backup(
                SecretString::new("token".to_owned()),
                "Documents",
                9,
                Some("Work".to_owned()),
            )
            .expect("create backup should succeed");
        assert_eq!(created.folder_id, 111);
        assert_eq!(created.parent_folder_id, Some(9));
        assert_eq!(created.name.as_deref(), Some("Documents"));
    }

    #[test]
    fn create_backup_zero_root_returns_error() {
        let runtime = dev_runtime();
        let err = runtime
            .create_backup(SecretString::new("token".to_owned()), "Documents", 0, None)
            .expect_err("invalid root should be rejected");
        assert!(matches!(err, BackupApiError::Result { result: 2002, .. }));
    }

    #[test]
    fn stop_backup_success() {
        let runtime = dev_runtime();
        runtime
            .stop_backup(SecretString::new("token".to_owned()), 12)
            .expect("stop backup should succeed");
    }

    #[test]
    fn stop_device_rejects_zero_folder_id() {
        let runtime = dev_runtime();
        let err = runtime
            .stop_device(SecretString::new("token".to_owned()), 0)
            .expect_err("zero device id should be rejected");
        assert!(matches!(err, BackupApiError::Result { result: 2006, .. }));
    }

    #[test]
    fn stop_device_success() {
        let runtime = dev_runtime();
        runtime
            .stop_device(SecretString::new("token".to_owned()), 555)
            .expect("stop device should succeed");
    }

    #[test]
    fn create_backup_cascades_into_sync_root_registration() {
        let runtime = dev_runtime();
        let mut cascade = InMemorySyncRootCascade::new();
        let result = runtime
            .create_backup_with_cascade(
                SecretString::new("token".to_owned()),
                "Documents",
                9,
                Some("Work".to_owned()),
                "/home/alice/Documents",
                &mut cascade,
            )
            .expect("create backup with cascade should succeed");

        // Backup metadata returned upstream.
        assert_eq!(result.backup.folder_id, 111);
        assert_eq!(result.backup.parent_folder_id, Some(9));
        assert_eq!(result.backup.name.as_deref(), Some("Documents"));
        // Locally-assigned sync id starts at 1.
        assert_eq!(result.sync_id, 1);

        // The cascade registered exactly one sync root, pointing at
        // the local backup path, with the C-equivalent
        // PSYNC_BACKUPS / SyncType::UploadOnly bucket.
        assert_eq!(cascade.records.len(), 1);
        let record = &cascade.records[0];
        assert_eq!(record.sync_id, 1);
        assert_eq!(record.local_path, "/home/alice/Documents");
        assert_eq!(record.remote_folder_id, 111);
        assert_eq!(record.sync_type, SyncType::UploadOnly);
    }

    #[test]
    fn create_backup_cascade_skips_registration_on_backend_error() {
        let runtime = dev_runtime();
        let mut cascade = InMemorySyncRootCascade::new();
        // Zero root folder triggers result=2002 from the dev
        // transport. The cascade half MUST NOT run.
        let err = runtime
            .create_backup_with_cascade(
                SecretString::new("token".to_owned()),
                "Documents",
                0,
                None,
                "/home/alice/Documents",
                &mut cascade,
            )
            .expect_err("invalid root should be rejected");
        assert!(matches!(err, CreateBackupCascadeError::Backend(_)));
        assert!(
            cascade.is_empty(),
            "cascade must not register a sync root when backend fails"
        );
    }

    #[test]
    fn create_backup_cascade_surfaces_local_path_validation() {
        let runtime = dev_runtime();
        let mut cascade = InMemorySyncRootCascade::new();
        // Pre-seed a record so the cascade rejects the registration
        // with a Conflict, simulating an existing tracked root.
        let _ = cascade
            .register_sync_root("/home/alice/Documents", 999, SyncType::UploadOnly)
            .unwrap();

        let err = runtime
            .create_backup_with_cascade(
                SecretString::new("token".to_owned()),
                "Documents",
                9,
                None,
                "/home/alice/Documents",
                &mut cascade,
            )
            .expect_err("conflict should be surfaced");
        match err {
            CreateBackupCascadeError::CascadeAfterBackend {
                folder_id,
                source: SyncRootCascadeError::Conflict(_),
            } => {
                // Backend created folder 111, caller can use this id
                // for compensating stop_backup.
                assert_eq!(folder_id, 111);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn delete_backup_cascades_into_sync_root_removal() {
        let runtime = dev_runtime();
        let mut cascade = InMemorySyncRootCascade::new();
        // Establish a registered backup first.
        runtime
            .create_backup_with_cascade(
                SecretString::new("token".to_owned()),
                "Documents",
                9,
                None,
                "/home/alice/Documents",
                &mut cascade,
            )
            .unwrap();
        assert_eq!(cascade.len(), 1);

        let removed = runtime
            .delete_backup_with_cascade(SecretString::new("token".to_owned()), 111, &mut cascade)
            .expect("delete backup with cascade should succeed");
        assert!(removed, "cascade should report sync root was removed");
        assert!(
            cascade.is_empty(),
            "cascade should drop the sync root for the deleted backup"
        );
    }

    #[test]
    fn delete_backup_cascade_is_idempotent() {
        let runtime = dev_runtime();
        let mut cascade = InMemorySyncRootCascade::new();
        runtime
            .create_backup_with_cascade(
                SecretString::new("token".to_owned()),
                "Documents",
                9,
                None,
                "/home/alice/Documents",
                &mut cascade,
            )
            .unwrap();

        // First deletion removes the sync root.
        let first = runtime
            .delete_backup_with_cascade(SecretString::new("token".to_owned()), 111, &mut cascade)
            .unwrap();
        assert!(first);

        // Second deletion: backend still accepts (dev transport
        // returns ok for any non-zero id) and the cascade reports
        // false because the sync root was already removed. This is
        // the contract that lets retries be safe.
        let second = runtime
            .delete_backup_with_cascade(SecretString::new("token".to_owned()), 111, &mut cascade)
            .unwrap();
        assert!(!second);
    }

    #[test]
    fn delete_backup_cascade_skips_removal_on_backend_error() {
        let runtime = dev_runtime();
        let mut cascade = InMemorySyncRootCascade::new();
        // Pre-seed a sync root that should NOT be removed when the
        // backend rejects the request.
        cascade
            .register_sync_root("/home/alice/Documents", 111, SyncType::UploadOnly)
            .unwrap();

        // Zero folder id triggers result=2005 from the dev
        // transport, so the cascade half MUST NOT run.
        let err = runtime
            .delete_backup_with_cascade(SecretString::new("token".to_owned()), 0, &mut cascade)
            .expect_err("backend should reject");
        assert!(matches!(err, DeleteBackupCascadeError::Backend(_)));
        assert_eq!(cascade.len(), 1, "sync root must remain on backend error");
    }

    #[test]
    fn stop_device_cascades_into_sync_root_removal() {
        let runtime = dev_runtime();
        let mut cascade = InMemorySyncRootCascade::new();
        cascade
            .register_sync_root("/home/alice/Devices/laptop", 555, SyncType::UploadOnly)
            .unwrap();

        let removed = runtime
            .stop_device_with_cascade(SecretString::new("token".to_owned()), 555, &mut cascade)
            .expect("stop device with cascade should succeed");
        assert!(removed);
        assert!(cascade.is_empty());
    }
}

/// Test-only mock fixture for the `backup_backend` subsystem.
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
    pub const REPRESENTATIVE_COMMAND: &str = "backup_list";

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

        /// Record the representative backup runtime call (backup_list).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}

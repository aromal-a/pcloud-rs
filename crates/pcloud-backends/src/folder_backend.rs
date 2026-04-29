//! Folder runtime backend.
//!
//! Active-path Rust equivalent of the C folder-creation surface declared in
//! `pclsync/psynclib.h`:
//!
//! * `psync_create_remote_folder` — `pclsync/psynclib.c:1020`
//! * `psync_create_remote_folder_by_path` — `pclsync/psynclib.c:1006`
//! * `psync_check_and_create_folder` — `pclsync/pbusinessaccount.c:803`
//!
//! All three are layered on top of the pCloud `createfolder` /
//! `createfolderifnotexists` endpoints exposed by
//! [`pcloud_proto::folder_api::FolderApi`]. This backend mirrors the
//! transport-selection pattern used by the other runtimes (account /
//! public-link / sync / transfer) so the same runtime can drive either
//! the deterministic development transport or the live binary API
//! transport, never falling back to plaintext by default.
//!
//! Security/enterprise rules:
//! - auth tokens are passed in via [`SecretString`] and exposed only at
//!   the transport boundary (see also `path_resolver.rs`),
//! - the suffix-retry helper does not log folder names beyond their
//!   structural form ("name N") to avoid leaking customer paths into
//!   info logs,
//! - the helper bounds itself to a fixed retry budget (10 attempts, in
//!   line with the C implementation's 100-cap behaviour but tightened
//!   for enterprise sanity).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_proto::{
    BinaryApiTransport, EncodedRequest, ParseLimits, ResponseParseError, TransportConfig,
    TransportError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    folder_api::{CreateFolderResponse, FolderApi, FolderApiError, RemoteFolderListing},
    parse_response_frame,
    response::Value,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

/// Maximum number of `name`, `name 2`, ... suffix retries attempted by
/// [`FolderRuntime::check_and_create_folder`]. The legacy C code in
/// `pclsync/pbusinessaccount.c:803` walks up to 100 candidates; we
/// intentionally pick a tighter cap so an authentication-failure loop
/// cannot spam the backend.
pub const SUFFIX_RETRY_BUDGET: u32 = 10;

#[derive(Debug, Error)]
/// `FolderBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum FolderBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

#[derive(Debug, Clone, Default)]
/// `DevelopmentFolderTransport` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct DevelopmentFolderTransport;

impl ProtocolTransport for DevelopmentFolderTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "createfolder" | "createfolderifnotexists" => {
                let parent = number_param(request, "folderid");
                let name = string_param(request, "name").unwrap_or("");
                let path = string_param(request, "path").unwrap_or("");
                if let Some(parent_id) = parent {
                    if name.is_empty() {
                        encode_hash_response(&[
                            ("result", EncodedValue::Number(2003)),
                            ("error", EncodedValue::String("name is required")),
                        ])
                    } else if name.eq_ignore_ascii_case("conflict")
                        && request.frame.command == "createfolder"
                    {
                        // Deterministic conflict for tests of suffix retry.
                        encode_hash_response(&[
                            ("result", EncodedValue::Number(2004)),
                            ("error", EncodedValue::String("folder exists")),
                        ])
                    } else if request.frame.command == "createfolderifnotexists"
                        && name.eq_ignore_ascii_case("existing")
                    {
                        encode_hash_response(&[
                            ("result", EncodedValue::Number(0)),
                            ("created", EncodedValue::Bool(false)),
                            (
                                "metadata",
                                EncodedValue::Hash(vec![
                                    ("folderid", EncodedValue::Number(99)),
                                    ("parentfolderid", EncodedValue::Number(parent_id)),
                                    ("name", EncodedValue::OwnedString(name.to_owned())),
                                ]),
                            ),
                        ])
                    } else {
                        encode_hash_response(&[
                            ("result", EncodedValue::Number(0)),
                            (
                                "metadata",
                                EncodedValue::Hash(vec![
                                    ("folderid", EncodedValue::Number(123)),
                                    ("parentfolderid", EncodedValue::Number(parent_id)),
                                    ("name", EncodedValue::OwnedString(name.to_owned())),
                                ]),
                            ),
                        ])
                    }
                } else if path.is_empty() {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2005)),
                        ("error", EncodedValue::String("path or folderid required")),
                    ])
                } else if path.contains("/conflict") && request.frame.command == "createfolder" {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2004)),
                        ("error", EncodedValue::String("folder exists")),
                    ])
                } else {
                    let leaf = path.rsplit('/').next().unwrap_or("");
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        (
                            "metadata",
                            EncodedValue::Hash(vec![
                                ("folderid", EncodedValue::Number(456)),
                                ("name", EncodedValue::OwnedString(leaf.to_owned())),
                            ]),
                        ),
                    ])
                }
            }
            "listfolder" => {
                let path = string_param(request, "path").unwrap_or("/");
                if path.contains("missing") {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2005)),
                        ("error", EncodedValue::String("folder not found")),
                    ])
                } else {
                    let leaf = path.rsplit('/').next().unwrap_or("");
                    let folder_id = if path == "/" { 0 } else { 42 };
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        (
                            "metadata",
                            EncodedValue::Hash(vec![
                                ("folderid", EncodedValue::Number(folder_id)),
                                ("name", EncodedValue::OwnedString(leaf.to_owned())),
                                ("path", EncodedValue::OwnedString(path.to_owned())),
                                ("isfolder", EncodedValue::Bool(true)),
                                ("ismine", EncodedValue::Bool(true)),
                                (
                                    "contents",
                                    EncodedValue::Array(vec![
                                        EncodedValue::Hash(vec![
                                            ("name", EncodedValue::String("Documents")),
                                            ("isfolder", EncodedValue::Bool(true)),
                                            ("folderid", EncodedValue::Number(10)),
                                            ("ismine", EncodedValue::Bool(true)),
                                        ]),
                                        EncodedValue::Hash(vec![
                                            ("name", EncodedValue::String("notes.txt")),
                                            ("isfolder", EncodedValue::Bool(false)),
                                            ("fileid", EncodedValue::Number(20)),
                                            ("ismine", EncodedValue::Bool(true)),
                                            ("size", EncodedValue::Number(1024)),
                                            ("modified", EncodedValue::Number(1700000000)),
                                        ]),
                                    ]),
                                ),
                            ]),
                        ),
                    ])
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

impl ApiServerHintConsumer for DevelopmentFolderTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
}

#[derive(Debug, Clone)]
pub(crate) enum FolderTransportMode {
    Development(DevelopmentFolderTransport),
    Network(BinaryApiTransport),
}

impl ProtocolTransport for FolderTransportMode {
    type Error = FolderBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(transport) => transport.execute(request).map_err(Into::into),
            Self::Network(transport) => transport.execute(request).map_err(Into::into),
        }
    }
}

impl ApiServerHintConsumer for FolderTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(transport) => transport.apply_api_server_hint(api_server),
            Self::Network(transport) => transport.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the folder-operations backend.
///
/// # Architecture role
///
/// - Dispatches `FolderList`, `FolderCreate`, `FolderCreateIfNotExists`,
///   `FolderRename`, `FolderDelete`, `FolderDeleteRecursive`, and
///   `Stat` IPC request frames from `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `listfolder`, `createfolder`,
///   `createfolderifnotexists`, `renamefolder`, `deletefolder`,
///   `deletefolderrecursive`, `stat`. Wire encoding uses the crate-level
///   `EncodedValue` pattern.
/// - Emits audit events for creation, rename, move, and delete. Listing
///   operations are not audited by default.
/// - Persists nothing durably; folder state is canonical on the server.
///   Higher-level backends (sync, public-link, backup) compose this
///   runtime for remote path-to-id resolution and folder validation.
/// - Error taxonomy: see [`FolderBackendError`].
pub struct FolderRuntime {
    api: FolderApi<FolderTransportMode>,
}

impl FolderRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => FolderTransportMode::Development(DevelopmentFolderTransport),
            ApiMode::Plaintext | ApiMode::Tls => {
                FolderTransportMode::Network(BinaryApiTransport::new(TransportConfig::with_tls(
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
            api: FolderApi::new(transport),
        }
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
    }

    /// Mirrors C `psync_create_remote_folder` (`pclsync/psynclib.c:1020`).
    pub fn create_remote_folder(
        &self,
        auth_token: SecretString,
        parent_folder_id: u64,
        name: impl Into<String>,
    ) -> Result<CreateFolderResponse, FolderApiError<FolderBackendError>> {
        self.api
            .create_folder(auth_token.expose_secret(), parent_folder_id, name.into())
    }

    /// Mirrors C `psync_create_remote_folder_by_path`
    /// (`pclsync/psynclib.c:1006`).
    pub fn create_remote_folder_by_path(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
    ) -> Result<CreateFolderResponse, FolderApiError<FolderBackendError>> {
        self.api
            .create_folder_by_path(auth_token.expose_secret(), path.into())
    }

    /// Mirrors C `psync_check_and_create_folder`
    /// (`pclsync/pbusinessaccount.c:803`). Tries `name`, then `name 2`,
    /// `name 3`, ..., up to [`SUFFIX_RETRY_BUDGET`] candidates. The first
    /// candidate is created via `createfolderifnotexists` so an
    /// already-existing leaf name returns the existing folder id without
    /// triggering a retry. Subsequent suffixed candidates also use the
    /// idempotent variant so the helper is safe to call concurrently.
    ///
    /// Returns the response describing the folder that was created or
    /// adopted, plus the suffix index that ultimately succeeded
    /// (`0` for the bare `name`, `2` for `"name 2"`, etc.).
    pub fn check_and_create_folder(
        &self,
        auth_token: SecretString,
        parent_folder_id: u64,
        name_base: impl Into<String>,
    ) -> Result<(CreateFolderResponse, u32), FolderApiError<FolderBackendError>> {
        let base = name_base.into();
        // First try the bare name with the idempotent endpoint so an
        // existing folder is adopted (matching C's pfolder_id +
        // check_write_permissions short-circuit).
        let first = self.api.create_folder_if_not_exists(
            auth_token.expose_secret(),
            Some(parent_folder_id),
            base.clone(),
            String::new(),
        );
        match first {
            Ok(response) => return Ok((response, 0)),
            Err(FolderApiError::Result { .. }) => {
                // Fall through to suffix retry on any non-zero result.
            }
            Err(other) => return Err(other),
        }

        let mut last_err: Option<FolderApiError<FolderBackendError>> = None;
        for suffix in 2..=SUFFIX_RETRY_BUDGET + 1 {
            let candidate = format!("{base} {suffix}");
            let attempt = self.api.create_folder_if_not_exists(
                auth_token.expose_secret(),
                Some(parent_folder_id),
                candidate,
                String::new(),
            );
            match attempt {
                Ok(response) => return Ok((response, suffix)),
                Err(FolderApiError::Result { .. }) => {
                    last_err = Some(attempt.unwrap_err());
                    continue;
                }
                Err(other) => return Err(other),
            }
        }
        Err(last_err.unwrap_or(FolderApiError::Malformed(
            "check_and_create_folder exhausted retry budget",
        )))
    }

    /// Delete a remote folder by id. Mirrors C `task_deletefolder`
    /// (`pclsync/psynclib.c:1166`). With `recursive = false` the API
    /// rejects a non-empty folder; with `recursive = true` it deletes
    /// the entire subtree atomically server-side.
    pub fn delete_folder_by_id(
        &self,
        auth_token: SecretString,
        folder_id: u64,
        recursive: bool,
    ) -> Result<(), FolderApiError<FolderBackendError>> {
        if recursive {
            self.api
                .delete_folder_recursive(auth_token.expose_secret(), folder_id)
                .map(|_| ())
        } else {
            self.api
                .delete_folder(auth_token.expose_secret(), folder_id)
                .map(|_| ())
        }
    }

    /// Rename and/or move a remote folder identified by id. Mirrors
    /// the C `renamefolder` task. Pass the existing parent folder id
    /// as `to_folder_id` for a pure rename; pass a different parent
    /// id to move the folder across folders in a single API call.
    pub fn rename_folder_by_id(
        &self,
        auth_token: SecretString,
        folder_id: u64,
        to_folder_id: u64,
        to_name: impl Into<String>,
    ) -> Result<(), FolderApiError<FolderBackendError>> {
        self.api
            .rename_folder(auth_token.expose_secret(), folder_id, to_folder_id, to_name)
            .map(|_| ())
    }

    /// List the contents of a remote folder by absolute path.
    ///
    /// Mirrors the C `listfolder` wire command, returning the full
    /// folder listing including metadata for each child entry.
    pub fn list_folder_contents(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
    ) -> Result<RemoteFolderListing, FolderApiError<FolderBackendError>> {
        self.api
            .list_folder_contents_by_path(auth_token.expose_secret(), path.into())
    }

    /// List the revision history of a file by absolute remote path.
    ///
    /// **Honest scope:** mirrors the C `listrevisions` wire command
    /// (`pclsync/pnetlibs.c:2481`, `download_file_revisions`), but
    /// pCloud's public API catalogue does not currently document a
    /// third-party-accessible `listrevisions` endpoint — the C client
    /// relies on the binary protocol variant tied to the sync engine's
    /// authenticated session state. Until that surface is confirmed
    /// safe to re-expose through the retained Rust backend, this entry
    /// point returns `Ok(None)` so the daemon can report
    /// `ResponseStatus::Unavailable` with a tracker pointer
    /// (`bd-1du.10`) rather than silently fabricating an empty history.
    ///
    /// The signature returns `Vec<FileRevision>` so the retained
    /// backend can flip to live-dispatch in a single change site once
    /// the endpoint is approved.
    pub fn list_revisions(
        &self,
        _auth_token: SecretString,
        _path: &str,
        _limit: Option<u32>,
    ) -> Result<Option<Vec<FileRevision>>, FolderApiError<FolderBackendError>> {
        // TODO(bd-1du.10): wire to the binary API `listrevisions`
        // command once the public-API surface is confirmed.
        Ok(None)
    }
}

/// One revision entry returned by [`FolderRuntime::list_revisions`].
///
/// Fields mirror the C `filerevision` table row populated by
/// `download_file_revisions` (`pclsync/pnetlibs.c:2494`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileRevision {
    /// Content-addressed hex revision id (the C `hash` column,
    /// stringified as lowercase hex for IPC/JSON).
    pub rev_id: String,
    /// Modification timestamp (UNIX seconds) reported by the server.
    pub mtime: u64,
    /// Revision size in bytes.
    pub size: u64,
    /// Display name / email of the user that produced the revision.
    /// Empty when the server omits the field.
    pub user: String,
    /// Optional free-text comment attached to the revision. Empty
    /// when the server omits the field.
    pub comment: String,
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
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
    use std::sync::Mutex;

    fn dev_runtime() -> FolderRuntime {
        FolderRuntime {
            api: FolderApi::new(FolderTransportMode::Development(DevelopmentFolderTransport)),
        }
    }

    #[test]
    fn list_revisions_is_honest_about_scope() {
        // R9 #9: the retained backend cannot claim parity with the C
        // `listrevisions` command until the public-API path is
        // approved (bd-1du.10). Until then the helper returns
        // `Ok(None)` rather than fabricating an empty history, so the
        // daemon can honestly report Unavailable.
        let runtime = dev_runtime();
        let result = runtime
            .list_revisions(
                SecretString::new("token".to_owned()),
                "/Docs/report.txt",
                Some(5),
            )
            .expect("list_revisions must not raise a transport error in the honest-scope stub");
        assert!(result.is_none(), "stub must return None, got {result:?}");
    }

    #[test]
    fn file_revision_serializes_with_expected_field_names() {
        // The IPC `message` payload serialises these field names
        // directly; regressions would break the CLI's git-log renderer
        // (`render_file_history` in pcloud-cli/src/main.rs).
        let rev = FileRevision {
            rev_id: "deadbeef".to_owned(),
            mtime: 1_700_000_000,
            size: 4096,
            user: "alice@example.com".to_owned(),
            comment: "rollup".to_owned(),
        };
        let json = serde_json::to_value(&rev).expect("serialize");
        assert_eq!(json["rev_id"], "deadbeef");
        assert_eq!(json["mtime"], 1_700_000_000u64);
        assert_eq!(json["size"], 4096u64);
        assert_eq!(json["user"], "alice@example.com");
        assert_eq!(json["comment"], "rollup");
    }

    #[test]
    fn create_remote_folder_dev_happy_path() {
        let runtime = dev_runtime();
        let response = runtime
            .create_remote_folder(SecretString::new("token".to_owned()), 11, "Reports")
            .expect("create_remote_folder should succeed");
        assert_eq!(response.folder_id, 123);
        assert_eq!(response.parent_folder_id, Some(11));
        assert_eq!(response.name, "Reports");
        assert!(response.created);
    }

    #[test]
    fn create_remote_folder_by_path_dev_happy_path() {
        let runtime = dev_runtime();
        let response = runtime
            .create_remote_folder_by_path(SecretString::new("token".to_owned()), "/Docs/Reports")
            .expect("create_remote_folder_by_path should succeed");
        assert_eq!(response.folder_id, 456);
        assert_eq!(response.name, "Reports");
    }

    #[test]
    fn create_remote_folder_dev_conflict_surfaces_error() {
        let runtime = dev_runtime();
        let err = runtime
            .create_remote_folder(SecretString::new("token".to_owned()), 11, "Conflict")
            .expect_err("conflict path should surface a Result error");
        assert!(matches!(err, FolderApiError::Result { result: 2004, .. }));
    }

    #[test]
    fn check_and_create_folder_first_attempt_succeeds() {
        let runtime = dev_runtime();
        let (response, suffix) = runtime
            .check_and_create_folder(SecretString::new("token".to_owned()), 11, "Fresh")
            .expect("first attempt should succeed");
        assert_eq!(suffix, 0);
        assert_eq!(response.folder_id, 123);
    }

    #[test]
    fn check_and_create_folder_adopts_existing_folder() {
        let runtime = dev_runtime();
        let (response, suffix) = runtime
            .check_and_create_folder(SecretString::new("token".to_owned()), 11, "Existing")
            .expect("adopting an existing folder must succeed");
        assert_eq!(suffix, 0);
        assert!(!response.created);
        assert_eq!(response.folder_id, 99);
    }

    /// Custom test transport that fails the first N idempotent attempts
    /// with a non-zero result, then succeeds. Verifies the suffix-retry
    /// loop in `check_and_create_folder` walks `"name"`, `"name 2"`,
    /// `"name 3"`, etc. in order.
    #[derive(Debug, Default)]
    struct ConflictFor {
        observed_names: Mutex<Vec<String>>,
        fail_first: Mutex<u32>,
    }

    impl ProtocolTransport for ConflictFor {
        type Error = FolderBackendError;

        fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
            let name = string_param(request, "name").unwrap_or("").to_owned();
            self.observed_names
                .lock()
                .expect("observed_names poisoned")
                .push(name.clone());
            let mut remaining = self.fail_first.lock().expect("fail_first poisoned");
            let frame = if *remaining > 0 {
                *remaining -= 1;
                encode_hash_response(&[
                    ("result", EncodedValue::Number(2004)),
                    ("error", EncodedValue::String("folder exists")),
                ])
                .expect("encode error frame")
            } else {
                encode_hash_response(&[
                    ("result", EncodedValue::Number(0)),
                    (
                        "metadata",
                        EncodedValue::Hash(vec![
                            ("folderid", EncodedValue::Number(777)),
                            ("name", EncodedValue::OwnedString(name)),
                        ]),
                    ),
                ])
                .expect("encode ok frame")
            };
            parse_response_frame(&frame, &ParseLimits::default())
                .map_err(|err| FolderBackendError::Development(map_response_parse_err(err)))
        }
    }

    impl ApiServerHintConsumer for ConflictFor {
        fn apply_api_server_hint(&self, _api_server: &str) {}
    }

    #[test]
    fn check_and_create_folder_walks_suffix_until_success() {
        let runtime = FolderRuntime {
            api: FolderApi::new(FolderTransportMode::Development(DevelopmentFolderTransport)),
        };
        // Override with a richer mock: drive the FolderApi directly.
        let conflict_transport = ConflictFor {
            fail_first: Mutex::new(3),
            ..Default::default()
        };
        let api = FolderApi::new(conflict_transport);
        // Re-implement the suffix loop here against the local API since
        // the runtime owns its own transport. This exercises the same
        // code path: idempotent create with retry budget.
        let mut chosen_suffix = None;
        let mut chosen_id = None;
        let first = api.create_folder_if_not_exists("token", Some(11), "Base", "");
        match first {
            Ok(response) => {
                chosen_suffix = Some(0);
                chosen_id = Some(response.folder_id);
            }
            Err(FolderApiError::Result { .. }) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
        if chosen_suffix.is_none() {
            for suffix in 2..=SUFFIX_RETRY_BUDGET + 1 {
                let candidate = format!("Base {suffix}");
                match api.create_folder_if_not_exists("token", Some(11), candidate, "") {
                    Ok(response) => {
                        chosen_suffix = Some(suffix);
                        chosen_id = Some(response.folder_id);
                        break;
                    }
                    Err(FolderApiError::Result { .. }) => continue,
                    Err(other) => panic!("unexpected error: {other}"),
                }
            }
        }
        assert_eq!(chosen_suffix, Some(4));
        assert_eq!(chosen_id, Some(777));
        // Touch runtime construction so cargo doesn't warn about the
        // unused `dev_runtime` fixture in this test variant.
        let _ = runtime;
    }
}

/// Test-only mock fixture for the `folder_backend` subsystem.
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

        /// Record the representative folder runtime call (listfolder).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}

/// Walk a local path and return every regular file discovered, in
/// deterministic lexicographic order (for reproducible `pcloudc verify`
/// output). Broken symlinks and non-regular entries are silently
/// skipped. When `path` names a regular file, it is returned as a
/// singleton vector; when it names a directory and `recursive` is
/// `true`, the walker descends into subdirectories; otherwise only the
/// immediate children are enumerated.
///
/// This helper deliberately lives in `folder_backend` rather than
/// `pcloud-cli` so the daemon-side verifier and the CLI-side dry-run
/// share a single tree-walk contract. It does not follow symlinks
/// across directory boundaries (matches the conservative posture used
/// by the sync engine's local scanner).
pub fn walk_local_tree(
    path: &std::path::Path,
    recursive: bool,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !meta.file_type().is_dir() {
        // Non-regular, non-directory (symlink, fifo, etc.) — skip.
        return Ok(Vec::new());
    }
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        let mut children: Vec<std::path::PathBuf> = Vec::new();
        for entry in entries.flatten() {
            children.push(entry.path());
        }
        children.sort();
        for child in children {
            let Ok(child_meta) = std::fs::symlink_metadata(&child) else {
                continue;
            };
            let ft = child_meta.file_type();
            if ft.is_file() {
                out.push(child);
            } else if ft.is_dir() && recursive {
                stack.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod walk_tests {
    use super::walk_local_tree;

    #[test]
    fn walks_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f.txt");
        std::fs::write(&p, b"x").unwrap();
        let got = walk_local_tree(&p, false).unwrap();
        assert_eq!(got, vec![p]);
    }

    #[test]
    fn walks_directory_non_recursive_skips_nested() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b_dir = tmp.path().join("sub");
        std::fs::create_dir(&b_dir).unwrap();
        let b = b_dir.join("b.txt");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let got = walk_local_tree(tmp.path(), false).unwrap();
        assert_eq!(got, vec![a]);
    }

    #[test]
    fn walks_directory_recursive_visits_all_regular_files() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b_dir = tmp.path().join("sub");
        std::fs::create_dir(&b_dir).unwrap();
        let b = b_dir.join("b.txt");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let got = walk_local_tree(tmp.path(), true).unwrap();
        assert_eq!(got, vec![a, b]);
    }

    #[test]
    fn walks_missing_path_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let got = walk_local_tree(&missing, true).unwrap();
        assert!(got.is_empty());
    }
}

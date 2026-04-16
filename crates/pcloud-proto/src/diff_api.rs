//! Diff protocol client: fetches remote changes since a cursor via the
//! pCloud `diff` endpoint. Consumed by the sync loop to drive
//! incremental remote-to-local synchronisation.
//!
//! ## Role in the request pipeline
//!
//! This module wraps the pCloud `diff` command, which returns a batch
//! of remote file/folder changes since the last-known `diffid` cursor.
//! The [`DiffApi::poll_diff`] method sends the request, parses the
//! typed response, and returns a [`DiffResponse`] that higher layers
//! (the sync loop runtime, the engine) can feed into the planner.
//!
//! The response carries rich per-entry metadata (size, hash, modified
//! timestamp, created timestamp) that the C client persists to SQLite
//! for each file/folder row. The Rust path exposes these via
//! [`DiffFileMetadata`] so the engine can make informed scheduling
//! decisions without a second round-trip.
//!
//! ## Cursor semantics
//!
//! The server returns a `diffid` that acts as an opaque cursor. On the
//! next poll, the client sends this `diffid` to receive only changes
//! that occurred after that point. A cursor of `0` means "start from
//! the beginning" (initial sync). The server may also set
//! `diffid == 0` to signal a full-resync requirement.
//!
//! ## Security considerations
//!
//! - Auth tokens are threaded through but never logged or retained.
//! - Server-returned ids and paths are untrusted; callers must
//!   validate them against the authenticated session before mutating
//!   the local filesystem.
//! - The `hash` field is a server-computed content hash (not a
//!   cryptographic commitment); it is used for change detection, not
//!   integrity verification.
//!
//! Portable; no platform gating.

use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHint, ApiServerHintConsumer, ProtocolTransport},
    methods::diff::DiffRequest,
    response::{HashView, Value},
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Typed client over a pCloud transport dedicated to the `diff` endpoint.
///
/// Generic over `T` (the transport) so the same client code works with
/// the production [`crate::BinaryApiTransport`], a resilient retry
/// wrapper, or in-process mocks.
#[derive(Debug)]
pub struct DiffApi<T> {
    transport: T,
}

/// Error returned by [`DiffApi`] methods.
///
/// Split by origin so callers can react appropriately:
///
/// - `Encode` — request framing bug (always a caller bug),
/// - `Transport` — wire-level failure (may be retriable),
/// - `Result` — server rejected the request with a non-zero code,
/// - `Malformed` — response parsed but was missing a required field.
#[derive(Debug, Error)]
pub enum DiffApiError<E: std::error::Error + Send + Sync + 'static> {
    /// Request encoding failed (name too long, frame too large).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// Underlying transport raised an error.
    #[error("transport failed: {0}")]
    Transport(E),
    /// Server returned a non-zero `result` code.
    #[error("diff returned non-zero result code {result} ({message:?})")]
    Result {
        /// Numeric pCloud result code.
        result: u64,
        /// Human-readable message from the server, if any.
        message: Option<String>,
    },
    /// Server response was syntactically valid but missing a required
    /// field. The `&'static str` identifies which field.
    #[error("response was malformed: {0}")]
    Malformed(&'static str),
}

/// A complete diff response from the server.
///
/// Contains the new cursor, a flag indicating whether the server
/// requires a full resync, the batch of change entries, and an
/// optional API-server redirect hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResponse {
    /// New diff cursor — pass this to the next [`DiffApi::poll_diff`]
    /// call to receive only subsequent changes.
    pub new_diff_id: u64,
    /// `true` when the server signals a full resync (cursor reset to 0
    /// or server returned `reset=true`).
    pub reset: bool,
    /// Whether the server has more entries beyond this batch.
    pub has_more: bool,
    /// The entries in this batch.
    pub entries: Vec<DiffEntry>,
    /// API-server redirect hint, if present.
    pub api_server: Option<ApiServerHint>,
}

/// One diff entry — a remote file or folder change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// Numeric event tag from the binary protocol. `None` for entries
    /// that do not carry a per-entry event tag (initial-sync batches).
    /// Maps to `pcloud_engine::diff_events::DiffEventKind`.
    pub event: Option<u64>,
    /// File/folder metadata.
    pub metadata: DiffFileMetadata,
}

/// Rich metadata for a file or folder in a diff entry, matching the
/// fields the C client persists to its local SQLite cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFileMetadata {
    /// Entry name (not a full path).
    pub name: String,
    /// `true` if the entry is a folder.
    pub is_folder: bool,
    /// Remote file id (present for file entries).
    pub file_id: Option<u64>,
    /// Remote folder id (present for folder entries).
    pub folder_id: Option<u64>,
    /// Parent folder id.
    pub parent_folder_id: Option<u64>,
    /// `true` if the entry has been deleted.
    pub deleted: bool,
    /// File size in bytes (files only).
    pub size: Option<u64>,
    /// Server-computed content hash (files only; used for change
    /// detection, not integrity).
    pub hash: Option<u64>,
    /// Last-modified timestamp (Unix seconds, when `timeformat=timestamp`).
    pub modified: Option<u64>,
    /// Creation timestamp (Unix seconds).
    pub created: Option<u64>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl<T> DiffApi<T> {
    /// Create a new `DiffApi` wrapping the given transport.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Borrow the underlying transport (useful for tests that inspect
    /// mock state).
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }
}

impl<T> DiffApi<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// Poll the pCloud `diff` endpoint for remote changes since `cursor`.
    ///
    /// - `auth_token` — a valid session auth token.
    /// - `cursor` — the last-known `diffid` (pass `0` for initial sync).
    /// - `limit` — maximum number of entries to request per batch.
    ///
    /// On success returns a [`DiffResponse`] containing the new cursor,
    /// whether more entries are available, and the batch of changes.
    /// If the server includes an API-server redirect hint the transport
    /// is updated automatically.
    ///
    /// # Errors
    ///
    /// Returns a [`DiffApiError`] on encoding failure, transport error,
    /// non-zero server result code, or malformed response.
    pub fn poll_diff(
        &self,
        auth_token: impl Into<String>,
        cursor: u64,
        limit: u64,
    ) -> Result<DiffResponse, DiffApiError<T::Error>> {
        let request = DiffRequest {
            cursor,
            limit,
            auth_token: auth_token.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(DiffApiError::Transport)?;
        let hash = response
            .as_hash()
            .ok_or(DiffApiError::Malformed("diff response was not a hash"))?;
        expect_ok_result(hash)?;

        let entries = hash
            .get_array("entries")
            .ok_or(DiffApiError::Malformed("diff response missing entries"))?
            .iter()
            .map(parse_diff_entry::<T::Error>)
            .collect::<Result<Vec<_>, _>>()?;

        let new_diff_id = hash
            .get_number("diffid")
            .or_else(|| hash.get_number("diffidfrom"))
            .ok_or(DiffApiError::Malformed("diff response missing diffid"))?;

        // The server signals a reset when it returns diffid=0 while the
        // client asked with a nonzero cursor, or when the response
        // carries an explicit `reset` flag.
        let reset = hash.get_bool("reset").unwrap_or(false) || (cursor > 0 && new_diff_id == 0);

        let api_server = extract_api_server_hint(hash);

        let response = DiffResponse {
            new_diff_id,
            reset,
            has_more: hash.get_bool("hasmore").unwrap_or(false),
            entries,
            api_server: api_server.clone(),
        };

        if let Some(hint) = api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Conversion: DiffResponse -> RemoteDiffBatch (engine types)
// ---------------------------------------------------------------------------

/// Convert a [`DiffResponse`] into a `pcloud_engine::diff_poller::RemoteDiffBatch`
/// for consumption by the sync engine.
///
/// `sync_id` — the sync root this batch applies to.
///
/// This is a free function rather than a method on `DiffResponse` so
/// that `pcloud-proto` does not depend on `pcloud-engine` (the
/// dependency flows the other way).
pub fn diff_response_to_batch(response: &DiffResponse, sync_id: u64) -> DiffResponseBatch {
    DiffResponseBatch {
        sync_id,
        cursor: response.new_diff_id,
        has_more: response.has_more,
        reset: response.reset,
        entries: response
            .entries
            .iter()
            .map(|entry| DiffResponseEntry {
                name: entry.metadata.name.clone(),
                is_folder: entry.metadata.is_folder,
                file_id: entry.metadata.file_id,
                folder_id: entry.metadata.folder_id,
                parent_folder_id: entry.metadata.parent_folder_id,
                deleted: entry.metadata.deleted,
                size: entry.metadata.size,
                hash: entry.metadata.hash,
                modified: entry.metadata.modified,
                created: entry.metadata.created,
                event: entry.event,
            })
            .collect(),
    }
}

/// Lightweight engine-facing batch that does not depend on
/// `pcloud-engine` types. The sync backend or engine adapter maps this
/// into `RemoteDiffBatch` / `RemoteDiffEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResponseBatch {
    /// Sync root identifier.
    pub sync_id: u64,
    /// New diff cursor.
    pub cursor: u64,
    /// Whether the server has more entries.
    pub has_more: bool,
    /// Whether the server signals a full resync.
    pub reset: bool,
    /// Entries in this batch.
    pub entries: Vec<DiffResponseEntry>,
}

/// One entry in a [`DiffResponseBatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResponseEntry {
    /// Entry name.
    pub name: String,
    /// `true` if folder.
    pub is_folder: bool,
    /// Remote file id.
    pub file_id: Option<u64>,
    /// Remote folder id.
    pub folder_id: Option<u64>,
    /// Parent folder id.
    pub parent_folder_id: Option<u64>,
    /// `true` if deleted.
    pub deleted: bool,
    /// File size in bytes.
    pub size: Option<u64>,
    /// Content hash.
    pub hash: Option<u64>,
    /// Modified timestamp (Unix seconds).
    pub modified: Option<u64>,
    /// Created timestamp (Unix seconds).
    pub created: Option<u64>,
    /// Numeric event tag.
    pub event: Option<u64>,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn parse_diff_entry<E>(value: &Value) -> Result<DiffEntry, DiffApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value
        .as_hash()
        .ok_or(DiffApiError::Malformed("diff entry was not a hash"))?;
    let metadata = hash
        .get_hash("metadata")
        .ok_or(DiffApiError::Malformed("diff entry missing metadata"))?;

    let deleted =
        hash.get_bool("deleted").unwrap_or(false) || metadata.get_bool("deleted").unwrap_or(false);

    Ok(DiffEntry {
        event: hash.get_number("event"),
        metadata: DiffFileMetadata {
            name: metadata
                .get_string("name")
                .ok_or(DiffApiError::Malformed("diff metadata missing name"))?
                .to_owned(),
            is_folder: metadata.get_bool("isfolder").unwrap_or(false),
            file_id: metadata.get_number("fileid"),
            folder_id: metadata.get_number("folderid"),
            parent_folder_id: metadata.get_number("parentfolderid"),
            deleted,
            size: metadata.get_number("size"),
            hash: metadata.get_number("hash"),
            modified: metadata.get_number("modified"),
            created: metadata.get_number("created"),
        },
    })
}

fn extract_api_server_hint(hash: HashView<'_>) -> Option<ApiServerHint> {
    let direct = hash.get_string("binapi").map(ToOwned::to_owned);
    let nested = hash.get_hash("apiserver").and_then(|apiserver| {
        apiserver
            .get_array("binapi")
            .and_then(|entries| entries.first())
            .and_then(Value::as_string)
            .map(ToOwned::to_owned)
            .or_else(|| apiserver.get_string("binapi").map(ToOwned::to_owned))
    });
    direct.or(nested).map(|binapi| ApiServerHint { binapi })
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), DiffApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }
    Err(DiffApiError::Result {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{io, sync::Mutex};

    use crate::{
        EncodedRequest,
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        response::Value,
    };

    use super::*;

    #[derive(Debug)]
    struct MockTransport {
        responses: Mutex<Vec<Value>>,
        hints: Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn with_responses(responses: Vec<Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                hints: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProtocolTransport for MockTransport {
        type Error = io::Error;

        fn execute(&self, _request: &EncodedRequest) -> Result<Value, Self::Error> {
            self.responses
                .lock()
                .expect("lock should not be poisoned")
                .pop()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, api_server: &str) {
            self.hints
                .lock()
                .expect("lock should not be poisoned")
                .push(api_server.to_owned());
        }
    }

    /// Build a minimal diff response with one file entry.
    fn one_file_response(diff_id: u64, has_more: bool) -> Value {
        Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(diff_id)),
            ("hasmore".to_owned(), Value::Bool(has_more)),
            (
                "entries".to_owned(),
                Value::Array(vec![Value::Hash(vec![
                    ("event".to_owned(), Value::Number(4)),
                    (
                        "metadata".to_owned(),
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("report.txt".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(false)),
                            ("fileid".to_owned(), Value::Number(42)),
                            ("parentfolderid".to_owned(), Value::Number(1)),
                            ("size".to_owned(), Value::Number(1024)),
                            ("hash".to_owned(), Value::Number(0xDEAD_BEEF)),
                            ("modified".to_owned(), Value::Number(1_700_000_000)),
                            ("created".to_owned(), Value::Number(1_699_000_000)),
                        ]),
                    ),
                ])]),
            ),
        ])
    }

    #[test]
    fn poll_diff_parses_file_entry_with_rich_metadata() {
        let transport = MockTransport::with_responses(vec![one_file_response(44, false)]);
        let api = DiffApi::new(transport);

        let resp = api.poll_diff("token", 0, 128).expect("should parse");

        assert_eq!(resp.new_diff_id, 44);
        assert!(!resp.has_more);
        assert!(!resp.reset);
        assert_eq!(resp.entries.len(), 1);

        let entry = &resp.entries[0];
        assert_eq!(entry.event, Some(4));
        assert_eq!(entry.metadata.name, "report.txt");
        assert!(!entry.metadata.is_folder);
        assert_eq!(entry.metadata.file_id, Some(42));
        assert_eq!(entry.metadata.parent_folder_id, Some(1));
        assert_eq!(entry.metadata.size, Some(1024));
        assert_eq!(entry.metadata.hash, Some(0xDEAD_BEEF));
        assert_eq!(entry.metadata.modified, Some(1_700_000_000));
        assert_eq!(entry.metadata.created, Some(1_699_000_000));
        assert!(!entry.metadata.deleted);
    }

    #[test]
    fn poll_diff_detects_reset_when_server_returns_zero_diffid() {
        let resp = Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(0)),
            ("hasmore".to_owned(), Value::Bool(false)),
            ("entries".to_owned(), Value::Array(Vec::new())),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        // cursor was 50, server returned 0 => reset
        let resp = api.poll_diff("token", 50, 128).expect("should parse");
        assert!(resp.reset);
        assert_eq!(resp.new_diff_id, 0);
    }

    #[test]
    fn poll_diff_no_reset_on_initial_sync() {
        let resp = Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(0)),
            ("hasmore".to_owned(), Value::Bool(false)),
            ("entries".to_owned(), Value::Array(Vec::new())),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        // cursor was 0, server returned 0 => NOT a reset
        let resp = api.poll_diff("token", 0, 128).expect("should parse");
        assert!(!resp.reset);
    }

    #[test]
    fn poll_diff_detects_explicit_reset_flag() {
        let resp = Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(10)),
            ("reset".to_owned(), Value::Bool(true)),
            ("hasmore".to_owned(), Value::Bool(false)),
            ("entries".to_owned(), Value::Array(Vec::new())),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        let resp = api.poll_diff("token", 5, 128).expect("should parse");
        assert!(resp.reset);
    }

    #[test]
    fn poll_diff_rejects_nonzero_result() {
        let resp = Value::Hash(vec![
            ("result".to_owned(), Value::Number(2000)),
            ("error".to_owned(), Value::String("auth expired".to_owned())),
            ("diffid".to_owned(), Value::Number(0)),
            ("entries".to_owned(), Value::Array(Vec::new())),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        let err = api
            .poll_diff("token", 0, 128)
            .expect_err("nonzero result should fail");
        assert!(matches!(err, DiffApiError::Result { result: 2000, .. }));
    }

    #[test]
    fn poll_diff_rejects_missing_metadata_name() {
        let resp = Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(1)),
            (
                "entries".to_owned(),
                Value::Array(vec![Value::Hash(vec![(
                    "metadata".to_owned(),
                    Value::Hash(vec![("fileid".to_owned(), Value::Number(9))]),
                )])]),
            ),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        let err = api
            .poll_diff("token", 0, 10)
            .expect_err("missing name should fail");
        assert!(err.to_string().contains("missing name"));
    }

    #[test]
    fn poll_diff_parses_folder_entry() {
        let resp = Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(7)),
            ("hasmore".to_owned(), Value::Bool(false)),
            (
                "entries".to_owned(),
                Value::Array(vec![Value::Hash(vec![
                    ("event".to_owned(), Value::Number(1)),
                    (
                        "metadata".to_owned(),
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("docs".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(true)),
                            ("folderid".to_owned(), Value::Number(5)),
                            ("parentfolderid".to_owned(), Value::Number(0)),
                            ("modified".to_owned(), Value::Number(1_700_000_000)),
                            ("created".to_owned(), Value::Number(1_699_000_000)),
                        ]),
                    ),
                ])]),
            ),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        let resp = api.poll_diff("token", 0, 128).expect("should parse");
        assert_eq!(resp.entries.len(), 1);
        let m = &resp.entries[0].metadata;
        assert!(m.is_folder);
        assert_eq!(m.folder_id, Some(5));
        assert!(m.file_id.is_none());
        assert!(m.size.is_none());
        assert!(m.hash.is_none());
    }

    #[test]
    fn poll_diff_handles_deleted_entry() {
        let resp = Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(15)),
            (
                "entries".to_owned(),
                Value::Array(vec![Value::Hash(vec![
                    ("event".to_owned(), Value::Number(6)),
                    ("deleted".to_owned(), Value::Bool(true)),
                    (
                        "metadata".to_owned(),
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("old.txt".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(false)),
                            ("fileid".to_owned(), Value::Number(99)),
                            ("parentfolderid".to_owned(), Value::Number(1)),
                        ]),
                    ),
                ])]),
            ),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        let resp = api.poll_diff("token", 10, 128).expect("should parse");
        assert!(resp.entries[0].metadata.deleted);
    }

    #[test]
    fn poll_diff_applies_api_server_hint() {
        let resp = Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(3)),
            ("entries".to_owned(), Value::Array(Vec::new())),
            (
                "apiserver".to_owned(),
                Value::Hash(vec![(
                    "binapi".to_owned(),
                    Value::Array(vec![Value::String("bineapi-eu.pcloud.com".to_owned())]),
                )]),
            ),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = DiffApi::new(transport);

        let resp = api.poll_diff("token", 0, 128).expect("should parse");
        assert!(resp.api_server.is_some());
        assert_eq!(
            api.transport().hints.lock().expect("lock").as_slice(),
            ["bineapi-eu.pcloud.com"]
        );
    }

    #[test]
    fn diff_response_to_batch_maps_correctly() {
        let response = DiffResponse {
            new_diff_id: 44,
            reset: false,
            has_more: true,
            entries: vec![DiffEntry {
                event: Some(4),
                metadata: DiffFileMetadata {
                    name: "report.txt".to_owned(),
                    is_folder: false,
                    file_id: Some(42),
                    folder_id: None,
                    parent_folder_id: Some(1),
                    deleted: false,
                    size: Some(1024),
                    hash: Some(0xDEAD),
                    modified: Some(1_700_000_000),
                    created: Some(1_699_000_000),
                },
            }],
            api_server: None,
        };

        let batch = diff_response_to_batch(&response, 7);
        assert_eq!(batch.sync_id, 7);
        assert_eq!(batch.cursor, 44);
        assert!(batch.has_more);
        assert!(!batch.reset);
        assert_eq!(batch.entries.len(), 1);
        assert_eq!(batch.entries[0].name, "report.txt");
        assert_eq!(batch.entries[0].size, Some(1024));
        assert_eq!(batch.entries[0].event, Some(4));
    }

    #[test]
    fn poll_diff_has_more_flag_propagates() {
        let transport = MockTransport::with_responses(vec![one_file_response(10, true)]);
        let api = DiffApi::new(transport);

        let resp = api.poll_diff("token", 5, 128).expect("should parse");
        assert!(resp.has_more);
    }

    #[test]
    fn poll_diff_cursor_advances() {
        // Simulate two successive polls: cursor 0->44, 44->88.
        let transport = MockTransport::with_responses(vec![
            one_file_response(44, true),
            one_file_response(88, false),
        ]);
        let api = DiffApi::new(transport);

        let resp1 = api.poll_diff("token", 0, 128).expect("poll 1");
        assert_eq!(resp1.new_diff_id, 44);
        assert!(resp1.has_more);

        let resp2 = api
            .poll_diff("token", resp1.new_diff_id, 128)
            .expect("poll 2");
        assert_eq!(resp2.new_diff_id, 88);
        assert!(!resp2.has_more);
    }
}

//! Sync protocol client: add/list/remove remote sync roots, syncability
//! classification, and sync-related server queries. Consumed by
//! `pcloud-backends::sync_backend` and the sync engine.
//!
//! ## Role in the request pipeline
//!
//! Wraps the pCloud `diff` / `listsyncfolders` / `sync_*` command
//! family. Builds requests via the method-builder layer, hands them
//! to the transport, and projects the resulting response trees into
//! domain types (diff batches, sync-folder descriptors) the engine
//! can feed directly into filesystem state.
//!
//! ## Security considerations
//!
//! Server-returned file / folder ids drive filesystem mutations in
//! higher layers; callers must not trust arbitrary ids without
//! checking ownership against the authenticated session. This
//! module does not persist state and holds no secrets.
//!
//! Portable; no platform gating.

use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHint, ApiServerHintConsumer, ProtocolTransport},
    methods::diff::DiffRequest,
    response::{HashView, Value},
};

/// Typed client over a pCloud transport for sync-related commands.
///
/// ## Design choices
///
/// Generic over `T` (the transport) so the same client code can be
/// driven by the production [`crate::BinaryApiTransport`], by a
/// resilient retry wrapper ([`crate::resilient_transport`]), or by
/// in-process mocks in tests. The transport is moved in — each
/// `SyncApi` owns its channel.
#[derive(Debug)]
pub struct SyncApi<T> {
    transport: T,
}

/// Error returned by any [`SyncApi`] method.
///
/// Splits failures by origin so callers can react appropriately:
///
/// - encode-time framing bugs (`Encode`) are always caller bugs,
/// - transport failures (`Transport`) may be retriable,
/// - protocol-level non-zero results (`Result`) are server rejections
///   the caller should surface,
/// - `Malformed` indicates a server-side protocol violation and
///   should be logged for investigation.
#[derive(Debug, Error)]
pub enum SyncApiError<E: std::error::Error + Send + Sync + 'static> {
    /// Request encoding failed (name too long, frame too large).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// Underlying transport raised an error. Carries the transport's
    /// own typed error so callers can inspect retriability.
    #[error("transport failed: {0}")]
    Transport(E),
    /// Server returned a non-zero `result` code.
    ///
    /// `message` carries the accompanying `error` string when the
    /// server includes one.
    #[error("diff returned non-zero result code {result} ({message:?})")]
    Result {
        /// Numeric pCloud result code.
        result: u64,
        /// Human-readable message from the server, if any.
        message: Option<String>,
    },
    /// Server response was syntactically valid but missing a field
    /// this client requires.
    ///
    /// The static `&str` identifies which field was missing; this
    /// is diagnostics-only and should never be used in control
    /// flow.
    #[error("response was malformed: {0}")]
    Malformed(&'static str),
}

/// `DiffBatch` — diff batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBatch {
    /// The `diff_id` field (diff id).
    pub diff_id: u64,
    /// The `has_more` field (has more).
    pub has_more: bool,
    /// The `entries` field (entries).
    pub entries: Vec<DiffEntry>,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
}

/// `DiffEntry` — diff entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// The `event` field (event).
    pub event: Option<u64>,
    /// The `metadata` field (metadata).
    pub metadata: DiffEntryMetadata,
}

/// `DiffEntryMetadata` — diff entry metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntryMetadata {
    /// The `name` field (name).
    pub name: String,
    /// The `is_folder` field (is folder).
    pub is_folder: bool,
    /// The `file_id` field (file id).
    pub file_id: Option<u64>,
    /// The `folder_id` field (folder id).
    pub folder_id: Option<u64>,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: Option<u64>,
    /// The `deleted` field (deleted).
    pub deleted: bool,
}

impl<T> SyncApi<T> {
    /// `new` — new.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T> SyncApi<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// `apply_api_server_hint` — apply api server hint.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.transport.apply_api_server_hint(api_server);
    }

    /// `diff` — diff.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn diff(
        &self,
        auth_token: impl Into<String>,
        cursor: u64,
        limit: u64,
    ) -> Result<DiffBatch, SyncApiError<T::Error>> {
        let request = DiffRequest {
            cursor,
            limit,
            auth_token: auth_token.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(SyncApiError::Transport)?;
        let hash = response
            .as_hash()
            .ok_or(SyncApiError::Malformed("diff response was not a hash"))?;
        expect_ok_result(hash)?;

        let entries = hash
            .get_array("entries")
            .ok_or(SyncApiError::Malformed("diff response missing entries"))?
            .iter()
            .map(parse_diff_entry::<T::Error>)
            .collect::<Result<Vec<_>, _>>()?;

        let batch = DiffBatch {
            diff_id: hash
                .get_number("diffid")
                .or_else(|| hash.get_number("diffidfrom"))
                .ok_or(SyncApiError::Malformed("diff response missing diffid"))?,
            has_more: hash.get_bool("hasmore").unwrap_or(false),
            entries,
            api_server: extract_api_server_hint(hash),
        };
        if let Some(hint) = batch.api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(batch)
    }
}

fn parse_diff_entry<E>(value: &Value) -> Result<DiffEntry, SyncApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value
        .as_hash()
        .ok_or(SyncApiError::Malformed("diff entry was not a hash"))?;
    let metadata = hash
        .get_hash("metadata")
        .ok_or(SyncApiError::Malformed("diff entry missing metadata"))?;

    Ok(DiffEntry {
        event: hash.get_number("event"),
        metadata: DiffEntryMetadata {
            name: metadata
                .get_string("name")
                .ok_or(SyncApiError::Malformed("diff metadata missing name"))?
                .to_owned(),
            is_folder: metadata.get_bool("isfolder").unwrap_or(false),
            file_id: metadata.get_number("fileid"),
            folder_id: metadata.get_number("folderid"),
            parent_folder_id: metadata.get_number("parentfolderid"),
            deleted: hash.get_bool("deleted").unwrap_or(false)
                || metadata.get_bool("deleted").unwrap_or(false),
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

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), SyncApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }

    Err(SyncApiError::Result {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Mutex};

    use crate::{
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        response::Value,
    };

    use super::SyncApi;

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

        fn execute(&self, _request: &crate::EncodedRequest) -> Result<Value, Self::Error> {
            self.responses
                .lock()
                .expect("responses lock should not be poisoned")
                .pop()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, api_server: &str) {
            self.hints
                .lock()
                .expect("hints lock should not be poisoned")
                .push(api_server.to_owned());
        }
    }

    #[test]
    fn diff_api_parses_entries_and_api_server_hint() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(44)),
            ("hasmore".to_owned(), Value::Bool(true)),
            (
                "entries".to_owned(),
                Value::Array(vec![Value::Hash(vec![
                    ("event".to_owned(), Value::Number(1)),
                    (
                        "metadata".to_owned(),
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("report.txt".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(false)),
                            ("fileid".to_owned(), Value::Number(9)),
                            ("parentfolderid".to_owned(), Value::Number(2)),
                        ]),
                    ),
                ])]),
            ),
            (
                "apiserver".to_owned(),
                Value::Hash(vec![(
                    "binapi".to_owned(),
                    Value::Array(vec![Value::String("bineapi-eu.pcloud.com".to_owned())]),
                )]),
            ),
        ])]);
        let api = SyncApi::new(transport);

        let batch = api.diff("auth", 0, 128).expect("diff should parse");

        assert_eq!(batch.diff_id, 44);
        assert!(batch.has_more);
        assert_eq!(batch.entries.len(), 1);
        assert_eq!(batch.entries[0].metadata.name, "report.txt");
        assert_eq!(batch.entries[0].metadata.file_id, Some(9));
        assert_eq!(
            api.transport
                .hints
                .lock()
                .expect("hints lock should not be poisoned")
                .as_slice(),
            ["bineapi-eu.pcloud.com"]
        );
    }

    #[test]
    fn diff_api_rejects_nonzero_result_code() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2000)),
            ("error".to_owned(), Value::String("diff failed".to_owned())),
            ("diffid".to_owned(), Value::Number(44)),
            ("hasmore".to_owned(), Value::Bool(false)),
            ("entries".to_owned(), Value::Array(Vec::new())),
        ])]);
        let api = SyncApi::new(transport);

        let err = api
            .diff("auth", 0, 128)
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::SyncApiError::Result {
                result: 2000,
                ref message
            } if message.as_deref() == Some("diff failed")
        ));
    }

    #[test]
    fn diff_api_rejects_entries_without_metadata_name() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("diffid".to_owned(), Value::Number(1)),
            (
                "entries".to_owned(),
                Value::Array(vec![Value::Hash(vec![(
                    "metadata".to_owned(),
                    Value::Hash(vec![("fileid".to_owned(), Value::Number(9))]),
                )])]),
            ),
        ])]);
        let api = SyncApi::new(transport);

        let err = api
            .diff("auth", 0, 10)
            .expect_err("missing metadata should fail");
        assert!(err.to_string().contains("missing name"));
    }
}

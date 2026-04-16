//! Transfer protocol client: `getfilelink`, `upload_create`,
//! `upload_write`, `upload_save`, and related upload/download helpers.
//! Consumed by `pcloud-backends::transfer_backend` and the SDK's direct
//! transfer helpers.
//!
//! ## Role in the request pipeline
//!
//! Implements the control-plane side of transfers: opening an
//! upload session, writing bytes, committing via `upload_save`, and
//! resolving download URLs via `getfilelink`. The actual byte
//! movement for downloads happens over HTTPS via
//! [`crate::http_download`]; uploads stream bytes inline through
//! the binary transport's `execute_with_body` path.
//!
//! The `PSYNC_*` constants mirror the upstream C client's tuning
//! knobs (block checksum size, pending upload-request depth,
//! backoff after transient failures). They are exposed so higher
//! layers can stay bit-for-bit compatible with the legacy behaviour
//! during the C-to-Rust transition.
//!
//! ## Security considerations
//!
//! - Download URLs returned by the server are time-limited and
//!   carry an embedded auth token; callers must not log them or
//!   persist them in world-readable storage.
//! - Block checksums are SHA-1 purely for interoperability with
//!   the legacy sync protocol — they are *not* a security
//!   primitive. End-to-end integrity is provided by TLS and by the
//!   pCloud crypto layer for encrypted files.
//! - Partial uploads are not auto-cancelled; callers must either
//!   commit via `upload_save` or explicitly drop the session.
//!
//! Portable; no platform gating.

use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHint, ApiServerHintConsumer, ProtocolTransport},
    methods::{
        download::GetFileLinkRequest,
        folder::{DeleteFileRequest, RenameFileRequest},
        upload::{
            ConflictParam, GetChecksumLinkRequest, PSYNC_CHECKSUM_FIELD, PSYNC_HASH_DIGEST_HEXLEN,
            UploadBlockChecksumsRequest, UploadCreateRequest, UploadDeleteRequest,
            UploadFileRequest, UploadInfoRequest, UploadWriteFromFileRequest,
        },
    },
    response::{HashView, Value},
};

pub use crate::methods::upload::{
    BlockChecksum, BlockChecksumHeader, PSYNC_COPY_BUFFER_SIZE, PSYNC_MAX_COPY_FROM_REQ,
    PSYNC_MAX_PENDING_UPLOAD_REQS, PSYNC_MIN_SIZE_FOR_CHECKSUMS, PSYNC_SLEEP_ON_FAILED_UPLOAD_MS,
    UploadErrorClass, decode_block_checksums, upload_sha1_hex,
};

/// `TransferApi` — transfer api.
#[derive(Debug)]
pub struct TransferApi<T> {
    transport: T,
}

/// `TransferApiError` — transfer api error.
#[derive(Debug, Error)]
pub enum TransferApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("transfer method returned non-zero result code {result} ({message:?})")]
    Result {
        /// The `result` field (result).
        result: u64,
        /// The `message` field (message).
        message: Option<String>,
    },
    /// `Malformed` variant (malformed).
    #[error("response was malformed: {0}")]
    Malformed(&'static str),
}

/// `DownloadLink` — download link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadLink {
    /// The `path` field (path).
    pub path: String,
    /// The `hosts` field (hosts).
    pub hosts: Vec<String>,
    /// The `download_tag` field (download tag).
    pub download_tag: Option<String>,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
}

/// `UploadSession` — upload session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadSession {
    /// The `upload_id` field (upload id).
    pub upload_id: u64,
    /// The `file_id` field (file id).
    pub file_id: Option<u64>,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: u64,
    /// The `file_name` field (file name).
    pub file_name: String,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
}

/// Response of `upload_info` (`pupload.c:1193-1213`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadInfo {
    /// The `chunk_id` field (chunk id).
    pub chunk_id: u64,
    /// The `size` field (size).
    pub size: u64,
    /// 40-byte lowercase hex digest in the response field named by
    /// `PSYNC_CHECKSUM` (`"sha1"`, `psettings.h:188`).
    pub sha1_hex: String,
}

/// Response of `getchecksumlink` (`pnetlibs.c:1626-1629`). The client then
/// issues a separate HTTP GET to fetch the binary checksum trailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumLink {
    /// The `hosts` field (hosts).
    pub hosts: Vec<String>,
    /// The `path` field (path).
    pub path: String,
    /// The `download_tag` field (download tag).
    pub download_tag: String,
}

/// Metadata snippet returned by `deletefile` and `renamefile` (mirrors
/// the file metadata shape returned by the C task handlers —
/// `pclsync/pupload.c:276-291`, `pclsync/pupload.c:1650-1661`).
/// Every field is best-effort: the server consistently returns metadata
/// on success but some older `deletefile` responses omit fields like
/// `parentfolderid`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenamedFileResponse {
    /// The `file_id` field (file id).
    pub file_id: Option<u64>,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: Option<u64>,
    /// The `name` field (name).
    pub name: Option<String>,
    /// The `is_deleted` field (is deleted).
    pub is_deleted: bool,
}

/// Parsed metadata snippet returned by the single-shot `uploadfile` call
/// (`pupload.c:746-753`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadFileResult {
    /// The `file_id` field (file id).
    pub file_id: u64,
    /// The `hash` field (hash).
    pub hash: u64,
    /// The `size` field (size).
    pub size: u64,
    /// The `file_name` field (file name).
    pub file_name: String,
    /// The `conflicted` field (conflicted).
    pub conflicted: bool,
    /// Server-reported SHA1 hex read from `checksums[0].sha1`
    /// (`pupload.c:750-753`).
    pub sha1_hex: String,
}

impl<T> TransferApi<T> {
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

impl<T> TransferApi<T>
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

    /// `get_file_link` — get file link.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn get_file_link(
        &self,
        auth_token: impl Into<String>,
        file_id: u64,
        forced_host: Option<String>,
    ) -> Result<DownloadLink, TransferApiError<T::Error>> {
        let request = GetFileLinkRequest {
            file_id,
            auth_token: auth_token.into(),
            forced_host,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "getfilelink response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        let link = DownloadLink {
            path: hash
                .get_string("path")
                .ok_or(TransferApiError::Malformed("getfilelink missing path"))?
                .to_owned(),
            hosts: parse_string_array(hash, "hosts")?,
            download_tag: hash
                .get_string("dwltag")
                .or_else(|| hash.get_string("downloadtag"))
                .map(ToOwned::to_owned),
            api_server: extract_api_server_hint(hash),
        };
        if let Some(hint) = link.api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(link)
    }

    /// `upload_create` — upload create.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn upload_create(
        &self,
        auth_token: impl Into<String>,
        parent_folder_id: u64,
        file_name: impl Into<String>,
        file_size: u64,
    ) -> Result<UploadSession, TransferApiError<T::Error>> {
        let request = UploadCreateRequest {
            auth_token: auth_token.into(),
            parent_folder_id,
            file_name: file_name.into(),
            file_size,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "upload_create response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        let session = UploadSession {
            upload_id: hash
                .get_number("uploadid")
                .ok_or(TransferApiError::Malformed(
                    "upload_create missing uploadid",
                ))?,
            file_id: hash.get_number("fileid"),
            parent_folder_id: request.parent_folder_id,
            file_name: request.file_name.clone(),
            api_server: extract_api_server_hint(hash),
        };
        if let Some(hint) = session.api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(session)
    }

    /// Abort an open upload session. Fire-and-forget in C
    /// (`pupload.c:1281-1286`); here we still surface any `result != 0`
    /// so callers can log — the C version discards the response.
    pub fn upload_delete(
        &self,
        auth_token: impl Into<String>,
        upload_id: u64,
    ) -> Result<(), TransferApiError<T::Error>> {
        let request = UploadDeleteRequest {
            auth_token: auth_token.into(),
            upload_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "upload_delete response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(())
    }

    /// Delete a remote file. Mirrors `task_deletefile`
    /// (`pclsync/pupload.c:1650-1661`) and `psync_send_task_unlink`
    /// (`pclsync/pfsupload.c:1327-1338`).
    pub fn delete_file(
        &self,
        auth_token: impl Into<String>,
        file_id: u64,
    ) -> Result<RenamedFileResponse, TransferApiError<T::Error>> {
        let request = DeleteFileRequest {
            auth_token: auth_token.into(),
            file_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "deletefile response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(parse_mutated_file(hash))
    }

    /// Rename and/or move a remote file. Mirrors `task_renameremotefile`
    /// (`pclsync/pupload.c:276-291`) and `psync_send_task_rename_file`
    /// (`pclsync/pfsupload.c:1438-1447`). A pure rename passes the
    /// existing parent as `to_folder_id`.
    pub fn rename_file(
        &self,
        auth_token: impl Into<String>,
        file_id: u64,
        to_folder_id: u64,
        to_name: impl Into<String>,
    ) -> Result<RenamedFileResponse, TransferApiError<T::Error>> {
        let request = RenameFileRequest {
            auth_token: auth_token.into(),
            file_id,
            to_folder_id,
            to_name: to_name.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "renamefile response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(parse_mutated_file(hash))
    }

    /// Issue `upload_info` and parse `{ id, size, sha1 }`
    /// (`pupload.c:1193-1213`, spec §2.6).
    pub fn upload_info(
        &self,
        auth_token: impl Into<String>,
        upload_id: u64,
        chunk_id: u64,
    ) -> Result<UploadInfo, TransferApiError<T::Error>> {
        let request = UploadInfoRequest {
            auth_token: auth_token.into(),
            upload_id,
            chunk_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "upload_info response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let id = hash
            .get_number("id")
            .ok_or(TransferApiError::Malformed("upload_info missing id"))?;
        let size = hash
            .get_number("size")
            .ok_or(TransferApiError::Malformed("upload_info missing size"))?;
        let sha1 = hash
            .get_string(PSYNC_CHECKSUM_FIELD)
            .ok_or(TransferApiError::Malformed("upload_info missing sha1"))?;
        if sha1.len() != PSYNC_HASH_DIGEST_HEXLEN {
            return Err(TransferApiError::Malformed(
                "upload_info sha1 hex has unexpected length",
            ));
        }
        Ok(UploadInfo {
            chunk_id: id,
            size,
            sha1_hex: sha1.to_owned(),
        })
    }

    /// Issue `upload_blockchecksums` and parse the JSON envelope. The binary
    /// trailer (`BlockChecksumHeader` + `Vec<BlockChecksum>`) must be read
    /// separately from the same socket; see `BlockChecksumHeader::decode`
    /// and `decode_block_checksums`.
    ///
    /// TODO(spec §9.5): live-API verification required before relying on
    /// pipelining this ahead of `upload_write` responses draining.
    pub fn upload_blockchecksums_begin(
        &self,
        auth_token: impl Into<String>,
        upload_id: u64,
    ) -> Result<(), TransferApiError<T::Error>> {
        let request = UploadBlockChecksumsRequest {
            auth_token: auth_token.into(),
            upload_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "upload_blockchecksums response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(())
    }

    /// `getchecksumlink` for a committed file — returns the HTTP fetch
    /// coordinates (`pnetlibs.c:1626-1629`).
    pub fn get_checksum_link(
        &self,
        auth_token: impl Into<String>,
        file_id: u64,
        hash: u64,
    ) -> Result<ChecksumLink, TransferApiError<T::Error>> {
        let request = GetChecksumLinkRequest {
            auth_token: auth_token.into(),
            file_id,
            hash,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(TransferApiError::Transport)?;
        let hash_view = response.as_hash().ok_or(TransferApiError::Malformed(
            "getchecksumlink response was not a hash",
        ))?;
        expect_ok_result(hash_view)?;
        let hosts = parse_string_array(hash_view, "hosts")?;
        let path = hash_view
            .get_string("path")
            .ok_or(TransferApiError::Malformed("getchecksumlink missing path"))?
            .to_owned();
        let dwltag = hash_view
            .get_string("dwltag")
            .ok_or(TransferApiError::Malformed(
                "getchecksumlink missing dwltag",
            ))?
            .to_owned();
        Ok(ChecksumLink {
            hosts,
            path,
            download_tag: dwltag,
        })
    }

    /// Encode an `upload_writefromfile` request (server-side copy).
    /// Returns the encoded frame so the caller can stream it alongside
    /// other pipelined upload writes. The response is asynchronous and
    /// matched by `id`, mirroring the C pipelined model (`pupload.c:843-859`).
    pub fn encode_upload_write_from_file(
        &self,
        auth_token: impl Into<String>,
        upload_id: u64,
        upload_offset: u64,
        chunk_id: u64,
        file_id: u64,
        hash: u64,
        source_offset: u64,
        count: u64,
    ) -> Result<crate::EncodedRequest, TransferApiError<T::Error>> {
        if count > PSYNC_MAX_COPY_FROM_REQ {
            return Err(TransferApiError::Malformed(
                "upload_writefromfile count exceeds PSYNC_MAX_COPY_FROM_REQ",
            ));
        }
        let request = UploadWriteFromFileRequest {
            auth_token: auth_token.into(),
            upload_id,
            upload_offset,
            chunk_id,
            file_id,
            hash,
            source_offset,
            count,
        };
        Ok(request.encode()?)
    }

    /// Encode a single-shot `uploadfile` request (without executing it). The
    /// caller is responsible for streaming `body_len` raw bytes immediately
    /// after the frame, matching the C `papi_send(..., fsize, 0)` pattern at
    /// `pupload.c:694-695`.
    pub fn encode_uploadfile(
        &self,
        auth_token: impl Into<String>,
        parent_folder_id: u64,
        filename: impl Into<String>,
        mtime: u64,
        ctime: Option<u64>,
        conflict: ConflictParam,
        body_len: u64,
    ) -> Result<crate::EncodedRequest, TransferApiError<T::Error>> {
        let request = UploadFileRequest {
            auth_token: auth_token.into(),
            parent_folder_id,
            filename: filename.into(),
            nopartial: true,
            mtime,
            ctime,
            conflict,
            body_len,
        };
        Ok(request.encode_with_body()?)
    }

    /// Parse the `uploadfile` response envelope (`pupload.c:734-775`). The
    /// caller must first stream the body and then hand the parsed response
    /// value back into this helper. We split encode from parse because the
    /// transport layer is responsible for the raw-body write.
    pub fn parse_uploadfile_response(
        response: &Value,
    ) -> Result<UploadFileResult, TransferApiError<T::Error>> {
        let hash = response.as_hash().ok_or(TransferApiError::Malformed(
            "uploadfile response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let metadata_array = hash
            .get_array("metadata")
            .ok_or(TransferApiError::Malformed("uploadfile missing metadata"))?;
        let meta =
            metadata_array
                .first()
                .and_then(Value::as_hash)
                .ok_or(TransferApiError::Malformed(
                    "uploadfile metadata[0] was not a hash",
                ))?;
        let file_id = meta
            .get_number("fileid")
            .ok_or(TransferApiError::Malformed(
                "uploadfile metadata missing fileid",
            ))?;
        let hash_num = meta.get_number("hash").ok_or(TransferApiError::Malformed(
            "uploadfile metadata missing hash",
        ))?;
        let size = meta.get_number("size").ok_or(TransferApiError::Malformed(
            "uploadfile metadata missing size",
        ))?;
        let file_name = meta
            .get_string("name")
            .ok_or(TransferApiError::Malformed(
                "uploadfile metadata missing name",
            ))?
            .to_owned();
        let conflicted = meta.get_bool("conflicted").unwrap_or(false);
        let checksums = hash
            .get_array("checksums")
            .ok_or(TransferApiError::Malformed("uploadfile missing checksums"))?;
        let first =
            checksums
                .first()
                .and_then(Value::as_hash)
                .ok_or(TransferApiError::Malformed(
                    "uploadfile checksums[0] was not a hash",
                ))?;
        let sha1 = first
            .get_string(PSYNC_CHECKSUM_FIELD)
            .ok_or(TransferApiError::Malformed(
                "uploadfile checksums missing sha1",
            ))?;
        if sha1.len() != PSYNC_HASH_DIGEST_HEXLEN {
            return Err(TransferApiError::Malformed(
                "uploadfile sha1 hex has unexpected length",
            ));
        }
        Ok(UploadFileResult {
            file_id,
            hash: hash_num,
            size,
            file_name,
            conflicted,
            sha1_hex: sha1.to_owned(),
        })
    }
}

fn parse_string_array<E>(
    hash: HashView<'_>,
    key: &'static str,
) -> Result<Vec<String>, TransferApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let array = hash
        .get_array(key)
        .ok_or(TransferApiError::Malformed("response missing string array"))?;
    array
        .iter()
        .map(|value| {
            value
                .as_string()
                .map(ToOwned::to_owned)
                .ok_or(TransferApiError::Malformed("array item was not a string"))
        })
        .collect()
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

fn parse_mutated_file(hash: HashView<'_>) -> RenamedFileResponse {
    let metadata = hash.get_hash("metadata");
    let (file_id, parent_folder_id, name, is_deleted) = match metadata {
        Some(meta) => (
            meta.get_number("fileid").or_else(|| meta.get_number("id")),
            meta.get_number("parentfolderid"),
            meta.get_string("name").map(ToOwned::to_owned),
            meta.get_bool("isdeleted").unwrap_or(false),
        ),
        None => (None, None, None, false),
    };
    RenamedFileResponse {
        file_id,
        parent_folder_id,
        name,
        is_deleted,
    }
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), TransferApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }

    Err(TransferApiError::Result {
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

    use super::TransferApi;

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
    fn get_file_link_parses_hosts_path_and_tag() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            (
                "path".to_owned(),
                Value::String("/get/abc/report.txt".to_owned()),
            ),
            (
                "hosts".to_owned(),
                Value::Array(vec![
                    Value::String("c1.pcloud.com".to_owned()),
                    Value::String("c2.pcloud.com".to_owned()),
                ]),
            ),
            (
                "dwltag".to_owned(),
                Value::String("download-tag".to_owned()),
            ),
            (
                "apiserver".to_owned(),
                Value::Hash(vec![(
                    "binapi".to_owned(),
                    Value::Array(vec![Value::String("bineapi-us.pcloud.com".to_owned())]),
                )]),
            ),
        ])]);
        let api = TransferApi::new(transport);

        let link = api
            .get_file_link("auth", 9, None)
            .expect("getfilelink should succeed");

        assert_eq!(link.path, "/get/abc/report.txt");
        assert_eq!(link.hosts, vec!["c1.pcloud.com", "c2.pcloud.com"]);
        assert_eq!(link.download_tag.as_deref(), Some("download-tag"));
        assert_eq!(
            api.transport
                .hints
                .lock()
                .expect("hints lock should not be poisoned")
                .as_slice(),
            ["bineapi-us.pcloud.com"]
        );
    }

    #[test]
    fn get_file_link_rejects_missing_path() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "hosts".to_owned(),
            Value::Array(vec![Value::String("c1.pcloud.com".to_owned())]),
        )])]);
        let api = TransferApi::new(transport);

        let err = api
            .get_file_link("auth", 9, None)
            .expect_err("missing path should fail");
        assert!(err.to_string().contains("missing path"));
    }

    #[test]
    fn get_file_link_rejects_nonzero_result_code() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2001)),
            ("error".to_owned(), Value::String("link failed".to_owned())),
            (
                "path".to_owned(),
                Value::String("/get/abc/report.txt".to_owned()),
            ),
            (
                "hosts".to_owned(),
                Value::Array(vec![Value::String("c1.pcloud.com".to_owned())]),
            ),
        ])]);
        let api = TransferApi::new(transport);

        let err = api
            .get_file_link("auth", 9, None)
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::TransferApiError::Result {
                result: 2001,
                ref message
            } if message.as_deref() == Some("link failed")
        ));
    }

    #[test]
    fn upload_create_parses_upload_id() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("uploadid".to_owned(), Value::Number(77)),
            ("fileid".to_owned(), Value::Number(9)),
        ])]);
        let api = TransferApi::new(transport);

        let session = api
            .upload_create("auth", 2, "report.txt", 1024)
            .expect("upload_create should succeed");

        assert_eq!(session.upload_id, 77);
        assert_eq!(session.file_id, Some(9));
        assert_eq!(session.parent_folder_id, 2);
        assert_eq!(session.file_name, "report.txt");
    }

    #[test]
    fn upload_create_rejects_missing_upload_id() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "fileid".to_owned(),
            Value::Number(9),
        )])]);
        let api = TransferApi::new(transport);

        let err = api
            .upload_create("auth", 2, "report.txt", 1024)
            .expect_err("missing uploadid should fail");
        assert!(err.to_string().contains("missing uploadid"));
    }

    #[test]
    fn upload_info_parses_id_size_and_sha1() {
        let sha1 = "a".repeat(super::PSYNC_HASH_DIGEST_HEXLEN);
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("id".to_owned(), Value::Number(5)),
            ("size".to_owned(), Value::Number(1024)),
            ("sha1".to_owned(), Value::String(sha1.clone())),
        ])]);
        let api = TransferApi::new(transport);
        let info = api.upload_info("auth", 77, 5).expect("upload_info ok");
        assert_eq!(info.chunk_id, 5);
        assert_eq!(info.size, 1024);
        assert_eq!(info.sha1_hex, sha1);
    }

    #[test]
    fn upload_info_rejects_bad_sha1_length() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("id".to_owned(), Value::Number(5)),
            ("size".to_owned(), Value::Number(1024)),
            ("sha1".to_owned(), Value::String("short".to_owned())),
        ])]);
        let api = TransferApi::new(transport);
        let err = api.upload_info("auth", 77, 5).expect_err("should fail");
        assert!(err.to_string().contains("sha1 hex"));
    }

    #[test]
    fn upload_delete_handles_ok_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = TransferApi::new(transport);
        api.upload_delete("auth", 42).expect("ok");
    }

    #[test]
    fn upload_blockchecksums_begin_accepts_ok() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = TransferApi::new(transport);
        api.upload_blockchecksums_begin("auth", 9).expect("ok");
    }

    #[test]
    fn get_checksum_link_parses_hosts_path_and_dwltag() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "hosts".to_owned(),
                Value::Array(vec![Value::String("c1.pcloud.com".to_owned())]),
            ),
            ("path".to_owned(), Value::String("/cs/abc".to_owned())),
            ("dwltag".to_owned(), Value::String("tag".to_owned())),
        ])]);
        let api = TransferApi::new(transport);
        let link = api.get_checksum_link("auth", 5, 99).expect("ok");
        assert_eq!(link.path, "/cs/abc");
        assert_eq!(link.download_tag, "tag");
        assert_eq!(link.hosts, vec!["c1.pcloud.com"]);
    }

    #[test]
    fn encode_upload_write_from_file_rejects_oversized_count() {
        let transport = MockTransport::with_responses(vec![]);
        let api = TransferApi::new(transport);
        let err = api
            .encode_upload_write_from_file(
                "auth",
                1,
                0,
                2,
                3,
                4,
                0,
                super::PSYNC_MAX_COPY_FROM_REQ + 1,
            )
            .expect_err("oversized count should be rejected");
        assert!(err.to_string().contains("PSYNC_MAX_COPY_FROM_REQ"));
    }

    #[test]
    fn encode_uploadfile_produces_body_bearing_frame() {
        let transport = MockTransport::with_responses(vec![]);
        let api = TransferApi::new(transport);
        let encoded = api
            .encode_uploadfile(
                "auth",
                7,
                "a.bin",
                1_700_000_000,
                None,
                super::ConflictParam::New,
                2048,
            )
            .expect("encode");
        assert_eq!(encoded.frame.command, "uploadfile");
    }

    #[test]
    fn parse_uploadfile_response_extracts_metadata_and_sha1() {
        let sha1 = "b".repeat(super::PSYNC_HASH_DIGEST_HEXLEN);
        let response = Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "metadata".to_owned(),
                Value::Array(vec![Value::Hash(vec![
                    ("fileid".to_owned(), Value::Number(101)),
                    ("hash".to_owned(), Value::Number(202)),
                    ("size".to_owned(), Value::Number(42)),
                    ("name".to_owned(), Value::String("a.bin".to_owned())),
                    ("conflicted".to_owned(), Value::Bool(true)),
                ])]),
            ),
            (
                "checksums".to_owned(),
                Value::Array(vec![Value::Hash(vec![(
                    "sha1".to_owned(),
                    Value::String(sha1.clone()),
                )])]),
            ),
        ]);
        let out =
            TransferApi::<MockTransport>::parse_uploadfile_response(&response).expect("parse ok");
        assert_eq!(out.file_id, 101);
        assert_eq!(out.hash, 202);
        assert_eq!(out.size, 42);
        assert_eq!(out.file_name, "a.bin");
        assert!(out.conflicted);
        assert_eq!(out.sha1_hex, sha1);
    }

    #[test]
    fn upload_create_rejects_nonzero_result_code() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2002)),
            (
                "error".to_owned(),
                Value::String("upload session refused".to_owned()),
            ),
            ("uploadid".to_owned(), Value::Number(77)),
        ])]);
        let api = TransferApi::new(transport);

        let err = api
            .upload_create("auth", 2, "report.txt", 1024)
            .expect_err("nonzero result should fail");

        assert!(matches!(
            err,
            super::TransferApiError::Result {
                result: 2002,
                ref message
            } if message.as_deref() == Some("upload session refused")
        ));
    }

    // ----- delete_file / rename_file --------------------------------------

    #[test]
    fn delete_file_parses_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("fileid".to_owned(), Value::Number(42)),
                    ("parentfolderid".to_owned(), Value::Number(7)),
                    ("name".to_owned(), Value::String("doomed.txt".to_owned())),
                    ("isdeleted".to_owned(), Value::Bool(true)),
                ]),
            ),
        ])]);
        let api = TransferApi::new(transport);
        let response = api
            .delete_file("auth", 42)
            .expect("deletefile should parse");
        assert_eq!(response.file_id, Some(42));
        assert_eq!(response.parent_folder_id, Some(7));
        assert_eq!(response.name.as_deref(), Some("doomed.txt"));
        assert!(response.is_deleted);
    }

    #[test]
    fn delete_file_tolerates_missing_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = TransferApi::new(transport);
        let response = api
            .delete_file("auth", 42)
            .expect("deletefile must accept metadata-less success");
        assert_eq!(response.file_id, None);
        assert!(!response.is_deleted);
    }

    #[test]
    fn delete_file_propagates_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2009)),
            (
                "error".to_owned(),
                Value::String("File not found.".to_owned()),
            ),
        ])]);
        let api = TransferApi::new(transport);
        let err = api
            .delete_file("auth", 42)
            .expect_err("missing file must surface as Result error");
        assert!(err.to_string().contains("2009"));
    }

    #[test]
    fn rename_file_parses_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("fileid".to_owned(), Value::Number(42)),
                    ("parentfolderid".to_owned(), Value::Number(9)),
                    ("name".to_owned(), Value::String("renamed.txt".to_owned())),
                ]),
            ),
        ])]);
        let api = TransferApi::new(transport);
        let response = api
            .rename_file("auth", 42, 9, "renamed.txt")
            .expect("renamefile should parse");
        assert_eq!(response.file_id, Some(42));
        assert_eq!(response.parent_folder_id, Some(9));
        assert_eq!(response.name.as_deref(), Some("renamed.txt"));
        assert!(!response.is_deleted);
    }

    #[test]
    fn rename_file_propagates_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2003)),
            (
                "error".to_owned(),
                Value::String("Access denied.".to_owned()),
            ),
        ])]);
        let api = TransferApi::new(transport);
        let err = api
            .rename_file("auth", 42, 9, "renamed.txt")
            .expect_err("access-denied rename must surface as Result error");
        assert!(err.to_string().contains("2003"));
    }
}

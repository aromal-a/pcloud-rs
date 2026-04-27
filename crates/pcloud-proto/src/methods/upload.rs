//! Upload wire methods. C citations use `pclsync/…:<line>` and are verified
//! against the legacy source; see `UPLOAD-SPEC-14042026.md`.
//!
//! Constants mirrored from `pclsync/psettings.h`:
//! * `PSYNC_COPY_BUFFER_SIZE`        = `256 * 1024`  (psettings.h:90)
//! * `PSYNC_MIN_SIZE_FOR_CHECKSUMS`  = `64 * 1024`   (psettings.h:82)
//! * `PSYNC_MAX_COPY_FROM_REQ`       = `32 * 1024 * 1024` (psettings.h:87)
//! * `PSYNC_MAX_PENDING_UPLOAD_REQS` = `16` (psettings.h:88)
//! * `PSYNC_SLEEP_ON_FAILED_UPLOAD_MS` = `2000` (psettings.h:152)
//! * `PSYNC_CHECKSUM` response field name = `"sha1"` (psettings.h:188)
//! * `PSYNC_HASH_DIGEST_HEXLEN`      = `40` chars (pssl.h:57)

// **PLATFORM:** all
// **GATING:** none (portable).

use sha1::{Digest, Sha1};

use crate::binary_api::{BinaryParam, EncodedRequest, FrameParseError, encode_request};
use crate::methods::ProtocolMethod;
use crate::redacted::RedactedProtoString;

/// 256 KiB socket write chunk (`psettings.h:90`).
pub const PSYNC_COPY_BUFFER_SIZE: usize = 256 * 1024;
/// Below this threshold a single-shot `uploadfile` is used; above, chunked
/// `upload_create`/`upload_write` (`psettings.h:82`, `pupload.c:1012`).
pub const PSYNC_MIN_SIZE_FOR_CHECKSUMS: u64 = 64 * 1024;
/// Max bytes in a single `upload_writefromfile`/`upload_writefromupload`
/// range (`psettings.h:87`, split at `pupload.c:1128`).
pub const PSYNC_MAX_COPY_FROM_REQ: u64 = 32 * 1024 * 1024;
/// Pipelined in-flight range requests (`psettings.h:88`).
pub const PSYNC_MAX_PENDING_UPLOAD_REQS: usize = 16;
/// Fixed sleep after a failed upload task (`psettings.h:152`, `pupload.c:1743`).
pub const PSYNC_SLEEP_ON_FAILED_UPLOAD_MS: u64 = 2_000;
/// Response field name for the per-upload content digest (`psettings.h:188`,
/// consumed at `pupload.c:1198`).
pub const PSYNC_CHECKSUM_FIELD: &str = "sha1";
/// Hex length of the content digest (`pssl.h:57`, used for memcmp at
/// `pupload.c:1209`).
pub const PSYNC_HASH_DIGEST_HEXLEN: usize = 40;

/// Compute a SHA1 digest of `bytes` and return it as the 40-byte lowercase
/// hex string that the pCloud API compares byte-for-byte against
/// server-reported `sha1` at `pupload.c:1209-1210` and `pupload.c:771`.
#[must_use]
pub fn upload_sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(PSYNC_HASH_DIGEST_HEXLEN);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    debug_assert_eq!(out.len(), PSYNC_HASH_DIGEST_HEXLEN);
    out
}

/// Conflict parameter shape used in `uploadfile` and `upload_save`
/// (`pupload.c:1495-1509`, also §5 of the spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictParam {
    /// `ifhash = <hash>` as PARAM_NUM. Conditional overwrite; server marks
    /// `conflicted=true` in metadata if the current remote hash differs
    /// (`pupload.c:1497-1498`).
    IfHash(u64),
    /// `ifhash = "new"` as PARAM_STR. Create-if-absent; server renames
    /// silently on collision (`pupload.c:1500-1501`).
    New,
    // TODO(bd-1du, spec §9.3, `pupload.c:1495-1509`): C always emits `ifhash`. A true
    // unconditional overwrite is not expressible at wire level and must be
    // verified on the live API before a variant is added.
}

impl ConflictParam {
    fn to_binary(&self) -> BinaryParam {
        match self {
            ConflictParam::IfHash(h) => BinaryParam::number("ifhash", *h),
            ConflictParam::New => BinaryParam::string("ifhash", "new"),
        }
    }
}

// -----------------------------------------------------------------------------
// 2.1 `uploadfile` — single-shot upload (size ≤ 64 KiB)
// Callsite: `pupload.c:694`; request params `pupload.c:661-675`.
// -----------------------------------------------------------------------------

/// `UploadFileRequest` — upload file request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadFileRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: u64,
    /// The `filename` field (filename).
    pub filename: String,
    /// `nopartial` — C always sends `1` (`pupload.c:665`).
    pub nopartial: bool,
    /// `mtime` — local modification timestamp (`pupload.c:670`).
    pub mtime: u64,
    /// `ctime` — optional birthtime (`pupload.c:668`, guarded by
    /// `PSYNC_HAS_BIRTHTIME`).
    pub ctime: Option<u64>,
    /// The `conflict` field (conflict).
    pub conflict: ConflictParam,
    /// Raw file bytes length appended to the frame
    /// (`papi_send(..., fsize, 0)` at `pupload.c:694-695`).
    pub body_len: u64,
}

impl UploadFileRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "uploadfile"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        // auth, folderid, filename, nopartial, timeformat, [ctime], mtime, ifhash
        let cap = 7 + usize::from(self.ctime.is_some());
        let mut out = Vec::with_capacity(cap);
        out.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        out.push(BinaryParam::number("folderid", self.parent_folder_id));
        out.push(BinaryParam::string("filename", self.filename.as_str()));
        out.push(BinaryParam::bool("nopartial", self.nopartial));
        out.push(BinaryParam::string("timeformat", "timestamp"));
        if let Some(ctime) = self.ctime {
            out.push(BinaryParam::number("ctime", ctime));
        }
        out.push(BinaryParam::number("mtime", self.mtime));
        out.push(self.conflict.to_binary());
        out
    }

    /// Encode, declaring the `body_len` trailing-bytes segment like C's
    /// `papi_send(..., fsize, 0)` at `pupload.c:694`.
    pub fn encode_with_body(&self) -> Result<EncodedRequest, FrameParseError> {
        encode_request(self.command_name(), &self.params(), Some(self.body_len))
    }
}

// -----------------------------------------------------------------------------
// 2.2 `upload_create` — already existed; kept verbatim.
// -----------------------------------------------------------------------------

/// `UploadCreateRequest` — upload create request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadCreateRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: u64,
    /// The `file_name` field (file name).
    pub file_name: String,
    /// The `file_size` field (file size).
    pub file_size: u64,
    /// Optional client-generated idempotency key (audit-06 H-4.2).
    ///
    /// When present, the daemon emits an extra `idempotencykey` parameter
    /// so a network retry of `upload_create → upload_write → upload_save`
    /// cannot produce a double-write — both the original and the retried
    /// invocation carry the same key, so the server can short-circuit
    /// duplicate session creation. The default `None` preserves the
    /// pre-audit wire format for older callers.
    pub idempotency_key: Option<String>,
}

impl UploadCreateRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "upload_create"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(5);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("folderid", self.parent_folder_id));
        params.push(BinaryParam::string("name", self.file_name.as_str()));
        params.push(BinaryParam::number("filesize", self.file_size));
        if let Some(key) = self.idempotency_key.as_deref() {
            params.push(BinaryParam::string("idempotencykey", key));
        }
        params
    }
}

impl ProtocolMethod for UploadCreateRequest {
    fn command_name(&self) -> &'static str {
        UploadCreateRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        UploadCreateRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// 2.3 `upload_write` — byte range at explicit offset (`pupload.c:811`).
// Already existed; kept as-is.
// -----------------------------------------------------------------------------

/// `UploadWriteRequest` — upload write request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadWriteRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `upload_id` field (upload id).
    pub upload_id: u64,
    /// The `upload_offset` field (upload offset).
    pub upload_offset: u64,
    /// The `chunk_id` field (chunk id).
    pub chunk_id: u64,
    /// Optional client-generated idempotency key matching the one supplied
    /// to [`UploadCreateRequest`] (audit-06 H-4.2). Allows the server to
    /// dedupe a retried `upload_write` against the same upload session.
    pub idempotency_key: Option<String>,
}

impl UploadWriteRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "upload_write"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(5);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("uploadoffset", self.upload_offset));
        params.push(BinaryParam::number("id", self.chunk_id));
        params.push(BinaryParam::number("uploadid", self.upload_id));
        if let Some(key) = self.idempotency_key.as_deref() {
            params.push(BinaryParam::string("idempotencykey", key));
        }
        params
    }

    /// `encode_with_body` — encode with body.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn encode_with_body(&self, body_len: u64) -> Result<EncodedRequest, FrameParseError> {
        encode_request(self.command_name(), &self.params(), Some(body_len))
    }
}

// -----------------------------------------------------------------------------
// 2.4 `upload_writefromfile` — server-side copy from remote file
// (`pupload.c:843-859`).
// -----------------------------------------------------------------------------

/// `UploadWriteFromFileRequest` — upload write from file request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadWriteFromFileRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `upload_id` field (upload id).
    pub upload_id: u64,
    /// The `upload_offset` field (upload offset).
    pub upload_offset: u64,
    /// The `chunk_id` field (chunk id).
    pub chunk_id: u64,
    /// The `file_id` field (file id).
    pub file_id: u64,
    /// The `hash` field (hash).
    pub hash: u64,
    /// Source offset inside the remote file (`pupload.c:851`).
    pub source_offset: u64,
    /// Byte count — must be ≤ `PSYNC_MAX_COPY_FROM_REQ`; splitting is the
    /// caller's responsibility (`pupload.c:1125-1131`).
    pub count: u64,
    /// Optional client-generated idempotency key matching the one supplied
    /// to [`UploadCreateRequest`] (audit-06 H-4.2). Server-side copies are
    /// idempotent on the source `(fileid, hash)` already, but the key
    /// allows the daemon to dedupe a retried server-side copy against the
    /// same upload session if the network drops mid-call.
    pub idempotency_key: Option<String>,
}

impl UploadWriteFromFileRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "upload_writefromfile"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(9);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("uploadoffset", self.upload_offset));
        params.push(BinaryParam::number("id", self.chunk_id));
        params.push(BinaryParam::number("uploadid", self.upload_id));
        params.push(BinaryParam::number("fileid", self.file_id));
        params.push(BinaryParam::number("hash", self.hash));
        params.push(BinaryParam::number("offset", self.source_offset));
        params.push(BinaryParam::number("count", self.count));
        if let Some(key) = self.idempotency_key.as_deref() {
            params.push(BinaryParam::string("idempotencykey", key));
        }
        params
    }
}

impl ProtocolMethod for UploadWriteFromFileRequest {
    fn command_name(&self) -> &'static str {
        UploadWriteFromFileRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        UploadWriteFromFileRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// 2.6 `upload_info` — final size + sha1 verification (`pupload.c:881-889`,
// response consumed at `pupload.c:1193-1213`).
// -----------------------------------------------------------------------------

/// `UploadInfoRequest` — upload info request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadInfoRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `upload_id` field (upload id).
    pub upload_id: u64,
    /// Client-assigned correlation id (`pupload.c:884`).
    pub chunk_id: u64,
}

impl UploadInfoRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "upload_info"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("uploadid", self.upload_id));
        params.push(BinaryParam::number("id", self.chunk_id));
        params
    }
}

impl ProtocolMethod for UploadInfoRequest {
    fn command_name(&self) -> &'static str {
        UploadInfoRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        UploadInfoRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// 2.7 `upload_save` — commit (`pupload.c:891-918`). Already existed; extended
// to carry optional `ctime` and conflict param per spec §2.7.
// -----------------------------------------------------------------------------

/// `UploadSaveRequest` — upload save request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadSaveRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: u64,
    /// The `file_name` field (file name).
    pub file_name: String,
    /// The `upload_id` field (upload id).
    pub upload_id: u64,
    /// The `modified_at_unix` field (modified at unix).
    pub modified_at_unix: u64,
    /// The `ctime` field (ctime).
    pub ctime: Option<u64>,
    /// The `conflict` field (conflict).
    pub conflict: Option<ConflictParam>,
    /// Optional client-generated idempotency key matching the one supplied
    /// to [`UploadCreateRequest`] (audit-06 H-4.2). Lets the server reject
    /// a retried `upload_save` with the same key after the original
    /// committed, preventing a duplicate entry.
    pub idempotency_key: Option<String>,
}

impl UploadSaveRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "upload_save"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        // auth, folderid, name, uploadid, timeformat, [ctime], mtime,
        // [ifhash], [idempotencykey]
        let cap = 6
            + usize::from(self.ctime.is_some())
            + usize::from(self.conflict.is_some())
            + usize::from(self.idempotency_key.is_some());
        let mut out = Vec::with_capacity(cap);
        out.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        out.push(BinaryParam::number("folderid", self.parent_folder_id));
        out.push(BinaryParam::string("name", self.file_name.as_str()));
        out.push(BinaryParam::number("uploadid", self.upload_id));
        out.push(BinaryParam::string("timeformat", "timestamp"));
        if let Some(ctime) = self.ctime {
            out.push(BinaryParam::number("ctime", ctime));
        }
        out.push(BinaryParam::number("mtime", self.modified_at_unix));
        if let Some(conflict) = &self.conflict {
            out.push(conflict.to_binary());
        }
        if let Some(key) = self.idempotency_key.as_deref() {
            out.push(BinaryParam::string("idempotencykey", key));
        }
        out
    }
}

impl ProtocolMethod for UploadSaveRequest {
    fn command_name(&self) -> &'static str {
        UploadSaveRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        UploadSaveRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// 2.8 `upload_delete` — abort/cleanup (`pupload.c:1281-1286`).
// -----------------------------------------------------------------------------

/// `UploadDeleteRequest` — upload delete request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadDeleteRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `upload_id` field (upload id).
    pub upload_id: u64,
}

impl UploadDeleteRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "upload_delete"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("uploadid", self.upload_id));
        params
    }
}

impl ProtocolMethod for UploadDeleteRequest {
    fn command_name(&self) -> &'static str {
        UploadDeleteRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        UploadDeleteRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// 2.9 `upload_blockchecksums` — resume-block index over an open uploadid
// (`pnetlibs.c:1676-1687`). JSON response with `result`; binary trailer is
// read separately from the same socket.
// -----------------------------------------------------------------------------

/// `UploadBlockChecksumsRequest` — upload block checksums request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadBlockChecksumsRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `upload_id` field (upload id).
    pub upload_id: u64,
}

impl UploadBlockChecksumsRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "upload_blockchecksums"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("uploadid", self.upload_id));
        params
    }
}

impl ProtocolMethod for UploadBlockChecksumsRequest {
    fn command_name(&self) -> &'static str {
        UploadBlockChecksumsRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        UploadBlockChecksumsRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// 2.10 `getchecksumlink` — HTTP blockchecksum fetch for committed files
// (`pnetlibs.c:1588-1605`).
// -----------------------------------------------------------------------------

/// `GetChecksumLinkRequest` — get checksum link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetChecksumLinkRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `file_id` field (file id).
    pub file_id: u64,
    /// The `hash` field (hash).
    pub hash: u64,
}

impl GetChecksumLinkRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "getchecksumlink"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("fileid", self.file_id));
        params.push(BinaryParam::number("hash", self.hash));
        params
    }
}

impl ProtocolMethod for GetChecksumLinkRequest {
    fn command_name(&self) -> &'static str {
        GetChecksumLinkRequest::command_name(self)
    }
    fn params(&self) -> Vec<BinaryParam> {
        GetChecksumLinkRequest::params(self)
    }
}

// -----------------------------------------------------------------------------
// Binary trailer decoder for `upload_blockchecksums` / `getchecksumlink`.
// Header: `pnetlibs.c:100-104`; per-block: `pnetlibs.c:79-82`.
// -----------------------------------------------------------------------------

/// `psync_block_checksum_header`: 24 bytes, host byte order. The C client
/// assumes little-endian (`pnetlibs.c:100-104`).
///
/// TODO(bd-1du, spec §9.2): live-API verification required before trusting this
/// layout on big-endian targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChecksumHeader {
    /// The `filesize` field (filesize).
    pub filesize: u64,
    /// The `blocksize` field (blocksize).
    pub blocksize: u32,
    /// The `_reserved` field ( reserved).
    pub _reserved: [u8; 12],
}

impl BlockChecksumHeader {
    /// `ENCODED_LEN` — encoded len.
    pub const ENCODED_LEN: usize = 24;

    /// `decode` — decode.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::ENCODED_LEN {
            return None;
        }
        let filesize = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let blocksize = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let mut reserved = [0u8; 12];
        reserved.copy_from_slice(&bytes[12..24]);
        Some(Self {
            filesize,
            blocksize,
            _reserved: reserved,
        })
    }

    /// `block_count` — block count.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn block_count(&self) -> u64 {
        if self.blocksize == 0 {
            return 0;
        }
        self.filesize.div_ceil(u64::from(self.blocksize))
    }
}

/// `psync_block_checksum`: 20-byte SHA1 + 4-byte adler32 (little-endian u32
/// per C host-order assumption; `pnetlibs.c:79-82`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChecksum {
    /// The `sha1` field (sha1).
    pub sha1: [u8; 20],
    /// The `adler` field (adler).
    pub adler: u32,
}

impl BlockChecksum {
    /// `ENCODED_LEN` — encoded len.
    pub const ENCODED_LEN: usize = 24;

    /// `decode` — decode.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::ENCODED_LEN {
            return None;
        }
        let mut sha1 = [0u8; 20];
        sha1.copy_from_slice(&bytes[0..20]);
        let adler = u32::from_le_bytes(bytes[20..24].try_into().ok()?);
        Some(Self { sha1, adler })
    }
}

/// Decode `count` consecutive `BlockChecksum` entries.
pub fn decode_block_checksums(bytes: &[u8], count: usize) -> Option<Vec<BlockChecksum>> {
    let needed = count.checked_mul(BlockChecksum::ENCODED_LEN)?;
    if bytes.len() < needed {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = i * BlockChecksum::ENCODED_LEN;
        out.push(BlockChecksum::decode(
            &bytes[off..off + BlockChecksum::ENCODED_LEN],
        )?);
    }
    Some(out)
}

// -----------------------------------------------------------------------------
// §6.1 Error classifier — keys off pCloud `result` codes
// (`pnetlibs.c:341-354`, `psync_handle_api_result`).
// -----------------------------------------------------------------------------

/// `UploadErrorClass` — upload error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadErrorClass {
    /// `result == 2000`: bad/expired login. C sets `PSTATUS_AUTH_BADLOGIN`
    /// then returns `PSYNC_NET_TEMPFAIL`. We surface `Auth` so callers can
    /// re-authenticate explicitly.
    Auth,
    /// Fatal for this task — do not retry (`pnetlibs.c:343-354`,
    /// codes 2003 / 2005 / 2007 / 2009 / 2029 / 2067 / 5002).
    PermFail,
    /// Retryable (all other nonzero results; network failures).
    TempFail,
}

impl UploadErrorClass {
    /// Classify a pCloud `result` number per `psync_handle_api_result`
    /// (`pnetlibs.c:341-354`). `result == 0` means success and returns
    /// `None`.
    #[must_use]
    pub fn classify(result: u64) -> Option<Self> {
        match result {
            0 => None,
            2000 => Some(Self::Auth),
            2003 | 2005 | 2007 | 2009 | 2029 | 2067 | 5002 => Some(Self::PermFail),
            _ => Some(Self::TempFail),
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_hex_empty_vector() {
        assert_eq!(
            upload_sha1_hex(b""),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
    }

    #[test]
    fn sha1_hex_abc_vector() {
        assert_eq!(
            upload_sha1_hex(b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn sha1_hex_long_vector() {
        // Classic FIPS-180 vector for 56 'a' repetition style check.
        let input = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            upload_sha1_hex(input),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_hex_length_is_always_40() {
        for bytes in [b"".as_ref(), b"x".as_ref(), &[0u8; 1024]] {
            assert_eq!(upload_sha1_hex(bytes).len(), PSYNC_HASH_DIGEST_HEXLEN);
        }
    }

    #[test]
    fn uploadfile_request_encodes_with_body_and_conflict_new() {
        let req = UploadFileRequest {
            auth_token: "t".into(),
            parent_folder_id: 7,
            filename: "a.bin".to_owned(),
            nopartial: true,
            mtime: 1_700_000_000,
            ctime: None,
            conflict: ConflictParam::New,
            body_len: 1024,
        };
        let encoded = req.encode_with_body().expect("uploadfile encodes");
        assert_eq!(encoded.frame.command, "uploadfile");
        // auth, folderid, filename, nopartial, timeformat, mtime, ifhash
        assert_eq!(encoded.frame.parameter_count, 7);
    }

    #[test]
    fn uploadfile_request_with_ctime_and_ifhash_counts_eight_params() {
        let req = UploadFileRequest {
            auth_token: "t".into(),
            parent_folder_id: 7,
            filename: "a.bin".to_owned(),
            nopartial: true,
            mtime: 1_700_000_000,
            ctime: Some(1_600_000_000),
            conflict: ConflictParam::IfHash(0xDEAD_BEEF),
            body_len: 0,
        };
        let encoded = req.encode_with_body().expect("encode");
        assert_eq!(encoded.frame.parameter_count, 8);
    }

    #[test]
    fn upload_delete_encodes() {
        let req = UploadDeleteRequest {
            auth_token: "t".into(),
            upload_id: 42,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "upload_delete");
        assert_eq!(encoded.frame.parameter_count, 2);
    }

    #[test]
    fn upload_info_encodes() {
        let req = UploadInfoRequest {
            auth_token: "t".into(),
            upload_id: 7,
            chunk_id: 3,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "upload_info");
        assert_eq!(encoded.frame.parameter_count, 3);
    }

    #[test]
    fn upload_blockchecksums_encodes() {
        let req = UploadBlockChecksumsRequest {
            auth_token: "t".into(),
            upload_id: 9,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "upload_blockchecksums");
        assert_eq!(encoded.frame.parameter_count, 2);
    }

    #[test]
    fn getchecksumlink_encodes() {
        let req = GetChecksumLinkRequest {
            auth_token: "t".into(),
            file_id: 5,
            hash: 99,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "getchecksumlink");
        assert_eq!(encoded.frame.parameter_count, 3);
    }

    #[test]
    fn upload_writefromfile_encodes_with_eight_params() {
        let req = UploadWriteFromFileRequest {
            auth_token: "t".into(),
            upload_id: 1,
            upload_offset: 0,
            chunk_id: 2,
            file_id: 3,
            hash: 4,
            source_offset: 5,
            count: 6,
            idempotency_key: None,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "upload_writefromfile");
        assert_eq!(encoded.frame.parameter_count, 8);
    }

    // audit-06 H-4.2: when an idempotency key is supplied,
    // `upload_writefromfile` carries one additional parameter.
    #[test]
    fn upload_writefromfile_idempotent_encodes_with_nine_params() {
        let req = UploadWriteFromFileRequest {
            auth_token: "t".into(),
            upload_id: 1,
            upload_offset: 0,
            chunk_id: 2,
            file_id: 3,
            hash: 4,
            source_offset: 5,
            count: 6,
            idempotency_key: Some("01H_test_key".to_owned()),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.parameter_count, 9);
    }

    #[test]
    fn upload_save_without_optional_fields_counts_six_params() {
        let req = UploadSaveRequest {
            auth_token: "t".into(),
            parent_folder_id: 1,
            file_name: "a".into(),
            upload_id: 2,
            modified_at_unix: 3,
            ctime: None,
            conflict: None,
            idempotency_key: None,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.parameter_count, 6);
    }

    #[test]
    fn upload_save_with_ctime_and_conflict_counts_eight_params() {
        let req = UploadSaveRequest {
            auth_token: "t".into(),
            parent_folder_id: 1,
            file_name: "a".into(),
            upload_id: 2,
            modified_at_unix: 3,
            ctime: Some(4),
            conflict: Some(ConflictParam::IfHash(99)),
            idempotency_key: None,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.parameter_count, 8);
    }

    // audit-06 H-4.2: a save with all of ctime + conflict + idempotency
    // key must encode 9 parameters.
    #[test]
    fn upload_save_with_idempotency_key_counts_nine_params() {
        let req = UploadSaveRequest {
            auth_token: "t".into(),
            parent_folder_id: 1,
            file_name: "a".into(),
            upload_id: 2,
            modified_at_unix: 3,
            ctime: Some(4),
            conflict: Some(ConflictParam::IfHash(99)),
            idempotency_key: Some("01H_save_key".to_owned()),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.parameter_count, 9);
    }

    // audit-06 H-4.2: upload_create with an idempotency key carries 5
    // parameters; without one, 4 (the legacy default).
    #[test]
    fn upload_create_idempotent_encodes_with_five_params() {
        let req = UploadCreateRequest {
            auth_token: "t".into(),
            parent_folder_id: 9,
            file_name: "a.bin".to_owned(),
            file_size: 1024,
            idempotency_key: Some("01H_create_key".to_owned()),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.parameter_count, 5);
    }

    // audit-06 H-4.2: upload_write with an idempotency key carries 5
    // parameters (auth + uploadoffset + id + uploadid + idempotencykey).
    #[test]
    fn upload_write_idempotent_encodes_with_five_params() {
        let req = UploadWriteRequest {
            auth_token: "t".into(),
            upload_id: 1,
            upload_offset: 0,
            chunk_id: 2,
            idempotency_key: Some("01H_write_key".to_owned()),
        };
        let encoded = req.encode_with_body(0).expect("encode");
        assert_eq!(encoded.frame.parameter_count, 5);
    }

    #[test]
    fn block_checksum_header_decodes_little_endian() {
        let mut bytes = [0u8; 24];
        bytes[0..8].copy_from_slice(&1_048_576u64.to_le_bytes());
        bytes[8..12].copy_from_slice(&4_096u32.to_le_bytes());
        let hdr = BlockChecksumHeader::decode(&bytes).expect("decode");
        assert_eq!(hdr.filesize, 1_048_576);
        assert_eq!(hdr.blocksize, 4_096);
        assert_eq!(hdr.block_count(), 256);
    }

    #[test]
    fn block_checksum_header_rejects_short_input() {
        assert!(BlockChecksumHeader::decode(&[0u8; 23]).is_none());
    }

    #[test]
    fn block_checksum_entry_decodes() {
        let mut bytes = [0u8; 24];
        for (i, b) in bytes.iter_mut().take(20).enumerate() {
            *b = i as u8;
        }
        bytes[20..24].copy_from_slice(&0xAABB_CCDDu32.to_le_bytes());
        let cs = BlockChecksum::decode(&bytes).expect("decode");
        assert_eq!(cs.sha1[0], 0);
        assert_eq!(cs.sha1[19], 19);
        assert_eq!(cs.adler, 0xAABB_CCDD);
    }

    #[test]
    fn block_checksums_decode_multiple() {
        let mut bytes = Vec::new();
        for i in 0..3u32 {
            let mut entry = [0u8; 24];
            entry[0] = i as u8;
            entry[20..24].copy_from_slice(&i.to_le_bytes());
            bytes.extend_from_slice(&entry);
        }
        let out = decode_block_checksums(&bytes, 3).expect("decode");
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].adler, 2);
    }

    #[test]
    fn block_checksums_decode_rejects_truncated() {
        assert!(decode_block_checksums(&[0u8; 23], 1).is_none());
    }

    #[test]
    fn error_classifier_zero_is_ok() {
        assert_eq!(UploadErrorClass::classify(0), None);
    }

    #[test]
    fn error_classifier_2000_is_auth() {
        assert_eq!(
            UploadErrorClass::classify(2000),
            Some(UploadErrorClass::Auth)
        );
    }

    #[test]
    fn error_classifier_perm_fail_codes() {
        for code in [2003u64, 2005, 2007, 2009, 2029, 2067, 5002] {
            assert_eq!(
                UploadErrorClass::classify(code),
                Some(UploadErrorClass::PermFail),
                "code {code} should be PermFail",
            );
        }
    }

    #[test]
    fn error_classifier_default_is_tempfail() {
        for code in [1u64, 1000, 2001, 2999, 9999] {
            assert_eq!(
                UploadErrorClass::classify(code),
                Some(UploadErrorClass::TempFail),
                "code {code} should fall through to TempFail",
            );
        }
    }

    #[test]
    fn constants_match_spec_values() {
        assert_eq!(PSYNC_COPY_BUFFER_SIZE, 256 * 1024);
        assert_eq!(PSYNC_MIN_SIZE_FOR_CHECKSUMS, 64 * 1024);
        assert_eq!(PSYNC_MAX_COPY_FROM_REQ, 32 * 1024 * 1024);
        assert_eq!(PSYNC_MAX_PENDING_UPLOAD_REQS, 16);
        assert_eq!(PSYNC_SLEEP_ON_FAILED_UPLOAD_MS, 2_000);
        assert_eq!(PSYNC_CHECKSUM_FIELD, "sha1");
        assert_eq!(PSYNC_HASH_DIGEST_HEXLEN, 40);
    }
}

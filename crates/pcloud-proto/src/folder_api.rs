//! Folder protocol client: listing, metadata, create/rename/delete, and
//! typed mutation-error classification. Consumed by
//! `pcloud-backends::folder_backend` and `pcloud-engine`.
//!
//! ## Role in the request pipeline
//!
//! Wraps the pCloud folder command family (`listfolder`,
//! `createfolder`, `renamefolder`, `deletefolderrecursive`, and
//! relatives). Each method encodes a typed request, dispatches it
//! through the supplied transport, and projects the response into
//! a domain type ([`RemoteFolderListing`], [`RemoteFolderInfo`],
//! …). Mutation failures are classified into
//! [`FsMutationErrorClass`] so higher layers can make retry /
//! surface decisions without string-matching server messages.
//!
//! ## Security considerations
//!
//! - Paths returned by the server are UTF-8 but otherwise
//!   untrusted; callers must not interpolate them into shell
//!   commands or untrusted-destination paths without validation.
//! - Folder ids drive mutations; the caller is responsible for
//!   authenticating that each id belongs to the active session.
//! - Delete-recursive is intentionally *not* idempotent-by-default
//!   at this layer — the caller must make that decision.
//!
//! Portable; no platform gating.

use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHint, ApiServerHintConsumer, ProtocolTransport},
    methods::folder::{
        CreateFolderRequest, DeleteFolderRecursiveRequest, DeleteFolderRequest,
        ListFolderByPathRequest, RenameFolderRequest,
    },
    response::{HashView, Value},
};

pub use crate::methods::folder::FsMutationErrorClass;

/// `FolderApi` — folder api.
#[derive(Debug)]
pub struct FolderApi<T> {
    transport: T,
}

/// `FolderApiError` — folder api error.
#[derive(Debug, Error)]
pub enum FolderApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("folder method returned non-zero result code {result} ({message:?})")]
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

/// `RemoteFolderInfo` — remote folder info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolderInfo {
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
    /// The `path` field (path).
    pub path: String,
    /// The `name` field (name).
    pub name: String,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
}

/// A single entry inside a `listfolder` response's `contents` array.
///
/// This mirrors just enough of the pCloud metadata shape to drive
/// path-to-id resolution for public-link traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolderEntry {
    /// The `name` field (name).
    pub name: String,
    /// The `is_folder` field (is folder).
    pub is_folder: bool,
    /// The `folder_id` field (folder id).
    pub folder_id: Option<u64>,
    /// The `file_id` field (file id).
    pub file_id: Option<u64>,
    /// `userid` from listfolder metadata. Server may omit this for
    /// "ismine" rows; resolution callers can cross-check via
    /// [`Self::is_mine`].
    pub owner_user_id: Option<u64>,
    /// `ismine` flag from listfolder metadata. Mirrors the C `pdiff`
    /// ownership branch (pdiff.c:857).
    pub is_mine: bool,
    /// `encrypted` flag from listfolder metadata.
    pub encrypted: bool,
    /// `isshared` flag from listfolder metadata.
    pub is_shared: bool,
    /// `PSYNC_PERM_*` bitmap derived from per-entry `canread`,
    /// `canmodify`, `cancreate`, `candelete`, `canmanage` flags. Mirrors
    /// `pfileops_get_perms` (pfileops.c:91). `None` when fields absent.
    pub permissions: Option<u32>,
    /// File size in bytes (`size` field in listfolder metadata). Always
    /// `None` for folders; generally `Some` for files but the server may
    /// omit it for some entry types.
    pub size: Option<u64>,
    /// Unix epoch seconds of last modification (`modified` in listfolder
    /// metadata). Best-effort: server omits for some entry types.
    pub modified: Option<u64>,
}

/// Result of a `createfolder` / `createfolderifnotexists` call. Mirrors
/// the metadata shape pCloud returns and reports whether the folder was
/// freshly created (`true`) or already existed and was returned by the
/// idempotent `createfolderifnotexists` path (`false`). For the
/// non-idempotent `createfolder` path, `created` is always `true` on a
/// successful response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFolderResponse {
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
    /// The `name` field (name).
    pub name: String,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: Option<u64>,
    /// The `created` field (created).
    pub created: bool,
}

/// Full `listfolder` response including the parent folder id and every
/// direct child entry. Used by the path resolver to distinguish folder vs
/// file, detect missing or ambiguous segments, and avoid fabricating ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolderListing {
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
    /// The `path` field (path).
    pub path: String,
    /// The `name` field (name).
    pub name: String,
    /// The `entries` field (entries).
    pub entries: Vec<RemoteFolderEntry>,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
    /// `userid` of the listed folder itself.
    pub owner_user_id: Option<u64>,
    /// `ismine` of the listed folder.
    pub is_mine: bool,
    /// `encrypted` of the listed folder.
    pub encrypted: bool,
    /// `isshared` of the listed folder.
    pub is_shared: bool,
    /// `PSYNC_PERM_*` bitmap of the listed folder.
    pub permissions: Option<u32>,
}

impl<T> FolderApi<T> {
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

impl<T> FolderApi<T>
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

    /// `list_folder_by_path` — list folder by path.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_folder_by_path(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<RemoteFolderInfo, FolderApiError<T::Error>> {
        let request = ListFolderByPathRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            path: path.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(FolderApiError::Transport)?;
        let hash = response.as_hash().ok_or(FolderApiError::Malformed(
            "listfolder response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        let metadata = hash.get_hash("metadata").ok_or(FolderApiError::Malformed(
            "listfolder response missing metadata",
        ))?;

        let folder = RemoteFolderInfo {
            folder_id: metadata
                .get_number("folderid")
                .or_else(|| metadata.get_number("id"))
                .ok_or(FolderApiError::Malformed(
                    "listfolder metadata missing folderid",
                ))?,
            path: request.path.clone(),
            name: metadata.get_string("name").unwrap_or_default().to_owned(),
            api_server: extract_api_server_hint(hash),
        };
        if let Some(hint) = folder.api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(folder)
    }

    /// Like [`Self::list_folder_by_path`] but also returns the direct
    /// children (name, kind, folder/file id) as typed entries.
    ///
    /// This is used by the daemon's path resolver to distinguish file vs
    /// folder and to detect ambiguous or missing segments without
    /// fabricating identifiers.
    pub fn list_folder_contents_by_path(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<RemoteFolderListing, FolderApiError<T::Error>> {
        let request = ListFolderByPathRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            path: path.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(FolderApiError::Transport)?;
        let hash = response.as_hash().ok_or(FolderApiError::Malformed(
            "listfolder response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        let metadata = hash.get_hash("metadata").ok_or(FolderApiError::Malformed(
            "listfolder response missing metadata",
        ))?;
        let folder_id = metadata
            .get_number("folderid")
            .or_else(|| metadata.get_number("id"))
            .ok_or(FolderApiError::Malformed(
                "listfolder metadata missing folderid",
            ))?;
        let name = metadata.get_string("name").unwrap_or_default().to_owned();
        let contents = metadata.get_array("contents").unwrap_or(&[]);
        let mut entries = Vec::with_capacity(contents.len());
        for entry in contents {
            let entry_hash = entry.as_hash().ok_or(FolderApiError::Malformed(
                "listfolder contents entry was not a hash",
            ))?;
            let entry_name = entry_hash
                .get_string("name")
                .ok_or(FolderApiError::Malformed(
                    "listfolder contents entry missing name",
                ))?
                .to_owned();
            let is_folder = entry_hash.get_bool("isfolder").unwrap_or(false);
            let folder_id_opt = entry_hash.get_number("folderid");
            let file_id_opt = entry_hash.get_number("fileid");
            let entry_meta = extract_metadata_facets(entry_hash);
            let size = if is_folder {
                None
            } else {
                entry_hash
                    .get_number("size")
                    .or_else(|| entry_hash.get_string("size").and_then(|s| s.parse().ok()))
            };
            let modified = entry_hash.get_number("modified").or_else(|| {
                entry_hash
                    .get_string("modified")
                    .and_then(parse_modified_string)
            });
            entries.push(RemoteFolderEntry {
                name: entry_name,
                is_folder,
                folder_id: folder_id_opt,
                file_id: file_id_opt,
                owner_user_id: entry_meta.owner_user_id,
                is_mine: entry_meta.is_mine,
                encrypted: entry_meta.encrypted,
                is_shared: entry_meta.is_shared,
                permissions: entry_meta.permissions,
                size,
                modified,
            });
        }

        let parent_meta = extract_metadata_facets(metadata);
        let listing = RemoteFolderListing {
            folder_id,
            path: request.path.clone(),
            name,
            entries,
            api_server: extract_api_server_hint(hash),
            owner_user_id: parent_meta.owner_user_id,
            is_mine: parent_meta.is_mine,
            encrypted: parent_meta.encrypted,
            is_shared: parent_meta.is_shared,
            permissions: parent_meta.permissions,
        };
        if let Some(hint) = listing.api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(listing)
    }

    /// Create a remote folder under `parent_folder_id` with leaf `name`.
    /// Mirrors the C `psync_create_remote_folder` call
    /// (`pclsync/psynclib.c:1020`) which uses the `createfolder` endpoint.
    pub fn create_folder(
        &self,
        auth_token: impl Into<String>,
        parent_folder_id: u64,
        name: impl Into<String>,
    ) -> Result<CreateFolderResponse, FolderApiError<T::Error>> {
        let request = CreateFolderRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            parent_folder_id: Some(parent_folder_id),
            name: name.into(),
            path: String::new(),
            folder_exists_ok: false,
        };
        self.execute_create_folder(request)
    }

    /// Create a remote folder by absolute path. Mirrors the C
    /// `psync_create_remote_folder_by_path` call
    /// (`pclsync/psynclib.c:1006`).
    pub fn create_folder_by_path(
        &self,
        auth_token: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<CreateFolderResponse, FolderApiError<T::Error>> {
        let request = CreateFolderRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            parent_folder_id: None,
            name: String::new(),
            path: path.into(),
            folder_exists_ok: false,
        };
        self.execute_create_folder(request)
    }

    /// Idempotent `createfolderifnotexists` variant. The pCloud backend
    /// returns the existing folder on conflict; the caller can inspect
    /// `CreateFolderResponse::created` to distinguish the two outcomes.
    pub fn create_folder_if_not_exists(
        &self,
        auth_token: impl Into<String>,
        parent_folder_id: Option<u64>,
        name: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<CreateFolderResponse, FolderApiError<T::Error>> {
        let request = CreateFolderRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            parent_folder_id,
            name: name.into(),
            path: path.into(),
            folder_exists_ok: true,
        };
        self.execute_create_folder(request)
    }

    /// Delete a remote folder non-recursively. Mirrors
    /// `psync_send_task_rmdir` (`pclsync/pfsupload_send.c:60-72`). The
    /// backend rejects non-empty folders, matching POSIX `rmdir`.
    pub fn delete_folder(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
    ) -> Result<RenamedFolderResponse, FolderApiError<T::Error>> {
        let request = DeleteFolderRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(FolderApiError::Transport)?;
        let hash = response.as_hash().ok_or(FolderApiError::Malformed(
            "deletefolder response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(parse_mutated_folder(hash))
    }

    /// Delete a remote folder recursively. Mirrors `task_deletefolderrec`
    /// (`pclsync/pupload.c:1663-1675`).
    pub fn delete_folder_recursive(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
    ) -> Result<RenamedFolderResponse, FolderApiError<T::Error>> {
        let request = DeleteFolderRecursiveRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(FolderApiError::Transport)?;
        let hash = response.as_hash().ok_or(FolderApiError::Malformed(
            "deletefolderrecursive response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(parse_mutated_folder(hash))
    }

    /// Rename and/or move a remote folder. Mirrors `task_renameremotefolder`
    /// (`pclsync/pupload.c:388-438`). A pure rename passes the existing
    /// parent as `to_folder_id`.
    pub fn rename_folder(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
        to_folder_id: u64,
        to_name: impl Into<String>,
    ) -> Result<RenamedFolderResponse, FolderApiError<T::Error>> {
        let request = RenameFolderRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
            to_folder_id,
            to_name: to_name.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(FolderApiError::Transport)?;
        let hash = response.as_hash().ok_or(FolderApiError::Malformed(
            "renamefolder response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(parse_mutated_folder(hash))
    }

    fn execute_create_folder(
        &self,
        request: CreateFolderRequest,
    ) -> Result<CreateFolderResponse, FolderApiError<T::Error>> {
        let folder_exists_ok = request.folder_exists_ok;
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(FolderApiError::Transport)?;
        let hash = response.as_hash().ok_or(FolderApiError::Malformed(
            "createfolder response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        let metadata = hash.get_hash("metadata").ok_or(FolderApiError::Malformed(
            "createfolder response missing metadata",
        ))?;
        let folder_id = metadata
            .get_number("folderid")
            .or_else(|| metadata.get_number("id"))
            .ok_or(FolderApiError::Malformed(
                "createfolder metadata missing folderid",
            ))?;
        let name = metadata.get_string("name").unwrap_or_default().to_owned();
        let parent_folder_id = metadata.get_number("parentfolderid");
        // `created` is false only when the idempotent path returned an
        // already-existing folder. The pCloud server signals this via
        // `created: true|false` in the top-level response hash; some
        // historical responses omit the field entirely on a fresh
        // creation, so missing-but-non-idempotent defaults to `true`.
        let created = if folder_exists_ok {
            hash.get_bool("created").unwrap_or(true)
        } else {
            true
        };

        if let Some(hint) = extract_api_server_hint(hash) {
            self.transport.apply_api_server_hint(&hint.binapi);
        }

        Ok(CreateFolderResponse {
            folder_id,
            name,
            parent_folder_id,
            created,
        })
    }
}

/// Metadata snippet returned by `deletefolder`,
/// `deletefolderrecursive`, and `renamefolder`. Every field is best-effort:
/// the C server consistently returns folder metadata on success but
/// older responses occasionally omit `parentfolderid`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenamedFolderResponse {
    /// The `folder_id` field (folder id).
    pub folder_id: Option<u64>,
    /// The `parent_folder_id` field (parent folder id).
    pub parent_folder_id: Option<u64>,
    /// The `name` field (name).
    pub name: Option<String>,
    /// The `is_deleted` field (is deleted).
    pub is_deleted: bool,
}

/// Parse a pCloud `modified`/`created` date string into Unix epoch seconds.
///
/// pCloud's listfolder returns dates in RFC2822 form when `timeformat` is
/// not set to `"timestamp"`, e.g. `"Wed, 02 Apr 2024 13:54:31 +0000"`.
/// Returns `None` if the input doesn't match. Pure-Rust, no chrono dep.
fn parse_modified_string(s: &str) -> Option<u64> {
    // Try plain-integer form first (some endpoints honor timeformat=timestamp
    // by returning a numeric string).
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    // RFC2822: "Day, DD Mon YYYY HH:MM:SS +ZZZZ"
    let trimmed = s.trim();
    let mut parts = trimmed.split_whitespace();
    let _dow = parts.next()?; // "Wed,"
    let dd: u32 = parts.next()?.parse().ok()?;
    let mon = parts.next()?;
    let yyyy: i32 = parts.next()?.parse().ok()?;
    let hms = parts.next()?;
    let tz = parts.next().unwrap_or("+0000");

    let lowercase_month = mon.to_ascii_lowercase();
    let month: u32 = match lowercase_month.get(..3)? {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let mut hms_iter = hms.split(':');
    let hh: u32 = hms_iter.next()?.parse().ok()?;
    let mm: u32 = hms_iter.next()?.parse().ok()?;
    let ss: u32 = hms_iter.next()?.parse().ok()?;
    if hms_iter.next().is_some() || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let leap_year = yyyy % 4 == 0 && (yyyy % 100 != 0 || yyyy % 400 == 0);
    let days_in_month = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if dd == 0 || dd > days_in_month {
        return None;
    }

    // Days-from-civil (Howard Hinnant) — converts (y,m,d) to days since 1970-01-01
    let yyyy = i64::from(yyyy);
    let y = if month <= 2 { yyyy - 1 } else { yyyy };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let dd = i64::from(dd);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut secs = days
        .checked_mul(86400)?
        .checked_add(i64::from(hh) * 3600)?
        .checked_add(i64::from(mm) * 60)?
        .checked_add(i64::from(ss))?;

    // tz offset: ±HHMM
    let tz = tz.as_bytes();
    if tz.len() != 5 || !tz[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let sign = match tz[0] {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    let tz_h = i64::from((tz[1] - b'0') * 10 + (tz[2] - b'0'));
    let tz_m = i64::from((tz[3] - b'0') * 10 + (tz[4] - b'0'));
    if tz_h > 23 || tz_m > 59 {
        return None;
    }
    // RFC2822 timestamp is local-with-offset; subtract offset to get UTC.
    secs = secs.checked_sub(sign * (tz_h * 3600 + tz_m * 60))?;
    u64::try_from(secs).ok()
}

fn parse_mutated_folder(hash: HashView<'_>) -> RenamedFolderResponse {
    let metadata = hash.get_hash("metadata");
    let (folder_id, parent_folder_id, name, is_deleted) = match metadata {
        Some(meta) => (
            meta.get_number("folderid")
                .or_else(|| meta.get_number("id")),
            meta.get_number("parentfolderid"),
            meta.get_string("name").map(ToOwned::to_owned),
            meta.get_bool("isdeleted").unwrap_or(false),
        ),
        None => (None, None, None, false),
    };
    RenamedFolderResponse {
        folder_id,
        parent_folder_id,
        name,
        is_deleted,
    }
}

/// `PSYNC_PERM_*` bitmap constants, mirroring `pclsync/psynclib.h:206-216`.
pub mod perm_bits {
    /// `READ` — read.
    pub const READ: u32 = 1;
    /// `CREATE` — create.
    pub const CREATE: u32 = 2;
    /// `MODIFY` — modify.
    pub const MODIFY: u32 = 4;
    /// `DELETE` — delete.
    pub const DELETE: u32 = 8;
    /// `MANAGE` — manage.
    pub const MANAGE: u32 = 16;
    /// `ALL` — all.
    pub const ALL: u32 = READ | CREATE | MODIFY | DELETE;
}

/// Subset of per-entry metadata facets (ownership, crypto, sharing,
/// permissions) extracted from a listfolder hash. Added so parent and child
/// entries share the same extraction code path.
struct MetadataFacets {
    owner_user_id: Option<u64>,
    is_mine: bool,
    encrypted: bool,
    is_shared: bool,
    permissions: Option<u32>,
}

fn extract_metadata_facets(hash: HashView<'_>) -> MetadataFacets {
    let owner_user_id = hash.get_number("userid");
    let is_mine = hash.get_bool("ismine").unwrap_or(false);
    let encrypted = hash.get_bool("encrypted").unwrap_or(false);
    let is_shared = hash.get_bool("isshared").unwrap_or(false);
    // `PSYNC_PERM_*` bitmap derived from per-entry `canread`/`canmodify`/
    // `cancreate`/`candelete`/`canmanage` (mirrors `pfileops_get_perms` at
    // pfileops.c:91). `ismine` rows always synthesize `PSYNC_PERM_ALL`
    // (pdiff.c:858). Otherwise the `can*` booleans are folded; `None` is
    // returned only when no caps were advertised at all so callers can
    // honestly distinguish "no permission" from "field omitted".
    let permissions = if is_mine {
        Some(perm_bits::ALL)
    } else {
        let mut bits: u32 = 0;
        let mut any = false;
        for (flag, bit) in [
            ("canread", perm_bits::READ),
            ("canmodify", perm_bits::MODIFY),
            ("cancreate", perm_bits::CREATE),
            ("candelete", perm_bits::DELETE),
            ("canmanage", perm_bits::MANAGE),
        ] {
            if let Some(v) = hash.get_bool(flag) {
                any = true;
                if v {
                    bits |= bit;
                }
            }
        }
        if any { Some(bits) } else { None }
    };
    MetadataFacets {
        owner_user_id,
        is_mine,
        encrypted,
        is_shared,
        permissions,
    }
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

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), FolderApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }

    Err(FolderApiError::Result {
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

    use super::FolderApi;

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
    fn list_folder_by_path_parses_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            (
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("folderid".to_owned(), Value::Number(42)),
                    ("name".to_owned(), Value::String("remote-sync".to_owned())),
                ]),
            ),
            (
                "binapi".to_owned(),
                Value::String("bineapi-eu.pcloud.com".to_owned()),
            ),
        ])]);
        let api = FolderApi::new(transport);

        let folder = api
            .list_folder_by_path("auth", "/remote-sync")
            .expect("listfolder should parse");

        assert_eq!(folder.folder_id, 42);
        assert_eq!(folder.path, "/remote-sync");
        assert_eq!(folder.name, "remote-sync");
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
    fn list_folder_by_path_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2005)),
            (
                "error".to_owned(),
                Value::String("Directory does not exist.".to_owned()),
            ),
        ])]);
        let api = FolderApi::new(transport);

        let err = api
            .list_folder_by_path("auth", "/missing")
            .expect_err("missing remote path should fail");

        assert!(err.to_string().contains("2005"));
    }

    #[test]
    fn list_folder_contents_by_path_parses_entries() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "metadata".to_owned(),
            Value::Hash(vec![
                ("folderid".to_owned(), Value::Number(10)),
                ("name".to_owned(), Value::String("Root".to_owned())),
                (
                    "contents".to_owned(),
                    Value::Array(vec![
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("docs".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(true)),
                            ("folderid".to_owned(), Value::Number(11)),
                        ]),
                        Value::Hash(vec![
                            ("name".to_owned(), Value::String("report.txt".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(false)),
                            ("fileid".to_owned(), Value::Number(42)),
                        ]),
                    ]),
                ),
            ]),
        )])]);
        let api = super::FolderApi::new(transport);

        let listing = api
            .list_folder_contents_by_path("auth", "/")
            .expect("listfolder should parse contents");

        assert_eq!(listing.folder_id, 10);
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].name, "docs");
        assert!(listing.entries[0].is_folder);
        assert_eq!(listing.entries[0].folder_id, Some(11));
        assert_eq!(listing.entries[1].name, "report.txt");
        assert!(!listing.entries[1].is_folder);
        assert_eq!(listing.entries[1].file_id, Some(42));
    }

    #[test]
    fn create_folder_parses_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "metadata".to_owned(),
            Value::Hash(vec![
                ("folderid".to_owned(), Value::Number(123)),
                ("parentfolderid".to_owned(), Value::Number(11)),
                ("name".to_owned(), Value::String("Reports".to_owned())),
            ]),
        )])]);
        let api = FolderApi::new(transport);

        let response = api
            .create_folder("token", 11, "Reports")
            .expect("create_folder should parse");

        assert_eq!(response.folder_id, 123);
        assert_eq!(response.parent_folder_id, Some(11));
        assert_eq!(response.name, "Reports");
        assert!(response.created);
    }

    #[test]
    fn create_folder_by_path_parses_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "metadata".to_owned(),
            Value::Hash(vec![
                ("folderid".to_owned(), Value::Number(456)),
                ("name".to_owned(), Value::String("X".to_owned())),
            ]),
        )])]);
        let api = FolderApi::new(transport);

        let response = api
            .create_folder_by_path("token", "/Docs/X")
            .expect("create_folder_by_path should parse");

        assert_eq!(response.folder_id, 456);
        assert!(response.created);
    }

    #[test]
    fn create_folder_if_not_exists_reports_existing_folder() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("created".to_owned(), Value::Bool(false)),
            (
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("folderid".to_owned(), Value::Number(7)),
                    ("name".to_owned(), Value::String("Existing".to_owned())),
                ]),
            ),
        ])]);
        let api = FolderApi::new(transport);

        let response = api
            .create_folder_if_not_exists("token", Some(0), "Existing", "")
            .expect("idempotent create should parse");
        assert_eq!(response.folder_id, 7);
        assert!(!response.created);
    }

    #[test]
    fn create_folder_propagates_error_result_codes() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2002)),
            (
                "error".to_owned(),
                Value::String("already exists".to_owned()),
            ),
        ])]);
        let api = FolderApi::new(transport);

        let err = api
            .create_folder("token", 11, "Reports")
            .expect_err("conflict must surface as Result error");
        assert!(err.to_string().contains("2002"));
    }

    // ----- delete_folder / delete_folder_recursive / rename_folder ---------

    fn mutated_folder_response(folder_id: u64, parent: u64, name: &str) -> Value {
        Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("folderid".to_owned(), Value::Number(folder_id)),
                    ("parentfolderid".to_owned(), Value::Number(parent)),
                    ("name".to_owned(), Value::String(name.to_owned())),
                    ("isdeleted".to_owned(), Value::Bool(true)),
                ]),
            ),
        ])
    }

    #[test]
    fn delete_folder_parses_metadata() {
        let transport =
            MockTransport::with_responses(vec![mutated_folder_response(11, 0, "Archive")]);
        let api = FolderApi::new(transport);
        let response = api
            .delete_folder("token", 11)
            .expect("deletefolder should parse");
        assert_eq!(response.folder_id, Some(11));
        assert_eq!(response.parent_folder_id, Some(0));
        assert_eq!(response.name.as_deref(), Some("Archive"));
        assert!(response.is_deleted);
    }

    #[test]
    fn delete_folder_recursive_parses_metadata() {
        let transport =
            MockTransport::with_responses(vec![mutated_folder_response(11, 0, "Archive")]);
        let api = FolderApi::new(transport);
        let response = api
            .delete_folder_recursive("token", 11)
            .expect("deletefolderrecursive should parse");
        assert_eq!(response.folder_id, Some(11));
        assert!(response.is_deleted);
    }

    #[test]
    fn delete_folder_recursive_tolerates_missing_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = FolderApi::new(transport);
        let response = api
            .delete_folder_recursive("token", 11)
            .expect("deletefolderrecursive must accept metadata-less success");
        assert_eq!(response.folder_id, None);
        assert!(!response.is_deleted);
    }

    #[test]
    fn rename_folder_parses_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("folderid".to_owned(), Value::Number(11)),
                    ("parentfolderid".to_owned(), Value::Number(3)),
                    ("name".to_owned(), Value::String("Renamed".to_owned())),
                ]),
            ),
        ])]);
        let api = FolderApi::new(transport);

        let response = api
            .rename_folder("token", 11, 3, "Renamed")
            .expect("renamefolder should parse");
        assert_eq!(response.folder_id, Some(11));
        assert_eq!(response.parent_folder_id, Some(3));
        assert_eq!(response.name.as_deref(), Some("Renamed"));
        assert!(!response.is_deleted);
    }

    #[test]
    fn delete_folder_propagates_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2005)),
            (
                "error".to_owned(),
                Value::String("Directory does not exist.".to_owned()),
            ),
        ])]);
        let api = FolderApi::new(transport);
        let err = api
            .delete_folder("token", 11)
            .expect_err("missing folder must surface as Result error");
        assert!(err.to_string().contains("2005"));
    }

    #[test]
    fn rename_folder_propagates_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2003)),
            (
                "error".to_owned(),
                Value::String("Access denied.".to_owned()),
            ),
        ])]);
        let api = FolderApi::new(transport);
        let err = api
            .rename_folder("token", 11, 3, "Renamed")
            .expect_err("access-denied rename must surface as Result error");
        assert!(err.to_string().contains("2003"));
    }
}

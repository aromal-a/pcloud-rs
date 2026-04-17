//! Backend abstraction for the FUSE adapter.
//!
//! The adapter does not depend on a concrete pCloud transport. Instead it
//! takes any implementation of [`FolderBackend`]. Two implementations are
//! provided:
//!
//! - [`ProtoFolderBackend`]: wraps [`pcloud_proto::folder_api::FolderApi`]
//!   and uses `list_folder_contents_by_path` for real network traffic.
//! - A mock implementation in the test module (`MockFolderBackend`) used
//!   by the adapter's unit tests.
//!
//! This split keeps `pcloud-fs` testable without standing up a transport
//! and avoids coupling the FUSE layer to transport error types.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_proto::auth_api::{ApiServerHintConsumer, ProtocolTransport};
use pcloud_proto::folder_api::{FolderApi, FolderApiError, RemoteFolderListing};
use pcloud_proto::http_download::{
    HttpDownloadConfig, HttpDownloadError, SignedDownload, fetch_download,
};
use pcloud_proto::transfer_api::{TransferApi, TransferApiError};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};

use crate::errors::FsError;

/// Minimal listing abstraction the FUSE adapter needs. One call returns
/// the folder id plus all direct children.
pub trait FolderBackend: Send + Sync + 'static {
    /// List the contents of `path`. `path` is an already-canonicalised
    /// pCloud path, e.g. `/` or `/docs/reports`.
    fn list_contents(&self, path: &str) -> Result<RemoteFolderListing, FsError>;

    /// Create a new directory `name` under `parent_path`. Default
    /// implementation returns [`FsError::Invalid`] so adapters without a
    /// folder-create transport stay read-only. Returns the new folder's
    /// id and name (the path is `parent_path/name`).
    fn create_folder(&self, _parent_path: &str, _name: &str) -> Result<u64, FsError> {
        Err(FsError::Invalid)
    }

    /// Remove an empty directory at `path`. Default [`FsError::Invalid`].
    fn delete_folder(&self, _path: &str) -> Result<(), FsError> {
        Err(FsError::Invalid)
    }
}

/// `FolderBackend` backed by a live `pcloud-proto` transport.
///
/// The auth token is held in a [`SecretString`] so that it is zeroised on
/// drop and excluded from any `Debug` output (see `pcloud-secret`).
pub struct ProtoFolderBackend<T> {
    api: FolderApi<T>,
    auth_token: SecretString,
}

impl<T> std::fmt::Debug for ProtoFolderBackend<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtoFolderBackend")
            .field("auth_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl<T> ProtoFolderBackend<T> {
    /// Construct a backend from a transport and an authenticated token.
    /// The token is stored in a [`SecretString`] so it zeroises on drop.
    pub fn new(transport: T, auth_token: SecretString) -> Self {
        Self {
            api: FolderApi::new(transport),
            auth_token,
        }
    }
}

impl<T> FolderBackend for ProtoFolderBackend<T>
where
    T: ProtocolTransport + ApiServerHintConsumer + Send + Sync + 'static,
{
    fn list_contents(&self, path: &str) -> Result<RemoteFolderListing, FsError> {
        self.api
            .list_folder_contents_by_path(self.auth_token.expose_secret(), path)
            .map_err(folder_error_to_fs)
    }

    fn create_folder(&self, parent_path: &str, name: &str) -> Result<u64, FsError> {
        // `create_folder_by_path` takes the full target path (parent + name).
        // Non-idempotent `createfolder` semantics: server returns EEXIST
        // when the name already exists, which maps to `FsError::Exists`
        // — matching POSIX `mkdir` behavior.
        let full_path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let resp = self
            .api
            .create_folder_by_path(self.auth_token.expose_secret(), full_path)
            .map_err(folder_error_to_fs)?;
        Ok(resp.folder_id)
    }

    fn delete_folder(&self, path: &str) -> Result<(), FsError> {
        // Resolve the folder id via listfolder, then call deletefolder.
        // This is 2 round-trips but keeps the trait signature path-only,
        // avoiding a stale-id cache. Empty-folder enforcement is done on
        // the server.
        let listing = self
            .api
            .list_folder_by_path(self.auth_token.expose_secret(), path)
            .map_err(folder_error_to_fs)?;
        self.api
            .delete_folder(self.auth_token.expose_secret(), listing.folder_id)
            .map_err(folder_error_to_fs)?;
        Ok(())
    }
}

fn folder_error_to_fs<E>(err: FolderApiError<E>) -> FsError
where
    E: std::error::Error + Send + Sync + 'static,
{
    match err {
        FolderApiError::Result { result, message } => FsError::from_pcloud_result(result, message),
        FolderApiError::Transport(e) => FsError::transport(e.to_string()),
        FolderApiError::Encode(_) | FolderApiError::Malformed(_) => FsError::Io,
    }
}

fn transfer_error_to_fs<E>(err: TransferApiError<E>) -> FsError
where
    E: std::error::Error + Send + Sync + 'static,
{
    match err {
        TransferApiError::Result { result, message } => {
            FsError::from_pcloud_result(result, message)
        }
        TransferApiError::Transport(e) => FsError::transport(e.to_string()),
        TransferApiError::Encode(_) | TransferApiError::Malformed(_) => FsError::Io,
    }
}

fn http_error_to_fs(err: HttpDownloadError) -> FsError {
    match err {
        HttpDownloadError::HttpStatus(403) => FsError::PermissionDenied,
        HttpDownloadError::HttpStatus(404) => FsError::NotFound,
        _ => FsError::transport(err.to_string()),
    }
}

// -----------------------------------------------------------------------------
// bd-1du.4.c FileBackend: open/read/release for mounted FUSE reads.
// -----------------------------------------------------------------------------

/// Opaque handle returned by [`FileBackend::open`]. It carries everything
/// needed to satisfy subsequent reads without re-resolving the signed URL,
/// but is otherwise opaque to callers.
#[derive(Debug, Clone)]
pub struct FileHandle {
    /// pCloud file id. Used as the page cache key.
    pub file_id: u64,
    /// Total file size reported by the backend, in bytes.
    pub size: u64,
    /// Preferred signed-download host. Subsequent reads reuse this host to
    /// keep byte-range fetches sticky on a single edge.
    pub host: String,
    /// Signed path component returned by `getfilelink`.
    pub path: String,
    /// Optional `dwltag` cookie attached to the signed URL.
    pub dwltag: Option<String>,
}

/// Read-path backend: resolves signed download URLs and fetches bytes.
///
/// Implementations are expected to be cheap to clone (e.g. `Arc`-wrapped
/// internally) because the FUSE adapter shares one instance across all
/// concurrent `open`/`read` calls.
pub trait FileBackend: Send + Sync + 'static {
    /// Resolve a signed download URL for `file_id` and return a handle.
    fn open(&self, file_id: u64) -> Result<FileHandle, FsError>;

    /// Fetch `len` bytes starting at `offset` from the file identified by
    /// `handle`. Implementations **must** honour the caller's `offset`/`len`
    /// exactly: returning fewer bytes than requested is only valid when the
    /// request crosses EOF, in which case the slice is truncated to the
    /// available bytes.
    fn read(&self, handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, FsError>;

    /// Release any backend-side resources tied to `handle`. Default is a
    /// no-op because the signed URL is stateless.
    fn release(&self, _handle: &FileHandle) -> Result<(), FsError> {
        Ok(())
    }
}

/// `FileBackend` backed by a live `pcloud-proto` transport.
///
/// The auth token is held in a [`SecretString`] so that it is zeroised on
/// drop and excluded from `Debug` output. It is only ever exposed to the
/// `getfilelink` RPC via [`ExposeSecret::expose_secret`]; the subsequent HTTP
/// GET uses only the resulting signed URL and an opaque `dwltag` cookie and
/// carries no bearer token.
pub struct ProtoFileBackend<T> {
    api: TransferApi<T>,
    auth_token: SecretString,
    http: HttpDownloadConfig,
}

impl<T> std::fmt::Debug for ProtoFileBackend<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtoFileBackend")
            .field("auth_token", &"<redacted>")
            .field("http", &self.http)
            .finish_non_exhaustive()
    }
}

impl<T> ProtoFileBackend<T> {
    /// Construct a file backend with the default HTTP download config.
    pub fn new(transport: T, auth_token: SecretString) -> Self {
        Self::with_http_config(transport, auth_token, HttpDownloadConfig::default())
    }

    /// Construct a file backend with a caller-provided
    /// [`HttpDownloadConfig`] (e.g. to override TLS settings or timeouts).
    pub fn with_http_config(
        transport: T,
        auth_token: SecretString,
        http: HttpDownloadConfig,
    ) -> Self {
        Self {
            api: TransferApi::new(transport),
            auth_token,
            http,
        }
    }
}

impl<T> FileBackend for ProtoFileBackend<T>
where
    T: ProtocolTransport + ApiServerHintConsumer + Send + Sync + 'static,
{
    fn open(&self, file_id: u64) -> Result<FileHandle, FsError> {
        let link = match self
            .api
            .get_file_link(self.auth_token.expose_secret(), file_id, None)
        {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "[pcloud-backend] getfilelink file_id={file_id} FAILED: {e:?}"
                );
                return Err(transfer_error_to_fs(e));
            }
        };
        eprintln!(
            "[pcloud-backend] getfilelink file_id={file_id} hosts={:?} path={} has_dwltag={}",
            link.hosts,
            link.path,
            link.download_tag.is_some()
        );
        let host = link
            .hosts
            .into_iter()
            .next()
            .ok_or_else(|| FsError::transport("getfilelink returned no hosts"))?;
        // `getfilelink` does not include file size; defer to a per-range
        // response on first read (the HTTP layer reports Content-Length).
        Ok(FileHandle {
            file_id,
            size: 0,
            host,
            path: link.path,
            dwltag: link.download_tag,
        })
    }

    fn read(&self, handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        if len == 0 {
            return Ok(Vec::new());
        }
        // Use an HTTP `Range:` header. This is more robust than appending
        // `&offset=&size=` to the path: the signed URL typically already
        // carries query parameters, so adding a duplicate key makes the
        // edge ignore the range and return bytes from offset 0 for every
        // page. A `Range` header has no such collision risk and maps to
        // pCloud's documented streaming protocol.
        let end = offset.saturating_add(len as u64);
        let download = SignedDownload {
            host: handle.host.clone(),
            port: None,
            path: handle.path.clone(),
            dwltag: handle.dwltag.clone(),
            range: Some((offset, end)),
        };
        match fetch_download(&download, &self.http) {
            Ok(v) => {
                eprintln!(
                    "[pcloud-backend] fetch host={} off={offset} len={len} got={}",
                    handle.host,
                    v.len()
                );
                Ok(v)
            }
            Err(e) => {
                eprintln!(
                    "[pcloud-backend] fetch host={} off={offset} len={len} FAILED: {e:?}",
                    handle.host
                );
                Err(http_error_to_fs(e))
            }
        }
    }
}

// -----------------------------------------------------------------------------
// bd-1du.4.e sub-task 3: `FileUploadBackend` backed by the live proto transport.
// -----------------------------------------------------------------------------

use crate::write_path::{FileUploadBackend, WritePathError};
use pcloud_proto::ProtocolMethod;
use pcloud_proto::methods::upload::{UploadSaveRequest, UploadWriteRequest};
use std::time::{SystemTime, UNIX_EPOCH};

/// Transport abstraction required by [`ProtoUploadBackend`]. Equivalent to
/// [`pcloud_proto::auth_api::ProtocolTransport`] plus a body-bearing execute
/// for `upload_write`. Implemented below for [`pcloud_proto::BinaryApiTransport`].
pub trait UploadTransport: Send + Sync + 'static {
    /// Transport-specific error surfaced by `execute` / `execute_with_body`.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Execute a header-only protocol request (e.g. `upload_create`,
    /// `upload_save`). No body is attached.
    fn execute(
        &self,
        request: &pcloud_proto::EncodedRequest,
    ) -> Result<pcloud_proto::response::Value, Self::Error>;

    /// Execute a request with an attached body (e.g. `upload_write`). The
    /// body bytes are streamed after the protocol header.
    fn execute_with_body(
        &self,
        request: &pcloud_proto::EncodedRequest,
        body: &[u8],
    ) -> Result<pcloud_proto::response::Value, Self::Error>;
}

impl UploadTransport for pcloud_proto::BinaryApiTransport {
    type Error = pcloud_proto::TransportError;

    fn execute(
        &self,
        request: &pcloud_proto::EncodedRequest,
    ) -> Result<pcloud_proto::response::Value, Self::Error> {
        <Self as ProtocolTransport>::execute(self, request)
    }

    fn execute_with_body(
        &self,
        request: &pcloud_proto::EncodedRequest,
        body: &[u8],
    ) -> Result<pcloud_proto::response::Value, Self::Error> {
        pcloud_proto::BinaryApiTransport::execute_with_body(self, request, body)
    }
}

/// `FileUploadBackend` implementation that drives the real pCloud upload
/// lifecycle (`upload_create` + `upload_write` + `upload_save`).
///
/// Parent-folder resolution is handled via [`FolderApi::list_folder_by_path`].
///
/// Unlink / rename are **not yet wired** in `pcloud-proto`; those calls return
/// a transport error so the write journal stays marked dirty and the caller
/// sees a clear failure instead of silent data loss. This is an honest gap
/// owned by bd-1du.4.e sub-task 3 — see CLAUDE.md.
pub struct ProtoUploadBackend<T> {
    transport: T,
    auth_token: SecretString,
}

impl<T> std::fmt::Debug for ProtoUploadBackend<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtoUploadBackend")
            .field("auth_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl<T> ProtoUploadBackend<T> {
    /// Construct an upload backend over a cloneable transport. The auth
    /// token is stored in a [`SecretString`] and never logged.
    pub fn new(transport: T, auth_token: SecretString) -> Self {
        Self {
            transport,
            auth_token,
        }
    }
}

impl<T> FileUploadBackend for ProtoUploadBackend<T>
where
    T: UploadTransport + ProtocolTransport + ApiServerHintConsumer + Clone + 'static,
{
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &std::path::Path,
    ) -> Result<(), WritePathError> {
        // Resolve parent folder id by path. FolderApi takes ownership of the
        // transport, so we clone the cheap (Arc-internal) `BinaryApiTransport`.
        let folder_api = FolderApi::new(self.transport.clone());
        let parent = folder_api
            .list_folder_by_path(self.auth_token.expose_secret(), parent_path)
            .map_err(|e| WritePathError::Upload(format!("resolve parent: {e}")))?;

        let bytes = std::fs::read(staging_file)
            .map_err(|e| WritePathError::Upload(format!("staging read: {e}")))?;

        // upload_create
        let create = pcloud_proto::methods::upload::UploadCreateRequest {
            auth_token: self.auth_token.expose_secret().to_owned(),
            parent_folder_id: parent.folder_id,
            file_name: name.to_owned(),
            file_size: bytes.len() as u64,
        };
        let encoded = create
            .encode()
            .map_err(|e| WritePathError::Upload(format!("encode upload_create: {e}")))?;
        let response = <T as UploadTransport>::execute(&self.transport, &encoded)
            .map_err(|e| WritePathError::Upload(format!("upload_create: {e}")))?;
        let hash = response
            .as_hash()
            .ok_or_else(|| WritePathError::Upload("upload_create: not a hash".to_owned()))?;
        let upload_id = hash
            .get_number("uploadid")
            .ok_or_else(|| WritePathError::Upload("upload_create: missing uploadid".to_owned()))?;

        // upload_write (streams body)
        let write_req = UploadWriteRequest {
            auth_token: self.auth_token.expose_secret().to_owned(),
            upload_id,
            upload_offset: 0,
            chunk_id: 0,
        };
        let encoded_write = write_req
            .encode_with_body(bytes.len() as u64)
            .map_err(|e| WritePathError::Upload(format!("encode upload_write: {e}")))?;
        let resp_w =
            <T as UploadTransport>::execute_with_body(&self.transport, &encoded_write, &bytes)
                .map_err(|e| WritePathError::Upload(format!("upload_write: {e}")))?;
        let hw = resp_w
            .as_hash()
            .ok_or_else(|| WritePathError::Upload("upload_write: not a hash".to_owned()))?;
        if matches!(hw.get_number("result"), Some(v) if v != 0) {
            return Err(WritePathError::Upload(format!(
                "upload_write result={}",
                hw.get_number("result").unwrap_or(0)
            )));
        }

        // upload_save
        let save = UploadSaveRequest {
            auth_token: self.auth_token.expose_secret().to_owned(),
            parent_folder_id: parent.folder_id,
            file_name: name.to_owned(),
            upload_id,
            modified_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            ctime: None,
            conflict: None,
        };
        let encoded_save = save
            .encode()
            .map_err(|e| WritePathError::Upload(format!("encode upload_save: {e}")))?;
        let resp_s = <T as UploadTransport>::execute(&self.transport, &encoded_save)
            .map_err(|e| WritePathError::Upload(format!("upload_save: {e}")))?;
        let hs = resp_s
            .as_hash()
            .ok_or_else(|| WritePathError::Upload("upload_save: not a hash".to_owned()))?;
        if matches!(hs.get_number("result"), Some(v) if v != 0) {
            return Err(WritePathError::Upload(format!(
                "upload_save result={}",
                hs.get_number("result").unwrap_or(0)
            )));
        }
        Ok(())
    }

    fn unlink_remote(&self, path: &str) -> Result<(), WritePathError> {
        // Resolve file_id by listing the parent folder. This mirrors the
        // C `task_deletefile` path which is also driven by a (parent,
        // name) resolution step.
        let (parent_path, name) = split_parent_name(path)
            .ok_or_else(|| WritePathError::Upload(format!("unlink: bad path {path}")))?;
        let folder_api = FolderApi::new(self.transport.clone());
        let listing = folder_api
            .list_folder_contents_by_path(self.auth_token.expose_secret(), &parent_path)
            .map_err(|e| WritePathError::Upload(format!("resolve parent for unlink: {e}")))?;
        let entry = listing
            .entries
            .iter()
            .find(|e| !e.is_folder && e.name == name)
            .ok_or_else(|| WritePathError::Upload(format!("unlink: {path} not found")))?;
        let file_id = entry
            .file_id
            .ok_or_else(|| WritePathError::Upload(format!("unlink: {path} missing fileid")))?;
        let transfer = TransferApi::new(self.transport.clone());
        transfer
            .delete_file(self.auth_token.expose_secret(), file_id)
            .map_err(|e| WritePathError::Upload(format!("deletefile: {e}")))?;
        Ok(())
    }

    fn rename_remote(&self, from: &str, to: &str) -> Result<(), WritePathError> {
        // Resolve source file_id and destination parent folder_id.
        let (from_parent, from_name) = split_parent_name(from)
            .ok_or_else(|| WritePathError::Upload(format!("rename: bad from {from}")))?;
        let (to_parent, to_name) = split_parent_name(to)
            .ok_or_else(|| WritePathError::Upload(format!("rename: bad to {to}")))?;

        let folder_api = FolderApi::new(self.transport.clone());
        let src_listing = folder_api
            .list_folder_contents_by_path(self.auth_token.expose_secret(), &from_parent)
            .map_err(|e| WritePathError::Upload(format!("resolve src parent: {e}")))?;
        let src_entry = src_listing
            .entries
            .iter()
            .find(|e| !e.is_folder && e.name == from_name)
            .ok_or_else(|| WritePathError::Upload(format!("rename: {from} not found")))?;
        let file_id = src_entry
            .file_id
            .ok_or_else(|| WritePathError::Upload(format!("rename: {from} missing fileid")))?;

        // Short-circuit: same parent means we only need the same folder_id.
        let to_folder_id = if to_parent == from_parent {
            src_listing.folder_id
        } else {
            folder_api
                .list_folder_by_path(self.auth_token.expose_secret(), &to_parent)
                .map_err(|e| WritePathError::Upload(format!("resolve dst parent: {e}")))?
                .folder_id
        };

        let transfer = TransferApi::new(self.transport.clone());
        let _renamed = transfer
            .rename_file(
                self.auth_token.expose_secret(),
                file_id,
                to_folder_id,
                to_name,
            )
            .map_err(|e| WritePathError::Upload(format!("renamefile: {e}")))?;
        Ok(())
    }
}

fn split_parent_name(path: &str) -> Option<(String, String)> {
    if path == "/" {
        return None;
    }
    let idx = path.rfind('/')?;
    let parent = if idx == 0 {
        "/".to_owned()
    } else {
        path[..idx].to_owned()
    };
    let name = path[idx + 1..].to_owned();
    if name.is_empty() {
        return None;
    }
    Some((parent, name))
}

/// Mock `FolderBackend` used by both internal unit tests and by the
/// 4.b integration test. Exposed publicly (not `#[cfg(test)]`) so that
/// downstream tests can consume it without a real transport dependency.
pub mod mock {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use pcloud_proto::folder_api::{RemoteFolderEntry, RemoteFolderListing};

    use super::FolderBackend;
    use crate::errors::FsError;

    /// Canned-response backend used by adapter unit tests.
    #[derive(Debug, Default)]
    pub struct MockFolderBackend {
        listings: Mutex<HashMap<String, Result<RemoteFolderListing, FsError>>>,
    }

    impl MockFolderBackend {
        /// Construct an empty mock backend with no seeded listings.
        pub fn new() -> Self {
            Self::default()
        }

        /// Seed a canned directory listing for `path`.
        ///
        /// `entries` is a tuple of `(name, is_folder, folder_id, file_id)`
        /// so tests can describe a directory in one call.
        pub fn insert_dir(
            &self,
            path: &str,
            folder_id: u64,
            entries: Vec<(&str, bool, Option<u64>, Option<u64>)>,
        ) {
            let listing = RemoteFolderListing {
                folder_id,
                path: path.to_owned(),
                name: path.rsplit('/').next().unwrap_or("").to_owned(),
                entries: entries
                    .into_iter()
                    .map(|(name, is_folder, fid, fileid)| RemoteFolderEntry {
                        name: name.to_owned(),
                        is_folder,
                        folder_id: fid,
                        file_id: fileid,
                        owner_user_id: None,
                        is_mine: false,
                        encrypted: false,
                        is_shared: false,
                        permissions: None,
                        size: None,
                        modified: None,
                    })
                    .collect(),
                api_server: None,
                owner_user_id: None,
                is_mine: false,
                encrypted: false,
                is_shared: false,
                permissions: None,
            };
            self.listings
                .lock()
                .expect("mock: listings mutex poisoned")
                .insert(path.to_owned(), Ok(listing));
        }

        /// Seed a canned error response for `path` so tests can exercise
        /// the error-translation paths in the adapter.
        pub fn insert_error(&self, path: &str, err: FsError) {
            self.listings
                .lock()
                .expect("mock: listings mutex poisoned")
                .insert(path.to_owned(), Err(err));
        }

        /// Seed a canned directory listing with explicit per-entry sizes,
        /// so integration tests that round-trip reads via a real FUSE
        /// mount can advertise non-zero file sizes (the kernel refuses
        /// to issue `read(2)` past what `getattr` reports). The tuple
        /// layout mirrors [`Self::insert_dir`] but adds a trailing
        /// `Option<u64>` carrying the byte size to publish.
        #[allow(clippy::type_complexity)]
        pub fn insert_dir_with_sizes(
            &self,
            path: &str,
            folder_id: u64,
            entries: Vec<(&str, bool, Option<u64>, Option<u64>, Option<u64>)>,
        ) {
            let listing = RemoteFolderListing {
                folder_id,
                path: path.to_owned(),
                name: path.rsplit('/').next().unwrap_or("").to_owned(),
                entries: entries
                    .into_iter()
                    .map(|(name, is_folder, fid, fileid, size)| RemoteFolderEntry {
                        name: name.to_owned(),
                        is_folder,
                        folder_id: fid,
                        file_id: fileid,
                        owner_user_id: None,
                        is_mine: false,
                        encrypted: false,
                        is_shared: false,
                        permissions: None,
                        size,
                        modified: None,
                    })
                    .collect(),
                api_server: None,
                owner_user_id: None,
                is_mine: false,
                encrypted: false,
                is_shared: false,
                permissions: None,
            };
            self.listings
                .lock()
                .expect("mock: listings mutex poisoned")
                .insert(path.to_owned(), Ok(listing));
        }
    }

    impl FolderBackend for MockFolderBackend {
        fn list_contents(&self, path: &str) -> Result<RemoteFolderListing, FsError> {
            let guard = self.listings.lock().expect("mock: listings mutex poisoned");
            match guard.get(path) {
                Some(Ok(l)) => Ok(l.clone()),
                Some(Err(e)) => Err(e.clone()),
                None => Err(FsError::NotFound),
            }
        }
    }

    // ---- FileBackend mock ---------------------------------------------------

    use super::{FileBackend, FileHandle};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Canned-content file backend. Stores a full in-memory byte buffer per
    /// `file_id` and serves byte-range reads out of it, tracking counters for
    /// opens/reads/releases so the read-path tests can assert call shapes.
    #[derive(Debug, Default)]
    pub struct MockFileBackend {
        files: Mutex<HashMap<u64, Vec<u8>>>,
        errors: Mutex<HashMap<u64, FsError>>,
        /// Count of `open` calls observed; read by tests.
        pub opens: AtomicU64,
        /// Count of `read` calls observed; read by tests.
        pub reads: AtomicU64,
        /// Count of `release` calls observed; read by tests.
        pub releases: AtomicU64,
    }

    impl MockFileBackend {
        /// Construct an empty mock file backend with no seeded files.
        pub fn new() -> Self {
            Self::default()
        }

        /// Seed `bytes` as the full content of `file_id`. Subsequent reads
        /// on this id will serve slices of this buffer.
        pub fn insert_file(&self, file_id: u64, bytes: Vec<u8>) {
            self.files
                .lock()
                .expect("mock: files mutex poisoned")
                .insert(file_id, bytes);
        }

        /// Seed a canned error for `file_id` so tests can exercise the
        /// `open` error path.
        pub fn insert_error(&self, file_id: u64, err: FsError) {
            self.errors
                .lock()
                .expect("mock: errors mutex poisoned")
                .insert(file_id, err);
        }
    }

    impl FileBackend for MockFileBackend {
        fn open(&self, file_id: u64) -> Result<FileHandle, FsError> {
            self.opens.fetch_add(1, Ordering::Relaxed);
            if let Some(err) = self
                .errors
                .lock()
                .expect("mock: errors mutex poisoned")
                .get(&file_id)
            {
                return Err(err.clone());
            }
            let files = self.files.lock().expect("mock: files mutex poisoned");
            let bytes = files.get(&file_id).ok_or(FsError::NotFound)?;
            Ok(FileHandle {
                file_id,
                size: bytes.len() as u64,
                host: "mock".to_owned(),
                path: format!("/mock/{file_id}"),
                dwltag: None,
            })
        }

        fn read(&self, handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if let Some(err) = self
                .errors
                .lock()
                .expect("mock: errors mutex poisoned")
                .get(&handle.file_id)
            {
                return Err(err.clone());
            }
            let files = self.files.lock().expect("mock: files mutex poisoned");
            let bytes = files.get(&handle.file_id).ok_or(FsError::NotFound)?;
            let off = offset as usize;
            if off >= bytes.len() {
                return Ok(Vec::new());
            }
            let end = off.saturating_add(len).min(bytes.len());
            Ok(bytes[off..end].to_vec())
        }

        fn release(&self, _handle: &FileHandle) -> Result<(), FsError> {
            self.releases.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
}

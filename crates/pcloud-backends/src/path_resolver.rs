//! Production-grade [`PublicLinkPathResolver`] backed by the pCloud drive
//! `listfolder` protocol call.
//!
//! The C client resolves public-link tree paths through its local pfs cache
//! (`pfs_fldr_id_by_path` / `pfs_fldr_resolve_path`). Until the Rust rewrite
//! owns a full pfs cache, we still need real path resolution for
//! `do_ptree_public_link` parity. This module implements a resolver that
//! walks each path segment by segment via authenticated `listfolder`
//! requests, caches successful resolutions with a bounded TTL, and
//! **refuses** to fabricate identifiers when a path cannot be resolved.
//!
//! Security / enterprise rules enforced here:
//!
//! - cache keys are `(sha256(auth_token), path)` so the secret token never
//!   appears in a cache key or log,
//! - the auth token is only passed to the transport (already required for
//!   any pCloud call) — it is never persisted, logged, or stored on `Self`
//!   in cleartext (it is wrapped in [`SecretString`]),
//! - on failure the resolver returns typed errors (`NotFound`,
//!   `ExpectedFolder`, `ExpectedFile`, `Ambiguous`, `Transport`) — never a
//!   `0` fallback,
//! - TTL entries are evicted on read and the cache is bounded in size to
//!   prevent unbounded memory growth on path-enumeration bursts.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    collections::HashMap,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use pcloud_model::ids::{RemoteFolderId, UserId};
use pcloud_proto::{
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    folder_api::{FolderApi, FolderApiError, RemoteFolderEntry, RemoteFolderListing},
    public_links_api::PublicLinkPathResolver,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Default cache TTL: resolutions are valid for 60s which is short enough
/// that moved/deleted folders are re-checked frequently but long enough to
/// collapse burst calls across the builder arrays (root, folders, files).
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(60);

/// Hard upper bound on cached entries. The daemon only issues tree-link
/// resolutions on user request, so this is plenty; an adversarial burst
/// cannot grow memory past this.
pub const DEFAULT_CACHE_CAPACITY: usize = 512;

/// Typed resolver errors. Every variant describes exactly why resolution
/// failed so callers can surface an honest diagnostic instead of an
/// invented `0` identifier.
#[derive(Debug, Error)]
pub enum PathResolveError {
    /// Path string was empty or did not start with `/`.
    #[error("path must be an absolute pCloud drive path starting with '/': {path:?}")]
    InvalidPath {
        /// Offending input path as supplied by the caller.
        path: String,
    },
    /// Some intermediate or final segment was not present in the parent
    /// listing.
    #[error("pCloud path {path:?} was not found (missing segment {segment:?})")]
    NotFound {
        /// Absolute path being resolved.
        path: String,
        /// Segment that was not found in the parent listing.
        segment: String,
    },
    /// Caller asked for a folder id but the path resolved to a file.
    #[error("pCloud path {path:?} resolves to a file, not a folder")]
    ExpectedFolder {
        /// Path that resolved to a file instead of a folder.
        path: String,
    },
    /// Caller asked for a file id but the path resolved to a folder.
    #[error("pCloud path {path:?} resolves to a folder, not a file")]
    ExpectedFile {
        /// Path that resolved to a folder instead of a file.
        path: String,
    },
    /// A parent listing contained more than one entry with the final
    /// segment name — we refuse to pick one arbitrarily.
    #[error("pCloud path {path:?} is ambiguous: {count} entries share the same name")]
    Ambiguous {
        /// Path whose final segment is ambiguous.
        path: String,
        /// Number of conflicting entries observed in the parent listing.
        count: usize,
    },
    /// `listfolder` succeeded but the matched entry was missing the
    /// expected numeric id. This should only happen on a broken server
    /// response and is treated as a hard failure.
    #[error("pCloud listing entry for {path:?} was missing its numeric id")]
    MissingId {
        /// Path whose listing entry lacked a numeric id.
        path: String,
    },
    /// Underlying proto/transport failure.
    #[error("pCloud listfolder failed while resolving {path:?}: {source}")]
    Transport {
        /// `path` field.
        path: String,
        #[source]
        /// `source` field.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Kind of entry expected at a resolved path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EntryKind {
    Folder,
    File,
}

#[derive(Debug, Clone)]
struct CachedEntry {
    id: u64,
    kind: EntryKind,
    expires_at: Instant,
}

/// Resolver that walks absolute pCloud-drive paths by issuing authenticated
/// `listfolder` calls and matches the final segment against the parent's
/// direct children.
pub struct RemotePathResolver<T> {
    folder_api: FolderApi<T>,
    auth_token: SecretString,
    cache: Mutex<HashMap<(Vec<u8>, String), CachedEntry>>,
    ttl: Duration,
    capacity: usize,
}

impl<T> std::fmt::Debug for RemotePathResolver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePathResolver")
            .field("ttl", &self.ttl)
            .field("capacity", &self.capacity)
            .finish()
    }
}

impl<T> RemotePathResolver<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// Create a new resolver. `auth_token` is stored as a `SecretString` so
    /// it zeroizes on drop and is never debug-printed.
    #[must_use]
    pub fn new(transport: T, auth_token: SecretString) -> Self {
        Self::with_ttl(
            transport,
            auth_token,
            DEFAULT_CACHE_TTL,
            DEFAULT_CACHE_CAPACITY,
        )
    }

    /// Create a resolver with an explicit TTL and capacity.
    #[must_use]
    pub fn with_ttl(
        transport: T,
        auth_token: SecretString,
        ttl: Duration,
        capacity: usize,
    ) -> Self {
        Self {
            folder_api: FolderApi::new(transport),
            auth_token,
            cache: Mutex::new(HashMap::new()),
            ttl,
            capacity: capacity.max(1),
        }
    }

    fn token_fingerprint(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.auth_token.expose_secret().as_bytes());
        hasher.finalize().to_vec()
    }

    fn cache_lookup(&self, fingerprint: &[u8], path: &str) -> Option<CachedEntry> {
        let mut cache = self.cache.lock().expect("cache mutex poisoned");
        let key = (fingerprint.to_vec(), path.to_owned());
        match cache.get(&key).cloned() {
            Some(entry) if entry.expires_at > Instant::now() => Some(entry),
            Some(_) => {
                cache.remove(&key);
                None
            }
            None => None,
        }
    }

    fn cache_store(&self, fingerprint: &[u8], path: &str, entry: CachedEntry) {
        let mut cache = self.cache.lock().expect("cache mutex poisoned");
        if cache.len() >= self.capacity
            && let Some(oldest_key) = cache
                .iter()
                .min_by_key(|(_, v)| v.expires_at)
                .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest_key);
        }
        cache.insert((fingerprint.to_vec(), path.to_owned()), entry);
    }

    fn resolve(&self, path: &str, expected: EntryKind) -> Result<u64, PathResolveError> {
        let normalized = normalize_path(path)?;
        let fingerprint = self.token_fingerprint();

        if let Some(cached) = self.cache_lookup(&fingerprint, &normalized) {
            return match_kind(&normalized, cached.kind, cached.id, expected);
        }

        if normalized == "/" {
            if matches!(expected, EntryKind::File) {
                return Err(PathResolveError::ExpectedFile { path: normalized });
            }
            let listing = self
                .folder_api
                .list_folder_contents_by_path(self.auth_token.expose_secret(), "/")
                .map_err(|err| transport_err(&normalized, err))?;
            let entry = CachedEntry {
                id: listing.folder_id,
                kind: EntryKind::Folder,
                expires_at: Instant::now() + self.ttl,
            };
            self.cache_store(&fingerprint, &normalized, entry.clone());
            return Ok(entry.id);
        }

        let (parent, last) = split_parent(&normalized);
        let listing = self
            .folder_api
            .list_folder_contents_by_path(self.auth_token.expose_secret(), parent.as_str())
            .map_err(|err| transport_err(&normalized, err))?;

        let matches: Vec<&RemoteFolderEntry> = listing
            .entries
            .iter()
            .filter(|entry| entry.name == last)
            .collect();
        if matches.is_empty() {
            return Err(PathResolveError::NotFound {
                path: normalized,
                segment: last,
            });
        }
        if matches.len() > 1 {
            return Err(PathResolveError::Ambiguous {
                path: normalized,
                count: matches.len(),
            });
        }
        let matched = matches[0];

        let (kind, id) = if matched.is_folder {
            let id = matched
                .folder_id
                .ok_or_else(|| PathResolveError::MissingId {
                    path: normalized.clone(),
                })?;
            (EntryKind::Folder, id)
        } else {
            let id = matched.file_id.ok_or_else(|| PathResolveError::MissingId {
                path: normalized.clone(),
            })?;
            (EntryKind::File, id)
        };

        self.cache_store(
            &fingerprint,
            &normalized,
            CachedEntry {
                id,
                kind,
                expires_at: Instant::now() + self.ttl,
            },
        );

        match_kind(&normalized, kind, id, expected)
    }

    /// Resolve an absolute pCloud-drive path to its folder id.
    ///
    /// Mirrors C `psync_get_fsfolderid_by_path` (psynclib.c:2170) +
    /// `pfs_fldr_idperm_by_path` (pfsfolder.c:342). The C path reads
    /// from the local fs cache; the Rust active path walks the
    /// canonical drive via authenticated `listfolder` so we never
    /// fabricate the `0`/`PSYNC_INVALID_FSFOLDERID` sentinel on miss.
    pub fn get_folder_id_by_path(&self, path: &str) -> Result<RemoteFolderId, PathResolveError> {
        let id = self.resolve(path, EntryKind::Folder)?;
        Ok(RemoteFolderId::new(id))
    }

    /// Read folder flags + permissions + sharing/encryption view via a
    /// targeted `listfolder` against the absolute path. Mirrors
    /// `psync_get_fsfolderflags_by_id` (psynclib.c:2176) and the
    /// `flags` + `permissions` out-params of
    /// `pfs_fldr_idperm_by_path`. The Rust active path queries the
    /// canonical drive so the answer is authoritative even when the
    /// local cache has not yet been populated.
    pub fn get_folder_flags(&self, path: &str) -> Result<FolderFlags, PathResolveError> {
        let normalized = normalize_path(path)?;
        let listing = self
            .folder_api
            .list_folder_contents_by_path(self.auth_token.expose_secret(), normalized.as_str())
            .map_err(|err| transport_err(&normalized, err))?;
        Ok(folder_flags_from_listing(&listing))
    }

    /// Read the owner user id of a folder by absolute path. Mirrors
    /// `psync_get_folder_ownerid` (psynclib.c:2088), which the C
    /// client reads from its local `folder` table by id. The Rust
    /// active path resolves through `listfolder` against the canonical
    /// drive.
    pub fn get_folder_owner_id(&self, path: &str) -> Result<UserId, PathResolveError> {
        let normalized = normalize_path(path)?;
        let listing = self
            .folder_api
            .list_folder_contents_by_path(self.auth_token.expose_secret(), normalized.as_str())
            .map_err(|err| transport_err(&normalized, err))?;
        match listing.owner_user_id {
            Some(id) => Ok(UserId::new(id)),
            None => Err(PathResolveError::MissingId { path: normalized }),
        }
    }
}

/// Folder metadata facets mirroring the C `flags` + `permissions`
/// columns of the local `folder` table that `pfs_fldr_flags_by_id` and
/// `pfs_fldr_idperm_by_path` populate.
///
/// Bit layout follows the C constants:
/// - `permissions` is a `PSYNC_PERM_*` bitmap (READ=1, CREATE=2,
///   MODIFY=4, DELETE=8, MANAGE=16; see `psynclib.h:206-216`).
/// - `encrypted` mirrors `PSYNC_FOLDER_FLAG_ENCRYPTED`
///   (pfoldersync.h:47).
/// - `shared` mirrors the listfolder `isshared` boolean.
/// - `readonly` is derived: `true` when permissions are present and
///   the CREATE+MODIFY+DELETE bits are all clear (caller cannot
///   mutate). C does not encode this as a single bit; the Rust path
///   surfaces it as a convenience.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderFlags {
    /// `permissions` field.
    pub permissions: Option<u32>,
    /// `encrypted` field.
    pub encrypted: bool,
    /// `shared` field.
    pub shared: bool,
    /// `readonly` field.
    pub readonly: bool,
}

fn folder_flags_from_listing(listing: &RemoteFolderListing) -> FolderFlags {
    use pcloud_proto::folder_api::perm_bits;
    const WRITE_MASK: u32 = perm_bits::CREATE | perm_bits::MODIFY | perm_bits::DELETE;
    let permissions = listing.permissions;
    let readonly = match permissions {
        Some(bits) => (bits & WRITE_MASK) == 0,
        None => false,
    };
    FolderFlags {
        permissions,
        encrypted: listing.encrypted,
        shared: listing.is_shared,
        readonly,
    }
}

/// Coarse synchronization status of an absolute local path. Mirrors
/// the C `external_status_t` returned by `psync_filesystem_status`
/// (psynclib.c:1903), which collapses the richer
/// `PSYNC_PATH_STATUS_*` codes (ppathstatus.h:40-46) into four
/// user-facing buckets.
///
/// Mapping (C -> Rust):
/// - `PSYNC_PATH_STATUS_IN_SYNC` -> [`FsPathStatus::InSync`] (`INSYNC`)
/// - `PSYNC_PATH_STATUS_IN_PROG` -> [`FsPathStatus::InProgress`] (`INPROG`)
/// - `PSYNC_PATH_STATUS_PAUSED` / `_REMOTE_FULL` / `_LOCAL_FULL`
///   -> [`FsPathStatus::NoSync`] (`NOSYNC`)
/// - anything else (`_NOT_OURS`, `_NOT_FOUND`) -> [`FsPathStatus::Invalid`]
///   (`INVSYNC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FsPathStatus {
    /// `InSync` variant.
    InSync,
    /// `InProgress` variant.
    InProgress,
    /// `NoSync` variant.
    NoSync,
    /// `Invalid` variant.
    Invalid,
}

/// Snapshot of sync-root + engine state that
/// [`filesystem_status`] consumes. Defined as a separate POD so callers
/// (the daemon `RuntimeShell` in production, fixture states in tests)
/// can construct it without pulling in the entire `EngineShell`.
#[derive(Debug, Clone, Default)]
pub struct FilesystemStatusInputs<'a> {
    /// Tracked sync roots (from `store.repositories.sync_graph`). Only
    /// `local_path` and `paused` are read. `local_path` should already
    /// be canonicalized to match the lookup path.
    pub sync_roots: &'a [SyncRootView<'a>],
    /// Sync ids that the runtime currently considers paused.
    pub paused_sync_ids: &'a [u64],
    /// Sync ids the engine has queued work against (any
    /// `PlannedOperation`). Presence means at least one operation is
    /// in flight or pending.
    pub queued_sync_ids: &'a [u64],
    /// Sync ids the engine has classified as conflict/error states
    /// (`PlannedOperation::Conflict`). Mirrors the C behavior of
    /// degrading paths under unresolved conflict to `NOSYNC`.
    pub errored_sync_ids: &'a [u64],
}

/// Minimal sync-root projection used by [`filesystem_status`]. This
/// avoids leaking the store record shape into the resolver crate.
#[derive(Debug, Clone, Copy)]
pub struct SyncRootView<'a> {
    /// `sync_id` field.
    pub sync_id: u64,
    /// `local_path` field.
    pub local_path: &'a str,
    /// `paused` field.
    pub paused: bool,
}

/// Classify a local filesystem path against the daemon's sync-root +
/// engine state. Mirrors `psync_filesystem_status` (psynclib.c:1903).
///
/// The classification is metadata-only — it does NOT touch the file
/// system. A path is considered to be "inside" a sync root when it is
/// equal to, or a descendant of, the canonicalized sync-root local
/// path. Paths outside any tracked sync root return
/// [`FsPathStatus::Invalid`] (the C `INVSYNC`).
pub fn filesystem_status(path: &Path, inputs: FilesystemStatusInputs<'_>) -> FsPathStatus {
    let target = path.to_string_lossy();
    let target_ref = target.as_ref();

    // Find the deepest matching sync root (longest local_path prefix
    // wins). C resolves through its parent-cache hash; longest-match
    // is the prefix-tree analogue.
    let mut best: Option<&SyncRootView<'_>> = None;
    for root in inputs.sync_roots {
        if path_within(target_ref, root.local_path)
            && best
                .map(|cur| root.local_path.len() > cur.local_path.len())
                .unwrap_or(true)
        {
            best = Some(root);
        }
    }

    let Some(root) = best else {
        return FsPathStatus::Invalid;
    };

    if root.paused || inputs.paused_sync_ids.contains(&root.sync_id) {
        return FsPathStatus::NoSync;
    }
    if inputs.errored_sync_ids.contains(&root.sync_id) {
        // C maps remote/local-full and unrecoverable error to NOSYNC.
        return FsPathStatus::NoSync;
    }
    if inputs.queued_sync_ids.contains(&root.sync_id) {
        return FsPathStatus::InProgress;
    }
    FsPathStatus::InSync
}

fn path_within(candidate: &str, root: &str) -> bool {
    if candidate == root {
        return true;
    }
    // Strict descendant: candidate must start with `root + '/'` (or
    // `root + std::path::MAIN_SEPARATOR`). This avoids `/foo` matching
    // `/foobar`.
    let sep = std::path::MAIN_SEPARATOR;
    if let Some(rest) = candidate.strip_prefix(root) {
        return rest.starts_with('/') || rest.starts_with(sep);
    }
    false
}

impl<T> PublicLinkPathResolver for RemotePathResolver<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    type Error = PathResolveError;

    fn resolve_folder(&self, path: &str) -> Result<u64, Self::Error> {
        self.resolve(path, EntryKind::Folder)
    }

    fn resolve_file(&self, path: &str) -> Result<u64, Self::Error> {
        self.resolve(path, EntryKind::File)
    }
}

fn match_kind(
    path: &str,
    actual: EntryKind,
    id: u64,
    expected: EntryKind,
) -> Result<u64, PathResolveError> {
    match (actual, expected) {
        (EntryKind::Folder, EntryKind::Folder) | (EntryKind::File, EntryKind::File) => Ok(id),
        (EntryKind::File, EntryKind::Folder) => Err(PathResolveError::ExpectedFolder {
            path: path.to_owned(),
        }),
        (EntryKind::Folder, EntryKind::File) => Err(PathResolveError::ExpectedFile {
            path: path.to_owned(),
        }),
    }
}

fn normalize_path(path: &str) -> Result<String, PathResolveError> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(PathResolveError::InvalidPath {
            path: path.to_owned(),
        });
    }
    let mut out = String::with_capacity(path.len());
    let mut prev_slash = false;
    for ch in path.chars() {
        if ch == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
            out.push('/');
        } else {
            prev_slash = false;
            out.push(ch);
        }
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    Ok(out)
}

fn split_parent(path: &str) -> (String, String) {
    // INVARIANT: all paths entering this function have been normalised by
    // `normalise_path`; the normaliser guarantees a leading '/' so rfind
    // can never return None.
    let idx = path
        .rfind('/')
        .expect("normalised path always contains '/'");
    let (parent, last) = path.split_at(idx);
    let parent = if parent.is_empty() {
        "/".to_owned()
    } else {
        parent.to_owned()
    };
    let last = last.trim_start_matches('/').to_owned();
    (parent, last)
}

fn transport_err<E>(path: &str, err: FolderApiError<E>) -> PathResolveError
where
    E: std::error::Error + Send + Sync + 'static,
{
    if let FolderApiError::Result { result: 2005, .. } = &err {
        return PathResolveError::NotFound {
            path: path.to_owned(),
            segment: path.to_owned(),
        };
    }
    PathResolveError::Transport {
        path: path.to_owned(),
        source: Box::new(err),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use pcloud_proto::{
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        response::Value,
    };
    use pcloud_secret::secret_string::SecretString;

    use super::{PathResolveError, RemotePathResolver};
    use pcloud_proto::public_links_api::PublicLinkPathResolver;

    #[derive(Clone)]
    struct MockTransport {
        inner: Arc<MockInner>,
    }

    struct MockInner {
        responses: Mutex<Vec<Value>>,
        calls: Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                inner: Arc::new(MockInner {
                    responses: Mutex::new(responses.into_iter().rev().collect()),
                    calls: Mutex::new(Vec::new()),
                }),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.inner
                .calls
                .lock()
                .expect("calls lock poisoned")
                .clone()
        }
    }

    impl ProtocolTransport for MockTransport {
        type Error = io::Error;

        fn execute(&self, request: &pcloud_proto::EncodedRequest) -> Result<Value, Self::Error> {
            let path = request
                .params
                .iter()
                .find(|p| p.name == "path")
                .and_then(|p| match &p.value {
                    pcloud_proto::binary_api::BinaryParamValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            self.inner
                .calls
                .lock()
                .expect("calls lock poisoned")
                .push(path);
            self.inner
                .responses
                .lock()
                .expect("responses lock poisoned")
                .pop()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, _api_server: &str) {}
    }

    fn listing(folder_id: u64, name: &str, entries: Vec<Value>) -> Value {
        Value::Hash(vec![(
            "metadata".to_owned(),
            Value::Hash(vec![
                ("folderid".to_owned(), Value::Number(folder_id)),
                ("name".to_owned(), Value::String(name.to_owned())),
                ("contents".to_owned(), Value::Array(entries)),
            ]),
        )])
    }

    fn folder_entry(name: &str, id: u64) -> Value {
        Value::Hash(vec![
            ("name".to_owned(), Value::String(name.to_owned())),
            ("isfolder".to_owned(), Value::Bool(true)),
            ("folderid".to_owned(), Value::Number(id)),
        ])
    }

    fn file_entry(name: &str, id: u64) -> Value {
        Value::Hash(vec![
            ("name".to_owned(), Value::String(name.to_owned())),
            ("isfolder".to_owned(), Value::Bool(false)),
            ("fileid".to_owned(), Value::Number(id)),
        ])
    }

    fn secret(value: &str) -> SecretString {
        SecretString::new(value.to_owned())
    }

    #[test]
    fn resolves_two_segment_folder_path() {
        let transport = MockTransport::new(vec![listing(
            0,
            "/",
            vec![folder_entry("docs", 11), file_entry("readme.txt", 42)],
        )]);
        let handle = transport.clone();
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let id = resolver.resolve_folder("/docs").expect("folder resolves");
        assert_eq!(id, 11);
        assert_eq!(handle.calls(), vec!["/".to_owned()]);
    }

    #[test]
    fn resolves_four_segment_file_path() {
        let transport =
            MockTransport::new(vec![listing(11, "Q1", vec![file_entry("report.pdf", 99)])]);
        let handle = transport.clone();
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let id = resolver
            .resolve_file("/docs/2025/Q1/report.pdf")
            .expect("file resolves");
        assert_eq!(id, 99);
        assert_eq!(handle.calls(), vec!["/docs/2025/Q1".to_owned()]);
    }

    #[test]
    fn not_found_segment_errors_explicitly() {
        let transport = MockTransport::new(vec![listing(0, "/", vec![folder_entry("other", 1)])]);
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let err = resolver
            .resolve_folder("/missing")
            .expect_err("missing segment must error");
        assert!(matches!(err, PathResolveError::NotFound { .. }));
    }

    #[test]
    fn folder_requested_but_file_returned_is_rejected() {
        let transport =
            MockTransport::new(vec![listing(0, "/", vec![file_entry("report.txt", 42)])]);
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let err = resolver
            .resolve_folder("/report.txt")
            .expect_err("kind mismatch must error");
        assert!(matches!(err, PathResolveError::ExpectedFolder { .. }));
    }

    #[test]
    fn file_requested_but_folder_returned_is_rejected() {
        let transport = MockTransport::new(vec![listing(0, "/", vec![folder_entry("docs", 11)])]);
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let err = resolver
            .resolve_file("/docs")
            .expect_err("kind mismatch must error");
        assert!(matches!(err, PathResolveError::ExpectedFile { .. }));
    }

    #[test]
    fn ambiguous_entries_are_rejected() {
        let transport = MockTransport::new(vec![listing(
            0,
            "/",
            vec![folder_entry("dup", 11), folder_entry("dup", 12)],
        )]);
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let err = resolver
            .resolve_folder("/dup")
            .expect_err("ambiguous entries must error");
        assert!(matches!(err, PathResolveError::Ambiguous { count: 2, .. }));
    }

    #[test]
    fn transport_2005_surfaces_as_not_found() {
        let transport = MockTransport::new(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2005)),
            ("error".to_owned(), Value::String("nope".to_owned())),
        ])]);
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let err = resolver
            .resolve_folder("/anything")
            .expect_err("2005 must error");
        assert!(matches!(err, PathResolveError::NotFound { .. }));
    }

    #[test]
    fn invalid_path_is_rejected_without_transport_call() {
        let transport = MockTransport::new(vec![]);
        let handle = transport.clone();
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let err = resolver
            .resolve_folder("relative/path")
            .expect_err("relative path must be rejected");
        assert!(matches!(err, PathResolveError::InvalidPath { .. }));
        assert!(handle.calls().is_empty());
    }

    #[test]
    fn successful_resolution_is_cached() {
        let transport = MockTransport::new(vec![listing(0, "/", vec![folder_entry("docs", 11)])]);
        let handle = transport.clone();
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let first = resolver.resolve_folder("/docs").expect("first call ok");
        let second = resolver.resolve_folder("/docs").expect("second call ok");
        assert_eq!(first, 11);
        assert_eq!(second, 11);
        assert_eq!(handle.calls().len(), 1);
    }

    #[test]
    fn mixed_file_and_folder_targets_both_resolve() {
        // Two listfolder calls for "/" — one folder lookup, one file
        // lookup, since the cache key is (token, path) and each call uses
        // the same parent path but a different expected kind. The first
        // lookup caches the entry with its actual kind so the second
        // request can short-circuit via the cache.
        let transport = MockTransport::new(vec![
            listing(
                0,
                "/",
                vec![folder_entry("docs", 11), file_entry("readme.txt", 42)],
            ),
            listing(
                0,
                "/",
                vec![folder_entry("docs", 11), file_entry("readme.txt", 42)],
            ),
        ]);
        let resolver = RemotePathResolver::new(transport, secret("tok"));

        let folder_id = resolver
            .resolve_folder("/docs")
            .expect("folder target resolves");
        let file_id = resolver
            .resolve_file("/readme.txt")
            .expect("file target resolves");
        assert_eq!(folder_id, 11);
        assert_eq!(file_id, 42);
    }
}

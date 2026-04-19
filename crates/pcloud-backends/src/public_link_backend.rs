//! Public-link backend: file/folder public link create/list/show/delete,
//! `changepublink` expire/password/upload-policy mutations, upload-link
//! CRUD, tree-link creation, upload-access helpers, bookmark/pin, and
//! screenshot-link helpers. Called from `pcloud-daemon::dispatch` and
//! the CLI/SDK; wraps `pcloud-proto::public_links_api`.
//!
//! Portable; no platform gating.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_model::public_links::{
    CreatedTreePublicLink, CreatedUploadLink, PublicLinkAccessEntry, PublicLinkBookmark,
    PublicLinkContents, PublicLinkSummary, PublicLinkUploadPolicy, UploadLinkSummary,
};
use pcloud_proto::{
    BinaryApiTransport, EncodedRequest, ParseLimits, PublicLinksApi, PublicLinksApiError,
    ResponseParseError, TransportConfig, TransportError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    parse_response_frame,
    public_links_api::{PublicLinkPathResolver, TreePublicLinkPaths},
    response::Value,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

#[derive(Debug, Clone, Default)]
/// `DevelopmentPublicLinkTransport` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct DevelopmentPublicLinkTransport;

impl ProtocolTransport for DevelopmentPublicLinkTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "listpublinks" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                (
                    "publinks",
                    EncodedValue::Array(vec![
                        EncodedValue::Hash(vec![
                            ("linkid", EncodedValue::Number(7)),
                            ("code", EncodedValue::String("alpha123")),
                            (
                                "link",
                                EncodedValue::String("https://e.pcloud.link/alpha123"),
                            ),
                            ("created", EncodedValue::Number(100)),
                            ("modified", EncodedValue::Number(200)),
                            ("isupload", EncodedValue::Bool(false)),
                            ("haspassword", EncodedValue::Bool(true)),
                            ("views", EncodedValue::Number(9)),
                            ("expires", EncodedValue::Number(300)),
                            (
                                "metadata",
                                EncodedValue::Hash(vec![
                                    ("name", EncodedValue::String("report.txt")),
                                    ("isfolder", EncodedValue::Bool(false)),
                                    ("fileid", EncodedValue::Number(42)),
                                    ("parentfolderid", EncodedValue::Number(2)),
                                ]),
                            ),
                        ]),
                        EncodedValue::Hash(vec![
                            ("linkid", EncodedValue::Number(8)),
                            ("code", EncodedValue::String("folder999")),
                            (
                                "link",
                                EncodedValue::String("https://e.pcloud.link/folder999"),
                            ),
                            ("created", EncodedValue::Number(101)),
                            ("modified", EncodedValue::Number(201)),
                            ("isupload", EncodedValue::Bool(true)),
                            ("haspassword", EncodedValue::Bool(false)),
                            ("views", EncodedValue::Number(2)),
                            (
                                "metadata",
                                EncodedValue::Hash(vec![
                                    ("name", EncodedValue::String("docs")),
                                    ("isfolder", EncodedValue::Bool(true)),
                                    ("folderid", EncodedValue::Number(17)),
                                    ("parentfolderid", EncodedValue::Number(0)),
                                ]),
                            ),
                        ]),
                    ]),
                ),
            ]),
            "showpublink" => {
                let code = string_param(request, "code").unwrap_or("unknown");
                encode_hash_response(&[
                    ("result", EncodedValue::Number(0)),
                    (
                        "metadata",
                        EncodedValue::Hash(vec![(
                            "contents",
                            EncodedValue::Array(vec![
                                EncodedValue::Hash(vec![
                                    ("name", EncodedValue::OwnedString(format!("{code}-docs"))),
                                    ("created", EncodedValue::Number(11)),
                                    ("modified", EncodedValue::Number(12)),
                                    ("isfolder", EncodedValue::Bool(true)),
                                    ("folderid", EncodedValue::Number(3)),
                                    ("icon", EncodedValue::Number(4)),
                                ]),
                                EncodedValue::Hash(vec![
                                    ("name", EncodedValue::String("report.txt")),
                                    ("created", EncodedValue::Number(21)),
                                    ("modified", EncodedValue::Number(22)),
                                    ("isfolder", EncodedValue::Bool(false)),
                                    ("fileid", EncodedValue::Number(5)),
                                    ("icon", EncodedValue::Number(6)),
                                ]),
                            ]),
                        )]),
                    ),
                ])
            }
            "deletepublink" => {
                let link_id = number_param(request, "linkid").unwrap_or_default();
                if link_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2001)),
                        ("error", EncodedValue::String("public link not found")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "getfilepublink" => {
                let path = string_param(request, "path").unwrap_or("unknown");
                encode_hash_response(&[
                    ("result", EncodedValue::Number(0)),
                    ("linkid", EncodedValue::Number(71)),
                    (
                        "link",
                        EncodedValue::OwnedString(format!(
                            "https://e.pcloud.link/file-{}",
                            sanitize_path_fragment(path)
                        )),
                    ),
                ])
            }
            "getfolderpublink" => {
                let path = string_param(request, "path").unwrap_or("unknown");
                encode_hash_response(&[
                    ("result", EncodedValue::Number(0)),
                    ("linkid", EncodedValue::Number(81)),
                    (
                        "link",
                        EncodedValue::OwnedString(format!(
                            "https://e.pcloud.link/folder-{}",
                            sanitize_path_fragment(path)
                        )),
                    ),
                ])
            }
            "changepublink" => {
                let link_id = number_param(request, "linkid").unwrap_or_default();
                if link_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2002)),
                        ("error", EncodedValue::String("invalid link")),
                    ])
                } else if string_param(request, "linkpassword").is_some() && link_id == 405 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2003)),
                        ("error", EncodedValue::String("invalid password")),
                    ])
                } else if (number_param(request, "enableuploadforeveryone").is_some()
                    || number_param(request, "enableuploadforchosenusers").is_some()
                    || number_param(request, "disableupload").is_some())
                    && link_id == 406
                {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2004)),
                        ("error", EncodedValue::String("invalid upload policy")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "listuploadlinks" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                (
                    "uploadlinks",
                    EncodedValue::Array(vec![EncodedValue::Hash(vec![
                        ("uploadlinkid", EncodedValue::Number(17)),
                        ("code", EncodedValue::String("upl-alpha")),
                        (
                            "link",
                            EncodedValue::String("https://u.pcloud.link/upl-alpha"),
                        ),
                        ("comment", EncodedValue::String("Drop files here")),
                        ("space", EncodedValue::Number(512)),
                        ("maxspace", EncodedValue::Number(2048)),
                        ("files", EncodedValue::Number(3)),
                        ("created", EncodedValue::Number(400)),
                        ("modified", EncodedValue::Number(500)),
                        (
                            "metadata",
                            EncodedValue::Hash(vec![
                                ("name", EncodedValue::String("incoming")),
                                ("isfolder", EncodedValue::Bool(true)),
                                ("folderid", EncodedValue::Number(91)),
                                ("parentfolderid", EncodedValue::Number(2)),
                                ("icon", EncodedValue::Number(7)),
                            ]),
                        ),
                    ])]),
                ),
            ]),
            "createuploadlink" => {
                let path = string_param(request, "path").unwrap_or("unknown");
                let comment = string_param(request, "comment").unwrap_or("");
                if comment.trim().is_empty() {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2101)),
                        ("error", EncodedValue::String("comment is required")),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        ("uploadlinkid", EncodedValue::Number(171)),
                        (
                            "link",
                            EncodedValue::OwnedString(format!(
                                "https://u.pcloud.link/{}-{}",
                                sanitize_path_fragment(path),
                                sanitize_path_fragment(comment)
                            )),
                        ),
                    ])
                }
            }
            "deleteuploadlink" => {
                let upload_link_id = number_param(request, "uploadlinkid").unwrap_or_default();
                if upload_link_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2102)),
                        ("error", EncodedValue::String("upload link not found")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "gettreepublink" => {
                let name = string_param(request, "name").unwrap_or("");
                let has_target = string_param(request, "folderid").is_some()
                    || string_param(request, "folderids").is_some()
                    || string_param(request, "fileids").is_some();
                if name.trim().is_empty() || !has_target {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2103)),
                        (
                            "error",
                            EncodedValue::String("tree link requires name and at least one target"),
                        ),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        ("linkid", EncodedValue::Number(271)),
                        (
                            "link",
                            EncodedValue::OwnedString(format!(
                                "https://e.pcloud.link/tree-{}",
                                sanitize_path_fragment(name)
                            )),
                        ),
                    ])
                }
            }
            "sendpublink" => {
                let code = string_param(request, "code").unwrap_or("");
                let mails = string_param(request, "mails").unwrap_or("");
                if code.trim().is_empty() {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2261)),
                        ("error", EncodedValue::String("invalid public link code")),
                    ])
                } else if !mails.contains('@') {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2231)),
                        ("error", EncodedValue::String("invalid email")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "publink/createfolderlinkandsend" => {
                let folder_id = number_param(request, "folderid").unwrap_or_default();
                let mail = string_param(request, "mail").unwrap_or("");
                if folder_id == 0 || !mail.contains('@') {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2231)),
                        ("error", EncodedValue::String("invalid folder or email")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "publink/listemailswithaccess" => {
                let link_id = number_param(request, "linkid").unwrap_or_default();
                if link_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2201)),
                        ("error", EncodedValue::String("invalid link")),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        (
                            "list",
                            EncodedValue::Array(vec![
                                EncodedValue::Hash(vec![
                                    ("email", EncodedValue::String("alice@example.com")),
                                    ("receiverid", EncodedValue::Number(33)),
                                ]),
                                EncodedValue::Hash(vec![
                                    ("email", EncodedValue::String("bob@example.com")),
                                    ("receiverid", EncodedValue::Number(44)),
                                ]),
                            ]),
                        ),
                    ])
                }
            }
            "publink/addaccess" => {
                let link_id = number_param(request, "linkid").unwrap_or_default();
                let mail = string_param(request, "mail").unwrap_or("");
                if link_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2201)),
                        ("error", EncodedValue::String("invalid link")),
                    ])
                } else if !mail.contains('@') {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2202)),
                        ("error", EncodedValue::String("invalid email")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "publink/removeaccess" => {
                let receiver_id = number_param(request, "receiverid").unwrap_or_default();
                if receiver_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2203)),
                        ("error", EncodedValue::String("receiver not found")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "publink/listpins" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                (
                    "list",
                    EncodedValue::Array(vec![
                        EncodedValue::Hash(vec![
                            (
                                "link",
                                EncodedValue::String("https://e.pcloud.link/alpha123"),
                            ),
                            ("name", EncodedValue::String("Alpha Pin")),
                            ("code", EncodedValue::String("alpha123")),
                            ("description", EncodedValue::String("Pinned alpha")),
                            ("ctime", EncodedValue::Number(700)),
                            ("locationid", EncodedValue::Number(8)),
                        ]),
                        EncodedValue::Hash(vec![
                            (
                                "link",
                                EncodedValue::String("https://e.pcloud.link/beta456"),
                            ),
                            ("name", EncodedValue::String("Beta Pin")),
                            ("code", EncodedValue::String("beta456")),
                            ("ctime", EncodedValue::Number(701)),
                            ("locationid", EncodedValue::Number(9)),
                        ]),
                    ]),
                ),
            ]),
            "publink/unpin" => {
                let location_id = number_param(request, "locationid").unwrap_or_default();
                if location_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2301)),
                        ("error", EncodedValue::String("bookmark not found")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "publink/changepin" => {
                let location_id = number_param(request, "locationid").unwrap_or_default();
                let name = string_param(request, "name").unwrap_or("");
                if location_id == 404 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2301)),
                        ("error", EncodedValue::String("bookmark not found")),
                    ])
                } else if name.trim().is_empty() {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2302)),
                        ("error", EncodedValue::String("bookmark name is required")),
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

impl ApiServerHintConsumer for DevelopmentPublicLinkTransport {
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
/// `PublicLinkBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum PublicLinkBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

/// Error raised when the runtime has no registered pCloud-drive path resolver.
///
/// The Rust daemon does not fabricate folder/file identifiers. Path-based tree
/// links (`ptree_public_link`) require a registered resolver backed by the
/// local pfs cache; until that is wired the runtime fails loudly instead of
/// silently returning `0`, matching the enterprise-security rule that secrets
/// and identifiers must never be invented.
#[derive(Debug, Error)]
#[error("no pCloud path resolver is registered for path {path:?}")]
pub struct UnregisteredPathResolverError {
    /// `path` field.
    pub path: String,
}

/// Default resolver used until the runtime is wired to a real pfs-backed
/// implementation. Always refuses to resolve a path so callers get an explicit
/// failure instead of a fabricated id.
#[derive(Debug, Clone, Default)]
pub struct UnregisteredPathResolver;

impl PublicLinkPathResolver for UnregisteredPathResolver {
    type Error = UnregisteredPathResolverError;

    fn resolve_folder(&self, path: &str) -> Result<u64, Self::Error> {
        Err(UnregisteredPathResolverError {
            path: path.to_owned(),
        })
    }

    fn resolve_file(&self, path: &str) -> Result<u64, Self::Error> {
        Err(UnregisteredPathResolverError {
            path: path.to_owned(),
        })
    }
}

/// Simple in-memory resolver backed by two lookup maps; intended for testing
/// and for callers that already have folder/file ids materialised.
#[derive(Debug, Clone, Default)]
pub struct StaticPublicLinkPathResolver {
    folders: std::collections::HashMap<String, u64>,
    files: std::collections::HashMap<String, u64>,
}

impl StaticPublicLinkPathResolver {
    #[must_use]
    /// Invoke `new` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn new() -> Self {
        Self::default()
    }

    /// Invoke `insert_folder` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn insert_folder(&mut self, path: impl Into<String>, id: u64) {
        self.folders.insert(path.into(), id);
    }

    /// Invoke `insert_file` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn insert_file(&mut self, path: impl Into<String>, id: u64) {
        self.files.insert(path.into(), id);
    }
}

impl PublicLinkPathResolver for StaticPublicLinkPathResolver {
    type Error = UnregisteredPathResolverError;

    fn resolve_folder(&self, path: &str) -> Result<u64, Self::Error> {
        self.folders
            .get(path)
            .copied()
            .ok_or_else(|| UnregisteredPathResolverError {
                path: path.to_owned(),
            })
    }

    fn resolve_file(&self, path: &str) -> Result<u64, Self::Error> {
        self.files
            .get(path)
            .copied()
            .ok_or_else(|| UnregisteredPathResolverError {
                path: path.to_owned(),
            })
    }
}

#[derive(Debug, Clone)]
/// `PublicLinkTransportMode` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum PublicLinkTransportMode {
    /// `Development` variant.
    Development(DevelopmentPublicLinkTransport),
    /// `Network` variant.
    Network(BinaryApiTransport),
}

impl ProtocolTransport for PublicLinkTransportMode {
    type Error = PublicLinkBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(transport) => transport.execute(request).map_err(Into::into),
            Self::Network(transport) => transport.execute(request).map_err(Into::into),
        }
    }
}

impl ApiServerHintConsumer for PublicLinkTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(transport) => transport.apply_api_server_hint(api_server),
            Self::Network(transport) => transport.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the public-link backend.
///
/// # Architecture role
///
/// - Dispatches `PublicLinkCreate` (file/folder/tree), `PublicLinkList`,
///   `PublicLinkShow`, `PublicLinkDelete`, `PublicLinkChange` (expire,
///   password, upload policy), `UploadLinkCreate`, `UploadLinkList`,
///   `UploadLinkDelete`, `ScreenshotLink`, `BookmarkLink`, and tree-link
///   request frames from `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `getfilepublink`,
///   `getfolderpublink`, `gettreepublink`, `listpublinks`, `showpublink`,
///   `deletepublink`, `changepublink`, `createuploadlink`,
///   `listuploadlinks`, `deleteuploadlink`, `getfilepubzip`,
///   `uploadtolink`. Tree-path resolution uses the embedded
///   `PublicLinkPathResolver` (from `pcloud_proto::public_links_api`)
///   backed by `crate::path_resolver::RemotePathResolver` for the
///   authenticated path, or [`UnregisteredPathResolver`] for the
///   unauthenticated path (which refuses to fabricate ids). Wire encoding
///   uses the crate-level `EncodedValue` pattern.
/// - Emits audit events for every link creation, mutation, and deletion.
/// - Persists nothing durably; public-link state is canonical on the
///   server.
/// - Error taxonomy: see [`PublicLinkBackendError`] and
///   [`UnregisteredPathResolverError`].
pub struct PublicLinkRuntime {
    api: PublicLinksApi<PublicLinkTransportMode>,
    transport: PublicLinkTransportMode,
}

impl PublicLinkRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => {
                PublicLinkTransportMode::Development(DevelopmentPublicLinkTransport)
            }
            ApiMode::Plaintext | ApiMode::Tls => PublicLinkTransportMode::Network(
                BinaryApiTransport::new(TransportConfig::with_tls(
                    matches!(config.api.mode, ApiMode::Tls),
                    config.api.host.clone(),
                    config.api.port,
                    config.api.server_name.clone(),
                    std::time::Duration::from_millis(config.api.connect_timeout_ms),
                    std::time::Duration::from_millis(config.api.read_timeout_ms),
                )),
            ),
        };

        Self {
            api: PublicLinksApi::new(transport.clone()),
            transport,
        }
    }

    /// Build a production pfs-style path resolver that uses the same
    /// transport as the public-link runtime. The resolver walks paths via
    /// authenticated `listfolder` calls and refuses to fabricate ids.
    #[must_use]
    pub fn path_resolver(
        &self,
        auth_token: SecretString,
    ) -> crate::path_resolver::RemotePathResolver<PublicLinkTransportMode> {
        crate::path_resolver::RemotePathResolver::new(self.transport.clone(), auth_token)
    }

    /// Invoke `list_public_links` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn list_public_links(
        &self,
        auth_token: SecretString,
    ) -> Result<Vec<PublicLinkSummary>, PublicLinksApiError<PublicLinkBackendError>> {
        self.api.list_public_links(auth_token.expose_secret())
    }

    /// Invoke `show_public_link` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn show_public_link(
        &self,
        auth_token: SecretString,
        code: impl Into<String>,
    ) -> Result<PublicLinkContents, PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .show_public_link(auth_token.expose_secret(), code.into())
    }

    /// Invoke `create_file_public_link` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn create_file_public_link(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
    ) -> Result<
        pcloud_model::public_links::CreatedPublicLink,
        PublicLinksApiError<PublicLinkBackendError>,
    > {
        self.api
            .create_file_public_link(auth_token.expose_secret(), path.into())
    }

    /// Invoke `create_folder_public_link` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn create_folder_public_link(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
    ) -> Result<
        pcloud_model::public_links::CreatedPublicLink,
        PublicLinksApiError<PublicLinkBackendError>,
    > {
        self.api
            .create_folder_public_link(auth_token.expose_secret(), path.into())
    }

    /// Invoke `delete_public_link` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn delete_public_link(
        &self,
        auth_token: SecretString,
        link_id: u64,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .delete_public_link(auth_token.expose_secret(), link_id)
    }

    /// Invoke `change_public_link_expire` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn change_public_link_expire(
        &self,
        auth_token: SecretString,
        link_id: u64,
        expire: Option<u64>,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .change_public_link_expire(auth_token.expose_secret(), link_id, expire)
    }

    /// Invoke `change_public_link_password` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    ///
    /// `password` is taken as `Option<SecretString>` (ncx.66) so the
    /// end-user-chosen link-protection secret is zeroized on drop and
    /// never appears unredacted in `Debug`. It is exposed exactly once,
    /// at the wire-encoding boundary, via `SecretString::expose_secret`.
    pub fn change_public_link_password(
        &self,
        auth_token: SecretString,
        link_id: u64,
        password: Option<SecretString>,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api.change_public_link_password(
            auth_token.expose_secret(),
            link_id,
            password.as_ref().map(|p| p.expose_secret().to_owned()),
        )
    }

    /// Invoke `change_public_link_upload` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn change_public_link_upload(
        &self,
        auth_token: SecretString,
        link_id: u64,
        policy: PublicLinkUploadPolicy,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .change_public_link_upload(auth_token.expose_secret(), link_id, policy)
    }

    /// Invoke `list_upload_links` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn list_upload_links(
        &self,
        auth_token: SecretString,
    ) -> Result<Vec<UploadLinkSummary>, PublicLinksApiError<PublicLinkBackendError>> {
        self.api.list_upload_links(auth_token.expose_secret())
    }

    /// Invoke `create_upload_link` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn create_upload_link(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
        comment: impl Into<String>,
        expire: Option<u64>,
        maxspace: Option<u64>,
        maxfiles: Option<u64>,
    ) -> Result<CreatedUploadLink, PublicLinksApiError<PublicLinkBackendError>> {
        self.api.create_upload_link(
            auth_token.expose_secret(),
            path.into(),
            comment.into(),
            expire,
            maxspace,
            maxfiles,
        )
    }

    /// Invoke `delete_upload_link` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn delete_upload_link(
        &self,
        auth_token: SecretString,
        upload_link_id: u64,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .delete_upload_link(auth_token.expose_secret(), upload_link_id)
    }

    #[allow(clippy::too_many_arguments)]
    /// Invoke `create_tree_public_link` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn create_tree_public_link(
        &self,
        auth_token: SecretString,
        name: impl Into<String>,
        root_folder_id: Option<u64>,
        folder_ids_csv: Option<String>,
        file_ids_csv: Option<String>,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Result<CreatedTreePublicLink, PublicLinksApiError<PublicLinkBackendError>> {
        self.api.create_tree_public_link(
            auth_token.expose_secret(),
            name.into(),
            root_folder_id,
            folder_ids_csv,
            file_ids_csv,
            expire,
            maxdownloads,
            maxtraffic,
        )
    }

    /// Invoke `list_public_link_access` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn list_public_link_access(
        &self,
        auth_token: SecretString,
        link_id: u64,
    ) -> Result<Vec<PublicLinkAccessEntry>, PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .list_public_link_access(auth_token.expose_secret(), link_id)
    }

    /// Invoke `add_public_link_access` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn add_public_link_access(
        &self,
        auth_token: SecretString,
        link_id: u64,
        email: impl Into<String>,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .add_public_link_access(auth_token.expose_secret(), link_id, email.into())
    }

    /// Invoke `remove_public_link_access` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn remove_public_link_access(
        &self,
        auth_token: SecretString,
        link_id: u64,
        receiver_id: u64,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .remove_public_link_access(auth_token.expose_secret(), link_id, receiver_id)
    }

    /// Invoke `list_bookmarks` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn list_bookmarks(
        &self,
        auth_token: SecretString,
    ) -> Result<Vec<PublicLinkBookmark>, PublicLinksApiError<PublicLinkBackendError>> {
        self.api.list_bookmarks(auth_token.expose_secret())
    }

    /// Invoke `remove_bookmark` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn remove_bookmark(
        &self,
        auth_token: SecretString,
        code: impl Into<String>,
        location_id: u64,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api
            .remove_bookmark(auth_token.expose_secret(), code.into(), location_id)
    }

    /// Invoke `change_bookmark` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn change_bookmark(
        &self,
        auth_token: SecretString,
        code: impl Into<String>,
        location_id: u64,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api.change_bookmark(
            auth_token.expose_secret(),
            code.into(),
            location_id,
            name.into(),
            description.into(),
        )
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
    }

    /// Mirrors the C `psync_send_publink` helper
    /// (`pclsync/psynclib.c:2217`). The caller supplies the existing
    /// public-link `code`, a comma-separated `mails` list, and a
    /// `message` body; the wire `source` is fixed to `1`.
    pub fn send_publink(
        &self,
        auth_token: SecretString,
        code: impl Into<String>,
        mails: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api.send_publink(
            auth_token.expose_secret(),
            code.into(),
            mails.into(),
            message.into(),
        )
    }

    /// Mirrors the C `do_psync_file_public_link` helper with optional
    /// `expire`, `maxdownloads`, and `maxtraffic`.
    pub fn create_file_public_link_with_options(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Result<
        pcloud_model::public_links::CreatedPublicLink,
        PublicLinksApiError<PublicLinkBackendError>,
    > {
        self.api.create_file_public_link_with_options(
            auth_token.expose_secret(),
            path.into(),
            expire,
            maxdownloads,
            maxtraffic,
        )
    }

    /// Mirrors the C `do_psync_folder_public_link_full` helper.
    pub fn create_folder_public_link_with_options(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
        password: Option<String>,
    ) -> Result<
        pcloud_model::public_links::CreatedPublicLink,
        PublicLinksApiError<PublicLinkBackendError>,
    > {
        self.api.create_folder_public_link_with_options(
            auth_token.expose_secret(),
            path.into(),
            expire,
            maxdownloads,
            maxtraffic,
            password,
        )
    }

    /// Mirrors the C `do_psync_screenshot_public_link` helper.
    pub fn create_screenshot_public_link(
        &self,
        auth_token: SecretString,
        path: impl Into<String>,
        has_delay: bool,
        delay_seconds: u64,
    ) -> Result<
        pcloud_model::public_links::CreatedPublicLink,
        PublicLinksApiError<PublicLinkBackendError>,
    > {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.api.create_screenshot_public_link(
            auth_token.expose_secret(),
            path.into(),
            has_delay,
            delay_seconds,
            now,
        )
    }

    /// Mirrors the C `do_psync_folder_updownlink_link` helper.
    pub fn create_folder_updownlink(
        &self,
        auth_token: SecretString,
        folder_id: u64,
        mail: impl Into<String>,
        can_upload: bool,
    ) -> Result<(), PublicLinksApiError<PublicLinkBackendError>> {
        self.api.create_folder_updownlink(
            auth_token.expose_secret(),
            folder_id,
            mail.into(),
            can_upload,
        )
    }

    /// Mirrors the C `do_ptree_public_link` helper using path-based targets.
    ///
    /// Resolution of pCloud drive paths to folder/file identifiers is
    /// delegated to `resolver`, which is expected to consult a local pfs
    /// cache. If a path cannot be resolved the call fails loudly instead of
    /// fabricating a zero identifier.
    #[allow(clippy::too_many_arguments)]
    pub fn create_tree_public_link_from_paths<R>(
        &self,
        auth_token: SecretString,
        name: impl Into<String>,
        paths: &TreePublicLinkPaths,
        resolver: &R,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Result<
        pcloud_model::public_links::CreatedTreePublicLink,
        PublicLinksApiError<PublicLinkBackendError>,
    >
    where
        R: PublicLinkPathResolver,
    {
        self.api.create_tree_public_link_from_paths(
            auth_token.expose_secret(),
            name.into(),
            paths,
            resolver,
            expire,
            maxdownloads,
            maxtraffic,
        )
    }

    /// Convenience wrapper that resolves paths via the runtime's own
    /// `RemotePathResolver`. This is the production entry point used when
    /// the caller does not inject a custom resolver; it still refuses to
    /// fabricate ids on any unresolved path.
    #[allow(clippy::too_many_arguments)]
    pub fn create_tree_public_link_from_paths_default(
        &self,
        auth_token: SecretString,
        name: impl Into<String>,
        paths: &TreePublicLinkPaths,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Result<
        pcloud_model::public_links::CreatedTreePublicLink,
        PublicLinksApiError<PublicLinkBackendError>,
    > {
        let resolver = self.path_resolver(auth_token.clone_secret());
        self.api.create_tree_public_link_from_paths(
            auth_token.expose_secret(),
            name.into(),
            paths,
            &resolver,
            expire,
            maxdownloads,
            maxtraffic,
        )
    }
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

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

fn sanitize_path_fragment(path: &str) -> String {
    path.trim_matches('/').replace('/', "-").replace(' ', "_")
}

/// Test-only mock fixture for the `public_link_backend` subsystem.
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
    pub const REPRESENTATIVE_COMMAND: &str = "listpublinks";

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

        /// Record the representative public-link runtime call (listpublinks).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}

//! T1.6.b.2 — verb dispatch for the WebDAV gateway.
//!
//! The dispatcher is pure: it maps a parsed [`HttpRequest`] to an
//! [`HttpResponse`] using a pluggable backend. The concrete
//! [`RemoteFsIpcBackend`](crate::RemoteFsIpcBackend) maps this contract to the
//! daemon's canonical `RemoteFs` IPC requests.
//! No sockets, no threads — those land in T1.6.c.
//!
//! # Verb coverage
//!
//! | Verb       | Behaviour                                                                |
//! |------------|--------------------------------------------------------------------------|
//! | `OPTIONS`  | `200 OK` with an `Allow` header and experimental marker.              |
//! | `PROPFIND` | Calls `IpcBackend::list_folder` and renders a `207 Multi-Status`.       |
//! | `GET`      | Calls `IpcBackend::get_file`; `200 OK` body or `404`.                   |
//! | `HEAD`     | Same as `GET` but the body is dropped.                                  |
//! | `PUT`      | Refused with `403` when `allow_writes = false`; otherwise calls         |
//! |            | `IpcBackend::put_file` and returns `201` on first create / `204` on    |
//! |            | overwrite.                                                              |
//! | `DELETE`   | `IpcBackend::delete` → `204 No Content`.                                |
//! | `MKCOL`    | `IpcBackend::mkdir` → `201 Created`.                                    |
//! | other      | `405 Method Not Allowed` with the same `Allow` header.                  |
//!
//! Read-only mode (the `ServerConfig::allow_writes = false` default)
//! refuses every mutating verb with `403 Forbidden` so even a
//! mis-wired backend cannot accidentally upload.
//!
//! # Error mapping
//!
//! [`BackendError::NotFound`] becomes `404`, [`BackendError::Conflict`]
//! becomes `409`, [`BackendError::TooLarge`] becomes `413`. Everything
//! else is `500`. Mappings are deliberately conservative — the
//! gateway is the public surface, so erring toward generic messages
//! keeps fingerprinting low.

// **PLATFORM:** all
// **GATING:** none.

use crate::ServerConfig;
use crate::http::{HttpRequest, HttpResponse};
use crate::propfind::{
    PropfindResource, PropfindResponseEntry, parse_propfind_or_allprop, render_multistatus,
};

/// Errors returned by [`IpcBackend`] implementations.
///
/// Variants are mapped to HTTP status codes by [`dispatch`] — see the
/// module docs.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// Path does not exist on the daemon side.
    #[error("not found")]
    NotFound,
    /// Path conflicts with the operation (e.g. PUT into a missing
    /// parent, MKCOL of an existing path).
    #[error("conflict")]
    Conflict,
    /// Request body exceeded the configured cap before reaching the
    /// daemon. Mapped to `413 Payload Too Large`.
    #[error("payload too large")]
    TooLarge,
    /// Catch-all for upstream IPC failures. The wrapped message is
    /// **not** echoed in the HTTP body — only logged — so the wire
    /// surface stays terse.
    #[error("upstream IPC failure: {0}")]
    Upstream(String),
}

/// One directory entry returned by [`IpcBackend::list_folder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendEntry {
    /// Leaf name (no slashes). The dispatcher joins this onto the
    /// request path to compose the response href.
    pub name: String,
    /// `true` for collections (folders).
    pub is_collection: bool,
    /// Length in bytes for files (`None` for collections or unknown).
    pub content_length: Option<u64>,
    /// IMF-fixdate string for the `D:getlastmodified` field, when the
    /// daemon surfaces it. Renderer omits the property when `None`.
    pub last_modified: Option<String>,
    /// MIME type for files. `None` for collections / unknowns.
    pub content_type: Option<String>,
}

/// Outcome returned by [`IpcBackend::put_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    /// The path did not exist before; the gateway returns `201
    /// Created`.
    Created,
    /// The path existed and was overwritten; the gateway returns
    /// `204 No Content` (matching RFC 4918 §9.7).
    Updated,
}

/// Pluggable backend the dispatcher calls for every WebDAV verb.
///
/// The concrete [`RemoteFsIpcBackend`](crate::RemoteFsIpcBackend) routes this
/// contract through owner-authenticated daemon IPC. Tests may also provide
/// in-memory implementations.
pub trait IpcBackend {
    /// List the immediate children of the collection at `path`.
    /// Returns `Err(NotFound)` when the collection itself is
    /// missing. Path is normalised origin-form (`/dav/photos`).
    fn list_folder(&self, path: &str) -> Result<Vec<BackendEntry>, BackendError>;

    /// Stat the resource at `path` so the dispatcher can describe
    /// the resource itself in addition to its children. Used by
    /// `PROPFIND Depth: 0` and as the leading entry in `Depth: 1`
    /// responses.
    fn stat(&self, path: &str) -> Result<BackendEntry, BackendError>;

    /// Read the full file body. The dispatcher buffers the result
    /// in memory — large files should be streamed by future
    /// extension; for the gateway scaffold this is correct.
    fn get_file(&self, path: &str) -> Result<Vec<u8>, BackendError>;

    /// Write `bytes` to `path`. Returns whether the path was newly
    /// created (`Created`) or overwritten (`Updated`).
    fn put_file(&mut self, path: &str, bytes: &[u8]) -> Result<PutOutcome, BackendError>;

    /// Delete the resource (file or empty collection) at `path`.
    fn delete(&mut self, path: &str) -> Result<(), BackendError>;

    /// Create a new collection at `path`. Returns `Conflict` when
    /// the path already exists or its parent is missing.
    fn mkdir(&mut self, path: &str) -> Result<(), BackendError>;
}

/// Allowed HTTP methods reported in `OPTIONS` and `405` responses.
const ALLOWED_METHODS: &str = "OPTIONS, GET, HEAD, PROPFIND, PUT, DELETE, MKCOL";

/// Dispatch one HTTP request to the backend and return the response.
///
/// `cfg.allow_writes = false` causes every mutating verb to return
/// `403 Forbidden` before reaching the backend, so a misconfigured
/// instance cannot accidentally accept uploads.
///
/// `cfg.max_put_body_bytes` is enforced here (the request body is
/// already bounded by the listener / parser; this is a second guard
/// for callers that bypass the parser).
pub fn dispatch(
    req: &HttpRequest,
    cfg: &ServerConfig,
    backend: &mut dyn IpcBackend,
) -> HttpResponse {
    match req.method.as_str() {
        "OPTIONS" => options_response(),
        "PROPFIND" => handle_propfind(req, backend),
        "GET" => handle_get(&req.path, backend, false),
        "HEAD" => handle_get(&req.path, backend, true),
        "PUT" => handle_put(req, cfg, backend),
        "DELETE" => handle_delete(&req.path, cfg, backend),
        "MKCOL" => handle_mkcol(&req.path, cfg, backend),
        _ => method_not_allowed(),
    }
}

fn options_response() -> HttpResponse {
    HttpResponse::status(200, "OK")
        .with_header("MS-Author-Via", "DAV")
        .with_header("X-pCloud-WebDAV-Status", "experimental-subset")
        .with_header("Allow", ALLOWED_METHODS)
}

fn method_not_allowed() -> HttpResponse {
    HttpResponse::status(405, "Method Not Allowed")
        .with_header("Allow", ALLOWED_METHODS)
        .with_body(b"method not allowed".to_vec())
}

fn forbidden_read_only() -> HttpResponse {
    HttpResponse::status(403, "Forbidden")
        .with_header("Content-Type", "text/plain; charset=utf-8")
        .with_body(b"WebDAV gateway is configured read-only".to_vec())
}

fn map_backend_error(err: BackendError) -> HttpResponse {
    match err {
        BackendError::NotFound => HttpResponse::status(404, "Not Found")
            .with_header("Content-Type", "text/plain; charset=utf-8")
            .with_body(b"not found".to_vec()),
        BackendError::Conflict => HttpResponse::status(409, "Conflict")
            .with_header("Content-Type", "text/plain; charset=utf-8")
            .with_body(b"conflict".to_vec()),
        BackendError::TooLarge => HttpResponse::status(413, "Payload Too Large")
            .with_header("Content-Type", "text/plain; charset=utf-8")
            .with_body(b"payload too large".to_vec()),
        BackendError::Upstream(msg) => {
            log::warn!("webdav: upstream IPC failure: {msg}");
            HttpResponse::status(500, "Internal Server Error")
                .with_header("Content-Type", "text/plain; charset=utf-8")
                .with_body(b"internal error".to_vec())
        }
    }
}

fn handle_propfind(req: &HttpRequest, backend: &mut dyn IpcBackend) -> HttpResponse {
    // Only a small subset of PROPFIND request shapes is structurally
    // distinct in our renderer (allprop / propname / named-props all
    // map to the same response shape today since the renderer emits
    // every property the daemon surfaces). Parsing the body is still
    // useful for malformed-request rejection.
    let body_text = match std::str::from_utf8(&req.body) {
        Ok(s) => s,
        Err(_) => {
            return HttpResponse::status(400, "Bad Request")
                .with_body(b"PROPFIND body must be UTF-8 XML".to_vec());
        }
    };
    if let Err(err) = parse_propfind_or_allprop(body_text) {
        return HttpResponse::status(400, "Bad Request")
            .with_body(format!("malformed PROPFIND body: {err}").into_bytes());
    }
    let depth = req.header("depth").unwrap_or("infinity");

    let self_entry = match backend.stat(&req.path) {
        Ok(e) => e,
        Err(err) => return map_backend_error(err),
    };
    let mut entries: Vec<PropfindResponseEntry> = Vec::new();
    entries.push(PropfindResponseEntry::ok(propfind_resource_from_path(
        &req.path,
        &self_entry,
    )));
    let want_children = match depth {
        "0" => false,
        "1" | "infinity" => self_entry.is_collection,
        _ => self_entry.is_collection,
    };
    if want_children {
        match backend.list_folder(&req.path) {
            Ok(children) => {
                for child in children {
                    let child_path = join_path(&req.path, &child.name);
                    entries.push(PropfindResponseEntry::ok(propfind_resource_from_path(
                        &child_path,
                        &child,
                    )));
                }
            }
            Err(err) => return map_backend_error(err),
        }
    }
    HttpResponse::ok_xml_multistatus(render_multistatus(&entries))
}

fn handle_get(path: &str, backend: &mut dyn IpcBackend, head_only: bool) -> HttpResponse {
    let stat = match backend.stat(path) {
        Ok(s) => s,
        Err(err) => return map_backend_error(err),
    };
    if stat.is_collection {
        // GET on a collection is implementation-defined; pick the
        // conservative reply rather than dumping XML for non-DAV
        // clients.
        return HttpResponse::status(405, "Method Not Allowed")
            .with_header("Allow", ALLOWED_METHODS)
            .with_body(b"GET on a collection is not supported".to_vec());
    }
    let body = match backend.get_file(path) {
        Ok(b) => b,
        Err(err) => return map_backend_error(err),
    };
    let mut resp = HttpResponse::status(200, "OK")
        .with_header(
            "Content-Type",
            stat.content_type
                .as_deref()
                .unwrap_or("application/octet-stream"),
        )
        .with_body(if head_only { Vec::new() } else { body });
    if let Some(modified) = stat.last_modified {
        resp = resp.with_header("Last-Modified", &modified);
    }
    resp
}

fn handle_put(req: &HttpRequest, cfg: &ServerConfig, backend: &mut dyn IpcBackend) -> HttpResponse {
    if !cfg.allow_writes {
        return forbidden_read_only();
    }
    if (req.body.len() as u64) > cfg.max_put_body_bytes {
        return map_backend_error(BackendError::TooLarge);
    }
    match backend.put_file(&req.path, &req.body) {
        Ok(PutOutcome::Created) => HttpResponse::status(201, "Created"),
        Ok(PutOutcome::Updated) => HttpResponse::status(204, "No Content"),
        Err(err) => map_backend_error(err),
    }
}

fn handle_delete(path: &str, cfg: &ServerConfig, backend: &mut dyn IpcBackend) -> HttpResponse {
    if !cfg.allow_writes {
        return forbidden_read_only();
    }
    match backend.delete(path) {
        Ok(()) => HttpResponse::status(204, "No Content"),
        Err(err) => map_backend_error(err),
    }
}

fn handle_mkcol(path: &str, cfg: &ServerConfig, backend: &mut dyn IpcBackend) -> HttpResponse {
    if !cfg.allow_writes {
        return forbidden_read_only();
    }
    match backend.mkdir(path) {
        Ok(()) => HttpResponse::status(201, "Created"),
        Err(err) => map_backend_error(err),
    }
}

fn propfind_resource_from_path(href: &str, entry: &BackendEntry) -> PropfindResource {
    PropfindResource {
        href: href.to_owned(),
        is_collection: entry.is_collection,
        content_length: if entry.is_collection {
            None
        } else {
            entry.content_length
        },
        last_modified: entry.last_modified.clone(),
        content_type: if entry.is_collection {
            None
        } else {
            entry.content_type.clone()
        },
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ListenerBinding;
    use std::collections::HashMap;
    use std::net::IpAddr;

    /// Mock backend: in-memory tree of (path → entry + maybe body).
    /// Suitable for unit-testing dispatch without touching IPC.
    #[derive(Default)]
    struct MockBackend {
        files: HashMap<String, (BackendEntry, Vec<u8>)>,
        folders: HashMap<String, BackendEntry>,
    }

    impl MockBackend {
        fn add_folder(&mut self, path: &str) {
            self.folders.insert(
                path.to_owned(),
                BackendEntry {
                    name: leaf(path),
                    is_collection: true,
                    content_length: None,
                    last_modified: None,
                    content_type: None,
                },
            );
        }
        fn add_file(&mut self, path: &str, body: &[u8], ctype: Option<&str>) {
            self.files.insert(
                path.to_owned(),
                (
                    BackendEntry {
                        name: leaf(path),
                        is_collection: false,
                        content_length: Some(body.len() as u64),
                        last_modified: None,
                        content_type: ctype.map(str::to_owned),
                    },
                    body.to_vec(),
                ),
            );
        }
    }

    fn leaf(path: &str) -> String {
        path.rsplit('/').next().unwrap_or("").to_owned()
    }

    impl IpcBackend for MockBackend {
        fn list_folder(&self, path: &str) -> Result<Vec<BackendEntry>, BackendError> {
            if !self.folders.contains_key(path) {
                return Err(BackendError::NotFound);
            }
            let prefix = if path.ends_with('/') {
                path.to_owned()
            } else {
                format!("{path}/")
            };
            let mut out = Vec::new();
            for (p, (entry, _)) in &self.files {
                if p.starts_with(&prefix) && !p[prefix.len()..].contains('/') {
                    out.push(entry.clone());
                }
            }
            for (p, entry) in &self.folders {
                if p == path {
                    continue;
                }
                if p.starts_with(&prefix) && !p[prefix.len()..].contains('/') {
                    out.push(entry.clone());
                }
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(out)
        }
        fn stat(&self, path: &str) -> Result<BackendEntry, BackendError> {
            if let Some(entry) = self.folders.get(path) {
                return Ok(entry.clone());
            }
            if let Some((entry, _)) = self.files.get(path) {
                return Ok(entry.clone());
            }
            Err(BackendError::NotFound)
        }
        fn get_file(&self, path: &str) -> Result<Vec<u8>, BackendError> {
            self.files
                .get(path)
                .map(|(_, body)| body.clone())
                .ok_or(BackendError::NotFound)
        }
        fn put_file(&mut self, path: &str, bytes: &[u8]) -> Result<PutOutcome, BackendError> {
            let outcome = if self.files.contains_key(path) {
                PutOutcome::Updated
            } else {
                PutOutcome::Created
            };
            self.files.insert(
                path.to_owned(),
                (
                    BackendEntry {
                        name: leaf(path),
                        is_collection: false,
                        content_length: Some(bytes.len() as u64),
                        last_modified: None,
                        content_type: Some("application/octet-stream".into()),
                    },
                    bytes.to_vec(),
                ),
            );
            Ok(outcome)
        }
        fn delete(&mut self, path: &str) -> Result<(), BackendError> {
            if self.files.remove(path).is_some() {
                return Ok(());
            }
            if self.folders.remove(path).is_some() {
                return Ok(());
            }
            Err(BackendError::NotFound)
        }
        fn mkdir(&mut self, path: &str) -> Result<(), BackendError> {
            if self.folders.contains_key(path) || self.files.contains_key(path) {
                return Err(BackendError::Conflict);
            }
            self.folders.insert(
                path.to_owned(),
                BackendEntry {
                    name: leaf(path),
                    is_collection: true,
                    content_length: None,
                    last_modified: None,
                    content_type: None,
                },
            );
            Ok(())
        }
    }

    fn req(method: &str, path: &str) -> HttpRequest {
        HttpRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn rw_cfg() -> ServerConfig {
        ServerConfig {
            binding: ListenerBinding::LocalTcp {
                host: IpAddr::from([127, 0, 0, 1]),
                port: 0,
            },
            max_put_body_bytes: 1024,
            allow_writes: true,
        }
    }

    fn ro_cfg() -> ServerConfig {
        ServerConfig {
            allow_writes: false,
            ..rw_cfg()
        }
    }

    #[test]
    fn options_demotes_compliance_and_advertises_subset() {
        let mut backend = MockBackend::default();
        let resp = dispatch(&req("OPTIONS", "/"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 200);
        assert!(
            !resp
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("DAV")),
            "an incomplete implementation must not claim a DAV compliance class"
        );
        assert!(resp.headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("X-pCloud-WebDAV-Status") && v == "experimental-subset"
        }));
        assert!(
            resp.headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("Allow") && v.contains("PROPFIND"))
        );
    }

    #[test]
    fn propfind_depth1_lists_children() {
        let mut backend = MockBackend::default();
        backend.add_folder("/dav");
        backend.add_file("/dav/cat.jpg", b"jpegbytes", Some("image/jpeg"));
        backend.add_file("/dav/notes.txt", b"hi", Some("text/plain"));
        let mut request = req("PROPFIND", "/dav");
        request.headers.push(("depth".into(), "1".into()));
        let resp = dispatch(&request, &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 207);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("/dav</D:href>"));
        assert!(body.contains("/dav/cat.jpg"));
        assert!(body.contains("/dav/notes.txt"));
        assert!(body.contains("<D:getcontenttype>image/jpeg"));
    }

    #[test]
    fn propfind_depth0_only_self() {
        let mut backend = MockBackend::default();
        backend.add_folder("/dav");
        backend.add_file("/dav/cat.jpg", b"x", None);
        let mut request = req("PROPFIND", "/dav");
        request.headers.push(("depth".into(), "0".into()));
        let resp = dispatch(&request, &rw_cfg(), &mut backend);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("/dav</D:href>"));
        // Children must NOT be present at depth 0.
        assert!(!body.contains("/dav/cat.jpg"));
    }

    #[test]
    fn propfind_missing_path_is_404() {
        let mut backend = MockBackend::default();
        let resp = dispatch(&req("PROPFIND", "/missing"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn get_returns_body_and_content_type() {
        let mut backend = MockBackend::default();
        backend.add_file("/dav/notes.txt", b"hello", Some("text/plain"));
        let resp = dispatch(&req("GET", "/dav/notes.txt"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello");
        assert!(resp
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("Content-Type") && v.starts_with("text/plain")));
    }

    #[test]
    fn head_returns_headers_without_body() {
        let mut backend = MockBackend::default();
        backend.add_file("/dav/x", b"abc", Some("text/plain"));
        let resp = dispatch(&req("HEAD", "/dav/x"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 200);
        assert!(resp.body.is_empty());
    }

    #[test]
    fn get_collection_is_405() {
        let mut backend = MockBackend::default();
        backend.add_folder("/dav");
        let resp = dispatch(&req("GET", "/dav"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 405);
    }

    #[test]
    fn get_missing_is_404() {
        let mut backend = MockBackend::default();
        let resp = dispatch(&req("GET", "/missing"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn put_creates_then_updates() {
        let mut backend = MockBackend::default();
        let mut request = req("PUT", "/dav/new.txt");
        request.body = b"first".to_vec();
        let resp = dispatch(&request, &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 201);
        let mut request2 = req("PUT", "/dav/new.txt");
        request2.body = b"second".to_vec();
        let resp = dispatch(&request2, &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 204);
        let stored = backend.get_file("/dav/new.txt").unwrap();
        assert_eq!(stored, b"second");
    }

    #[test]
    fn put_in_read_only_mode_is_403() {
        let mut backend = MockBackend::default();
        let mut request = req("PUT", "/dav/x");
        request.body = b"x".to_vec();
        let resp = dispatch(&request, &ro_cfg(), &mut backend);
        assert_eq!(resp.status, 403);
        assert!(backend.get_file("/dav/x").is_err());
    }

    #[test]
    fn put_above_body_cap_is_413() {
        let mut backend = MockBackend::default();
        let mut cfg = rw_cfg();
        cfg.max_put_body_bytes = 4;
        let mut request = req("PUT", "/dav/big");
        request.body = b"too long".to_vec();
        let resp = dispatch(&request, &cfg, &mut backend);
        assert_eq!(resp.status, 413);
    }

    #[test]
    fn delete_in_read_only_mode_is_403() {
        let mut backend = MockBackend::default();
        backend.add_file("/dav/x", b"x", None);
        let resp = dispatch(&req("DELETE", "/dav/x"), &ro_cfg(), &mut backend);
        assert_eq!(resp.status, 403);
        assert!(backend.get_file("/dav/x").is_ok());
    }

    #[test]
    fn delete_existing_returns_204() {
        let mut backend = MockBackend::default();
        backend.add_file("/dav/x", b"x", None);
        let resp = dispatch(&req("DELETE", "/dav/x"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 204);
        assert!(backend.get_file("/dav/x").is_err());
    }

    #[test]
    fn mkcol_existing_is_409() {
        let mut backend = MockBackend::default();
        backend.add_folder("/dav");
        let resp = dispatch(&req("MKCOL", "/dav"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 409);
    }

    #[test]
    fn mkcol_new_is_201() {
        let mut backend = MockBackend::default();
        let resp = dispatch(&req("MKCOL", "/dav/new"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 201);
        assert!(backend.stat("/dav/new").is_ok());
    }

    #[test]
    fn unknown_method_is_405() {
        let mut backend = MockBackend::default();
        let resp = dispatch(&req("LOCK", "/dav"), &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 405);
        assert!(
            resp.headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("Allow") && v.contains("PROPFIND"))
        );
    }

    #[test]
    fn propfind_malformed_body_is_400() {
        let mut backend = MockBackend::default();
        backend.add_folder("/dav");
        let mut request = req("PROPFIND", "/dav");
        request.body = b"<not-valid/>".to_vec();
        let resp = dispatch(&request, &rw_cfg(), &mut backend);
        assert_eq!(resp.status, 400);
    }
}

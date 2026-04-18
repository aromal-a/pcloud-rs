#![allow(clippy::pedantic)]
//! `pcloud-mockserver` — a tiny, in-process HTTP mock of the pCloud REST API.
//!
//! The mock is intentionally minimal: it binds to `127.0.0.1:0` (random
//! port), speaks just enough HTTP/1.1 to answer `GET` / `POST` requests, and
//! returns JSON bodies that match the shapes returned by the real pCloud
//! backend for a small set of endpoints used by our integration tests.
//!
//! ## Purpose
//!
//! This crate exists to unlock CI-friendly verification for flows that would
//! otherwise require real pCloud credentials (and therefore cannot run in the
//! default test matrix). The mock is:
//!
//! * **offline** — no outbound network is performed;
//! * **hermetic** — state lives in a `Mutex<MockState>` owned by the handle
//!   and is reset every time a new server is started;
//! * **credential-free** — it accepts a single well-known fake test token
//!   (see [`TEST_TOKEN`]) and rejects anything else with pCloud error
//!   [`ERR_INVALID_TOKEN`] (`"Invalid or expired login token."`).
//!
//! Because no real secrets are involved, there is no `SecretString` /
//! zeroize exposure; however the mock still refuses to echo tokens back in
//! error bodies to keep future real-token misuse from leaking into logs.
//!
//! ## Fixture scenarios covered
//!
//! Each mocked endpoint supports a distinct integration-test scenario. The
//! per-endpoint docs below list the scenario each fixture unlocks:
//!
//! * `/userinfo` — authenticated identity lookup (auth/TFA happy paths).
//! * `/listfolder` — virtual folder traversal used by sync-root validation.
//! * `/upload_create` / `/upload_write` / `/upload_save` — chunked upload
//!   state machine used by the transfer backend end-to-end.
//! * `/getfilepublink` / `/listpubs` — public-link create + list parity.
//! * `/listshares` / `/sharefolder` — outgoing share parity fixtures.
//! * `/listnotifications` / `/readnotifications` — notification inbox
//!   counter parity used by the daemon's notifier.
//! * `/createbackup` / `/stopdevice` — backup/device lifecycle parity.
//! * `/healthz` — unauthenticated liveness probe (used by readiness tests).
//!
//! Any endpoint URL can additionally be suffixed with `?inject_error=N` to
//! force that request to return pCloud-style error code `N`; this powers
//! negative-path tests (timeout, invalid-token, quota, etc.) without needing
//! a dedicated endpoint per error.
//!
//! ## Not a production feature
//!
//! `pcloud-mockserver` is a test-only crate. It MUST NOT be depended on by
//! any production/runtime code path. The daemon, CLI, and SDK do not link
//! it. All endpoints are hard-coded stubs; they are not RFC-correct HTTP
//! servers and do not implement pipelining, chunked encoding, or TLS.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

/// The single well-known auth token the mock accepts.
///
/// Any request that presents a different token (via `?auth=…` query string
/// or `Authorization: Bearer …` header) receives pCloud result code
/// [`ERR_INVALID_TOKEN`]. The value is deliberately high-entropy-looking so
/// that accidentally checking it into a real keystore is obvious, but it is
/// **not** a secret — it is a hard-coded public test constant used only by
/// the mock and by tests that talk to it.
///
/// **TEST-ONLY.** Never use this value outside of test code or the mock
/// server. Secret-scanning rules must be configured to ignore this
/// constant (it is a known-fake value, not a live credential).
///
/// Scenario: any test that needs an authenticated endpoint uses this token.
pub const TEST_TOKEN: &str = "MOCK-TEST-TOKEN-0000000000000000";

/// pCloud-style error code for an invalid or expired login token.
///
/// Returned by the mock when a request presents any token other than
/// [`TEST_TOKEN`] to a non-public endpoint.
///
/// Scenario: negative-path auth tests that assert a client correctly
/// surfaces token rotation / expiry.
pub const ERR_INVALID_TOKEN: u64 = 2094;

/// pCloud-style error code for "Log in required."
///
/// Returned by the mock when a request omits an auth token entirely against
/// a non-public endpoint.
///
/// Scenario: tests that verify unauthenticated callers are rejected before
/// any side effect occurs.
pub const ERR_LOGIN_REQUIRED: u64 = 1000;

/// Generic "injected" failure code used by the `?inject_error=N` hook.
///
/// Callers pick any `N` they want — the mock echoes it back verbatim as the
/// `result` field. This constant is the conventional default for tests that
/// don't need a specific pCloud error variant.
///
/// Scenario: error-handling tests that assert the client propagates a
/// backend failure without panicking, without leaking state, and without
/// silently succeeding.
pub const ERR_GENERIC_INJECTED: u64 = 5000;

/// Handle returned by [`MockServer::start`].
///
/// Owns the accept thread and the shared [`MockState`]. Dropping the handle
/// signals the accept loop to exit, unblocks `accept()` with a local poke
/// connection, and joins the thread. [`shutdown`](Self::shutdown) is the
/// explicit equivalent and is useful when a test wants to assert the
/// shutdown path itself.
///
/// A `MockHandle` is `!Clone` on purpose: there must be exactly one owner
/// of the server lifecycle so that Drop semantics stay deterministic.
pub struct MockHandle {
    base_url: String,
    shutdown_flag: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    state: Arc<Mutex<MockState>>,
    local_addr: SocketAddr,
}

impl MockHandle {
    /// Returns the base URL of the form `http://127.0.0.1:<port>` (no
    /// trailing slash).
    ///
    /// Tests build endpoint URLs by appending `/userinfo?auth=…` etc.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the bound local socket address.
    ///
    /// Useful for tests that want the chosen port explicitly (for example
    /// to configure a daemon pointed at this mock via a `SocketAddr`
    /// rather than a URL string).
    pub fn addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns an `Arc` clone of the shared server state.
    ///
    /// Tests use this to seed fixtures (e.g. inserting a [`MockFile`] before
    /// making a request) or to assert post-conditions (e.g. that a
    /// `sharefolder` call actually recorded a [`ShareEntry`]). The lock is
    /// a `Mutex`, so holders must keep the guard scope short to avoid
    /// stalling the accept loop.
    pub fn state(&self) -> Arc<Mutex<MockState>> {
        Arc::clone(&self.state)
    }

    /// Explicitly shut the server down and join the accept thread.
    ///
    /// Equivalent to letting the handle drop, except that it takes
    /// ownership so a test can assert shutdown happened at a specific
    /// point rather than at scope exit.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
        // Poke the listener so `accept()` unblocks.
        let _ = std::net::TcpStream::connect_timeout(&self.local_addr, Duration::from_millis(250));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for MockHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// In-memory state shared by all requests.
///
/// Tests reach into this via [`MockHandle::state`] to assert what the
/// server saw (post-condition checks) or to seed data before making a
/// request (fixture setup). The struct is deliberately plain-old-data: all
/// fields are `pub` so tests can mutate freely without going through a
/// builder.
///
/// No secrets are stored here — the mock operates entirely on synthetic
/// bytes, fake emails, and a single hard-coded token constant.
#[derive(Debug, Default)]
pub struct MockState {
    /// Map of `fileid -> MockFile`, built up by the
    /// `upload_create` / `upload_write` / `upload_save` fixture chain. Also
    /// seedable directly to exercise download / public-link scenarios.
    pub files: HashMap<u64, MockFile>,
    /// Map of `folderid -> folder name` representing a very small virtual
    /// tree. Entry `0` is always the synthetic root `"/"`.
    pub folders: HashMap<u64, String>,
    /// Outstanding chunked-upload sessions keyed by the `uploadid` returned
    /// from `/upload_create`. The `Vec<u8>` accumulates bytes written via
    /// `/upload_write` until `/upload_save` consumes it.
    pub uploads: HashMap<u64, Vec<u8>>,
    /// Created share entries, keyed by mock `shareid`. Exposed so shares
    /// tests can assert outgoing share creation without parsing JSON.
    pub shares: HashMap<u64, ShareEntry>,
    /// Created public links, keyed by `linkid` mapping to the opaque
    /// `code`. Supports public-link create/list scenarios.
    pub public_links: HashMap<u64, String>,
    /// Unread notifications counter surfaced by `/listnotifications` as
    /// `nnew`. Reset to `0` by `/readnotifications`.
    pub unread_notifications: u64,
    /// Monotonic id generator shared across files, folders, uploads,
    /// shares, and public links. Starts at `1_000` so tests can hard-code
    /// small ids (0..1000) for seeded fixtures without colliding.
    pub next_id: u64,
    /// Bound base URL (convenience — duplicates [`MockHandle::base_url`]).
    pub base_url: String,
}

/// Seeded or synthetic file entry stored inside the mock server's state.
///
/// Scenario: download, `listfolder`, and public-link tests seed a
/// `MockFile` directly via [`MockHandle::state`] to avoid having to drive
/// the full upload state machine when only the read path is under test.
#[derive(Debug, Clone)]
pub struct MockFile {
    /// Basename of the file as surfaced to clients (no path component).
    pub name: String,
    /// Parent folder id the file is attached to; use `0` for the virtual
    /// root.
    pub parent_folder_id: u64,
    /// Raw bytes returned on `getfilelink` / download requests.
    pub bytes: Vec<u8>,
}

/// Synthetic share entry exposed by the mock's `/listshares` response.
///
/// Scenario: outgoing-share tests assert that the client encoded
/// `folderid`, `mail`, and `permissions` correctly on the wire. The field
/// shapes mirror the real pCloud REST surface.
#[derive(Debug, Clone)]
pub struct ShareEntry {
    /// Folder id being shared.
    pub folderid: u64,
    /// Email address of the share target (fake domain in tests).
    pub mail: String,
    /// Permission bitmask, matches the pCloud REST `permissions` field.
    pub permissions: u64,
}

impl MockState {
    fn new() -> Self {
        let mut s = Self {
            next_id: 1_000,
            ..Self::default()
        };
        // Seed a root folder.
        s.folders.insert(0, "/".to_owned());
        s.unread_notifications = 0;
        s
    }

    /// Allocate a fresh monotonic id.
    ///
    /// Exposed for tests that seed fixtures directly (files, folders,
    /// shares, public links). The counter is shared across all entity
    /// kinds, so ids are globally unique within a single mock lifetime.
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// Entry point for starting the mock server.
///
/// This is a zero-sized type used purely as a namespace for
/// [`MockServer::start`]. It exists so call sites read as
/// `MockServer::start()` rather than a loose free function, matching the
/// style of other test harness crates in the workspace.
pub struct MockServer;

impl MockServer {
    /// Bind to `127.0.0.1:0` (OS-assigned random port) and spawn the
    /// accept loop.
    ///
    /// Returns a [`MockHandle`] that exposes the chosen port, the shared
    /// [`MockState`], and an explicit shutdown trigger. Every call starts
    /// an independent server with fresh state — callers can run several
    /// mocks concurrently in the same process without interference.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` if the listener cannot bind (for example the
    /// loopback interface is unavailable) or if the accept thread cannot
    /// be spawned.
    pub fn start() -> std::io::Result<MockHandle> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        // Nonblocking accept via short read timeout on the listener is not
        // portable; instead we rely on the shutdown poke to unblock accept().
        let local_addr = listener.local_addr()?;
        let base_url = format!("http://{local_addr}");
        let state = Arc::new(Mutex::new({
            let mut st = MockState::new();
            st.base_url = base_url.clone();
            st
        }));
        let shutdown_flag = Arc::new(AtomicBool::new(false));

        let state_thr = Arc::clone(&state);
        let shutdown_thr = Arc::clone(&shutdown_flag);
        let join = thread::Builder::new()
            .name("pcloud-mockserver".into())
            .spawn(move || accept_loop(listener, state_thr, shutdown_thr))?;

        Ok(MockHandle {
            base_url,
            shutdown_flag,
            join: Some(join),
            state,
            local_addr,
        })
    }
}

fn accept_loop(listener: TcpListener, state: Arc<Mutex<MockState>>, shutdown: Arc<AtomicBool>) {
    // Short timeout so we can re-check the shutdown flag periodically.
    for stream in listener.incoming() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        match stream {
            Ok(s) => {
                let st = Arc::clone(&state);
                // Per-connection thread keeps the server trivially correct.
                let _ = thread::Builder::new()
                    .name("pcloud-mockserver-conn".into())
                    .spawn(move || {
                        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                        handle_connection(s, st);
                    });
            }
            Err(_) => break,
        }
    }
}

struct ParsedRequest {
    // Captured HTTP verb. Currently not consulted by the mock dispatcher
    // (all mocked endpoints accept any verb) but retained so future mock
    // routes can reject wrong verbs without restructuring the parser.
    #[allow(dead_code)]
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn handle_connection(mut stream: TcpStream, state: Arc<Mutex<MockState>>) {
    let req = match read_request(&mut stream) {
        Some(r) => r,
        None => return,
    };
    let resp = dispatch(&req, &state);
    let _ = stream.write_all(&resp);
    let _ = stream.flush();
}

fn read_request(stream: &mut TcpStream) -> Option<ParsedRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let full_path = parts.next()?.to_owned();

    let (path, raw_query) = match full_path.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (full_path.clone(), String::new()),
    };
    let query = parse_query(&raw_query);

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }

    let mut body = Vec::new();
    if let Some(len) = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        && len > 0
    {
        body.resize(len, 0);
        reader.read_exact(&mut body).ok()?;
    }

    Some(ParsedRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn parse_query(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for kv in raw.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        out.insert(url_decode(k), url_decode(v));
    }
    out
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

fn auth_token_from(req: &ParsedRequest) -> Option<String> {
    if let Some(t) = req.query.get("auth") {
        return Some(t.clone());
    }
    // Also accept Authorization: Bearer ...
    if let Some(h) = req.headers.get("authorization")
        && let Some(rest) = h.strip_prefix("Bearer ")
    {
        return Some(rest.to_owned());
    }
    None
}

/// Routes that do not require a valid token.
fn is_public_endpoint(path: &str) -> bool {
    matches!(path, "/userinfo_unauthenticated" | "/healthz")
}

fn dispatch(req: &ParsedRequest, state: &Arc<Mutex<MockState>>) -> Vec<u8> {
    // Error injection hook: ?inject_error=N wins over anything else.
    if let Some(code) = req
        .query
        .get("inject_error")
        .and_then(|v| v.parse::<u64>().ok())
    {
        return json_response(200, &error_body(code, "Injected error"));
    }

    // Token validation (skipped for public endpoints).
    if !is_public_endpoint(&req.path) {
        match auth_token_from(req) {
            Some(t) if t == TEST_TOKEN => {}
            Some(_) => {
                return json_response(
                    200,
                    &error_body(ERR_INVALID_TOKEN, "Invalid or expired login token."),
                );
            }
            None => return json_response(200, &error_body(ERR_LOGIN_REQUIRED, "Log in required.")),
        }
    }

    let mut state = state.lock().expect("mock state poisoned");
    match req.path.as_str() {
        "/healthz" => json_response(200, &json!({"result": 0, "status": "ok"})),
        "/userinfo" => handle_userinfo(),
        "/listfolder" => handle_listfolder(&mut state, req),
        "/getfilepublink" => handle_getfilepublink(&mut state, req),
        "/listpubs" | "/listpublinks" => handle_listpubs(&state),
        "/upload_create" => handle_upload_create(&mut state, req),
        "/upload_write" => handle_upload_write(&mut state, req),
        "/upload_save" => handle_upload_save(&mut state, req),
        "/listnotifications" => handle_listnotifications(&state),
        "/readnotifications" => handle_readnotifications(&mut state),
        "/listshares" => handle_listshares(&state),
        "/sharefolder" => handle_sharefolder(&mut state, req),
        "/createbackup" => handle_createbackup(&mut state, req),
        "/stopdevice" => handle_stopdevice(&mut state, req),
        _ => json_response(
            404,
            &error_body(2002, "A component of parent directory does not exist."),
        ),
    }
}

fn handle_userinfo() -> Vec<u8> {
    json_response(
        200,
        &json!({
            "result": 0,
            "userid": 42,
            "email": "mock@example.invalid",
            "emailverified": true,
            "premium": false,
            "quota": 10_737_418_240u64,  // 10 GiB
            "usedquota": 1_024u64,
            "language": "en",
            "registered": "2026-01-01 00:00:00",
        }),
    )
}

fn handle_listfolder(state: &mut MockState, req: &ParsedRequest) -> Vec<u8> {
    let folderid: u64 = req
        .query
        .get("folderid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let contents: Vec<Value> = state
        .files
        .iter()
        .filter(|(_, f)| f.parent_folder_id == folderid)
        .map(|(id, f)| {
            json!({
                "isfolder": false,
                "fileid": id,
                "name": f.name,
                "size": f.bytes.len(),
            })
        })
        .collect();
    json_response(
        200,
        &json!({
            "result": 0,
            "metadata": {
                "folderid": folderid,
                "name": state.folders.get(&folderid).cloned().unwrap_or_else(|| "/".to_owned()),
                "isfolder": true,
                "contents": contents,
            }
        }),
    )
}

fn handle_getfilepublink(state: &mut MockState, req: &ParsedRequest) -> Vec<u8> {
    let fileid: u64 = req
        .query
        .get("fileid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if !state.files.contains_key(&fileid) {
        return json_response(200, &error_body(2009, "File not found."));
    }
    let linkid = state.alloc_id();
    let code = format!("XMock{linkid}");
    state.public_links.insert(linkid, code.clone());
    json_response(
        200,
        &json!({
            "result": 0,
            "linkid": linkid,
            "link": format!("https://my.pcloud.com/publink/show?code={code}"),
            "code": code,
        }),
    )
}

fn handle_listpubs(state: &MockState) -> Vec<u8> {
    let pubs: Vec<Value> = state
        .public_links
        .iter()
        .map(|(id, code)| json!({"linkid": id, "code": code}))
        .collect();
    json_response(200, &json!({"result": 0, "publinks": pubs}))
}

fn handle_upload_create(state: &mut MockState, _req: &ParsedRequest) -> Vec<u8> {
    let upload_id = state.alloc_id();
    state.uploads.insert(upload_id, Vec::new());
    json_response(200, &json!({"result": 0, "uploadid": upload_id}))
}

fn handle_upload_write(state: &mut MockState, req: &ParsedRequest) -> Vec<u8> {
    let upload_id: u64 = match req.query.get("uploadid").and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return json_response(200, &error_body(1002, "uploadid missing")),
    };
    let offset: usize = req
        .query
        .get("uploadoffset")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let Some(buf) = state.uploads.get_mut(&upload_id) else {
        return json_response(200, &error_body(2037, "Upload not found."));
    };
    if buf.len() < offset + req.body.len() {
        buf.resize(offset + req.body.len(), 0);
    }
    buf[offset..offset + req.body.len()].copy_from_slice(&req.body);
    json_response(200, &json!({"result": 0, "bytes_written": req.body.len()}))
}

fn handle_upload_save(state: &mut MockState, req: &ParsedRequest) -> Vec<u8> {
    let upload_id: u64 = match req.query.get("uploadid").and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return json_response(200, &error_body(1002, "uploadid missing")),
    };
    let name = req
        .query
        .get("name")
        .cloned()
        .unwrap_or_else(|| "unnamed.bin".to_owned());
    let folderid: u64 = req
        .query
        .get("folderid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let Some(bytes) = state.uploads.remove(&upload_id) else {
        return json_response(200, &error_body(2037, "Upload not found."));
    };
    let file_id = state.alloc_id();
    let size = bytes.len();
    state.files.insert(
        file_id,
        MockFile {
            name: name.clone(),
            parent_folder_id: folderid,
            bytes,
        },
    );
    json_response(
        200,
        &json!({
            "result": 0,
            "metadata": [{
                "fileid": file_id,
                "name": name,
                "parentfolderid": folderid,
                "size": size,
                "isfolder": false,
            }]
        }),
    )
}

fn handle_listnotifications(state: &MockState) -> Vec<u8> {
    json_response(
        200,
        &json!({
            "result": 0,
            "notificationid": 0,
            "notifications": [],
            "ntotal": 0,
            "nnew": state.unread_notifications,
        }),
    )
}

fn handle_readnotifications(state: &mut MockState) -> Vec<u8> {
    state.unread_notifications = 0;
    json_response(200, &json!({"result": 0}))
}

fn handle_listshares(state: &MockState) -> Vec<u8> {
    let shares: Vec<Value> = state
        .shares
        .iter()
        .map(|(id, s)| {
            json!({
                "shareid": id, "folderid": s.folderid,
                "tomail": s.mail, "permissions": s.permissions,
            })
        })
        .collect();
    json_response(
        200,
        &json!({
            "result": 0,
            "shares": { "incoming": [], "outgoing": shares },
        }),
    )
}

fn handle_sharefolder(state: &mut MockState, req: &ParsedRequest) -> Vec<u8> {
    let folderid: u64 = req
        .query
        .get("folderid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mail = req.query.get("mail").cloned().unwrap_or_default();
    let permissions: u64 = req
        .query
        .get("permissions")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    if mail.is_empty() {
        return json_response(200, &error_body(2025, "Invalid 'mail' parameter."));
    }
    let shareid = state.alloc_id();
    state.shares.insert(
        shareid,
        ShareEntry {
            folderid,
            mail,
            permissions,
        },
    );
    json_response(200, &json!({"result": 0, "shareid": shareid}))
}

fn handle_createbackup(state: &mut MockState, req: &ParsedRequest) -> Vec<u8> {
    let backup_name = req
        .query
        .get("name")
        .cloned()
        .unwrap_or_else(|| "backup".to_owned());
    let folder_id = state.alloc_id();
    state.folders.insert(folder_id, backup_name.clone());
    json_response(
        200,
        &json!({"result": 0, "folderid": folder_id, "name": backup_name}),
    )
}

fn handle_stopdevice(_state: &mut MockState, req: &ParsedRequest) -> Vec<u8> {
    let device_id: u64 = req
        .query
        .get("deviceid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    json_response(
        200,
        &json!({"result": 0, "deviceid": device_id, "stopped": true}),
    )
}

fn error_body(code: u64, msg: &str) -> Value {
    json!({"result": code, "error": msg})
}

fn json_response(status: u16, body: &Value) -> Vec<u8> {
    let body_bytes = serde_json::to_vec(body).expect("canned JSON must serialize");
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "OK",
    };
    let mut resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    ).into_bytes();
    resp.extend_from_slice(&body_bytes);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_get(url: &str) -> (u16, String) {
        // Parse scheme://host:port/path?q into SocketAddr + request line.
        let rest = url.strip_prefix("http://").expect("http url");
        let (authority, path_and_query) = match rest.split_once('/') {
            Some((a, r)) => (a.to_owned(), format!("/{r}")),
            None => (rest.to_owned(), "/".to_owned()),
        };
        let addr: SocketAddr = authority.parse().expect("sockaddr");
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
        let req = format!(
            "GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf).to_string();
        let status: u16 = text
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
        (status, body)
    }

    fn http_post(url: &str, body: &[u8]) -> (u16, String) {
        let rest = url.strip_prefix("http://").expect("http url");
        let (authority, path_and_query) = match rest.split_once('/') {
            Some((a, r)) => (a.to_owned(), format!("/{r}")),
            None => (rest.to_owned(), "/".to_owned()),
        };
        let addr: SocketAddr = authority.parse().expect("sockaddr");
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
        let head = format!(
            "POST {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf).to_string();
        let status: u16 = text
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
        (status, body)
    }

    #[test]
    fn starts_and_serves_userinfo_with_valid_token() {
        let h = MockServer::start().unwrap();
        let (status, body) = http_get(&format!("{}/userinfo?auth={TEST_TOKEN}", h.base_url()));
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        assert_eq!(v["email"], "mock@example.invalid");
    }

    #[test]
    fn rejects_unknown_token_with_2094() {
        let h = MockServer::start().unwrap();
        let (_, body) = http_get(&format!("{}/userinfo?auth=nope", h.base_url()));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 2094);
    }

    #[test]
    fn inject_error_wins() {
        let h = MockServer::start().unwrap();
        let (_, body) = http_get(&format!(
            "{}/userinfo?auth={TEST_TOKEN}&inject_error=2000",
            h.base_url()
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 2000);
    }

    #[test]
    fn upload_roundtrip_persists_file() {
        let h = MockServer::start().unwrap();
        let base = h.base_url().to_owned();

        // create
        let (_, body) = http_get(&format!("{base}/upload_create?auth={TEST_TOKEN}"));
        let v: Value = serde_json::from_str(&body).unwrap();
        let upload_id = v["uploadid"].as_u64().unwrap();

        // write
        let payload: Vec<u8> = (0u8..64).collect();
        let (_, _) = http_post(
            &format!("{base}/upload_write?auth={TEST_TOKEN}&uploadid={upload_id}&uploadoffset=0"),
            &payload,
        );
        // save
        let (_, body) = http_get(&format!(
            "{base}/upload_save?auth={TEST_TOKEN}&uploadid={upload_id}&name=t.bin&folderid=0"
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        let meta = &v["metadata"][0];
        let file_id = meta["fileid"].as_u64().unwrap();
        assert_eq!(meta["size"], 64);

        // listfolder shows it
        let (_, body) = http_get(&format!("{base}/listfolder?auth={TEST_TOKEN}&folderid=0"));
        let v: Value = serde_json::from_str(&body).unwrap();
        let contents = v["metadata"]["contents"].as_array().unwrap();
        assert!(contents.iter().any(|e| e["fileid"] == file_id));
    }

    #[test]
    fn sharefolder_and_listshares() {
        let h = MockServer::start().unwrap();
        let base = h.base_url().to_owned();
        let (_, body) = http_get(&format!(
            "{base}/sharefolder?auth={TEST_TOKEN}&folderid=0&mail=a@b.invalid&permissions=1"
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        let (_, body) = http_get(&format!("{base}/listshares?auth={TEST_TOKEN}"));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        assert_eq!(v["shares"]["outgoing"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn notifications_mark_read() {
        let h = MockServer::start().unwrap();
        {
            let st_arc = h.state();
            let mut st = st_arc.lock().unwrap();
            st.unread_notifications = 5;
        }
        let (_, body) = http_get(&format!(
            "{}/listnotifications?auth={TEST_TOKEN}",
            h.base_url()
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["nnew"], 5);
        let (_, body) = http_get(&format!(
            "{}/readnotifications?auth={TEST_TOKEN}",
            h.base_url()
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        let (_, body) = http_get(&format!(
            "{}/listnotifications?auth={TEST_TOKEN}",
            h.base_url()
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["nnew"], 0);
    }

    #[test]
    fn createbackup_and_stopdevice() {
        let h = MockServer::start().unwrap();
        let (_, body) = http_get(&format!(
            "{}/createbackup?auth={TEST_TOKEN}&name=my-backup",
            h.base_url()
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        assert_eq!(v["name"], "my-backup");

        let (_, body) = http_get(&format!(
            "{}/stopdevice?auth={TEST_TOKEN}&deviceid=7",
            h.base_url()
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        assert_eq!(v["deviceid"], 7);
        assert_eq!(v["stopped"], true);
    }

    #[test]
    fn getfilepublink_and_listpubs() {
        let h = MockServer::start().unwrap();
        let base = h.base_url().to_owned();

        // Seed a file directly.
        let file_id = {
            let st_arc = h.state();
            let mut st = st_arc.lock().unwrap();
            let fid = st.alloc_id();
            st.files.insert(
                fid,
                MockFile {
                    name: "x.bin".into(),
                    parent_folder_id: 0,
                    bytes: vec![1, 2, 3],
                },
            );
            fid
        };

        let (_, body) = http_get(&format!(
            "{base}/getfilepublink?auth={TEST_TOKEN}&fileid={file_id}"
        ));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        assert!(v["link"].as_str().unwrap().contains("publink/show?code="));

        let (_, body) = http_get(&format!("{base}/listpubs?auth={TEST_TOKEN}"));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"], 0);
        assert_eq!(v["publinks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn shutdown_joins_thread() {
        let h = MockServer::start().unwrap();
        // Explicit shutdown.
        h.shutdown();
    }
}

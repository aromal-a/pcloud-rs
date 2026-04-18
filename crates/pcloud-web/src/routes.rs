//! HTTP routes for the single-user Web UI.
//!
//! Route map:
//!
//! - `GET  /`              — status landing page (also issues CSRF cookie).
//! - `GET  /api/status`    — JSON mirror of landing page.
//! - `GET  /health`        — liveness probe (no IPC).
//! - `GET  /sync`          — HTML list of sync roots + add form.
//! - `POST /sync`          — add a sync root (CSRF required).
//! - `DELETE /sync/{id}`   — remove a sync root (CSRF required).
//! - `GET  /publinks`      — HTML list of active public links + create form.
//! - `POST /publinks`      — create a public link (CSRF required).
//! - `DELETE /publinks/{code}` — revoke a public link (CSRF required).
//! - `GET  /activity`      — recent audit events; HTML or NDJSON.
//! - `GET  /settings`      — redacted config view.
//! - `GET  /metrics`       — 404 (metrics feature not compiled in).
//!
//! CSRF uses the **double-submit cookie** pattern: `GET /` sets a
//! random, opaque `pcw_csrf` cookie; every mutating handler requires a
//! matching `X-CSRF-Token` request header. Because the cookie is
//! `HttpOnly; SameSite=Strict` only the same-origin (loopback-only)
//! caller can read it and submit it back.
//!
//! All responses intended for browsers carry a restrictive CSP and
//! `X-Content-Type-Options: nosniff`. Any daemon-sourced field is
//! HTML-escaped via [`crate::templates::escape`] (re-exported here as
//! [`xml_escape`]) before interpolation.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::Path;
use std::sync::atomic::Ordering;

use axum::{
    Form, Router,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use pcloud_ipc::{IpcClient, Method, Request, Response as IpcResponse, ResponseStatus};
use pcloud_model::public_links::PublicLinkUploadPolicy;
use pcloud_model::sync::SyncType;
use pcloud_secret::ExposeSecret;
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::{AppState, templates};

/// HTML-entity escape helper. Publicly re-exported so handler-local
/// `format!` interpolations can safely embed daemon-sourced text.
pub(crate) use crate::templates::escape as xml_escape;

/// Content-Security-Policy applied to every HTML response.
///
/// `default-src 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'`.
/// Inline `<style>` is tolerated for the base chrome; inline/external
/// scripts are flatly forbidden.
const CSP: &str = "default-src 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'";

/// Cookie name for the double-submit CSRF token.
const CSRF_COOKIE: &str = "pcw_csrf";
/// Request header the caller must echo the cookie value into.
const CSRF_HEADER: &str = "x-csrf-token";
/// Request header that mutating routes require for session authentication.
/// The value must match the token logged at daemon startup.
const WEB_TOKEN_HEADER: &str = "x-pcloud-web-token";

/// Build the Axum router with the provided shared [`AppState`].
pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(api_status))
        .route("/health", get(health))
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
        .route("/sync", get(sync_list).post(sync_add))
        .route("/sync/:id", delete(sync_remove))
        .route("/publinks", get(publinks_list).post(publinks_create))
        .route("/publinks/:code", delete(publinks_revoke))
        .route("/activity", get(activity))
        .route("/settings", get(settings))
        .route("/metrics", get(metrics))
        .with_state(state)
}

// -------------------------------------------------------------------
// Core / misc
// -------------------------------------------------------------------

/// `GET /health` — liveness probe. Never touches the daemon.
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// `GET /livez` — Kubernetes-style liveness probe.
///
/// Returns 200 `"ok"` unconditionally: if the process is running and
/// the HTTP server is accepting requests, the process is alive.
/// Orchestrators that use `/livez` for liveness should only restart the
/// container when this returns non-200 (i.e. the web server itself is
/// wedged), not when the daemon is temporarily unreachable.
async fn livez() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// `GET /readyz` — Kubernetes-style readiness probe.
///
/// Returns 200 `"ok"` when the daemon has completed initialization and
/// set the shared `ready` flag to `true`, or 503 `"not ready"` while
/// initialization is still in progress. Orchestrators should not route
/// traffic to this instance until this endpoint returns 200.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if state.ready.load(Ordering::Acquire) {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

/// `GET /` — HTML landing page + CSRF cookie issuance.
async fn index(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let status = fetch_status(&state.socket_path).await;
    let token = existing_or_new_csrf(&headers);
    let body = templates::render_index(&status);
    html_response_with_csrf(body, &token)
}

/// `GET /api/status` — JSON mirror of the landing page.
async fn api_status(State(state): State<AppState>) -> Response {
    let status = fetch_status(&state.socket_path).await;
    let body = json!({
        "online": status.online,
        "message": status.message,
        "sync_root_count": status.sync_root_count,
        "mount_state": status.mount_state,
        "raw": status.raw,
    });
    json_response(&body)
}

// -------------------------------------------------------------------
// Sync roots
// -------------------------------------------------------------------

/// `GET /sync` — list sync roots + add form.
async fn sync_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = existing_or_new_csrf(&headers);
    let ipc = call_ipc(
        &state.socket_path,
        Request::Plain {
            method: Method::GetSyncRoots,
        },
    )
    .await;
    let pending = call_ipc(
        &state.socket_path,
        Request::Plain {
            method: Method::GetPending,
        },
    )
    .await;

    let (raw, online) = raw_and_online(&ipc);
    let (pending_raw, _) = raw_and_online(&pending);

    let body = render_sync_page(online, &raw, &pending_raw, &token);
    html_response_with_csrf(body, &token)
}

/// Form payload for `POST /sync`.
#[derive(Debug, Deserialize)]
struct SyncAddForm {
    local_path: String,
    remote_path: String,
    /// Optional: "full" (default), "download", "upload".
    #[serde(default)]
    sync_type: String,
}

/// `POST /sync` — add a sync root. Session token and CSRF required.
async fn sync_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SyncAddForm>,
) -> Response {
    if let Err(resp) = require_web_token(&headers, state.web_token.expose_secret()) {
        return resp;
    }
    if let Err(resp) = require_csrf(&headers) {
        return resp;
    }
    // Map the optional form sync_type token to the typed enum. Anything
    // unrecognised silently falls back to `None` (daemon default = Full).
    // The CLI is the authoritative surface for the 9-alias parser; the
    // web form accepts the three narrow tokens that the UI emits.
    let sync_type = match form.sync_type.trim().to_ascii_lowercase().as_str() {
        "" | "full" | "both" | "bilateral" => None,
        "download" | "download-only" | "mirror" | "down" => Some(SyncType::DownloadOnly),
        "upload" | "upload-only" | "backup" | "up" => Some(SyncType::UploadOnly),
        _ => None,
    };
    let req = Request::SyncRootAdd {
        local_path: form.local_path,
        remote_path: form.remote_path,
        sync_type,
    };
    let ipc = call_ipc(&state.socket_path, req).await;

    ipc_redirect_response(ipc, "/sync")
}

/// `DELETE /sync/{id}` — remove a sync root. Session token and CSRF required.
async fn sync_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
) -> Response {
    if let Err(resp) = require_web_token(&headers, state.web_token.expose_secret()) {
        return resp;
    }
    if let Err(resp) = require_csrf(&headers) {
        return resp;
    }
    let ipc = call_ipc(&state.socket_path, Request::SyncRootRemove { sync_id: id }).await;
    ipc_plain_response(ipc)
}

// -------------------------------------------------------------------
// Public links
// -------------------------------------------------------------------

/// `GET /publinks` — list active public links + create form.
async fn publinks_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = existing_or_new_csrf(&headers);
    let ipc = call_ipc(
        &state.socket_path,
        Request::Plain {
            method: Method::ListPublicLinks,
        },
    )
    .await;
    let (raw, online) = raw_and_online(&ipc);
    let body = render_publinks_page(online, &raw, &token);
    html_response_with_csrf(body, &token)
}

/// Form payload for `POST /publinks`.
#[derive(Debug, Deserialize)]
struct PublinkCreateForm {
    path: String,
    /// Optional UNIX-seconds expiry. Empty/omitted means no expiry.
    #[serde(default)]
    expiry: String,
    /// Optional link password. Empty means none.
    ///
    /// Zeroized on `Drop` so the cleartext password does not linger in
    /// heap memory after the request handler completes.
    #[serde(default)]
    password: String,
}

impl Drop for PublinkCreateForm {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.password.zeroize();
    }
}

/// `POST /publinks` — create a public link. Session token and CSRF required.
async fn publinks_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<PublinkCreateForm>,
) -> Response {
    if let Err(resp) = require_web_token(&headers, state.web_token.expose_secret()) {
        return resp;
    }
    if let Err(resp) = require_csrf(&headers) {
        return resp;
    }

    // File vs folder is ambiguous from path alone in this MVP; we
    // pick folder for paths ending with `/`, otherwise file. A real
    // UI would query the daemon for the node kind first.
    let is_folder = form.path.ends_with('/');
    let create_req = if is_folder {
        Request::CreateFolderPublicLink {
            path: form.path.clone(),
        }
    } else {
        Request::CreateFilePublicLink {
            path: form.path.clone(),
        }
    };
    let ipc = call_ipc(&state.socket_path, create_req).await;

    // Try to extract a link_id from the create response so we can
    // apply expiry/password in follow-up calls. Best-effort; the
    // daemon's message shape is still advisory (bd-1du.10).
    let link_id = ipc
        .as_ref()
        .ok()
        .and_then(|r| serde_json::from_str::<serde_json::Value>(&r.message).ok())
        .and_then(|v| v.get("link_id").and_then(|x| x.as_u64()));

    if let Some(id) = link_id {
        if let Ok(expire) = form.expiry.parse::<u64>() {
            let _ = call_ipc(
                &state.socket_path,
                Request::ChangePublicLinkExpire {
                    link_id: id,
                    expire: Some(expire),
                },
            )
            .await;
        }
        if !form.password.is_empty() {
            let _ = call_ipc(
                &state.socket_path,
                Request::ChangePublicLinkPassword {
                    link_id: id,
                    password: Some(form.password.clone().into()),
                },
            )
            .await;
        }
    }

    ipc_redirect_response(ipc, "/publinks")
}

/// `DELETE /publinks/{code}` — revoke a public link. Session token and CSRF required.
async fn publinks_revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(code): AxumPath<String>,
) -> Response {
    if let Err(resp) = require_web_token(&headers, state.web_token.expose_secret()) {
        return resp;
    }
    if let Err(resp) = require_csrf(&headers) {
        return resp;
    }

    // Daemon deletes by numeric link id. If the operator passed a
    // numeric code directly, use it as the id; otherwise resolve the
    // code → id via `ShowPublicLink` first.
    let link_id = if let Ok(id) = code.parse::<u64>() {
        Some(id)
    } else {
        let show = call_ipc(&state.socket_path, Request::ShowPublicLink { code }).await;
        show.ok()
            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r.message).ok())
            .and_then(|v| v.get("link_id").and_then(|x| x.as_u64()))
    };

    let Some(id) = link_id else {
        return (
            StatusCode::NOT_FOUND,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            "unknown public link",
        )
            .into_response();
    };

    let ipc = call_ipc(
        &state.socket_path,
        Request::DeletePublicLink { link_id: id },
    )
    .await;
    ipc_plain_response(ipc)
}

// -------------------------------------------------------------------
// Activity / settings / metrics
// -------------------------------------------------------------------

/// `GET /activity` — last-100 audit events. Content type is negotiated:
/// `Accept: application/json` (or `application/x-ndjson`) returns the
/// raw JSON payload; otherwise HTML.
async fn activity(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = existing_or_new_csrf(&headers);
    let ipc = call_ipc(
        &state.socket_path,
        Request::Plain {
            method: Method::ListNotifications,
        },
    )
    .await;
    let (raw, online) = raw_and_online(&ipc);

    let want_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("application/json") || s.contains("application/x-ndjson")
        })
        .unwrap_or(false);

    if want_json {
        let body = json!({
            "online": online,
            "events": raw,
        });
        return json_response(&body);
    }

    let body = render_activity_page(online, &raw, &token);
    html_response_with_csrf(body, &token)
}

/// `GET /settings` — redacted config view.
async fn settings(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = existing_or_new_csrf(&headers);
    let socket = state.socket_path.display().to_string();
    let body = render_settings_page(&socket, &token);
    html_response_with_csrf(body, &token)
}

/// `GET /metrics` — placeholder. The `metrics` feature is not
/// compiled in for this crate; the route always returns `404` with a
/// descriptive body so scripted consumers get a clear signal.
async fn metrics() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )],
        "metrics feature is not enabled in this build",
    )
        .into_response()
}

// -------------------------------------------------------------------
// Status summary (shared with templates.rs)
// -------------------------------------------------------------------

/// Summary of the daemon's reported status, used by both the HTML and
/// JSON renderers on `GET /`.
#[derive(Debug, Clone, Default)]
pub(crate) struct StatusSummary {
    /// Daemon reachable over IPC and returned [`ResponseStatus::Ok`].
    pub online: bool,
    /// Short human-readable status line.
    pub message: String,
    /// Best-effort sync-root count parsed from the IPC message.
    pub sync_root_count: Option<u64>,
    /// Best-effort mount state string parsed from the IPC message.
    pub mount_state: Option<String>,
    /// Raw IPC message as returned by the daemon.
    pub raw: Option<String>,
}

async fn fetch_status(socket_path: &Path) -> StatusSummary {
    let result = call_ipc(
        socket_path,
        Request::Plain {
            method: Method::GetStatus,
        },
    )
    .await;

    match result {
        Ok(resp) => parse_status(&resp),
        Err(msg) => StatusSummary {
            online: false,
            message: msg,
            ..StatusSummary::default()
        },
    }
}

fn parse_status(resp: &IpcResponse) -> StatusSummary {
    let online = matches!(resp.status, ResponseStatus::Ok);
    let (sync_root_count, mount_state) = serde_json::from_str::<serde_json::Value>(&resp.message)
        .ok()
        .map(|v| {
            let roots = v
                .get("sync_root_count")
                .and_then(|x| x.as_u64())
                .or_else(|| {
                    v.get("sync_roots")
                        .and_then(|x| x.as_array())
                        .map(|a| a.len() as u64)
                });
            let mount = v
                .get("mount_state")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            (roots, mount)
        })
        .unwrap_or((None, None));

    StatusSummary {
        online,
        message: if online {
            "Online".into()
        } else {
            format!("Offline ({:?})", resp.status)
        },
        sync_root_count,
        mount_state,
        raw: Some(resp.message.clone()),
    }
}

// -------------------------------------------------------------------
// IPC helpers
// -------------------------------------------------------------------

async fn call_ipc(socket_path: &Path, req: Request) -> Result<IpcResponse, String> {
    if socket_path.as_os_str().is_empty() {
        return Err("daemon offline (no IPC socket configured)".to_string());
    }
    let socket_path = socket_path.to_path_buf();
    match tokio::task::spawn_blocking(move || IpcClient.send(&socket_path, &req)).await {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(e)) => Err(format!("daemon unreachable: {e}")),
        Err(e) => Err(format!("ipc task failed: {e}")),
    }
}

fn raw_and_online(res: &Result<IpcResponse, String>) -> (String, bool) {
    match res {
        Ok(r) => (r.message.clone(), matches!(r.status, ResponseStatus::Ok)),
        Err(e) => (e.clone(), false),
    }
}

fn ipc_redirect_response(ipc: Result<IpcResponse, String>, to: &str) -> Response {
    match ipc {
        Ok(r) if matches!(r.status, ResponseStatus::Ok) => Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::LOCATION, to)
            .header(header::CACHE_CONTROL, "no-store")
            .body(axum::body::Body::empty())
            .unwrap_or_else(|_| StatusCode::OK.into_response()),
        Ok(r) => (
            StatusCode::BAD_GATEWAY,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            format!("daemon: {:?} {}", r.status, r.message),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            e,
        )
            .into_response(),
    }
}

fn ipc_plain_response(ipc: Result<IpcResponse, String>) -> Response {
    match ipc {
        Ok(r) if matches!(r.status, ResponseStatus::Ok) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            r.message,
        )
            .into_response(),
        Ok(r) => (
            StatusCode::BAD_GATEWAY,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            format!("daemon: {:?} {}", r.status, r.message),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            e,
        )
            .into_response(),
    }
}

// -------------------------------------------------------------------
// CSRF
// -------------------------------------------------------------------

/// Read the caller's existing CSRF cookie (if valid) or mint a fresh
/// one. Tokens are 128 bits of OS randomness hex-encoded.
fn existing_or_new_csrf(headers: &HeaderMap) -> String {
    if let Some(t) = read_csrf_cookie(headers)
        && is_valid_token(&t)
    {
        return t;
    }
    mint_csrf_token()
}

fn mint_csrf_token() -> String {
    let mut buf = [0u8; 16];
    // getrandom panics on EIO from the kernel — which is fine for a
    // MVP loopback-only surface: we would not be serving a browser
    // on a system with no RNG anyway.
    getrandom::getrandom(&mut buf).expect("getrandom");
    let mut s = String::with_capacity(32);
    for b in buf {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

fn is_valid_token(t: &str) -> bool {
    t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit())
}

fn read_csrf_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(&format!("{CSRF_COOKIE}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

/// Double-submit check: the `X-CSRF-Token` header MUST match the
/// `pcw_csrf` cookie and MUST be well-formed.
///
/// The `Err` variant carries a pre-built 403 [`Response`]. It is
/// intentionally large (axum bodies are boxed internally); the lint
/// is silenced because boxing a one-shot error path adds noise
/// without saving meaningful memory on the happy path.
#[allow(clippy::result_large_err)]
fn require_csrf(headers: &HeaderMap) -> Result<(), Response> {
    let cookie = read_csrf_cookie(headers);
    let header_tok = headers
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let (Some(c), Some(h)) = (cookie, header_tok) else {
        return Err(csrf_reject("missing CSRF token"));
    };
    if !is_valid_token(&c) || !is_valid_token(&h) {
        return Err(csrf_reject("malformed CSRF token"));
    }
    // Constant-time compare (over the ASCII hex). Both sides are the
    // same fixed length by construction.
    let eq = c
        .as_bytes()
        .iter()
        .zip(h.as_bytes().iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
        && c.len() == h.len();
    if !eq {
        return Err(csrf_reject("CSRF token mismatch"));
    }
    Ok(())
}

fn csrf_reject(msg: &'static str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )],
        msg,
    )
        .into_response()
}

/// Session-token gate for mutating routes.
///
/// Compares the `X-PCloud-Web-Token` header value against the daemon's
/// startup token using a constant-time byte comparison to prevent
/// timing side-channels. Returns `Err(401 Unauthorized)` when the header
/// is absent, malformed, or does not match. Read-only routes (`GET /`,
/// `GET /health`, etc.) do not call this.
#[allow(clippy::result_large_err)]
fn require_web_token(headers: &HeaderMap, expected: &str) -> Result<(), Response> {
    let provided = headers
        .get(WEB_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Constant-time compare via `subtle::ConstantTimeEq` to prevent
    // timing side-channels on web-token validation.
    let matches: bool = provided.as_bytes().ct_eq(expected.as_bytes()).into();
    if !matches {
        return Err((
            StatusCode::UNAUTHORIZED,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            "missing or invalid X-PCloud-Web-Token",
        )
            .into_response());
    }
    Ok(())
}

// -------------------------------------------------------------------
// Response builders
// -------------------------------------------------------------------

fn html_response_with_csrf(body: String, csrf: &str) -> Response {
    let mut resp = Response::new(axum::body::Body::from(body));
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    let cookie = format!("{CSRF_COOKIE}={csrf}; HttpOnly; SameSite=Strict; Path=/");
    if let Ok(val) = HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, val);
    }
    resp
}

fn json_response(body: &serde_json::Value) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    let mut resp = Response::new(axum::body::Body::from(bytes));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp
}

// -------------------------------------------------------------------
// Page renderers (format! + xml_escape). Kept here rather than in
// templates.rs because they are new surfaces introduced alongside
// the new handlers and share CSRF + escaping conventions.
// -------------------------------------------------------------------

fn page_shell(title: &str, body: &str, csrf: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>{title}</title>\
<style>body{{font-family:system-ui,sans-serif;max-width:860px;margin:2em auto;padding:0 1em}}\
h1{{font-size:1.4em}} table{{border-collapse:collapse;width:100%}}\
th,td{{text-align:left;border-bottom:1px solid #ddd;padding:0.3em 0.5em}}\
form{{margin:1em 0;padding:0.6em;border:1px solid #ccc;background:#fafafa}}\
label{{display:block;margin:0.3em 0}}\
nav a{{margin-right:1em}} pre{{background:#f4f4f4;padding:0.6em;overflow-x:auto}}\
</style></head><body>\
<nav><a href=\"/\">status</a><a href=\"/sync\">sync</a>\
<a href=\"/publinks\">publinks</a><a href=\"/activity\">activity</a>\
<a href=\"/settings\">settings</a></nav>\
<meta name=\"csrf-token\" content=\"{csrf_attr}\">\
{body}\
</body></html>",
        title = xml_escape(title),
        csrf_attr = xml_escape(csrf),
        body = body,
    )
}

fn render_sync_page(online: bool, roots_raw: &str, pending_raw: &str, csrf: &str) -> String {
    let online_badge = if online { "Online" } else { "Offline" };
    let body = format!(
        "<h1>Sync roots</h1>\
<p>Daemon: <strong>{online}</strong></p>\
<h2>Roots</h2><pre>{roots}</pre>\
<h2>Pending</h2><pre>{pending}</pre>\
<h2>Add sync root</h2>\
<form method=\"post\" action=\"/sync\">\
<label>Local path <input name=\"local_path\" required></label>\
<label>Remote path <input name=\"remote_path\" required></label>\
<label>Type \
<select name=\"sync_type\">\
<option value=\"full\">full</option>\
<option value=\"download\">download</option>\
<option value=\"upload\">upload</option>\
</select></label>\
<button type=\"submit\">add</button>\
<p><em>Submission requires the X-CSRF-Token header \
(double-submit cookie pattern; no JS bundled).</em></p>\
</form>",
        online = xml_escape(online_badge),
        roots = xml_escape(roots_raw),
        pending = xml_escape(pending_raw),
    );
    let _ = csrf;
    page_shell("pcloud-rs — sync", &body, csrf)
}

fn render_publinks_page(online: bool, raw: &str, csrf: &str) -> String {
    let online_badge = if online { "Online" } else { "Offline" };
    let body = format!(
        "<h1>Public links</h1>\
<p>Daemon: <strong>{online}</strong></p>\
<h2>Active links</h2><pre>{raw}</pre>\
<h2>Create public link</h2>\
<form method=\"post\" action=\"/publinks\">\
<label>Path <input name=\"path\" required placeholder=\"/folder/file.txt or /folder/\"></label>\
<label>Expiry (unix seconds, optional) <input name=\"expiry\"></label>\
<label>Password (optional) <input name=\"password\" type=\"password\"></label>\
<button type=\"submit\">create</button>\
</form>",
        online = xml_escape(online_badge),
        raw = xml_escape(raw),
    );
    page_shell("pcloud-rs — publinks", &body, csrf)
}

fn render_activity_page(online: bool, raw: &str, csrf: &str) -> String {
    let body = format!(
        "<h1>Activity</h1>\
<p>Daemon: <strong>{online}</strong>. Last 100 audit events (best effort).</p>\
<pre>{raw}</pre>\
<p><small>Send <code>Accept: application/json</code> for NDJSON output.</small></p>",
        online = xml_escape(if online { "Online" } else { "Offline" }),
        raw = xml_escape(raw),
    );
    page_shell("pcloud-rs — activity", &body, csrf)
}

/// Redact secret-bearing keys from a settings view. The pcloud-web
/// process holds no secrets itself — this is defence in depth against
/// future settings additions leaking through the config snapshot.
fn redact_settings(socket_path: &str) -> Vec<(&'static str, String)> {
    vec![
        ("socket_path", socket_path.to_string()),
        ("bind_addr", format!("{}", crate::DEFAULT_BIND_ADDR)),
        ("auth_token", "<redacted>".into()),
        ("password", "<redacted>".into()),
        ("crypto_passphrase", "<redacted>".into()),
        ("csrf_cookie_name", CSRF_COOKIE.into()),
        ("csp", CSP.into()),
    ]
}

fn render_settings_page(socket_path: &str, csrf: &str) -> String {
    let rows: String = redact_settings(socket_path)
        .into_iter()
        .map(|(k, v)| {
            format!(
                "<tr><th>{}</th><td>{}</td></tr>",
                xml_escape(k),
                xml_escape(&v)
            )
        })
        .collect();
    let body = format!(
        "<h1>Settings</h1>\
<p>Read-only view. Secret-bearing fields are redacted.</p>\
<table>{rows}</table>",
    );
    page_shell("pcloud-rs — settings", &body, csrf)
}

// Silence-unused imports for types we accept in forms but don't yet
// round-trip end-to-end (SyncType/PublicLinkUploadPolicy will be used
// when sync_type/policy editing is wired — tracked with bd-1du.10).
#[allow(dead_code)]
fn _enum_type_parity(_t: SyncType, _p: PublicLinkUploadPolicy) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_csrf_is_valid() {
        let t = mint_csrf_token();
        assert!(is_valid_token(&t));
    }

    #[test]
    fn malformed_csrf_rejected() {
        assert!(!is_valid_token(""));
        assert!(!is_valid_token("zzzz"));
        assert!(!is_valid_token(&"a".repeat(31)));
    }

    #[test]
    fn redact_settings_hides_secrets() {
        let rows = redact_settings("/tmp/x.sock");
        let get = |k: &str| {
            rows.iter()
                .find(|(kk, _)| *kk == k)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("auth_token"), "<redacted>");
        assert_eq!(get("password"), "<redacted>");
        assert_eq!(get("crypto_passphrase"), "<redacted>");
        assert_eq!(get("socket_path"), "/tmp/x.sock");
    }

    #[test]
    fn web_token_gate_rejects_missing_token() {
        let headers = HeaderMap::new();
        assert!(require_web_token(&headers, "deadbeef").is_err());
    }

    #[test]
    fn web_token_gate_rejects_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert(WEB_TOKEN_HEADER, HeaderValue::from_static("wrongtoken"));
        assert!(require_web_token(&headers, "deadbeef").is_err());
    }

    #[test]
    fn web_token_gate_admits_correct_token() {
        let token = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let mut headers = HeaderMap::new();
        headers.insert(WEB_TOKEN_HEADER, HeaderValue::from_str(token).unwrap());
        assert!(require_web_token(&headers, token).is_ok());
    }
}

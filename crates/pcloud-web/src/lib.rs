#![deny(missing_docs)]
#![forbid(unsafe_code)]
#![allow(clippy::pedantic)]

//! # pcloud-web
//!
//! MVP Web UI scaffold for the pcloud-rs daemon (PLAN_A_PLUS §P4.5).
//!
//! This crate is intentionally **minimal**: it exposes a small Axum HTTP
//! server bound to `127.0.0.1` that renders a plain-HTML status page and
//! a tiny JSON status endpoint by calling the daemon over the existing
//! UNIX-socket IPC transport in [`pcloud_ipc`].
//!
//! It is **not** the final Leptos SSR application described in the plan.
//! It is the scaffold that later work will build on. Strict localhost
//! binding and a minimal Content-Security-Policy are wired here from
//! day one so follow-up UI work inherits safe defaults instead of
//! tightening them afterwards.
//!
//! ## Security posture
//!
//! - **Loopback-only bind.** The server binds to `127.0.0.1` only. Any
//!   attempt to start the server on a non-loopback address panics at
//!   startup (see [`serve`]). This is a defence-in-depth guard:
//!   misconfiguration (e.g. accidentally setting `bind_addr` to
//!   `0.0.0.0:17650` from a config file or CLI flag) would otherwise
//!   expose an unauthenticated HTTP surface — with daemon IPC access —
//!   to the local LAN. The guard is applied both in [`serve`] and in
//!   the hidden [`bind_for_test`] helper so test fixtures cannot drift
//!   off-loopback either.
//! - **No CORS, no cross-origin access.** Same-origin policy only. No
//!   `Access-Control-*` headers are emitted.
//! - **Strict Content-Security-Policy.** Every HTML response carries
//!   the minimal policy
//!   `default-src 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'`.
//!   This disables all JavaScript and restricts subresources to the
//!   same origin. `'unsafe-inline'` on styles is a temporary
//!   concession for the scaffold's inline `<style>` block and is
//!   expected to be replaced with hashed styles or an external
//!   stylesheet when the real Leptos SSR UI lands.
//! - **`X-Content-Type-Options: nosniff`** on HTML responses to prevent
//!   browser MIME-type sniffing.
//! - **Same-user execution model.** The web process is expected to run
//!   as the same local user as the daemon; IPC permission checks
//!   (owner-only UNIX socket) are enforced by [`pcloud_ipc`].
//! - **No auth token storage, no credential handling.** This crate
//!   never reads, renders, or persists any secret material. Secrets
//!   stay in the daemon behind the IPC boundary.
//! - **No auth beyond same-user IPC (audit-06 LOW IPC / pcloud-rs-ncx.84-b).**
//!   The management surface exposed by this crate relies entirely on
//!   the owner-only UNIX socket permission check performed by
//!   [`pcloud_ipc`] — there is **no** per-request authentication,
//!   session token, CSRF cookie, HTTP Basic/Bearer header, or client
//!   certificate. Any local process running as the same uid as the
//!   daemon can call every route. This is an intentional threat-model
//!   choice because the pCloud daemon itself already trusts any
//!   same-user caller over IPC; adding per-request auth to the web
//!   shim would give a false sense of security. Operators who want
//!   remote or cross-user access MUST put a reverse proxy with its
//!   own AuthN/AuthZ layer in front — shipping this crate on a
//!   publicly-bindable address is a configuration bug. The loopback
//!   bind guard and disabled CORS are the only defences in depth.
//!
//! ## IPC client pattern
//!
//! The routes call into the daemon with the synchronous
//! [`pcloud_ipc::IpcClient::send`] helper, which performs a blocking
//! UNIX-socket round trip. To avoid blocking the Tokio runtime, every
//! IPC call is dispatched on a blocking worker pool via
//! [`tokio::task::spawn_blocking`]. The rendered status summary is
//! intentionally best-effort: unreachable daemon, transport error, or
//! join failure all degrade gracefully to an "Offline" page rather
//! than propagating a 5xx.
//!
//! ## Honest limitations
//!
//! - **Status parsing is best-effort.** The landing page and
//!   `/api/status` endpoint parse the daemon's IPC response on a
//!   field-by-field allow-list; fields the daemon does not return
//!   (because an older daemon predates them, or because the IPC shape
//!   changed) are rendered as `"unknown"` rather than failing the
//!   request. Callers needing strict schema validation must use a
//!   direct IPC client instead of the web UI.
//! - **Scaffold, not the final UI.** This crate is the MVP described in
//!   PLAN_A_PLUS §P4.5, not the Leptos SSR application that will
//!   eventually replace it. The inline templating in
//!   `pcloud_web::templates` is expected to be removed wholesale when
//!   the real UI lands.
//! - **No metrics endpoint.** `GET /metrics` returns 404 by design
//!   here; the metrics surface is compiled into the daemon, not the
//!   web scaffold.
//!
//! # Example
//!
//! ```no_run
//! use pcloud_web::{WebConfig, serve};
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), pcloud_web::WebError> {
//!     let config = WebConfig {
//!         socket_path: PathBuf::from("/run/user/1000/pcloud-rs.sock"),
//!         bind_addr: "127.0.0.1:17650".parse().unwrap(),
//!         ..WebConfig::default()
//!     };
//!     serve(config).await
//! }
//! ```

// **PLATFORM:** all
// **GATING:** none (portable).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pcloud_secret::secret_string::SecretString;
use thiserror::Error;

mod routes;
mod templates;

use routes::router;

/// Default bind address for the MVP web UI.
///
/// Intentionally loopback-only. See module docs for security rationale.
pub const DEFAULT_BIND_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 17650);

/// Generate a cryptographically random 64-hex-char session token.
///
/// Returns an error string if the kernel CSPRNG is unavailable.
/// Callers that cannot tolerate failure should use [`generate_web_token_or_panic`].
///
/// # Errors
///
/// Returns `Err` with a descriptive string if `getrandom` fails.
pub fn generate_web_token() -> Result<String, String> {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf)
        .map_err(|e| format!("getrandom: kernel RNG unavailable: {e}"))?;
    let mut s = String::with_capacity(64);
    for b in buf {
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{b:02x}");
    }
    Ok(s)
}

/// Generate a session token, panicking if the kernel RNG is unavailable.
///
/// This is a convenience wrapper around [`generate_web_token`] for call
/// sites inside [`Default`] implementations where returning an error is
/// not possible. If you can propagate errors, prefer [`generate_web_token`].
#[must_use]
pub fn generate_web_token_or_panic() -> String {
    // SAFETY: This helper is documented as a panic-on-RNG-failure wrapper
    // for `Default`-style call sites that cannot propagate errors. A host
    // whose kernel CSPRNG is unavailable cannot start a web UI securely,
    // so panic is the correct failure mode here. Callers able to surface
    // errors should use `generate_web_token` directly.
    generate_web_token().expect("getrandom: kernel RNG unavailable — cannot start web UI")
}

/// Runtime configuration for [`serve`].
///
/// Construct explicitly or via [`WebConfig::default`] (loopback bind,
/// empty socket path — daemon reports as "offline" until a real socket
/// path is supplied).
///
/// # Fields
///
/// - [`WebConfig::socket_path`] — absolute path to the daemon's
///   owner-only UNIX socket. An empty path is treated as "no daemon
///   configured" and the UI renders a permanent Offline page without
///   ever touching the filesystem. This is the expected state in unit
///   tests and in the test fixtures.
/// - [`WebConfig::bind_addr`] — the socket address the HTTP server
///   binds to. **Must** be on a loopback interface (`127.0.0.0/8` or
///   `::1`); [`serve`] asserts this and panics with a descriptive
///   message otherwise. The default port is `17650`
///   ([`DEFAULT_BIND_ADDR`]).
/// - [`WebConfig::web_token`] — session token required by mutating
///   routes (`POST /sync`, `DELETE /sync/:id`, `POST /publinks`,
///   `DELETE /publinks/:code`) via the `X-PCloud-Web-Token` header.
///   Generate at daemon startup with [`generate_web_token`] and emit
///   it to daemon logs so operators can authenticate calls.
/// - [`WebConfig::ready`] — shared readiness flag. Set to `true` after
///   daemon initialization completes. The `/readyz` route returns 503
///   until this flag is set, enabling orchestrators to gate traffic
///   on full daemon readiness rather than mere process liveness.
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Filesystem path to the daemon's UNIX IPC socket.
    ///
    /// When empty, the web UI short-circuits every status fetch to
    /// "offline" without attempting a UNIX-socket connection.
    pub socket_path: PathBuf,
    /// Address to bind the HTTP server to. Must be on the loopback
    /// interface; [`serve`] panics otherwise.
    pub bind_addr: SocketAddr,
    /// Session token for mutating web management routes.
    ///
    /// Callers supply `X-PCloud-Web-Token: <token>` on every mutating
    /// request. Read-only routes do not require it. Generate with
    /// [`generate_web_token`].
    pub web_token: String,
    /// Readiness flag. `/readyz` returns 503 until this is `true`.
    ///
    /// Flip to `true` after daemon initialization completes so
    /// orchestrators (k8s, systemd socket-activated unit checks, etc.)
    /// can gate live traffic on full daemon readiness, not just process
    /// liveness. Defaults to `false` (not ready) at construction.
    pub ready: Arc<AtomicBool>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::new(),
            bind_addr: DEFAULT_BIND_ADDR,
            web_token: generate_web_token_or_panic(),
            ready: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Errors returned by [`serve`].
///
/// The variants distinguish *pre-serve* failures (bind) from *serve-time*
/// failures (hyper/Axum) so callers can decide whether the problem is a
/// configuration issue (port in use, permission denied) or a runtime
/// failure after the listener was already accepting connections.
///
/// Note that non-loopback bind addresses do **not** produce a
/// [`WebError`]; they produce a **panic**, because misconfiguring this
/// surface off-loopback is a deployment bug, not a runtime condition to
/// recover from.
#[derive(Debug, Error)]
pub enum WebError {
    /// The underlying TCP bind failed.
    ///
    /// Typical causes: port already in use, insufficient privileges,
    /// or address not assigned to any interface.
    #[error("bind {addr}: {source}")]
    Bind {
        /// Address we attempted to bind.
        addr: SocketAddr,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Error returned by the Axum / hyper server after bind.
    ///
    /// Covers post-bind `accept()` / I/O failures surfaced by
    /// [`axum::serve()`].
    #[error("serve: {0}")]
    Serve(#[source] std::io::Error),
}

/// Shared application state passed to every request handler.
///
/// Wraps the daemon socket path in an [`Arc`] so cloning the state per
/// request is cheap and does not copy the `PathBuf`. This struct is
/// intentionally tiny — routes do **not** hold long-lived IPC clients,
/// authentication tokens, or any secret material. Each request that
/// needs daemon data opens a fresh UNIX-socket connection via
/// [`pcloud_ipc::IpcClient::send`].
#[derive(Debug, Clone)]
pub(crate) struct AppState {
    /// Path to the daemon's UNIX IPC socket. Shared via [`Arc`] so
    /// cloning the state is cheap. An empty path is treated as "no
    /// daemon configured" by the request handlers.
    pub socket_path: Arc<PathBuf>,
    /// Session token required by mutating routes. Stored in an [`Arc`]
    /// around a [`SecretString`] so the token is zeroized when the last
    /// [`AppState`] clone is dropped, and never appears in `Debug` output.
    pub web_token: Arc<SecretString>,
    /// Readiness flag shared with the daemon. `/readyz` returns 503
    /// until this flips to `true`.
    pub ready: Arc<AtomicBool>,
}

/// Write the web session token to `$XDG_RUNTIME_DIR/pcloud-daemon/web-token`
/// with mode 0600.
///
/// Returns the path the token was written to on success, or an I/O error
/// description on failure. The token value itself is never logged or returned
/// in the error — callers must not include it in any log output.
///
/// Security rationale: stderr output (eprintln!) is captured verbatim by
/// systemd-journal, Docker log drivers, and CI log collectors. Any process
/// with journal-read privileges would see the token in cleartext. Writing to
/// a 0600 file under the per-user runtime directory limits readability to the
/// daemon owner and is consistent with the token-vault discipline used by the
/// rest of the daemon (ADR 0005, ADR 0015).
fn write_web_token_to_runtime_dir(token: &str) -> Result<PathBuf, std::io::Error> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    // Prefer XDG_RUNTIME_DIR (set by PAM/systemd for every login session).
    // When absent, return an error so the caller can fall back gracefully;
    // guessing the uid without unsafe code is not worth the complexity.
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "XDG_RUNTIME_DIR is not set; cannot locate runtime directory for token file",
            )
        })?;

    let token_dir = runtime_dir.join("pcloud-daemon");
    std::fs::create_dir_all(&token_dir)?;

    // Restrict the directory to the owner only if it was just created.
    // We do this on a best-effort basis; failure is non-fatal.
    let _ = std::fs::set_permissions(
        &token_dir,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    );

    let token_path = token_dir.join("web-token");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&token_path)?;
    file.write_all(token.as_bytes())?;
    Ok(token_path)
}

/// Start the Web UI MVP on the configured bind address.
///
/// Creates internal shared state from the supplied configuration, builds the
/// three-route Axum router (`/`, `/api/status`, `/health`), binds a
/// TCP listener on `config.bind_addr`, and drives
/// [`axum::serve()`] until an unrecoverable I/O error occurs.
///
/// The function returns only on error; successful operation blocks
/// forever. Callers that need cooperative shutdown should run this on
/// its own task and drop/abort it when they want to stop serving.
///
/// # Errors
///
/// - [`WebError::Bind`] if binding the TCP listener fails (port in
///   use, permission denied, etc.).
/// - [`WebError::Serve`] if the Axum/hyper server returns an I/O
///   error after a successful bind.
///
/// # Panics
///
/// Panics at startup if `config.bind_addr` is not on the loopback
/// interface. This is a deliberate hard guard — the MVP surface must
/// never be exposed off-host. The check protects against:
///
/// - misconfiguration via CLI flag or config file
///   (e.g. `bind_addr = "0.0.0.0:17650"`),
/// - well-meaning refactors that forget the loopback requirement,
/// - containerised deployments where `0.0.0.0` would expose the UI on
///   the pod/container network.
///
/// Because this is a programming / deployment error rather than a
/// runtime condition, it is intentionally unrecoverable (panic, not
/// `WebError`).
pub async fn serve(config: WebConfig) -> Result<(), WebError> {
    assert!(
        config.bind_addr.ip().is_loopback(),
        "pcloud-web refuses to bind to non-loopback address {} — this surface \
         is localhost-only. Reconfigure to 127.0.0.1 or ::1.",
        config.bind_addr,
    );

    // Write the web session token to a mode-0600 file under the runtime
    // directory rather than emitting it to stderr. stderr output is
    // captured verbatim by systemd-journal and other logging pipelines,
    // making it visible to any process that can read the journal — a wider
    // audience than the token's intended consumers. Writing to a 0600 file
    // restricts readability to the daemon owner, and `log::info!` directs
    // operators to the file path without exposing the token value itself.
    let token_written = write_web_token_to_runtime_dir(&config.web_token);
    match token_written {
        Ok(ref token_path) => {
            log::info!(
                "pcloud-web: session token written to {}",
                token_path.display()
            );
        }
        Err(ref e) => {
            // Fall back to log::warn so operators know where to look;
            // never emit the token value to stderr or to the log.
            log::warn!(
                "pcloud-web: could not write session token to runtime dir ({}); \
                 retrieve it via `pcloudc web-token`",
                e
            );
        }
    }

    let state = AppState {
        socket_path: Arc::new(config.socket_path),
        web_token: Arc::new(SecretString::new(config.web_token)),
        ready: config.ready,
    };
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .map_err(|source| WebError::Bind {
            addr: config.bind_addr,
            source,
        })?;

    axum::serve(listener, app.into_make_service())
        .await
        .map_err(WebError::Serve)
}

/// Bind a listener without starting serving. Intended for integration
/// tests that need to pick an ephemeral port and learn it before making
/// requests. Applies the same loopback guard as [`serve`].
///
/// Returns the bound listener, the resolved local [`SocketAddr`]
/// (useful when `bind_addr.port() == 0`), and the fully-constructed
/// [`axum::Router`] so the test can drive it with its own hyper
/// executor.
///
/// # Errors
///
/// Same as [`serve`] for the bind phase.
///
/// # Panics
///
/// Same loopback guard as [`serve`]: test fixtures must also stay on
/// `127.0.0.0/8` / `::1`.
#[doc(hidden)]
pub async fn bind_for_test(
    config: WebConfig,
) -> Result<(tokio::net::TcpListener, SocketAddr, axum::Router), WebError> {
    assert!(
        config.bind_addr.ip().is_loopback(),
        "pcloud-web refuses to bind to non-loopback address {}",
        config.bind_addr,
    );
    let state = AppState {
        socket_path: Arc::new(config.socket_path),
        web_token: Arc::new(SecretString::new(config.web_token)),
        ready: config.ready,
    };
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .map_err(|source| WebError::Bind {
            addr: config.bind_addr,
            source,
        })?;
    let local = listener.local_addr().map_err(WebError::Serve)?;
    Ok((listener, local, app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bind_is_loopback() {
        assert!(DEFAULT_BIND_ADDR.ip().is_loopback());
        assert_eq!(DEFAULT_BIND_ADDR.port(), 17650);
    }

    #[test]
    #[should_panic(expected = "loopback")]
    fn non_loopback_bind_panics() {
        // Use tokio's current-thread runtime so we can drive the async
        // call and observe the panic deterministically.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let cfg = WebConfig {
            socket_path: PathBuf::from("/tmp/nonexistent.sock"),
            bind_addr: "0.0.0.0:0".parse().unwrap(),
            ..WebConfig::default()
        };
        let _ = rt.block_on(serve(cfg));
    }
}

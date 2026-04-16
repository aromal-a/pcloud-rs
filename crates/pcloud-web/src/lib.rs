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
//!     };
//!     serve(config).await
//! }
//! ```

// **PLATFORM:** all
// **GATING:** none (portable).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

mod routes;
mod templates;

use routes::router;

/// Default bind address for the MVP web UI.
///
/// Intentionally loopback-only. See module docs for security rationale.
pub const DEFAULT_BIND_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 17650);

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
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::new(),
            bind_addr: DEFAULT_BIND_ADDR,
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

    let state = AppState {
        socket_path: Arc::new(config.socket_path),
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
        };
        let _ = rt.block_on(serve(cfg));
    }
}

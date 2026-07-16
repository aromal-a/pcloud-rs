#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
//! Experimental local HTTP/WebDAV subset.
//!
//! # Scope
//!
//! This crate contains a bounded HTTP/1.1 parser, a subset dispatcher,
//! a loopback TCP listener, and an owner-authenticated daemon IPC adapter.
//! It is not currently bootstrapped or shipped as a pCloud gateway.
//!
//! It also makes no RFC 4918 compliance-class claim. In particular,
//! COPY, MOVE, LOCK, UNLOCK, conditional requests, ETags, byte ranges,
//! streaming request/response bodies, and production Unix-socket binding
//! are absent. The `OPTIONS` response deliberately omits the `DAV` header
//! until an external compliance suite validates the corresponding class.
//!
//! The implemented experimental subset is:
//!
//! | Verb        | Status              | Acceptance step (T1.6) |
//! |-------------|---------------------|------------------------|
//! | `OPTIONS`   | dispatcher          | T1.6.b.2               |
//! | `PROPFIND`  | parser + dispatcher | T1.6.a + T1.6.b.2      |
//! | `GET`/`HEAD`| dispatcher          | T1.6.b.2               |
//! | `PUT`       | dispatcher          | T1.6.b.2               |
//! | `MKCOL`     | dispatcher          | T1.6.b.2               |
//! | `DELETE`    | dispatcher          | T1.6.b.2               |
//!
//! Tests exercise both the pure dispatcher and the concrete daemon IPC
//! request mapping. The shipped daemon does not start the listener.
//!
//! The crate intentionally has **zero new heavy deps**: the WebDAV
//! body shapes are a tiny fraction of RFC 4918 and we hand-roll the
//! XML parser + builder over `&str` so the binary stays small and
//! the surface is auditable.
//!
//! # Why not `dav-server` / `webdav-handler`
//!
//! The popular Rust WebDAV crates pull async runtimes (tokio /
//! hyper) and full filesystem trait stacks. [`RemoteFsIpcBackend`] uses the
//! canonical daemon `RemoteFs` IPC surface and hands large bodies through
//! owner-private temporary files rather than embedding them in IPC frames.
//!
//! # Listener policy
//!
//! The configuration models two bindings:
//!
//! - **Unix domain socket** under the runtime directory. This is currently
//!   rejected as unsupported by [`TcpServer::bind`].
//! - **Local-only TCP** on `127.0.0.1:<port>` for clients (Photos.app
//!   on macOS) that cannot speak Unix sockets.
//!
//! [`ServerConfig::validate`] rejects non-loopback TCP addresses. Port zero
//! remains intended for tests; this experimental crate is not bootstrapped
//! by the production daemon.

pub mod handler;
pub mod http;
pub mod ipc_backend;
mod propfind;
pub mod server;

pub use ipc_backend::RemoteFsIpcBackend;

pub use handler::{BackendEntry, BackendError, IpcBackend, PutOutcome, dispatch};
pub use http::{HttpParseError, HttpRequest, HttpResponse, parse_request};
pub use propfind::{
    PropfindError, PropfindRequest, PropfindResource, PropfindResponseEntry, parse_propfind,
    parse_propfind_or_allprop, render_multistatus,
};
pub use server::{ServerError, TcpServer};

use std::net::IpAddr;

/// WebDAV listener binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerBinding {
    /// Unix domain socket. Owner-only `0600` mode is enforced when
    /// the listener actually opens.
    UnixSocket {
        /// Absolute path to the socket file.
        path: std::path::PathBuf,
    },
    /// Local-only TCP. The host portion MUST be a loopback address
    /// (`127.0.0.0/8` or `::1`); validation refuses any other bind.
    LocalTcp {
        /// Loopback IP. The validator rejects non-loopback values so
        /// the gateway cannot accidentally publish to a LAN.
        host: IpAddr,
        /// TCP port. Use `0` for an OS-assigned ephemeral port (test
        /// builds only).
        port: u16,
    },
}

/// Top-level gateway configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Where the listener binds.
    pub binding: ListenerBinding,
    /// Maximum body size (bytes) accepted on `PUT`. Defaults to
    /// 256 MiB — large enough for typical photo-uploads, small
    /// enough that a runaway client cannot fill the daemon's heap.
    pub max_put_body_bytes: u64,
    /// Whether `PUT` and `MKCOL` are allowed. When `false` the
    /// gateway is read-only (still serves `OPTIONS`/`PROPFIND`/
    /// `GET`), which matches the principle of least authority for
    /// quick-look style clients.
    pub allow_writes: bool,
}

/// Default `PUT` body cap: 256 MiB.
pub const DEFAULT_MAX_PUT_BODY_BYTES: u64 = 256 * 1024 * 1024;

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            binding: ListenerBinding::LocalTcp {
                host: IpAddr::from([127, 0, 0, 1]),
                port: 0,
            },
            max_put_body_bytes: DEFAULT_MAX_PUT_BODY_BYTES,
            allow_writes: false,
        }
    }
}

/// Errors returned by [`ServerConfig::validate`].
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// `LocalTcp` binding requested a non-loopback host.
    #[error("WebDAV gateway TCP binding must be loopback (got {got})")]
    NonLoopbackTcpBind {
        /// Offending host.
        got: IpAddr,
    },
    /// `UnixSocket` path was empty / not absolute.
    #[error("WebDAV gateway socket path must be absolute (got {got:?})")]
    SocketPathNotAbsolute {
        /// Offending path.
        got: std::path::PathBuf,
    },
    /// Body cap is zero.
    #[error("WebDAV gateway max_put_body_bytes must be > 0")]
    ZeroBodyCap,
}

impl ServerConfig {
    /// Validate the binding + body cap.
    ///
    /// # Errors
    ///
    /// See [`ConfigError`] variants. Validation runs once at server
    /// bootstrap so a misconfigured profile fails fast instead of
    /// silently accepting a wide-open TCP listener.
    pub fn validate(&self) -> Result<(), ConfigError> {
        match &self.binding {
            ListenerBinding::LocalTcp { host, .. } => {
                if !host.is_loopback() {
                    return Err(ConfigError::NonLoopbackTcpBind { got: *host });
                }
            }
            ListenerBinding::UnixSocket { path } => {
                if !path.is_absolute() || path.as_os_str().is_empty() {
                    return Err(ConfigError::SocketPathNotAbsolute { got: path.clone() });
                }
            }
        }
        if self.max_put_body_bytes == 0 {
            return Err(ConfigError::ZeroBodyCap);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_loopback_read_only() {
        let cfg = ServerConfig::default();
        cfg.validate().unwrap();
        assert!(!cfg.allow_writes);
        match cfg.binding {
            ListenerBinding::LocalTcp { host, .. } => {
                assert!(host.is_loopback());
            }
            _ => panic!("default must be LocalTcp"),
        }
    }

    #[test]
    fn validate_rejects_non_loopback_tcp() {
        let cfg = ServerConfig {
            binding: ListenerBinding::LocalTcp {
                host: IpAddr::from([192, 168, 1, 1]),
                port: 8080,
            },
            ..Default::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::NonLoopbackTcpBind { .. })
        ));
    }

    #[test]
    fn validate_rejects_relative_socket_path() {
        let cfg = ServerConfig {
            binding: ListenerBinding::UnixSocket {
                path: std::path::PathBuf::from("relative/path.sock"),
            },
            ..Default::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::SocketPathNotAbsolute { .. })
        ));
    }

    #[test]
    fn validate_rejects_zero_body_cap() {
        let cfg = ServerConfig {
            max_put_body_bytes: 0,
            ..Default::default()
        };
        assert!(matches!(cfg.validate(), Err(ConfigError::ZeroBodyCap)));
    }

    #[test]
    fn ipv6_loopback_is_accepted() {
        use std::net::Ipv6Addr;
        let cfg = ServerConfig {
            binding: ListenerBinding::LocalTcp {
                host: IpAddr::V6(Ipv6Addr::LOCALHOST),
                port: 0,
            },
            ..Default::default()
        };
        cfg.validate().unwrap();
    }
}

//! # Local IPC transport
//!
//! **PLATFORM: Unix (Linux, FreeBSD, OpenBSD, NetBSD, macOS).**
//! **GATING: the file currently compiles only on Unix because it uses
//! `std::os::unix::net::{UnixListener, UnixStream}`. Peer authentication
//! is dispatched via the [`crate::platform`] module: Linux uses
//! `SO_PEERCRED` (see `platform::linux`), BSD/macOS use `getpeereid`
//! (see `platform::unix`). Windows named-pipe support is stubbed in
//! `platform::windows`.**

use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use thiserror::Error;

use crate::{
    IpcClient, IpcServer,
    auth::PeerIdentity,
    methods::{Request, RequestEnvelope, Response},
    protocol::ProtocolError,
    server::{IpcError, MAX_REQUEST_BYTES},
};

const IPC_REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors raised by the Unix-socket transport. All variants are safe to
/// surface to the peer except when the framing is already broken
/// (in which case the transport layer closes the connection instead).
#[derive(Debug, Error)]
pub enum IpcTransportError {
    /// Underlying OS-level I/O failure (socket bind, accept, read,
    /// write, `fs::set_permissions`, etc). Wraps `std::io::Error`
    /// verbatim; inspect `.kind()` to classify.
    ///
    /// # Recovery
    /// Transient kinds (`TimedOut`, `WouldBlock`, `UnexpectedEof`,
    /// `ConnectionReset`, `BrokenPipe`) are treated as client-side
    /// issues and the listener continues. Permanent kinds (`PermissionDenied`,
    /// `AddrInUse` on bind, `NotFound` on the runtime dir) are fatal for
    /// the listener and require operator intervention.
    #[error("filesystem or socket IO failure: {0}")]
    Io(#[from] std::io::Error),
    /// Framing / codec error (truncated header, version mismatch,
    /// oversize declared payload, JSON error). Forwarded from
    /// [`ProtocolError`].
    ///
    /// # Recovery
    /// Fatal for this request. The listener writes an `InvalidRequest`
    /// response and stays up, except for `TruncatedHeader` and
    /// `PayloadTooLarge` which require closing the connection.
    #[error("IPC protocol failure: {0}")]
    Protocol(#[from] ProtocolError),
    /// Server-level precondition violation — currently only
    /// [`IpcError::RequestTooLarge`]. The connection MUST be closed
    /// without writing a response (a reply would itself be a DoS vector
    /// against a misbehaving peer).
    ///
    /// # Recovery
    /// Fatal for this connection; the listener stays up. Not retryable
    /// without reducing the request body below [`MAX_REQUEST_BYTES`].
    #[error("IPC server-level failure: {0}")]
    Server(#[from] IpcError),
    /// Peer credentials (uid, pid, SID) could not be recovered from the
    /// platform backend: `SO_PEERCRED` returned non-zero on Linux,
    /// `getpeereid(3)` failed on BSD/macOS, or the Windows SID
    /// comparison against the pipe owner failed. The transport treats
    /// this as an unauthorized peer and responds
    /// [`crate::methods::ResponseStatus::Unauthorized`].
    ///
    /// # Recovery
    /// Fatal for this request. Not retryable without fixing the peer
    /// process's identity (run as the daemon-owning user).
    #[error("failed to read peer credentials")]
    PeerCredentialsUnavailable,
}

/// Owned, bound IPC server listening on a Unix domain socket.
///
/// Created by [`IpcServer::bind`]. The socket file lives at
/// `socket_path()` with mode `0600` under a `0700` parent directory;
/// [`Drop`] unlinks the socket on shutdown.
///
/// Thread-safety: `BoundIpcServer` is `Send + Sync` via `UnixListener`.
/// Concurrent accept from multiple threads is permitted, but the
/// current helper surface is deliberately single-request per call
/// (see [`Self::serve_once`]).
#[derive(Debug)]
pub struct BoundIpcServer {
    listener: UnixListener,
    socket_path: PathBuf,
    owner_uid: u32,
}

impl BoundIpcServer {
    /// Absolute path of the Unix socket file. Useful for CLI callers
    /// and audit logging.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Set the accept timeout on the underlying listener socket. When
    /// set, [`serve_once`](Self::serve_once) will return
    /// `Err(IpcTransportError::Io(ErrorKind::WouldBlock))` if no
    /// connection arrives within the given duration. Pass `None` to
    /// restore blocking mode.
    ///
    /// Used by the session-refresh integration so the serve loop can
    /// periodically wake and run the refresh tick even when idle.
    pub fn set_accept_timeout(&self, timeout: Option<Duration>) -> Result<(), IpcTransportError> {
        self.listener
            .set_nonblocking(false)
            .map_err(IpcTransportError::Io)?;
        // `UnixListener` does not expose `set_timeout` directly; we
        // use the raw fd. On Unix, `SO_RCVTIMEO` on a listening socket
        // controls the `accept(2)` timeout.
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.listener.as_raw_fd();
            let tv = match timeout {
                Some(d) => libc::timeval {
                    tv_sec: d.as_secs() as libc::time_t,
                    tv_usec: d.subsec_micros() as libc::suseconds_t,
                },
                None => libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
            };
            let ret = unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &tv as *const libc::timeval as *const libc::c_void,
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                )
            };
            if ret != 0 {
                return Err(IpcTransportError::Io(std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    /// Accept a single connection, verify the peer's uid, decode one
    /// framed request, hand it to `handler`, and write the response
    /// back.
    ///
    /// Error isolation discipline:
    ///
    /// * Oversized frame declaration → close without replying (the
    ///   stream is not framed-recoverable).
    /// * Malformed request → reply `ResponseStatus::InvalidRequest`.
    /// * Unauthorized peer → reply `ResponseStatus::Unauthorized`.
    /// * Slow client → honor the 5-second read timeout and return
    ///   without a reply.
    pub fn serve_once<F>(&self, handler: F) -> Result<(), IpcTransportError>
    where
        F: FnOnce(Request) -> Response,
    {
        let (stream, _) = self.listener.accept()?;
        self.serve_stream_once(stream, handler, IPC_REQUEST_READ_TIMEOUT)
    }

    fn serve_stream_once<F>(
        &self,
        mut stream: UnixStream,
        handler: F,
        read_timeout: Duration,
    ) -> Result<(), IpcTransportError>
    where
        F: FnOnce(Request) -> Response,
    {
        stream.set_read_timeout(Some(read_timeout))?;
        let server = IpcServer::new(self.owner_uid);
        let peer = match peer_identity(&stream) {
            Ok(peer) => peer,
            Err(_) => {
                let _ = read_framed_request(&mut stream);
                let _ = write_response(
                    &mut stream,
                    &server,
                    crate::methods::ResponseStatus::Unauthorized,
                    "peer credentials unavailable",
                );
                return Ok(());
            }
        };
        if !server.authorize_peer(&peer) {
            let _ = read_framed_request(&mut stream);
            let _ = write_response(
                &mut stream,
                &server,
                crate::methods::ResponseStatus::Unauthorized,
                format!("unauthorized peer uid={}, pid={}", peer.uid, peer.pid),
            );
            return Ok(());
        }

        let request_bytes = match read_framed_request(&mut stream) {
            Ok(bytes) => bytes,
            Err(err) => {
                return handle_client_error(&mut stream, &server, err);
            }
        };
        let envelope = match server.decode_envelope(&request_bytes) {
            Ok(envelope) => envelope,
            Err(err) => {
                return handle_client_error(&mut stream, &server, IpcTransportError::Protocol(err));
            }
        };
        // The traceparent (if any) is observable on the envelope before
        // dispatch; downstream observability code is responsible for
        // re-attaching it to spans. The handler API stays Request-only
        // to keep existing daemon dispatch sites untouched.
        let response = handler(envelope.request);
        let _ = write_response(&mut stream, &server, response.status, response.message);
        Ok(())
    }
}

impl Drop for BoundIpcServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

impl IpcServer {
    /// Bind an IPC listener at `socket_path`.
    ///
    /// The daemon's runtime directory (`socket_path.parent()`) is
    /// created with mode `0700` when it does not yet exist; any stale
    /// socket at `socket_path` is removed first; the new socket is
    /// `chmod`-ed to `0600` (owner read/write only). The returned
    /// [`BoundIpcServer`] unlinks the socket on [`Drop`].
    pub fn bind(&self, socket_path: &Path) -> Result<BoundIpcServer, IpcTransportError> {
        if let Some(parent) = socket_path.parent() {
            let parent_missing = !parent.exists();
            fs::create_dir_all(parent)?;
            if parent_missing {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }

        if socket_path.exists() {
            fs::remove_file(socket_path)?;
        }

        let listener = UnixListener::bind(socket_path)?;
        fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;

        Ok(BoundIpcServer {
            listener,
            socket_path: socket_path.to_path_buf(),
            owner_uid: self.owner_uid(),
        })
    }
}

impl IpcClient {
    /// Connect to the daemon's Unix socket at `socket_path`, send the
    /// framed `request`, shut down the write half (so the daemon sees
    /// EOF without waiting), and read the framed response to completion.
    pub fn send(
        &self,
        socket_path: &Path,
        request: &Request,
    ) -> Result<Response, IpcTransportError> {
        self.send_envelope(socket_path, &RequestEnvelope::new(request.clone()))
    }

    /// Envelope-aware send: serializes a [`RequestEnvelope`] (request +
    /// optional `traceparent`) and round-trips it through the same
    /// owner-verified Unix socket transport as [`Self::send`]. Existing
    /// `&Request` callers stay on `send`; observability-aware callers
    /// build the envelope explicitly via `RequestEnvelope::new(req)
    /// .with_traceparent(tp)` and dispatch through this method.
    pub fn send_envelope(
        &self,
        socket_path: &Path,
        envelope: &RequestEnvelope,
    ) -> Result<Response, IpcTransportError> {
        let request_bytes = self.prepare_envelope(envelope)?;
        let mut stream = UnixStream::connect(socket_path)?;
        stream.write_all(&request_bytes)?;
        stream.shutdown(std::net::Shutdown::Write)?;

        let mut response_bytes = Vec::new();
        stream.read_to_end(&mut response_bytes)?;
        Ok(self.parse_response(&response_bytes)?)
    }
}

fn read_framed_request(stream: &mut UnixStream) -> Result<Vec<u8>, IpcTransportError> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header)?;

    let payload_len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    // Enforce the per-request cap BEFORE any allocation proportional to the
    // attacker-controlled length prefix. On violation the caller closes the
    // connection — the stream is not in a framed-recoverable state.
    if payload_len > MAX_REQUEST_BYTES {
        return Err(IpcTransportError::Server(IpcError::RequestTooLarge {
            declared: payload_len,
            max: MAX_REQUEST_BYTES,
        }));
    }

    let mut bytes = Vec::with_capacity(8 + payload_len);
    bytes.extend_from_slice(&header);
    let mut payload = vec![0u8; payload_len];
    stream.read_exact(&mut payload)?;
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

fn handle_client_error(
    stream: &mut UnixStream,
    server: &IpcServer,
    err: IpcTransportError,
) -> Result<(), IpcTransportError> {
    match err {
        // Oversized declared frame length: the stream is not in a
        // framed-recoverable state. Close the connection immediately
        // without writing a response, which could itself be a vector
        // for resource exhaustion against a misbehaving peer.
        IpcTransportError::Server(IpcError::RequestTooLarge { .. }) => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            Ok(())
        }
        IpcTransportError::Protocol(protocol_err) => {
            let _ = write_response(
                stream,
                server,
                crate::methods::ResponseStatus::InvalidRequest,
                protocol_err.to_string(),
            );
            Ok(())
        }
        IpcTransportError::Io(io_err)
            if matches!(
                io_err.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            ) =>
        {
            Ok(())
        }
        other => Err(other),
    }
}

fn write_response(
    stream: &mut UnixStream,
    server: &IpcServer,
    status: crate::methods::ResponseStatus,
    message: impl Into<String>,
) -> Result<(), IpcTransportError> {
    let response_bytes = server.encode_status(status, message.into())?;
    stream.write_all(&response_bytes)?;
    stream.flush()?;
    Ok(())
}

/// Recover the peer identity for a connected `UnixStream` via the
/// compile-time-selected platform backend.
///
/// - On Linux this calls `getsockopt(SO_PEERCRED)`
///   (see [`crate::platform::linux`]).
/// - On FreeBSD/OpenBSD/NetBSD/macOS this calls `getpeereid(3)`
///   (see [`crate::platform::unix`]); pid is not available on these
///   platforms and is reported as `0`.
fn peer_identity(stream: &UnixStream) -> Result<PeerIdentity, IpcTransportError> {
    #[cfg(target_os = "linux")]
    let (uid, pid) = crate::platform::linux::peer_ucred(stream)?;

    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "macos"
    ))]
    let (uid, pid) = crate::platform::unix::peer_ucred(stream)?;

    Ok(PeerIdentity { uid, pid })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::{thread, time::Duration};

    use crate::{Method, Request, Response, ResponseStatus, protocol};

    use crate::auth::current_effective_uid;

    use super::{IpcClient, IpcServer};

    #[test]
    fn uds_transport_roundtrip_works() {
        let socket_path = std::env::temp_dir().join(format!(
            "pcloud-ipc-test-{}-{}.sock",
            std::process::id(),
            "roundtrip"
        ));
        let server = IpcServer::new(current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        let handle = thread::spawn(move || {
            bound
                .serve_once(|request| match request {
                    Request::Plain {
                        method: Method::GetHealth,
                    } => Response {
                        status: ResponseStatus::Ok,
                        message: "healthy".to_owned(),
                    },
                    _ => Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "unexpected request".to_owned(),
                    },
                })
                .expect("server should handle request");
        });

        thread::sleep(Duration::from_millis(20));

        let client = IpcClient;
        let response = client
            .send(
                &socket_path,
                &Request::Plain {
                    method: Method::GetHealth,
                },
            )
            .expect("client send should succeed");

        assert_eq!(response.status, ResponseStatus::Ok);
        assert_eq!(response.message, "healthy");
        handle.join().expect("server thread should exit");
    }

    #[test]
    fn unauthorized_peer_is_rejected_before_dispatch() {
        let socket_path = std::env::temp_dir().join(format!(
            "pcloud-ipc-test-{}-{}.sock",
            std::process::id(),
            "unauthorized"
        ));
        let server = IpcServer::new(current_effective_uid().saturating_add(1));
        let bound = server.bind(&socket_path).expect("socket should bind");

        let handle = thread::spawn(move || {
            bound
                .serve_once(|_| Response {
                    status: ResponseStatus::Ok,
                    message: "should not run".to_owned(),
                })
                .expect("server should reject unauthorized peer cleanly");
        });

        thread::sleep(Duration::from_millis(20));

        let client = IpcClient;
        let response = client
            .send(
                &socket_path,
                &Request::Plain {
                    method: Method::GetHealth,
                },
            )
            .expect("client send should receive unauthorized response");

        assert_eq!(response.status, ResponseStatus::Unauthorized);
        assert!(response.message.contains("unauthorized peer"));
        handle.join().expect("server thread should exit");
    }

    #[test]
    fn server_handles_request_without_waiting_for_client_eof() {
        let socket_path = std::env::temp_dir().join(format!(
            "pcloud-ipc-test-{}-{}.sock",
            std::process::id(),
            "no-eof"
        ));
        let server = IpcServer::new(current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        let handle = thread::spawn(move || {
            bound
                .serve_once(|request| match request {
                    Request::Plain {
                        method: Method::GetHealth,
                    } => Response {
                        status: ResponseStatus::Ok,
                        message: "healthy".to_owned(),
                    },
                    _ => Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "unexpected request".to_owned(),
                    },
                })
                .expect("server should handle request");
        });

        thread::sleep(Duration::from_millis(20));

        let mut stream = UnixStream::connect(&socket_path).expect("client should connect");
        let request_bytes = protocol::encode_request_bare(&Request::Plain {
            method: Method::GetHealth,
        })
        .expect("request should encode");
        stream
            .write_all(&request_bytes)
            .expect("request should write");

        let mut response_bytes = Vec::new();
        stream
            .read_to_end(&mut response_bytes)
            .expect("response should read");
        let response = protocol::decode_response(&response_bytes).expect("response should decode");

        assert_eq!(response.payload.status, ResponseStatus::Ok);
        assert_eq!(response.payload.message, "healthy");
        handle.join().expect("server thread should exit");
    }

    #[test]
    fn slow_client_timeout_does_not_prevent_followup_request() {
        let socket_path = std::env::temp_dir().join(format!(
            "pcloud-ipc-test-{}-{}.sock",
            std::process::id(),
            "slow-client"
        ));
        let server = IpcServer::new(current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        let handle = thread::spawn(move || {
            let (stream, _) = bound.listener.accept().expect("slow client should connect");
            bound
                .serve_stream_once(
                    stream,
                    |_| Response {
                        status: ResponseStatus::Ok,
                        message: "unexpected".to_owned(),
                    },
                    Duration::from_millis(50),
                )
                .expect("slow client should be isolated");
            bound
                .serve_once(|request| match request {
                    Request::Plain {
                        method: Method::GetHealth,
                    } => Response {
                        status: ResponseStatus::Ok,
                        message: "healthy".to_owned(),
                    },
                    _ => Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "unexpected request".to_owned(),
                    },
                })
                .expect("followup request should still be served");
        });

        thread::sleep(Duration::from_millis(20));
        let _slow_client = UnixStream::connect(&socket_path).expect("slow client should connect");
        thread::sleep(Duration::from_millis(80));

        let client = IpcClient;
        let response = client
            .send(
                &socket_path,
                &Request::Plain {
                    method: Method::GetHealth,
                },
            )
            .expect("followup client send should succeed");

        assert_eq!(response.status, ResponseStatus::Ok);
        assert_eq!(response.message, "healthy");
        handle.join().expect("server thread should exit");
    }

    #[test]
    fn malformed_request_is_rejected_without_killing_followup_request() {
        let socket_path = std::env::temp_dir().join(format!(
            "pcloud-ipc-test-{}-{}.sock",
            std::process::id(),
            "malformed-request"
        ));
        let server = IpcServer::new(current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        let handle = thread::spawn(move || {
            bound
                .serve_once(|_| Response {
                    status: ResponseStatus::Ok,
                    message: "unexpected".to_owned(),
                })
                .expect("malformed request should be isolated");
            bound
                .serve_once(|request| match request {
                    Request::Plain {
                        method: Method::GetHealth,
                    } => Response {
                        status: ResponseStatus::Ok,
                        message: "healthy".to_owned(),
                    },
                    _ => Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "unexpected request".to_owned(),
                    },
                })
                .expect("followup request should still be served");
        });

        thread::sleep(Duration::from_millis(20));

        let mut malformed = UnixStream::connect(&socket_path).expect("client should connect");
        // payload_len=0, version=0 (mismatch), message_type=0: this is a
        // well-framed but protocol-invalid request. It should produce an
        // InvalidRequest response (NOT a connection drop, which is reserved
        // for oversized-frame denial-of-service attempts).
        malformed
            .write_all(&[0, 0, 0, 0, 0, 0, 0, 0])
            .expect("malformed framed header should write");
        malformed
            .shutdown(std::net::Shutdown::Write)
            .expect("client should shutdown write half");

        let mut response_bytes = Vec::new();
        malformed
            .read_to_end(&mut response_bytes)
            .expect("malformed response should read");
        let response = protocol::decode_response(&response_bytes).expect("response should decode");
        assert_eq!(response.payload.status, ResponseStatus::InvalidRequest);

        let client = IpcClient;
        let followup = client
            .send(
                &socket_path,
                &Request::Plain {
                    method: Method::GetHealth,
                },
            )
            .expect("followup client send should succeed");

        assert_eq!(followup.status, ResponseStatus::Ok);
        assert_eq!(followup.message, "healthy");
        handle.join().expect("server thread should exit");
    }
}

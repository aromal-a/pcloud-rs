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
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Mutex;

use thiserror::Error;

use crate::{
    IpcClient, IpcServer,
    auth::PeerIdentity,
    methods::{Request, RequestEnvelope, Response},
    protocol::ProtocolError,
    server::{IpcError, MAX_REQUEST_BYTES},
};

/// Default hard cap on the number of simultaneously active IPC
/// connections across the entire process.
///
/// When the cap is reached, newly accepted connections are closed
/// immediately (without reading or responding) so the server-side thread
/// pool cannot be exhausted by a burst of idle or slow clients.
/// 128 is well above typical real-world concurrency (one or two CLI
/// callers at a time) while bounding worst-case thread/fd consumption.
///
/// # Runtime override (ncx.59)
///
/// This is a default only. The active cap is held in
/// [`MAX_IPC_CONNECTIONS_RUNTIME`] and can be raised or lowered at
/// daemon startup via [`set_ipc_connection_caps`], which is wired from
/// `pcloud-config` [`ResourceLimits`](pcloud_config::limits::ResourceLimits).
/// The runtime API makes enterprise deployments with large numbers of
/// local automation clients (e.g. per-service-account CLI callers)
/// possible without a rebuild.
pub const MAX_IPC_CONNECTIONS: usize = 128;

/// Default per-peer (per-UID) cap on simultaneously active IPC
/// connections.
///
/// Even when the process-global cap has not been reached, a single local
/// user cannot hold more than this many concurrent connections. This
/// prevents a malicious or buggy local user from monopolising the global
/// slot pool before other users can connect.
///
/// Default: 32 (generous for legitimate use; tight enough to limit abuse).
///
/// See [`MAX_IPC_CONNECTIONS`] for the runtime-override story (ncx.59).
pub const MAX_IPC_CONNECTIONS_PER_PEER: usize = 32;

/// Active process-wide IPC connection cap.
///
/// Initialised to [`MAX_IPC_CONNECTIONS`]; mutable at daemon bootstrap
/// via [`set_ipc_connection_caps`] so operators can raise the cap without
/// recompiling. Read on every accepted connection in
/// [`ConnectionGuard::acquire`].
static MAX_IPC_CONNECTIONS_RUNTIME: AtomicUsize = AtomicUsize::new(MAX_IPC_CONNECTIONS);

/// Active per-peer IPC connection cap.
///
/// Initialised to [`MAX_IPC_CONNECTIONS_PER_PEER`]; mutable at daemon
/// bootstrap via [`set_ipc_connection_caps`]. Read on every accepted
/// connection in [`ConnectionGuard::acquire`].
static MAX_IPC_CONNECTIONS_PER_PEER_RUNTIME: AtomicUsize =
    AtomicUsize::new(MAX_IPC_CONNECTIONS_PER_PEER);

/// Install runtime caps for the process-global and per-peer IPC
/// connection limits (ncx.59, P3-E6).
///
/// Called exactly once at daemon bootstrap from `pcloud-daemon::bootstrap`
/// so the values sourced from the validated
/// [`ResourceLimits`](pcloud_config::limits::ResourceLimits) configuration
/// override the compile-time defaults. Calling again simply overwrites
/// the previous values; the caller is responsible for not racing caps
/// during live serve (the daemon only wires this at startup).
///
/// `global` must be at least `per_peer`; otherwise the per-peer cap is
/// clamped down to `global` silently. Both are capped at `usize::MAX`;
/// passing `0` effectively disables accepts.
pub fn set_ipc_connection_caps(global: usize, per_peer: usize) {
    let per_peer = per_peer.min(global);
    MAX_IPC_CONNECTIONS_RUNTIME.store(global, AtomicOrdering::Release);
    MAX_IPC_CONNECTIONS_PER_PEER_RUNTIME.store(per_peer, AtomicOrdering::Release);
}

/// Inspect the active process-global IPC connection cap.
#[must_use]
pub fn ipc_connection_cap() -> usize {
    MAX_IPC_CONNECTIONS_RUNTIME.load(AtomicOrdering::Acquire)
}

/// Inspect the active per-peer IPC connection cap.
#[must_use]
pub fn ipc_connection_cap_per_peer() -> usize {
    MAX_IPC_CONNECTIONS_PER_PEER_RUNTIME.load(AtomicOrdering::Acquire)
}

/// Process-wide active connection counter. Incremented on accept,
/// decremented when the connection handler returns (via RAII guard).
static ACTIVE_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);

/// Per-peer (per-UID) active connection counters.
///
/// Each entry is inserted on the first accepted connection from a uid
/// and removed (entry deleted) when the count drops back to zero.
/// The `Mutex` is only contended at accept/disconnect time — never
/// during request processing — so lock duration is O(1).
static PEER_CONNECTIONS: Mutex<Option<HashMap<u32, usize>>> = Mutex::new(None);

/// RAII guard that decrements [`ACTIVE_CONNECTIONS`] and the per-peer
/// counter for `peer_uid` on drop.
struct ConnectionGuard {
    peer_uid: u32,
}

impl ConnectionGuard {
    /// Attempt to acquire a global + per-peer connection slot for `peer_uid`.
    ///
    /// Returns `None` when either:
    /// * the process-global cap ([`MAX_IPC_CONNECTIONS`]) is reached, or
    /// * the per-peer cap ([`MAX_IPC_CONNECTIONS_PER_PEER`]) for `peer_uid`
    ///   is reached.
    ///
    /// Both caps are checked and incremented atomically under the
    /// `PEER_CONNECTIONS` mutex to avoid TOCTOU races.
    fn acquire(peer_uid: u32) -> Option<Self> {
        // ncx.59: read runtime-configurable caps on every accept. The
        // atomic load is cheap (Acquire ordering matches the Release
        // store in `set_ipc_connection_caps`) and the value is set once
        // at bootstrap, so there is no steady-state contention.
        let global_cap = MAX_IPC_CONNECTIONS_RUNTIME.load(AtomicOrdering::Acquire);
        let per_peer_cap = MAX_IPC_CONNECTIONS_PER_PEER_RUNTIME.load(AtomicOrdering::Acquire);

        // Lock the per-peer map first, then CAS the global counter.
        // Holding the lock during the global CAS is intentional: it
        // serialises the (check global, check peer, increment both)
        // triple so no two threads can both succeed past either cap.
        let mut map_guard = PEER_CONNECTIONS.lock().unwrap_or_else(|p| p.into_inner());
        let map = map_guard.get_or_insert_with(HashMap::new);

        // Check and reserve the per-peer slot first (cheaper check).
        let peer_count = map.entry(peer_uid).or_insert(0);
        if *peer_count >= per_peer_cap {
            return None;
        }

        // Check and reserve the global slot.
        // Use a CAS loop so we never overshoot even under concurrent pressure.
        loop {
            let global = ACTIVE_CONNECTIONS.load(AtomicOrdering::Relaxed);
            if global >= global_cap {
                // Clean up the tentative per-peer reservation.
                if *peer_count == 0 {
                    map.remove(&peer_uid);
                }
                return None;
            }
            if ACTIVE_CONNECTIONS
                .compare_exchange(
                    global,
                    global + 1,
                    AtomicOrdering::AcqRel,
                    AtomicOrdering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }

        // Both slots secured — commit the per-peer increment.
        *peer_count += 1;
        Some(Self { peer_uid })
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        // Decrement global counter.
        ACTIVE_CONNECTIONS.fetch_sub(1, AtomicOrdering::Release);

        // Decrement per-peer counter and remove the entry when it hits zero.
        let mut map_guard = PEER_CONNECTIONS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(map) = map_guard.as_mut()
            && let Some(count) = map.get_mut(&self.peer_uid) {
                *count -= 1;
                if *count == 0 {
                    map.remove(&self.peer_uid);
                }
            }
    }
}

const IPC_REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Write timeout applied to the response write so a slow or stalled
/// client cannot hold the server-side stream open indefinitely after the
/// request has been dispatched.
const IPC_RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

// macOS `launch_activate_socket` — available in libSystem since macOS 10.9.
// Not exposed by the `libc` crate, so we declare it manually.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn launch_activate_socket(
        name: *const std::os::raw::c_char,
        fds: *mut *mut std::os::raw::c_int,
        cnt: *mut usize,
    ) -> std::os::raw::c_int;
}

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
///
/// # Why `accept_and_spawn` is not used in production
///
/// [`Self::accept_and_spawn`] spawns a new OS thread per connection and
/// requires the handler closure to be `Clone + Send + 'static`. The
/// production daemon handler closes over a `&mut RuntimeShell`, and
/// `RuntimeShell` is intentionally `!Send` (it holds raw pointers and
/// non-`Send` SQLite connection state). Migrating dispatch to
/// `accept_and_spawn` would therefore require either an `Arc<Mutex<RuntimeShell>>`
/// wrapper (introducing lock contention on every IPC call) or a full
/// refactor to a channel-based dispatch model. The current single-threaded
/// [`Self::serve_once`] loop is the deliberate production path;
/// `accept_and_spawn` is retained for embedders whose handler types are
/// `Send`.
///
/// This decision is formalised in
/// `docs/adr/0019-ipc-serve-loop-single-threaded.md` (ncx.56 — audit-06
/// §7-sonnet M2). That ADR lists the read/write timeouts and connection
/// caps that bound worst-case latency today, and the conditions under
/// which the serve loop may migrate to a channel-based dispatcher.
#[cfg(unix)]
#[derive(Debug)]
pub struct BoundIpcServer {
    listener: UnixListener,
    socket_path: PathBuf,
    owner_uid: u32,
}

#[cfg(unix)]
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
    /// # Crash-recovery invariant
    ///
    /// Each IPC connection carries exactly one request and receives exactly
    /// one atomic response before the connection is closed. There is no
    /// mid-stream state. If the daemon crashes between accepting the
    /// connection and writing the response the client gets a broken-pipe
    /// error and can safely retry; no partial state is left on the daemon
    /// side because each handler invocation is logically atomic — the
    /// daemon either completes the operation and replies, or it crashes
    /// before the operation is committed (audit-06 LOW IPC L-1 / ncx.84-a).
    ///
    /// # Error isolation discipline
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
        self.serve_once_with_peer(|_peer, request| handler(request))
    }

    /// Peer-aware variant of [`Self::serve_once`]. The handler receives
    /// both the resolved [`PeerIdentity`] (after owner-uid authorization)
    /// and the decoded [`Request`]. Used by the daemon to thread peer
    /// uid through the privileged-audit log and the per-peer rate
    /// limiter. Cross-platform; peer identity fields may be
    /// platform-specific placeholders (see [`PeerIdentity`]).
    pub fn serve_once_with_peer<F>(&self, handler: F) -> Result<(), IpcTransportError>
    where
        F: FnOnce(PeerIdentity, Request) -> Response,
    {
        let (mut stream, _) = self.listener.accept()?;

        // Recover peer identity before enforcing connection caps so the
        // per-peer cap can be applied to the correct uid.  On failure we
        // respond Unauthorized and return; no slot is consumed.
        let peer = match peer_identity(&stream) {
            Ok(p) => p,
            Err(_) => {
                let server = IpcServer::new(self.owner_uid);
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

        // Enforce the per-process AND per-peer connection caps.  When
        // either cap is reached we close the incoming stream immediately
        // and return success so the outer serve loop continues accepting
        // new connections (and shedding excess ones) without propagating
        // an error.  The client will see a connection reset.
        let _guard = match ConnectionGuard::acquire(peer.uid) {
            Some(g) => g,
            None => {
                eprintln!(
                    "pcloud-ipc: connection cap reached (global={MAX_IPC_CONNECTIONS}, \
                     per-peer={MAX_IPC_CONNECTIONS_PER_PEER}); \
                     closing connection from uid={}", peer.uid
                );
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return Ok(());
            }
        };
        let peer_for_handler = peer;
        self.serve_stream_once_with_peer(
            stream,
            peer,
            move |req| handler(peer_for_handler, req),
            IPC_REQUEST_READ_TIMEOUT,
        )
    }

    /// Accept a single connection and dispatch it on a **dedicated OS
    /// thread**, returning immediately after the thread is spawned.
    ///
    /// This is the thread-per-connection entry point. It eliminates the
    /// single-threaded bottleneck present in [`Self::serve_once`]: slow
    /// backend calls (auth RTT, crypto unlock) no longer block subsequent
    /// clients from being accepted and dispatched.
    ///
    /// The connection cap ([`MAX_IPC_CONNECTIONS`]) is still enforced:
    /// when the cap is already reached, the incoming stream is closed
    /// immediately and this method returns `Ok(())` without spawning.
    ///
    /// # Handler requirements
    ///
    /// `handler` must be `Clone + Send + 'static`. Typically this is
    /// an `Arc`-wrapped dispatcher: `Arc<dyn Fn(Request) -> Response +
    /// Send + Sync>`. The clone is taken once per accepted connection so
    /// the thread owns its own handle.
    ///
    /// # Errors
    ///
    /// Only the `accept(2)` syscall itself can fail here; any error that
    /// occurs inside the spawned thread is logged to stderr (pcloud-ipc
    /// avoids a `log` dependency) and does not propagate back to the
    /// caller.
    pub fn accept_and_spawn<F>(&self, handler: F) -> Result<(), IpcTransportError>
    where
        F: Fn(Request) -> Response + Clone + Send + 'static,
    {
        let (mut stream, _addr) = self.listener.accept()?;

        // Recover peer identity before enforcing connection caps so the
        // per-peer cap can be applied to the correct uid.
        let peer = match peer_identity(&stream) {
            Ok(p) => p,
            Err(_) => {
                let server = IpcServer::new(self.owner_uid);
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

        // Enforce the per-process AND per-peer connection caps before
        // spawning so we never launch more threads than either cap allows.
        let guard = match ConnectionGuard::acquire(peer.uid) {
            Some(g) => g,
            None => {
                eprintln!(
                    "pcloud-ipc: connection cap reached (global={MAX_IPC_CONNECTIONS}, \
                     per-peer={MAX_IPC_CONNECTIONS_PER_PEER}); \
                     closing connection from uid={}", peer.uid
                );
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return Ok(());
            }
        };

        let owner_uid = self.owner_uid;
        let handler = handler.clone();

        std::thread::Builder::new()
            .name("pcloud-ipc-conn".to_owned())
            .spawn(move || {
                // The guard is moved into the thread so ACTIVE_CONNECTIONS
                // and the per-peer counter are decremented when this thread
                // exits, regardless of whether the request succeeds or fails.
                let _guard = guard;
                let server_ctx = IpcServer::new(owner_uid);
                let result = serve_stream_standalone_with_peer(
                    stream,
                    &server_ctx,
                    peer,
                    handler,
                    IPC_REQUEST_READ_TIMEOUT,
                );
                if let Err(e) = result {
                    eprintln!("pcloud-ipc: connection error: {e}");
                }
            })
            .map_err(IpcTransportError::Io)?;

        Ok(())
    }

    /// Legacy entry-point used by tests that accept a stream directly
    /// (e.g. `slow_client` test). Peer identity is re-resolved internally.
    #[cfg_attr(not(test), allow(dead_code))]
    fn serve_stream_once<F>(
        &self,
        stream: UnixStream,
        handler: F,
        read_timeout: Duration,
    ) -> Result<(), IpcTransportError>
    where
        F: FnOnce(Request) -> Response,
    {
        let server = IpcServer::new(self.owner_uid);
        // Resolve peer identity; on failure respond Unauthorized and return.
        let peer = match peer_identity(&stream) {
            Ok(p) => p,
            Err(_) => {
                let mut s = stream;
                let _ = read_framed_request(&mut s);
                let _ = write_response(
                    &mut s,
                    &server,
                    crate::methods::ResponseStatus::Unauthorized,
                    "peer credentials unavailable",
                );
                return Ok(());
            }
        };
        self.serve_stream_once_with_peer(stream, peer, handler, read_timeout)
    }

    /// Core stream handler that receives a pre-resolved [`PeerIdentity`].
    /// Called by both [`Self::serve_once`] (after cap enforcement) and the
    /// legacy `serve_stream_once` shim used by tests.
    fn serve_stream_once_with_peer<F>(
        &self,
        mut stream: UnixStream,
        peer: PeerIdentity,
        handler: F,
        read_timeout: Duration,
    ) -> Result<(), IpcTransportError>
    where
        F: FnOnce(Request) -> Response,
    {
        stream.set_read_timeout(Some(read_timeout))?;
        let server = IpcServer::new(self.owner_uid);

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
        // Apply a write timeout before sending the response so a stalled
        // or malicious client cannot block the serve thread indefinitely
        // after the request has already been dispatched.
        let _ = stream.set_write_timeout(Some(IPC_RESPONSE_WRITE_TIMEOUT));
        if let Err(err) = write_response(&mut stream, &server, response.status, response.message) {
            // BrokenPipe / ConnectionReset are expected when the client
            // disconnects after sending the request but before reading
            // the response (e.g. timeout on the client side). Log at
            // trace level to avoid spamming operators in normal operation.
            log::trace!("pcloud-ipc: write_response failed (client disconnected?): {err}");
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for BoundIpcServer {
    /// Unlink the Unix-domain socket on drop (RAII cleanup).
    ///
    /// # SIGKILL race
    ///
    /// SIGKILL cannot be caught or blocked — if the process receives SIGKILL
    /// while `BoundIpcServer` is live, the socket file is left behind. This
    /// is unavoidable in Rust (and in any language): the kernel does not
    /// invoke destructors on SIGKILL. To mitigate this:
    ///
    /// 1. The serve loop observes SIGTERM (caught) and exits cleanly, which
    ///    triggers this `Drop` before the process dies. Callers should ensure
    ///    SIGTERM is sent before SIGKILL (systemd's `KillMode=control-group`
    ///    with a `TimeoutStopSec` does this automatically).
    /// 2. `IpcServer::bind` removes any stale socket file that already exists
    ///    at the path before binding (so leftover sockets from a prior
    ///    SIGKILL do not prevent restart).
    ///
    /// These two policies together mean SIGKILL leaves at most one stale
    /// socket that is cleaned up on the next daemon start.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

impl IpcServer {
    /// Try to receive a pre-activated socket from launchd (macOS only).
    ///
    /// Returns `Ok(Some(BoundIpcServer))` if launchd provided a socket for
    /// `socket_name`. Returns `Ok(None)` if no launchd socket is available
    /// (normal startup without launchd socket activation). Returns `Err` only
    /// on unexpected failures (e.g. an invalid socket name containing NUL).
    ///
    /// `socket_name` must match the `Sockets` key in the launchd plist.
    /// By convention we use `"pcloud-ipc"`.
    ///
    /// `socket_path` is recorded in the returned `BoundIpcServer` so that
    /// the [`Drop`] impl can unlink it on shutdown (launchd does not do this
    /// automatically).
    #[cfg(target_os = "macos")]
    pub fn try_launchd_socket(
        &self,
        socket_name: &str,
        socket_path: &std::path::Path,
    ) -> Result<Option<BoundIpcServer>, IpcTransportError> {
        use std::ffi::CString;
        use std::os::fd::FromRawFd;
        use std::os::unix::net::UnixListener;

        let name = CString::new(socket_name).map_err(|_| {
            IpcTransportError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "launchd socket name contains NUL byte",
            ))
        })?;

        let mut fds: *mut std::os::raw::c_int = std::ptr::null_mut();
        let mut count: usize = 0;

        // SAFETY: `launch_activate_socket` writes into `fds`/`count` only on
        // success (return value 0). The returned fd array is heap-allocated by
        // launchd and must be freed with `free(3)`.
        let rc = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut count) };

        if rc != 0 {
            // pcloud-rs-0cx: distinguish "not running under launchd"
            // (normal) from an unexpected errno (operator-visible).
            // Previously all non-zero rcs silently returned `Ok(None)`
            // and the caller fell back to `bind()`, masking real
            // launchd integration failures (e.g. misconfigured plist,
            // EPERM). The "normal" errnos still return `Ok(None)` so
            // non-launchd-supervised runs continue to fall through to
            // regular bind; any other errno is surfaced as `Err` so
            // the daemon startup path can log + exit with a structured
            // code instead of silently degrading.
            //
            // ENOENT (2)  — no such socket in the launchd plist: normal.
            // ESRCH (3)   — not running under launchd: normal.
            match rc {
                libc::ENOENT | libc::ESRCH => return Ok(None),
                other => {
                    return Err(IpcTransportError::Io(std::io::Error::from_raw_os_error(
                        other,
                    )));
                }
            }
        }

        if count == 0 || fds.is_null() {
            // SAFETY: fds was set by launchd and may need to be freed even if
            // count is 0; free(NULL) is always safe.
            unsafe { libc::free(fds as *mut libc::c_void) };
            return Ok(None);
        }

        // Take the first fd; we only configure one socket per plist entry.
        // SAFETY: `fds` is non-null (checked above) and `count >= 1`, so
        // dereferencing the first element is within the launchd-allocated array.
        let fd = unsafe { *fds };
        // SAFETY: `fds` was allocated by launchd (`launch_activate_socket`
        // uses malloc internally); `free` is the matching deallocator.
        // After this call `fds` is no longer valid — we never use it again.
        unsafe { libc::free(fds as *mut libc::c_void) };

        // SAFETY: launchd hands us a valid, pre-bound, pre-listening socket fd.
        let listener = unsafe { UnixListener::from_raw_fd(fd) };

        // Ensure the socket is in blocking mode (launchd may deliver it
        // non-blocking depending on activation flags).
        listener
            .set_nonblocking(false)
            .map_err(IpcTransportError::Io)?;

        Ok(Some(BoundIpcServer {
            listener,
            owner_uid: self.owner_uid(),
            socket_path: socket_path.to_path_buf(),
        }))
    }

    /// Bind an IPC listener at `socket_path`.
    ///
    /// The daemon's runtime directory (`socket_path.parent()`) is
    /// created with mode `0700` when it does not yet exist; any stale
    /// socket at `socket_path` is removed first; the new socket is
    /// `chmod`-ed to `0600` (owner read/write only). The returned
    /// [`BoundIpcServer`] unlinks the socket on [`Drop`].
    ///
    /// Unix-only. The Windows path uses named pipes and lives in
    /// `crate::platform::windows::WindowsIpc` — wiring it through this
    /// `BoundIpcServer` surface is deferred (bd-xplat-windows).
    #[cfg(unix)]
    pub fn bind(&self, socket_path: &Path) -> Result<BoundIpcServer, IpcTransportError> {
        if let Some(parent) = socket_path.parent() {
            let parent_missing = !parent.exists();
            fs::create_dir_all(parent)?;
            // Re-apply 0700 on every bind, not just when the directory was
            // newly created. An existing directory could have been left with
            // relaxed permissions by a previous install, upgrade, or manual
            // operation. We only do this when we own the directory — if the
            // parent is a system directory (e.g. /tmp) we skip the chmod to
            // avoid a PermissionDenied error. When the directory was newly
            // created we always own it, so the `parent_missing` fast path
            // always applies.
            if parent_missing {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            } else {
                // Re-chmod existing dirs only if we own them.
                use std::os::unix::fs::MetadataExt;
                if let Ok(meta) = fs::metadata(parent)
                    && meta.uid() == self.owner_uid() {
                        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                    }
            }
        }

        if socket_path.exists() {
            fs::remove_file(socket_path)?;
        }

        // TOCTOU NOTE (audit-06 P3-A2): there is a brief window between
        // `UnixListener::bind` (which creates the socket with mode 0o777 &
        // ~umask) and the `set_permissions` call below that sets it to 0o600.
        // A local attacker racing in that window could connect before chmod.
        //
        // Primary mitigation: the parent directory is already 0o700 (set above),
        // so no local user other than the owner can even see the socket path
        // during the window. The chmod here provides defence-in-depth only.
        //
        // A fully race-free alternative would use `fchmod(fd, 0o600)` on the
        // bound socket fd before the first `accept`. Rust's std does not expose
        // fchmod on `UnixListener`. A future hardening pass could use `nix::sys::
        // stat::fchmod` to eliminate the window entirely (audit-06 LOW residual).
        let listener = UnixListener::bind(socket_path)?;
        fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;

        Ok(BoundIpcServer {
            listener,
            socket_path: socket_path.to_path_buf(),
            owner_uid: self.owner_uid(),
        })
    }
}

#[cfg(unix)]
impl IpcClient {
    /// Connect to the daemon's Unix socket at `socket_path`, send the
    /// framed `request`, shut down the write half (so the daemon sees
    /// EOF without waiting), and read the framed response to completion.
    ///
    /// Unix-only. Windows clients use named pipes through
    /// `crate::platform::windows::WindowsIpc` — bd-xplat-windows tracks
    /// unifying both paths behind a single `IpcClient` surface.
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

/// Module-level variant of `serve_stream_standalone` that receives a
/// pre-resolved [`PeerIdentity`] (already recovered by `accept_and_spawn`
/// before the connection slot was acquired).  Used by the thread closure
/// spawned in [`BoundIpcServer::accept_and_spawn`].
#[cfg(unix)]
fn serve_stream_standalone_with_peer<F>(
    mut stream: UnixStream,
    server: &IpcServer,
    peer: PeerIdentity,
    handler: F,
    read_timeout: Duration,
) -> Result<(), IpcTransportError>
where
    F: FnOnce(Request) -> Response,
{
    stream.set_read_timeout(Some(read_timeout))?;

    if !server.authorize_peer(&peer) {
        let _ = read_framed_request(&mut stream);
        let _ = write_response(
            &mut stream,
            server,
            crate::methods::ResponseStatus::Unauthorized,
            format!("unauthorized peer uid={}, pid={}", peer.uid, peer.pid),
        );
        return Ok(());
    }

    let request_bytes = match read_framed_request(&mut stream) {
        Ok(bytes) => bytes,
        Err(err) => {
            return handle_client_error(&mut stream, server, err);
        }
    };
    let envelope = match server.decode_envelope(&request_bytes) {
        Ok(envelope) => envelope,
        Err(err) => {
            return handle_client_error(&mut stream, server, IpcTransportError::Protocol(err));
        }
    };
    let response = handler(envelope.request);
    let _ = stream.set_write_timeout(Some(IPC_RESPONSE_WRITE_TIMEOUT));
    let _ = write_response(&mut stream, server, response.status, response.message);
    Ok(())
}

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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
///
/// Unix-only. The Windows named-pipe backend recovers the peer SID via
/// `crate::platform::windows::peer_uid` and does not go through this
/// function.
#[cfg(unix)]
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

#[cfg(all(test, unix))]
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

    // Global serialisation lock for connection-cap tests.  The per-peer
    // tests mutate `ACTIVE_CONNECTIONS` / `PEER_CONNECTIONS` (process-wide
    // statics shared across cargo's default parallel test runner) so we
    // hold this mutex for the duration of each test to keep their delta
    // assertions stable.
    static PER_PEER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Verify that the per-peer cap is enforced even when the global cap
    /// has not been reached.  We set `MAX_IPC_CONNECTIONS_PER_PEER` = 32,
    /// so a single uid exhausting 32 slots should be denied a 33rd while
    /// total connections < 128.
    ///
    /// This test exercises [`ConnectionGuard::acquire`] directly to avoid
    /// requiring real IPC round-trips for all 33 slots (which would be slow).
    #[test]
    fn per_peer_cap_enforced_even_when_global_cap_not_hit() {
        use super::{ConnectionGuard, MAX_IPC_CONNECTIONS_PER_PEER, ACTIVE_CONNECTIONS, PEER_CONNECTIONS};
        use std::sync::atomic::Ordering;

        let _lock = PER_PEER_TEST_LOCK.lock().unwrap();
        let test_uid: u32 = 0xBEEF_0001;

        // Reset any leftover state from previous tests.
        {
            let mut map = PEER_CONNECTIONS.lock().unwrap();
            if let Some(m) = map.as_mut() {
                m.remove(&test_uid);
            }
        }
        let baseline = ACTIVE_CONNECTIONS.load(Ordering::Relaxed);

        // Acquire MAX_IPC_CONNECTIONS_PER_PEER slots — all should succeed.
        let mut guards: Vec<ConnectionGuard> = (0..MAX_IPC_CONNECTIONS_PER_PEER)
            .map(|_| {
                ConnectionGuard::acquire(test_uid)
                    .expect("slot within per-peer cap should be available")
            })
            .collect();

        // The next acquire must be denied (per-peer cap reached).
        assert!(
            ConnectionGuard::acquire(test_uid).is_none(),
            "per-peer cap should deny the ({MAX_IPC_CONNECTIONS_PER_PEER}+1)th connection"
        );

        // Verify global counter increased by exactly the number we acquired.
        assert_eq!(
            ACTIVE_CONNECTIONS.load(Ordering::Relaxed),
            baseline + MAX_IPC_CONNECTIONS_PER_PEER
        );

        // Release all guards; counter returns to baseline, map entry cleared.
        guards.clear();

        assert_eq!(ACTIVE_CONNECTIONS.load(Ordering::Relaxed), baseline);
        {
            let map = PEER_CONNECTIONS.lock().unwrap();
            assert!(
                map.as_ref().is_none_or(|m| !m.contains_key(&test_uid)),
                "per-peer entry should be removed after all connections close"
            );
        }
    }

    /// Verify that the per-peer counter is correctly decremented (and the
    /// map entry removed) when a connection closes, allowing subsequent
    /// connections from the same peer.
    #[test]
    fn per_peer_cap_resets_on_disconnect() {
        use super::{ConnectionGuard, ACTIVE_CONNECTIONS, PEER_CONNECTIONS};
        use std::sync::atomic::Ordering;

        let _lock = PER_PEER_TEST_LOCK.lock().unwrap();
        let test_uid: u32 = 0xBEEF_0002;

        // Reset any leftover state.
        {
            let mut map = PEER_CONNECTIONS.lock().unwrap();
            if let Some(m) = map.as_mut() {
                m.remove(&test_uid);
            }
        }
        let baseline = ACTIVE_CONNECTIONS.load(Ordering::Relaxed);

        // Acquire and immediately release one slot, three times in a row.
        for round in 0..3 {
            let guard = ConnectionGuard::acquire(test_uid)
                .unwrap_or_else(|| panic!("round {round}: slot should be available after disconnect"));
            drop(guard);

            assert_eq!(
                ACTIVE_CONNECTIONS.load(Ordering::Relaxed),
                baseline,
                "round {round}: global counter should return to baseline after drop"
            );
            let map = PEER_CONNECTIONS.lock().unwrap();
            assert!(
                map.as_ref().is_none_or(|m| !m.contains_key(&test_uid)),
                "round {round}: per-peer entry should be absent after drop"
            );
        }
    }
}

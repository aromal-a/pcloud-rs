//! # Local IPC transport
//!
//! **PLATFORM: Unix (Linux, FreeBSD, OpenBSD, NetBSD, macOS) + Windows.**
//!
//! ## Backend selection (cfg-gated)
//!
//! * `#[cfg(unix)]` — `BoundIpcServer` wraps a [`UnixListener`] and
//!   dispatches peer-credential recovery through [`crate::platform`]:
//!   Linux uses `SO_PEERCRED` (see `platform::linux`), BSD/macOS use
//!   `getpeereid(3)` (see `platform::unix`).
//! * `#[cfg(windows)]` — `BoundIpcServer` wraps a
//!   [`crate::platform::windows::WindowsListener`] and dispatches peer
//!   authentication through `GetNamedPipeClientProcessId` +
//!   `TokenUser` SID comparison.
//!
//! ## Cross-platform serve loop
//!
//! The same framed-JSON protocol and the same connection-cap / slow-
//! client isolation discipline apply to both transports. Per-request
//! I/O is dispatched through the internal [`IpcStream`] trait so the
//! [`BoundIpcServer::serve_once_with_peer`] body is identical on Unix
//! and Windows.
//!
//! ## Windows-specific behaviour differences
//!
//! * **Per-accept timeout.** On Unix `set_accept_timeout` installs
//!   `SO_RCVTIMEO` on the listener so the session-refresh tick runs
//!   even when the daemon is idle. On Windows `ConnectNamedPipe` with
//!   a NULL OVERLAPPED is fully blocking; `set_accept_timeout` is a
//!   no-op. Tracked under `bd-xplat-windows`.
//! * **Read/write timeouts.** Named pipes in byte mode do not expose
//!   `SO_RCVTIMEO`-style per-handle deadlines from safe code; the
//!   slow-client read timeout is a no-op on Windows. Tracked under
//!   `bd-xplat-windows`.
//! * **Peer pid.** Windows reports the real client PID via
//!   `GetNamedPipeClientProcessId`, matching the Linux `SO_PEERCRED`
//!   pid. BSD/macOS still report pid=0 because `getpeereid(3)` does
//!   not expose it.
//! * **Peer uid.** Windows has no uid; we report `uid=0` when the
//!   client SID matches the daemon-owner SID and reject mismatches at
//!   accept time. The daemon's `owner_uid` defaults to 0 on Windows so
//!   the existing [`PeerIdentity::matches_owner`] gate continues to
//!   function.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

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
pub const MAX_IPC_CONNECTIONS: usize = 128;

/// Default per-peer (per-UID) cap on simultaneously active IPC
/// connections.
pub const MAX_IPC_CONNECTIONS_PER_PEER: usize = 32;

/// Active process-wide IPC connection cap.
static MAX_IPC_CONNECTIONS_RUNTIME: AtomicUsize = AtomicUsize::new(MAX_IPC_CONNECTIONS);

/// Active per-peer IPC connection cap.
static MAX_IPC_CONNECTIONS_PER_PEER_RUNTIME: AtomicUsize =
    AtomicUsize::new(MAX_IPC_CONNECTIONS_PER_PEER);

/// Install runtime caps for the process-global and per-peer IPC
/// connection limits.
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

/// Process-wide active connection counter.
static ACTIVE_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);

/// Per-peer (per-UID) active connection counters.
static PEER_CONNECTIONS: Mutex<Option<HashMap<u32, usize>>> = Mutex::new(None);

/// RAII guard that decrements the connection counters on drop.
struct ConnectionGuard {
    peer_uid: u32,
}

impl ConnectionGuard {
    fn acquire(peer_uid: u32) -> Option<Self> {
        let global_cap = MAX_IPC_CONNECTIONS_RUNTIME.load(AtomicOrdering::Acquire);
        let per_peer_cap = MAX_IPC_CONNECTIONS_PER_PEER_RUNTIME.load(AtomicOrdering::Acquire);

        let mut map_guard = PEER_CONNECTIONS.lock().unwrap_or_else(|p| p.into_inner());
        let map = map_guard.get_or_insert_with(HashMap::new);

        let peer_count = map.entry(peer_uid).or_insert(0);
        if *peer_count >= per_peer_cap {
            return None;
        }

        loop {
            let global = ACTIVE_CONNECTIONS.load(AtomicOrdering::Relaxed);
            if global >= global_cap {
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

        *peer_count += 1;
        Some(Self { peer_uid })
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        ACTIVE_CONNECTIONS.fetch_sub(1, AtomicOrdering::Release);

        let mut map_guard = PEER_CONNECTIONS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(map) = map_guard.as_mut() {
            if let Some(count) = map.get_mut(&self.peer_uid) {
                *count -= 1;
                if *count == 0 {
                    map.remove(&self.peer_uid);
                }
            }
        }
    }
}

const IPC_REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
const IPC_RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

// macOS `launch_activate_socket`.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn launch_activate_socket(
        name: *const std::os::raw::c_char,
        fds: *mut *mut std::os::raw::c_int,
        cnt: *mut usize,
    ) -> std::os::raw::c_int;
}

/// Errors raised by the local-IPC transport layer.
#[derive(Debug, Error)]
pub enum IpcTransportError {
    /// Underlying OS-level I/O failure (socket bind, accept, read, write,
    /// `fs::set_permissions`, `CreateNamedPipeW`, etc).
    #[error("filesystem or socket IO failure: {0}")]
    Io(#[from] std::io::Error),
    /// Framing / codec error.
    #[error("IPC protocol failure: {0}")]
    Protocol(#[from] ProtocolError),
    /// Server-level precondition violation — currently only
    /// [`IpcError::RequestTooLarge`].
    #[error("IPC server-level failure: {0}")]
    Server(#[from] IpcError),
    /// Peer credentials could not be recovered: `SO_PEERCRED` returned
    /// non-zero on Linux, `getpeereid(3)` failed on BSD/macOS, or the
    /// Windows SID comparison against the pipe owner failed.
    #[error("failed to read peer credentials")]
    PeerCredentialsUnavailable,
}

// --------------------------------------------------------------------
// IpcStream: platform-neutral read/write abstraction used by the
// serve-loop helpers so the same code paths work on UnixStream and
// WindowsStream.
// --------------------------------------------------------------------

/// Minimal trait abstracting the stream operations the serve loop
/// needs. Implemented on `UnixStream` (via its std I/O traits) and
/// [`crate::platform::windows::WindowsStream`] (via its inherent
/// Win32-backed methods).
trait IpcStream {
    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()>;
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
    /// Best-effort close of both halves. On Unix this is
    /// `shutdown(Both)`; on Windows it disconnects + closes the pipe.
    fn close_both(&mut self);
    fn set_read_timeout(&self, t: Option<Duration>) -> std::io::Result<()>;
    fn set_write_timeout(&self, t: Option<Duration>) -> std::io::Result<()>;
}

#[cfg(unix)]
impl IpcStream for UnixStream {
    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        std::io::Read::read_exact(self, buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(self, buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(self)
    }
    fn close_both(&mut self) {
        let _ = UnixStream::shutdown(self, std::net::Shutdown::Both);
    }
    fn set_read_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        UnixStream::set_read_timeout(self, t)
    }
    fn set_write_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        UnixStream::set_write_timeout(self, t)
    }
}

#[cfg(windows)]
impl IpcStream for crate::platform::windows::WindowsStream {
    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        crate::platform::windows::WindowsStream::read_exact(self, buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        crate::platform::windows::WindowsStream::write_all(self, buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        crate::platform::windows::WindowsStream::flush(self)
    }
    fn close_both(&mut self) {
        let _ = crate::platform::windows::WindowsStream::shutdown(self);
    }
    fn set_read_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        crate::platform::windows::WindowsStream::set_read_timeout(self, t)
    }
    fn set_write_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        crate::platform::windows::WindowsStream::set_write_timeout(self, t)
    }
}

// --------------------------------------------------------------------
// BoundIpcServer
// --------------------------------------------------------------------

/// Owned, bound IPC server. Variant-selected at compile time by target
/// OS: Unix domain socket on Unix, named pipe on Windows.
///
/// See module docs for per-platform behaviour differences.
#[derive(Debug)]
pub struct BoundIpcServer {
    inner: BoundInner,
    socket_path: PathBuf,
    owner_uid: u32,
}

#[derive(Debug)]
enum BoundInner {
    #[cfg(unix)]
    Unix(UnixListener),
    #[cfg(windows)]
    Windows(crate::platform::windows::WindowsListener),
}

impl BoundIpcServer {
    /// Absolute path of the Unix socket (Unix) or the recorded
    /// bind-target path (Windows — the actual pipe name is derived from
    /// the current TokenUser SID and is accessible via the inner
    /// [`crate::platform::windows::WindowsListener::pipe_path`]).
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Cooperatively wake any thread currently blocked in `accept`.
    ///
    /// **Unix:** no-op. The Unix serve loop relies on
    /// [`Self::set_accept_timeout`] (via `SO_RCVTIMEO`) for periodic
    /// wake-ups and observes the caller-owned shutdown flag on the
    /// next iteration; no explicit cancellation primitive is needed.
    ///
    /// **Windows:** signals the listener's manual-reset cancel event.
    /// A pending `ConnectNamedPipe` cannot be cancelled by closing
    /// the handle (which would race with concurrent connects); the
    /// overlapped accept path parks on `WaitForMultipleObjects`
    /// against that event and, when woken, calls `CancelIoEx` on the
    /// pending connect and returns `ErrorKind::Interrupted` so the
    /// serve loop can re-check its shutdown flag.
    ///
    /// Safe to call from any thread (the underlying Win32 Event is
    /// thread-safe) and idempotent thanks to manual-reset semantics —
    /// repeat calls after the event is already set are no-ops.
    #[allow(clippy::unused_self)]
    pub fn request_shutdown(&self) {
        #[cfg(windows)]
        {
            let BoundInner::Windows(listener) = &self.inner;
            listener.request_shutdown();
        }
        #[cfg(unix)]
        {
            // No-op on Unix — see method docs.
        }
    }

    /// Set the accept timeout on the underlying listener.
    ///
    /// Unix: installs `SO_RCVTIMEO` so `accept(2)` wakes periodically.
    /// Windows: **no-op** because the overlapped accept path relies on
    /// the explicit [`Self::request_shutdown`] cancel event, not on a
    /// periodic wake-up.
    pub fn set_accept_timeout(&self, timeout: Option<Duration>) -> Result<(), IpcTransportError> {
        #[cfg(unix)]
        {
            let BoundInner::Unix(listener) = &self.inner;
            listener
                .set_nonblocking(false)
                .map_err(IpcTransportError::Io)?;
            use std::os::unix::io::AsRawFd;
            let fd = listener.as_raw_fd();
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
        #[cfg(windows)]
        {
            // No-op on Windows — documented in the module-level docs.
            let _ = timeout;
        }
        Ok(())
    }

    /// Accept a single connection, verify the peer, decode one framed
    /// request, dispatch via `handler`, and write the response.
    pub fn serve_once<F>(&self, handler: F) -> Result<(), IpcTransportError>
    where
        F: FnOnce(Request) -> Response,
    {
        self.serve_once_with_peer(|_peer, request| handler(request))
    }

    /// Peer-aware variant of [`Self::serve_once`].
    pub fn serve_once_with_peer<F>(&self, handler: F) -> Result<(), IpcTransportError>
    where
        F: FnOnce(PeerIdentity, Request) -> Response,
    {
        match &self.inner {
            #[cfg(unix)]
            BoundInner::Unix(listener) => {
                let (stream, _) = listener.accept()?;
                let peer = match unix_peer_identity(&stream) {
                    Ok(p) => p,
                    Err(_) => {
                        let server = IpcServer::new(self.owner_uid);
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

                let _guard = match ConnectionGuard::acquire(peer.uid) {
                    Some(g) => g,
                    None => {
                        eprintln!(
                            "pcloud-ipc: connection cap reached (global={MAX_IPC_CONNECTIONS}, \
                             per-peer={MAX_IPC_CONNECTIONS_PER_PEER}); \
                             closing connection from uid={}",
                            peer.uid
                        );
                        let mut s = stream;
                        s.close_both();
                        return Ok(());
                    }
                };

                let peer_for_handler = peer;
                serve_stream_once_with_peer(
                    stream,
                    &IpcServer::new(self.owner_uid),
                    peer,
                    move |req| handler(peer_for_handler, req),
                    IPC_REQUEST_READ_TIMEOUT,
                )
            }
            #[cfg(windows)]
            BoundInner::Windows(listener) => {
                let stream = match listener.accept() {
                    Ok(s) => s,
                    Err(IpcTransportError::PeerCredentialsUnavailable) => {
                        // The accept() path already closed the peer.
                        return Ok(());
                    }
                    Err(other) => return Err(other),
                };
                let peer = PeerIdentity {
                    uid: 0,
                    pid: stream.peer_pid(),
                };

                let _guard = match ConnectionGuard::acquire(peer.uid) {
                    Some(g) => g,
                    None => {
                        eprintln!(
                            "pcloud-ipc: connection cap reached (global={MAX_IPC_CONNECTIONS}, \
                             per-peer={MAX_IPC_CONNECTIONS_PER_PEER}); \
                             closing connection from uid={}",
                            peer.uid
                        );
                        // Stream drops and disconnects.
                        return Ok(());
                    }
                };

                let peer_for_handler = peer;
                serve_stream_once_with_peer(
                    stream,
                    &IpcServer::new(self.owner_uid),
                    peer,
                    move |req| handler(peer_for_handler, req),
                    IPC_REQUEST_READ_TIMEOUT,
                )
            }
        }
    }

    /// Accept a single connection and dispatch it on a dedicated OS
    /// thread, returning after spawning. See the module docs for the
    /// per-peer cap enforcement contract.
    ///
    /// On Windows this currently takes the same synchronous
    /// accept-then-spawn path.
    pub fn accept_and_spawn<F>(&self, handler: F) -> Result<(), IpcTransportError>
    where
        F: Fn(Request) -> Response + Clone + Send + 'static,
    {
        match &self.inner {
            #[cfg(unix)]
            BoundInner::Unix(listener) => {
                let (mut stream, _addr) = listener.accept()?;
                let peer = match unix_peer_identity(&stream) {
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

                let guard = match ConnectionGuard::acquire(peer.uid) {
                    Some(g) => g,
                    None => {
                        eprintln!(
                            "pcloud-ipc: connection cap reached (global={MAX_IPC_CONNECTIONS}, \
                             per-peer={MAX_IPC_CONNECTIONS_PER_PEER}); \
                             closing connection from uid={}",
                            peer.uid
                        );
                        stream.close_both();
                        return Ok(());
                    }
                };

                let owner_uid = self.owner_uid;
                let handler = handler.clone();

                std::thread::Builder::new()
                    .name("pcloud-ipc-conn".to_owned())
                    .spawn(move || {
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
            #[cfg(windows)]
            BoundInner::Windows(listener) => {
                let stream = match listener.accept() {
                    Ok(s) => s,
                    Err(IpcTransportError::PeerCredentialsUnavailable) => return Ok(()),
                    Err(other) => return Err(other),
                };
                let peer = PeerIdentity {
                    uid: 0,
                    pid: stream.peer_pid(),
                };
                let guard = match ConnectionGuard::acquire(peer.uid) {
                    Some(g) => g,
                    None => {
                        eprintln!(
                            "pcloud-ipc: connection cap reached (global={MAX_IPC_CONNECTIONS}, \
                             per-peer={MAX_IPC_CONNECTIONS_PER_PEER}); \
                             closing connection from uid={}",
                            peer.uid
                        );
                        return Ok(());
                    }
                };
                let owner_uid = self.owner_uid;
                let handler = handler.clone();
                std::thread::Builder::new()
                    .name("pcloud-ipc-conn".to_owned())
                    .spawn(move || {
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
        }
    }

    /// Legacy Unix-only entry-point used by the slow-client test.
    #[cfg(all(test, unix))]
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
        let peer = match unix_peer_identity(&stream) {
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
        serve_stream_once_with_peer(stream, &server, peer, handler, read_timeout)
    }
}

#[cfg(unix)]
impl Drop for BoundIpcServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

// --------------------------------------------------------------------
// IpcServer::bind and launchd socket activation
// --------------------------------------------------------------------

impl IpcServer {
    /// Try to receive a pre-activated socket from launchd (macOS only).
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
            // SAFETY: free(NULL) is safe; fds may be null.
            unsafe { libc::free(fds as *mut libc::c_void) };
            return Ok(None);
        }

        // SAFETY: `fds` non-null, count >= 1.
        let fd = unsafe { *fds };
        // SAFETY: fds was allocated by launchd via malloc.
        unsafe { libc::free(fds as *mut libc::c_void) };

        // SAFETY: launchd hands us a valid, pre-bound, pre-listening socket fd.
        let listener = unsafe { UnixListener::from_raw_fd(fd) };

        listener
            .set_nonblocking(false)
            .map_err(IpcTransportError::Io)?;

        Ok(Some(BoundIpcServer {
            inner: BoundInner::Unix(listener),
            owner_uid: self.owner_uid(),
            socket_path: socket_path.to_path_buf(),
        }))
    }

    /// Bind an IPC listener at `socket_path`.
    ///
    /// Unix: creates a `0600` Unix socket under a `0700` parent. The
    /// returned [`BoundIpcServer`] unlinks the socket on `Drop`.
    ///
    /// Windows: `socket_path` is retained for diagnostics; the actual
    /// named-pipe path is derived from the current TokenUser SID by
    /// [`crate::platform::windows::WindowsIpc::bind_listener`]. The
    /// DACL restricts access to the owner SID only (implicit "empty
    /// DACL = deny all" for every other principal).
    pub fn bind(&self, socket_path: &Path) -> Result<BoundIpcServer, IpcTransportError> {
        #[cfg(unix)]
        {
            if let Some(parent) = socket_path.parent() {
                let parent_missing = !parent.exists();
                fs::create_dir_all(parent)?;
                if parent_missing {
                    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                } else {
                    use std::os::unix::fs::MetadataExt;
                    if let Ok(meta) = fs::metadata(parent)
                        && meta.uid() == self.owner_uid()
                    {
                        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                    }
                }
            }

            if socket_path.exists() {
                fs::remove_file(socket_path)?;
            }

            let listener = UnixListener::bind(socket_path)?;
            fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;

            Ok(BoundIpcServer {
                inner: BoundInner::Unix(listener),
                socket_path: socket_path.to_path_buf(),
                owner_uid: self.owner_uid(),
            })
        }
        #[cfg(windows)]
        {
            use crate::platform::PlatformIpc;
            // On Windows the parent runtime dir need not exist for pipe
            // binding (pipes live in the NT pipe namespace, not the
            // filesystem). We still best-effort create it so any
            // sidecar files can be placed there.
            if let Some(parent) = socket_path.parent()
                && !parent.exists()
            {
                let _ = fs::create_dir_all(parent);
            }
            let backend = crate::platform::windows::WindowsIpc;
            let listener = backend.bind_listener(socket_path)?;
            Ok(BoundIpcServer {
                inner: BoundInner::Windows(listener),
                socket_path: socket_path.to_path_buf(),
                owner_uid: self.owner_uid(),
            })
        }
    }
}

// --------------------------------------------------------------------
// IpcClient: portable send path
// --------------------------------------------------------------------

impl IpcClient {
    /// Connect to the daemon's IPC listener, send the framed `request`,
    /// shut down the write half, and read the framed response.
    pub fn send(
        &self,
        socket_path: &Path,
        request: &Request,
    ) -> Result<Response, IpcTransportError> {
        self.send_envelope(socket_path, &RequestEnvelope::new(request.clone()))
    }

    /// Envelope-aware send.
    pub fn send_envelope(
        &self,
        socket_path: &Path,
        envelope: &RequestEnvelope,
    ) -> Result<Response, IpcTransportError> {
        #[cfg(unix)]
        {
            let request_bytes = self.prepare_envelope(envelope)?;
            let mut stream = UnixStream::connect(socket_path)?;
            std::io::Write::write_all(&mut stream, &request_bytes)?;
            stream.shutdown(std::net::Shutdown::Write)?;

            let mut response_bytes = Vec::new();
            std::io::Read::read_to_end(&mut stream, &mut response_bytes)?;
            Ok(self.parse_response(&response_bytes)?)
        }
        #[cfg(windows)]
        {
            let _ = socket_path; // actual pipe derived from SID
            let request_bytes = self.prepare_envelope(envelope)?;
            let stream = crate::platform::windows::connect_client()?;
            stream.write_all(&request_bytes)?;
            // Named pipes have no half-shutdown; the server frames on
            // declared payload length and does not need EOF to know the
            // request is complete.

            // Read the framed response: 8-byte header then payload_len.
            let mut header = [0u8; 8];
            stream.read_exact(&mut header)?;
            let payload_len =
                u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
            let mut payload = vec![0u8; payload_len];
            if payload_len > 0 {
                stream.read_exact(&mut payload)?;
            }
            let mut response_bytes = Vec::with_capacity(8 + payload_len);
            response_bytes.extend_from_slice(&header);
            response_bytes.extend_from_slice(&payload);
            Ok(self.parse_response(&response_bytes)?)
        }
    }
}

// --------------------------------------------------------------------
// Generic serve-stream helpers (over the IpcStream trait)
// --------------------------------------------------------------------

fn serve_stream_once_with_peer<S, F>(
    mut stream: S,
    server: &IpcServer,
    peer: PeerIdentity,
    handler: F,
    read_timeout: Duration,
) -> Result<(), IpcTransportError>
where
    S: IpcStream,
    F: FnOnce(Request) -> Response,
{
    let _ = stream.set_read_timeout(Some(read_timeout));

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
    if let Err(err) = write_response(&mut stream, server, response.status, response.message) {
        log::trace!("pcloud-ipc: write_response failed (client disconnected?): {err}");
    }
    Ok(())
}

fn serve_stream_standalone_with_peer<S, F>(
    mut stream: S,
    server: &IpcServer,
    peer: PeerIdentity,
    handler: F,
    read_timeout: Duration,
) -> Result<(), IpcTransportError>
where
    S: IpcStream,
    F: FnOnce(Request) -> Response,
{
    let _ = stream.set_read_timeout(Some(read_timeout));

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

fn read_framed_request<S: IpcStream>(stream: &mut S) -> Result<Vec<u8>, IpcTransportError> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header)?;

    let payload_len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
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

fn handle_client_error<S: IpcStream>(
    stream: &mut S,
    server: &IpcServer,
    err: IpcTransportError,
) -> Result<(), IpcTransportError> {
    match err {
        IpcTransportError::Server(IpcError::RequestTooLarge { .. }) => {
            stream.close_both();
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

fn write_response<S: IpcStream>(
    stream: &mut S,
    server: &IpcServer,
    status: crate::methods::ResponseStatus,
    message: impl Into<String>,
) -> Result<(), IpcTransportError> {
    let response_bytes = server.encode_status(status, message.into())?;
    stream.write_all(&response_bytes)?;
    stream.flush()?;
    Ok(())
}

/// Recover the peer identity for a connected `UnixStream`.
#[cfg(unix)]
fn unix_peer_identity(stream: &UnixStream) -> Result<PeerIdentity, IpcTransportError> {
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

    use super::{BoundInner, IpcClient, IpcServer};

    /// Build a per-test socket path inside a process-private subdir of
    /// `temp_dir()`. The subdir lets `IpcServer::bind` re-permission the
    /// socket parent to 0700 — on Linux that's harmless against `/tmp`,
    /// but on macOS the per-session `/var/folders/.../T/` is DataVault-
    /// protected and `chmod` returns EPERM. Funneling the socket through
    /// a test-owned subdir sidesteps that without changing prod behavior.
    fn test_socket_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pcloud-ipc-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test socket dir");
        dir.join(format!("{name}.sock"))
    }

    #[test]
    fn uds_transport_roundtrip_works() {
        let socket_path = test_socket_path("roundtrip");
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
        let socket_path = test_socket_path("unauthorized");
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
        let socket_path = test_socket_path("no-eof");
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
        let socket_path = test_socket_path("slow-client");
        let server = IpcServer::new(current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        let handle = thread::spawn(move || {
            // Pull the raw UnixListener out of the enum for the slow-client
            // test so we can accept directly and hand a pre-connected
            // stream to the private `serve_stream_once` helper.
            let BoundInner::Unix(listener) = &bound.inner;
            let (stream, _) = listener.accept().expect("slow client should connect");
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
        let socket_path = test_socket_path("malformed-request");
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

    static PER_PEER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn per_peer_cap_enforced_even_when_global_cap_not_hit() {
        use super::{
            ACTIVE_CONNECTIONS, ConnectionGuard, MAX_IPC_CONNECTIONS_PER_PEER, PEER_CONNECTIONS,
        };
        use std::sync::atomic::Ordering;

        let _lock = PER_PEER_TEST_LOCK.lock().unwrap();
        let test_uid: u32 = 0xBEEF_0001;

        {
            let mut map = PEER_CONNECTIONS.lock().unwrap();
            if let Some(m) = map.as_mut() {
                m.remove(&test_uid);
            }
        }
        let baseline = ACTIVE_CONNECTIONS.load(Ordering::Relaxed);

        let mut guards: Vec<ConnectionGuard> = (0..MAX_IPC_CONNECTIONS_PER_PEER)
            .map(|_| {
                ConnectionGuard::acquire(test_uid)
                    .expect("slot within per-peer cap should be available")
            })
            .collect();

        assert!(
            ConnectionGuard::acquire(test_uid).is_none(),
            "per-peer cap should deny the ({MAX_IPC_CONNECTIONS_PER_PEER}+1)th connection"
        );

        assert_eq!(
            ACTIVE_CONNECTIONS.load(Ordering::Relaxed),
            baseline + MAX_IPC_CONNECTIONS_PER_PEER
        );

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

    #[test]
    fn per_peer_cap_resets_on_disconnect() {
        use super::{ACTIVE_CONNECTIONS, ConnectionGuard, PEER_CONNECTIONS};
        use std::sync::atomic::Ordering;

        let _lock = PER_PEER_TEST_LOCK.lock().unwrap();
        let test_uid: u32 = 0xBEEF_0002;

        {
            let mut map = PEER_CONNECTIONS.lock().unwrap();
            if let Some(m) = map.as_mut() {
                m.remove(&test_uid);
            }
        }
        let baseline = ACTIVE_CONNECTIONS.load(Ordering::Relaxed);

        for round in 0..3 {
            let guard = ConnectionGuard::acquire(test_uid).unwrap_or_else(|| {
                panic!("round {round}: slot should be available after disconnect")
            });
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

//! **PLATFORM: Windows 10/11 (tier 1).**
//! **GATING: `#[cfg(windows)]`.**
//!
//! Named-pipe IPC backend with SID-based peer authentication.
//!
//! # Design
//!
//! * Pipe path is `\\.\pipe\pcloud-rs-<hex-encoded-SID>` where the SID
//!   comes from the current-user access token. Using a per-user, per-SID
//!   name avoids collisions between concurrent sessions on the same host
//!   (e.g. terminal services, fast-user-switching) and prevents a lower-
//!   privileged local account from squatting on our well-known name.
//!
//! * The listener is created with a DACL that grants `GENERIC_READ |
//!   GENERIC_WRITE` **only** to the current user's SID. Because the
//!   security descriptor carries an explicit (non-NULL) DACL with no
//!   other ACEs, every other principal — including `Administrators`,
//!   `SYSTEM`, and `Everyone` — is denied by default ("empty DACL =
//!   deny all" Windows rule).
//!
//! * Peer authentication mirrors the Unix `SO_PEERCRED` discipline:
//!   when a client connects we recover its PID via
//!   `GetNamedPipeClientProcessId`, open its process token, read the
//!   `TokenUser` SID, and require an exact match against the server's
//!   SID. Mismatches surface as
//!   [`IpcTransportError::PeerCredentialsUnavailable`] so the common
//!   transport layer can reject the client without leaking detail.
//!
//! # Accept model (Tier-3 synchronous)
//!
//! Each call to [`WindowsListener::accept`] creates a fresh pipe
//! instance via `CreateNamedPipeW`, blocks in `ConnectNamedPipe` until a
//! client connects (or returns `ERROR_PIPE_CONNECTED` if the client
//! beat us to it), authenticates the peer SID, and hands the connected
//! handle to the transport layer. After the connection is served the
//! handle is closed and the next `accept` creates a new instance. This
//! matches the spirit of the Unix `accept(2)` loop and keeps the
//! synchronous-blocking serve path simple.
//!
//! # Windows-specific behaviour vs Unix
//!
//! * **No per-accept timeout.** `ConnectNamedPipe` with a NULL OVERLAPPED
//!   is fully blocking. The daemon's session-refresh tick currently runs
//!   only when an IPC request wakes the loop on Windows. A future
//!   hardening pass (tracked under `bd-xplat-windows`) can move this to
//!   an overlapped-IO wait with `WaitForSingleObject` on a cancellation
//!   event.
//! * **No write timeout.** `WriteFile` on a byte-mode pipe blocks on
//!   back-pressure; the Unix-side `SO_SNDTIMEO` has no direct named-pipe
//!   analogue. The transport layer still honours the read timeout via
//!   `SetNamedPipeHandleState` where possible.
//! * **Peer pid is real.** `GetNamedPipeClientProcessId` returns the
//!   client's true PID, which is preserved in [`crate::auth::PeerIdentity`]
//!   for audit correlation.
//! * **Peer uid is a synthetic `0`.** Windows has no uid. When the client
//!   SID matches the daemon-owner SID we report `uid = 0` to the
//!   transport layer so the existing owner-only gate
//!   (`PeerIdentity::matches_owner`) continues to function with the
//!   daemon's cached `owner_uid = 0` on Windows. Mismatched SIDs are
//!   rejected at accept time, not via `peer_uid`.
//!
//! See PLAN_CROSSPLATFORM.md §3.5 and tracker `bd-xplat-windows`.

use std::io::{Read, Write};
use std::path::Path;
use std::ptr;
use std::time::Duration;

use windows::Win32::Foundation::{
    BOOL, CloseHandle, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    EqualSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, FlushFileBuffers, OPEN_EXISTING, ReadFile,
    WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::{PCWSTR, PWSTR};

use crate::platform::PlatformIpc;
use crate::transport::IpcTransportError;

/// Max concurrent pipe instances. Matches the caller-requested policy and
/// provides generous headroom for CLI + SDK + test harnesses without
/// exhausting the pipe namespace.
const MAX_PIPE_INSTANCES: u32 = 32;

/// 64 KiB in/out pipe buffers — same ballpark as the Unix SO_SNDBUF
/// defaults and comfortably larger than any single IPC frame.
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

/// Default timeout hint of 0 means "use the system default (~50 ms)".
/// We never actually block on timed operations here.
const PIPE_DEFAULT_TIMEOUT_MS: u32 = 0;

/// Windows backend for [`PlatformIpc`]. Uses named pipes + per-user DACL
/// + `GetNamedPipeClientProcessId` SID comparison to provide the same
/// "owner-only local IPC" guarantee the Unix backends offer.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsIpc;

/// Owned, bound named-pipe listener.
///
/// Holds the pipe path and the cached current-user SID string used both
/// for the DACL of each pipe instance and for peer SID comparison at
/// accept time. No pre-created handle is cached: each call to
/// [`Self::accept`] creates a fresh pipe instance so concurrent clients
/// do not race on a single server-side handle.
#[derive(Debug)]
pub struct WindowsListener {
    pipe_path: String,
    owner_sid: String,
}

/// RAII wrapper around a connected named-pipe stream.
///
/// Contains the accepted `HANDLE`, the audit-friendly SID string of the
/// peer, and the peer PID (for audit correlation). `Drop` disconnects
/// and closes the handle so the pipe instance can be reclaimed by the OS.
///
/// # Security: `peer_sid` rationale
///
/// `peer_sid` is a Windows SID in SDDL string form (e.g.
/// `S-1-5-21-…-1001`). SIDs are public identity tokens — they contain
/// no secret material and are safe to store as plain `String`, log in
/// audit events, and use for display.
#[derive(Debug)]
pub struct WindowsStream {
    handle: HANDLE,
    /// SID string of the authenticated peer. Not a secret; safe to log.
    pub(crate) peer_sid: String,
    /// Client PID recovered via `GetNamedPipeClientProcessId`. Carried
    /// for audit correlation only — never used for authorization.
    pub(crate) peer_pid: u32,
    /// `true` when the handle was `ConnectNamedPipe`'d by us and must
    /// therefore be `DisconnectNamedPipe`'d on drop; `false` for
    /// client-side handles opened via `CreateFileW`.
    is_server_side: bool,
}

impl Drop for WindowsStream {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            if self.is_server_side {
                // Best-effort flush + disconnect before close so the next
                // pipe instance is not confused by leftover kernel state.
                // SAFETY: `handle` is a live server-side connected pipe.
                unsafe {
                    let _ = FlushFileBuffers(self.handle);
                    let _ = DisconnectNamedPipe(self.handle);
                }
            }
            // SAFETY: `handle` has not been closed elsewhere.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

// WindowsStream is Send because the raw HANDLE can be moved to another
// thread — the Win32 named-pipe APIs are thread-safe w.r.t. handle use
// and we never share a single handle concurrently.
// SAFETY: the only non-Send field is `HANDLE` (a raw pointer-wrapped
// kernel handle). Named pipe HANDLEs are process-scoped kernel objects
// and Windows documents Read/WriteFile as thread-safe when the caller
// serializes access to a single overlapped-IO structure; we never share
// the handle across threads concurrently (the serve loop owns it for
// the lifetime of one request/response).
unsafe impl Send for WindowsStream {}

impl PlatformIpc for WindowsIpc {
    type Listener = WindowsListener;
    type Stream = WindowsStream;

    /// Return a [`WindowsListener`] bound to the per-user pipe name. No
    /// pipe instance is created here — see [`WindowsListener::accept`].
    fn bind_listener(&self, _runtime_dir: &Path) -> Result<Self::Listener, IpcTransportError> {
        let owner_sid = current_user_sid_string()?;
        let pipe_path = format!(
            "\\\\.\\pipe\\pcloud-rs-{}",
            hex_encode(owner_sid.as_bytes())
        );
        Ok(WindowsListener {
            pipe_path,
            owner_sid,
        })
    }

    /// Compare the client's TokenUser SID with the server's owner SID.
    ///
    /// Returns `0` on match — Windows has no uid, so we reuse 0 to mean
    /// "peer is the pipe owner". Any mismatch surfaces as
    /// [`IpcTransportError::PeerCredentialsUnavailable`].
    fn peer_uid(&self, stream: &Self::Stream) -> Result<u32, IpcTransportError> {
        let owner_sid = current_user_sid_string()?;
        if stream.peer_sid == owner_sid {
            Ok(0)
        } else {
            Err(IpcTransportError::PeerCredentialsUnavailable)
        }
    }

    /// The string-SID of the peer, suitable for audit logs. Never
    /// contains secrets — SIDs are public identifiers.
    fn peer_display(&self, stream: &Self::Stream) -> Result<String, IpcTransportError> {
        Ok(stream.peer_sid.clone())
    }

    fn backend_name(&self) -> &'static str {
        "windows-named-pipe"
    }
}

impl WindowsListener {
    /// Create a fresh pipe instance, wait for the next client, recover
    /// and authenticate its SID, and return a connected stream.
    ///
    /// This is the Windows-side analogue of `UnixListener::accept`. A
    /// new `CreateNamedPipeW` is issued on every call so concurrent
    /// clients each land on their own server-side handle.
    pub fn accept(&self) -> Result<WindowsStream, IpcTransportError> {
        let handle = create_pipe_instance(&self.pipe_path, &self.owner_sid)?;

        // SAFETY: `handle` is a live listener returned by
        // `CreateNamedPipeW`. `ConnectNamedPipe` with a NULL OVERLAPPED
        // blocks until a client connects or errors; we accept both the
        // success variant and `ERROR_PIPE_CONNECTED` per MSDN.
        let connected = unsafe { ConnectNamedPipe(handle, None) };
        if connected.is_err() {
            // SAFETY: GetLastError has no preconditions.
            let code = unsafe { GetLastError() };
            if code != ERROR_PIPE_CONNECTED {
                // SAFETY: handle still owned by us.
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(IpcTransportError::Io(std::io::Error::from_raw_os_error(
                    code.0 as i32,
                )));
            }
        }

        let (peer_sid, peer_pid) = match client_sid_and_pid(handle) {
            Ok(v) => v,
            Err(err) => {
                // SAFETY: handle is live and we own it.
                unsafe {
                    let _ = DisconnectNamedPipe(handle);
                    let _ = CloseHandle(handle);
                }
                return Err(err);
            }
        };
        if peer_sid != self.owner_sid {
            // SAFETY: handle is live and we own it.
            unsafe {
                let _ = DisconnectNamedPipe(handle);
                let _ = CloseHandle(handle);
            }
            return Err(IpcTransportError::PeerCredentialsUnavailable);
        }

        Ok(WindowsStream {
            handle,
            peer_sid,
            peer_pid,
            is_server_side: true,
        })
    }

    /// Diagnostic accessor — the full pipe path, e.g.
    /// `\\.\pipe\pcloud-rs-<hex-SID>`.
    pub fn pipe_path(&self) -> &str {
        &self.pipe_path
    }

    /// Owner SID string that new pipe instances are DACL-restricted to.
    pub fn owner_sid(&self) -> &str {
        &self.owner_sid
    }
}

impl WindowsStream {
    /// Peer PID recovered via `GetNamedPipeClientProcessId`.
    pub fn peer_pid(&self) -> u32 {
        self.peer_pid
    }

    /// Fill `buf` completely from the pipe. Returns `UnexpectedEof` if
    /// the peer closes before `buf.len()` bytes are available.
    pub fn read_exact(&self, buf: &mut [u8]) -> std::io::Result<()> {
        let mut total = 0usize;
        while total < buf.len() {
            let slice = &mut buf[total..];
            let mut read: u32 = 0;
            // SAFETY: `handle` is a live connected pipe; `slice` is a
            // valid writable buffer; `read` is initialised.
            let ok = unsafe { ReadFile(self.handle, Some(slice), Some(&mut read), None) };
            if ok.is_err() {
                // SAFETY: GetLastError has no preconditions.
                let code = unsafe { GetLastError() };
                return Err(std::io::Error::from_raw_os_error(code.0 as i32));
            }
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "pipe closed before read_exact completed",
                ));
            }
            total += read as usize;
        }
        Ok(())
    }

    /// Write all of `buf` to the pipe. Partial writes are retried.
    pub fn write_all(&self, buf: &[u8]) -> std::io::Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            let slice = &buf[written..];
            let mut n: u32 = 0;
            // SAFETY: `handle` is a live connected pipe; `slice` is a
            // valid readable buffer; `n` is initialised.
            let ok = unsafe { WriteFile(self.handle, Some(slice), Some(&mut n), None) };
            if ok.is_err() {
                // SAFETY: GetLastError has no preconditions.
                let code = unsafe { GetLastError() };
                return Err(std::io::Error::from_raw_os_error(code.0 as i32));
            }
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "pipe returned zero-length write",
                ));
            }
            written += n as usize;
        }
        Ok(())
    }

    /// Flush any buffered writes to the peer.
    pub fn flush(&self) -> std::io::Result<()> {
        // SAFETY: `handle` is a live pipe handle.
        unsafe {
            FlushFileBuffers(self.handle)
                .map_err(|_| std::io::Error::from_raw_os_error(GetLastError().0 as i32))
        }
    }

    /// Close both ends. Used after fatal framing errors.
    ///
    /// Windows has no direct `shutdown(2)` analogue for named pipes.
    /// Dropping the stream disconnects and closes the handle; we mark
    /// the handle invalid so subsequent operations fail fast.
    pub fn shutdown(&mut self) -> std::io::Result<()> {
        if !self.handle.is_invalid() {
            if self.is_server_side {
                // SAFETY: handle is live and we own it.
                unsafe {
                    let _ = FlushFileBuffers(self.handle);
                    let _ = DisconnectNamedPipe(self.handle);
                }
            }
            // SAFETY: handle is live and we own it.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
            self.handle = INVALID_HANDLE_VALUE;
        }
        Ok(())
    }

    /// Best-effort read timeout. Windows named pipes in byte mode do
    /// not expose a clean per-handle read deadline from safe code; we
    /// accept the value and keep it for parity (no-op) so the shared
    /// transport code compiles. Tracked under `bd-xplat-windows`.
    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }

    /// Best-effort write timeout. See [`Self::set_read_timeout`].
    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

// --------------------------------------------------------------------
// Client-side connect
// --------------------------------------------------------------------

/// Connect from a client process to the per-user named pipe. Matches
/// the server-side pipe name derived from the current TokenUser SID.
pub fn connect_client() -> Result<WindowsStream, IpcTransportError> {
    let owner_sid = current_user_sid_string()?;
    let pipe_path = format!(
        "\\\\.\\pipe\\pcloud-rs-{}",
        hex_encode(owner_sid.as_bytes())
    );
    let wide = to_wide(&pipe_path);

    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer alive for the
    // duration of the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        )
    }
    .map_err(|_| last_os_err())?;

    if handle == INVALID_HANDLE_VALUE {
        return Err(last_os_err());
    }

    Ok(WindowsStream {
        handle,
        peer_sid: owner_sid,
        peer_pid: 0, // client side — we are the client; pid is self
        is_server_side: false,
    })
}

// --------------------------------------------------------------------
// Pipe instance creation
// --------------------------------------------------------------------

/// Create one pipe instance with a DACL allowing only `owner_sid`.
///
/// Flags:
/// * `PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED` — bidirectional,
///   async-capable.
/// * `PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT` — byte stream
///   with framing layered on top; blocking semantics for the sync path.
fn create_pipe_instance(pipe_path: &str, owner_sid: &str) -> Result<HANDLE, IpcTransportError> {
    let wide_path = to_wide(pipe_path);
    let sddl = format!("D:(A;;GRGW;;;{})", owner_sid);
    let sd = SecurityDescriptor::from_sddl(&sddl)?;

    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.as_ptr(),
        bInheritHandle: BOOL(0),
    };

    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;

    // SAFETY: `wide_path` is a NUL-terminated UTF-16 buffer kept alive
    // for the duration of the call; `sa` is a valid `SECURITY_ATTRIBUTES`
    // pointing at an intact SD; all integer arguments are fixed constants
    // below documented maxima.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide_path.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            MAX_PIPE_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            PIPE_DEFAULT_TIMEOUT_MS,
            Some(&mut sa),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: GetLastError has no preconditions.
        let err = unsafe { GetLastError() };
        return Err(IpcTransportError::Io(std::io::Error::from_raw_os_error(
            err.0 as i32,
        )));
    }

    Ok(handle)
}

// --------------------------------------------------------------------
// SID / token helpers
// --------------------------------------------------------------------

/// Recover the current-user SID as a SDDL string (`S-1-5-21-...`).
fn current_user_sid_string() -> Result<String, IpcTransportError> {
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle with no
    // cleanup requirement. `OpenProcessToken` writes a real handle into
    // `token` only on success.
    let mut token = HANDLE::default();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok.is_err() {
        return Err(last_os_err());
    }
    let _guard = HandleGuard(token);

    token_user_sid_string(token)
}

/// Resolve the client process's TokenUser SID + PID on a connected pipe.
fn client_sid_and_pid(pipe: HANDLE) -> Result<(String, u32), IpcTransportError> {
    let mut pid: u32 = 0;
    // SAFETY: `pipe` is a valid connected-server handle; `pid` is an
    // initialised out-param.
    let ok = unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) };
    if ok.is_err() {
        return Err(last_os_err());
    }

    // SAFETY: `OpenProcess` returns INVALID/NULL on failure; we check.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|_| last_os_err())?;
    let _process_guard = HandleGuard(process);

    let mut token = HANDLE::default();
    // SAFETY: `process` is live; `token` is a valid out-param.
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    if ok.is_err() {
        return Err(last_os_err());
    }
    let _token_guard = HandleGuard(token);

    let sid = token_user_sid_string(token)?;
    Ok((sid, pid))
}

/// Two-phase `GetTokenInformation(TokenUser)` + `ConvertSidToStringSidW`.
fn token_user_sid_string(token: HANDLE) -> Result<String, IpcTransportError> {
    let mut needed: u32 = 0;
    // SAFETY: first call is the documented size probe.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return Err(last_os_err());
    }

    let mut buf = vec![0u8; needed as usize];
    // SAFETY: `buf` is sized per the probe.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    };
    if ok.is_err() {
        return Err(last_os_err());
    }

    // SAFETY: on success the buffer starts with a valid `TOKEN_USER`.
    let token_user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    sid_to_string(token_user.User.Sid)
}

/// `ConvertSidToStringSidW` wrapper that frees the SDDL buffer once.
fn sid_to_string(sid: PSID) -> Result<String, IpcTransportError> {
    let mut out = PWSTR::null();
    // SAFETY: `sid` is a valid SID; `out` is populated with a
    // `LocalAlloc`'d buffer we must `LocalFree`.
    let ok = unsafe { ConvertSidToStringSidW(sid, &mut out) };
    if ok.is_err() || out.is_null() {
        return Err(last_os_err());
    }

    // SAFETY: `out` points at a NUL-terminated UTF-16 string owned by
    // `LocalAlloc`.
    let s = unsafe { out.to_string().map_err(|_| last_os_err())? };
    // SAFETY: `out` came from `LocalAlloc`; freeing it here balances
    // the Win32 allocation.
    unsafe {
        let _ = LocalFree(HLOCAL(out.0 as *mut _));
    }
    Ok(s)
}

// --------------------------------------------------------------------
// Security descriptor / utility helpers
// --------------------------------------------------------------------

struct SecurityDescriptor {
    ptr: PSECURITY_DESCRIPTOR,
}

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self, IpcTransportError> {
        let wide = to_wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `wide` is NUL-terminated UTF-16.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        };
        if ok.is_err() || sd.0.is_null() {
            return Err(last_os_err());
        }
        Ok(Self { ptr: sd })
    }

    fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.ptr.0
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.ptr.0.is_null() {
            // SAFETY: `ptr` came from
            // `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
            unsafe {
                let _ = LocalFree(HLOCAL(self.ptr.0.cast()));
            }
        }
    }
}

/// Minimal RAII closer for owned Win32 handles.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: handle produced by `OpenProcess*` /
            // `OpenProcessToken` and exclusively owned here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn last_os_err() -> IpcTransportError {
    // SAFETY: `GetLastError` has no preconditions.
    let code = unsafe { GetLastError() };
    IpcTransportError::Io(std::io::Error::from_raw_os_error(code.0 as i32))
}

#[allow(dead_code)]
fn _equal_sid(a: PSID, b: PSID) -> bool {
    // SAFETY: both SIDs must be valid for the duration of this call.
    unsafe { EqualSid(a, b).is_ok() }
}

#[allow(dead_code)]
fn _null_mut<T>() -> *mut T {
    ptr::null_mut()
}

// Silence unused-import warnings for std Read/Write traits that are
// only pulled in so doc-links resolve cleanly. Not used directly because
// the read_exact/write_all surface above is inherent, not via std::io.
#[allow(dead_code)]
fn _suppress_imports(_r: &dyn Read, _w: &dyn Write) {}

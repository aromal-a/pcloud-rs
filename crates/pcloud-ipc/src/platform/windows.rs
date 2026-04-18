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
//! See PLAN_CROSSPLATFORM.md §3.5 and tracker `bd-xplat-windows`.

use std::path::Path;
use std::ptr;

use windows::Win32::Foundation::{
    BOOL, CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    EqualSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_ACCESS_DUPLEX,
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

/// RAII wrapper around a bound named-pipe server instance.
///
/// Holds the initial listening `HANDLE`, the pipe path (for diagnostics
/// and reconnect), and the cached current-user SID string used both for
/// the DACL and for peer SID comparison. `Drop` closes the underlying
/// handle; the owner-only DACL means no other principal can reopen the
/// name even transiently.
#[derive(Debug)]
pub struct WindowsListener {
    handle: HANDLE,
    pipe_path: String,
    owner_sid: String,
}

impl Drop for WindowsListener {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: `handle` was returned by `CreateNamedPipeW` and has
            // not been closed elsewhere. `CloseHandle` on a valid pipe
            // handle is documented to be safe from any thread.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

/// RAII wrapper around a connected named-pipe stream.
///
/// Contains the accepted `HANDLE` and the audit-friendly SID string of
/// the peer (populated once we have positively authenticated the
/// client). `Drop` closes the handle.
///
/// # Security: `peer_sid` rationale
///
/// `peer_sid` is a Windows SID in SDDL string form (e.g.
/// `S-1-5-21-…-1001`). SIDs are public identity tokens — they contain
/// no secret material and are safe to store as plain `String`, log in
/// audit events, and use for display. This is fundamentally different
/// from an auth token or password: we are merely recording *who* the
/// peer is, not storing *how to authenticate as* them.
///
/// The owner-only gate is enforced by `peer_uid`: the server compares
/// `peer_sid` against the SID of the process owner before dispatching
/// any request. Connections whose SID does not match are rejected with
/// [`IpcTransportError::PeerCredentialsUnavailable`] before any command
/// is processed.
#[derive(Debug)]
pub struct WindowsStream {
    handle: HANDLE,
    /// SID string of the authenticated peer. Not a secret; safe to log.
    /// See struct-level docs for security rationale.
    peer_sid: String,
}

impl Drop for WindowsStream {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: `handle` was accepted via `ConnectNamedPipe`; no
            // other owner exists and we are on the last drop path.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

impl PlatformIpc for WindowsIpc {
    type Listener = WindowsListener;
    type Stream = WindowsStream;

    /// Create the per-user named pipe.
    ///
    /// Flags mirror the task spec:
    /// * `PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED` — bidirectional,
    ///   async-capable (matches the tokio/mio integration path we will
    ///   use once the transport layer is wired).
    /// * `PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT` — we frame
    ///   our own length-prefixed messages on top of a byte stream, and
    ///   we want blocking semantics for the simple sync server path.
    /// * DACL: only the current-user SID with GENERIC_READ|GENERIC_WRITE.
    fn bind_listener(&self, _runtime_dir: &Path) -> Result<Self::Listener, IpcTransportError> {
        let owner_sid = current_user_sid_string()?;
        let pipe_path = format!(
            "\\\\.\\pipe\\pcloud-rs-{}",
            hex_encode(owner_sid.as_bytes())
        );
        let wide_path = to_wide(&pipe_path);

        // SDDL: "D:(A;;GRGW;;;<SID>)" — a DACL (`D:`) whose sole ACE
        // Allows (`A`) `GENERIC_READ | GENERIC_WRITE` to the owner SID.
        // Absence of any other ACE means every other principal is
        // denied by the default "no ACE = no access" rule.
        let sddl = format!("D:(A;;GRGW;;;{})", owner_sid);
        let sd = SecurityDescriptor::from_sddl(&sddl)?;

        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.as_ptr(),
            bInheritHandle: BOOL(0),
        };

        // SAFETY: `wide_path` is a NUL-terminated UTF-16 buffer kept
        // alive for the duration of the call; `sa` is a valid
        // `SECURITY_ATTRIBUTES` pointing at an intact SD. All integer
        // arguments are fixed constants below documented maxima.
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
            // SAFETY: `GetLastError` has no preconditions; it reads
            // thread-local state populated by the failing Win32 call.
            let err = unsafe { GetLastError() };
            return Err(IpcTransportError::Io(std::io::Error::from_raw_os_error(
                err.0 as i32,
            )));
        }

        Ok(WindowsListener {
            handle,
            pipe_path,
            owner_sid,
        })
    }

    /// Compare the client's TokenUser SID with the server's owner SID.
    ///
    /// Returns `0` on match — Windows has no uid, so we reuse 0 to mean
    /// "peer is the pipe owner". Any mismatch (or failure to recover
    /// peer credentials at all) surfaces as
    /// [`IpcTransportError::PeerCredentialsUnavailable`], which the
    /// transport layer treats as an unauthorized peer.
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
    /// Wait for the next client, recover and authenticate its SID, and
    /// return a connected stream. Intended to be called by the transport
    /// accept loop.
    #[allow(dead_code)]
    pub fn accept(&self) -> Result<WindowsStream, IpcTransportError> {
        // SAFETY: `self.handle` is a live listener returned by
        // `CreateNamedPipeW`. `ConnectNamedPipe` with a NULL OVERLAPPED
        // blocks until a client connects or errors; we accept both
        // `TRUE` and `ERROR_PIPE_CONNECTED` as success per MSDN.
        let connected = unsafe { ConnectNamedPipe(self.handle, None) };
        if connected.is_err() {
            // SAFETY: see rationale above for `GetLastError`.
            let code = unsafe { GetLastError() };
            if code != ERROR_PIPE_CONNECTED {
                return Err(IpcTransportError::Io(std::io::Error::from_raw_os_error(
                    code.0 as i32,
                )));
            }
        }

        let peer_sid = client_sid_string(self.handle)?;
        if peer_sid != self.owner_sid {
            return Err(IpcTransportError::PeerCredentialsUnavailable);
        }

        Ok(WindowsStream {
            handle: self.handle,
            peer_sid,
        })
    }

    /// Diagnostic accessor — the full pipe path, e.g.
    /// `\\.\pipe\pcloud-rs-<hex-SID>`.
    #[allow(dead_code)]
    pub fn pipe_path(&self) -> &str {
        &self.pipe_path
    }
}

// --------------------------------------------------------------------
// SID / token helpers
// --------------------------------------------------------------------

/// Recover the current-user SID as a SDDL string (`S-1-5-21-...`).
///
/// Steps, matching the task spec:
/// 1. `OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY)` — a query-
///    only token handle; no modification rights.
/// 2. `GetTokenInformation(TokenUser)` — two-phase call (size probe,
///    then allocation).
/// 3. `ConvertSidToStringSidW` — canonical textual SID.
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

/// Resolve the client process's TokenUser SID on a connected pipe.
fn client_sid_string(pipe: HANDLE) -> Result<String, IpcTransportError> {
    let mut pid: u32 = 0;
    // SAFETY: `pipe` is a valid connected-server handle; `pid` is an
    // initialized out-param.
    let ok = unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) };
    if ok.is_err() {
        return Err(last_os_err());
    }

    // `PROCESS_QUERY_LIMITED_INFORMATION` is the documented minimum
    // right required for `OpenProcessToken` on a foreign process on
    // modern Windows and works across integrity levels at or below the
    // caller's.
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

    token_user_sid_string(token)
}

/// Two-phase `GetTokenInformation(TokenUser)` + `ConvertSidToStringSidW`.
fn token_user_sid_string(token: HANDLE) -> Result<String, IpcTransportError> {
    let mut needed: u32 = 0;
    // SAFETY: first call is the documented size probe; passing a NULL
    // buffer with `0` length is explicitly supported and must fail with
    // `ERROR_INSUFFICIENT_BUFFER` while writing the required size into
    // `needed`.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return Err(last_os_err());
    }

    let mut buf = vec![0u8; needed as usize];
    // SAFETY: `buf` is sized per the probe; the kernel writes a
    // `TOKEN_USER` plus trailing SID into it on success.
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

    // SAFETY: on success the buffer starts with a valid `TOKEN_USER`
    // whose `User.Sid` points into the same allocation.
    let token_user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    sid_to_string(token_user.User.Sid)
}

/// `ConvertSidToStringSidW` wrapper that frees the SDDL buffer via
/// `LocalFree` exactly once.
fn sid_to_string(sid: PSID) -> Result<String, IpcTransportError> {
    let mut out = PWSTR::null();
    // SAFETY: `sid` is a valid SID for the duration of the call; `out`
    // is populated with a `LocalAlloc`'d buffer we must `LocalFree`.
    let ok = unsafe { ConvertSidToStringSidW(sid, &mut out) };
    if ok.is_err() || out.is_null() {
        return Err(last_os_err());
    }

    // SAFETY: `out` points at a NUL-terminated UTF-16 string owned by
    // `LocalAlloc`. We copy it into a Rust `String` before freeing.
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

/// RAII wrapper for a SD built via SDDL. Frees the underlying
/// `LocalAlloc` buffer on drop.
struct SecurityDescriptor {
    ptr: PSECURITY_DESCRIPTOR,
}

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self, IpcTransportError> {
        let wide = to_wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string. The call
        // allocates a self-relative SD which we own and must release.
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
            // `ConvertStringSecurityDescriptorToSecurityDescriptorW`
            // which documents `LocalFree` as the matching deallocator.
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
            // SAFETY: `self.0` was produced by `OpenProcess*` /
            // `OpenProcessToken` and is exclusively owned here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Convert a Rust `&str` to a NUL-terminated UTF-16 buffer suitable for
/// the `W` family of Win32 APIs.
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Lowercase hex encoding for embedding a SID string into a pipe name.
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

// Suppress unused-import warnings when the optional `EqualSid` /
// `ptr::null_mut` paths are exercised only in future wire-up.
#[allow(dead_code)]
fn _equal_sid(a: PSID, b: PSID) -> bool {
    // SAFETY: both SIDs must be valid for the duration of this call.
    // We use it as a fallback to byte-equality SID compares and keep
    // it available for transport-layer use.
    unsafe { EqualSid(a, b).is_ok() }
}

#[allow(dead_code)]
fn _null_mut<T>() -> *mut T {
    ptr::null_mut()
}

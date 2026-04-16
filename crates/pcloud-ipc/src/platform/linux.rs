//! **PLATFORM: Linux only.** Peer authentication via `getsockopt(SO_PEERCRED)`.
//! **GATING: `#[cfg(target_os = "linux")]`.**
//!
//! Linux-specific because SO_PEERCRED is a Linux kernel feature.
//! BSD and macOS use `getpeereid(3)` instead — see `platform::unix`.

use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use crate::platform::PlatformIpc;
use crate::transport::IpcTransportError;

/// Linux backend for [`PlatformIpc`]. Uses AF_UNIX datagram-oriented
/// stream sockets and `SO_PEERCRED` for peer authentication.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxIpc;

impl PlatformIpc for LinuxIpc {
    type Listener = UnixListener;
    type Stream = UnixStream;

    fn bind_listener(&self, runtime_dir: &Path) -> Result<Self::Listener, IpcTransportError> {
        // The actual bind sequence (permission setup, cleanup of stale
        // sockets) lives in `transport::IpcServer::bind`; this helper is
        // provided for symmetry with the trait and for future callers.
        let listener = UnixListener::bind(runtime_dir)?;
        Ok(listener)
    }

    fn peer_uid(&self, stream: &Self::Stream) -> Result<u32, IpcTransportError> {
        let fd = stream.as_raw_fd();
        let mut peer = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

        // SAFETY: fd is a live UnixStream descriptor, peer points to valid writable memory,
        // and len is initialized to the correct structure size for SO_PEERCRED.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut peer as *mut libc::ucred as *mut libc::c_void,
                &mut len,
            )
        };

        if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
            return Err(IpcTransportError::PeerCredentialsUnavailable);
        }

        Ok(peer.uid)
    }

    fn peer_display(&self, stream: &Self::Stream) -> Result<String, IpcTransportError> {
        let fd = stream.as_raw_fd();
        let mut peer = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

        // SAFETY: see peer_uid; same preconditions.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut peer as *mut libc::ucred as *mut libc::c_void,
                &mut len,
            )
        };

        if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
            return Err(IpcTransportError::PeerCredentialsUnavailable);
        }

        Ok(format!("uid={}, pid={}", peer.uid, peer.pid))
    }

    fn backend_name(&self) -> &'static str {
        "linux-so-peercred"
    }
}

/// Recover both uid and pid from a connected `UnixStream`. Used by
/// `transport.rs` to populate [`crate::auth::PeerIdentity`] which carries
/// pid for audit correlation.
pub(crate) fn peer_ucred(stream: &UnixStream) -> Result<(u32, u32), IpcTransportError> {
    let fd = stream.as_raw_fd();
    let mut peer = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

    // SAFETY: fd is a live UnixStream descriptor, peer points to valid writable memory,
    // and len is initialized to the correct structure size for SO_PEERCRED.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut peer as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };

    if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return Err(IpcTransportError::PeerCredentialsUnavailable);
    }

    Ok((peer.uid, peer.pid as u32))
}

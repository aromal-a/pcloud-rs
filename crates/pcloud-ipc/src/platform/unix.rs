//! **PLATFORM: FreeBSD, OpenBSD, NetBSD, macOS.**
//! **GATING: `#[cfg(any(target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "macos"))]`.**
//!
//! BSD/macOS peer authentication via `getpeereid(3)` on AF_UNIX.
//! NOT suitable for Linux (no `getpeereid`; use SO_PEERCRED — see `platform::linux`).

use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use crate::platform::PlatformIpc;
use crate::transport::IpcTransportError;

/// BSD/macOS backend for [`PlatformIpc`]. Uses AF_UNIX stream sockets and
/// `getpeereid(3)` for peer authentication. Unlike SO_PEERCRED this does
/// not return the peer pid, so `peer_display` cannot include it.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnixIpc;

impl PlatformIpc for UnixIpc {
    type Listener = UnixListener;
    type Stream = UnixStream;

    fn bind_listener(&self, runtime_dir: &Path) -> Result<Self::Listener, IpcTransportError> {
        let listener = UnixListener::bind(runtime_dir)?;
        Ok(listener)
    }

    fn peer_uid(&self, stream: &Self::Stream) -> Result<u32, IpcTransportError> {
        let (uid, _gid) = getpeereid(stream)?;
        Ok(uid)
    }

    fn peer_display(&self, stream: &Self::Stream) -> Result<String, IpcTransportError> {
        let (uid, gid) = getpeereid(stream)?;
        Ok(format!("uid={}, gid={}", uid, gid))
    }

    fn backend_name(&self) -> &'static str {
        "unix-getpeereid"
    }
}

fn getpeereid(stream: &UnixStream) -> Result<(u32, u32), IpcTransportError> {
    let fd = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;

    // SAFETY: fd is a live AF_UNIX stream descriptor; `uid` and `gid`
    // are valid writable `uid_t`/`gid_t` locations. `getpeereid(3)` only
    // writes to them when it returns zero, and reads nothing through
    // these pointers otherwise.
    let rc = unsafe { libc::getpeereid(fd, &mut uid as *mut _, &mut gid as *mut _) };

    if rc != 0 {
        return Err(IpcTransportError::PeerCredentialsUnavailable);
    }

    Ok((uid as u32, gid as u32))
}

/// Recover the peer uid (and a synthetic pid of 0, since `getpeereid`
/// does not expose it) from a connected `UnixStream`. Used by
/// `transport.rs` to populate [`crate::auth::PeerIdentity`].
pub(crate) fn peer_ucred(stream: &UnixStream) -> Result<(u32, u32), IpcTransportError> {
    let (uid, _gid) = getpeereid(stream)?;
    Ok((uid, 0))
}

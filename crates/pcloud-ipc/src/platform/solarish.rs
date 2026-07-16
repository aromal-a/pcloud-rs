//! **PLATFORM: illumos and Solaris.**
//! **GATING: `#[cfg(any(target_os = "illumos", target_os = "solaris"))]`.**
//!
//! Peer authentication for AF_UNIX streams through `getpeerucred(3)`.
//! Unlike BSD `getpeereid(3)`, the Solaris credential object exposes both
//! the peer effective UID and PID. The object is owned by the caller and
//! must be released with `ucred_free(3)`.

use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use crate::platform::PlatformIpc;
use crate::transport::IpcTransportError;

/// illumos/Solaris AF_UNIX backend using `getpeerucred(3)`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SolarishIpc;

impl PlatformIpc for SolarishIpc {
    type Listener = UnixListener;
    type Stream = UnixStream;

    fn bind_listener(&self, runtime_dir: &Path) -> Result<Self::Listener, IpcTransportError> {
        Ok(UnixListener::bind(runtime_dir)?)
    }

    fn peer_uid(&self, stream: &Self::Stream) -> Result<u32, IpcTransportError> {
        peer_ucred(stream).map(|(uid, _pid)| uid)
    }

    fn peer_display(&self, stream: &Self::Stream) -> Result<String, IpcTransportError> {
        let (uid, pid) = peer_ucred(stream)?;
        Ok(format!("uid={uid}, pid={pid}"))
    }

    fn backend_name(&self) -> &'static str {
        "solarish-getpeerucred"
    }
}

/// Recover the peer effective UID and PID from a connected AF_UNIX stream.
pub(crate) fn peer_ucred(stream: &UnixStream) -> Result<(u32, u32), IpcTransportError> {
    let mut credential: *mut libc::ucred_t = std::ptr::null_mut();

    // SAFETY: `stream` owns a live connected AF_UNIX descriptor and
    // `credential` is a valid out-pointer. On success libc transfers one
    // credential-object reference to this function.
    let rc = unsafe { libc::getpeerucred(stream.as_raw_fd(), &mut credential) };
    if rc != 0 || credential.is_null() {
        if !credential.is_null() {
            // SAFETY: a non-null value written by getpeerucred is owned by
            // the caller even when a defensive error path is taken.
            unsafe { libc::ucred_free(credential) };
        }
        return Err(IpcTransportError::PeerCredentialsUnavailable);
    }

    // SAFETY: `credential` is non-null and remains live until ucred_free.
    let uid = unsafe { libc::ucred_geteuid(credential) };
    // SAFETY: same credential-object lifetime as above.
    let pid = unsafe { libc::ucred_getpid(credential) };
    // SAFETY: release exactly the reference returned by getpeerucred.
    unsafe { libc::ucred_free(credential) };

    if pid < 0 {
        return Err(IpcTransportError::PeerCredentialsUnavailable);
    }

    Ok((uid as u32, pid as u32))
}

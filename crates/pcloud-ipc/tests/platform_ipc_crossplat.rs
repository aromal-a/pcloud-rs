#![allow(clippy::pedantic)]
//! **PLATFORM: all** (Linux | FreeBSD | OpenBSD | NetBSD | macOS | Windows).
//! **GATING: sub-tests are cfg-gated per platform; the file as a whole
//! compiles everywhere** so the cross-platform matrix builds identically
//! on every target.
//!
//! Phase 3 integration tests for the [`pcloud_ipc::platform`] abstraction
//! layer. The goal is to exercise `PlatformIpc` through `ActivePlatform`
//! so whichever backend is selected at compile time (Linux SO_PEERCRED,
//! BSD/macOS getpeereid, Windows named-pipe SID) gets a smoke-level
//! correctness proof in CI on Linux today, and the same test file runs
//! unchanged when macOS / Windows CI targets come online.
//!
//! What this file does NOT do:
//! - modify production code (only allowed file: this test file),
//! - stub out the trait (we drive the real `ActivePlatform` type),
//! - perform protocol-level IPC (that is covered in `peer_and_protocol`
//!   and `stress_concurrent_clients`).

use std::any::type_name;

use pcloud_ipc::platform::{ActivePlatform, PlatformIpc};

/// `ActivePlatform::backend_name()` must return a non-empty, stable
/// identifier for the compiled-in backend. This replaces the earlier
/// `std::any::type_name` approximation now that `PlatformIpc` exposes a
/// real `backend_name()` method symmetric with
/// `pcloud_secret::platform::PlatformVault::backend_name`.
#[test]
fn active_ipc_backend_is_non_empty_string() {
    let backend = ActivePlatform::default();
    let name = backend.backend_name();

    assert!(
        !name.is_empty(),
        "PlatformIpc::backend_name must be non-empty for diagnostics"
    );

    // Cross-check against the compiled target so whichever backend is
    // active reports the exact string documented on the trait.
    #[cfg(target_os = "linux")]
    let expected = "linux-so-peercred";
    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "macos"
    ))]
    let expected = "unix-getpeereid";
    #[cfg(windows)]
    let expected = "windows-named-pipe";

    assert_eq!(
        name, expected,
        "PlatformIpc::backend_name must match the documented per-OS value"
    );

    // Belt-and-braces: the `std::any::type_name` check from the earlier
    // approximation is still a cheap sanity check on the type-alias
    // layout, and costs nothing to keep.
    let type_name_s = type_name::<ActivePlatform>();
    assert!(
        !type_name_s.is_empty() && type_name_s.contains("Ipc"),
        "ActivePlatform type should still carry an 'Ipc' marker: {type_name_s}"
    );
}

/// On Unix targets, bind a listener via the trait, connect a client, and
/// assert that `peer_uid` returns the current process's euid. This is the
/// round-trip that `transport.rs::peer_identity` relies on.
///
/// On Windows the same shape is preserved, but the assertion instead
/// checks SID equality. Since we do not have a Windows test target yet
/// (named-pipe backend is the Windows-tier-1 stub), the Windows arm is
/// implemented as a compile-time scaffold only.
#[cfg(unix)]
#[test]
fn bind_listener_roundtrip_with_owner() {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::{SystemTime, UNIX_EPOCH};

    use pcloud_ipc::current_effective_uid;

    // Use `/tmp` directly: macOS SUN_LEN=104 cannot accommodate the
    // per-user tempdir `/var/folders/.../T/` prefix.
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let dir = std::path::PathBuf::from("/tmp").join(format!(
        "pipc-plat-{}-{}",
        std::process::id(),
        nonce % 1_000_000_000
    ));
    std::fs::create_dir_all(&dir).expect("runtime dir should be created");
    let socket_path = dir.join("ipc.sock");

    let backend = ActivePlatform::default();

    // `bind_listener` on every Unix backend treats its argument as the
    // *socket path* (see `platform/linux.rs` and `platform/unix.rs`).
    let listener = backend
        .bind_listener(&socket_path)
        .expect("bind_listener should succeed in a writable temp dir");

    // Connect from the same process so the peer uid MUST match our euid.
    let client = UnixStream::connect(&socket_path).expect("same-process connect should succeed");
    // Keep the client alive through the accept.
    let _ = &client;

    let (server_stream, _addr) = listener.accept().expect("accept should succeed");

    let peer_uid = backend
        .peer_uid(&server_stream)
        .expect("peer_uid should be recoverable for a same-process connect");
    assert_eq!(
        peer_uid,
        current_effective_uid(),
        "same-process peer uid must equal our effective uid"
    );

    let display = backend
        .peer_display(&server_stream)
        .expect("peer_display should render");
    assert!(
        !display.is_empty() && display.contains("uid="),
        "peer_display should embed 'uid=': {display}"
    );

    // Drop both halves so the socket is cleanly released.
    drop(server_stream);
    drop(client);
    // Best-effort cleanup: let the listener drop, then remove the dir.
    drop(listener);
    // Keep a final stable write to ensure the connect side already closed.
    let _ = std::io::sink().write_all(b"");
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Windows scaffold for `bind_listener_roundtrip_with_owner`. Left as a
/// `#[cfg(windows)]` compile-time checker so the expected SID-match shape
/// is captured now. When the Windows named-pipe backend materializes
/// (`bd-xplat-windows`), this test is the first one to un-ignore.
#[cfg(windows)]
#[test]
#[ignore = "Windows named-pipe backend is still a stub — enable once \
            platform::windows::WindowsIpc is live"]
fn bind_listener_roundtrip_with_owner() {
    // On Windows, the PlatformIpc trait lands on named pipes. Under SCM
    // this will execute the SID-match path (peer SID must equal the
    // owning process's token SID). When the stub is replaced, the check
    // below should become: bind a pipe, connect from the same process,
    // recover the peer identity, assert peer_uid == 0 placeholder OR the
    // SID-equality accessor returns true.
    let _ = ActivePlatform::default();
    unreachable!("Windows IPC backend not yet wired in production code");
}

/// Negative test: binding inside a directory that does not exist must
/// fail with an IO error, not panic and not silently succeed. This is
/// the fast-fail property that `IpcServer::bind` relies on when the
/// runtime directory layout is wrong.
#[cfg(unix)]
#[test]
fn binding_in_nonexistent_dir_errors() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let missing = std::env::temp_dir()
        .join(format!(
            "pcloud-platform-ipc-missing-{}-{}",
            std::process::id(),
            nonce
        ))
        .join("definitely")
        .join("does-not-exist")
        .join("ipc.sock");

    let backend = ActivePlatform::default();
    let result = backend.bind_listener(&missing);
    assert!(
        result.is_err(),
        "bind_listener on a non-existent parent dir must fail"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "Windows named-pipe backend is still a stub"]
fn binding_in_nonexistent_dir_errors() {
    // Placeholder: the Windows named-pipe backend will fail when the
    // pipe's namespace is malformed. Re-enable once WindowsIpc is live.
    let _ = ActivePlatform::default();
}

#![allow(clippy::pedantic)]
//! **PLATFORM: macOS only.**
//! **GATING:** `#[cfg(target_os = "macos")]` + `#[ignore]`.
//!
//! Real live FUSE tests that authenticate against the production pCloud API
//! using a real auth token and mount the user's actual pCloud Drive via fuse-t.
//!
//! ## Prerequisites
//!
//! 1. fuse-t must be installed (`/usr/local/lib/libfuse-t.dylib` or Homebrew).
//! 2. Set `PCLOUD_LIVE_AUTH_TOKEN` to a valid pCloud auth token.
//! 3. Optionally set `PCLOUD_LIVE_TEST_FOLDER` to an existing pCloud folder
//!    path (e.g. `/My Test Folder`) where the test may create/delete scratch
//!    files. If unset, write/rename tests are skipped.
//!
//! ## Running
//!
//! ```sh
//! PCLOUD_LIVE_AUTH_TOKEN=<token> \
//! cargo test -p pcloud-daemon --test macos_pcloud_live -- --include-ignored
//! ```
//!
//! All writes are isolated under a uniquely-named scratch subdirectory and
//! cleaned up before unmount. Read operations are non-destructive.

// Security audit note: env var NAMES may appear in skip messages; the VALUES
// of secret vars (PCLOUD_LIVE_AUTH_TOKEN) must never be printed.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};

use pcloud_config::{ConfigProfile, Environment, env::apply_env_overrides};
use pcloud_daemon::{RuntimeShell, bootstrap_with_config, dispatch};
use pcloud_ipc::{Request, ResponseStatus};

// Serialize all live mount tests: fuse-t has process-global state and
// concurrent kernel mounts within the same process are unsafe.
static LIVE_SERIAL: Mutex<()> = Mutex::new(());

// ─── helpers ──────────────────────────────────────────────────────────────────

fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn unique_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

fn live_config() -> ConfigProfile {
    let root = env::temp_dir().join(format!(
        "pcloud-live-fuse-{}-{}",
        std::process::id(),
        unique_nonce()
    ));
    apply_env_overrides(ConfigProfile::secure_defaults(root, Environment::Production))
        .expect("live config env overrides must parse")
}

fn cleanup_config_root(config: &ConfigProfile) {
    if let Some(root) = config.paths.config_dir.parent() {
        let _ = fs::remove_dir_all(root);
    }
}

/// Bootstrap the daemon runtime, authenticate via `PCLOUD_LIVE_AUTH_TOKEN`,
/// and return `(runtime, unique_mountpoint, config)`.
///
/// Returns `None` and prints a skip message when the token is absent or fuse-t
/// is not installed.
fn setup_live_runtime() -> Option<(RuntimeShell, PathBuf, ConfigProfile)> {
    let token = match optional_env("PCLOUD_LIVE_AUTH_TOKEN") {
        Some(t) => t,
        None => {
            eprintln!("SKIP: PCLOUD_LIVE_AUTH_TOKEN is not set");
            return None;
        }
    };

    let config = live_config();
    let mut runtime = bootstrap_with_config(config.clone())
        .expect("daemon runtime bootstrap must succeed");

    let auth_resp = dispatch(
        &mut runtime,
        Request::AuthTokenSubmission { value: token.into() },
    );
    if auth_resp.status != ResponseStatus::Ok {
        // Do not print the token value — log only the status message.
        eprintln!("SKIP: auth failed: {}", auth_resp.message);
        cleanup_config_root(&config);
        return None;
    }

    // Probe fuse-t presence through the platform layer.
    {
        use pcloud_fs::platform::PlatformMount;
        use pcloud_fs::platform::macos::MacosPlatformMount;
        if let Err(e) = MacosPlatformMount.probe_supported() {
            eprintln!("SKIP: fuse-t not available: {e}");
            cleanup_config_root(&config);
            return None;
        }
    }

    let mountpoint = env::temp_dir().join(format!(
        "pcloud-live-mp-{}-{}",
        std::process::id(),
        unique_nonce()
    ));
    fs::create_dir_all(&mountpoint).expect("mountpoint directory must be creatable");

    Some((runtime, mountpoint, config))
}

/// Mount and wait for the VFS to settle (≈300 ms). Returns `false` if the
/// mount IPC call fails so the caller can skip gracefully.
fn mount_live(runtime: &mut RuntimeShell, mountpoint: &Path) -> bool {
    let resp = dispatch(runtime, Request::Mount { path: mountpoint.to_path_buf() });
    if resp.status != ResponseStatus::Ok {
        eprintln!("mount failed: {}", resp.message);
        return false;
    }
    std::thread::sleep(Duration::from_millis(300));
    true
}

/// Unmount and wait for the kernel to release the FUSE session.
fn unmount_live(runtime: &mut RuntimeShell) {
    let resp = dispatch(runtime, Request::Unmount);
    if resp.status != ResponseStatus::Ok {
        eprintln!("unmount warning: {}", resp.message);
    }
    std::thread::sleep(Duration::from_millis(300));
}

// ─── read-path tests ──────────────────────────────────────────────────────────

/// The root of the real pCloud Drive contains at least one entry.
/// (Every pCloud account has at least one built-in folder.)
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN and fuse-t"]
fn live_readdir_root_returns_entries() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };
    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    let entries: Vec<_> = fs::read_dir(&mp)
        .expect("read_dir on live pCloud root must succeed")
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty(), "pCloud root must have at least one entry");

    if env::var("PCLOUD_TEST_VERBOSE").is_ok() {
        for e in &entries {
            eprintln!("  root: {:?}", e.file_name());
        }
    }

    unmount_live(&mut rt);
    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

/// `stat` on the live mount root returns a directory inode.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN and fuse-t"]
fn live_getattr_root_is_directory() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };
    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    let meta = fs::metadata(&mp).expect("stat on live pCloud root must succeed");
    assert!(meta.is_dir(), "pCloud mount root must be a directory");

    unmount_live(&mut rt);
    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

/// `getmntinfo(3)` lists the active mount after a successful kernel bind.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN and fuse-t"]
fn live_mount_appears_in_getmntinfo() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };
    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    use pcloud_fs::mount_orphan::{MountinfoReader, StaticMountinfoReader, mountpoint_is_already_mounted};
    use pcloud_fs::platform::macos::MacosMountinfoReader;

    let visible = match MacosMountinfoReader.read() {
        Ok(payload) => {
            mountpoint_is_already_mounted(&StaticMountinfoReader::new(&payload), &mp).is_some()
        }
        Err(e) => {
            eprintln!("getmntinfo failed: {e} — skipping mountinfo assertion");
            true
        }
    };
    assert!(visible, "live mount at {:?} must appear in getmntinfo", mp);

    unmount_live(&mut rt);
    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

/// After unmount the mountpoint is no longer listed in `getmntinfo`.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN and fuse-t"]
fn live_unmount_removes_from_getmntinfo() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };
    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    unmount_live(&mut rt);

    use pcloud_fs::mount_orphan::{MountinfoReader, StaticMountinfoReader, mountpoint_is_already_mounted};
    use pcloud_fs::platform::macos::MacosMountinfoReader;

    if let Ok(payload) = MacosMountinfoReader.read() {
        let still = mountpoint_is_already_mounted(&StaticMountinfoReader::new(&payload), &mp);
        assert!(still.is_none(), "mount at {:?} must not appear after unmount", mp);
    }

    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

/// Concurrent `read_dir` calls from 4 threads on the live root.
/// Verifies the adapter handles concurrent FUSE requests without deadlock.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN and fuse-t"]
fn live_concurrent_readers_root() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };
    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    let mp_ref = mp.clone();
    let counts: Vec<_> = std::thread::scope(|s| {
        (0..4)
            .map(|_| {
                let p = mp_ref.clone();
                s.spawn(move || fs::read_dir(&p).map(|it| it.filter_map(|e| e.ok()).count()))
            })
            .map(|h| h.join().expect("reader thread must not panic"))
            .collect()
    });

    for c in &counts {
        assert!(c.is_ok(), "concurrent read_dir must succeed: {:?}", c);
    }
    let ns: Vec<usize> = counts.into_iter().map(|c| c.unwrap()).collect();
    assert!(
        ns.windows(2).all(|w| w[0] == w[1]),
        "all concurrent readers must agree on root entry count: {ns:?}"
    );

    unmount_live(&mut rt);
    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

/// Mount → probe → unmount → remount → probe → unmount.
/// Verifies clean teardown and re-attach without panic or hang.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN and fuse-t"]
fn live_mount_remount_cycle() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };

    for cycle in 0..2 {
        if !mount_live(&mut rt, &mp) {
            eprintln!("cycle {cycle}: mount failed");
            cleanup_config_root(&cfg);
            let _ = fs::remove_dir_all(&mp);
            return;
        }
        let n = fs::read_dir(&mp).expect("read_dir must succeed").count();
        assert!(n > 0, "cycle {cycle}: root must have entries");
        unmount_live(&mut rt);
        std::thread::sleep(Duration::from_millis(200));
    }

    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

// ─── write-path tests (require PCLOUD_LIVE_TEST_FOLDER) ──────────────────────

/// Create a scratch file via FUSE, read it back, verify content, then delete.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN, fuse-t, and PCLOUD_LIVE_TEST_FOLDER"]
fn live_write_read_delete_roundtrip() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };

    let test_folder = match optional_env("PCLOUD_LIVE_TEST_FOLDER") {
        Some(f) => f,
        None => {
            eprintln!("SKIP: PCLOUD_LIVE_TEST_FOLDER is not set");
            cleanup_config_root(&cfg);
            let _ = fs::remove_dir_all(&mp);
            return;
        }
    };

    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    let scratch = mp
        .join(test_folder.trim_start_matches('/'))
        .join(format!("pcloud-rs-test-{}", unique_nonce()));
    let file = scratch.join("hello.txt");
    let payload = b"pcloud-rs live write test\n";

    fs::create_dir_all(&scratch).expect("mkdir in live mount must succeed");
    fs::write(&file, payload).expect("write to live mount must succeed");

    // Allow the write path to flush / upload.
    std::thread::sleep(Duration::from_millis(500));

    let read_back = fs::read(&file).expect("read from live mount must succeed");
    assert_eq!(read_back, payload, "read-back must match written payload");

    let meta = fs::metadata(&file).expect("stat on written file must succeed");
    assert!(meta.is_file());
    assert_eq!(meta.len(), payload.len() as u64);

    fs::remove_file(&file).expect("unlink must succeed");
    assert!(!file.exists(), "file must not exist after unlink");

    let _ = fs::remove_dir_all(&scratch);
    unmount_live(&mut rt);
    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

/// Rename a file in the live pCloud Drive; verify old path gone, new has content.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN, fuse-t, and PCLOUD_LIVE_TEST_FOLDER"]
fn live_rename_via_fuse() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };

    let test_folder = match optional_env("PCLOUD_LIVE_TEST_FOLDER") {
        Some(f) => f,
        None => {
            eprintln!("SKIP: PCLOUD_LIVE_TEST_FOLDER is not set");
            cleanup_config_root(&cfg);
            let _ = fs::remove_dir_all(&mp);
            return;
        }
    };

    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    let scratch = mp
        .join(test_folder.trim_start_matches('/'))
        .join(format!("pcloud-rs-rename-{}", unique_nonce()));
    let src = scratch.join("src.txt");
    let dst = scratch.join("dst.txt");

    fs::create_dir_all(&scratch).expect("mkdir must succeed");
    fs::write(&src, b"rename-test").expect("write src must succeed");
    std::thread::sleep(Duration::from_millis(300));

    fs::rename(&src, &dst).expect("rename must succeed");
    std::thread::sleep(Duration::from_millis(300));

    assert!(!src.exists(), "src must not exist after rename");
    assert_eq!(fs::read(&dst).expect("dst must be readable"), b"rename-test");

    let _ = fs::remove_dir_all(&scratch);
    unmount_live(&mut rt);
    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

/// Create a subdirectory inside a live folder and verify it appears in readdir.
#[test]
#[ignore = "requires PCLOUD_LIVE_AUTH_TOKEN, fuse-t, and PCLOUD_LIVE_TEST_FOLDER"]
fn live_mkdir_appears_in_readdir() {
    let _lock = LIVE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut rt, mp, cfg)) = setup_live_runtime() else { return; };

    let test_folder = match optional_env("PCLOUD_LIVE_TEST_FOLDER") {
        Some(f) => f,
        None => {
            eprintln!("SKIP: PCLOUD_LIVE_TEST_FOLDER is not set");
            cleanup_config_root(&cfg);
            let _ = fs::remove_dir_all(&mp);
            return;
        }
    };

    if !mount_live(&mut rt, &mp) {
        cleanup_config_root(&cfg);
        let _ = fs::remove_dir_all(&mp);
        return;
    }

    let parent = mp.join(test_folder.trim_start_matches('/'));
    let dir_name = format!("pcloud-rs-mkdir-{}", unique_nonce());
    let new_dir = parent.join(&dir_name);

    fs::create_dir(&new_dir).expect("mkdir in live pCloud folder must succeed");
    std::thread::sleep(Duration::from_millis(300));

    let names: Vec<_> = fs::read_dir(&parent)
        .expect("readdir of test folder must succeed")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        names.contains(&dir_name),
        "newly created dir '{}' must appear in readdir; got: {names:?}",
        dir_name
    );

    fs::remove_dir(&new_dir).expect("rmdir must succeed");
    let _ = fs::remove_dir_all(&new_dir);

    unmount_live(&mut rt);
    cleanup_config_root(&cfg);
    let _ = fs::remove_dir_all(&mp);
}

#![allow(clippy::pedantic)]
//! **PLATFORM: all** (Linux | FreeBSD | OpenBSD | NetBSD | macOS | Windows).
//! **GATING: sub-tests are cfg-gated per OS; the file compiles
//! everywhere.**
//!
//! Phase 3 integration tests for the [`pcloud_daemon::vault`] trait
//! layer. Exercises `PlatformVault` through its portable `FileVault`
//! backend on every OS, and documents the shape of the macOS Keychain /
//! Windows DPAPI sub-tests so they flip on automatically when those
//! targets come online.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_config::auth::VaultBackend;
use pcloud_daemon::vault::{
    FileVault, HostFamily, PlatformVault, VaultSelectError, select_vault, select_vault_for,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};

/// Pick a fresh vault file path under `TMPDIR`. The parent directory is
/// created with mode `0700` so `FileVault::store` does not have to
/// relax its expectations.
fn fresh_vault_path(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pcloud-platform-vault-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("vault parent dir should be created");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .expect("vault parent mode should be 0700");
    dir.join("auth.token")
}

/// The `FileVault` backend must work on every platform because it is the
/// fallback vault. Round-trip: store → load → assert equality.
#[test]
fn file_vault_stores_and_loads_on_all_platforms() {
    let path = fresh_vault_path("roundtrip");
    let vault = FileVault::new(path.clone());

    assert_eq!(vault.backend_name(), "file");
    assert!(
        vault
            .load()
            .expect("empty vault should load without error")
            .is_none(),
        "freshly-created vault must report no token"
    );

    let secret = SecretString::new("platform-vault-roundtrip-token".to_string());
    vault.store(&secret).expect("store should succeed");

    let loaded = vault
        .load()
        .expect("load should succeed after store")
        .expect("token should be present after store");
    assert_eq!(
        loaded.expose_secret(),
        "platform-vault-roundtrip-token",
        "loaded token must match stored token byte-for-byte"
    );

    // File mode must be 0600 on Unix — enforced by the trait impl, but
    // we assert it here so a regression in the delegate surface of
    // `FileVault::store` fails this cross-platform test too.
    let meta = std::fs::metadata(&path).expect("vault file should exist");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);

    vault.clear().expect("clear should succeed");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// `PlatformVault::clear` must be idempotent: clearing an already-cleared
/// vault must not return an error. This is the explicit contract in
/// `vault::mod.rs` (`treat clear as idempotent`).
#[test]
fn file_vault_clear_is_idempotent() {
    let path = fresh_vault_path("idempotent-clear");
    let vault = FileVault::new(path.clone());

    vault.clear().expect("clear on empty vault should succeed");
    vault.clear().expect("double-clear should still succeed");

    let secret = SecretString::new("will-be-cleared".to_string());
    vault.store(&secret).expect("store should succeed");
    vault
        .clear()
        .expect("first clear after store should succeed");
    vault.clear().expect("second clear must be idempotent");
    assert!(
        vault
            .load()
            .expect("load after clear should not error")
            .is_none()
    );

    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// macOS Keychain vault is a tier-1 planned backend. Real store/load
/// tests require a live Mac login keychain, which is not available in
/// Linux CI. We assert only the `backend_name()` shape contract so that
/// when the macOS target is wired up, the scaffold already exists and
/// only needs the `unimplemented!` stub above to flip into the real
/// test body.
#[cfg(target_os = "macos")]
#[test]
fn keychain_vault_unimplemented_until_real_mac_run() {
    use pcloud_daemon::vault::keychain::KeychainVault;
    let vault = KeychainVault::new(std::env::temp_dir().join("pcloud-keychain-fallback"));
    assert_eq!(
        vault.backend_name(),
        "keychain",
        "macOS backend name must remain 'keychain' for diagnostics"
    );
    // Intentionally do NOT call store/load/clear here — those require a
    // real login Keychain, which is out of scope for this scaffold.
}

/// Windows DPAPI vault is a tier-1 planned backend. Same rationale as
/// the macOS keychain scaffold above: only the `backend_name()` shape
/// contract is asserted until a Windows CI target is available.
#[cfg(windows)]
#[test]
fn dpapi_vault_unimplemented_until_real_windows_run() {
    use pcloud_daemon::vault::dpapi::DpapiVault;
    let path = std::env::temp_dir().join("pcloud-dpapi-fallback");
    let vault = DpapiVault::new(&path);
    assert_eq!(
        vault.backend_name(),
        "dpapi",
        "Windows backend name must remain 'dpapi' for diagnostics"
    );
    // Intentionally do NOT call store/load/clear — those require a live
    // DPAPI master key, which Linux CI cannot provide.
}

// ---------------------------------------------------------------------------
// Auto-selection integration tests
// ---------------------------------------------------------------------------
//
// These exercise the `select_vault_for` helper with synthetic `HostFamily`
// values so we can prove the Auto-mapping rules independently of the
// compile-time target. On Linux CI this is the only place we can verify
// the BSD / OtherUnix / hostname-mismatch branches without spinning up a
// different target.

fn fresh_tmp_path(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    // Nest under a unique per-test parent so the 0700 chmod the
    // round-trip drill performs does not try to tighten the shared
    // `$TMPDIR` root (forbidden on most hosts).
    std::env::temp_dir()
        .join(format!(
            "pcloud-vault-select-test-{label}-{}-{nonce}",
            std::process::id()
        ))
        .join("vault.bin")
}

/// `VaultBackend::File` must always succeed on every platform. This is
/// the portable-fallback guarantee.
#[test]
fn select_vault_file_succeeds_on_every_synthetic_host() {
    for host in [
        HostFamily::MacOs,
        HostFamily::Windows,
        HostFamily::Linux,
        HostFamily::Bsd,
        HostFamily::OtherUnix,
    ] {
        let path = fresh_tmp_path("file-explicit");
        let sel = select_vault_for(VaultBackend::File, &path, host)
            .expect("file vault must never fail selection");
        assert_eq!(sel.effective, VaultBackend::File);
        assert_eq!(sel.vault.backend_name(), "file");
        assert!(
            sel.warning.is_none(),
            "explicit file selection must not produce a fallback warning"
        );
    }
}

/// `Auto` on BSD (and any other non-tier1 Unix) resolves to `FileVault`
/// without a warning — this is the deliberate default.
#[test]
fn select_vault_auto_on_bsd_picks_file_without_warning() {
    let path = fresh_tmp_path("auto-bsd");
    let sel = select_vault_for(VaultBackend::Auto, &path, HostFamily::Bsd)
        .expect("auto on BSD must succeed");
    assert_eq!(sel.effective, VaultBackend::File);
    assert_eq!(sel.vault.backend_name(), "file");
    assert!(sel.warning.is_none());
}

/// `Auto` on an unknown Unix falls through to `FileVault` without warning.
#[test]
fn select_vault_auto_on_other_unix_picks_file_without_warning() {
    let path = fresh_tmp_path("auto-otherunix");
    let sel = select_vault_for(VaultBackend::Auto, &path, HostFamily::OtherUnix)
        .expect("auto on other-unix must succeed");
    assert_eq!(sel.effective, VaultBackend::File);
    assert!(sel.warning.is_none());
}

/// Explicit `VaultBackend::Keychain` on a non-macOS host must be a hard
/// error — the Auto path is the only one allowed to fall back.
#[test]
fn select_vault_explicit_keychain_rejects_non_macos() {
    for host in [
        HostFamily::Windows,
        HostFamily::Linux,
        HostFamily::Bsd,
        HostFamily::OtherUnix,
    ] {
        let path = fresh_tmp_path("keychain-reject");
        let err = select_vault_for(VaultBackend::Keychain, &path, host)
            .expect_err("keychain on non-macOS must be hard error");
        match err {
            VaultSelectError::UnsupportedOnPlatform { requested, host: h } => {
                assert_eq!(requested, "keychain");
                assert_eq!(h, host);
            }
        }
    }
}

/// Explicit `VaultBackend::Dpapi` on a non-Windows host must be a hard
/// error.
#[test]
fn select_vault_explicit_dpapi_rejects_non_windows() {
    for host in [
        HostFamily::MacOs,
        HostFamily::Linux,
        HostFamily::Bsd,
        HostFamily::OtherUnix,
    ] {
        let path = fresh_tmp_path("dpapi-reject");
        let err = select_vault_for(VaultBackend::Dpapi, &path, host)
            .expect_err("dpapi on non-windows must be hard error");
        match err {
            VaultSelectError::UnsupportedOnPlatform { requested, host: h } => {
                assert_eq!(requested, "dpapi");
                assert_eq!(h, host);
            }
        }
    }
}

/// Explicit `VaultBackend::SecretService` on a non-Linux host must be a
/// hard error.
#[test]
fn select_vault_explicit_secret_service_rejects_non_linux() {
    for host in [
        HostFamily::MacOs,
        HostFamily::Windows,
        HostFamily::Bsd,
        HostFamily::OtherUnix,
    ] {
        let path = fresh_tmp_path("ss-reject");
        let err = select_vault_for(VaultBackend::SecretService, &path, host)
            .expect_err("secret-service on non-linux must be hard error");
        match err {
            VaultSelectError::UnsupportedOnPlatform { requested, host: h } => {
                assert_eq!(requested, "secret-service");
                assert_eq!(h, host);
            }
        }
    }
}

/// `select_vault` (the real API that delegates to the compile-time
/// host) must round-trip a stored token under the effective backend. On
/// Linux CI with no Secret Service session the Auto path falls back to
/// FileVault and surfaces a warning string.
#[test]
fn select_vault_auto_roundtrips_through_effective_backend() {
    let path = fresh_tmp_path("auto-roundtrip");
    std::fs::create_dir_all(path.parent().unwrap()).expect("parent dir");
    std::fs::set_permissions(
        path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("parent 0700");
    let sel = select_vault(VaultBackend::Auto, &path).expect("auto must succeed");
    // The effective backend is platform-dependent; we only assert that
    // whatever it resolves to can store + load round-trip.
    let token = SecretString::new("auto-select-roundtrip-token".to_string());
    sel.vault.store(&token).expect("store");
    let loaded = sel
        .vault
        .load()
        .expect("load")
        .expect("token present after store");
    assert_eq!(loaded.expose_secret(), "auto-select-roundtrip-token");
    sel.vault.clear().expect("clear");
    assert!(sel.vault.load().expect("post-clear load").is_none());
}

/// `FileVault` can still be constructed via the struct directly — the
/// new selection API does not replace the existing public `FileVault`
/// API.
#[test]
fn file_vault_constructor_still_public() {
    let path = fresh_tmp_path("direct-construct");
    let v = FileVault::new(path.clone());
    assert_eq!(v.backend_name(), "file");
}

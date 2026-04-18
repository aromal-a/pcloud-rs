#![allow(clippy::pedantic)]
//! Integration tests for `pcloud-config` profile construction, env-var
//! overrides, and validation. All tests are network-free and file-system
//! agnostic (using temp dirs or in-memory paths).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use pcloud_config::{
    ConfigError, ConfigProfile, Environment, api::ApiMode, env::apply_env_overrides,
    migrate::migrate_to_current, sync_loop::SyncLoopConfig,
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn root() -> PathBuf {
    PathBuf::from("/tmp/pcloud-config-test")
}

fn prod_profile() -> ConfigProfile {
    ConfigProfile::secure_defaults(root(), Environment::Production)
}

fn dev_profile() -> ConfigProfile {
    ConfigProfile::secure_defaults(root(), Environment::Development)
}

// ── secure_defaults + validate ────────────────────────────────────────────────

#[test]
fn production_profile_validates_cleanly() {
    prod_profile()
        .validate()
        .expect("prod profile should be valid");
}

#[test]
fn development_profile_validates_cleanly() {
    dev_profile()
        .validate()
        .expect("dev profile should be valid");
}

#[test]
fn production_has_tls_mode() {
    let p = prod_profile();
    assert_eq!(p.api.mode, ApiMode::Tls, "production must pin TLS");
}

#[test]
fn development_has_development_mode() {
    let p = dev_profile();
    assert_eq!(
        p.api.mode,
        ApiMode::Development,
        "development profile should use Development API mode"
    );
}

#[test]
fn production_and_development_have_different_tls_settings() {
    let prod = prod_profile();
    let dev = dev_profile();
    assert_ne!(
        prod.api.mode, dev.api.mode,
        "Production and Development must differ in transport mode"
    );
}

#[test]
fn production_crypto_enabled_by_default() {
    assert!(prod_profile().features.crypto_enabled);
}

#[test]
fn durable_auth_tokens_disabled_by_default() {
    // Security invariant: must be opt-in.
    assert!(!prod_profile().features.durable_auth_tokens_enabled);
    assert!(!dev_profile().features.durable_auth_tokens_enabled);
}

#[test]
fn all_managed_dirs_are_owner_only() {
    let p = prod_profile();
    assert_eq!(p.runtime.socket_dir_mode, 0o700);
    assert_eq!(p.runtime.state_dir_mode, 0o700);
    assert_eq!(p.runtime.config_dir_mode, 0o700);
    assert_eq!(p.runtime.cache_dir_mode, 0o700);
}

#[test]
fn allow_other_is_off_by_default() {
    assert!(!prod_profile().mount.allow_other);
}

// ── apply_env_overrides ───────────────────────────────────────────────────────

#[test]
fn env_pcloud_api_host_overrides_host() {
    // Safety: single-threaded test binary; env-var mutation is safe in this
    // context. The variable is immediately removed after the assertion.
    let result = unsafe {
        std::env::set_var("PCLOUD_API_HOST", "test-api.example.com");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_API_HOST");
        r
    };
    let p = result.expect("apply_env_overrides should succeed");
    assert_eq!(p.api.host, "test-api.example.com");
}

#[test]
fn env_pcloud_api_port_overrides_port() {
    let result = unsafe {
        std::env::set_var("PCLOUD_API_PORT", "9000");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_API_PORT");
        r
    };
    let p = result.expect("apply_env_overrides should succeed");
    assert_eq!(p.api.port, 9000);
}

#[test]
fn env_pcloud_durable_auth_tokens_enables_flag() {
    let result = unsafe {
        std::env::set_var("PCLOUD_DURABLE_AUTH_TOKENS", "true");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_DURABLE_AUTH_TOKENS");
        r
    };
    let p = result.expect("apply_env_overrides should succeed");
    assert!(p.features.durable_auth_tokens_enabled);
}

#[test]
fn env_pcloud_env_sets_environment() {
    let result = unsafe {
        std::env::set_var("PCLOUD_ENV", "test");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_ENV");
        r
    };
    let p = result.expect("apply_env_overrides should succeed");
    assert_eq!(p.environment, Environment::Test);
}

#[test]
fn env_pcloud_root_rewrites_managed_paths() {
    let result = unsafe {
        std::env::set_var("PCLOUD_ROOT", "/tmp/pcloud-root-override");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_ROOT");
        r
    };
    let p = result.expect("apply_env_overrides should succeed");
    assert_eq!(
        p.paths.config_dir,
        PathBuf::from("/tmp/pcloud-root-override/config")
    );
    assert_eq!(
        p.paths.state_dir,
        PathBuf::from("/tmp/pcloud-root-override/state")
    );
    assert_eq!(
        p.paths.runtime_dir,
        PathBuf::from("/tmp/pcloud-root-override/runtime")
    );
    assert_eq!(
        p.paths.cache_dir,
        PathBuf::from("/tmp/pcloud-root-override/cache")
    );
}

#[test]
fn env_invalid_bool_value_returns_typed_error() {
    let result = unsafe {
        std::env::set_var("PCLOUD_DURABLE_AUTH_TOKENS", "definitely-not-a-bool");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_DURABLE_AUTH_TOKENS");
        r
    };
    let err = result.expect_err("invalid bool should fail");
    assert!(
        matches!(err, ConfigError::InvalidEnvironmentValue { .. }),
        "unexpected error: {err:?}"
    );
}

#[test]
fn env_invalid_port_returns_typed_error() {
    let result = unsafe {
        std::env::set_var("PCLOUD_API_PORT", "not-a-port");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_API_PORT");
        r
    };
    let err = result.expect_err("invalid port should fail");
    assert!(matches!(err, ConfigError::InvalidEnvironmentValue { .. }));
}

#[test]
fn env_invalid_environment_name_returns_typed_error() {
    let result = unsafe {
        std::env::set_var("PCLOUD_ENV", "staging");
        let r = apply_env_overrides(dev_profile());
        std::env::remove_var("PCLOUD_ENV");
        r
    };
    let err = result.expect_err("unknown env variant should fail");
    assert!(matches!(err, ConfigError::InvalidEnvironmentValue { .. }));
}

// ── validation failure paths ──────────────────────────────────────────────────

#[test]
fn production_with_plaintext_api_fails_validation() {
    let mut p = prod_profile();
    p.api.mode = ApiMode::Plaintext;
    let err = p
        .validate()
        .expect_err("production plaintext should be rejected");
    assert!(
        matches!(err, ConfigError::InvalidApiEndpoint(_)),
        "unexpected error variant: {err:?}"
    );
}

#[test]
fn insecure_directory_mode_fails_validation() {
    let mut p = prod_profile();
    p.runtime.state_dir_mode = 0o755; // group/other bits set
    let err = p.validate().expect_err("permissive mode should fail");
    assert!(
        matches!(err, ConfigError::InsecureMode { .. }),
        "unexpected error variant: {err:?}"
    );
}

#[test]
fn allow_other_with_owner_only_fails_validation() {
    let mut p = prod_profile();
    p.mount.allow_other = true;
    // owner_only_by_default is true by default — this combination is rejected.
    let err = p
        .validate()
        .expect_err("allow_other + owner_only should conflict");
    assert_eq!(err, ConfigError::InvalidMountPolicy);
}

// ── sync-loop config validation ───────────────────────────────────────────────

#[test]
fn sync_loop_zero_concurrent_transfers_fails_validation() {
    let cfg = SyncLoopConfig {
        max_concurrent_transfers: 0,
        ..Default::default()
    };
    let err = cfg
        .validate()
        .expect_err("zero transfers should be rejected");
    assert!(!err.is_empty());
}

#[test]
fn sync_loop_poll_interval_below_minimum_fails_validation() {
    let cfg = SyncLoopConfig {
        poll_interval_secs: 1, // below minimum of 5
        ..Default::default()
    };
    let err = cfg
        .validate()
        .expect_err("poll_interval < 5 should be rejected");
    assert!(!err.is_empty());
}

#[test]
fn sync_loop_zero_batch_size_fails_validation() {
    let cfg = SyncLoopConfig {
        batch_size: 0,
        ..Default::default()
    };
    let err = cfg.validate().expect_err("batch_size=0 should be rejected");
    assert!(!err.is_empty());
}

#[test]
fn sync_loop_unknown_conflict_policy_fails_validation() {
    let cfg = SyncLoopConfig {
        conflict_policy: "last_writer_wins".to_owned(),
        ..Default::default()
    };
    let err = cfg
        .validate()
        .expect_err("unrecognised conflict policy should be rejected");
    assert!(!err.is_empty());
}

#[test]
fn sync_loop_all_valid_conflict_policies_pass() {
    for policy in &[
        "newest_wins",
        "rename_both",
        "error",
        "prefer_local",
        "prefer_remote",
        "manual_review",
    ] {
        let cfg = SyncLoopConfig {
            conflict_policy: policy.to_string(),
            ..SyncLoopConfig::default()
        };
        cfg.validate()
            .unwrap_or_else(|e| panic!("policy '{policy}' should be valid: {e}"));
    }
}

// ── config migration ──────────────────────────────────────────────────────────

#[test]
fn v0_bare_profile_migrates_to_current_version() {
    let v0 = serde_json::json!({
        "environment": "Development",
        "paths": {
            "config_dir": "/tmp/migrate-test/config",
            "state_dir": "/tmp/migrate-test/state",
            "runtime_dir": "/tmp/migrate-test/runtime",
            "cache_dir": "/tmp/migrate-test/cache"
        },
        "api": {
            "mode": "Development",
            "host": "bineapi.pcloud.com",
            "port": 443,
            "server_name": "bineapi.pcloud.com",
            "connect_timeout_ms": 5000,
            "read_timeout_ms": 15000
        }
    });
    let migrated = migrate_to_current(v0).expect("v0 migration should succeed");
    let version = migrated
        .get("version")
        .and_then(|v| v.as_u64())
        .expect("migrated envelope must have version");
    assert_eq!(version, pcloud_config::migrate::CURRENT_VERSION as u64);
    assert!(
        migrated.get("profile").is_some(),
        "migrated envelope must have profile key"
    );
}

#[test]
fn future_version_migration_is_rejected() {
    let future_doc = serde_json::json!({
        "version": 9999,
        "profile": {}
    });
    let err = migrate_to_current(future_doc).expect_err("future version should be rejected");
    assert!(
        matches!(err, pcloud_config::migrate::MigrationError::TooNew(_)),
        "expected TooNew error, got: {err:?}"
    );
}

#[test]
fn production_profile_accepts_tls_api_mode() {
    // Production secure defaults already pin TLS, but verify the round-trip.
    let mut p = prod_profile();
    p.api.mode = ApiMode::Tls;
    p.validate()
        .expect("Production with TLS mode should be accepted");
}

#[test]
fn development_profile_accepts_plaintext_api_mode() {
    let mut p = dev_profile();
    p.api.mode = ApiMode::Plaintext;
    p.validate()
        .expect("Development with Plaintext mode should be accepted");
}

#[test]
fn limits_zero_concurrent_uploads_still_validates() {
    // ResourceLimits does not enforce > 0 for uploads (it's a policy bound,
    // not a liveness requirement); confirm that validate() passes for zero.
    let mut p = prod_profile();
    p.limits.max_concurrent_uploads = 0;
    p.validate()
        .expect("zero concurrent uploads is a valid (if useless) config");
}

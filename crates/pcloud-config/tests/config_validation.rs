#![allow(clippy::pedantic)]
//! Integration tests for `pcloud-config` profile construction, env-var
//! overrides, and validation. All tests are network-free and file-system
//! agnostic (using temp dirs or in-memory paths).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;
use std::sync::Mutex;

use pcloud_config::{
    ConfigError, ConfigProfile, Environment,
    api::ApiMode,
    env::apply_env_overrides,
    migrate::migrate_to_current,
    paths::{ManagedPaths, PcloudDirs},
    schema::validate_document,
    sync_loop::SyncLoopConfig,
};

// ── helpers ───────────────────────────────────────────────────────────────────

type DocumentMutation = (&'static str, Box<dyn Fn(&mut serde_json::Value)>);

fn root() -> PathBuf {
    // `validate()` requires absolute paths. Unix `/tmp/...` satisfies that;
    // Windows absolute paths need a drive letter. Keep the test platform-
    // correct without depending on real filesystem state (the fixtures
    // never touch the disk).
    #[cfg(unix)]
    {
        PathBuf::from("/tmp/pcloud-config-test")
    }
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\pcloud-config-test")
    }
}

fn prod_profile() -> ConfigProfile {
    ConfigProfile::secure_defaults(root(), Environment::Production)
}

fn dev_profile() -> ConfigProfile {
    ConfigProfile::secure_defaults(root(), Environment::Development)
}

const PCLOUD_ENV_KEYS: &[&str] = &[
    "PCLOUD_ROOT",
    "PCLOUD_ENV",
    "PCLOUD_API_MODE",
    "PCLOUD_API_HOST",
    "PCLOUD_API_PORT",
    "PCLOUD_API_SERVER_NAME",
    "PCLOUD_API_CONNECT_TIMEOUT_MS",
    "PCLOUD_API_READ_TIMEOUT_MS",
    "PCLOUD_PLUGINS_ENABLED",
    "PCLOUD_PLUGIN_ALLOW_NETWORK",
    "PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL",
    "PCLOUD_PLUGIN_ALLOW_CRYPTO",
    "PCLOUD_DURABLE_AUTH_TOKENS",
    "PCLOUD_VAULT",
    "PCLOUD_MOUNT_CACHE_SIZE_MB",
    "PCLOUD_MOUNT_PAGE_CACHE_ENTRIES",
    "PCLOUD_MOUNT_METADATA_TTL_SECS",
    "PCLOUD_AUTO_MOUNT_PATH",
    "PCLOUD_MIGRATE_LEGACY_PATHS",
    "HOME",
];

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_pcloud_env<T>(overrides: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = PCLOUD_ENV_KEYS
        .iter()
        .map(|key| (*key, std::env::var_os(key)))
        .collect();
    // SAFETY (test-only): Rust 2024 marks `std::env::set_var` /
    // `remove_var` as unsafe because they race with libc getenv readers
    // across threads. The enclosing `ENV_LOCK` mutex (acquired above)
    // serialises every test that touches process env, so no concurrent
    // reader observes the intermediate clear→set state.
    // SAFETY: see preceding paragraph.
    unsafe {
        for key in PCLOUD_ENV_KEYS {
            std::env::remove_var(key);
        }
        for (key, value) in overrides {
            std::env::set_var(key, value);
        }
    }
    let result = f();
    // SAFETY: same ENV_LOCK-protected window as above (test-only).
    unsafe {
        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
    result
}

#[test]
fn managed_paths_report_each_relative_field() {
    let absolute = root();
    for field in ["config_dir", "state_dir", "runtime_dir", "cache_dir"] {
        let mut paths = ManagedPaths {
            config_dir: absolute.join("config"),
            state_dir: absolute.join("state"),
            runtime_dir: absolute.join("runtime"),
            cache_dir: absolute.join("cache"),
        };
        match field {
            "config_dir" => paths.config_dir = "relative-config".into(),
            "state_dir" => paths.state_dir = "relative-state".into(),
            "runtime_dir" => paths.runtime_dir = "relative-runtime".into(),
            "cache_dir" => paths.cache_dir = "relative-cache".into(),
            _ => unreachable!(),
        }
        assert!(matches!(
            paths.validate(),
            Err(ConfigError::PathMustBeAbsolute { field: actual }) if actual == field
        ));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn opted_in_legacy_path_migration_copies_recursively_without_overwriting() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let legacy = home.join(".pcloud");
    std::fs::create_dir_all(legacy.join("config/nested")).unwrap();
    std::fs::create_dir_all(legacy.join("state")).unwrap();
    std::fs::create_dir_all(legacy.join("cache")).unwrap();
    std::fs::write(legacy.join("config/nested/profile.json"), b"config").unwrap();
    std::fs::write(legacy.join("state/store.db"), b"state").unwrap();
    std::fs::write(legacy.join("cache/blob"), b"cache").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        legacy.join("state/store.db"),
        legacy.join("config/ignored-link"),
    )
    .unwrap();

    let destination = temp.path().join("canonical");
    let dirs = PcloudDirs {
        config: destination.join("config"),
        data: destination.join("data"),
        cache: destination.join("cache"),
        runtime: destination.join("runtime"),
    };
    let home_text = home.to_string_lossy();
    with_pcloud_env(
        &[
            ("HOME", home_text.as_ref()),
            ("PCLOUD_MIGRATE_LEGACY_PATHS", "1"),
        ],
        || {
            assert_eq!(
                PcloudDirs::legacy_linux_home().as_deref(),
                Some(legacy.as_path())
            );
            assert!(dirs.migrate_from_legacy_if_needed().unwrap());
            assert_eq!(
                std::fs::read(dirs.config.join("nested/profile.json")).unwrap(),
                b"config"
            );
            assert_eq!(std::fs::read(dirs.data.join("store.db")).unwrap(), b"state");
            assert_eq!(std::fs::read(dirs.cache.join("blob")).unwrap(), b"cache");
            assert!(!dirs.config.join("ignored-link").exists());

            // Every destination is now non-empty, so a second migration is
            // intentionally a no-op and cannot overwrite canonical data.
            std::fs::write(dirs.data.join("store.db"), b"canonical").unwrap();
            assert!(!dirs.migrate_from_legacy_if_needed().unwrap());
            assert_eq!(
                std::fs::read(dirs.data.join("store.db")).unwrap(),
                b"canonical"
            );
        },
    );

    with_pcloud_env(&[("HOME", home_text.as_ref())], || {
        assert!(!dirs.migrate_from_legacy_if_needed().unwrap())
    });
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
    let result = with_pcloud_env(&[("PCLOUD_API_HOST", "test-api.example.com")], || {
        apply_env_overrides(dev_profile())
    });
    let p = result.expect("apply_env_overrides should succeed");
    assert_eq!(p.api.host, "test-api.example.com");
}

#[test]
fn env_pcloud_api_port_overrides_port() {
    let result = with_pcloud_env(&[("PCLOUD_API_PORT", "9000")], || {
        apply_env_overrides(dev_profile())
    });
    let p = result.expect("apply_env_overrides should succeed");
    assert_eq!(p.api.port, 9000);
}

#[test]
fn env_pcloud_durable_auth_tokens_enables_flag() {
    let result = with_pcloud_env(&[("PCLOUD_DURABLE_AUTH_TOKENS", "true")], || {
        apply_env_overrides(dev_profile())
    });
    let p = result.expect("apply_env_overrides should succeed");
    assert!(p.features.durable_auth_tokens_enabled);
}

#[test]
fn env_pcloud_env_sets_environment() {
    let result = with_pcloud_env(&[("PCLOUD_ENV", "test")], || {
        apply_env_overrides(dev_profile())
    });
    let p = result.expect("apply_env_overrides should succeed");
    assert_eq!(p.environment, Environment::Test);
}

#[test]
fn env_pcloud_root_rewrites_managed_paths() {
    let result = with_pcloud_env(&[("PCLOUD_ROOT", "/tmp/pcloud-root-override")], || {
        apply_env_overrides(dev_profile())
    });
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
fn env_override_matrix_covers_every_targeted_runtime_setting() {
    let result = with_pcloud_env(
        &[
            ("PCLOUD_ENV", "production"),
            ("PCLOUD_API_MODE", "tls"),
            ("PCLOUD_API_SERVER_NAME", "api.coverage.example"),
            ("PCLOUD_API_CONNECT_TIMEOUT_MS", "1234"),
            ("PCLOUD_API_READ_TIMEOUT_MS", "5678"),
            ("PCLOUD_PLUGINS_ENABLED", "yes"),
            ("PCLOUD_PLUGIN_ALLOW_NETWORK", "on"),
            ("PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL", "1"),
            ("PCLOUD_PLUGIN_ALLOW_CRYPTO", "true"),
            ("PCLOUD_VAULT", "file"),
            ("PCLOUD_MOUNT_CACHE_SIZE_MB", "512"),
            ("PCLOUD_MOUNT_PAGE_CACHE_ENTRIES", "8192"),
            ("PCLOUD_MOUNT_METADATA_TTL_SECS", "17"),
            ("PCLOUD_AUTO_MOUNT_PATH", "/tmp/pcloud-auto-mount"),
        ],
        || apply_env_overrides(dev_profile()),
    )
    .unwrap();
    assert_eq!(result.environment, Environment::Production);
    assert_eq!(result.api.mode, ApiMode::Tls);
    assert_eq!(result.api.server_name, "api.coverage.example");
    assert_eq!(result.api.connect_timeout_ms, 1234);
    assert_eq!(result.api.read_timeout_ms, 5678);
    assert!(result.extensions.plugins_enabled);
    assert!(result.extensions.allow_network_capability);
    assert!(result.extensions.allow_sync_control_capability);
    assert!(result.extensions.allow_crypto_capability);
    assert_eq!(result.mount.cache_size_mb, 512);
    assert_eq!(result.mount.page_cache_entries, 8192);
    assert_eq!(result.mount.metadata_ttl_secs, 17);
    assert_eq!(
        result.mount.auto_mount_path.as_deref(),
        Some(std::path::Path::new("/tmp/pcloud-auto-mount"))
    );

    for (key, value) in [
        ("PCLOUD_API_MODE", "invalid"),
        ("PCLOUD_API_CONNECT_TIMEOUT_MS", "invalid"),
        ("PCLOUD_MOUNT_CACHE_SIZE_MB", "invalid"),
        ("PCLOUD_VAULT", "invalid"),
    ] {
        assert!(
            with_pcloud_env(&[(key, value)], || apply_env_overrides(dev_profile())).is_err(),
            "{key}"
        );
    }
}

#[test]
fn env_invalid_bool_value_returns_typed_error() {
    let result = with_pcloud_env(
        &[("PCLOUD_DURABLE_AUTH_TOKENS", "definitely-not-a-bool")],
        || apply_env_overrides(dev_profile()),
    );
    let err = result.expect_err("invalid bool should fail");
    assert!(
        matches!(err, ConfigError::InvalidEnvironmentValue { .. }),
        "unexpected error: {err:?}"
    );
}

#[test]
fn env_invalid_port_returns_typed_error() {
    let result = with_pcloud_env(&[("PCLOUD_API_PORT", "not-a-port")], || {
        apply_env_overrides(dev_profile())
    });
    let err = result.expect_err("invalid port should fail");
    assert!(matches!(err, ConfigError::InvalidEnvironmentValue { .. }));
}

#[test]
fn env_invalid_environment_name_returns_typed_error() {
    let result = with_pcloud_env(&[("PCLOUD_ENV", "staging")], || {
        apply_env_overrides(dev_profile())
    });
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

#[test]
fn schema_validator_reports_every_supported_shape_and_bound() {
    fn envelope() -> serde_json::Value {
        serde_json::json!({
            "version": pcloud_config::migrate::CURRENT_VERSION,
            "profile": dev_profile(),
        })
    }

    fn reasons(doc: &serde_json::Value) -> Vec<pcloud_config::schema::SchemaViolation> {
        validate_document(doc, &serde_json::to_string_pretty(doc).unwrap())
    }

    for scalar in [
        serde_json::Value::Null,
        serde_json::Value::Bool(true),
        serde_json::json!(1),
        serde_json::json!("text"),
        serde_json::json!([]),
    ] {
        let errors = reasons(&scalar);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].reason.contains("expected object"));
        assert_eq!(errors[0].line, Some(1));
        assert!(format!("{}", errors[0]).contains("at "));
    }

    let mut missing = envelope();
    missing["profile"]
        .as_object_mut()
        .unwrap()
        .remove("environment");
    let missing_errors = reasons(&missing);
    assert!(
        missing_errors
            .iter()
            .any(|error| error.pointer == "/profile/environment" && error.line.is_none())
    );
    assert!(format!("{}", missing_errors[0]).starts_with("at "));

    let mut additional = envelope();
    additional["profile"]["bad~/key"] = serde_json::json!(true);
    assert!(
        reasons(&additional)
            .iter()
            .any(|error| error.pointer.contains("bad~0~1key"))
    );

    let mutations: Vec<DocumentMutation> = vec![
        (
            "string type",
            Box::new(|doc| doc["profile"]["api"]["host"] = serde_json::json!(9)),
        ),
        (
            "string enum",
            Box::new(|doc| doc["profile"]["environment"] = serde_json::json!("Staging")),
        ),
        (
            "array type",
            Box::new(|doc| {
                doc["profile"]["extensions"]["trusted_plugin_keys"] = serde_json::json!("bad")
            }),
        ),
        (
            "array item",
            Box::new(|doc| {
                doc["profile"]["extensions"]["trusted_plugin_keys"] = serde_json::json!([["bad"]])
            }),
        ),
        (
            "number type",
            Box::new(|doc| {
                doc["profile"]["resilience"]["rate_limit_refill_per_sec"] =
                    serde_json::json!("fast")
            }),
        ),
        (
            "boolean type",
            Box::new(|doc| doc["profile"]["features"]["crypto_enabled"] = serde_json::json!("yes")),
        ),
        (
            "integer type",
            Box::new(|doc| doc["profile"]["api"]["port"] = serde_json::json!(1.5)),
        ),
        (
            "signed minimum",
            Box::new(|doc| doc["version"] = serde_json::json!(-1)),
        ),
        (
            "signed maximum",
            Box::new(|doc| doc["profile"]["api"]["port"] = serde_json::json!(70_000)),
        ),
        (
            "array string item",
            Box::new(|doc| {
                doc["profile"]["data_residency"]["allowed_regions"] = serde_json::json!(["eu", 2])
            }),
        ),
    ];
    for (label, mutate) in mutations {
        let mut doc = envelope();
        mutate(&mut doc);
        assert!(!reasons(&doc).is_empty(), "{label} unexpectedly passed");
    }

    let mut unsigned_max = envelope();
    unsigned_max["profile"]["api"]["port"] =
        serde_json::Value::Number(serde_json::Number::from(u64::MAX));
    assert!(
        reasons(&unsigned_max)
            .iter()
            .any(|error| error.reason.contains("above maximum"))
    );

    let mut unsigned_ok = envelope();
    unsigned_ok["version"] =
        serde_json::Value::Number(serde_json::Number::from(i64::MAX as u64 + 1));
    assert!(reasons(&unsigned_ok).is_empty());

    let mut any = envelope();
    any["profile"]["api"]["tls_revocation_check"] =
        serde_json::json!({"future": ["shape", 1, true]});
    assert!(reasons(&any).is_empty());
}

#[test]
fn standalone_policy_defaults_and_ipc_limit_errors_cover_public_contracts() {
    let limits: pcloud_config::limits::ResourceLimits = serde_json::from_value(serde_json::json!({
        "max_concurrent_uploads": 4,
        "max_concurrent_downloads": 4,
        "max_parser_frame_bytes": 8_388_608
    }))
    .unwrap();
    assert_eq!(limits.max_ipc_connections, 128);
    assert_eq!(limits.max_ipc_connections_per_peer, 32);
    limits.validate_ipc_limits().unwrap();

    for (global, per_peer) in [(0, 1), (65_536, 1), (128, 0), (65_535, 65_536), (2, 3)] {
        let candidate = pcloud_config::limits::ResourceLimits {
            max_ipc_connections: global,
            max_ipc_connections_per_peer: per_peer,
            ..limits.clone()
        };
        assert!(
            candidate.validate_ipc_limits().is_err(),
            "{global}/{per_peer}"
        );
    }

    let mount: pcloud_config::mount::MountPolicy = serde_json::from_value(serde_json::json!({
        "allow_other": false,
        "owner_only_by_default": true
    }))
    .unwrap();
    assert_eq!(
        mount.cache_size_mb,
        pcloud_config::mount::MountPolicy::DEFAULT_CACHE_SIZE_MB
    );
    assert_eq!(
        mount.page_cache_entries,
        pcloud_config::mount::MountPolicy::DEFAULT_PAGE_CACHE_ENTRIES
    );
    assert_eq!(
        mount.metadata_ttl_secs,
        pcloud_config::mount::MountPolicy::DEFAULT_METADATA_TTL_SECS
    );
    assert!(mount.auto_mount_path.is_none());

    let observability = pcloud_config::observability::ObservabilityFlags::default();
    assert!(observability.structured_logs_enabled);
    assert!(observability.audit_export_enabled);
    assert!(!observability.tracing_enabled);
    assert!(!observability.metrics_enabled);
}

#[test]
fn crypto_kms_validation_matrix_rejects_every_incomplete_provider_shape() {
    use pcloud_config::crypto_kms::{CryptoConfig, CryptoKmsConfig, CryptoMode};

    assert_eq!(CryptoMode::Raw.tag(), "raw");
    assert_eq!(CryptoMode::Kms.tag(), "kms");
    assert!(CryptoConfig::default().validate().is_ok());
    assert!(
        CryptoConfig {
            mode: CryptoMode::Kms,
            kms: None,
        }
        .validate()
        .is_err()
    );
    assert!(
        CryptoConfig {
            mode: CryptoMode::Kms,
            kms: Some(CryptoKmsConfig::Null),
        }
        .validate()
        .is_err()
    );

    let aws = |region: &str, key_id: &str| CryptoKmsConfig::Aws {
        region: region.to_owned(),
        key_id: key_id.to_owned(),
    };
    assert!(aws("eu-central-1", "").validate().is_err());
    assert_eq!(aws("eu-central-1", "key").tag(), "aws");

    let vault = |url: &str, transit_key: &str, token_env: &str| CryptoKmsConfig::Vault {
        url: url.to_owned(),
        transit_key: transit_key.to_owned(),
        token_env: token_env.to_owned(),
    };
    for provider in [
        vault("", "key", "TOKEN"),
        vault("https://user@vault.example", "key", "TOKEN"),
        vault("https://user:password@vault.example", "key", "TOKEN"),
        vault("https://vault.example", "", "TOKEN"),
    ] {
        assert!(provider.validate().is_err(), "{provider:?}");
    }
    assert_eq!(
        vault("https://vault.example", "key", "TOKEN").tag(),
        "vault"
    );

    let pkcs11 = |module: &str, pin: &str, key: &str| CryptoKmsConfig::Pkcs11 {
        module_path: module.to_owned(),
        slot_id: 7,
        pin_env: pin.to_owned(),
        key_label: key.to_owned(),
    };
    for provider in [
        pkcs11("", "PIN", "key"),
        pkcs11("/module.so", "", "key"),
        pkcs11("/module.so", "PIN", ""),
    ] {
        assert!(provider.validate().is_err(), "{provider:?}");
    }
}

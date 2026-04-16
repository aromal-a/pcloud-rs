//! Config-file loader with permission check, schema validation, and
//! versioned migrations.
//!
//! The loader is deliberately strict:
//! - refuses group- or world-readable files in production,
//! - warns but still loads in development,
//! - validates against the published JSON schema before deserializing,
//! - migrates older documents (v0/v1) to the current version in memory.
//!
//! `ConfigProfile::load` (in `lib.rs`) delegates to
//! [`ConfigProfile::load_with_validation`].

// **PLATFORM:** Unix (Linux, BSD, macOS)
// **GATING:** #[cfg(unix)].

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{
    ConfigError, ConfigProfile, Environment,
    migrate::{MigrationError, migrate_to_current},
    schema::{SchemaViolation, validate_document},
};

/// Behavioural knobs for loading a config file.
#[derive(Debug, Clone, Copy)]
pub struct LoadOptions {
    /// When true, skip the group/world permission rejection. Wire this to
    /// a CLI flag like `--insecure-config` (dev only).
    pub insecure_permissions: bool,
    /// The environment the CLI believes it is running in. Controls whether
    /// insecure permissions are a hard error (Production) or a logged
    /// warning (Development/Test).
    pub enforcement_environment: Environment,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            insecure_permissions: false,
            enforcement_environment: Environment::Production,
        }
    }
}

impl LoadOptions {
    /// Enforce the posture appropriate for `env`: production refuses
    /// group/world-readable files; dev/test logs a warning.
    ///
    /// ```
    /// use pcloud_config::{Environment, LoadOptions};
    /// let opts = LoadOptions::enforcing(Environment::Production);
    /// assert!(!opts.insecure_permissions);
    /// assert_eq!(opts.enforcement_environment, Environment::Production);
    /// ```
    #[must_use]
    pub fn enforcing(env: Environment) -> Self {
        Self {
            insecure_permissions: false,
            enforcement_environment: env,
        }
    }

    /// Shorthand for development-environment load options.
    ///
    /// ```
    /// use pcloud_config::{Environment, LoadOptions};
    /// let opts = LoadOptions::development();
    /// assert_eq!(opts.enforcement_environment, Environment::Development);
    /// ```
    #[must_use]
    pub fn development() -> Self {
        Self {
            insecure_permissions: false,
            enforcement_environment: Environment::Development,
        }
    }
}

/// Result of a successful load including any non-fatal diagnostics emitted
/// along the way (e.g. permission warnings in development).
#[derive(Debug, Clone)]
pub struct LoadedProfile {
    /// Fully validated [`ConfigProfile`] ready for consumption by the daemon,
    /// SDK, or CLI. Produced from the on-disk envelope after migration and
    /// schema validation.
    pub profile: ConfigProfile,
    /// Non-fatal diagnostics collected during load. Each string is
    /// human-readable and already carries the offending path and mode.
    ///
    /// # When a warning is emitted vs. when the same condition is fatal
    ///
    /// | Condition                                              | Dev / Test                 | Production                                   | With `insecure_permissions = true`   |
    /// |--------------------------------------------------------|----------------------------|----------------------------------------------|---------------------------------------|
    /// | Config file has any group/other permission bit         | warning appended here      | hard error [`ConfigError::InsecureConfigPermissions`] | warning appended here (any env)       |
    /// | Config file is owner-only (`mode & 0o077 == 0`)        | no warning                 | no warning                                   | no warning                            |
    /// | Schema violation / JSON parse error                    | hard error                 | hard error                                   | hard error                            |
    /// | Migration failure / missing profile                    | hard error                 | hard error                                   | hard error                            |
    /// | Post-deserialize [`ConfigProfile::validate`] failure   | hard error                 | hard error                                   | hard error                            |
    ///
    /// In other words: `warnings` is reserved for *permission-posture
    /// relaxations*. Every other anomaly aborts the load. The vector is
    /// always `Vec::new()` on a clean owner-only file, regardless of
    /// environment.
    pub warnings: Vec<String>,
    /// The `version` field as read from disk, **before** migration is
    /// applied. Useful for telemetry (e.g. "N profiles still on v0") and
    /// for operator messages that tell users when a one-way migration
    /// just happened. The in-memory [`Self::profile`] is always at
    /// [`crate::migrate::CURRENT_VERSION`] regardless of this field.
    pub source_version: u32,
}

impl ConfigProfile {
    /// Backwards-compatible entry point. Enforces production posture by
    /// default and does not accept insecure permissions.
    pub fn load(path: &Path) -> Result<ConfigProfile, ConfigError> {
        let opts = LoadOptions::enforcing(Environment::Production);
        Self::load_with_validation(path, opts).map(|l| l.profile)
    }

    /// Full-fidelity loader: permission check, schema validation, migration,
    /// then typed deserialization.
    pub fn load_with_validation(
        path: &Path,
        opts: LoadOptions,
    ) -> Result<LoadedProfile, ConfigError> {
        let mut warnings = Vec::new();

        check_permissions(path, &opts, &mut warnings)?;

        let source = fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(format!("read {}: {}", path.display(), e)))?;

        let raw: Value = serde_json::from_str(&source)
            .map_err(|e| ConfigError::InvalidJson(format!("{}: {}", path.display(), e)))?;

        let source_version = raw
            .as_object()
            .and_then(|o| o.get("version"))
            .and_then(Value::as_u64)
            .map(|v| v as u32)
            .unwrap_or(0);

        let migrated = migrate_to_current(raw).map_err(|e: MigrationError| {
            ConfigError::Migration(format!("{}: {}", path.display(), e))
        })?;

        let violations = validate_document(&migrated, &source);
        if !violations.is_empty() {
            return Err(ConfigError::SchemaViolations(format_violations(
                path,
                &violations,
            )));
        }

        let profile_value = migrated
            .get("profile")
            .cloned()
            .ok_or_else(|| ConfigError::Migration("missing profile after migration".into()))?;

        let profile: ConfigProfile = serde_json::from_value(profile_value).map_err(|e| {
            ConfigError::InvalidJson(format!("{}: deserialize: {}", path.display(), e))
        })?;

        profile.validate()?;

        Ok(LoadedProfile {
            profile,
            warnings,
            source_version,
        })
    }
}

fn format_violations(path: &Path, violations: &[SchemaViolation]) -> String {
    let mut out = format!("{}:\n", path.display());
    for v in violations {
        out.push_str("  - ");
        out.push_str(&v.to_string());
        out.push('\n');
    }
    out
}

#[cfg(unix)]
fn check_permissions(
    path: &Path,
    opts: &LoadOptions,
    warnings: &mut Vec<String>,
) -> Result<(), ConfigError> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path)
        .map_err(|e| ConfigError::Io(format!("stat {}: {}", path.display(), e)))?;
    let mode = meta.mode() & 0o777;
    let insecure = mode & 0o077 != 0;

    if !insecure {
        return Ok(());
    }

    if opts.insecure_permissions {
        warnings.push(format!(
            "config file {} has mode {:o}; loading anyway because --insecure-config was set",
            path.display(),
            mode
        ));
        return Ok(());
    }

    match opts.enforcement_environment {
        Environment::Production => Err(ConfigError::InsecureConfigPermissions {
            path: path.display().to_string(),
            mode,
        }),
        Environment::Development | Environment::Test => {
            warnings.push(format!(
                "WARN: config file {} has group/world-readable mode {:o}; \
                 fix with `chmod 600 {}` (ignored in {:?})",
                path.display(),
                mode,
                path.display(),
                opts.enforcement_environment,
            ));
            Ok(())
        }
    }
}

#[cfg(not(unix))]
fn check_permissions(
    _path: &Path,
    _opts: &LoadOptions,
    _warnings: &mut Vec<String>,
) -> Result<(), ConfigError> {
    Ok(())
}

/// Helper exposed for integration tests and tooling that need the canonical
/// envelope path candidate list. Not used by the core loader.
#[must_use]
pub fn default_candidate_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".config").join("pcloud").join("config.json"),
        home.join(".pcloud").join("config.json"),
    ]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    fn write_envelope(dir: &Path, name: &str, json: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    fn valid_envelope_json() -> String {
        let profile = ConfigProfile::secure_defaults(
            PathBuf::from("/tmp/pcloud-loader-test"),
            Environment::Development,
        );
        serde_json::to_string_pretty(&serde_json::json!({
            "version": crate::migrate::CURRENT_VERSION,
            "profile": profile,
        }))
        .unwrap()
    }

    #[test]
    fn secure_mode_file_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_envelope(tmp.path(), "cfg.json", &valid_envelope_json(), 0o600);
        let loaded = ConfigProfile::load_with_validation(
            &p,
            LoadOptions::enforcing(Environment::Production),
        )
        .unwrap();
        assert_eq!(loaded.source_version, crate::migrate::CURRENT_VERSION);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn group_readable_file_is_rejected_in_production() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_envelope(tmp.path(), "cfg.json", &valid_envelope_json(), 0o644);
        let err = ConfigProfile::load_with_validation(
            &p,
            LoadOptions::enforcing(Environment::Production),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::InsecureConfigPermissions { .. }));
    }

    #[test]
    fn group_readable_file_warns_in_development() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_envelope(tmp.path(), "cfg.json", &valid_envelope_json(), 0o644);
        let loaded = ConfigProfile::load_with_validation(
            &p,
            LoadOptions::enforcing(Environment::Development),
        )
        .unwrap();
        assert!(!loaded.warnings.is_empty());
    }

    #[test]
    fn insecure_flag_overrides_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_envelope(tmp.path(), "cfg.json", &valid_envelope_json(), 0o644);
        let opts = LoadOptions {
            insecure_permissions: true,
            enforcement_environment: Environment::Production,
        };
        let loaded = ConfigProfile::load_with_validation(&p, opts).unwrap();
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.contains("--insecure-config"))
        );
    }

    #[test]
    fn schema_violation_produces_error_with_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        // Inject a bogus property at /profile/api to trip additionalProperties.
        let mut envelope: serde_json::Value = serde_json::from_str(&valid_envelope_json()).unwrap();
        envelope["profile"]["api"]["bogus"] = serde_json::json!(true);
        let p = write_envelope(
            tmp.path(),
            "cfg.json",
            &serde_json::to_string_pretty(&envelope).unwrap(),
            0o600,
        );
        let err = ConfigProfile::load_with_validation(&p, LoadOptions::development()).unwrap_err();
        match err {
            ConfigError::SchemaViolations(msg) => {
                assert!(msg.contains("/profile/api/bogus"));
            }
            other => panic!("wrong error: {:?}", other),
        }
    }

    #[test]
    fn v0_document_is_migrated_and_loads() {
        let profile = ConfigProfile::secure_defaults(
            PathBuf::from("/tmp/pcloud-loader-v0"),
            Environment::Development,
        );
        let bare = serde_json::to_value(&profile).unwrap();
        // Strip observability to mimic an on-disk v0 doc.
        let mut as_obj = bare.as_object().cloned().unwrap();
        as_obj.remove("observability");
        let legacy_json = serde_json::to_string_pretty(&serde_json::Value::Object(as_obj)).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let p = write_envelope(tmp.path(), "cfg.json", &legacy_json, 0o600);
        let loaded = ConfigProfile::load_with_validation(&p, LoadOptions::development()).unwrap();
        assert_eq!(loaded.source_version, 0);
        // Observability should be present (added by migration).
        assert!(loaded.profile.observability.structured_logs_enabled);
    }

    #[test]
    fn invalid_json_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_envelope(tmp.path(), "cfg.json", "not json", 0o600);
        let err = ConfigProfile::load_with_validation(&p, LoadOptions::development()).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidJson(_)));
    }

    #[test]
    fn default_load_uses_production_enforcement() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_envelope(tmp.path(), "cfg.json", &valid_envelope_json(), 0o644);
        let err = ConfigProfile::load(&p).unwrap_err();
        assert!(matches!(err, ConfigError::InsecureConfigPermissions { .. }));
    }
}

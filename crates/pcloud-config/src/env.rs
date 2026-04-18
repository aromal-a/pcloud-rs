//! `PCLOUD_*` environment-variable overrides for a [`ConfigProfile`].
//!
//! Overrides are applied *after* deserialization and *before* validation,
//! so they must themselves yield a profile that passes
//! [`ConfigProfile::validate`]. Unset or empty env vars are ignored; any
//! set-but-malformed value aborts the load with
//! [`ConfigError::InvalidEnvironmentValue`].
//!
//! # Precedence
//!
//! The effective value of a given field is resolved in this order (later
//! entries win):
//!
//! 1. Struct default (`ConfigProfile::secure_defaults`) — the bottom layer
//!    when no file is present.
//! 2. On-disk envelope (`profile.<key>`) — deserialized from the config
//!    file and passed through [`crate::migrate::migrate_to_current`].
//! 3. Targeted `PCLOUD_*` env var — applied to the specific field.
//! 4. Coarse `PCLOUD_ROOT` — rewrites every path in `paths.*` and
//!    `extensions.plugin_dir`. Applied **first** inside
//!    [`apply_env_overrides`] so targeted overrides below still win for
//!    their specific field.
//! 5. `PCLOUD_ENV` snap — flipping the environment also snaps
//!    `api.mode` to the new secure default **unless** `PCLOUD_API_MODE` is
//!    also set, in which case the explicit mode wins.
//!
//! Validation (`ConfigProfile::validate`) runs after all overrides, so an
//! override that produces an invalid profile (e.g.
//! `PCLOUD_ENV=production` with `PCLOUD_API_MODE=plaintext`) is rejected
//! with [`ConfigError::InvalidApiEndpoint`] rather than silently accepted.
//!
//! # Full mapping (targeted overrides → TOML/JSON key)
//!
//! | Env var                              | Target TOML/JSON key                             | Notes                                        |
//! |--------------------------------------|--------------------------------------------------|----------------------------------------------|
//! | `PCLOUD_ROOT`                        | `paths.config_dir`, `paths.state_dir`, `paths.runtime_dir`, `paths.cache_dir`, `extensions.plugin_dir` | Coarse override — re-roots all managed paths under `<root>/{config,state,runtime,cache,plugins}`. |
//! | `PCLOUD_ENV`                         | `environment`                                    | Also snaps `api.mode` if `PCLOUD_API_MODE` is not set. |
//! | `PCLOUD_API_MODE`                    | `api.mode`                                       | Wins over the `PCLOUD_ENV` mode snap.        |
//! | `PCLOUD_API_HOST`                    | `api.host`                                       |                                              |
//! | `PCLOUD_API_PORT`                    | `api.port`                                       | Must parse as `u16`.                         |
//! | `PCLOUD_API_SERVER_NAME`             | `api.server_name`                                | TLS SNI / cert verification name.            |
//! | `PCLOUD_API_CONNECT_TIMEOUT_MS`      | `api.connect_timeout_ms`                         | Must parse as `u64`; `0` fails validation.   |
//! | `PCLOUD_API_READ_TIMEOUT_MS`         | `api.read_timeout_ms`                            | Must parse as `u64`; `0` fails validation.   |
//! | `PCLOUD_PLUGINS_ENABLED`             | `extensions.plugins_enabled`                     | Boolean grammar below.                       |
//! | `PCLOUD_PLUGIN_ALLOW_NETWORK`        | `extensions.allow_network_capability`            | Requires `plugins_enabled=true`.             |
//! | `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL`   | `extensions.allow_sync_control_capability`       | Requires `plugins_enabled=true`.             |
//! | `PCLOUD_PLUGIN_ALLOW_CRYPTO`         | `extensions.allow_crypto_capability`             | Requires `plugins_enabled=true`.             |
//! | `PCLOUD_DURABLE_AUTH_TOKENS`         | `features.durable_auth_tokens_enabled`           | Gates on-disk auth-token vault.              |
//! | `PCLOUD_VAULT`                       | `auth.backend`                                   | Selects vault backend (`auto`/`file`/`keychain`/`dpapi`/`secret-service`). |
//! | `PCLOUD_MOUNT_CACHE_SIZE_MB`         | `mount.cache_size_mb`                            | Page-cache memory budget in MiB.             |
//! | `PCLOUD_MOUNT_PAGE_CACHE_ENTRIES`    | `mount.page_cache_entries`                       | Max metadata-cache entries (LRU).            |
//! | `PCLOUD_MOUNT_METADATA_TTL_SECS`    | `mount.metadata_ttl_secs`                        | Metadata-cache TTL in seconds.               |
//! | `PCLOUD_AUTO_MOUNT_PATH`            | `mount.auto_mount_path`                          | If set, auto-mount at this path on daemon start. |
//!
//! Boolean env vars accept `1`/`0`, `true`/`false`, `yes`/`no`, `on`/`off`
//! (case-insensitive). Enum values for `PCLOUD_ENV` are `dev` / `development`,
//! `test`, `prod` / `production`. Enum values for `PCLOUD_API_MODE` are
//! `dev` / `development`, `plain` / `plaintext` / `tcp`, `tls` / `ssl`.
//! Enum values for `PCLOUD_VAULT` are `auto`, `file`, `keychain` (alias
//! `mac`/`macos`), `dpapi` (alias `win`/`windows`), `secret-service`
//! (alias `ss`/`secretservice`).
//! Empty / whitespace-only values are treated as unset.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{env, path::PathBuf};

use crate::{ConfigError, ConfigProfile, Environment, api::ApiMode, auth::VaultBackend};

/// Apply `PCLOUD_*` env-var overrides to `profile` in place and return it.
///
/// See the module-level table for the full list of variables. Note: when
/// `PCLOUD_ENV` flips the active [`Environment`], `api.mode` is snapped to
/// the new environment's secure default *unless* `PCLOUD_API_MODE` is also
/// set (in which case the explicit mode wins).
pub fn apply_env_overrides(mut profile: ConfigProfile) -> Result<ConfigProfile, ConfigError> {
    if let Some(root) = optional_env("PCLOUD_ROOT") {
        let root = PathBuf::from(root);
        profile.paths.config_dir = root.join("config");
        profile.paths.state_dir = root.join("state");
        profile.paths.runtime_dir = root.join("runtime");
        profile.paths.cache_dir = root.join("cache");
        profile.extensions.plugin_dir = root.join("plugins");
    }

    if let Some(environment) = optional_env("PCLOUD_ENV") {
        profile.environment = parse_environment("PCLOUD_ENV", &environment)?;
        if env::var_os("PCLOUD_API_MODE").is_none() {
            profile.api.mode = ApiMode::secure_default_for(profile.environment);
        }
    }

    if let Some(api_mode) = optional_env("PCLOUD_API_MODE") {
        profile.api.mode = parse_api_mode("PCLOUD_API_MODE", &api_mode)?;
    }
    if let Some(host) = optional_env("PCLOUD_API_HOST") {
        profile.api.host = host;
    }
    if let Some(port) = optional_env("PCLOUD_API_PORT") {
        profile.api.port = parse_u16("PCLOUD_API_PORT", &port)?;
    }
    if let Some(server_name) = optional_env("PCLOUD_API_SERVER_NAME") {
        profile.api.server_name = server_name;
    }
    if let Some(timeout) = optional_env("PCLOUD_API_CONNECT_TIMEOUT_MS") {
        profile.api.connect_timeout_ms = parse_u64("PCLOUD_API_CONNECT_TIMEOUT_MS", &timeout)?;
    }
    if let Some(timeout) = optional_env("PCLOUD_API_READ_TIMEOUT_MS") {
        profile.api.read_timeout_ms = parse_u64("PCLOUD_API_READ_TIMEOUT_MS", &timeout)?;
    }
    if let Some(enabled) = optional_env("PCLOUD_PLUGINS_ENABLED") {
        profile.extensions.plugins_enabled = parse_bool("PCLOUD_PLUGINS_ENABLED", &enabled)?;
    }
    if let Some(enabled) = optional_env("PCLOUD_PLUGIN_ALLOW_NETWORK") {
        profile.extensions.allow_network_capability =
            parse_bool("PCLOUD_PLUGIN_ALLOW_NETWORK", &enabled)?;
    }
    if let Some(enabled) = optional_env("PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL") {
        profile.extensions.allow_sync_control_capability =
            parse_bool("PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL", &enabled)?;
    }
    if let Some(enabled) = optional_env("PCLOUD_PLUGIN_ALLOW_CRYPTO") {
        profile.extensions.allow_crypto_capability =
            parse_bool("PCLOUD_PLUGIN_ALLOW_CRYPTO", &enabled)?;
    }
    if let Some(enabled) = optional_env("PCLOUD_DURABLE_AUTH_TOKENS") {
        profile.features.durable_auth_tokens_enabled =
            parse_bool("PCLOUD_DURABLE_AUTH_TOKENS", &enabled)?;
    }
    if let Some(backend) = optional_env("PCLOUD_VAULT") {
        profile.auth.backend = VaultBackend::parse("PCLOUD_VAULT", &backend)?;
    }
    if let Some(v) = optional_env("PCLOUD_MOUNT_CACHE_SIZE_MB") {
        profile.mount.cache_size_mb = parse_u32("PCLOUD_MOUNT_CACHE_SIZE_MB", &v)?;
    }
    if let Some(v) = optional_env("PCLOUD_MOUNT_PAGE_CACHE_ENTRIES") {
        profile.mount.page_cache_entries = parse_u32("PCLOUD_MOUNT_PAGE_CACHE_ENTRIES", &v)?;
    }
    if let Some(v) = optional_env("PCLOUD_MOUNT_METADATA_TTL_SECS") {
        profile.mount.metadata_ttl_secs = parse_u32("PCLOUD_MOUNT_METADATA_TTL_SECS", &v)?;
    }
    if let Some(v) = optional_env("PCLOUD_AUTO_MOUNT_PATH") {
        if !v.is_empty() {
            profile.mount.auto_mount_path = Some(std::path::PathBuf::from(v));
        }
    }

    Ok(profile)
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn parse_environment(name: &'static str, value: &str) -> Result<Environment, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "development" | "dev" => Ok(Environment::Development),
        "test" => Ok(Environment::Test),
        "production" | "prod" => Ok(Environment::Production),
        _ => Err(ConfigError::InvalidEnvironmentValue {
            name,
            value: value.to_owned(),
        }),
    }
}

fn parse_api_mode(name: &'static str, value: &str) -> Result<ApiMode, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "development" | "dev" => Ok(ApiMode::Development),
        "plaintext" | "plain" | "tcp" => Ok(ApiMode::Plaintext),
        "tls" | "ssl" => Ok(ApiMode::Tls),
        _ => Err(ConfigError::InvalidEnvironmentValue {
            name,
            value: value.to_owned(),
        }),
    }
}

fn parse_bool(name: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidEnvironmentValue {
            name,
            value: value.to_owned(),
        }),
    }
}

fn parse_u16(name: &'static str, value: &str) -> Result<u16, ConfigError> {
    value
        .parse()
        .map_err(|_| ConfigError::InvalidEnvironmentValue {
            name,
            value: value.to_owned(),
        })
}

fn parse_u32(name: &'static str, value: &str) -> Result<u32, ConfigError> {
    value
        .parse()
        .map_err(|_| ConfigError::InvalidEnvironmentValue {
            name,
            value: value.to_owned(),
        })
}

fn parse_u64(name: &'static str, value: &str) -> Result<u64, ConfigError> {
    value
        .parse()
        .map_err(|_| ConfigError::InvalidEnvironmentValue {
            name,
            value: value.to_owned(),
        })
}

#![forbid(unsafe_code)]
//! # pcloud-config
//!
//! Configuration model for the Rust `pcloud-rs` path: profiles, environment
//! selection, feature flags, runtime limits, extension policy, and API
//! endpoint bindings. Production profiles refuse transport downgrade and
//! refuse plaintext persistence of crypto key material.
//!
//! # On-disk envelope
//!
//! Profiles persist as a versioned JSON envelope. The canonical shape is
//! `{ "version": 2, "profile": { ... } }`; the JSON Schema describing it
//! verbatim is exposed as [`schema::CONFIG_SCHEMA_JSON`]. Every on-disk
//! document is migrated through [`migrate::migrate_to_current`] before
//! typed deserialization, so v0 (legacy bare profile) and v1 (envelope
//! without observability) files still load cleanly.
//!
//! Top-level profile fields correspond one-to-one with the modules in this
//! crate:
//!
//! | TOML/JSON key   | Module                | Purpose                                   |
//! |-----------------|-----------------------|-------------------------------------------|
//! | `environment`   | [`Environment`]       | Profile class (Development/Test/Production) |
//! | `paths`         | [`paths`]             | Managed directories                       |
//! | `api`           | [`api`]               | API endpoint + TLS policy                 |
//! | `extensions`    | [`extensions`]        | Plugin loader policy                      |
//! | `runtime`       | [`runtime`]           | Directory permission modes                |
//! | `features`      | [`features`]          | Feature flag toggles                      |
//! | `limits`        | [`limits`]            | Concurrency and parser bounds             |
//! | `mount`         | [`mount`]             | FUSE mount policy                         |
//! | `observability` | [`observability`]     | Telemetry toggles                         |
//! | `resilience`    | [`resilience`]        | Rate limit / breaker / retry              |
//! | `auth`          | [`auth`]              | Auth vault backend selection              |
//!
//! # Environment overrides
//!
//! Every field listed above has an opt-in environment-variable override
//! applied by [`env::apply_env_overrides`]. See that function's documentation
//! for the full `PCLOUD_*` → field mapping.
//!
//! # Security posture
//!
//! - Production TLS is mandatory ([`api::ApiEndpoint::validate`]).
//! - Managed directory modes reject any group/other bit
//!   ([`runtime::RuntimePolicy::validate`]).
//! - Config files that are group/world-readable are refused in production
//!   (`ConfigProfile::load_with_validation`).
//! - Durable auth-token persistence is opt-in
//!   ([`features::FeatureFlags::durable_auth_tokens_enabled`]).
//!
//! # Examples
//!
//! Build secure defaults under a test root and validate them:
//!
//! ```
//! use std::path::PathBuf;
//! use pcloud_config::{ConfigProfile, Environment};
//! let profile = ConfigProfile::secure_defaults(
//!     PathBuf::from("/tmp/pcloud-doc"),
//!     Environment::Production,
//! );
//! profile.validate().unwrap();
//! assert!(profile.features.crypto_enabled);
//! assert!(!profile.features.durable_auth_tokens_enabled);
//! ```

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

pub mod api;
pub mod audit_verifier;
pub mod auth;
pub mod crypto_kms;
pub mod data_residency;
pub mod env;
pub mod extensions;
pub mod features;
pub mod file_history;
pub mod ha;
pub mod integrity_sweeper;
pub mod limits;
pub mod loader;
pub mod migrate;
pub mod mount;
pub mod observability;
pub mod paths;
pub mod rate_limit;
pub mod resilience;
pub mod runtime;
pub mod schema;
pub mod sync_loop;
pub mod upgrade;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    api::ApiEndpoint, auth::AuthPolicy, crypto_kms::CryptoConfig,
    data_residency::DataResidencyPolicy, extensions::ExtensionPolicy, features::FeatureFlags,
    file_history::FileHistoryConfig, limits::ResourceLimits, mount::MountPolicy,
    observability::ObservabilityFlags, paths::ManagedPaths, rate_limit::RateLimitPolicy,
    resilience::ResiliencePolicy, runtime::RuntimePolicy, sync_loop::SyncLoopConfig,
    upgrade::UpgradePolicy,
};

pub use loader::{LoadOptions, LoadedProfile};

/// Crate identifier used in audit and telemetry records.
///
/// ```
/// assert_eq!(pcloud_config::CRATE_NAME, "pcloud-config");
/// ```
pub const CRATE_NAME: &str = "pcloud-config";

/// Profile class that pins transport and persistence policy.
///
/// The variant selected at runtime determines whether plaintext API
/// transport is rejected, whether group-readable config files are fatal,
/// and which set of [`api::ApiMode`] secure defaults apply.
///
/// Persists in the envelope's `profile.environment` field as a string
/// (`"Development"`, `"Test"`, or `"Production"`). Overridden at runtime by
/// `PCLOUD_ENV` (values `dev`/`development`, `test`, `prod`/`production`).
///
/// # Production rejection rules
///
/// When the active variant is [`Environment::Production`], the following
/// checks become hard errors rather than warnings:
///
/// | Rejected condition                                     | Enforced by                                 | Error variant                                       |
/// |--------------------------------------------------------|---------------------------------------------|-----------------------------------------------------|
/// | `api.mode == Plaintext` (TLS-mandatory)                | [`api::ApiEndpoint::validate`]              | [`ConfigError::InvalidApiEndpoint`]                 |
/// | `api.host` empty in plaintext/TLS                      | [`api::ApiEndpoint::validate`]              | [`ConfigError::InvalidApiEndpoint`]                 |
/// | `api.port == 0` in plaintext/TLS                       | [`api::ApiEndpoint::validate`]              | [`ConfigError::InvalidApiEndpoint`]                 |
/// | `api.server_name` empty in TLS                         | [`api::ApiEndpoint::validate`]              | [`ConfigError::InvalidApiEndpoint`]                 |
/// | `connect_timeout_ms == 0` / `read_timeout_ms == 0`     | [`api::ApiEndpoint::validate`]              | [`ConfigError::InvalidApiEndpoint`]                 |
/// | Config file has any group/other permission bit         | [`ConfigProfile::load_with_validation`]     | [`ConfigError::InsecureConfigPermissions`]          |
/// | Any managed directory mode has group/other bits set    | [`runtime::RuntimePolicy::validate`]        | [`ConfigError::InsecureMode`]                       |
///
/// The same checks emit non-fatal warnings under
/// [`Environment::Development`] / [`Environment::Test`] so operators can
/// exercise the full load path without relaxing production posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Environment {
    /// Loose posture used for local workstations and fixtures. Plaintext
    /// API transport is permitted, group/world-readable config files
    /// produce [`LoadedProfile::warnings`] entries instead of hard errors,
    /// and [`api::ApiMode::secure_default_for`] returns
    /// [`api::ApiMode::Development`]. Never appropriate for deployed
    /// systems — nothing in this variant asserts TLS or owner-only files.
    Development,
    /// Behaves like [`Environment::Development`] for transport/permission
    /// enforcement but signals "automated test harness" to downstream
    /// tooling (e.g. suppresses interactive prompts). Tests that need
    /// strict posture — for example to verify that plaintext is rejected
    /// or that port=0 fails — should construct [`Environment::Production`]
    /// explicitly rather than rely on Test.
    Test,
    /// Strict posture for deployed systems. Plaintext API transport is
    /// refused ([`api::ApiMode::Tls`] is the only acceptable wire mode),
    /// `port == 0` is refused, empty `host`/`server_name` are refused,
    /// zero-valued timeouts are refused, and any config file with
    /// group/world permission bits (`mode & 0o077 != 0`) is refused with
    /// [`ConfigError::InsecureConfigPermissions`]. All secure defaults
    /// assume a multi-user system with untrusted local neighbours.
    Production,
}

/// Fully validated configuration profile consumed by the daemon, SDK, and
/// CLI.
///
/// Constructed via [`ConfigProfile::secure_defaults`] (in memory) or
/// [`ConfigProfile::load`] (from disk). Never weaken a profile after
/// validation — re-run [`ConfigProfile::validate`] after any mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigProfile {
    /// Active [`Environment`] — controls TLS enforcement and permission
    /// posture.
    pub environment: Environment,
    /// Managed on-disk directories (config/state/runtime/cache).
    pub paths: ManagedPaths,
    /// API endpoint binding (host, port, transport mode, timeouts).
    pub api: ApiEndpoint,
    /// Plugin loader policy (disabled by default).
    pub extensions: ExtensionPolicy,
    /// Required permission modes for each managed directory.
    pub runtime: RuntimePolicy,
    /// Product feature flag toggles.
    pub features: FeatureFlags,
    /// Resource-use bounds (concurrency, parser frame size).
    pub limits: ResourceLimits,
    /// FUSE mount policy (owner-only by default).
    pub mount: MountPolicy,
    /// Observability / telemetry toggles. Defaults via serde if missing
    /// from disk (v1 documents).
    #[serde(default)]
    pub observability: ObservabilityFlags,
    /// Resilience policy (rate limit, breaker, retry). Defaults via serde
    /// if missing from disk.
    #[serde(default)]
    pub resilience: ResiliencePolicy,
    /// Data-residency allow-list policy. Defaults via serde to
    /// "allow all regions, non-strict" so older envelopes still load.
    #[serde(default)]
    pub data_residency: DataResidencyPolicy,
    /// Auth-subsystem policy (vault backend selection). Optional on
    /// disk — older envelopes default to [`AuthPolicy::default`], which
    /// is `backend = VaultBackend::Auto`.
    #[serde(default)]
    pub auth: AuthPolicy,
    /// IPC-layer per-category rate limit policy. Defaults via
    /// `#[serde(default)]` so older envelopes load with
    /// [`rate_limit::RateLimitPolicy::secure_defaults`].
    #[serde(default)]
    pub rate_limit: RateLimitPolicy,
    /// Tier-2 active-passive HA policy (file-lock lease handoff). Opt-in:
    /// disabled by default so the daemon behaves identically to the
    /// single-instance model. See [`ha::HaPolicy`] and
    /// `docs/enterprise/ha.md` §4.2.
    #[serde(default)]
    pub ha: ha::HaPolicy,
    /// Optional `[crypto]` section with an embedded
    /// [`crypto_kms::CryptoKmsConfig`]. When absent the daemon wires
    /// `pcloud_kms::NullKms` (legacy local-Argon2 DEK path). When
    /// present, the daemon constructs the matching provider and injects
    /// it into the `CryptoShell` via `set_kms_provider`.
    #[serde(default)]
    pub crypto: CryptoConfig,
    /// Optional `[file_history]` section configuring the
    /// [`file_history::FileHistoryConfig::revision_url`] that the daemon
    /// uses to resolve `log` / `diff` / `restore`. When absent or empty,
    /// the daemon wires a `NullRevisionProvider` and returns a structured
    /// "not configured" response on every call.
    #[serde(default)]
    pub file_history: FileHistoryConfig,
    /// Optional `[upgrade]` section controlling the daemon-handoff
    /// timing used by rolling upgrades. Defaults via
    /// `#[serde(default)]` so older envelopes load cleanly. See
    /// [`upgrade::UpgradePolicy`] and
    /// `docs/book/src/operations/upgrade.md`.
    #[serde(default)]
    pub upgrade: UpgradePolicy,
    /// Optional `[sync]` section controlling the background sync loop.
    /// When absent, the loop is enabled with default poll intervals.
    /// See [`sync_loop::SyncLoopConfig`].
    #[serde(default)]
    pub sync_loop: SyncLoopConfig,
}

/// Errors returned by profile construction, validation, and loading.
///
/// Every variant is non-recoverable at this layer: callers either surface
/// the error to the user or fall back to [`ConfigProfile::secure_defaults`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A managed directory was provided as a relative path. Only absolute
    /// paths are accepted so behaviour does not depend on CWD.
    #[error("managed path '{field}' must be absolute")]
    PathMustBeAbsolute {
        /// Name of the offending field (e.g. `"config_dir"`).
        field: &'static str,
    },
    /// A directory mode contained group or other permission bits.
    /// Rejected unconditionally — managed directories must be `0700` or
    /// stricter.
    #[error("runtime mode for '{field}' is too permissive: {mode:o}")]
    InsecureMode {
        /// Name of the offending field (e.g. `"state_dir_mode"`).
        field: &'static str,
        /// The offending Unix mode, reported in octal.
        mode: u32,
    },
    /// `mount.allow_other` is set alongside `mount.owner_only_by_default`;
    /// those are mutually exclusive.
    #[error("allow_other requires explicit opt-in to disable owner-only mode")]
    InvalidMountPolicy,
    /// API endpoint configuration is internally inconsistent. See the
    /// embedded reason string for details.
    #[error("api endpoint is invalid: {0}")]
    InvalidApiEndpoint(&'static str),
    /// Extension policy is internally inconsistent (e.g. capability grants
    /// without `plugins_enabled`).
    #[error("extension policy is invalid: {0}")]
    InvalidExtensionPolicy(&'static str),
    /// `[crypto]` section is internally inconsistent — most commonly
    /// `mode = "kms"` with no or explicitly `null` `[crypto.kms]`
    /// provider, which would silently downgrade to the master-key
    /// path. Bootstrap refuses to start in this state.
    #[error("crypto config is invalid: {0}")]
    InvalidCryptoConfig(&'static str),
    /// `[sync]` section has out-of-range values.
    #[error("sync loop config is invalid: {0}")]
    InvalidSyncLoopConfig(&'static str),
    /// An enum-style env variable held an unrecognised value.
    #[error("invalid environment value '{value}' for {name}")]
    InvalidEnvironmentValue {
        /// Name of the env var or field that rejected the value.
        name: &'static str,
        /// The offending raw value as provided.
        value: String,
    },
    /// I/O failure while reading config metadata or contents.
    #[error("config file I/O error: {0}")]
    Io(String),
    /// The config file exists but is not parseable JSON.
    #[error("config file is not valid JSON: {0}")]
    InvalidJson(String),
    /// The config file parses as JSON but violates the envelope schema.
    /// The wrapped message lists each violation with its JSON pointer.
    #[error("config file failed schema validation: {0}")]
    SchemaViolations(String),
    /// A [`migrate::MigrationError`] bubbled through the loader.
    #[error("config file migration failed: {0}")]
    Migration(String),
    /// The config file mode has group or other permission bits set and
    /// the load options forbid that. Override with `--insecure-config`
    /// only in development.
    #[error(
        "config file '{path}' has insecure permissions (mode {mode:o}): refusing to load. \
         Pass --insecure-config to override in development."
    )]
    InsecureConfigPermissions {
        /// Display path of the offending file.
        path: String,
        /// The offending Unix mode, reported in octal.
        mode: u32,
    },
}

impl ConfigProfile {
    /// Produce a secure-by-default [`ConfigProfile`] rooted at `root`.
    ///
    /// - Owner-only (`0700`) mode on every managed directory.
    /// - Crypto enabled, durable auth tokens opt-in only.
    /// - Production profile pins TLS transport.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use pcloud_config::{ConfigProfile, Environment};
    /// let p = ConfigProfile::secure_defaults(PathBuf::from("/tmp/x"), Environment::Production);
    /// assert_eq!(p.runtime.state_dir_mode, 0o700);
    /// assert!(!p.mount.allow_other);
    /// ```
    #[must_use]
    pub fn secure_defaults(root: PathBuf, environment: Environment) -> Self {
        Self {
            environment,
            paths: ManagedPaths {
                config_dir: root.join("config"),
                state_dir: root.join("state"),
                runtime_dir: root.join("runtime"),
                cache_dir: root.join("cache"),
            },
            api: ApiEndpoint::secure_defaults(environment),
            extensions: ExtensionPolicy::secure_defaults(root.join("plugins")),
            runtime: RuntimePolicy {
                socket_dir_mode: 0o700,
                state_dir_mode: 0o700,
                config_dir_mode: 0o700,
                cache_dir_mode: 0o700,
            },
            features: FeatureFlags {
                p2p_enabled: false,
                crypto_enabled: true,
                durable_auth_tokens_enabled: false,
                integrity_sweeper: crate::integrity_sweeper::IntegritySweeperConfig::default(),
                audit_verifier: crate::audit_verifier::AuditVerifierConfig::default(),
            },
            limits: ResourceLimits {
                max_concurrent_uploads: 4,
                max_concurrent_downloads: 4,
                max_parser_frame_bytes: 8 * 1024 * 1024,
            },
            mount: MountPolicy {
                allow_other: false,
                owner_only_by_default: true,
                cache_size_mb: MountPolicy::DEFAULT_CACHE_SIZE_MB,
                page_cache_entries: MountPolicy::DEFAULT_PAGE_CACHE_ENTRIES,
                metadata_ttl_secs: MountPolicy::DEFAULT_METADATA_TTL_SECS,
            },
            observability: ObservabilityFlags::secure_defaults(),
            resilience: ResiliencePolicy::secure_defaults(),
            data_residency: DataResidencyPolicy::default(),
            auth: AuthPolicy::default(),
            rate_limit: RateLimitPolicy::secure_defaults(),
            ha: ha::HaPolicy::secure_defaults(),
            crypto: CryptoConfig::default(),
            file_history: FileHistoryConfig::default(),
            upgrade: UpgradePolicy::secure_defaults(),
            sync_loop: SyncLoopConfig::default(),
        }
    }

    /// Validate every sub-policy in the profile. Returns the first
    /// violation encountered; no partial success.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use pcloud_config::{ConfigProfile, Environment};
    /// let p = ConfigProfile::secure_defaults(PathBuf::from("/tmp/x"), Environment::Development);
    /// assert!(p.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.paths.validate()?;
        self.api.validate(self.environment)?;
        self.extensions.validate()?;
        self.runtime.validate()?;
        self.file_history.validate(self.environment)?;
        self.crypto
            .validate()
            .map_err(ConfigError::InvalidCryptoConfig)?;

        if self.mount.allow_other && self.mount.owner_only_by_default {
            return Err(ConfigError::InvalidMountPolicy);
        }

        self.sync_loop
            .validate()
            .map_err(ConfigError::InvalidSyncLoopConfig)?;

        Ok(())
    }
}

/// Funnel [`ConfigError`] into the workspace-wide unified error taxonomy so
/// SDK/CLI surfaces can yield a single error type at the boundary. The
/// original [`ConfigError`] is preserved as the `source` of the unified
/// error, and the category is always [`pcloud_error::Category::Config`].
impl From<ConfigError> for pcloud_error::Error {
    fn from(err: ConfigError) -> Self {
        use pcloud_error::IntoUnified;
        err.into_unified(pcloud_error::Category::Config)
    }
}

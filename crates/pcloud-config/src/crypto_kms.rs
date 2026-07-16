//! Enterprise KMS configuration attached to a [`crate::ConfigProfile`].
//!
//! Persists in the envelope's optional `profile.crypto.kms` object. When
//! absent the daemon wires `NullKms` (the legacy local-Argon2 DEK path).
//! When present, the daemon constructs the matching provider
//! (`pcloud_kms::AwsKms` / `HashicorpVault` / `Pkcs11Hsm`) and injects
//! it into `CryptoShell` via `set_kms_provider` before `start`.
//!
//! The config struct is **declarative** — it does **not** hold secrets.
//! Vault tokens and PKCS#11 PINs are pulled from environment variables
//! named in the config, so the on-disk profile can stay world-unreadable
//! without also carrying the token.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Top-level `[crypto]` section of the profile.
///
/// Two knobs today: `mode` selects the DEK source for the sector path
/// (legacy Argon2-derived master key vs KMS-wrapped DEK) and `kms`
/// carries the optional KMS provider selector. Future crypto-related
/// runtime knobs (e.g. sector size override, wrapping algorithm) would
/// attach here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CryptoConfig {
    /// DEK-sourcing mode for the sector-encryption path.
    ///
    /// - [`CryptoMode::Raw`] (default) — legacy: per-file keys are
    ///   derived from the Argon2id master key.
    /// - [`CryptoMode::Kms`] — enterprise: DEK is wrapped by the
    ///   provider in `kms` below. Bootstrap refuses `Kms` without
    ///   a matching `[crypto.kms]` block.
    #[serde(default)]
    pub mode: CryptoMode,
    /// Optional KMS integration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms: Option<CryptoKmsConfig>,
}

/// DEK-sourcing mode mirror of `pcloud_crypto::CryptoMode`, as a
/// plain config enum so the profile file stays declarative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CryptoMode {
    /// Legacy Argon2-derived master-key path. Default for single-user
    /// deployments. Works even with `NullKms`.
    #[default]
    Raw,
    /// KMS-wrapped DEK path. Bootstrap rejects this selection when
    /// `[crypto.kms]` is absent or set to `Null`.
    Kms,
}

impl CryptoMode {
    /// Short safe-to-log tag (`"raw"` / `"kms"`).
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            CryptoMode::Raw => "raw",
            CryptoMode::Kms => "kms",
        }
    }
}

impl CryptoConfig {
    /// Cross-field validation.
    ///
    /// Refuses `mode = "kms"` when no [`CryptoKmsConfig`] is set, or
    /// when the configured provider is the explicit [`CryptoKmsConfig::Null`]
    /// (which cannot actually wrap a DEK). This is the hard bootstrap
    /// error called out in the task: misconfigured `kms` mode fails
    /// loudly at config load time instead of silently downgrading to
    /// the Argon2 path.
    ///
    /// # Errors
    /// Returns a static reason string — mapped to the caller's error
    /// taxonomy (`ConfigError::SchemaInvalid` at bootstrap).
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Some(k) = &self.kms {
            k.validate()?;
        }
        if matches!(self.mode, CryptoMode::Kms) {
            match &self.kms {
                None => {
                    return Err("crypto.mode = \"kms\" requires a [crypto.kms] provider section");
                }
                Some(CryptoKmsConfig::Null) => {
                    return Err(
                        "crypto.mode = \"kms\" is incompatible with crypto.kms.provider = \"null\"",
                    );
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

/// `[crypto.kms]` provider selector plus per-provider parameters.
///
/// Exactly one provider should be selected. The daemon rejects an empty
/// or multiply-populated record during validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case")]
pub enum CryptoKmsConfig {
    /// Explicit "KMS disabled" choice. Behaves the same as omitting
    /// the `[crypto.kms]` section entirely but documents intent.
    Null,

    /// AWS KMS. Requires the `aws` Cargo feature at the pcloud-kms layer.
    Aws {
        /// Target AWS region, e.g. `us-east-1`.
        region: String,
        /// KMS key ARN or alias, e.g.
        /// `arn:aws:kms:us-east-1:123:key/abcd-…`.
        key_id: String,
    },

    /// HashiCorp Vault Transit. Requires the `vault` Cargo feature.
    Vault {
        /// Base URL (e.g. `https://vault.example.com:8200`).
        url: String,
        /// Transit key name (not path).
        transit_key: String,
        /// Env var that holds the Vault auth token. The token is
        /// **never** read from the config file itself.
        token_env: String,
    },

    /// PKCS#11 HSM. Requires the `pkcs11` Cargo feature.
    Pkcs11 {
        /// Absolute path to the vendor PKCS#11 shared library,
        /// e.g. `/usr/lib/softhsm/libsofthsm2.so`.
        module_path: String,
        /// Numeric slot id on the token.
        slot_id: u64,
        /// Env var that holds the user PIN. The PIN is **never** read
        /// from the config file itself.
        pin_env: String,
        /// `CKA_LABEL` of the wrapping AES key that must already exist
        /// inside the HSM.
        key_label: String,
    },
}

impl CryptoKmsConfig {
    /// Short, safe-to-log provider tag.
    ///
    /// ```
    /// use pcloud_config::crypto_kms::CryptoKmsConfig;
    /// assert_eq!(CryptoKmsConfig::Null.tag(), "null");
    /// ```
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            CryptoKmsConfig::Null => "null",
            CryptoKmsConfig::Aws { .. } => "aws",
            CryptoKmsConfig::Vault { .. } => "vault",
            CryptoKmsConfig::Pkcs11 { .. } => "pkcs11",
        }
    }

    /// Basic shape validation.
    ///
    /// Provider-specific round-trip checks (can we reach the KMS, is the
    /// key policy correct) happen in `pcloud-kms` at construction time.
    ///
    /// # Errors
    /// Returns a static reason string when a required field is empty
    /// (e.g. `region`, `url`, `module_path`). Callers map this into
    /// their own error taxonomy.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            CryptoKmsConfig::Null => Ok(()),
            CryptoKmsConfig::Aws { region, key_id } => {
                if region.is_empty() {
                    return Err("crypto.kms.aws.region must be non-empty");
                }
                if key_id.is_empty() {
                    return Err("crypto.kms.aws.key_id must be non-empty");
                }
                Ok(())
            }
            CryptoKmsConfig::Vault {
                url,
                transit_key,
                token_env,
            } => {
                if url.is_empty() {
                    return Err("crypto.kms.vault.url must be non-empty");
                }
                let parsed = url::Url::parse(url)
                    .map_err(|_| "crypto.kms.vault.url must be an absolute https URL")?;
                if parsed.scheme() != "https" {
                    return Err("crypto.kms.vault.url must use https");
                }
                if parsed.host_str().is_none() {
                    return Err("crypto.kms.vault.url must include a host");
                }
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return Err("crypto.kms.vault.url must not include credentials");
                }
                if transit_key.is_empty() {
                    return Err("crypto.kms.vault.transit_key must be non-empty");
                }
                if token_env.is_empty() {
                    return Err("crypto.kms.vault.token_env must be non-empty");
                }
                Ok(())
            }
            CryptoKmsConfig::Pkcs11 {
                module_path,
                pin_env,
                key_label,
                ..
            } => {
                if module_path.is_empty() {
                    return Err("crypto.kms.pkcs11.module_path must be non-empty");
                }
                if pin_env.is_empty() {
                    return Err("crypto.kms.pkcs11.pin_env must be non-empty");
                }
                if key_label.is_empty() {
                    return Err("crypto.kms.pkcs11.key_label must be non-empty");
                }
                Ok(())
            }
        }
    }
}

/// Errors returned by [`CryptoKmsConfig::build_provider`] when the
/// `kms-factory` feature is enabled.
#[cfg(feature = "kms-factory")]
#[derive(Debug, thiserror::Error)]
pub enum BuildProviderError {
    /// A required environment variable (pin / token) was not set or was
    /// empty. The name is included so the operator can fix their
    /// deployment.
    #[error("crypto.kms requires env var '{0}' to be set")]
    MissingEnv(String),
    /// The declarative config failed shape validation.
    #[error("crypto.kms config invalid: {0}")]
    InvalidConfig(&'static str),
    /// The provider constructor itself returned an error (network
    /// failure at init time, bad region, bad module path).
    #[error("crypto.kms provider init failed: {0}")]
    KmsInit(#[from] pcloud_kms::KmsError),
    /// The configured provider is not compiled into this build. The
    /// operator must rebuild `pcloud-kms` with the matching feature
    /// (`aws`, `vault`, or `pkcs11`).
    #[error("crypto.kms provider '{0}' is not compiled into this build")]
    ProviderFeatureDisabled(&'static str),
}

#[cfg(feature = "kms-factory")]
impl CryptoKmsConfig {
    /// Build a concrete [`pcloud_kms::KmsProvider`] from the declarative
    /// record.
    ///
    /// Secrets are **never** stored in the config itself — `Vault`
    /// and `Pkcs11` variants both reference an env var name (`token_env`
    /// / `pin_env`). This function reads that env var into a
    /// [`pcloud_secret::secret_string::SecretString`] so the secret
    /// never enters a non-zeroizing string.
    ///
    /// # Errors
    /// See [`BuildProviderError`].
    pub fn build_provider(&self) -> Result<Box<dyn pcloud_kms::KmsProvider>, BuildProviderError> {
        self.validate().map_err(BuildProviderError::InvalidConfig)?;

        match self {
            CryptoKmsConfig::Null => Ok(Box::new(pcloud_kms::NullKms)),

            CryptoKmsConfig::Aws { region, key_id: _ } => {
                #[cfg(feature = "aws-kms")]
                {
                    Ok(Box::new(pcloud_kms::AwsKms::new(region.clone())))
                }
                #[cfg(not(feature = "aws-kms"))]
                {
                    let _ = region;
                    Err(BuildProviderError::ProviderFeatureDisabled("aws"))
                }
            }

            CryptoKmsConfig::Vault {
                url,
                transit_key,
                token_env,
            } => {
                #[cfg(feature = "vault-kms")]
                {
                    let raw = std::env::var(token_env)
                        .map_err(|_| BuildProviderError::MissingEnv(token_env.clone()))?;
                    if raw.is_empty() {
                        return Err(BuildProviderError::MissingEnv(token_env.clone()));
                    }
                    let token = pcloud_secret::secret_string::SecretString::new(raw);
                    let v =
                        pcloud_kms::HashicorpVault::new(url.clone(), token, transit_key.clone())?;
                    Ok(Box::new(v))
                }
                #[cfg(not(feature = "vault-kms"))]
                {
                    let _ = (url, transit_key, token_env);
                    Err(BuildProviderError::ProviderFeatureDisabled("vault"))
                }
            }

            CryptoKmsConfig::Pkcs11 {
                module_path,
                slot_id,
                pin_env,
                key_label,
            } => {
                #[cfg(feature = "pkcs11-kms")]
                {
                    let raw = std::env::var(pin_env)
                        .map_err(|_| BuildProviderError::MissingEnv(pin_env.clone()))?;
                    if raw.is_empty() {
                        return Err(BuildProviderError::MissingEnv(pin_env.clone()));
                    }
                    let pin = pcloud_secret::secret_string::SecretString::new(raw);
                    let p = pcloud_kms::Pkcs11Hsm::new_from_module(
                        module_path,
                        *slot_id,
                        pin,
                        key_label,
                    )?;
                    Ok(Box::new(p))
                }
                #[cfg(not(feature = "pkcs11-kms"))]
                {
                    let _ = (module_path, slot_id, pin_env, key_label);
                    Err(BuildProviderError::ProviderFeatureDisabled("pkcs11"))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_tag() {
        assert_eq!(CryptoKmsConfig::Null.tag(), "null");
        assert!(CryptoKmsConfig::Null.validate().is_ok());
    }

    #[test]
    fn aws_missing_region() {
        let c = CryptoKmsConfig::Aws {
            region: String::new(),
            key_id: "arn:…".into(),
        };
        assert_eq!(c.tag(), "aws");
        assert!(c.validate().is_err());
    }

    #[test]
    fn vault_token_env_required() {
        let c = CryptoKmsConfig::Vault {
            url: "https://vault".into(),
            transit_key: "k".into(),
            token_env: String::new(),
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn vault_url_must_be_https() {
        let c = CryptoKmsConfig::Vault {
            url: "http://vault.example.com:8200".into(),
            transit_key: "k".into(),
            token_env: "PCLOUD_VAULT_TOKEN".into(),
        };
        assert_eq!(c.validate(), Err("crypto.kms.vault.url must use https"));
    }

    #[test]
    fn vault_url_must_be_absolute() {
        let c = CryptoKmsConfig::Vault {
            url: "vault.example.com".into(),
            transit_key: "k".into(),
            token_env: "PCLOUD_VAULT_TOKEN".into(),
        };
        assert_eq!(
            c.validate(),
            Err("crypto.kms.vault.url must be an absolute https URL")
        );
    }

    #[test]
    fn vault_url_accepts_https() {
        let c = CryptoKmsConfig::Vault {
            url: "https://vault.example.com:8200".into(),
            transit_key: "k".into(),
            token_env: "PCLOUD_VAULT_TOKEN".into(),
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn pkcs11_requires_all_fields() {
        let c = CryptoKmsConfig::Pkcs11 {
            module_path: "/lib/hsm.so".into(),
            slot_id: 0,
            pin_env: "PCLOUD_PKCS11_PIN".into(),
            key_label: "kek".into(),
        };
        assert_eq!(c.tag(), "pkcs11");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn round_trip_json() {
        let aws = CryptoKmsConfig::Aws {
            region: "us-east-1".into(),
            key_id: "arn:x".into(),
        };
        let json = serde_json::to_string(&aws).unwrap();
        let back: CryptoKmsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(aws, back);
    }

    #[test]
    fn crypto_config_default_has_no_kms() {
        let c: CryptoConfig = serde_json::from_str("{}").unwrap();
        assert!(c.kms.is_none());
    }
}

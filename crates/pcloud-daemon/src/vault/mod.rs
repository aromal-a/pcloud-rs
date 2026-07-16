//! **PLATFORM: all.** Cross-platform vault abstraction for the auth token.
//!
//! - `vault::file::FileVault`         — universal fallback (all platforms)
//! - `vault::keychain::KeychainVault` — macOS login keychain (tier 1)
//! - `vault::dpapi::DpapiVault`       — Windows 10/11 DPAPI (tier 1)
//! - `vault::secret_service::SecretServiceVault` — Linux Secret Service
//!   (tier 1, with file fallback)
//!
//! All four backends are real implementations — no `unimplemented!()`
//! stubs. Runtime selection is driven by
//! [`pcloud_config::auth::VaultBackend`] on the active
//! [`pcloud_config::ConfigProfile`]. When the value is
//! [`pcloud_config::auth::VaultBackend::Auto`] (the default), the daemon
//! picks the platform-native backend and falls back to `FileVault` on
//! init failure. Explicit values are honoured verbatim and error out
//! clearly on platform mismatch (e.g. `keychain` on Linux).
//!
//! The trait surface is deliberately narrow — load / store / clear /
//! backend_name — and mirrors what `auth_vault::{load_token, store_token,
//! clear_token}` already expose. The auth token is the only secret we
//! persist, so this trait does not try to generalize over arbitrary
//! secrets.
//!
//! # Security posture (ADR 0005, ADR 0007)
//!
//! Persisted auth tokens are opt-in (see ADR 0005, "durable auth
//! tokens"). Every backend MUST:
//!
//! - carry `AuthToken` as [`SecretString`] so zeroization on drop is
//!   enforced by the type system,
//! - redact the token in any `Debug` / log output,
//! - apply owner-only permissions (`0600` file, `0700` parent dir for
//!   `FileVault`; per-user scope for keychain / DPAPI / Secret Service),
//! - validate ownership and mode before returning a persisted token.
//!
//! Password persistence is intentionally not available through this
//! trait — see ADR 0007 ("no cleartext credential persistence"). The
//! legacy C client persisted passwords; the Rust rewrite marks that
//! behaviour as `Rejected` in the parity matrix.

use std::path::{Path, PathBuf};

use pcloud_config::auth::VaultBackend;
use pcloud_secret::secret_string::SecretString;

pub mod dpapi;
pub mod file;
pub mod keychain;
pub mod secret_service;
#[cfg(windows)]
mod windows_secure_file;

pub use file::FileVault;

/// The single auth token persisted on behalf of the current user.
///
/// Kept as a type alias over `SecretString` so callers never hold raw
/// `String` values. A future change may promote this to a newtype that
/// carries e.g. expiration metadata, but for Phase 0 it intentionally
/// mirrors the existing on-disk representation.
pub type AuthToken = SecretString;

/// Errors surfaced by any [`PlatformVault`] implementation.
pub type VaultError = crate::auth_vault::AuthVaultError;

/// Result alias used by [`PlatformVault`] methods.
pub type Result<T> = std::result::Result<T, VaultError>;

/// Cross-platform auth token vault.
///
/// Implementations MUST:
/// - return `Ok(None)` from [`PlatformVault::load`] if no token has ever
///   been stored (i.e. "no vault yet" is not an error),
/// - refuse to return a token that fails the backend's integrity /
///   ownership checks,
/// - never log, `Debug`-print, or otherwise leak the token bytes,
/// - treat [`PlatformVault::clear`] as idempotent (no error if nothing
///   was stored).
pub trait PlatformVault: Send + Sync + 'static {
    /// Load the persisted auth token, if any.
    fn load(&self) -> Result<Option<AuthToken>>;

    /// Persist the given auth token, replacing any previous value.
    fn store(&self, token: &AuthToken) -> Result<()>;

    /// Remove any persisted auth token. Idempotent.
    fn clear(&self) -> Result<()>;

    /// Human-readable backend identifier, used for logs and diagnostics.
    fn backend_name(&self) -> &'static str;
}

/// Host-family discriminator used by the [`VaultBackend::Auto`] picker.
///
/// Exposed as a small enum (rather than direct `cfg!` checks) so the
/// selection logic can be unit-tested with synthetic hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFamily {
    /// `target_os = "macos"`.
    MacOs,
    /// `target_os = "windows"`.
    Windows,
    /// `target_os = "linux"`.
    Linux,
    /// FreeBSD / OpenBSD / NetBSD / DragonflyBSD.
    Bsd,
    /// Any other Unix (Solaris, illumos, …). Routes to [`FileVault`].
    OtherUnix,
}

impl HostFamily {
    /// Detect the host family at compile time for the current build target.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(windows)]
        {
            Self::Windows
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(any(
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            Self::Bsd
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "linux",
            windows,
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )))]
        {
            Self::OtherUnix
        }
    }
}

/// Selection errors surfaced by [`select_vault`].
#[derive(Debug, thiserror::Error)]
pub enum VaultSelectError {
    /// The caller explicitly requested a backend that cannot run on the
    /// current build target (e.g. `keychain` on Linux).
    #[error(
        "vault backend '{requested}' is not supported on this platform (host: {host:?}). \
         Use 'auto' to pick the platform-native backend or 'file' for the portable fallback."
    )]
    UnsupportedOnPlatform {
        /// Requested backend name (kebab-case).
        requested: &'static str,
        /// Detected host family.
        host: HostFamily,
    },
}

/// Outcome of [`select_vault`]: the chosen backend plus the effective
/// [`VaultBackend`] name that was resolved. When `requested` was
/// [`VaultBackend::Auto`], the `effective` value reflects the actual
/// platform-native backend that was picked (or [`VaultBackend::File`] if
/// the platform-native backend failed to initialise).
pub struct VaultSelection {
    /// The backend instance, boxed behind the trait.
    pub vault: Box<dyn PlatformVault>,
    /// Effective (resolved) backend that was chosen.
    pub effective: VaultBackend,
    /// Non-fatal warning produced during selection (e.g. "Secret
    /// Service unavailable, falling back to FileVault"). Callers SHOULD
    /// log this at warn level.
    pub warning: Option<String>,
}

impl std::fmt::Debug for VaultSelection {
    /// Redacted `Debug` impl that never exposes the inner vault object
    /// beyond its stable `backend_name()` identifier — matching the
    /// project secret-hygiene rule that vault handles never leak
    /// structure into log output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultSelection")
            .field("backend", &self.vault.backend_name())
            .field("effective", &self.effective)
            .field("warning", &self.warning)
            .finish()
    }
}

/// Resolve a [`VaultBackend`] selection into a concrete boxed
/// [`PlatformVault`].
///
/// # Mapping
///
/// - [`VaultBackend::Auto`] picks the platform-native backend (macOS →
///   Keychain, Windows → DPAPI, Linux → Secret Service, BSD/Other → File).
///   When the platform-native backend fails to initialise, the function
///   logs a one-line warning in the returned `VaultSelection::warning`
///   and falls back to [`FileVault`].
/// - [`VaultBackend::File`] is honoured verbatim on every platform.
/// - [`VaultBackend::Keychain`] requires `target_os = "macos"`; any other
///   host returns [`VaultSelectError::UnsupportedOnPlatform`].
/// - [`VaultBackend::Dpapi`] requires `target_os = "windows"`; any other
///   host returns [`VaultSelectError::UnsupportedOnPlatform`].
/// - [`VaultBackend::SecretService`] requires `target_os = "linux"`; any
///   other host returns [`VaultSelectError::UnsupportedOnPlatform`].
///
/// # Security notes
///
/// - The `file_fallback_path` is used both for the `FileVault` (when the
///   selected backend is `File` or when `Auto` falls back) and as the
///   fallback ciphertext location for `DpapiVault`.
/// - Explicit requests NEVER fall back silently. Only `Auto` falls back,
///   and only with a surfaced warning string.
pub fn select_vault(
    requested: VaultBackend,
    file_fallback_path: &Path,
) -> std::result::Result<VaultSelection, VaultSelectError> {
    select_vault_for(requested, file_fallback_path, HostFamily::current())
}

/// Host-aware variant of [`select_vault`]. Used by the cross-platform
/// tests to exercise the selection logic for every host family without
/// recompiling.
///
/// On a given build, only the backends compiled for the target can be
/// *constructed*, so this helper honours the `host` discriminator only
/// when it matches the build target. Mismatched host values collapse to
/// the portable [`FileVault`] with a surfaced warning — this is purely
/// a test aid and is never exposed publicly.
#[doc(hidden)]
pub fn select_vault_for(
    requested: VaultBackend,
    file_fallback_path: &Path,
    host: HostFamily,
) -> std::result::Result<VaultSelection, VaultSelectError> {
    let file_fallback_path: PathBuf = file_fallback_path.to_path_buf();

    // Helper: produce the file-backed vault with no warning.
    let make_file = || VaultSelection {
        vault: Box::new(FileVault::new(file_fallback_path.clone())) as Box<dyn PlatformVault>,
        effective: VaultBackend::File,
        warning: None,
    };

    // Helper: fallback-with-warning (Auto path only).
    let make_file_fallback = |reason: String| VaultSelection {
        vault: Box::new(FileVault::new(file_fallback_path.clone())) as Box<dyn PlatformVault>,
        effective: VaultBackend::File,
        warning: Some(reason),
    };

    match requested {
        VaultBackend::File => Ok(make_file()),

        VaultBackend::Auto => match host {
            HostFamily::MacOs => {
                #[cfg(target_os = "macos")]
                {
                    let vault = keychain::KeychainVault::new(file_fallback_path.clone());
                    return Ok(VaultSelection {
                        vault: Box::new(vault) as Box<dyn PlatformVault>,
                        effective: VaultBackend::Keychain,
                        warning: None,
                    });
                }
                #[cfg(not(target_os = "macos"))]
                Ok(make_file_fallback(
                    "auto: host reported macOS but binary is not built for macOS; \
                     falling back to FileVault"
                        .to_owned(),
                ))
            }
            HostFamily::Windows => {
                #[cfg(windows)]
                {
                    let ciphertext_path = file_fallback_path.with_extension("dpapi");
                    let vault = dpapi::DpapiVault::new(ciphertext_path);
                    Ok(VaultSelection {
                        vault: Box::new(vault) as Box<dyn PlatformVault>,
                        effective: VaultBackend::Dpapi,
                        warning: None,
                    })
                }
                #[cfg(not(windows))]
                Ok(make_file_fallback(
                    "auto: host reported Windows but binary is not built for Windows; \
                     falling back to FileVault"
                        .to_owned(),
                ))
            }
            HostFamily::Linux => {
                #[cfg(target_os = "linux")]
                {
                    // Probe Secret Service availability; on failure log a
                    // one-line warning and fall back to FileVault. We
                    // attempt a connect to confirm a session D-Bus and
                    // Secret Service daemon are actually reachable;
                    // otherwise this Auto path would hang login attempts
                    // on headless hosts.
                    match ::secret_service::blocking::SecretService::connect(
                        ::secret_service::EncryptionType::Dh,
                    ) {
                        Ok(_) => Ok(VaultSelection {
                            vault: Box::new(self::secret_service::SecretServiceVault::new())
                                as Box<dyn PlatformVault>,
                            effective: VaultBackend::SecretService,
                            warning: None,
                        }),
                        Err(err) => Ok(make_file_fallback(format!(
                            "auto: Secret Service unavailable ({err}); falling back to FileVault"
                        ))),
                    }
                }
                #[cfg(not(target_os = "linux"))]
                Ok(make_file_fallback(
                    "auto: host reported Linux but binary is not built for Linux; \
                     falling back to FileVault"
                        .to_owned(),
                ))
            }
            HostFamily::Bsd | HostFamily::OtherUnix => Ok(make_file()),
        },

        VaultBackend::Keychain => {
            if !matches!(host, HostFamily::MacOs) {
                return Err(VaultSelectError::UnsupportedOnPlatform {
                    requested: "keychain",
                    host,
                });
            }
            #[cfg(target_os = "macos")]
            {
                let vault = keychain::KeychainVault::new(file_fallback_path.clone());
                Ok(VaultSelection {
                    vault: Box::new(vault) as Box<dyn PlatformVault>,
                    effective: VaultBackend::Keychain,
                    warning: None,
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                // `host` claims macOS but we're cross-compiled for a
                // different target — surface the mismatch as a hard
                // error rather than silently degrading.
                Err(VaultSelectError::UnsupportedOnPlatform {
                    requested: "keychain",
                    host,
                })
            }
        }

        VaultBackend::Dpapi => {
            if !matches!(host, HostFamily::Windows) {
                return Err(VaultSelectError::UnsupportedOnPlatform {
                    requested: "dpapi",
                    host,
                });
            }
            #[cfg(windows)]
            {
                let ciphertext_path = file_fallback_path.with_extension("dpapi");
                let vault = dpapi::DpapiVault::new(ciphertext_path);
                Ok(VaultSelection {
                    vault: Box::new(vault) as Box<dyn PlatformVault>,
                    effective: VaultBackend::Dpapi,
                    warning: None,
                })
            }
            #[cfg(not(windows))]
            Err(VaultSelectError::UnsupportedOnPlatform {
                requested: "dpapi",
                host,
            })
        }

        VaultBackend::SecretService => {
            if !matches!(host, HostFamily::Linux) {
                return Err(VaultSelectError::UnsupportedOnPlatform {
                    requested: "secret-service",
                    host,
                });
            }
            #[cfg(target_os = "linux")]
            {
                Ok(VaultSelection {
                    vault: Box::new(self::secret_service::SecretServiceVault::new())
                        as Box<dyn PlatformVault>,
                    effective: VaultBackend::SecretService,
                    warning: None,
                })
            }
            #[cfg(not(target_os = "linux"))]
            Err(VaultSelectError::UnsupportedOnPlatform {
                requested: "secret-service",
                host,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallback_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-vault-select-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn explicit_file_works_on_every_host() {
        let path = fallback_path();
        for host in [
            HostFamily::MacOs,
            HostFamily::Windows,
            HostFamily::Linux,
            HostFamily::Bsd,
            HostFamily::OtherUnix,
        ] {
            let sel = select_vault_for(VaultBackend::File, &path, host)
                .expect("file vault must never fail selection");
            assert_eq!(sel.effective, VaultBackend::File);
            assert_eq!(sel.vault.backend_name(), "file");
            assert!(sel.warning.is_none());
        }
    }

    #[test]
    fn auto_on_bsd_selects_file() {
        let sel = select_vault_for(VaultBackend::Auto, &fallback_path(), HostFamily::Bsd)
            .expect("auto on BSD must succeed");
        assert_eq!(sel.effective, VaultBackend::File);
        assert_eq!(sel.vault.backend_name(), "file");
    }

    #[test]
    fn auto_on_other_unix_selects_file() {
        let sel = select_vault_for(VaultBackend::Auto, &fallback_path(), HostFamily::OtherUnix)
            .expect("auto on other-unix must succeed");
        assert_eq!(sel.effective, VaultBackend::File);
    }

    #[test]
    fn explicit_keychain_on_non_macos_is_hard_error() {
        for host in [
            HostFamily::Windows,
            HostFamily::Linux,
            HostFamily::Bsd,
            HostFamily::OtherUnix,
        ] {
            let err = select_vault_for(VaultBackend::Keychain, &fallback_path(), host)
                .expect_err("keychain must reject non-macOS host");
            match err {
                VaultSelectError::UnsupportedOnPlatform { requested, host: h } => {
                    assert_eq!(requested, "keychain");
                    assert_eq!(h, host);
                }
            }
        }
    }

    #[test]
    fn explicit_dpapi_on_non_windows_is_hard_error() {
        for host in [
            HostFamily::MacOs,
            HostFamily::Linux,
            HostFamily::Bsd,
            HostFamily::OtherUnix,
        ] {
            let err = select_vault_for(VaultBackend::Dpapi, &fallback_path(), host)
                .expect_err("dpapi must reject non-windows host");
            match err {
                VaultSelectError::UnsupportedOnPlatform { requested, host: h } => {
                    assert_eq!(requested, "dpapi");
                    assert_eq!(h, host);
                }
            }
        }
    }

    #[test]
    fn explicit_secret_service_on_non_linux_is_hard_error() {
        for host in [
            HostFamily::MacOs,
            HostFamily::Windows,
            HostFamily::Bsd,
            HostFamily::OtherUnix,
        ] {
            let err = select_vault_for(VaultBackend::SecretService, &fallback_path(), host)
                .expect_err("secret-service must reject non-linux host");
            match err {
                VaultSelectError::UnsupportedOnPlatform { requested, host: h } => {
                    assert_eq!(requested, "secret-service");
                    assert_eq!(h, host);
                }
            }
        }
    }
}

// **PLATFORM:** all (Linux, BSD, macOS, Windows).
// **GATING:** `PcloudDirs::legacy_linux_home()` is Linux-only at runtime;
//             `PcloudDirs::migrate_from_legacy_if_needed()` is behind the
//             `PCLOUD_MIGRATE_LEGACY_PATHS=1` env opt-in.
//
//! Canonical on-disk path layout for the pcloud-rs Rust workspace.
//!
//! This module is the single source of truth for config/data/cache/runtime
//! directory discovery. It wraps the `directories` crate so the same code
//! paths Just Work on every supported platform:
//!
//! - **Linux / *BSD:** XDG Base Directory Specification
//!   (`$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` / `$XDG_CACHE_HOME` /
//!   `$XDG_RUNTIME_DIR` with `~/.config`, `~/.local/share`, `~/.cache`
//!   fallbacks).
//! - **macOS:** `~/Library/Application Support`, `~/Library/Caches`,
//!   `~/Library/Preferences`.
//! - **Windows:** `%APPDATA%\pcloud\pcloud-rs\config`,
//!   `%APPDATA%\pcloud\pcloud-rs\data`,
//!   `%LOCALAPPDATA%\pcloud\pcloud-rs\cache`.
//!
//! The legacy `~/.pcloud/` path is read-only and consulted only as a
//! migration source on Linux. New writes always land under the
//! XDG-canonical locations. Migration is never silent — opt in with
//! `PCLOUD_MIGRATE_LEGACY_PATHS=1` and call
//! [`PcloudDirs::migrate_from_legacy_if_needed`] explicitly.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// Absolute on-disk directories managed by the daemon.
///
/// Persisted as the `paths` block of the profile envelope. Every path
/// must be absolute (see [`ManagedPaths::validate`]) so behaviour never
/// depends on CWD. Platform defaults are discovered by
/// [`PcloudDirs::discover`] and projected with
/// [`PcloudDirs::to_managed_paths`]; the entire block can be
/// overridden by `PCLOUD_ROOT` (test/multi-instance isolation) via
/// [`crate::env::apply_env_overrides`].
///
/// In [`crate::Environment::Production`] the permission posture of
/// each directory is enforced by
/// [`crate::runtime::RuntimePolicy::validate`]: any group/other bit is
/// fatal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPaths {
    /// Persistent user configuration: profile envelope and auth-token
    /// vault. Maps to TOML/JSON key `paths.config_dir`. Default (Linux):
    /// `$XDG_CONFIG_HOME/pcloud/pcloud-rs`. Valid values: any absolute
    /// path. **Security:** must be `0700` in production
    /// ([`crate::runtime::RuntimePolicy`]); holds the auth-token vault
    /// file. Overridden as a side effect of `PCLOUD_ROOT`. Example:
    /// `config_dir = "/home/alice/.config/pcloud/pcloud-rs"`.
    pub config_dir: PathBuf,
    /// Persistent daemon state: SQLite store, audit log, sync DB. Maps
    /// to `paths.state_dir`. Default (Linux):
    /// `$XDG_DATA_HOME/pcloud/pcloud-rs`. Valid values: any absolute
    /// path. **Security:** must be `0700` in production; leak here
    /// exposes sync metadata and audit trail. Example:
    /// `state_dir = "/home/alice/.local/share/pcloud/pcloud-rs"`.
    pub state_dir: PathBuf,
    /// Per-boot runtime data: IPC socket, PID file. Maps to
    /// `paths.runtime_dir`. Default (Linux): `$XDG_RUNTIME_DIR/pcloud/pcloud-rs`
    /// when set, otherwise `<cache>/pcloud-rs-runtime`. Valid values:
    /// any absolute path. **Security:** must be `0700` in production —
    /// loosening this exposes the IPC control socket to other local
    /// users. Example: `runtime_dir = "/run/user/1000/pcloud/pcloud-rs"`.
    pub runtime_dir: PathBuf,
    /// Non-persistent cache: thumbnails, FUSE staging, transient blobs.
    /// Maps to `paths.cache_dir`. Default (Linux):
    /// `$XDG_CACHE_HOME/pcloud/pcloud-rs`. Valid values: any absolute
    /// path. **Security:** must be `0700` in production; FUSE staging
    /// may briefly hold plaintext of encrypted content. Example:
    /// `cache_dir = "/home/alice/.cache/pcloud/pcloud-rs"`.
    pub cache_dir: PathBuf,
}

impl ManagedPaths {
    /// Well-known socket path under the runtime dir used for the local
    /// IPC listener.
    ///
    /// ```
    /// use pcloud_config::{ConfigProfile, Environment};
    /// let p = ConfigProfile::secure_defaults(
    ///     std::env::temp_dir().join("pcloud-ipc-path"),
    ///     Environment::Development,
    /// );
    /// assert!(p.paths.ipc_socket_path().ends_with("pcloud.sock"));
    /// ```
    #[must_use]
    pub fn ipc_socket_path(&self) -> PathBuf {
        self.runtime_dir.join("pcloud.sock")
    }

    /// Path of the owner-only auth-token vault file used by the daemon.
    ///
    /// ```
    /// use pcloud_config::{ConfigProfile, Environment};
    /// let p = ConfigProfile::secure_defaults(
    ///     std::env::temp_dir().join("pcloud-vault-path"),
    ///     Environment::Development,
    /// );
    /// assert!(p.paths.auth_token_vault_path().ends_with("auth_token"));
    /// ```
    #[must_use]
    pub fn auth_token_vault_path(&self) -> PathBuf {
        self.config_dir.join("auth_token")
    }

    /// Reject any profile whose managed directories are not absolute.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use pcloud_config::paths::ManagedPaths;
    /// let bad = ManagedPaths {
    ///     config_dir: PathBuf::from("rel"),
    ///     state_dir: PathBuf::from("/ok"),
    ///     runtime_dir: PathBuf::from("/ok"),
    ///     cache_dir: PathBuf::from("/ok"),
    /// };
    /// assert!(bad.validate().is_err());
    /// ```
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, path) in [
            ("config_dir", &self.config_dir),
            ("state_dir", &self.state_dir),
            ("runtime_dir", &self.runtime_dir),
            ("cache_dir", &self.cache_dir),
        ] {
            if !path.is_absolute() {
                return Err(ConfigError::PathMustBeAbsolute { field });
            }
        }

        Ok(())
    }
}

/// Cross-platform directory discovery for pcloud-rs.
///
/// The fields are the *canonical* per-role directories for the current
/// user and platform. They are never subdirectories of a single `root` —
/// that pattern is reserved for `PCLOUD_ROOT` overrides (multi-instance
/// or test isolation). Use [`PcloudDirs::discover`] on first launch;
/// fall back to
/// [`ConfigProfile::secure_defaults`](crate::ConfigProfile::secure_defaults)
/// with an explicit root only when `PCLOUD_ROOT` is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcloudDirs {
    /// Persistent user configuration (TOML/JSON, auth-token vault, etc.).
    pub config: PathBuf,
    /// Persistent user data (SQLite store, audit log, state).
    pub data: PathBuf,
    /// Non-persistent cache (thumbnails, FUSE staging).
    pub cache: PathBuf,
    /// Per-boot runtime (IPC sockets, PID files). Only Linux provides a
    /// true tmpfs at `$XDG_RUNTIME_DIR`; on macOS/Windows this is a
    /// long-lived cache subdirectory.
    pub runtime: PathBuf,
}

// Qualifier tuple fed to `directories::ProjectDirs`. Kept as constants so
// every crate agrees on the same vendor/app identifier.
const PROJECT_QUALIFIER: &str = "com";
const PROJECT_ORG: &str = "pcloud";
const PROJECT_APP: &str = "pcloud-rs";

impl PcloudDirs {
    /// Discover canonical per-platform directories via the `directories`
    /// crate. Returns [`ConfigError::Io`] if no valid home directory can
    /// be resolved (e.g. `HOME` unset on Unix, no known-folder handle on
    /// Windows).
    ///
    /// # Per-platform resolution and fallbacks
    ///
    /// ## Linux / *BSD (XDG Base Directory Specification)
    ///
    /// - `config` → `$XDG_CONFIG_HOME/pcloud/pcloud-rs`. If `XDG_CONFIG_HOME`
    ///   is unset or empty, falls back to `$HOME/.config/pcloud/pcloud-rs`.
    /// - `data` → `$XDG_DATA_HOME/pcloud/pcloud-rs`. Fallback:
    ///   `$HOME/.local/share/pcloud/pcloud-rs`.
    /// - `cache` → `$XDG_CACHE_HOME/pcloud/pcloud-rs`. Fallback:
    ///   `$HOME/.cache/pcloud/pcloud-rs`.
    /// - `runtime` → `$XDG_RUNTIME_DIR/pcloud/pcloud-rs` when set (typically
    ///   a tmpfs `/run/user/<uid>`). When unset we do **not** invent a
    ///   tmpfs path; instead we fall back to
    ///   `<cache>/pcloud-rs-runtime`. Downstream callers must still apply
    ///   `0700` via [`crate::runtime::RuntimePolicy`].
    /// - Hard error: `HOME` unset and no `XDG_*_HOME` override →
    ///   [`ConfigError::Io`].
    ///
    /// ## macOS (Apple "standard directories")
    ///
    /// - `config` → `~/Library/Application Support/com.pcloud.pcloud-rs`.
    /// - `data` → `~/Library/Application Support/com.pcloud.pcloud-rs`
    ///   (same root as config on Apple platforms by convention).
    /// - `cache` → `~/Library/Caches/com.pcloud.pcloud-rs`.
    /// - `runtime` → macOS has no `$XDG_RUNTIME_DIR` analogue; falls back
    ///   to `<cache>/pcloud-rs-runtime` (long-lived, not a tmpfs).
    /// - Hard error: no resolvable home directory for the current user →
    ///   [`ConfigError::Io`].
    ///
    /// ## Windows (Known Folders)
    ///
    /// - `config` → `%APPDATA%\pcloud\pcloud-rs\config`.
    /// - `data` → `%APPDATA%\pcloud\pcloud-rs\data`.
    /// - `cache` → `%LOCALAPPDATA%\pcloud\pcloud-rs\cache`.
    /// - `runtime` → no runtime-dir concept on Windows; falls back to
    ///   `%LOCALAPPDATA%\pcloud\pcloud-rs\cache\pcloud-rs-runtime`.
    /// - Hard error: neither `%APPDATA%` nor `%LOCALAPPDATA%` resolvable
    ///   via the `SHGetKnownFolderPath` path → [`ConfigError::Io`].
    ///
    /// ```
    /// # use pcloud_config::paths::PcloudDirs;
    /// let dirs = PcloudDirs::discover().unwrap();
    /// assert!(dirs.config.is_absolute());
    /// assert!(dirs.data.is_absolute());
    /// assert!(dirs.cache.is_absolute());
    /// assert!(dirs.runtime.is_absolute());
    /// ```
    pub fn discover() -> Result<Self, ConfigError> {
        let proj = directories::ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORG, PROJECT_APP)
            .ok_or_else(|| {
                ConfigError::Io("no valid home directory (HOME / known folder missing)".to_string())
            })?;

        // `runtime_dir()` is `Some` only on Linux when `$XDG_RUNTIME_DIR`
        // is set. Fall back to a cache-resident subdirectory elsewhere.
        let runtime = proj
            .runtime_dir()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| proj.cache_dir().join("pcloud-rs-runtime"));

        Ok(Self {
            config: proj.config_dir().to_path_buf(),
            data: proj.data_dir().to_path_buf(),
            cache: proj.cache_dir().to_path_buf(),
            runtime,
        })
    }

    /// Legacy Linux home directory (`$HOME/.pcloud`). Returns `None` on
    /// non-Linux platforms or when `HOME` is unset. Consulted **only** as
    /// a read-only migration source.
    #[must_use]
    pub fn legacy_linux_home() -> Option<PathBuf> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".pcloud"))
    }

    /// Project the discovered directories into a [`ManagedPaths`] value
    /// suitable for [`ConfigProfile`](crate::ConfigProfile). No disk I/O
    /// is performed here; the caller is responsible for `mkdir -p` +
    /// `chmod`.
    #[must_use]
    pub fn to_managed_paths(&self) -> ManagedPaths {
        ManagedPaths {
            config_dir: self.config.clone(),
            state_dir: self.data.clone(),
            runtime_dir: self.runtime.clone(),
            cache_dir: self.cache.clone(),
        }
    }

    /// Copy (never move) legacy `~/.pcloud/` contents into the canonical
    /// XDG layout when the user has opted in with
    /// `PCLOUD_MIGRATE_LEGACY_PATHS=1`. No-op on non-Linux, when the
    /// legacy directory does not exist, when the opt-in env var is unset,
    /// or when the destination already contains data.
    pub fn migrate_from_legacy_if_needed(&self) -> Result<bool, ConfigError> {
        if std::env::var_os("PCLOUD_MIGRATE_LEGACY_PATHS")
            .map(|v| v != "1")
            .unwrap_or(true)
        {
            return Ok(false);
        }
        let Some(legacy) = Self::legacy_linux_home() else {
            return Ok(false);
        };
        if !legacy.is_dir() {
            return Ok(false);
        }
        let mut migrated_any = false;
        // Legacy layout was `root/{config,state,runtime,cache}`. Map each
        // subdirectory to its XDG-canonical destination. Skip whenever
        // the destination already has contents — never overwrite.
        for (src_name, dst) in [
            ("config", &self.config),
            ("state", &self.data),
            ("cache", &self.cache),
        ] {
            let src = legacy.join(src_name);
            if !src.is_dir() {
                continue;
            }
            if dst.is_dir()
                && std::fs::read_dir(dst)
                    .map(|mut it| it.next().is_some())
                    .unwrap_or(false)
            {
                continue;
            }
            copy_dir_recursive(&src, dst).map_err(|e| ConfigError::Io(e.to_string()))?;
            migrated_any = true;
        }
        Ok(migrated_any)
    }
}

/// Recursively copy `src` into `dst`, creating `dst` if necessary.
/// Symlinks are ignored (the legacy `~/.pcloud/` tree never contains any).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_absolute_paths() {
        let d = PcloudDirs::discover().expect("discover");
        assert!(d.config.is_absolute(), "config: {:?}", d.config);
        assert!(d.data.is_absolute(), "data: {:?}", d.data);
        assert!(d.cache.is_absolute(), "cache: {:?}", d.cache);
        assert!(d.runtime.is_absolute(), "runtime: {:?}", d.runtime);
    }

    #[test]
    fn to_managed_paths_round_trips() {
        // `validate()` requires each path to be absolute. On Unix `/a/cfg`
        // satisfies that; on Windows absolute paths need a drive letter
        // (`C:\a\cfg`) or UNC prefix. Pick the right shape per target.
        #[cfg(unix)]
        let (cfg, data, cache, runtime) = (
            PathBuf::from("/a/cfg"),
            PathBuf::from("/a/data"),
            PathBuf::from("/a/cache"),
            PathBuf::from("/a/run"),
        );
        #[cfg(windows)]
        let (cfg, data, cache, runtime) = (
            PathBuf::from(r"C:\a\cfg"),
            PathBuf::from(r"C:\a\data"),
            PathBuf::from(r"C:\a\cache"),
            PathBuf::from(r"C:\a\run"),
        );
        let d = PcloudDirs {
            config: cfg,
            data,
            cache,
            runtime,
        };
        let mp = d.to_managed_paths();
        assert_eq!(mp.config_dir, d.config);
        assert_eq!(mp.state_dir, d.data);
        assert_eq!(mp.cache_dir, d.cache);
        assert_eq!(mp.runtime_dir, d.runtime);
        mp.validate().unwrap();
    }

    #[test]
    fn legacy_linux_home_gated_by_target() {
        let got = PcloudDirs::legacy_linux_home();
        if cfg!(target_os = "linux") {
            if std::env::var_os("HOME").is_some() {
                let p = got.expect("HOME set -> Some");
                assert!(p.ends_with(".pcloud"));
            }
        } else {
            assert!(got.is_none());
        }
    }
}

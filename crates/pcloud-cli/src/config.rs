//! User-facing CLI config file at `~/.pcloud/config.toml` (or a path
//! supplied via `--config <path>`). Holds **non-secret** defaults for
//! every option that `pcloudc login` (and a few other commands) accept
//! on the command line. Secrets — passwords, auth tokens, recovery
//! codes — are NEVER persisted here; they live only in the auth-token
//! vault (opt-in) or in transient `SecretString`s in memory.
//!
//! The file is auto-created on first invocation with all defaults
//! commented in TOML so users can edit interactively. It is loaded on
//! every CLI run; CLI flags always override the file.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// All persistent, non-secret CLI defaults.
///
/// Field-level docs become the comment block in the auto-generated
/// TOML so users have inline help when they edit the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliConfig {
    /// Default pCloud account username. Overridden by `-u <name>`.
    pub username: Option<String>,
    /// Where to mount pCloud Drive when `pcloudc login -m` is given
    /// without a path, or when no `-m` flag is given at all. When
    /// neither this nor the flag is set, defaults to `~/pCloudDrive`.
    pub mountpoint: Option<PathBuf>,
    /// FUSE mount options string, e.g. `"nodev,nosuid"`.
    /// Note: `allow_other` / `allow_root` are intentionally ignored
    /// here — the daemon's mount-policy validator refuses them
    /// regardless of what's in this file.
    pub fuse_opts: Option<String>,
    /// Path to the daemon's debug log. Default: `~/.pcloud/state/daemon.log`.
    pub log_path: Option<PathBuf>,
    /// Path to the filesystem-event log. Default: disabled (`None`).
    pub fs_event_log: Option<PathBuf>,
    /// Daemon logging verbosity. One of: `error`, `warn`, `info`,
    /// `debug`, `trace`. Defaults to `warn`.
    pub log_level: Option<String>,
    /// When `true`, `pcloudc login` will request that pCloud trust
    /// this device after a successful 2FA so subsequent logins skip
    /// the challenge. Equivalent to `-r/--trust-device`.
    pub trust_device: bool,
    /// When `true`, `pcloudc login` will treat the account password
    /// as the crypto-folder password too (skips the second prompt).
    /// Equivalent to `-y/--passascrypto`.
    pub passascrypto: bool,
    /// When `true`, `pcloudc login` will turn on the auth-token vault
    /// after success so the daemon can auto-restore the session on
    /// restart. **The vault stores the long-lived auth token, NOT
    /// the password.** Equivalent to `-s/--save-password`.
    pub save_password: bool,
    /// When `true`, `pcloudc login` prompts for the crypto password
    /// after auth and unlocks the crypto folder. Equivalent to
    /// `-c/--crypto`.
    pub crypto: bool,
    /// Maximum local cache size in **gigabytes**. Mirrors C
    /// `pcloud-rs --cache-size <GB>`. Default 5 GiB. Takes effect on
    /// the next daemon start.
    pub cache_size_gb: Option<u64>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            username: None,
            mountpoint: None,
            fuse_opts: None,
            log_path: None,
            fs_event_log: None,
            log_level: Some("warn".to_owned()),
            trust_device: false,
            passascrypto: false,
            save_password: false,
            crypto: false,
            cache_size_gb: Some(5),
        }
    }
}

impl CliConfig {
    /// **PLATFORM:** all. Resolve the config file path: explicit
    /// `--config` wins, then `${PCLOUD_CONFIG}` env var, otherwise the
    /// XDG-canonical `<config_dir>/config.toml` (e.g.
    /// `~/.config/pcloud-rs/config.toml` on Linux, `~/Library/Preferences/…`
    /// on macOS, `%APPDATA%\pcloud\pcloud-rs\config\config.toml` on
    /// Windows). Legacy `~/.pcloud/config.toml` is consulted read-only
    /// when it exists and the XDG location does not.
    pub fn default_path(explicit: Option<&Path>) -> PathBuf {
        if let Some(p) = explicit {
            return p.to_path_buf();
        }
        if let Some(p) = std::env::var_os("PCLOUD_CONFIG") {
            return PathBuf::from(p);
        }
        match pcloud_config::paths::PcloudDirs::discover() {
            Ok(dirs) => {
                let xdg = dirs.config.join("config.toml");
                // Legacy fallback (Linux only) when the user has not yet
                // migrated: read from the old location so `pcloudc` does
                // not suddenly "forget" the existing config. New writes
                // still land at `xdg` once the file is re-saved.
                if !xdg.exists()
                    && let Some(legacy) = pcloud_config::paths::PcloudDirs::legacy_linux_home()
                {
                    let legacy_file = legacy.join("config.toml");
                    if legacy_file.exists() {
                        return legacy_file;
                    }
                }
                xdg
            }
            Err(_) => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                home.join(".pcloud").join("config.toml")
            }
        }
    }

    /// Load from `path`, creating it with defaults if missing.
    /// Permissions: the file is forced to mode 0644 (non-secret) and
    /// the parent dir to 0700.
    pub fn load_or_init(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            // Unix-only 0700 parent chmod; Windows inherits ACLs from the
            // creating user (bd-xplat-windows tracks native ACL hardening).
            #[cfg(unix)]
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
        if !path.exists() {
            let cfg = Self::default();
            cfg.write_with_comments(path)?;
            return Ok(cfg);
        }
        let raw = fs::read_to_string(path)?;
        Ok(Self::parse_toml(&raw))
    }

    /// Write the file with leading `# …` comments per field so the
    /// human-edited file is self-documenting.
    pub fn write_with_comments(&self, path: &Path) -> std::io::Result<()> {
        let body = self.render_toml();
        let mut opts = fs::OpenOptions::new();
        opts.create(true).truncate(true).write(true);
        // Unix-only 0644 mode; Windows ACL path handled by bd-xplat-windows.
        #[cfg(unix)]
        opts.mode(0o644);
        let mut f = opts.open(path)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        Ok(())
    }

    fn render_toml(&self) -> String {
        let mut s = String::new();
        s.push_str("# pcloudc config file. Auto-generated; safe to edit.\n");
        s.push_str("# Secrets (passwords, tokens) are NEVER stored here.\n");
        s.push_str("# CLI flags always override values in this file.\n\n");

        write_opt_str(
            &mut s,
            "username",
            "Default pCloud account name (-u override).",
            self.username.as_deref(),
        );
        write_opt_path(
            &mut s,
            "mountpoint",
            "Mount path for `pcloudc login -m`. Defaults to ~/pCloudDrive when unset.",
            self.mountpoint.as_deref(),
        );
        write_opt_str(
            &mut s,
            "fuse_opts",
            "FUSE mount options. `allow_other`/`allow_root` are silently rejected.",
            self.fuse_opts.as_deref(),
        );
        write_opt_path(
            &mut s,
            "log_path",
            "Daemon log path (default: ~/.pcloud/state/daemon.log).",
            self.log_path.as_deref(),
        );
        write_opt_path(
            &mut s,
            "fs_event_log",
            "Filesystem-event log path (default: disabled).",
            self.fs_event_log.as_deref(),
        );
        write_opt_str(
            &mut s,
            "log_level",
            "Daemon log verbosity: error|warn|info|debug|trace.",
            self.log_level.as_deref(),
        );
        write_bool(
            &mut s,
            "trust_device",
            "Tell pCloud to trust this device after 2FA.",
            self.trust_device,
        );
        write_bool(
            &mut s,
            "passascrypto",
            "Treat account password as crypto password.",
            self.passascrypto,
        );
        write_bool(
            &mut s,
            "save_password",
            "Enable auth-token vault after login (token only, NOT password).",
            self.save_password,
        );
        write_bool(
            &mut s,
            "crypto",
            "Prompt for crypto password and unlock after login.",
            self.crypto,
        );
        write_opt_u64(
            &mut s,
            "cache_size_gb",
            "Local cache size in gigabytes (default 5). Takes effect on next daemon start.",
            self.cache_size_gb,
        );
        s
    }

    /// Minimal TOML parser tolerant of comments / blank lines /
    /// `key = "value"` and `key = true|false` only. We hand-roll to
    /// avoid pulling in serde_toml as a runtime dep for the CLI.
    fn parse_toml(raw: &str) -> Self {
        let mut out = Self::default();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let val = v.trim().trim_matches('"');
            match key {
                "username" => out.username = nonempty(val),
                "mountpoint" => out.mountpoint = nonempty(val).map(PathBuf::from),
                "fuse_opts" => out.fuse_opts = nonempty(val),
                "log_path" => out.log_path = nonempty(val).map(PathBuf::from),
                "fs_event_log" => out.fs_event_log = nonempty(val).map(PathBuf::from),
                "log_level" => out.log_level = nonempty(val),
                "trust_device" => out.trust_device = parse_bool(val),
                "passascrypto" => out.passascrypto = parse_bool(val),
                "save_password" => out.save_password = parse_bool(val),
                "crypto" => out.crypto = parse_bool(val),
                "cache_size_gb" => out.cache_size_gb = nonempty(val).and_then(|s| s.parse().ok()),
                _ => {}
            }
        }
        out
    }
}

fn nonempty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_owned())
    }
}

fn parse_bool(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on")
}

fn write_opt_str(buf: &mut String, key: &str, doc: &str, val: Option<&str>) {
    buf.push_str("# ");
    buf.push_str(doc);
    buf.push('\n');
    match val {
        Some(v) => buf.push_str(&format!("{key} = \"{v}\"\n\n")),
        None => buf.push_str(&format!("# {key} = \"\"\n\n")),
    }
}

fn write_opt_path(buf: &mut String, key: &str, doc: &str, val: Option<&Path>) {
    write_opt_str(buf, key, doc, val.and_then(|p| p.to_str()));
}

fn write_bool(buf: &mut String, key: &str, doc: &str, val: bool) {
    buf.push_str("# ");
    buf.push_str(doc);
    buf.push('\n');
    buf.push_str(&format!("{key} = {val}\n\n"));
}

fn write_opt_u64(buf: &mut String, key: &str, doc: &str, val: Option<u64>) {
    buf.push_str("# ");
    buf.push_str(doc);
    buf.push('\n');
    match val {
        Some(v) => buf.push_str(&format!("{key} = {v}\n\n")),
        None => buf.push_str(&format!("# {key} = 0\n\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_round_trips_via_toml() {
        let cfg = CliConfig::default();
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        cfg.write_with_comments(&path).unwrap();
        let loaded = CliConfig::load_or_init(&path).unwrap();
        assert_eq!(cfg, loaded);
    }

    #[test]
    fn load_or_init_creates_with_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let cfg = CliConfig::load_or_init(&path).unwrap();
        assert_eq!(cfg, CliConfig::default());
        assert!(path.exists());
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# pcloudc config file"));
    }

    #[test]
    fn parser_skips_comments_and_blanks() {
        let toml = r#"
            # comment
            username = "alice"
            trust_device = true
            crypto = false
            log_level = "info"
        "#;
        let cfg = CliConfig::parse_toml(toml);
        assert_eq!(cfg.username, Some("alice".into()));
        assert!(cfg.trust_device);
        assert!(!cfg.crypto);
        assert_eq!(cfg.log_level, Some("info".into()));
    }
}

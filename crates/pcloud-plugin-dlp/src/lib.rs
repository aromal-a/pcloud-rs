#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

//! Built-in pre-upload DLP (data-loss-prevention) scanner plugin.
//!
//! This crate implements the first in-tree plugin for the pcloud-rs
//! Rust rewrite, following the B5 plugin design. It scans a small
//! prefix of a local file (typically 4 KiB) before upload and emits an
//! [`UploadScanVerdict`] that the
//! host enforces.
//!
//! # Capabilities
//!
//! The plugin declares only
//! [`PluginCapability::ObserveStatus`].
//! It never initiates network traffic, sync control, or crypto unlock.
//!
//! # Privacy rules
//!
//! The plugin never logs the raw path or file contents. Audit events
//! carry a stable `path_hash` (SHA-256 of the raw path, hex), the list
//! of rule IDs that fired, and the final verdict.
//!
//! # Modes
//!
//! * **Audit-only** (default): every scan returns
//!   [`UploadScanVerdict::Allow`],
//!   but a [`DlpAuditEvent`] is emitted for any hit so operators can
//!   observe what *would* have been blocked.
//! * **Strict**: a matching rule yields
//!   [`UploadScanVerdict::Deny`].

use std::collections::{BTreeMap, BTreeSet};

use pcloud_plugin_api::{PluginCapability, PluginOperation, UploadScanVerdict};
use regex::bytes::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Errors returned by [`DlpScanner`].
#[derive(Debug, Error)]
pub enum DlpError {
    /// The operation dispatched to [`DlpScanner::scan`] was not a
    /// [`PluginOperation::PreUploadScan`].
    #[error("unsupported operation for DLP scanner")]
    UnsupportedOperation,
    /// A built-in regex failed to compile. This should never happen at
    /// runtime — it indicates a programmer error.
    #[error("built-in regex compilation failed: {0}")]
    RegexCompile(String),
}

/// Configuration block for the DLP plugin. Typically deserialised from
/// the `[plugins.dlp]` section of the daemon TOML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DlpConfig {
    /// Global enable switch.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// If true, any rule match yields
    /// [`UploadScanVerdict::Deny`].
    /// If false (default), the scan is audit-only and always allows.
    #[serde(default)]
    pub strict_mode: bool,
    /// Per-rule enable/disable. Unlisted rules default to enabled.
    #[serde(default)]
    pub rules: BTreeMap<String, bool>,
    /// Wall-clock budget for a single scan in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout_ms() -> u64 {
    5000
}

impl Default for DlpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strict_mode: false,
            rules: BTreeMap::new(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

impl DlpConfig {
    /// Whether a specific rule is enabled. Unknown rule IDs default to
    /// enabled.
    #[must_use]
    pub fn rule_enabled(&self, rule_id: &str) -> bool {
        self.rules.get(rule_id).copied().unwrap_or(true)
    }
}

/// Non-secret audit event emitted by the DLP plugin.
///
/// Contains a path *hash* rather than the raw path, never the file
/// contents, and only the rule IDs that fired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DlpAuditEvent {
    /// SHA-256 of the raw path, hex-encoded. Stable across runs.
    pub path_hash: String,
    /// Rule IDs that matched, sorted for determinism.
    pub rule_ids: Vec<String>,
    /// Final verdict the plugin returned to the host.
    pub verdict: UploadScanVerdict,
}

/// Combined result of a single scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlpScanResult {
    /// Verdict to hand to the host.
    pub verdict: UploadScanVerdict,
    /// Structured audit event the host should forward to its audit
    /// sink. Present on every scan, including allow-no-match.
    pub audit: DlpAuditEvent,
}

/// Known built-in rule identifiers. Kept stable for config + audit.
pub mod rule_ids {
    #![allow(missing_docs)]
    pub const AWS_ACCESS_KEY: &str = "AWS_ACCESS_KEY";
    pub const AWS_SECRET_KEY: &str = "AWS_SECRET_KEY";
    pub const PRIVATE_KEY_PEM: &str = "PRIVATE_KEY_PEM";
    pub const JWT: &str = "JWT";
    pub const GENERIC_PASSWORD_LITERAL: &str = "GENERIC_PASSWORD_LITERAL";
    pub const HIGH_ENTROPY: &str = "HIGH_ENTROPY";
}

struct CompiledRule {
    id: &'static str,
    re: Regex,
}

/// DLP scanner implementing the B5 pre-upload hook.
pub struct DlpScanner {
    cfg: DlpConfig,
    rules: Vec<CompiledRule>,
    aws_secret_context: Regex,
}

impl DlpScanner {
    /// Build a new scanner from its [`DlpConfig`].
    pub fn new(cfg: DlpConfig) -> Result<Self, DlpError> {
        let rules = vec![
            CompiledRule {
                id: rule_ids::AWS_ACCESS_KEY,
                re: compile(r"AKIA[0-9A-Z]{16}")?,
            },
            CompiledRule {
                id: rule_ids::PRIVATE_KEY_PEM,
                re: compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")?,
            },
            CompiledRule {
                id: rule_ids::JWT,
                re: compile(r"eyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]+")?,
            },
            CompiledRule {
                id: rule_ids::GENERIC_PASSWORD_LITERAL,
                re: compile(r"(?i)password[=:]\s*\S{8,}")?,
            },
        ];
        // AWS secret: look for the word "secret" somewhere near a
        // high-entropy base64-ish token (>= 40 chars).
        let aws_secret_context = compile(r"(?i)secret[^\n]{0,40}[A-Za-z0-9+/]{40,}")?;
        Ok(Self {
            cfg,
            rules,
            aws_secret_context,
        })
    }

    /// The capability this plugin needs. Always
    /// [`PluginCapability::ObserveStatus`].
    #[must_use]
    pub fn required_capability(&self) -> PluginCapability {
        PluginCapability::ObserveStatus
    }

    /// Perform a scan of a [`PluginOperation::PreUploadScan`] payload.
    pub fn scan(&self, op: &PluginOperation) -> Result<DlpScanResult, DlpError> {
        let (path, first_bytes) = match op {
            PluginOperation::PreUploadScan {
                path, first_bytes, ..
            } => (path.as_str(), first_bytes.as_slice()),
            _ => return Err(DlpError::UnsupportedOperation),
        };

        let path_hash = hash_path(path);

        if !self.cfg.enabled {
            return Ok(DlpScanResult {
                verdict: UploadScanVerdict::Allow,
                audit: DlpAuditEvent {
                    path_hash,
                    rule_ids: Vec::new(),
                    verdict: UploadScanVerdict::Allow,
                },
            });
        }

        let mut hits: BTreeSet<String> = BTreeSet::new();

        for rule in &self.rules {
            if !self.cfg.rule_enabled(rule.id) {
                continue;
            }
            if rule.re.is_match(first_bytes) {
                hits.insert(rule.id.to_string());
            }
        }

        // AWS_SECRET_KEY: contextual — "secret" + long base64-ish token.
        if self.cfg.rule_enabled(rule_ids::AWS_SECRET_KEY)
            && self.aws_secret_context.is_match(first_bytes)
        {
            hits.insert(rule_ids::AWS_SECRET_KEY.to_string());
        }

        // HIGH_ENTROPY: skip if first_bytes begins with a known
        // compressed/binary magic.
        if self.cfg.rule_enabled(rule_ids::HIGH_ENTROPY) && !is_known_binary_magic(first_bytes) {
            let window = &first_bytes[..first_bytes.len().min(4096)];
            if !window.is_empty() && shannon_entropy(window) > 7.5 {
                hits.insert(rule_ids::HIGH_ENTROPY.to_string());
            }
        }

        let rule_ids: Vec<String> = hits.into_iter().collect();
        let verdict = if rule_ids.is_empty() {
            UploadScanVerdict::Allow
        } else if self.cfg.strict_mode {
            UploadScanVerdict::Deny
        } else {
            UploadScanVerdict::Allow
        };

        Ok(DlpScanResult {
            audit: DlpAuditEvent {
                path_hash,
                rule_ids,
                verdict,
            },
            verdict,
        })
    }
}

fn compile(pattern: &str) -> Result<Regex, DlpError> {
    Regex::new(pattern).map_err(|e| DlpError::RegexCompile(e.to_string()))
}

fn hash_path(path: &str) -> String {
    let mut h = Sha256::new();
    h.update(path.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Compute Shannon entropy in bits/byte over `buf`.
#[must_use]
pub fn shannon_entropy(buf: &[u8]) -> f64 {
    if buf.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in buf {
        counts[b as usize] += 1;
    }
    let len = buf.len() as f64;
    let mut entropy = 0.0_f64;
    for &c in &counts {
        if c == 0 {
            continue;
        }
        let p = c as f64 / len;
        entropy -= p * p.log2();
    }
    entropy
}

/// True if `buf` starts with a known compressed or binary-container
/// magic signature for which high entropy is expected (not a DLP
/// signal).
#[must_use]
pub fn is_known_binary_magic(buf: &[u8]) -> bool {
    const MAGICS: &[&[u8]] = &[
        &[0x1f, 0x8b],                                     // gzip
        &[0x50, 0x4b, 0x03, 0x04],                         // zip / docx / jar
        &[0x50, 0x4b, 0x05, 0x06],                         // empty zip
        &[0x50, 0x4b, 0x07, 0x08],                         // spanned zip
        &[0xff, 0xd8, 0xff],                               // jpeg
        b"\x89PNG\r\n",                                    // png
        b"BZh",                                            // bzip2
        &[0xfd, b'7', b'z', b'X', b'Z', 0x00],             // xz
        &[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c],             // 7z
        b"%PDF",                                           // pdf
        b"OggS",                                           // ogg
        b"fLaC",                                           // flac
        &[0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p'], // mp4 ftyp (partial)
        b"RIFF",                                           // riff (wav, avi, webp)
        b"GIF8",                                           // gif
    ];
    MAGICS.iter().any(|m| buf.starts_with(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(bytes: &[u8]) -> PluginOperation {
        PluginOperation::PreUploadScan {
            path: "/tmp/secret.txt".to_string(),
            size: bytes.len() as u64,
            content_hash: "deadbeef".to_string(),
            first_bytes: bytes.to_vec(),
            mime_guess: None,
        }
    }

    #[test]
    fn detects_aws_access_key_in_first_bytes() {
        let scanner = DlpScanner::new(DlpConfig::default()).unwrap();
        let buf = b"config:\naws_access_key = AKIAABCDEFGHIJKLMNOP\n";
        let res = scanner.scan(&op(buf)).unwrap();
        assert!(
            res.audit
                .rule_ids
                .iter()
                .any(|r| r == rule_ids::AWS_ACCESS_KEY)
        );
    }

    #[test]
    fn detects_private_key_pem_header() {
        let scanner = DlpScanner::new(DlpConfig::default()).unwrap();
        let buf = b"-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n";
        let res = scanner.scan(&op(buf)).unwrap();
        assert!(
            res.audit
                .rule_ids
                .iter()
                .any(|r| r == rule_ids::PRIVATE_KEY_PEM)
        );
    }

    #[test]
    fn high_entropy_random_buffer_triggers_rule() {
        let scanner = DlpScanner::new(DlpConfig::default()).unwrap();
        // Build a 4 KiB buffer with near-uniform byte distribution.
        let mut buf = Vec::with_capacity(4096);
        for i in 0..4096u32 {
            // LCG-ish mix to avoid obvious periodicity.
            let x = i.wrapping_mul(2654435761);
            buf.push(((x ^ (x >> 16)) & 0xff) as u8);
        }
        let entropy = shannon_entropy(&buf);
        assert!(entropy > 7.5, "test buffer entropy too low: {entropy}");
        let res = scanner.scan(&op(&buf)).unwrap();
        assert!(
            res.audit
                .rule_ids
                .iter()
                .any(|r| r == rule_ids::HIGH_ENTROPY),
            "expected HIGH_ENTROPY, got {:?}",
            res.audit.rule_ids
        );
    }

    #[test]
    fn known_compressed_magic_skips_entropy_rule() {
        let scanner = DlpScanner::new(DlpConfig::default()).unwrap();
        // gzip magic + high-entropy payload.
        let mut buf = vec![0x1f, 0x8b, 0x08, 0x00];
        for i in 0..4096u32 {
            let x = i.wrapping_mul(2654435761);
            buf.push(((x ^ (x >> 16)) & 0xff) as u8);
        }
        let res = scanner.scan(&op(&buf)).unwrap();
        assert!(
            !res.audit
                .rule_ids
                .iter()
                .any(|r| r == rule_ids::HIGH_ENTROPY),
            "HIGH_ENTROPY must not fire on known binary magic"
        );
    }

    #[test]
    fn strict_mode_returns_deny_on_match_else_allow() {
        let cfg = DlpConfig {
            strict_mode: true,
            ..DlpConfig::default()
        };
        let scanner = DlpScanner::new(cfg).unwrap();

        let hit = scanner
            .scan(&op(b"AKIAABCDEFGHIJKLMNOP in config\n"))
            .unwrap();
        assert_eq!(hit.verdict, UploadScanVerdict::Deny);

        let clean = scanner.scan(&op(b"just some harmless text\n")).unwrap();
        assert_eq!(clean.verdict, UploadScanVerdict::Allow);
        assert!(clean.audit.rule_ids.is_empty());
    }

    #[test]
    fn audit_only_mode_returns_allow_but_emits_event() {
        let scanner = DlpScanner::new(DlpConfig::default()).unwrap();
        let res = scanner
            .scan(&op(b"-----BEGIN OPENSSH PRIVATE KEY-----\nxxx"))
            .unwrap();
        assert_eq!(res.verdict, UploadScanVerdict::Allow);
        assert!(!res.audit.rule_ids.is_empty());
        assert_eq!(res.audit.verdict, UploadScanVerdict::Allow);
        // Path is never surfaced in the event.
        assert_ne!(res.audit.path_hash, "/tmp/secret.txt");
        assert_eq!(res.audit.path_hash.len(), 64);
    }

    #[test]
    fn rejects_non_preupload_operation() {
        let scanner = DlpScanner::new(DlpConfig::default()).unwrap();
        let err = scanner.scan(&PluginOperation::ObserveHealth).unwrap_err();
        matches!(err, DlpError::UnsupportedOperation);
    }

    #[test]
    fn disabled_plugin_always_allows_no_rules() {
        let cfg = DlpConfig {
            enabled: false,
            strict_mode: true,
            ..DlpConfig::default()
        };
        let scanner = DlpScanner::new(cfg).unwrap();
        let res = scanner
            .scan(&op(b"AKIAABCDEFGHIJKLMNOP secret stuff"))
            .unwrap();
        assert_eq!(res.verdict, UploadScanVerdict::Allow);
        assert!(res.audit.rule_ids.is_empty());
    }

    #[test]
    fn per_rule_disable_suppresses_match() {
        let mut rules = BTreeMap::new();
        rules.insert(rule_ids::AWS_ACCESS_KEY.to_string(), false);
        let cfg = DlpConfig {
            strict_mode: true,
            rules,
            ..DlpConfig::default()
        };
        let scanner = DlpScanner::new(cfg).unwrap();
        let res = scanner
            .scan(&op(b"token = AKIAABCDEFGHIJKLMNOP\n"))
            .unwrap();
        assert!(
            !res.audit
                .rule_ids
                .iter()
                .any(|r| r == rule_ids::AWS_ACCESS_KEY)
        );
    }
}

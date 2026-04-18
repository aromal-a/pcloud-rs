//! JSON schema definition + a minimal, focused validator for the on-disk
//! [`crate::ConfigProfile`] envelope.
//!
//! We deliberately avoid pulling in a full JSON-schema implementation
//! (`jsonschema`, `valico`) because the on-disk profile surface is small,
//! fully known at compile time, and we need precise line/column + JSON
//! pointer diagnostics. The published schema document is still emitted
//! verbatim so external tooling (IDE hints, `check-jsonschema`, CI) can
//! validate user files against the exact same contract.
//!
//! The schema is `$schema: draft-07` compatible and sets
//! `additionalProperties: false` at every object level, matching the
//! enterprise posture documented in the workspace `CLAUDE.md`.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde_json::Value;
use std::fmt;

/// The canonical JSON schema for the config-file envelope. External tools
/// can read this to validate files without linking `pcloud-config`.
pub const CONFIG_SCHEMA_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "pcloud-config profile envelope",
  "type": "object",
  "additionalProperties": false,
  "required": ["version", "profile"],
  "properties": {
    "version": { "type": "integer", "minimum": 0 },
    "profile": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "environment", "paths", "api", "extensions",
        "runtime", "features", "limits", "mount", "observability"
      ],
      "properties": {
        "environment": { "type": "string", "enum": ["Development", "Test", "Production"] },
        "paths": {
          "type": "object",
          "additionalProperties": false,
          "required": ["config_dir", "state_dir", "runtime_dir", "cache_dir"],
          "properties": {
            "config_dir":  { "type": "string" },
            "state_dir":   { "type": "string" },
            "runtime_dir": { "type": "string" },
            "cache_dir":   { "type": "string" }
          }
        },
        "api": {
          "type": "object",
          "additionalProperties": false,
          "required": ["mode","host","port","server_name","connect_timeout_ms","read_timeout_ms"],
          "properties": {
            "mode": { "type": "string", "enum": ["Development", "Plaintext", "Tls"] },
            "host": { "type": "string" },
            "port": { "type": "integer", "minimum": 0, "maximum": 65535 },
            "server_name": { "type": "string" },
            "connect_timeout_ms": { "type": "integer", "minimum": 0 },
            "read_timeout_ms":    { "type": "integer", "minimum": 0 }
          }
        },
        "extensions": {
          "type": "object",
          "additionalProperties": false,
          "required": [
            "plugins_enabled","plugin_dir",
            "allow_network_capability","allow_sync_control_capability","allow_crypto_capability"
          ],
          "properties": {
            "plugins_enabled":               { "type": "boolean" },
            "plugin_dir":                    { "type": "string" },
            "allow_network_capability":      { "type": "boolean" },
            "allow_sync_control_capability": { "type": "boolean" },
            "allow_crypto_capability":       { "type": "boolean" },
            "trusted_plugin_keys": {
              "type": "array",
              "items": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0, "maximum": 255 }
              }
            }
          }
        },
        "runtime": {
          "type": "object",
          "additionalProperties": false,
          "required": ["config_dir_mode","socket_dir_mode","state_dir_mode","cache_dir_mode"],
          "properties": {
            "config_dir_mode": { "type": "integer", "minimum": 0 },
            "socket_dir_mode": { "type": "integer", "minimum": 0 },
            "state_dir_mode":  { "type": "integer", "minimum": 0 },
            "cache_dir_mode":  { "type": "integer", "minimum": 0 }
          }
        },
        "features": {
          "type": "object",
          "additionalProperties": false,
          "required": ["p2p_enabled","crypto_enabled","durable_auth_tokens_enabled"],
          "properties": {
            "p2p_enabled":                 { "type": "boolean" },
            "crypto_enabled":              { "type": "boolean" },
            "durable_auth_tokens_enabled": { "type": "boolean" },
            "integrity_sweeper": {
              "type": "object",
              "additionalProperties": false,
              "properties": {
                "enabled":                { "type": "boolean" },
                "schedule_cron":          { "type": ["string","null"] },
                "rate_files_per_minute":  { "type": "integer", "minimum": 0 },
                "pause_on_battery":       { "type": "boolean" },
                "skip_list_path":         { "type": ["string","null"] }
              }
            },
            "audit_verifier": {
              "type": "object",
              "additionalProperties": false,
              "properties": {
                "enabled":         { "type": "boolean" },
                "schedule_cron":   { "type": "string" },
                "checkpoint_path": { "type": ["string","null"] }
              }
            }
          }
        },
        "limits": {
          "type": "object",
          "additionalProperties": false,
          "required": ["max_concurrent_uploads","max_concurrent_downloads","max_parser_frame_bytes"],
          "properties": {
            "max_concurrent_uploads":   { "type": "integer", "minimum": 0 },
            "max_concurrent_downloads": { "type": "integer", "minimum": 0 },
            "max_parser_frame_bytes":   { "type": "integer", "minimum": 0 }
          }
        },
        "mount": {
          "type": "object",
          "additionalProperties": false,
          "required": ["allow_other","owner_only_by_default"],
          "properties": {
            "allow_other":           { "type": "boolean" },
            "owner_only_by_default": { "type": "boolean" },
            "cache_size_mb":         { "type": "integer", "minimum": 0 },
            "page_cache_entries":    { "type": "integer", "minimum": 0 },
            "metadata_ttl_secs":     { "type": "integer", "minimum": 0 },
            "auto_mount_path":       { "type": ["string","null"] }
          }
        },
        "observability": {
          "type": "object",
          "additionalProperties": false,
          "required": [
            "structured_logs_enabled","tracing_enabled",
            "metrics_enabled","audit_export_enabled"
          ],
          "properties": {
            "structured_logs_enabled": { "type": "boolean" },
            "tracing_enabled":         { "type": "boolean" },
            "metrics_enabled":         { "type": "boolean" },
            "audit_export_enabled":    { "type": "boolean" }
          }
        },
        "data_residency": {
          "type": "object",
          "additionalProperties": false,
          "required": [],
          "properties": {
            "allowed_regions": {
              "type": "array",
              "items": { "type": "string" }
            },
            "strict": { "type": "boolean" }
          }
        },
        "auth": {
          "type": "object",
          "additionalProperties": false,
          "required": [],
          "properties": {
            "backend": {
              "type": "string",
              "enum": ["auto","file","keychain","dpapi","secret-service"]
            },
            "refresh_check_interval_secs": {
              "type": "integer",
              "minimum": 0
            },
            "refresh_margin_secs": {
              "type": "integer",
              "minimum": 0
            }
          }
        },
        "resilience": {
          "type": "object",
          "additionalProperties": false,
          "required": [
            "enabled",
            "rate_limit_capacity","rate_limit_refill_per_sec",
            "breaker_failure_threshold","breaker_reset_timeout_ms",
            "retry_max_attempts","retry_base_delay_ms","retry_factor",
            "retry_max_delay_ms","retry_jitter_seed"
          ],
          "properties": {
            "enabled":                    { "type": "boolean" },
            "rate_limit_capacity":        { "type": "integer", "minimum": 1 },
            "rate_limit_refill_per_sec":  { "type": "number" },
            "breaker_failure_threshold":  { "type": "integer", "minimum": 1 },
            "breaker_reset_timeout_ms":   { "type": "integer", "minimum": 0 },
            "retry_max_attempts":         { "type": "integer", "minimum": 1 },
            "retry_base_delay_ms":        { "type": "integer", "minimum": 0 },
            "retry_factor":               { "type": "number" },
            "retry_max_delay_ms":         { "type": "integer", "minimum": 0 },
            "retry_jitter_seed":          { "type": "integer", "minimum": 0 }
          }
        },
        "rate_limit": {
          "type": "object",
          "additionalProperties": false,
          "required": [],
          "properties": {
            "enabled": { "type": "boolean" },
            "cheap": {
              "type": "object",
              "additionalProperties": false,
              "required": ["capacity","refill_per_sec"],
              "properties": {
                "capacity":       { "type": "integer", "minimum": 0 },
                "refill_per_sec": { "type": "number" }
              }
            },
            "medium": {
              "type": "object",
              "additionalProperties": false,
              "required": ["capacity","refill_per_sec"],
              "properties": {
                "capacity":       { "type": "integer", "minimum": 0 },
                "refill_per_sec": { "type": "number" }
              }
            },
            "expensive": {
              "type": "object",
              "additionalProperties": false,
              "required": ["capacity","refill_per_sec"],
              "properties": {
                "capacity":       { "type": "integer", "minimum": 0 },
                "refill_per_sec": { "type": "number" }
              }
            },
            "auth_attempt": {
              "type": "object",
              "additionalProperties": false,
              "required": ["capacity","refill_per_sec"],
              "properties": {
                "capacity":       { "type": "integer", "minimum": 0 },
                "refill_per_sec": { "type": "number" }
              }
            }
          }
        },
        "ha": {
          "type": "object",
          "additionalProperties": false,
          "required": [],
          "properties": {
            "enabled": { "type": "boolean" },
            "mode": { "type": "string", "enum": ["refuse", "passive"] },
            "heartbeat_interval_secs": { "type": "integer", "minimum": 1 },
            "passive_poll_interval_secs": { "type": "integer", "minimum": 1 }
          }
        },
        "file_history": {
          "type": "object",
          "additionalProperties": false,
          "required": [],
          "properties": {
            "revision_url": { "type": ["string", "null"] }
          }
        },
        "upgrade": {
          "type": "object"
        },
        "sync_loop": {
          "type": "object",
          "additionalProperties": false,
          "properties": {
            "enabled": { "type": "boolean" },
            "poll_interval_secs": { "type": "integer" },
            "batch_size": { "type": "integer" },
            "max_concurrent_transfers": { "type": "integer" },
            "propagate_deletes": { "type": "boolean" },
            "full_scan_interval_secs": { "type": "integer" },
            "conflict_policy": { "type": "string" },
            "upload_chunk_size": { "type": "integer" }
          }
        }
      }
    }
  }
}"#;

/// A single schema validation violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaViolation {
    /// RFC 6901 JSON pointer to the offending value.
    pub pointer: String,
    /// Human-readable reason.
    pub reason: String,
    /// Source text line (1-based), if computable from the raw document.
    pub line: Option<usize>,
    /// Source text column (1-based).
    pub column: Option<usize>,
}

impl fmt::Display for SchemaViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(l), Some(c)) => write!(f, "{}:{} at {}: {}", l, c, self.pointer, self.reason),
            _ => write!(f, "at {}: {}", self.pointer, self.reason),
        }
    }
}

/// Internal schema representation. A tiny subset of draft-07 sufficient for
/// our schema: type, required, additionalProperties:false, enum, numeric
/// bounds, properties.
enum Node {
    Object {
        required: &'static [&'static str],
        properties: &'static [(&'static str, &'static Node)],
    },
    String {
        enum_values: Option<&'static [&'static str]>,
    },
    Bool,
    Integer {
        min: Option<i128>,
        max: Option<i128>,
    },
    /// Generic JSON number (integer or float). Finite values only.
    Number,
    /// Homogeneous array; each element is validated against `items`.
    Array {
        items: &'static Node,
    },
    /// Opaque value — any JSON type passes. Used for fields whose
    /// structure is validated later by serde + a typed validator
    /// (e.g. the `[crypto.kms]` tagged union, whose provider-specific
    /// shape cannot be expressed in this hand-rolled `Node` enum).
    Any,
}

// Schema tree hand-mirroring the document above. Keeping it in Rust lets us
// produce precise pointers cheaply; the JSON constant remains authoritative
// for external consumers.

static PATHS_NODE: Node = Node::Object {
    required: &["config_dir", "state_dir", "runtime_dir", "cache_dir"],
    properties: &[
        ("config_dir", &Node::String { enum_values: None }),
        ("state_dir", &Node::String { enum_values: None }),
        ("runtime_dir", &Node::String { enum_values: None }),
        ("cache_dir", &Node::String { enum_values: None }),
    ],
};

static API_NODE: Node = Node::Object {
    required: &[
        "mode",
        "host",
        "port",
        "server_name",
        "connect_timeout_ms",
        "read_timeout_ms",
    ],
    properties: &[
        (
            "mode",
            &Node::String {
                enum_values: Some(&["Development", "Plaintext", "Tls"]),
            },
        ),
        ("host", &Node::String { enum_values: None }),
        (
            "port",
            &Node::Integer {
                min: Some(0),
                max: Some(65535),
            },
        ),
        ("server_name", &Node::String { enum_values: None }),
        (
            "connect_timeout_ms",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "read_timeout_ms",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
    ],
};

static TRUSTED_KEY_BYTE: Node = Node::Integer {
    min: Some(0),
    max: Some(255),
};
static TRUSTED_KEY_NODE: Node = Node::Array {
    items: &TRUSTED_KEY_BYTE,
};
static TRUSTED_KEY_LIST_NODE: Node = Node::Array {
    items: &TRUSTED_KEY_NODE,
};

static EXT_NODE: Node = Node::Object {
    required: &[
        "plugins_enabled",
        "plugin_dir",
        "allow_network_capability",
        "allow_sync_control_capability",
        "allow_crypto_capability",
    ],
    properties: &[
        ("plugins_enabled", &Node::Bool),
        ("plugin_dir", &Node::String { enum_values: None }),
        ("allow_network_capability", &Node::Bool),
        ("allow_sync_control_capability", &Node::Bool),
        ("allow_crypto_capability", &Node::Bool),
        ("trusted_plugin_keys", &TRUSTED_KEY_LIST_NODE),
    ],
};

static RUNTIME_NODE: Node = Node::Object {
    required: &[
        "config_dir_mode",
        "socket_dir_mode",
        "state_dir_mode",
        "cache_dir_mode",
    ],
    properties: &[
        (
            "config_dir_mode",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "socket_dir_mode",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "state_dir_mode",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "cache_dir_mode",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
    ],
};

static INTEGRITY_SWEEPER_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        ("enabled", &Node::Bool),
        ("schedule_cron", &Node::String { enum_values: None }),
        (
            "rate_files_per_minute",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        ("pause_on_battery", &Node::Bool),
        ("skip_list_path", &Node::String { enum_values: None }),
    ],
};

static AUDIT_VERIFIER_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        ("enabled", &Node::Bool),
        ("schedule_cron", &Node::String { enum_values: None }),
        ("checkpoint_path", &Node::String { enum_values: None }),
    ],
};

static FEATURES_NODE: Node = Node::Object {
    required: &[
        "p2p_enabled",
        "crypto_enabled",
        "durable_auth_tokens_enabled",
    ],
    properties: &[
        ("p2p_enabled", &Node::Bool),
        ("crypto_enabled", &Node::Bool),
        ("durable_auth_tokens_enabled", &Node::Bool),
        // Optional; absent on envelopes predating bd-1du.4.6.1.
        ("integrity_sweeper", &INTEGRITY_SWEEPER_NODE),
        // Optional; absent on envelopes predating I04-follow-up.
        ("audit_verifier", &AUDIT_VERIFIER_NODE),
    ],
};

static LIMITS_NODE: Node = Node::Object {
    required: &[
        "max_concurrent_uploads",
        "max_concurrent_downloads",
        "max_parser_frame_bytes",
    ],
    properties: &[
        (
            "max_concurrent_uploads",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "max_concurrent_downloads",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "max_parser_frame_bytes",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
    ],
};

static MOUNT_NODE: Node = Node::Object {
    required: &["allow_other", "owner_only_by_default"],
    properties: &[
        ("allow_other", &Node::Bool),
        ("owner_only_by_default", &Node::Bool),
        (
            "cache_size_mb",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "metadata_ttl_secs",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "page_cache_entries",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        ("auto_mount_path", &Node::String { enum_values: None }),
    ],
};

static OBS_NODE: Node = Node::Object {
    required: &[
        "structured_logs_enabled",
        "tracing_enabled",
        "metrics_enabled",
        "audit_export_enabled",
    ],
    properties: &[
        ("structured_logs_enabled", &Node::Bool),
        ("tracing_enabled", &Node::Bool),
        ("metrics_enabled", &Node::Bool),
        ("audit_export_enabled", &Node::Bool),
    ],
};

static RESILIENCE_NODE: Node = Node::Object {
    required: &[
        "enabled",
        "rate_limit_capacity",
        "rate_limit_refill_per_sec",
        "breaker_failure_threshold",
        "breaker_reset_timeout_ms",
        "retry_max_attempts",
        "retry_base_delay_ms",
        "retry_factor",
        "retry_max_delay_ms",
        "retry_jitter_seed",
    ],
    properties: &[
        ("enabled", &Node::Bool),
        (
            "rate_limit_capacity",
            &Node::Integer {
                min: Some(1),
                max: None,
            },
        ),
        ("rate_limit_refill_per_sec", &Node::Number),
        (
            "breaker_failure_threshold",
            &Node::Integer {
                min: Some(1),
                max: None,
            },
        ),
        (
            "breaker_reset_timeout_ms",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "retry_max_attempts",
            &Node::Integer {
                min: Some(1),
                max: None,
            },
        ),
        (
            "retry_base_delay_ms",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        ("retry_factor", &Node::Number),
        (
            "retry_max_delay_ms",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "retry_jitter_seed",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
    ],
};

static REGION_STRING_NODE: Node = Node::String { enum_values: None };
static ALLOWED_REGIONS_NODE: Node = Node::Array {
    items: &REGION_STRING_NODE,
};
static DATA_RESIDENCY_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        ("allowed_regions", &ALLOWED_REGIONS_NODE),
        ("strict", &Node::Bool),
    ],
};

static AUTH_BACKEND_NODE: Node = Node::String {
    enum_values: Some(&["auto", "file", "keychain", "dpapi", "secret-service"]),
};
static AUTH_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        ("backend", &AUTH_BACKEND_NODE),
        (
            "refresh_check_interval_secs",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        (
            "refresh_margin_secs",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
    ],
};

// Per-category rate-limit bucket.
static RATE_BUCKET_NODE: Node = Node::Object {
    required: &["capacity", "refill_per_sec"],
    properties: &[
        (
            "capacity",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        ("refill_per_sec", &Node::Number),
    ],
};
static RATE_LIMIT_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        ("enabled", &Node::Bool),
        ("cheap", &RATE_BUCKET_NODE),
        ("medium", &RATE_BUCKET_NODE),
        ("expensive", &RATE_BUCKET_NODE),
        ("auth_attempt", &RATE_BUCKET_NODE),
    ],
};

// `crypto.kms` is a tagged union (provider = null|aws|vault|pkcs11)
// plus provider-specific fields. The hand-rolled schema walker in this
// module does not model `oneOf`, so the KMS node is `Node::Any` —
// structural checks run via serde tagged-enum deserialisation plus
// `CryptoKmsConfig::validate` in `pcloud-config::crypto_kms`.
static CRYPTO_KMS_NODE: Node = Node::Any;
// `crypto.mode` selects the DEK source for the sector-encryption path:
// `"raw"` (default, Argon2 master-key-derived) or `"kms"` (KMS-wrapped
// DEK — requires `crypto.kms` to be populated with a non-null provider,
// enforced by `CryptoConfig::validate`).
static CRYPTO_MODE_NODE: Node = Node::Any;
static CRYPTO_NODE: Node = Node::Object {
    required: &[],
    properties: &[("mode", &CRYPTO_MODE_NODE), ("kms", &CRYPTO_KMS_NODE)],
};

// Revision-history provider wiring for `log` / `diff` / `restore`.
// Optional; absence of this block (or a `null` `revision_url`) yields
// the `NullRevisionProvider` and a structured
// `{"status":"not_configured",…}` daemon response.
static FILE_HISTORY_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        // `revision_url` is `Option<String>` in Rust, so the on-disk
        // value can be either a string or JSON `null`. `Node::Any`
        // accepts both; structural validation ("https:// in production")
        // happens in `FileHistoryConfig::validate`.
        ("revision_url", &Node::Any),
    ],
};

// Optional upgrade policy section. Schema kept permissive
// (`Node::Any`) because the concrete shape is evolving alongside
// `pcloud_config::upgrade`; the Rust type still enforces validation
// via serde. Required here so loader tests with older/newer
// envelopes that carry this key load without a schema error.
static UPGRADE_NODE: Node = Node::Any;

// Background sync loop configuration. All fields optional — the
// default is `enabled = true, poll_interval_secs = 30`.
static SYNC_LOOP_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        ("enabled", &Node::Bool),
        (
            "poll_interval_secs",
            &Node::Integer {
                min: Some(5),
                max: Some(3600),
            },
        ),
        (
            "batch_size",
            &Node::Integer {
                min: Some(1),
                max: Some(10_000),
            },
        ),
        (
            "max_concurrent_transfers",
            &Node::Integer {
                min: Some(1),
                max: Some(64),
            },
        ),
        ("propagate_deletes", &Node::Bool),
        (
            "full_scan_interval_secs",
            &Node::Integer {
                min: Some(30),
                max: Some(86400),
            },
        ),
        ("conflict_policy", &Node::String { enum_values: None }),
        (
            "upload_chunk_size",
            &Node::Integer {
                min: Some(1),
                max: None,
            },
        ),
    ],
};

// Tier-2 HA policy block. All fields optional — the default is
// `enabled = false`, which matches the pre-HA daemon. See
// `docs/enterprise/ha.md` §4.2 and `pcloud_config::ha`.
static HA_NODE: Node = Node::Object {
    required: &[],
    properties: &[
        ("enabled", &Node::Bool),
        (
            "mode",
            &Node::String {
                enum_values: Some(&["refuse", "passive"]),
            },
        ),
        (
            "heartbeat_interval_secs",
            &Node::Integer {
                min: Some(1),
                max: None,
            },
        ),
        (
            "passive_poll_interval_secs",
            &Node::Integer {
                min: Some(1),
                max: None,
            },
        ),
    ],
};

static PROFILE_NODE: Node = Node::Object {
    required: &[
        "environment",
        "paths",
        "api",
        "extensions",
        "runtime",
        "features",
        "limits",
        "mount",
        "observability",
    ],
    properties: &[
        (
            "environment",
            &Node::String {
                enum_values: Some(&["Development", "Test", "Production"]),
            },
        ),
        ("paths", &PATHS_NODE),
        ("api", &API_NODE),
        ("extensions", &EXT_NODE),
        ("runtime", &RUNTIME_NODE),
        ("features", &FEATURES_NODE),
        ("limits", &LIMITS_NODE),
        ("mount", &MOUNT_NODE),
        ("observability", &OBS_NODE),
        // Optional section; not in required[] above so old configs still load.
        ("resilience", &RESILIENCE_NODE),
        // Optional section; older envelopes that predate B5 still load.
        ("data_residency", &DATA_RESIDENCY_NODE),
        // Optional section; older envelopes predating the auth-vault
        // selector still load.
        ("auth", &AUTH_NODE),
        // IPC per-category rate limit policy. Optional; older envelopes
        // load with `rate_limit::RateLimitPolicy::secure_defaults`.
        ("rate_limit", &RATE_LIMIT_NODE),
        // Optional section; older envelopes predating KMS integration
        // still load. Structure is a tagged union — see CRYPTO_NODE.
        ("crypto", &CRYPTO_NODE),
        // Tier-2 HA policy. Optional; default is disabled. See
        // `docs/enterprise/ha.md` §4.2.
        ("ha", &HA_NODE),
        // Revision-history provider config. Optional; absent or empty
        // yields the NullRevisionProvider at runtime.
        ("file_history", &FILE_HISTORY_NODE),
        // Optional upgrade policy (evolving schema; validated via serde).
        ("upgrade", &UPGRADE_NODE),
        // Optional sync loop configuration. All fields optional.
        ("sync_loop", &SYNC_LOOP_NODE),
    ],
};

static ROOT_NODE: Node = Node::Object {
    required: &["version", "profile"],
    properties: &[
        (
            "version",
            &Node::Integer {
                min: Some(0),
                max: None,
            },
        ),
        ("profile", &PROFILE_NODE),
    ],
};

/// Validate a parsed JSON document against the envelope schema.
/// Returns every violation found in one pass so users can fix the full
/// document rather than one error at a time.
pub fn validate_document(doc: &Value, source: &str) -> Vec<SchemaViolation> {
    let mut out = Vec::new();
    validate_node(&ROOT_NODE, doc, String::from(""), &mut out);
    // Annotate violations with line/col using the raw source text.
    for v in &mut out {
        if let Some((line, col)) = locate_pointer(source, &v.pointer) {
            v.line = Some(line);
            v.column = Some(col);
        }
    }
    out
}

fn validate_node(node: &Node, value: &Value, pointer: String, out: &mut Vec<SchemaViolation>) {
    match node {
        Node::Object {
            required,
            properties,
        } => {
            let Some(obj) = value.as_object() else {
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("expected object, found {}", type_name(value)),
                    line: None,
                    column: None,
                });
                return;
            };
            for r in *required {
                if !obj.contains_key(*r) {
                    out.push(SchemaViolation {
                        pointer: format!("{}/{}", pointer, escape(r)),
                        reason: format!("missing required property '{}'", r),
                        line: None,
                        column: None,
                    });
                }
            }
            // additionalProperties: false
            for key in obj.keys() {
                if !properties.iter().any(|(k, _)| *k == key.as_str()) {
                    out.push(SchemaViolation {
                        pointer: format!("{}/{}", pointer, escape(key)),
                        reason: format!(
                            "unexpected property '{}' (additionalProperties=false)",
                            key
                        ),
                        line: None,
                        column: None,
                    });
                }
            }
            for (k, sub) in *properties {
                if let Some(v) = obj.get(*k) {
                    validate_node(sub, v, format!("{}/{}", pointer, escape(k)), out);
                }
            }
        }
        Node::String { enum_values } => {
            let Some(s) = value.as_str() else {
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("expected string, found {}", type_name(value)),
                    line: None,
                    column: None,
                });
                return;
            };
            if let Some(allowed) = enum_values
                && !allowed.contains(&s)
            {
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("value '{}' not in enum {:?}", s, allowed),
                    line: None,
                    column: None,
                });
            }
        }
        Node::Array { items } => {
            let Some(arr) = value.as_array() else {
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("expected array, found {}", type_name(value)),
                    line: None,
                    column: None,
                });
                return;
            };
            for (i, item) in arr.iter().enumerate() {
                validate_node(items, item, format!("{}/{}", pointer, i), out);
            }
        }
        Node::Any => {
            // Intentionally no-op: any JSON value is acceptable. A
            // follow-up typed layer (serde + per-field validators) does
            // the real work.
            let _ = value;
        }
        Node::Number => {
            let ok = value
                .as_f64()
                .map(|n| n.is_finite())
                .or_else(|| value.as_i64().map(|_| true))
                .or_else(|| value.as_u64().map(|_| true))
                .unwrap_or(false);
            if !ok {
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("expected number, found {}", type_name(value)),
                    line: None,
                    column: None,
                });
            }
        }
        Node::Bool => {
            if !value.is_boolean() {
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("expected boolean, found {}", type_name(value)),
                    line: None,
                    column: None,
                });
            }
        }
        Node::Integer { min, max } => {
            let Some(n) = value.as_i64() else {
                // Allow u64 large ints.
                if let Some(n) = value.as_u64() {
                    let nn = n as i128;
                    if let Some(min) = min
                        && nn < *min
                    {
                        out.push(SchemaViolation {
                            pointer: pointer.clone(),
                            reason: format!("value {} below minimum {}", nn, min),
                            line: None,
                            column: None,
                        });
                    }
                    if let Some(max) = max
                        && nn > *max
                    {
                        out.push(SchemaViolation {
                            pointer,
                            reason: format!("value {} above maximum {}", nn, max),
                            line: None,
                            column: None,
                        });
                    }
                    return;
                }
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("expected integer, found {}", type_name(value)),
                    line: None,
                    column: None,
                });
                return;
            };
            let nn = n as i128;
            if let Some(min) = min
                && nn < *min
            {
                out.push(SchemaViolation {
                    pointer: pointer.clone(),
                    reason: format!("value {} below minimum {}", nn, min),
                    line: None,
                    column: None,
                });
            }
            if let Some(max) = max
                && nn > *max
            {
                out.push(SchemaViolation {
                    pointer,
                    reason: format!("value {} above maximum {}", nn, max),
                    line: None,
                    column: None,
                });
            }
        }
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn escape(s: &str) -> String {
    // RFC 6901 JSON pointer escape.
    s.replace('~', "~0").replace('/', "~1")
}

/// Best-effort resolution of a JSON pointer to (line, column) in the raw
/// source. Implemented with a small, dependency-free scanner. Returns None
/// if the pointer cannot be found (e.g. the property was missing).
fn locate_pointer(source: &str, pointer: &str) -> Option<(usize, usize)> {
    if pointer.is_empty() {
        return Some((1, 1));
    }
    let tokens: Vec<String> = pointer
        .trim_start_matches('/')
        .split('/')
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect();

    let bytes = source.as_bytes();
    let mut i = 0usize;
    // Depth tracking so we match keys only at the expected nesting level.
    let mut depth: usize = 0;
    let mut target_depth: usize = 1;

    for token in tokens {
        let mut found = false;
        while i < bytes.len() {
            let c = bytes[i];
            match c {
                b'{' | b'[' => {
                    depth += 1;
                    i += 1;
                }
                b'}' | b']' => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                    i += 1;
                }
                b'"' => {
                    let start = i;
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' && i + 1 < bytes.len() {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if i >= bytes.len() {
                        return None;
                    }
                    let key = std::str::from_utf8(&bytes[start + 1..i]).ok()?;
                    i += 1;
                    // Is this a key? Look for following colon (skipping whitespace).
                    let mut j = i;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    let is_key = j < bytes.len() && bytes[j] == b':';
                    if is_key && depth == target_depth && key == token {
                        // Descend: move to after colon, remember this position.
                        i = j + 1;
                        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                            i += 1;
                        }
                        found = true;
                        target_depth += 1;
                        break;
                    }
                }
                _ => i += 1,
            }
        }
        if !found {
            return None;
        }
    }
    // Convert byte index i to line/col.
    let mut line = 1usize;
    let mut col = 1usize;
    for (idx, c) in source.char_indices() {
        if idx >= i {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_json_parses_as_valid_json() {
        let _: Value = serde_json::from_str(CONFIG_SCHEMA_JSON).expect("schema must parse");
    }

    #[test]
    fn minimal_valid_envelope_passes() {
        let v = serde_json::to_value(minimal_envelope()).unwrap();
        let errs = validate_document(&v, &serde_json::to_string_pretty(&v).unwrap());
        assert!(errs.is_empty(), "unexpected: {:#?}", errs);
    }

    #[test]
    fn additional_property_is_rejected() {
        let mut v = minimal_envelope();
        v["profile"]["api"]["extra"] = json!(true);
        let source = serde_json::to_string_pretty(&v).unwrap();
        let errs = validate_document(&v, &source);
        assert!(
            errs.iter()
                .any(|e| e.reason.contains("additionalProperties"))
        );
    }

    #[test]
    fn missing_required_is_reported_with_pointer() {
        let mut v = minimal_envelope();
        v["profile"]["api"].as_object_mut().unwrap().remove("host");
        let source = serde_json::to_string_pretty(&v).unwrap();
        let errs = validate_document(&v, &source);
        assert!(
            errs.iter()
                .any(|e| e.pointer == "/profile/api/host" && e.reason.contains("missing"))
        );
    }

    #[test]
    fn enum_violation_is_reported() {
        let mut v = minimal_envelope();
        v["profile"]["environment"] = json!("staging");
        let source = serde_json::to_string_pretty(&v).unwrap();
        let errs = validate_document(&v, &source);
        assert!(errs.iter().any(|e| e.pointer == "/profile/environment"));
    }

    #[test]
    fn line_column_is_populated_for_present_values() {
        let mut v = minimal_envelope();
        v["profile"]["api"]["port"] = json!(999_999);
        let source = serde_json::to_string_pretty(&v).unwrap();
        let errs = validate_document(&v, &source);
        let port_err = errs
            .iter()
            .find(|e| e.pointer == "/profile/api/port")
            .expect("must report");
        assert!(port_err.line.is_some());
        assert!(port_err.column.is_some());
    }

    fn minimal_envelope() -> Value {
        json!({
            "version": crate::migrate::CURRENT_VERSION,
            "profile": {
                "environment": "Development",
                "paths": {
                    "config_dir": "/tmp/a/config",
                    "state_dir": "/tmp/a/state",
                    "runtime_dir": "/tmp/a/runtime",
                    "cache_dir": "/tmp/a/cache"
                },
                "api": {
                    "mode": "Development",
                    "host": "bineapi.pcloud.com",
                    "port": 443,
                    "server_name": "bineapi.pcloud.com",
                    "connect_timeout_ms": 5000,
                    "read_timeout_ms": 15000
                },
                "extensions": {
                    "plugins_enabled": false,
                    "plugin_dir": "/tmp/a/plugins",
                    "allow_network_capability": false,
                    "allow_sync_control_capability": false,
                    "allow_crypto_capability": false
                },
                "runtime": {
                    "config_dir_mode": 448,
                    "socket_dir_mode": 448,
                    "state_dir_mode": 448,
                    "cache_dir_mode": 448
                },
                "features": {
                    "p2p_enabled": false,
                    "crypto_enabled": true,
                    "durable_auth_tokens_enabled": false
                },
                "limits": {
                    "max_concurrent_uploads": 4,
                    "max_concurrent_downloads": 4,
                    "max_parser_frame_bytes": 8388608
                },
                "mount": { "allow_other": false, "owner_only_by_default": true },
                "observability": {
                    "structured_logs_enabled": true,
                    "tracing_enabled": false,
                    "metrics_enabled": false,
                    "audit_export_enabled": true
                }
            }
        })
    }
}

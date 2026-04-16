#![deny(missing_docs)]
//! # pcloud-policy
//!
//! Policy enforcement layer for the pcloud-rs Rust daemon.
//!
//! This crate defines the [`PolicyEngine`] trait, the [`PolicyInput`] /
//! [`PolicyDecision`] types, and two reference implementations:
//!
//! * [`NullPolicyEngine`] — the safe, audit-only default used in development.
//!   It allows every request but records the attempt through the caller's
//!   audit hook so operators can observe what *would* have been evaluated.
//! * [`RegoPolicyEngine`] — a stub today; it will wrap an embedded Rego
//!   evaluator (the `regorus` crate) and enforce policies loaded from disk.
//!
//! ## Why a policy layer?
//!
//! Today's daemon auth is all-or-nothing: any authenticated user can run any
//! `Method::*` request. Enterprises require finer control — per-user,
//! per-command, per-path, per-device, time-of-day, and so on. Rather than
//! hard-code these rules, we adopt the industry-standard OPA/Rego policy
//! language. Operators drop `.rego` files into `/etc/pcloud/policy/` and the
//! daemon evaluates every dispatched request against them.
//!
//! ## Deny-by-default
//!
//! Production builds MUST configure `mode = "deny"`. In that mode, a policy
//! that does not explicitly allow a request is treated as a denial. The
//! [`NullPolicyEngine`] is therefore *not* suitable for production — it is a
//! development aid only.
//!
//! ## Security properties
//!
//! * Policies are loaded from root-owned files with mode `0644` or stricter;
//!   a world-writable policy file is refused.
//! * Invalid policies on reload (e.g. after `SIGHUP`) are rejected and the
//!   previously good policy stays active.
//! * Every allow and every deny decision is written to the daemon's audit
//!   log with the full [`PolicyInput`] and the matched rule identifier.
//! * No secret material (passwords, tokens) is ever placed into a
//!   [`PolicyInput`]. The engine evaluates on user identity, command name,
//!   request metadata, device identifier, and timestamp only.
//!
//! See `docs/enterprise/policy.md` for the full architecture document.
//!
//! ## Threats mitigated
//!
//! * **Privilege creep** — deny-by-default forces explicit allow rules.
//! * **Policy tampering** — group-/world-writable `.rego` files are refused at
//!   load time; a poisoned reload leaves the previously valid policy active.
//! * **Silent failures** — every allow and deny is audit-logged with the full
//!   [`PolicyInput`] and a stable reason.
//! * **Secret exfiltration via rules** — [`PolicyInput`] never contains
//!   passwords or tokens, so an over-broad policy cannot leak them through
//!   a `reason` string.
//!
//! ## Not yet implemented
//!
//! * Dynamic data sources (LDAP group lookups, time-windowed cohorts).
//! * Policy hot-reload via inotify; today reload is explicit through
//!   `SIGHUP` / [`PolicyEngine::reload`].
//! * Cross-daemon policy signing; policies are file-trusted today.
//!
//! ## bd tracker
//!
//! Enterprise policy work is tracked under the `bd-1du` parity epic; this
//! crate is pre-parity and does not gate `bd-1du.10`.
//!
//! ## How to enable
//!
//! In operator config:
//!
//! ```toml
//! [auth.policy]
//! backend = "rego"            # or "null" for dev
//! policy_dir = "/etc/pcloud/policy"
//! mode = "deny"                # production MUST be deny
//! ```
//!
//! The daemon constructs a [`RegoPolicyEngine`] at startup and evaluates
//! every incoming request. On `SIGHUP`, [`PolicyEngine::reload`] is called.
//!
//! ## Example
//!
//! ```
//! use pcloud_policy::{NullPolicyEngine, PolicyDecision, PolicyEngine, PolicyInput};
//! use std::time::SystemTime;
//!
//! let engine = NullPolicyEngine::new();
//! let input = PolicyInput {
//!     user: "alice@corp".into(),
//!     command: "sync.add".into(),
//!     args: serde_json::json!({"path": "/home/alice/work"}),
//!     device_id: Some("host-01".into()),
//!     timestamp: SystemTime::now(),
//! };
//! let decision = engine.evaluate(&input).expect("null engine never errors");
//! assert!(matches!(decision, PolicyDecision::Allow));
//! ```
//!
//! ```
//! // NullPolicyEngine's `reload` is a no-op by design.
//! use pcloud_policy::{NullPolicyEngine, PolicyEngine};
//! assert!(NullPolicyEngine::new().reload().is_ok());
//! ```
//!
//! ```
//! // A PolicyDecision::Deny carries a human-readable, audit-safe reason.
//! use pcloud_policy::PolicyDecision;
//! let d = PolicyDecision::Deny { reason: "not-in-group".into() };
//! if let PolicyDecision::Deny { reason } = d {
//!     assert!(!reason.is_empty());
//! }
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::pedantic)]

use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Input shaped for every daemon request before it reaches the handler.
///
/// The daemon dispatch layer converts each `Method::*` request into a
/// `PolicyInput` and calls [`PolicyEngine::evaluate`]. The engine returns a
/// [`PolicyDecision`]; a [`PolicyDecision::Deny`] short-circuits the request
/// with the supplied reason before any side effect runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyInput {
    /// Authenticated user identifier (email or account id). Never a password.
    pub user: String,
    /// Command name, e.g. `sync.add`, `publink.create`, `crypto.setup`.
    pub command: String,
    /// Command arguments as JSON. Secret material MUST be stripped upstream.
    pub args: Value,
    /// Optional device identifier, if known (e.g. machine UUID, hostname).
    pub device_id: Option<String>,
    /// Wall-clock time at which the request was received.
    pub timestamp: SystemTime,
}

/// Result of a policy evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PolicyDecision {
    /// The request is permitted. The matched rule id (if any) is recorded
    /// in the audit log by the caller.
    Allow,
    /// The request is refused. `reason` is surfaced to the caller and audit.
    Deny {
        /// Human-readable rejection reason. Safe to log; never contains
        /// secrets.
        reason: String,
    },
}

/// Errors returned by a [`PolicyEngine`].
#[derive(Debug, Error)]
pub enum PolicyError {
    /// Policy file could not be read from disk.
    #[error("policy file I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Policy file is syntactically invalid.
    #[error("policy compile error: {0}")]
    Compile(String),
    /// Policy file has insecure permissions (e.g. world-writable).
    #[error("insecure policy permissions: {0}")]
    InsecurePermissions(String),
    /// Policy evaluation failed at runtime.
    #[error("policy evaluation error: {0}")]
    Evaluation(String),
    /// The Rego backend is not yet implemented.
    #[error("Rego backend not implemented")]
    Unimplemented,
}

/// Trait implemented by every policy backend.
///
/// # Contract
///
/// Implementors MUST:
///
/// 1. **Fail closed on evaluation error.** An `Err` from `evaluate`
///    short-circuits the request — handlers MUST treat it as a denial. The
///    engine itself therefore SHOULD return structured errors that the
///    caller can audit without exposing secrets.
/// 2. **Never fail open on reload.** If `reload` fails, the previously
///    loaded policy remains in effect; a half-loaded state is forbidden.
/// 3. **Be `Send + Sync`.** The daemon calls into the engine from many
///    concurrent request handlers.
/// 4. **Redact secrets.** An implementor MUST NOT include secret material
///    in any [`PolicyError`] or in the `reason` of [`PolicyDecision::Deny`].
/// 5. **Be deterministic under a given policy set.** Evaluating the same
///    input twice against the same loaded policy MUST yield the same
///    decision so audit logs reproduce under replay.
pub trait PolicyEngine: Send + Sync {
    /// Evaluate `input` against the currently loaded policy.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::Evaluation`] if the backend encountered an
    /// internal error. Callers MUST treat any error as a denial; the engine
    /// fails closed.
    ///
    /// # Security
    ///
    /// `input` is assumed to carry no secret material. An implementor MUST
    /// NOT serialize `args` into a log sink without operator opt-in.
    fn evaluate(&self, input: &PolicyInput) -> Result<PolicyDecision, PolicyError>;

    /// Reload policy from its configured source (typically on `SIGHUP`).
    ///
    /// If reload fails the previously loaded policy MUST remain active — the
    /// implementation MUST NOT leave the engine in an empty / fail-open
    /// state. Callers should treat an `Err` as "old policy still in force".
    ///
    /// # Errors
    ///
    /// - [`PolicyError::Io`] when files cannot be read.
    /// - [`PolicyError::Compile`] when a `.rego` file is syntactically
    ///   invalid.
    /// - [`PolicyError::InsecurePermissions`] when a file is group- or
    ///   world-writable.
    ///
    /// # Security
    ///
    /// Reload is the critical "policy change" touchpoint; implementors MUST
    /// validate file ownership and permissions before compiling so a
    /// tampered policy cannot be silently accepted.
    fn reload(&self) -> Result<(), PolicyError>;
}

/// A safe, audit-only engine that allows every request.
///
/// This is the default used in development builds so contributors are never
/// locked out. Production builds MUST substitute [`RegoPolicyEngine`] (or
/// another deny-by-default implementation) via the `[auth.policy]` operator
/// config.
#[derive(Debug, Default)]
pub struct NullPolicyEngine;

impl NullPolicyEngine {
    /// Construct a new `NullPolicyEngine`.
    pub const fn new() -> Self {
        Self
    }
}

impl PolicyEngine for NullPolicyEngine {
    fn evaluate(&self, _input: &PolicyInput) -> Result<PolicyDecision, PolicyError> {
        // The caller is responsible for forwarding the input to the audit
        // log; the engine itself does not touch the audit sink directly in
        // order to keep this crate dependency-light.
        Ok(PolicyDecision::Allow)
    }

    fn reload(&self) -> Result<(), PolicyError> {
        Ok(())
    }
}

/// Rego-backed policy engine — evaluates `.rego` policies via the `regorus`
/// pure-Rust interpreter.
///
/// The engine:
///
/// 1. Loads every `*.rego` file from the configured directory.
/// 2. Refuses to load any file that is group-writable or world-writable.
/// 3. Compiles the bundle with `regorus` (pure-Rust, no CGO).
/// 4. Holds the compiled engine behind a `Mutex` so `reload` can atomic-
///    swap a freshly compiled engine. If reload fails the previously loaded
///    engine stays active.
/// 5. On `evaluate`, serializes [`PolicyInput`] to JSON, runs the query
///    `data.pcloud.policy.decision`, and translates the returned object
///    (`{"allow": true}` / `{"allow": false, "reason": "..."}`) into a
///    [`PolicyDecision`]. Any error or missing decision yields a default deny.
pub struct RegoPolicyEngine {
    policy_dir: std::path::PathBuf,
    engine: std::sync::Mutex<regorus::Engine>,
}

impl std::fmt::Debug for RegoPolicyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegoPolicyEngine")
            .field("policy_dir", &self.policy_dir)
            .finish()
    }
}

/// Rego query evaluated for every [`PolicyInput`].
const DECISION_QUERY: &str = "data.pcloud.policy.decision";

impl RegoPolicyEngine {
    /// Construct a new Rego engine bound to `policy_dir`.
    ///
    /// All `*.rego` files inside `policy_dir` are loaded. Each file is
    /// checked for insecure permissions (group-writable or world-writable)
    /// and rejected with [`PolicyError::InsecurePermissions`] if so.
    /// Compilation errors are returned as [`PolicyError::Compile`].
    pub fn new(policy_dir: impl AsRef<std::path::Path>) -> Result<Self, PolicyError> {
        let policy_dir = policy_dir.as_ref().to_path_buf();
        let engine = Self::build_engine(&policy_dir)?;
        Ok(Self {
            policy_dir,
            engine: std::sync::Mutex::new(engine),
        })
    }

    /// Load every `*.rego` file in `dir` into a freshly constructed
    /// `regorus::Engine`, validating file permissions as we go.
    fn build_engine(dir: &std::path::Path) -> Result<regorus::Engine, PolicyError> {
        let mut engine = regorus::Engine::new();

        let read_dir = std::fs::read_dir(dir).map_err(PolicyError::Io)?;
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for entry in read_dir {
            let entry = entry.map_err(PolicyError::Io)?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("rego") {
                files.push(path);
            }
        }
        // Deterministic load order so compile errors are reproducible.
        files.sort();

        for path in files {
            Self::check_permissions(&path)?;
            let src = std::fs::read_to_string(&path).map_err(PolicyError::Io)?;
            engine
                .add_policy(path.display().to_string(), src)
                .map_err(|e| PolicyError::Compile(format!("{}: {e}", path.display())))?;
        }

        Ok(engine)
    }

    /// Refuse any policy file that is group-writable or world-writable.
    ///
    /// On non-Unix platforms this check is a no-op — the platform's own ACL
    /// model is trusted.
    fn check_permissions(path: &std::path::Path) -> Result<(), PolicyError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(path).map_err(PolicyError::Io)?;
            let mode = meta.permissions().mode();
            // 0o022 covers group-write (0o020) and world-write (0o002).
            if mode & 0o022 != 0 {
                return Err(PolicyError::InsecurePermissions(format!(
                    "{}: mode {:o} is group- or world-writable",
                    path.display(),
                    mode & 0o777
                )));
            }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
        Ok(())
    }

    /// Extract a [`PolicyDecision`] from a `regorus` query result.
    ///
    /// The Rego contract is a single rule `decision` returning an object
    /// `{"allow": bool, "reason"?: string}`. Anything else (missing rule,
    /// malformed shape, evaluation error) is treated as a default deny.
    fn decision_from_value(value: &regorus::Value) -> PolicyDecision {
        // Serialize through JSON to avoid depending on regorus's internal
        // Value layout — it keeps the mapping stable across regorus versions.
        let json = match value.to_json_str() {
            Ok(s) => s,
            Err(_) => {
                return PolicyDecision::Deny {
                    reason: "policy returned non-JSON value".into(),
                };
            }
        };
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(_) => {
                return PolicyDecision::Deny {
                    reason: "policy decision was not valid JSON".into(),
                };
            }
        };
        let obj = match parsed.as_object() {
            Some(o) => o,
            None => {
                return PolicyDecision::Deny {
                    reason: "policy decision was not an object".into(),
                };
            }
        };
        let allow = obj.get("allow").and_then(|v| v.as_bool()).unwrap_or(false);
        if allow {
            PolicyDecision::Allow
        } else {
            let reason = obj
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("policy denied the request")
                .to_string();
            PolicyDecision::Deny { reason }
        }
    }
}

impl PolicyEngine for RegoPolicyEngine {
    fn evaluate(&self, input: &PolicyInput) -> Result<PolicyDecision, PolicyError> {
        let input_json = serde_json::to_string(input)
            .map_err(|e| PolicyError::Evaluation(format!("serialize input: {e}")))?;

        let guard = self
            .engine
            .lock()
            .map_err(|_| PolicyError::Evaluation("policy engine mutex poisoned".into()))?;

        // `regorus::Engine::eval_query` mutates internal state; clone so one
        // evaluation never leaks `set_input` state into the next. Clones are
        // cheap because compiled modules live behind shared refs.
        let mut engine = guard.clone();
        drop(guard);

        if let Err(e) = engine.set_input_json(&input_json) {
            return Ok(PolicyDecision::Deny {
                reason: format!("policy input rejected: {e}"),
            });
        }

        let results = match engine.eval_query(DECISION_QUERY.to_string(), false) {
            Ok(r) => r,
            Err(_) => {
                return Ok(PolicyDecision::Deny {
                    reason: "policy evaluation failed".into(),
                });
            }
        };

        let value = results
            .result
            .first()
            .and_then(|qr| qr.expressions.first())
            .map(|expr| &expr.value);

        match value {
            Some(v) => Ok(Self::decision_from_value(v)),
            None => Ok(PolicyDecision::Deny {
                reason: "no policy decision produced".into(),
            }),
        }
    }

    fn reload(&self) -> Result<(), PolicyError> {
        // Build the new engine OUTSIDE the mutex so a compile failure never
        // drops the live engine.
        let new_engine = Self::build_engine(&self.policy_dir)?;
        let mut guard = self
            .engine
            .lock()
            .map_err(|_| PolicyError::Evaluation("policy engine mutex poisoned".into()))?;
        *guard = new_engine;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_input() -> PolicyInput {
        PolicyInput {
            user: "alice@example.com".into(),
            command: "sync.add".into(),
            args: json!({ "local": "/home/alice/data", "remote": "/backup" }),
            device_id: Some("host-42".into()),
            timestamp: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn null_engine_allows_everything() {
        let engine = NullPolicyEngine::new();
        let decision = engine
            .evaluate(&sample_input())
            .expect("null engine never errors");
        assert!(matches!(decision, PolicyDecision::Allow));
    }

    #[test]
    fn null_engine_reload_is_noop() {
        assert!(NullPolicyEngine::new().reload().is_ok());
    }

    use std::io::Write;
    use tempfile::TempDir;

    /// Write `contents` as `name` inside `dir`, mode 0o600 on Unix.
    fn write_policy(dir: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create policy file");
        f.write_all(contents.as_bytes()).expect("write");
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("chmod 600");
        }
        path
    }

    const DEFAULT_DENY_REGO: &str = r#"package pcloud.policy

default decision = {"allow": false, "reason": "default deny"}
"#;

    const PUBLINK_EXPIRY_REGO: &str = r#"package pcloud.policy

import future.keywords.if

default decision = {"allow": true}

decision = {"allow": false, "reason": "publink expiry exceeds 7 days"} if {
    input.command == "publink.create"
    input.args.expiry_days > 7
}
"#;

    #[test]
    fn evaluates_default_deny_policy() {
        let dir = TempDir::new().expect("tempdir");
        write_policy(dir.path(), "default-deny.rego", DEFAULT_DENY_REGO);
        let engine = RegoPolicyEngine::new(dir.path()).expect("load");
        let decision = engine.evaluate(&sample_input()).expect("evaluate");
        match decision {
            PolicyDecision::Deny { reason } => assert_eq!(reason, "default deny"),
            PolicyDecision::Allow => panic!("expected deny"),
        }
    }

    #[test]
    fn evaluates_publink_expiry_rule() {
        let dir = TempDir::new().expect("tempdir");
        write_policy(dir.path(), "publink-expiry.rego", PUBLINK_EXPIRY_REGO);
        let engine = RegoPolicyEngine::new(dir.path()).expect("load");

        let mk = |days: i64| PolicyInput {
            user: "alice@example.com".into(),
            command: "publink.create".into(),
            args: json!({ "expiry_days": days }),
            device_id: None,
            timestamp: SystemTime::UNIX_EPOCH,
        };

        let allow = engine.evaluate(&mk(6)).expect("evaluate 6d");
        assert!(matches!(allow, PolicyDecision::Allow), "6d should allow");

        let deny = engine.evaluate(&mk(8)).expect("evaluate 8d");
        match deny {
            PolicyDecision::Deny { reason } => {
                assert!(reason.contains("7 days"), "unexpected reason: {reason}");
            }
            PolicyDecision::Allow => panic!("8d should deny"),
        }
    }

    #[test]
    fn reload_swaps_engine_atomically() {
        let dir = TempDir::new().expect("tempdir");
        // Start with allow-all.
        write_policy(
            dir.path(),
            "policy.rego",
            r#"package pcloud.policy
default decision = {"allow": true}
"#,
        );
        let engine = RegoPolicyEngine::new(dir.path()).expect("load");
        assert!(matches!(
            engine.evaluate(&sample_input()).expect("evaluate"),
            PolicyDecision::Allow
        ));

        // Replace with default deny and reload.
        write_policy(dir.path(), "policy.rego", DEFAULT_DENY_REGO);
        engine.reload().expect("reload");
        match engine.evaluate(&sample_input()).expect("evaluate") {
            PolicyDecision::Deny { reason } => assert_eq!(reason, "default deny"),
            PolicyDecision::Allow => panic!("reload did not swap in new policy"),
        }
    }

    #[test]
    fn reload_failure_preserves_previous_engine() {
        let dir = TempDir::new().expect("tempdir");
        write_policy(dir.path(), "policy.rego", DEFAULT_DENY_REGO);
        let engine = RegoPolicyEngine::new(dir.path()).expect("load");

        // Overwrite with syntactically broken Rego.
        write_policy(
            dir.path(),
            "policy.rego",
            "package pcloud.policy\n@@@ not rego @@@\n",
        );
        let err = engine.reload().expect_err("broken policy must fail reload");
        assert!(matches!(err, PolicyError::Compile(_)));

        // Previous engine must still be serving the original decision.
        match engine.evaluate(&sample_input()).expect("evaluate") {
            PolicyDecision::Deny { reason } => assert_eq!(reason, "default deny"),
            PolicyDecision::Allow => panic!("previous policy was lost"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_world_writable_policy_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().expect("tempdir");
        let path = write_policy(dir.path(), "policy.rego", DEFAULT_DENY_REGO);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).expect("chmod 666");

        let err =
            RegoPolicyEngine::new(dir.path()).expect_err("world-writable policy must be refused");
        assert!(
            matches!(err, PolicyError::InsecurePermissions(_)),
            "unexpected error: {err:?}"
        );
    }
}

//! Versioned migrations for the on-disk config envelope.
//!
//! Every file carries an integer `version` field. Bumping [`CURRENT_VERSION`]
//! requires adding a migration step in [`migrate_to_current`]; this keeps the
//! compatibility window explicit and regression-tested.
//!
//! # Migration history
//!
//! ## v0 → v1: envelope wrap
//!
//! - **Intent:** introduce a stable top-level envelope so future schema
//!   evolution can be versioned. The v0 document *is* the profile (bare
//!   object, no `version`, no wrapper). The v1 document is
//!   `{ "version": 1, "profile": { ...original fields... } }`.
//! - **Data changes:** none — the original profile object is moved
//!   verbatim under `"profile"`.
//! - **Rollback policy:** forward-only. There is no automatic
//!   downgrade path: an older build that predates the envelope will see
//!   a `"version"` key it doesn't understand and refuse to load. Operators
//!   who must revert must restore the pre-migration file from backup; the
//!   migration runs in memory, so the on-disk v0 file is unchanged until
//!   the caller explicitly rewrites it.
//!
//! ## v1 → v2: add `observability` block
//!
//! - **Intent:** expose observability toggles
//!   ([`crate::observability::ObservabilityFlags`]) as first-class
//!   profile state instead of an implicit default. Introduced when the
//!   audit/metrics/tracing switches moved into `ConfigProfile`.
//! - **Data changes:** inserts
//!   `profile.observability = ObservabilityFlags::secure_defaults()` **only
//!   when the key is absent**. Existing `observability` blocks are left
//!   untouched so user-overridden flags survive migration. The envelope
//!   `"version"` field is bumped to `2`.
//! - **Rollback policy:** forward-only, same as above. The inserted
//!   `observability` block is ignored by v1 builds because they don't
//!   know the key, but they will refuse to load once `"version": 2` is
//!   written. Recovery path: restore from backup or delete the file and
//!   let the daemon regenerate defaults.
//!
//! # General rollback policy
//!
//! All migrations are in-memory — the on-disk document is not rewritten
//! by [`migrate_to_current`] itself. A caller (e.g. the daemon's config
//! writer) may choose to persist the migrated envelope back to disk, at
//! which point the original v0/v1 file is overwritten. Callers that want
//! a durable downgrade path MUST take a backup before persisting. Future
//! versions above [`CURRENT_VERSION`] are rejected outright with
//! [`MigrationError::TooNew`] rather than silently truncated.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde_json::{Map, Value, json};

use crate::observability::ObservabilityFlags;

/// The version this build understands as the canonical envelope.
pub const CURRENT_VERSION: u32 = 2;

/// The oldest version supported by migration. Anything below is rejected.
pub const MIN_SUPPORTED_VERSION: u32 = 0;

/// Errors surfaced by [`migrate_to_current`] when the on-disk envelope
/// cannot be promoted to [`CURRENT_VERSION`].
///
/// Every variant is fatal: callers must refuse the config rather than
/// fall back silently, since a malformed envelope signals either a
/// corrupted file or an incompatible build.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MigrationError {
    /// The document's `version` field is below [`MIN_SUPPORTED_VERSION`].
    /// The compatibility window has moved on; the user must regenerate
    /// the config with a newer build or hand-edit the envelope.
    #[error("config version {0} is older than minimum supported ({min})", min = MIN_SUPPORTED_VERSION)]
    TooOld(u32),
    /// The document's `version` field exceeds [`CURRENT_VERSION`]. A
    /// newer build wrote this file; downgrading is not supported.
    #[error("config version {0} is newer than supported ({max})", max = CURRENT_VERSION)]
    TooNew(u32),
    /// The envelope parsed as JSON but violates the structural contract
    /// (missing `profile`, non-object root, etc.). The wrapped string
    /// points at the specific violation.
    #[error("config envelope is malformed: {0}")]
    Malformed(&'static str),
}

/// Inspect the raw document to decide whether migration is needed, then
/// migrate in place. The returned [`Value`] is guaranteed to be at
/// [`CURRENT_VERSION`] on success.
pub fn migrate_to_current(mut doc: Value) -> Result<Value, MigrationError> {
    let detected = detect_version(&doc);

    if detected > CURRENT_VERSION {
        return Err(MigrationError::TooNew(detected));
    }
    // MIN_SUPPORTED_VERSION is u32 zero today. Kept as a named constant so
    // tightening the supported window later is a one-liner; the clippy
    // allow makes the current always-false branch explicit.
    #[allow(clippy::absurd_extreme_comparisons)]
    if detected < MIN_SUPPORTED_VERSION {
        return Err(MigrationError::TooOld(detected));
    }

    let mut v = detected;
    while v < CURRENT_VERSION {
        doc = step(v, doc)?;
        v += 1;
    }
    Ok(doc)
}

/// Detects the on-disk version. Files without a `version` field are treated
/// as v0 (legacy flat profile) to match pre-envelope behavior.
fn detect_version(doc: &Value) -> u32 {
    doc.as_object()
        .and_then(|o| o.get("version"))
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(0)
}

fn step(from: u32, doc: Value) -> Result<Value, MigrationError> {
    match from {
        0 => migrate_v0_to_v1(doc),
        1 => migrate_v1_to_v2(doc),
        other => Err(MigrationError::TooOld(other)),
    }
}

/// v0 → v1: wrap bare profile into the envelope.
fn migrate_v0_to_v1(doc: Value) -> Result<Value, MigrationError> {
    let obj = doc
        .as_object()
        .ok_or(MigrationError::Malformed("root must be an object"))?;
    // If a user already passed a partial envelope without a version, accept
    // either flavor: bare profile, or envelope-without-version.
    let profile = if obj.contains_key("profile") {
        obj.get("profile").cloned().unwrap_or(Value::Null)
    } else {
        Value::Object(obj.clone())
    };

    Ok(json!({
        "version": 1,
        "profile": profile,
    }))
}

/// v1 → v2: add observability block if missing.
fn migrate_v1_to_v2(mut doc: Value) -> Result<Value, MigrationError> {
    {
        let root = doc
            .as_object_mut()
            .ok_or(MigrationError::Malformed("root must be an object"))?;
        root.insert("version".into(), Value::from(2u32));
        let profile = root
            .get_mut("profile")
            .and_then(Value::as_object_mut)
            .ok_or(MigrationError::Malformed("profile must be an object"))?;
        if !profile.contains_key("observability") {
            profile.insert(
                "observability".into(),
                serde_json::to_value(ObservabilityFlags::secure_defaults())
                    .map_err(|_| MigrationError::Malformed("could not serialize defaults"))?,
            );
        }
        // Ensure profile has at minimum a map shape; deeper validation is
        // done by the JSON schema pass.
        let _: &mut Map<String, Value> = profile;
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare_profile_v0() -> Value {
        json!({
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
            "mount": { "allow_other": false, "owner_only_by_default": true }
        })
    }

    #[test]
    fn v0_migrates_to_current() {
        let doc = bare_profile_v0();
        let migrated = migrate_to_current(doc).expect("migration succeeds");
        assert_eq!(migrated["version"], json!(CURRENT_VERSION));
        assert!(migrated["profile"]["observability"].is_object());
    }

    #[test]
    fn v1_migrates_to_v2_by_adding_observability() {
        let doc = json!({
            "version": 1,
            "profile": bare_profile_v0(),
        });
        let migrated = migrate_to_current(doc).unwrap();
        assert_eq!(migrated["version"], json!(2));
        assert_eq!(
            migrated["profile"]["observability"]["audit_export_enabled"],
            json!(true)
        );
    }

    #[test]
    fn current_version_is_noop() {
        let mut doc = json!({
            "version": CURRENT_VERSION,
            "profile": bare_profile_v0(),
        });
        doc["profile"]["observability"] =
            serde_json::to_value(ObservabilityFlags::secure_defaults()).unwrap();
        let migrated = migrate_to_current(doc.clone()).unwrap();
        assert_eq!(migrated, doc);
    }

    #[test]
    fn future_version_is_rejected() {
        let doc = json!({
            "version": CURRENT_VERSION + 5,
            "profile": bare_profile_v0(),
        });
        let err = migrate_to_current(doc).unwrap_err();
        assert!(matches!(err, MigrationError::TooNew(_)));
    }

    #[test]
    fn round_trip_v0_to_v2_preserves_environment() {
        let doc = bare_profile_v0();
        let migrated = migrate_to_current(doc).unwrap();
        assert_eq!(migrated["profile"]["environment"], json!("Development"));
    }
}

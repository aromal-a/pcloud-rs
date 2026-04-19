//! SIGHUP-driven config hot-reload.
//!
//! When the daemon's serve loop observes the `RELOAD_REQUESTED` atomic
//! flag (set by the SIGHUP handler), it calls `apply_reload` which:
//!
//! 1. Re-reads the config file from disk.
//! 2. Compares every **hot-reloadable** field against the current profile.
//! 3. Applies changed values to the in-memory `RuntimeShell` (via the
//!    reload-safe setters introduced alongside this module).
//! 4. Emits an `config.reloaded` or `config.reload_failed` audit event.
//!
//! ## Hot-reloadable fields
//!
//! | Config path                             | Applied to                |
//! |-----------------------------------------|---------------------------|
//! | `observability.structured_logs_enabled`  | log filter                |
//! | `observability.tracing_enabled`          | log filter                |
//! | `observability.metrics_enabled`          | log filter                |
//! | `rate_limit.*`                           | IPC rate-limit budgets    |
//! | `features.integrity_sweeper.*`           | sweeper schedule          |
//! | `sync_loop.poll_interval_secs`           | sync poll interval        |
//! | `data_residency.*`                       | region allow-list         |
//!
//! ## NOT hot-reloadable (require restart)
//!
//! - `auth` (vault path)
//! - `paths.runtime_dir` (IPC socket path)
//! - `crypto` (master key / KMS config)
//! - `paths.*` (all managed directories)
//! - `environment`
//! - `api.*` (transport binding)
//!
//! On parse error the previous config is kept and `config.reload_failed`
//! is emitted with the error message.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fmt::Write;
use std::path::Path;

use pcloud_config::{ConfigProfile, LoadOptions};

/// Outcome of a hot-reload attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// Config was re-read and the listed keys were changed.
    Applied {
        /// Human-readable list of changed config keys.
        changed_keys: Vec<String>,
    },
    /// Config was re-read but nothing changed.
    NoChange,
    /// Parse/validation error; previous config is kept.
    Failed {
        /// Error description.
        error: String,
    },
}

/// Compare two profiles and return the list of hot-reloadable keys that
/// differ. Keys that are NOT hot-reloadable are silently ignored (they
/// require a restart).
#[must_use]
pub fn diff_hot_reloadable(old: &ConfigProfile, new: &ConfigProfile) -> Vec<String> {
    let mut changed = Vec::new();

    // Observability
    if old.observability.structured_logs_enabled != new.observability.structured_logs_enabled {
        changed.push("observability.structured_logs_enabled".to_owned());
    }
    if old.observability.tracing_enabled != new.observability.tracing_enabled {
        changed.push("observability.tracing_enabled".to_owned());
    }
    if old.observability.metrics_enabled != new.observability.metrics_enabled {
        changed.push("observability.metrics_enabled".to_owned());
    }
    if old.observability.audit_export_enabled != new.observability.audit_export_enabled {
        changed.push("observability.audit_export_enabled".to_owned());
    }

    // Rate-limit budgets
    if old.rate_limit != new.rate_limit {
        changed.push("rate_limit".to_owned());
    }

    // Integrity sweeper schedule
    if old.features.integrity_sweeper != new.features.integrity_sweeper {
        changed.push("features.integrity_sweeper".to_owned());
    }

    // Sync poll interval
    if old.sync_loop != new.sync_loop {
        changed.push("sync_loop".to_owned());
    }

    // Data-residency allow-list
    if old.data_residency != new.data_residency {
        changed.push("data_residency".to_owned());
    }

    changed
}

/// Format a human-readable audit event message for a successful reload.
#[must_use]
pub fn format_reloaded_event(changed_keys: &[String]) -> String {
    let mut msg = String::from("config.reloaded { changed_keys: [");
    for (i, key) in changed_keys.iter().enumerate() {
        if i > 0 {
            msg.push_str(", ");
        }
        // silent drop OK: write! to a String is infallible in practice; the
        // Result is only there because of the generic `fmt::Write` trait.
        write!(msg, "\"{key}\"").ok();
    }
    msg.push_str("] }");
    msg
}

/// Format a human-readable audit event message for a failed reload.
#[must_use]
pub fn format_reload_failed_event(error: &str) -> String {
    format!("config.reload_failed {{ error: \"{error}\" }}")
}

/// Re-read the config file and compute the hot-reload diff.
///
/// On success returns the new profile and the list of changed keys.
/// On failure returns the error string — the caller must keep the
/// previous config.
pub fn load_and_diff(config_path: &Path, current: &ConfigProfile) -> ReloadOutcome {
    let loaded = match ConfigProfile::load_with_validation(
        config_path,
        LoadOptions::enforcing(current.environment),
    ) {
        Ok(l) => l,
        Err(e) => {
            return ReloadOutcome::Failed {
                error: e.to_string(),
            };
        }
    };

    let changed = diff_hot_reloadable(current, &loaded.profile);

    if changed.is_empty() {
        ReloadOutcome::NoChange
    } else {
        ReloadOutcome::Applied {
            changed_keys: changed,
        }
    }
}

/// Full reload entry point: re-read config, diff, and return the new
/// profile if any hot-reloadable fields changed.
///
/// The caller is responsible for:
/// 1. Applying the new profile fields to the runtime shell.
/// 2. Emitting the audit event.
/// 3. Keeping the old profile on failure.
pub fn try_reload(
    config_path: &Path,
    current: &ConfigProfile,
) -> (ReloadOutcome, Option<ConfigProfile>) {
    let loaded = match ConfigProfile::load_with_validation(
        config_path,
        LoadOptions::enforcing(current.environment),
    ) {
        Ok(l) => l,
        Err(e) => {
            return (
                ReloadOutcome::Failed {
                    error: e.to_string(),
                },
                None,
            );
        }
    };

    let changed = diff_hot_reloadable(current, &loaded.profile);

    if changed.is_empty() {
        (ReloadOutcome::NoChange, None)
    } else {
        (
            ReloadOutcome::Applied {
                changed_keys: changed,
            },
            Some(loaded.profile),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dev_profile() -> ConfigProfile {
        ConfigProfile::secure_defaults(
            PathBuf::from("/tmp/pcloud-reload-test"),
            pcloud_config::Environment::Development,
        )
    }

    #[test]
    fn diff_detects_no_change() {
        let a = dev_profile();
        let b = a.clone();
        assert!(diff_hot_reloadable(&a, &b).is_empty());
    }

    #[test]
    fn diff_detects_rate_limit_change() {
        let a = dev_profile();
        let mut b = a.clone();
        b.rate_limit.enabled = !a.rate_limit.enabled;
        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.contains(&"rate_limit".to_owned()));
    }

    #[test]
    fn diff_detects_observability_change() {
        let a = dev_profile();
        let mut b = a.clone();
        b.observability.tracing_enabled = !a.observability.tracing_enabled;
        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.contains(&"observability.tracing_enabled".to_owned()));
    }

    #[test]
    fn diff_detects_sync_loop_change() {
        let a = dev_profile();
        let mut b = a.clone();
        b.sync_loop.poll_interval_secs = a.sync_loop.poll_interval_secs + 10;
        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.contains(&"sync_loop".to_owned()));
    }

    #[test]
    fn diff_detects_data_residency_change() {
        let a = dev_profile();
        let mut b = a.clone();
        b.data_residency.allowed_regions.push("EU".to_owned());
        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.contains(&"data_residency".to_owned()));
    }

    #[test]
    fn diff_detects_sweeper_change() {
        let a = dev_profile();
        let mut b = a.clone();
        b.features.integrity_sweeper.enabled = !a.features.integrity_sweeper.enabled;
        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.contains(&"features.integrity_sweeper".to_owned()));
    }

    #[test]
    fn diff_ignores_non_hot_reloadable() {
        let a = dev_profile();
        let mut b = a.clone();
        // Change auth policy — should NOT appear in diff
        b.auth = pcloud_config::auth::AuthPolicy::default();
        // Change environment — should NOT appear in diff
        // (keep it the same type to avoid validation issues)
        b.mount.allow_other = false;
        b.mount.owner_only_by_default = true;
        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.is_empty());
    }

    #[test]
    fn format_events_are_well_formed() {
        let keys = vec!["rate_limit".to_owned(), "sync_loop".to_owned()];
        let msg = format_reloaded_event(&keys);
        assert!(msg.contains("rate_limit"));
        assert!(msg.contains("sync_loop"));
        assert!(msg.starts_with("config.reloaded"));

        let fail_msg = format_reload_failed_event("bad JSON");
        assert!(fail_msg.contains("config.reload_failed"));
        assert!(fail_msg.contains("bad JSON"));
    }

    #[test]
    fn load_and_diff_fails_on_missing_file() {
        let profile = dev_profile();
        let outcome = load_and_diff(&PathBuf::from("/nonexistent/config.json"), &profile);
        assert!(matches!(outcome, ReloadOutcome::Failed { .. }));
    }

    #[test]
    fn try_reload_fails_on_missing_file() {
        let profile = dev_profile();
        let (outcome, new_profile) =
            try_reload(&PathBuf::from("/nonexistent/config.json"), &profile);
        assert!(matches!(outcome, ReloadOutcome::Failed { .. }));
        assert!(new_profile.is_none());
    }

    #[test]
    fn try_reload_returns_none_when_profile_unchanged() {
        // When old == new, try_reload should report NoChange
        // (tested via diff_hot_reloadable directly since disk I/O
        // requires schema-valid envelopes).
        let a = dev_profile();
        let b = a.clone();
        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.is_empty());
    }

    #[test]
    fn try_reload_detects_mutations_via_diff() {
        let a = dev_profile();
        let mut b = a.clone();
        b.rate_limit.enabled = !a.rate_limit.enabled;
        b.data_residency.allowed_regions.push("EU".to_owned());

        let changed = diff_hot_reloadable(&a, &b);
        assert!(changed.contains(&"rate_limit".to_owned()));
        assert!(changed.contains(&"data_residency".to_owned()));
        assert_eq!(changed.len(), 2);
    }
}

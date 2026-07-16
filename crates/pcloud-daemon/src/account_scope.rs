//! T2.8.b — account-scoped bootstrap inputs.
//!
//! When the daemon is launched as a per-account sub-daemon under the
//! `pcloud_supervisor::SupervisorRegistry`, each instance must
//! isolate its on-disk state, auth-vault file, and IPC socket from
//! every other account so that two accounts can run concurrently
//! without colliding. This module ships the value type
//! ([`AccountScope`]) that callers pass into
//! [`crate::bootstrap::bootstrap_with_config_and_account`] to drive
//! that isolation.
//!
//! Single-tenant bootstrap (no `AccountScope`) keeps the historical
//! single-tree layout under `paths.state_dir` / `paths.runtime_dir`
//! verbatim. Account-scoped bootstrap inserts an `account-{id}`
//! prefix under both roots so the per-account store, vault, and IPC
//! socket sit beside (not on top of) the single-tenant ones.
//!
//! # Path layout
//!
//! For an [`AccountScope`] with `id = 7` and `label = "work"`:
//!
//! - store    → `<state_dir>/account-7/store.sqlite3`
//! - vault    → `<state_dir>/account-7/vault.dat`
//! - socket   → `<runtime_dir>/account-7/ipc.sock`
//!
//! Each per-account directory is provisioned with `0700` perms to
//! match the existing single-tenant security posture (see the
//! `state_dir_mode` / `socket_dir_mode` block in
//! [`crate::bootstrap::bootstrap_with_config`]).

// **PLATFORM:** all
// **GATING:** none.

use std::path::{Path, PathBuf};

/// Per-account scope that drives isolated on-disk paths during
/// daemon bootstrap.
///
/// Constructed from a `pcloud_supervisor::AccountSlot` (the
/// supervisor's stable `id` becomes [`AccountScope::id`] and the
/// operator-friendly slot label becomes [`AccountScope::label`])
/// or from a `--account` CLI flag once the supervisor wires sub-
/// daemon spawning. The value is intentionally `Clone` so it can
/// flow through both bootstrap and downstream log-decoration sites
/// without owning the live registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountScope {
    /// Stable per-account identifier. Must match the corresponding
    /// `pcloud_supervisor::AccountId`.
    pub id: u64,
    /// Operator-friendly label. Used purely for log decoration; not
    /// embedded in any filesystem path (paths use `id` only so a
    /// label rename does not move state on disk).
    pub label: String,
}

impl AccountScope {
    /// Construct an [`AccountScope`] from explicit id + label.
    #[must_use]
    pub fn new(id: u64, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
        }
    }

    /// Per-account subdirectory name (e.g. `account-7`).
    ///
    /// The id-only encoding is deliberate — the label is operator-
    /// facing metadata and may be renamed without rewriting on-disk
    /// state.
    #[must_use]
    pub fn subdir_name(&self) -> String {
        format!("account-{}", self.id)
    }

    /// Resolve the per-account state directory under
    /// `paths.state_dir`.
    #[must_use]
    pub fn state_subdir(&self, state_dir: &Path) -> PathBuf {
        state_dir.join(self.subdir_name())
    }

    /// Resolve the per-account runtime directory under
    /// `paths.runtime_dir`.
    #[must_use]
    pub fn runtime_subdir(&self, runtime_dir: &Path) -> PathBuf {
        runtime_dir.join(self.subdir_name())
    }

    /// Per-account SQLite store path.
    #[must_use]
    pub fn store_path(&self, state_dir: &Path) -> PathBuf {
        self.state_subdir(state_dir).join("store.sqlite3")
    }

    /// Per-account auth-token vault path.
    #[must_use]
    pub fn vault_path(&self, state_dir: &Path) -> PathBuf {
        self.state_subdir(state_dir).join("vault.dat")
    }

    /// Per-account IPC socket path.
    #[must_use]
    pub fn socket_path(&self, runtime_dir: &Path) -> PathBuf {
        self.runtime_subdir(runtime_dir).join("ipc.sock")
    }

    /// Log-prefix decoration for tracing per-account daemon output.
    /// Example: `"[account=work]"`.
    #[must_use]
    pub fn log_prefix(&self) -> String {
        format!("[account={}]", self.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn subdir_name_uses_id_only() {
        let scope = AccountScope::new(7, "work");
        assert_eq!(scope.subdir_name(), "account-7");
        // Label rename does not change the on-disk subdir name.
        let renamed = AccountScope::new(7, "office");
        assert_eq!(renamed.subdir_name(), "account-7");
    }

    #[test]
    fn paths_compose_under_provided_roots() {
        let scope = AccountScope::new(42, "home");
        let state = PathBuf::from("/var/lib/pcloud");
        let run = PathBuf::from("/run/pcloud");
        assert_eq!(
            scope.store_path(&state),
            PathBuf::from("/var/lib/pcloud/account-42/store.sqlite3")
        );
        assert_eq!(
            scope.vault_path(&state),
            PathBuf::from("/var/lib/pcloud/account-42/vault.dat")
        );
        assert_eq!(
            scope.socket_path(&run),
            PathBuf::from("/run/pcloud/account-42/ipc.sock")
        );
    }

    #[test]
    fn log_prefix_uses_label() {
        let scope = AccountScope::new(1, "work");
        assert_eq!(scope.log_prefix(), "[account=work]");
    }
}

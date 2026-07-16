#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
//! T2.8 — multi-account supervisor scaffold.
//!
//! # AI-scope deliverable
//!
//! Per-account registry + IPC routing model. The supervisor's job
//! is to multiplex N per-account daemon instances behind one host
//! process so the CLI can drive multiple pCloud accounts without
//! running N separate daemon processes. The actual sub-daemon
//! spawning + auth-vault-per-account wiring is the load-bearing
//! follow-up — it requires refactoring `pcloud-daemon::bootstrap`
//! to accept an account scope, which is its own substantial PR.
//!
//! What this crate ships:
//!
//! - `AccountId` — typed identifier per account.
//! - `AccountSlot` — per-account state (label, socket path, status).
//! - `SupervisorRegistry` — registry of all known accounts; CRUD +
//!   "default account" pointer + status updates.
//! - `route_request(account_id, registry)` — picks the right
//!   per-account daemon socket for an incoming CLI request, with
//!   explicit `Default` / `ByLabel` / `ByEnv` resolution rules.
//!
//! All pure compute, all tested. The CLI's `pcloudc account
//! add/remove/switch/list` commands consume this model directly.

// **PLATFORM:** all
// **GATING:** none.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod spawner;
pub use spawner::{SpawnError, SpawnedDaemon, spawn_account, stop_account};

/// Stable per-account identifier. Allocated by the supervisor on
/// first `add_account` call; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccountId(u64);

impl AccountId {
    /// Construct an `AccountId` from a `u64`.
    #[must_use]
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Underlying `u64`.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Operational status of an account slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    /// Account exists but its sub-daemon is not running.
    Stopped,
    /// Sub-daemon is starting (auth vault unlocking, store
    /// migrations applying).
    Starting,
    /// Sub-daemon is live and serving requests on the slot's
    /// socket.
    Running,
    /// Sub-daemon crashed; supervisor will restart it on the next
    /// supervisor tick.
    Crashed,
}

/// Per-account state record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSlot {
    /// Stable id allocated on `add_account`.
    pub id: AccountId,
    /// Operator-friendly label (e.g. `"work"`, `"home"`). The CLI
    /// resolves `--account work` to the matching slot.
    pub label: String,
    /// Sub-daemon's IPC socket path. The supervisor synthesises
    /// it from a per-account directory under the runtime root so
    /// no two accounts share a socket.
    ///
    /// **Cross-reference (T2.8.b):** when wired through
    /// `pcloud_daemon::bootstrap::bootstrap_with_config_and_account`
    /// the corresponding `AccountScope { id, label }` causes that
    /// daemon's `paths.runtime_dir` to be rewritten to
    /// `<runtime_dir>/account-{id}` so the bootstrap-derived
    /// `ipc_socket_path()` lands inside the same per-account
    /// subtree this `socket_path` should point at. Constructing an
    /// `AccountScope` from an `AccountSlot` is therefore the bridge
    /// between the registry model and bootstrap-aware sub-daemon
    /// spawning.
    pub socket_path: PathBuf,
    /// Current operational status.
    pub status: AccountStatus,
}

/// Top-level registry of all known accounts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorRegistry {
    /// Account id → slot.
    accounts: BTreeMap<u64, AccountSlot>,
    /// Default account used when the CLI does not pass
    /// `--account`.
    default_account: Option<AccountId>,
    /// Counter for the next id allocation.
    next_id: u64,
}

/// Errors returned by registry operations.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SupervisorError {
    /// Empty / whitespace-only label.
    #[error("account label must be non-empty")]
    InvalidLabel,
    /// Label collides with an existing account.
    #[error("account label {0:?} already exists")]
    LabelTaken(String),
    /// Account id was not found in the registry.
    #[error("account {0:?} not found")]
    NotFound(AccountId),
    /// Caller asked for the default account but none was set.
    #[error("no default account is set")]
    NoDefault,
}

impl SupervisorRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new account. Returns the freshly allocated id. The
    /// first account added is also marked as default.
    ///
    /// # Errors
    ///
    /// - [`SupervisorError::InvalidLabel`] when the label is
    ///   empty / whitespace.
    /// - [`SupervisorError::LabelTaken`] when another account
    ///   already uses the same label.
    pub fn add_account(
        &mut self,
        label: impl Into<String>,
        socket_path: PathBuf,
    ) -> Result<AccountId, SupervisorError> {
        let label: String = label.into();
        if label.trim().is_empty() {
            return Err(SupervisorError::InvalidLabel);
        }
        if self.accounts.values().any(|s| s.label == label) {
            return Err(SupervisorError::LabelTaken(label));
        }
        self.next_id += 1;
        let id = AccountId::new(self.next_id);
        let slot = AccountSlot {
            id,
            label,
            socket_path,
            status: AccountStatus::Stopped,
        };
        self.accounts.insert(id.get(), slot);
        if self.default_account.is_none() {
            self.default_account = Some(id);
        }
        Ok(id)
    }

    /// Remove an account. Clears the default pointer if it was
    /// pointing at the removed account.
    ///
    /// # Errors
    ///
    /// [`SupervisorError::NotFound`] if no such account exists.
    pub fn remove_account(&mut self, id: AccountId) -> Result<(), SupervisorError> {
        if self.accounts.remove(&id.get()).is_none() {
            return Err(SupervisorError::NotFound(id));
        }
        if self.default_account == Some(id) {
            self.default_account = None;
        }
        Ok(())
    }

    /// Look up by id.
    #[must_use]
    pub fn get(&self, id: AccountId) -> Option<&AccountSlot> {
        self.accounts.get(&id.get())
    }

    /// Look up by label (case-sensitive). Returns the first match
    /// (labels are unique by construction).
    #[must_use]
    pub fn by_label(&self, label: &str) -> Option<&AccountSlot> {
        self.accounts.values().find(|s| s.label == label)
    }

    /// Set the default account.
    ///
    /// # Errors
    ///
    /// [`SupervisorError::NotFound`] if `id` is not in the
    /// registry.
    pub fn set_default(&mut self, id: AccountId) -> Result<(), SupervisorError> {
        if !self.accounts.contains_key(&id.get()) {
            return Err(SupervisorError::NotFound(id));
        }
        self.default_account = Some(id);
        Ok(())
    }

    /// Update an account's status. No-op if the id is missing.
    pub fn update_status(&mut self, id: AccountId, status: AccountStatus) {
        if let Some(slot) = self.accounts.get_mut(&id.get()) {
            slot.status = status;
        }
    }

    /// Number of accounts in the registry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    /// `true` when the registry has no accounts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Return the default account id if set.
    #[must_use]
    pub fn default_id(&self) -> Option<AccountId> {
        self.default_account
    }

    /// Iterate accounts in id order (deterministic).
    pub fn iter(&self) -> impl Iterator<Item = &AccountSlot> {
        self.accounts.values()
    }
}

/// Hint a CLI invocation supplies to pick which account a request
/// targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountHint {
    /// `--account <id>` — explicit numeric id.
    ById(AccountId),
    /// `--account <label>` — operator-supplied label.
    ByLabel(String),
    /// `PCLOUD_ACCOUNT=<label>` env-var override.
    ByEnvLabel(String),
    /// No hint; use the registry's default.
    Default,
}

/// Resolve an `AccountHint` against a [`SupervisorRegistry`] and
/// return the slot the request should route to.
///
/// # Errors
///
/// - [`SupervisorError::NotFound`] when an explicit id / label is
///   supplied but does not match.
/// - [`SupervisorError::NoDefault`] when `Default` is requested
///   but no default is set.
pub fn route_request<'r>(
    hint: &AccountHint,
    registry: &'r SupervisorRegistry,
) -> Result<&'r AccountSlot, SupervisorError> {
    match hint {
        AccountHint::ById(id) => registry.get(*id).ok_or(SupervisorError::NotFound(*id)),
        AccountHint::ByLabel(label) | AccountHint::ByEnvLabel(label) => registry
            .by_label(label)
            .ok_or_else(|| SupervisorError::NotFound(AccountId::new(0))),
        AccountHint::Default => {
            let id = registry.default_id().ok_or(SupervisorError::NoDefault)?;
            registry.get(id).ok_or(SupervisorError::NotFound(id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sock(name: &str) -> PathBuf {
        PathBuf::from(format!("/run/pcloud/{name}.sock"))
    }

    #[test]
    fn add_account_allocates_increasing_ids() {
        let mut reg = SupervisorRegistry::new();
        let a = reg.add_account("work", sock("work")).unwrap();
        let b = reg.add_account("home", sock("home")).unwrap();
        assert!(b.get() > a.get());
    }

    #[test]
    fn first_added_account_becomes_default() {
        let mut reg = SupervisorRegistry::new();
        let a = reg.add_account("work", sock("work")).unwrap();
        assert_eq!(reg.default_id(), Some(a));
        reg.add_account("home", sock("home")).unwrap();
        assert_eq!(reg.default_id(), Some(a)); // does not change
    }

    #[test]
    fn empty_label_rejected() {
        let mut reg = SupervisorRegistry::new();
        assert_eq!(
            reg.add_account("", sock("x")).unwrap_err(),
            SupervisorError::InvalidLabel
        );
        assert_eq!(
            reg.add_account("   ", sock("x")).unwrap_err(),
            SupervisorError::InvalidLabel
        );
    }

    #[test]
    fn duplicate_label_rejected() {
        let mut reg = SupervisorRegistry::new();
        reg.add_account("work", sock("work")).unwrap();
        match reg.add_account("work", sock("other")).unwrap_err() {
            SupervisorError::LabelTaken(label) => assert_eq!(label, "work"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn remove_account_clears_default_when_pointing_at_it() {
        let mut reg = SupervisorRegistry::new();
        let a = reg.add_account("work", sock("work")).unwrap();
        let b = reg.add_account("home", sock("home")).unwrap();
        assert_eq!(reg.default_id(), Some(a));
        reg.remove_account(a).unwrap();
        assert_eq!(reg.default_id(), None);
        // Removing the non-default does not touch default.
        let mut reg2 = SupervisorRegistry::new();
        reg2.add_account("work", sock("work")).unwrap();
        let h = reg2.add_account("home", sock("home")).unwrap();
        reg2.remove_account(h).unwrap();
        assert_eq!(reg2.default_id().map(|x| x.get()), Some(1));
        let _ = b;
    }

    #[test]
    fn route_default_when_unset_errors() {
        let reg = SupervisorRegistry::new();
        assert_eq!(
            route_request(&AccountHint::Default, &reg).unwrap_err(),
            SupervisorError::NoDefault
        );
    }

    #[test]
    fn route_by_label() {
        let mut reg = SupervisorRegistry::new();
        let work = reg.add_account("work", sock("work")).unwrap();
        let home = reg.add_account("home", sock("home")).unwrap();
        let slot = route_request(&AccountHint::ByLabel("home".into()), &reg).unwrap();
        assert_eq!(slot.id, home);
        let slot = route_request(&AccountHint::ByLabel("work".into()), &reg).unwrap();
        assert_eq!(slot.id, work);
    }

    #[test]
    fn route_by_env_label_treated_same_as_by_label() {
        let mut reg = SupervisorRegistry::new();
        reg.add_account("work", sock("work")).unwrap();
        let slot = route_request(&AccountHint::ByEnvLabel("work".into()), &reg).unwrap();
        assert_eq!(slot.label, "work");
    }

    #[test]
    fn route_by_id() {
        let mut reg = SupervisorRegistry::new();
        let a = reg.add_account("work", sock("work")).unwrap();
        let slot = route_request(&AccountHint::ById(a), &reg).unwrap();
        assert_eq!(slot.id, a);
    }

    #[test]
    fn route_unknown_label_errors() {
        let mut reg = SupervisorRegistry::new();
        reg.add_account("work", sock("work")).unwrap();
        let err = route_request(&AccountHint::ByLabel("nope".into()), &reg).unwrap_err();
        assert!(matches!(err, SupervisorError::NotFound(_)));
    }

    #[test]
    fn update_status_round_trips() {
        let mut reg = SupervisorRegistry::new();
        let a = reg.add_account("work", sock("work")).unwrap();
        assert_eq!(reg.get(a).unwrap().status, AccountStatus::Stopped);
        reg.update_status(a, AccountStatus::Running);
        assert_eq!(reg.get(a).unwrap().status, AccountStatus::Running);
    }

    #[test]
    fn iter_is_id_ordered() {
        let mut reg = SupervisorRegistry::new();
        reg.add_account("work", sock("work")).unwrap();
        reg.add_account("home", sock("home")).unwrap();
        reg.add_account("fun", sock("fun")).unwrap();
        let ids: Vec<u64> = reg.iter().map(|s| s.id.get()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn serde_roundtrip() {
        let mut reg = SupervisorRegistry::new();
        reg.add_account("work", sock("work")).unwrap();
        reg.add_account("home", sock("home")).unwrap();
        let json = serde_json::to_string(&reg).unwrap();
        let back: SupervisorRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(reg, back);
    }

    /// Acceptance pivot: two accounts running concurrently, each
    /// CLI invocation targets one by hint.
    #[test]
    fn end_to_end_two_accounts_route_independently() {
        let mut reg = SupervisorRegistry::new();
        let work = reg.add_account("work", sock("work")).unwrap();
        let home = reg.add_account("home", sock("home")).unwrap();
        reg.update_status(work, AccountStatus::Running);
        reg.update_status(home, AccountStatus::Running);

        // CLI invocation `pcloudc --account work status` routes to work.
        let slot = route_request(&AccountHint::ByLabel("work".into()), &reg).unwrap();
        assert_eq!(slot.id, work);
        assert_eq!(slot.socket_path, sock("work"));

        // `pcloudc status` (no --account, work is default) → work.
        let slot = route_request(&AccountHint::Default, &reg).unwrap();
        assert_eq!(slot.id, work);

        // `PCLOUD_ACCOUNT=home pcloudc status` → home.
        let slot = route_request(&AccountHint::ByEnvLabel("home".into()), &reg).unwrap();
        assert_eq!(slot.id, home);
    }
}

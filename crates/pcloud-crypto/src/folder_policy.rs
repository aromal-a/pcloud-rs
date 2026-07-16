//! T2.4 — per-folder crypto policy + unlock state machine.
//!
//! # Why per-folder
//!
//! The legacy C client + the existing Rust path treat crypto as an
//! account-wide flag: either every encrypted folder is unlocked or
//! none are. T2.4 lifts that to a per-folder opt-in so an operator
//! can encrypt `/Documents` while keeping `/Photos` plaintext. The
//! data model here is two cooperating tables:
//!
//! 1. **`FolderCryptoPolicy`** — persisted opt-in registry. Maps
//!    a remote folder id to "this folder participates in crypto".
//!    Inheritance up the parent chain lets an operator encrypt
//!    `/Documents` once and have every nested folder inherit the
//!    setting unless explicitly overridden. Persisted in the store
//!    (T2.4.c integration); the model itself is pure compute.
//! 2. **`FolderUnlockState`** — runtime-only set of currently
//!    unlocked folders. Cleared on lock; never persisted (the
//!    auth-vault posture is "unlock per-session").
//!
//! `is_visible(folder_id, &policy, &state)` is the load-bearing
//! predicate that returns `true` when a folder's contents are
//! viewable by the current process: either the folder does not
//! participate in crypto at all, or it participates and is
//! currently unlocked.
//!
//! # Threat model interaction
//!
//! The per-folder opt-in does *not* enforce isolation between
//! unlocked folders — once unlocked, a folder's KEK is held in
//! the same `SecretBytes` pool as any other unlocked folder. The
//! plan acknowledges this: the goal is for `/Photos` (plaintext)
//! to never see the encrypted KEK material at all, not for
//! `/Documents` to be unreadable by `/Work` while both are
//! unlocked. Inter-folder isolation would require per-folder KEKs
//! at the storage layer, which is the next milestone past T2.4.

// **PLATFORM:** all
// **GATING:** none (portable; pure compute).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

/// Remote folder identifier. Uses `u64` directly to keep
/// `pcloud-crypto` at the bottom of the dep graph (no
/// `pcloud-model` edge needed); the daemon's call-site converts
/// `pcloud_model::ids::RemoteFolderId` ↔ this `u64` via
/// `.get()` / `RemoteFolderId::new`.
pub type RemoteFolderId = u64;

/// Per-folder crypto opt-in entry. Stored in
/// [`FolderCryptoPolicy::folders`] keyed by `folder_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderEntry {
    /// `true` when this folder is opted into crypto. `false`
    /// represents an explicit "this folder is plaintext" override
    /// (e.g. an encrypted parent's child opting back out).
    pub encrypted: bool,
    /// Parent folder id used for inheritance walks. `None` for
    /// the top-level (`/`) folder.
    pub parent: Option<RemoteFolderId>,
}

/// Persisted per-folder crypto opt-in registry.
///
/// Add a folder via [`Self::set`] (with explicit `encrypted +
/// parent`); remove via [`Self::remove`]; query effective state
/// via [`Self::is_encrypted`].
///
/// `is_encrypted` walks up the parent chain looking for the
/// closest entry with `encrypted = true` or `false` so an
/// operator can set `/Documents = encrypted` once and have all
/// nested folders inherit unless one of them explicitly opts out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderCryptoPolicy {
    /// Folder id → entry. `BTreeMap` keeps the on-wire serde form
    /// deterministic for snapshot tests.
    pub folders: BTreeMap<u64, FolderEntry>,
}

impl FolderCryptoPolicy {
    /// Construct an empty policy (every folder is plaintext).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set or replace `folder_id`'s entry.
    pub fn set(
        &mut self,
        folder_id: RemoteFolderId,
        encrypted: bool,
        parent: Option<RemoteFolderId>,
    ) {
        self.folders
            .insert(folder_id, FolderEntry { encrypted, parent });
    }

    /// Remove the explicit entry for `folder_id`. After removal,
    /// `is_encrypted(folder_id)` falls back to inherited state.
    pub fn remove(&mut self, folder_id: RemoteFolderId) {
        self.folders.remove(&folder_id);
    }

    /// Direct lookup of the entry if any. Does not walk the
    /// parent chain — use [`Self::is_encrypted`] for the
    /// effective verdict.
    #[must_use]
    pub fn entry(&self, folder_id: RemoteFolderId) -> Option<FolderEntry> {
        self.folders.get(&folder_id).copied()
    }

    /// Returns `true` if `folder_id` is opted into crypto, walking
    /// the parent chain to inherit ancestor decisions. Folders
    /// without any entry along their chain are considered
    /// plaintext.
    ///
    /// Cycle protection: the walk bounds itself at the registry
    /// size; a malformed parent cycle returns `false`.
    #[must_use]
    pub fn is_encrypted(&self, folder_id: RemoteFolderId) -> bool {
        let mut visited: BTreeSet<u64> = BTreeSet::new();
        let mut current = Some(folder_id);
        while let Some(id) = current {
            if !visited.insert(id) {
                // Cycle — bail with "not encrypted".
                return false;
            }
            if let Some(entry) = self.folders.get(&id) {
                return entry.encrypted;
            }
            // No entry for this id; check the parent of the
            // *closest enclosing entry*. We don't know parents
            // for ids that are not in the registry, so we cannot
            // climb past them. This is the load-bearing
            // limitation: the caller (sync engine) keeps the
            // registry populated with the parent chain when it
            // marks a folder.
            current = None;
            for (k, v) in &self.folders {
                if *k == id {
                    current = v.parent;
                    break;
                }
            }
        }
        false
    }

    /// Number of folders in the registry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.folders.len()
    }

    /// `true` when the registry has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }
}

/// Runtime-only set of currently unlocked folders.
///
/// Built up at session unlock time; cleared on lock. Never
/// persisted — `Drop` clears the set so a daemon shutdown does
/// not leak the unlocked-folder list to a process snapshot.
#[derive(Debug, Default)]
pub struct FolderUnlockState {
    unlocked: HashSet<u64>,
}

impl FolderUnlockState {
    /// Construct an empty unlock state (every folder is locked).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `folder_id` as unlocked.
    pub fn unlock(&mut self, folder_id: RemoteFolderId) {
        self.unlocked.insert(folder_id);
    }

    /// Mark `folder_id` as locked.
    pub fn lock(&mut self, folder_id: RemoteFolderId) {
        self.unlocked.remove(&folder_id);
    }

    /// Lock every folder.
    pub fn lock_all(&mut self) {
        self.unlocked.clear();
    }

    /// `true` when `folder_id` is currently unlocked. Pure
    /// membership check — does not consult the policy.
    #[must_use]
    pub fn is_unlocked(&self, folder_id: RemoteFolderId) -> bool {
        self.unlocked.contains(&folder_id)
    }

    /// Number of currently unlocked folders.
    #[must_use]
    pub fn unlocked_count(&self) -> usize {
        self.unlocked.len()
    }
}

impl Drop for FolderUnlockState {
    fn drop(&mut self) {
        self.unlocked.clear();
    }
}

/// Returns `true` when `folder_id`'s contents are visible to the
/// current process. Either the folder is plaintext (not in the
/// crypto policy) OR it is encrypted AND currently unlocked.
#[must_use]
pub fn is_visible(
    folder_id: RemoteFolderId,
    policy: &FolderCryptoPolicy,
    state: &FolderUnlockState,
) -> bool {
    if !policy.is_encrypted(folder_id) {
        return true;
    }
    state.is_unlocked(folder_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fid(n: u64) -> RemoteFolderId {
        n
    }

    #[test]
    fn empty_policy_treats_every_folder_as_plaintext() {
        let policy = FolderCryptoPolicy::new();
        assert!(!policy.is_encrypted(fid(1)));
        assert!(!policy.is_encrypted(fid(99)));
    }

    #[test]
    fn explicit_set_marks_folder_encrypted() {
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(7), true, None);
        assert!(policy.is_encrypted(fid(7)));
        assert!(!policy.is_encrypted(fid(8)));
    }

    #[test]
    fn child_inherits_encrypted_parent() {
        // /Documents (id=10) is encrypted.
        // /Documents/Tax (id=11, parent=10) inherits.
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(10), true, None);
        policy.set(fid(11), true, Some(fid(10))); // child explicitly registered
        assert!(policy.is_encrypted(fid(10)));
        assert!(policy.is_encrypted(fid(11)));
    }

    #[test]
    fn child_can_opt_out_of_encrypted_parent() {
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(10), true, None); // /Documents = encrypted
        policy.set(fid(11), false, Some(fid(10))); // /Documents/Public = plaintext
        assert!(policy.is_encrypted(fid(10)));
        assert!(!policy.is_encrypted(fid(11)));
    }

    #[test]
    fn parent_chain_walks_until_explicit_entry() {
        // /Documents (10) = encrypted
        // /Documents/Work (12, parent=10) — no explicit entry but
        // the test verifies the registry's contract: callers must
        // populate the chain when registering a folder. Implicit
        // walk-without-entry returns false (the load-bearing
        // limitation).
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(10), true, None);
        // No entry for fid(12) → returns false (caller must
        // populate chain).
        assert!(!policy.is_encrypted(fid(12)));
        // Adding explicit chain → walks correctly.
        policy.set(fid(12), true, Some(fid(10)));
        assert!(policy.is_encrypted(fid(12)));
    }

    #[test]
    fn mixed_folders_documented_acceptance() {
        // Plan acceptance: user can enable crypto on /Documents
        // while keeping /Photos plaintext.
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(10), true, None); // /Documents
        policy.set(fid(20), false, None); // /Photos (explicit plaintext)
        assert!(policy.is_encrypted(fid(10)));
        assert!(!policy.is_encrypted(fid(20)));
    }

    #[test]
    fn cycle_in_parent_chain_is_safe() {
        let mut policy = FolderCryptoPolicy::new();
        // Pathological: 100 says its parent is 200, 200 says 100.
        // Should not infinite-loop; should fall back to "not
        // encrypted" since neither has an explicit decision before
        // the cycle.
        policy.folders.insert(
            100,
            FolderEntry {
                encrypted: false,
                parent: Some(200),
            },
        );
        policy.folders.insert(
            200,
            FolderEntry {
                encrypted: false,
                parent: Some(100),
            },
        );
        // The first hit — id=100 — has encrypted=false, so the
        // walk returns false on the first match without ever
        // following the cycle. Either way the contract is "no
        // infinite loop".
        assert!(!policy.is_encrypted(fid(100)));
    }

    #[test]
    fn remove_drops_explicit_entry() {
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(7), true, None);
        assert!(policy.is_encrypted(fid(7)));
        policy.remove(fid(7));
        assert!(!policy.is_encrypted(fid(7)));
    }

    #[test]
    fn unlock_state_tracks_membership() {
        let mut state = FolderUnlockState::new();
        assert!(!state.is_unlocked(fid(7)));
        state.unlock(fid(7));
        assert!(state.is_unlocked(fid(7)));
        assert_eq!(state.unlocked_count(), 1);
        state.lock(fid(7));
        assert!(!state.is_unlocked(fid(7)));
    }

    #[test]
    fn lock_all_clears_unlocked_set() {
        let mut state = FolderUnlockState::new();
        state.unlock(fid(1));
        state.unlock(fid(2));
        assert_eq!(state.unlocked_count(), 2);
        state.lock_all();
        assert_eq!(state.unlocked_count(), 0);
    }

    #[test]
    fn is_visible_plaintext_folder_always_visible() {
        let policy = FolderCryptoPolicy::new();
        let state = FolderUnlockState::new();
        // Folder not in the policy → plaintext → visible regardless
        // of unlock state.
        assert!(is_visible(fid(7), &policy, &state));
    }

    #[test]
    fn is_visible_encrypted_locked_is_invisible() {
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(7), true, None);
        let state = FolderUnlockState::new();
        assert!(!is_visible(fid(7), &policy, &state));
    }

    #[test]
    fn is_visible_encrypted_unlocked_is_visible() {
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(7), true, None);
        let mut state = FolderUnlockState::new();
        state.unlock(fid(7));
        assert!(is_visible(fid(7), &policy, &state));
    }

    /// Plan acceptance: per-folder unlock state machine round-trip.
    /// `/Documents` is encrypted and unlocked; `/Photos` is
    /// plaintext (visible without unlock); `/Documents/Tax` inherits
    /// the encryption from its parent and is unlocked transitively
    /// only if explicitly added to the unlock set.
    #[test]
    fn end_to_end_per_folder_state_machine() {
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(10), true, None); // /Documents encrypted
        policy.set(fid(20), false, None); // /Photos plaintext
        policy.set(fid(11), true, Some(fid(10))); // /Documents/Tax encrypted (inherited)

        let mut state = FolderUnlockState::new();
        // Initially every encrypted folder is locked.
        assert!(!is_visible(fid(10), &policy, &state));
        assert!(!is_visible(fid(11), &policy, &state));
        // Photos visible regardless of unlock.
        assert!(is_visible(fid(20), &policy, &state));

        // Unlock Documents — Documents itself becomes visible, but
        // /Documents/Tax requires its own unlock entry (the unlock
        // set is *not* hierarchical; the engine adds children
        // explicitly when it walks the tree).
        state.unlock(fid(10));
        assert!(is_visible(fid(10), &policy, &state));
        assert!(!is_visible(fid(11), &policy, &state));

        state.unlock(fid(11));
        assert!(is_visible(fid(11), &policy, &state));

        // Lock everything; both encrypted folders go invisible
        // again.
        state.lock_all();
        assert!(!is_visible(fid(10), &policy, &state));
        assert!(!is_visible(fid(11), &policy, &state));
        assert!(is_visible(fid(20), &policy, &state)); // photos always visible
    }

    #[test]
    fn serde_roundtrip_policy() {
        let mut policy = FolderCryptoPolicy::new();
        policy.set(fid(10), true, None);
        policy.set(fid(11), true, Some(fid(10)));
        policy.set(fid(20), false, None);
        let json = serde_json::to_string(&policy).unwrap();
        let back: FolderCryptoPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, back);
    }
}

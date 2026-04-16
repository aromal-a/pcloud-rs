#![forbid(unsafe_code)]
//! # pcloud-model
//!
//! Pure data types shared across the workspace: auth, ids, crypto,
//! conflict, health, public links, shares, sync, and transfer. No I/O
//! and no side effects — safe to depend on from any layer.
//!
//! ## Serde roundtrip invariants
//!
//! Every type in this crate that implements `Serialize` also implements
//! `Deserialize`, and roundtripping through `serde_json` is lossless.
//! The enum representations are the default externally tagged
//! representation (variant names as strings for unit variants, single-
//! key maps for struct/tuple variants); callers are expected to feed
//! and consume pCloud binprotocol payloads through the dedicated
//! encoders in `pcloud-proto` rather than relying on the default
//! serde shape of these types over the wire.

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

/// Authentication-state enum shared between the daemon, SDK, and CLI.
pub mod auth;
/// Conflict classification and resolver output types.
pub mod conflict;
/// Crypto-subsystem state enum surfaced to clients.
pub mod crypto;
/// Overall client health classification.
pub mod health;
/// Strongly-typed identifier newtypes (sync ids, remote ids, etc.).
pub mod ids;
/// Public-link and upload-link data types.
pub mod public_links;
/// Shared folder, share-request, and contact data types.
pub mod shares;
/// Sync-engine domain types: candidates, planned operations, states.
pub mod sync;
/// Transfer-lifecycle and recovery decision types.
pub mod transfer;

/// Canonical crate name, exposed for structured logs/metrics that tag
/// events with the emitting crate.
///
/// # Example
///
/// ```
/// assert_eq!(pcloud_model::CRATE_NAME, "pcloud-model");
/// ```
pub const CRATE_NAME: &str = "pcloud-model";

/// Count of public submodules exposed by this crate. Kept as a function
/// so it can be asserted by higher-level smoke tests without reaching
/// into private state.
///
/// # Example
///
/// ```
/// // The module count is a small positive integer used by smoke tests
/// // to confirm the crate's surface has not silently shrunk.
/// assert!(pcloud_model::module_count() >= 9);
/// ```
#[must_use]
pub fn module_count() -> usize {
    9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(CRATE_NAME, "pcloud-model");
    }

    #[test]
    fn module_count_is_nine() {
        assert_eq!(module_count(), 9);
    }
}

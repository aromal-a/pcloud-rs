#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
//! T2.5 — sandboxed plugin host scaffold.
//!
//! # AI-scope deliverable
//!
//! This crate ships the **capability model + message bus** that any
//! plugin-execution backend (wasmtime, a future native sandbox, an
//! out-of-process worker) plugs into. The actual `wasmtime` runtime
//! integration is the follow-up step — it pulls a heavy dep tree
//! and the plan acceptance ("a sample plugin runs sandboxed; an
//! attempted `fs::write` is denied") needs a real `wasm32-wasi`
//! sample plugin.
//!
//! # Capability model
//!
//! Plugins receive an explicit allowlist of capabilities at load
//! time. The host refuses every host-side action that is not in
//! the list. The minimum viable set:
//!
//! | Capability         | Description                                                     |
//! |--------------------|-----------------------------------------------------------------|
//! | `ReadAccountInfo`  | Read user-id / email / quota                                    |
//! | `ReadFolderListing`| Walk the remote folder tree (no file body access)               |
//! | `ReadFileMetadata` | Stat individual files (no body access)                          |
//! | `EnqueueLocalLog`  | Append a structured event to the daemon's audit log             |
//!
//! Notably absent:
//! - `WriteAnything` — no plugin can mutate state in T2.5; the
//!   message bus is read-only.
//! - `Network` — plugins cannot speak HTTP / TCP. Communication
//!   is restricted to the typed message bus.
//! - `Filesystem` — no `fs::read` / `fs::write` exposed. Plugins
//!   request remote-file metadata via the bus instead.
//!
//! This is the principle-of-least-authority posture the plan
//! demands. Future capabilities (`MutateFolderTags`, `EmitMetric`)
//! can be added one-by-one with explicit operator opt-in.
//!
//! # Backend trait
//!
//! [`PluginBackend`] is the seam where wasmtime / a future native
//! sandbox / an out-of-process worker plugs in. The
//! [`NoopBackend`] in this crate proves the call shape and
//! exercises the capability denial logic without pulling
//! wasmtime.

// **PLATFORM:** all
// **GATING:** none.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Capability the host can grant to a plugin. Granting is
/// explicit (the absence of a capability denies the
/// corresponding host-side action).
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Read user-id / email / quota.
    ReadAccountInfo,
    /// Walk remote folder listings (no file body access).
    ReadFolderListing,
    /// Stat individual files (no body access).
    ReadFileMetadata,
    /// Append a structured event to the daemon's audit log.
    EnqueueLocalLog,
}

/// Plugin identity. The host uses this to bind the capability
/// allowlist to the loaded module so a plugin cannot spoof
/// another plugin's caps in cross-plugin messages.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(String);

impl PluginId {
    /// Construct a plugin id from a string. Empty / whitespace-
    /// only ids are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::InvalidPluginId`] when the id is
    /// empty or whitespace-only.
    pub fn new(id: impl Into<String>) -> Result<Self, HostError> {
        let id: String = id.into();
        if id.trim().is_empty() {
            return Err(HostError::InvalidPluginId);
        }
        Ok(Self(id))
    }

    /// String view of the id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Compiled capability set bound to one plugin instance.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CapabilitySet {
    granted: BTreeSet<Capability>,
}

impl CapabilitySet {
    /// Empty set (no capabilities; every host call denies).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a set from an iterator of capabilities.
    #[must_use]
    pub fn from_capabilities<I: IntoIterator<Item = Capability>>(caps: I) -> Self {
        Self {
            granted: caps.into_iter().collect(),
        }
    }

    /// Grant `cap`.
    pub fn grant(&mut self, cap: Capability) {
        self.granted.insert(cap);
    }

    /// Revoke `cap`. No-op if not granted.
    pub fn revoke(&mut self, cap: Capability) {
        self.granted.remove(&cap);
    }

    /// `true` when `cap` has been granted.
    #[must_use]
    pub fn allows(&self, cap: Capability) -> bool {
        self.granted.contains(&cap)
    }

    /// Sorted list of granted capabilities (for audit / diagnostics).
    #[must_use]
    pub fn granted(&self) -> Vec<Capability> {
        self.granted.iter().copied().collect()
    }
}

/// One message the plugin sends to the host. Every variant maps
/// to a single [`Capability`] check enforced by `HostBus::authorise`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HostRequest {
    /// "Tell me my account email and quota."
    AccountInfo,
    /// "List the immediate children of folder N."
    ListFolder {
        /// Remote folder id.
        folder_id: u64,
    },
    /// "Stat file N." Returns metadata; never body bytes.
    StatFile {
        /// Remote file id.
        file_id: u64,
    },
    /// "Append this event to the daemon audit log."
    AuditLog {
        /// Free-form category label (e.g. `"plugin.scan.summary"`).
        category: String,
        /// Free-form payload string.
        payload: String,
    },
}

impl HostRequest {
    /// Capability required to dispatch this request.
    #[must_use]
    pub fn required_capability(&self) -> Capability {
        match self {
            Self::AccountInfo => Capability::ReadAccountInfo,
            Self::ListFolder { .. } => Capability::ReadFolderListing,
            Self::StatFile { .. } => Capability::ReadFileMetadata,
            Self::AuditLog { .. } => Capability::EnqueueLocalLog,
        }
    }
}

/// One message the host returns to the plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HostResponse {
    /// Reply to `AccountInfo`.
    AccountInfo {
        /// User id (pCloud numeric).
        user_id: u64,
        /// Account email.
        email: String,
    },
    /// Reply to `ListFolder`.
    FolderListing {
        /// One-line per child entry; structure is
        /// `{name, file_id_or_folder_id, is_folder}`. Renderers
        /// know the shape; the host serialises it as JSON.
        entries_json: String,
    },
    /// Reply to `StatFile`.
    FileMetadata {
        /// Size in bytes.
        size: u64,
        /// Modification timestamp (unix seconds).
        modified: u64,
    },
    /// Reply to `AuditLog`. Always `Ok` when granted; the daemon
    /// logs the event without echoing the payload back.
    AuditAck,
}

/// Errors raised by the plugin host.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum HostError {
    /// Plugin id was empty / whitespace-only.
    #[error("plugin id must be non-empty")]
    InvalidPluginId,
    /// Plugin attempted an action requiring a capability it was
    /// not granted. The host echoes the capability so operators
    /// can see what to add to the allowlist (or which plugin to
    /// audit for over-reach).
    #[error("plugin {plugin} denied: missing capability {capability:?}")]
    CapabilityDenied {
        /// Plugin id that attempted the action.
        plugin: String,
        /// Capability that would have been required.
        capability: Capability,
    },
    /// Backend (wasmtime / no-op) refused to load the module.
    #[error("plugin backend rejected module: {0}")]
    LoadFailed(String),
}

/// Trait the host uses to drive a concrete plugin runtime.
/// `NoopBackend` is the default implementation (proves the call
/// shape); a wasmtime-backed implementation is the follow-up.
pub trait PluginBackend {
    /// Load a wasm module / native plugin and return a handle.
    /// Returns [`HostError::LoadFailed`] if the bytes are
    /// invalid / the backend cannot run them.
    fn load(&mut self, plugin_id: &PluginId, module_bytes: &[u8]) -> Result<(), HostError>;

    /// Hand the plugin one host response. Plugins consume these
    /// asynchronously; the host calls this for each pending
    /// reply.
    fn deliver(&mut self, plugin_id: &PluginId, response: &HostResponse) -> Result<(), HostError>;
}

/// No-op backend: accepts any module, accepts any delivery.
/// Used by tests and as the default until wasmtime lands.
#[derive(Debug, Default)]
pub struct NoopBackend;

impl PluginBackend for NoopBackend {
    fn load(&mut self, _plugin_id: &PluginId, _module_bytes: &[u8]) -> Result<(), HostError> {
        Ok(())
    }
    fn deliver(
        &mut self,
        _plugin_id: &PluginId,
        _response: &HostResponse,
    ) -> Result<(), HostError> {
        Ok(())
    }
}

/// Host-side message bus. Owns the capability-set table and
/// dispatches plugin requests through the configured backend.
#[derive(Debug, Default)]
pub struct HostBus {
    capabilities: std::collections::BTreeMap<String, CapabilitySet>,
}

impl HostBus {
    /// Empty bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin with its capability set. Replaces any
    /// existing entry for the same id.
    pub fn register(&mut self, plugin_id: PluginId, caps: CapabilitySet) {
        self.capabilities
            .insert(plugin_id.as_str().to_owned(), caps);
    }

    /// Drop a plugin's capability set.
    pub fn deregister(&mut self, plugin_id: &PluginId) {
        self.capabilities.remove(plugin_id.as_str());
    }

    /// Return the capability set for `plugin_id`. Empty set when
    /// the plugin is not registered.
    #[must_use]
    pub fn capabilities_of(&self, plugin_id: &PluginId) -> CapabilitySet {
        self.capabilities
            .get(plugin_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// Authorise a request against `plugin_id`'s capability set.
    /// The host should call this before performing any action on
    /// behalf of the plugin.
    ///
    /// # Errors
    ///
    /// [`HostError::CapabilityDenied`] when the request requires
    /// a capability the plugin was not granted (or when the
    /// plugin is not registered at all).
    pub fn authorise(&self, plugin_id: &PluginId, request: &HostRequest) -> Result<(), HostError> {
        let caps = self.capabilities_of(plugin_id);
        let required = request.required_capability();
        if !caps.allows(required) {
            return Err(HostError::CapabilityDenied {
                plugin: plugin_id.as_str().to_owned(),
                capability: required,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(s: &str) -> PluginId {
        PluginId::new(s).unwrap()
    }

    #[test]
    fn plugin_id_rejects_empty() {
        assert_eq!(PluginId::new("").unwrap_err(), HostError::InvalidPluginId);
        assert_eq!(
            PluginId::new("   ").unwrap_err(),
            HostError::InvalidPluginId
        );
    }

    #[test]
    fn capability_set_grant_revoke() {
        let mut caps = CapabilitySet::new();
        assert!(!caps.allows(Capability::ReadAccountInfo));
        caps.grant(Capability::ReadAccountInfo);
        assert!(caps.allows(Capability::ReadAccountInfo));
        caps.revoke(Capability::ReadAccountInfo);
        assert!(!caps.allows(Capability::ReadAccountInfo));
    }

    #[test]
    fn host_request_maps_to_required_capability() {
        let r = HostRequest::AccountInfo;
        assert_eq!(r.required_capability(), Capability::ReadAccountInfo);
        let r = HostRequest::ListFolder { folder_id: 7 };
        assert_eq!(r.required_capability(), Capability::ReadFolderListing);
        let r = HostRequest::StatFile { file_id: 7 };
        assert_eq!(r.required_capability(), Capability::ReadFileMetadata);
        let r = HostRequest::AuditLog {
            category: "x".into(),
            payload: "y".into(),
        };
        assert_eq!(r.required_capability(), Capability::EnqueueLocalLog);
    }

    #[test]
    fn unregistered_plugin_is_denied() {
        let bus = HostBus::new();
        let err = bus
            .authorise(&pid("ghost"), &HostRequest::AccountInfo)
            .unwrap_err();
        match err {
            HostError::CapabilityDenied { plugin, capability } => {
                assert_eq!(plugin, "ghost");
                assert_eq!(capability, Capability::ReadAccountInfo);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn registered_plugin_with_cap_passes_authorise() {
        let mut bus = HostBus::new();
        bus.register(
            pid("scan"),
            CapabilitySet::from_capabilities([Capability::ReadFolderListing]),
        );
        bus.authorise(&pid("scan"), &HostRequest::ListFolder { folder_id: 1 })
            .unwrap();
    }

    #[test]
    fn registered_plugin_without_cap_is_denied() {
        let mut bus = HostBus::new();
        bus.register(
            pid("scan"),
            CapabilitySet::from_capabilities([Capability::ReadFolderListing]),
        );
        // Plugin has folder-listing but no file-metadata cap.
        let err = bus
            .authorise(&pid("scan"), &HostRequest::StatFile { file_id: 1 })
            .unwrap_err();
        match err {
            HostError::CapabilityDenied { capability, .. } => {
                assert_eq!(capability, Capability::ReadFileMetadata);
            }
            other => panic!("{other:?}"),
        }
    }

    /// Acceptance pivot: a plugin without the audit-log capability
    /// cannot enqueue an event.
    #[test]
    fn audit_log_denied_without_capability() {
        let mut bus = HostBus::new();
        bus.register(
            pid("readonly-scanner"),
            CapabilitySet::from_capabilities([Capability::ReadFolderListing]),
        );
        let req = HostRequest::AuditLog {
            category: "x".into(),
            payload: "y".into(),
        };
        let err = bus.authorise(&pid("readonly-scanner"), &req).unwrap_err();
        assert!(matches!(
            err,
            HostError::CapabilityDenied {
                capability: Capability::EnqueueLocalLog,
                ..
            }
        ));
    }

    #[test]
    fn deregister_drops_capabilities() {
        let mut bus = HostBus::new();
        bus.register(
            pid("p"),
            CapabilitySet::from_capabilities([Capability::ReadAccountInfo]),
        );
        bus.deregister(&pid("p"));
        assert!(bus.authorise(&pid("p"), &HostRequest::AccountInfo).is_err());
    }

    #[test]
    fn noop_backend_round_trips() {
        let mut backend = NoopBackend;
        backend.load(&pid("p"), b"\0asm\x01\x00\x00\x00").unwrap();
        backend
            .deliver(
                &pid("p"),
                &HostResponse::AccountInfo {
                    user_id: 1,
                    email: "a@b".into(),
                },
            )
            .unwrap();
    }

    #[test]
    fn capabilities_serde_roundtrip() {
        let caps = CapabilitySet::from_capabilities([
            Capability::ReadAccountInfo,
            Capability::EnqueueLocalLog,
        ]);
        let json = serde_json::to_string(&caps).unwrap();
        let back: CapabilitySet = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, back);
    }

    #[test]
    fn host_request_serde_roundtrip() {
        let r = HostRequest::AuditLog {
            category: "scan.done".into(),
            payload: "{\"count\":42}".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: HostRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}

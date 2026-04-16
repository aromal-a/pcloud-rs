#![forbid(unsafe_code)]
//! # pcloud-p2p
//!
//! LAN peer-acceleration **scaffolding** (Roadmap R9 #4).
//!
//! ## Status: scaffolded, not wired
//!
//! This crate currently provides **only typed configuration shells** for
//! a future peer-to-peer LAN acceleration path:
//!
//! * [`policy::P2pPolicy`] — a single on/off switch.
//! * [`discovery::PeerDiscovery`] — a maximum-peers tuning knob.
//! * [`transfer::PeerTransfer`] — a maximum concurrent-streams knob.
//! * [`P2pShell`] — a composition of the three, with a
//!   [`P2pShell::summary`] helper.
//!
//! **There is deliberately no networking code here.** No discovery
//! protocol, no peer inventory, no peer-to-peer transfer engine, and no
//! integration with `pcloud-daemon`. The shells are serde-stable
//! placeholders so that configuration files can reserve the namespace
//! without committing the daemon to LAN acceleration.
//!
//! The `pcloud-daemon` runtime does **not** import this crate as a
//! runtime dependency: enabling `policy.enabled = true` today has no
//! observable effect beyond the string emitted by [`P2pShell::summary`].
//! Any release note, CLI flag, or documentation that claims LAN
//! acceleration is active on the basis of this crate is wrong.
//!
//! Do not claim LAN peer acceleration parity on the basis of this crate.
//! When a real implementation lands it will replace these shells (or
//! sit behind them) and the crate-level docs will be updated to
//! document the active runtime.
//!
//! ## Planned architecture (non-binding design sketch)
//!
//! The eventual LAN-acceleration implementation is expected to have
//! three layers, each living in its own module so the scaffolded shells
//! can grow into real types without breaking the `P2pShell` composition:
//!
//! 1. **Peer discovery ([`discovery`])** — multicast DNS (mDNS /
//!    DNS-SD) advertisement and browsing on the local link. Each daemon
//!    publishes a service record tagged with its account-scoped peer
//!    id and a freshness nonce; browsers collect responders into the
//!    bounded peer inventory capped by [`discovery::PeerDiscovery`].
//!    mDNS was chosen because it needs no infrastructure (no router
//!    cooperation, no central rendezvous), degrades cleanly on
//!    networks that block multicast, and can be disabled in one
//!    switch via [`policy::P2pPolicy`].
//! 2. **Content-hash mediation (daemon-mediated)** — before any peer
//!    transfer the initiator asks the pCloud API to vouch for a
//!    `(content_hash, remote_file_id)` pair. Peers only accept inbound
//!    requests whose content-hash has been signed by the server in the
//!    last short window. This prevents a malicious LAN neighbour from
//!    serving poisoned bytes in place of a legitimate object: the
//!    client verifies the received stream against the server-signed
//!    hash before committing to local storage.
//! 3. **Peer transport ([`transfer`])** — small-MTU UDP with hole-
//!    punching so two clients behind the same NAT (or even on the
//!    same subnet with a stateful firewall) can establish a direct
//!    session. Concurrency is bounded by
//!    [`transfer::PeerTransfer::max_parallel_streams`]. TLS-PSK keyed
//!    by the server-mediated hash provides integrity and
//!    confidentiality on the LAN path. TCP fallback is expected for
//!    environments where UDP is filtered.
//!
//! Everything above is **plan, not code**. Treat this section as a
//! contract the rewrite intends to honour, not a description of
//! behaviour shipping today.
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

/// Peer discovery primitives (LAN scan, peer inventory).
pub mod discovery;
/// P2P on/off policy and gating rules.
pub mod policy;
/// Peer-to-peer transfer tuning surface.
pub mod transfer;

pub use discovery::{DiscoveryRuntime, InstanceId, P2pError, PeerInfo};

/// Crate identifier used in logs and telemetry.
pub const CRATE_NAME: &str = "pcloud-p2p";

/// mDNS service type advertised and browsed by the discovery runtime.
pub const SERVICE_TYPE: &str = "_pcloud-rs._tcp.local.";

/// Composition of the three P2P sub-shells (policy / discovery / transfer).
///
/// # Honest scope (2026-04-15)
///
/// This shell now owns a real mDNS discovery runtime (see
/// [`DiscoveryRuntime`]) gated by [`policy::P2pPolicy::enabled`]. **That
/// is all it does.** There is no UDP hole-punch, no content-hash
/// mediation, no peer transport, and no actual peer-to-peer transfer.
/// Those require real network-conformance testing and are tracked under
/// `bd-1du.10` / R9 #4. Do not claim LAN peer transfer parity on the
/// basis of the discovery runtime alone.
#[derive(Debug, Default)]
pub struct P2pShell {
    /// P2P on/off policy.
    pub policy: policy::P2pPolicy,
    /// Peer discovery configuration.
    pub discovery: discovery::PeerDiscovery,
    /// Transfer tuning configuration.
    pub transfer: transfer::PeerTransfer,
    /// Active mDNS discovery runtime when started, `None` otherwise.
    runtime: Option<DiscoveryRuntime>,
}

impl P2pShell {
    /// Construct a disabled-by-default shell with no active runtime.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Render a single-line human-readable summary of the shell state.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "p2p(enabled={}, running={}, max_peers={}, streams={})",
            self.policy.enabled,
            self.runtime.is_some(),
            self.discovery.max_peers,
            self.transfer.max_parallel_streams
        )
    }

    /// Start the mDNS discovery runtime: spawn a responder advertising
    /// [`SERVICE_TYPE`] with TXT keys `instance=<uuid>` and
    /// `user=<sha256-of-uid+host>`, plus a browser that collects peers.
    ///
    /// # Honest scope
    ///
    /// Discovery only. No UDP hole-punch, no transfer, no vouching.
    /// Tracked under `bd-1du.10` / R9 #4.
    ///
    /// # Errors
    ///
    /// Returns a [`P2pError`] if the mDNS daemon cannot be started,
    /// the service cannot be registered, or the browser cannot be
    /// attached. Callers should treat start failures as non-fatal —
    /// the daemon continues without LAN discovery.
    pub fn start(&mut self, user_hint: &str, host_hint: &str) -> Result<(), P2pError> {
        if self.runtime.is_some() {
            return Ok(());
        }
        let rt = DiscoveryRuntime::start(user_hint, host_hint, self.discovery.max_peers)?;
        self.runtime = Some(rt);
        Ok(())
    }

    /// Stop the active mDNS runtime, if any. Idempotent.
    pub fn stop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown();
        }
    }

    /// Whether an mDNS runtime is currently active.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.runtime.is_some()
    }

    /// Return a snapshot of currently-known peers. Empty when the
    /// runtime is not active.
    #[must_use]
    pub fn peers(&self) -> Vec<PeerInfo> {
        match &self.runtime {
            Some(rt) => rt.peers(),
            None => Vec::new(),
        }
    }

    /// Instance id advertised by the active runtime, or `None` when
    /// discovery is not running.
    #[must_use]
    pub fn instance_id(&self) -> Option<InstanceId> {
        self.runtime.as_ref().map(DiscoveryRuntime::instance_id)
    }
}

impl Drop for P2pShell {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{discovery::PeerDiscovery, policy::P2pPolicy, transfer::PeerTransfer};

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(CRATE_NAME, "pcloud-p2p");
    }

    #[test]
    fn default_shell_is_disabled() {
        let shell = P2pShell::default();
        assert!(!shell.policy.enabled);
        assert_eq!(shell.discovery.max_peers, 32);
        assert_eq!(shell.transfer.max_parallel_streams, 2);
    }

    #[test]
    fn summary_reflects_state_happy_path() {
        let shell = P2pShell::default();
        assert_eq!(
            shell.summary(),
            "p2p(enabled=false, running=false, max_peers=32, streams=2)"
        );
    }

    #[test]
    fn summary_reflects_custom_enabled_state() {
        let shell = P2pShell {
            policy: P2pPolicy { enabled: true },
            discovery: PeerDiscovery { max_peers: 8 },
            transfer: PeerTransfer {
                max_parallel_streams: 4,
            },
            runtime: None,
        };
        assert_eq!(
            shell.summary(),
            "p2p(enabled=true, running=false, max_peers=8, streams=4)"
        );
    }

    #[test]
    fn summary_boundary_zero_values() {
        let shell = P2pShell {
            policy: P2pPolicy { enabled: false },
            discovery: PeerDiscovery { max_peers: 0 },
            transfer: PeerTransfer {
                max_parallel_streams: 0,
            },
            runtime: None,
        };
        assert_eq!(
            shell.summary(),
            "p2p(enabled=false, running=false, max_peers=0, streams=0)"
        );
    }

    #[test]
    fn summary_boundary_usize_max() {
        let shell = P2pShell {
            policy: P2pPolicy { enabled: true },
            discovery: PeerDiscovery {
                max_peers: usize::MAX,
            },
            transfer: PeerTransfer {
                max_parallel_streams: usize::MAX,
            },
            runtime: None,
        };
        // Just exercises formatting with boundary values; no panic is the invariant.
        assert!(shell.summary().contains(&usize::MAX.to_string()));
    }

    #[test]
    fn peers_endpoint_returns_empty_when_no_peers() {
        // Discovery runtime not started → peers() must be empty and
        // is_running() must be false. This is the honest "IPC surface
        // works without binding mDNS sockets" contract.
        let shell = P2pShell::default();
        assert!(!shell.is_running());
        assert!(shell.peers().is_empty());
        assert!(shell.instance_id().is_none());
    }

    #[test]
    fn peer_list_serde_roundtrip() {
        // Serde stability for the peer-list wire shape. Ensures the
        // daemon IPC surface and CLI decoder never drift.
        let peers = vec![
            PeerInfo {
                instance_id: InstanceId("a1b2".to_owned()),
                user_tag: "deadbeef".to_owned(),
                addrs: vec!["192.168.1.2:41234".to_owned()],
                hostname: "alpha.local".to_owned(),
                port: 41234,
            },
            PeerInfo {
                instance_id: InstanceId("c3d4".to_owned()),
                user_tag: "cafef00d".to_owned(),
                addrs: vec!["[fe80::1]:41234".to_owned()],
                hostname: "beta.local".to_owned(),
                port: 41234,
            },
        ];
        let j = serde_json::to_string(&peers).unwrap();
        let back: Vec<PeerInfo> = serde_json::from_str(&j).unwrap();
        assert_eq!(peers, back);
    }

    #[test]
    fn policy_serde_roundtrip() {
        let p = P2pPolicy { enabled: true };
        let j = serde_json::to_string(&p).unwrap();
        let back: P2pPolicy = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn discovery_serde_roundtrip() {
        let d = PeerDiscovery { max_peers: 16 };
        let j = serde_json::to_string(&d).unwrap();
        let back: PeerDiscovery = serde_json::from_str(&j).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn transfer_serde_roundtrip() {
        let t = PeerTransfer {
            max_parallel_streams: 7,
        };
        let j = serde_json::to_string(&t).unwrap();
        let back: PeerTransfer = serde_json::from_str(&j).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn discovery_rejects_invalid_json() {
        // Empty object is missing required field.
        let r: Result<PeerDiscovery, _> = serde_json::from_str("{}");
        assert!(r.is_err());
    }

    #[test]
    fn policy_rejects_invalid_json() {
        let r: Result<P2pPolicy, _> = serde_json::from_str("{\"enabled\":\"nope\"}");
        assert!(r.is_err());
    }

    #[test]
    fn policy_default_is_sane() {
        // The P2P kill-switch must default to OFF — opt-in only.
        let p = P2pPolicy::default();
        assert!(!p.enabled);
    }

    #[test]
    fn discovery_runtime_constructs() {
        // Smoke test: constructing a runtime must not touch the network
        // beyond what mdns-sd does internally on start. We immediately
        // shut it down so no sockets leak across the test boundary.
        let rt = DiscoveryRuntime::start("user-hint", "host-hint", 4)
            .expect("discovery runtime should construct");
        assert_eq!(rt.instance_id().0, "user-hint@host-hint");
        assert!(rt.peers().is_empty());
        rt.shutdown();
    }

    #[test]
    fn peer_info_serde_roundtrip() {
        // Single PeerInfo round-trip — guards the wire shape used by
        // the daemon IPC and CLI decoders.
        let p = PeerInfo {
            instance_id: InstanceId("abc123".to_owned()),
            user_tag: "deadbeef".to_owned(),
            addrs: vec!["10.0.0.1:41234".to_owned()],
            hostname: "node.local".to_owned(),
            port: 41234,
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: PeerInfo = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }
}

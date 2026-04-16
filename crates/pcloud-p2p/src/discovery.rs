// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Peer discovery configuration (maximum peers to track at once).
///
/// The planned discovery transport is mDNS / DNS-SD on the local link:
/// each daemon publishes an account-scoped service record and browses
/// for peers publishing the same account id. The `max_peers` cap exists
/// to bound memory usage on noisy networks (e.g. a large corporate
/// subnet) — once the inventory is full new responders are discarded
/// rather than evicting an already-verified peer.
///
/// In the current scaffolded state nothing is advertised, nothing is
/// browsed, and the cap is never enforced. The struct exists so that
/// operators can reserve the tuning knob in configuration today without
/// a migration later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDiscovery {
    /// Hard cap on the number of simultaneously-tracked peers.
    ///
    /// Intended to be consulted by the future mDNS browser when
    /// admitting a newly-seen peer into the inventory. A value of `0`
    /// disables peer admission entirely (equivalent to a narrower
    /// kill-switch than [`super::policy::P2pPolicy::enabled`]).
    pub max_peers: usize,
}

impl Default for PeerDiscovery {
    fn default() -> Self {
        Self { max_peers: 32 }
    }
}

// ------------------------------------------------------------------
// Scaffolding types referenced by `lib.rs` until the real mDNS
// discovery runtime lands (tracked under `bd-1du.10` / R9 #4). These
// are deliberately inert: no networking, no threads, no sockets.
// ------------------------------------------------------------------

/// Opaque instance identifier advertised on the LAN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceId(pub String);

/// Information about a discovered peer. Scaffolding shape only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Peer's advertised instance id.
    pub instance_id: InstanceId,
    /// Account-scoped user tag the peer advertises.
    pub user_tag: String,
    /// Socket addresses the peer is reachable on.
    pub addrs: Vec<String>,
    /// Hostname the peer advertises.
    pub hostname: String,
    /// Port the peer listens on.
    pub port: u16,
}

/// Error surface for the (future) discovery runtime. Empty today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum P2pError {
    /// Placeholder until real errors exist.
    #[error("p2p discovery unimplemented: {0}")]
    Unimplemented(&'static str),
}

/// Inert handle that pretends to own an mDNS responder. Never
/// advertises or browses — see the crate-level docs.
#[derive(Debug)]
pub struct DiscoveryRuntime {
    instance_id: InstanceId,
    #[allow(dead_code)]
    max_peers: usize,
}

impl DiscoveryRuntime {
    /// Start a no-op runtime. Kept so `P2pShell::start` compiles; real
    /// discovery will land under `bd-1du.10` / R9 #4.
    ///
    /// # Errors
    /// Never fails today; kept `Result`-shaped for forward compatibility.
    pub fn start(user_hint: &str, host_hint: &str, max_peers: usize) -> Result<Self, P2pError> {
        Ok(Self {
            instance_id: InstanceId(format!("{user_hint}@{host_hint}")),
            max_peers,
        })
    }

    /// Shutdown hook. No-op today.
    pub fn shutdown(self) {}

    /// Snapshot of known peers. Always empty on the scaffold.
    #[must_use]
    pub fn peers(&self) -> Vec<PeerInfo> {
        Vec::new()
    }

    /// Advertised instance id for this runtime.
    #[must_use]
    pub fn instance_id(&self) -> InstanceId {
        self.instance_id.clone()
    }
}

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Peer-to-peer transfer tuning knobs.
///
/// The planned transport is UDP with hole-punching, keyed per session
/// by a server-vouched content hash (see the crate-level "Planned
/// architecture" section). Received bytes are verified against the
/// server-signed hash before commit, so a misbehaving peer cannot
/// poison the local cache even if it wins a race to respond.
///
/// The `max_parallel_streams` cap bounds the damage a runaway peer set
/// can do to local bandwidth and file-descriptor budgets. TCP fallback
/// is expected to share the same cap.
///
/// In the current scaffolded state no streams are opened and the cap is
/// never enforced; the field is preserved so configuration can reserve
/// the knob today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerTransfer {
    /// Maximum concurrent in-flight transfer streams with LAN peers.
    ///
    /// Applies to the aggregate of inbound and outbound peer streams.
    /// A value of `0` will disable peer transfers even if
    /// [`super::policy::P2pPolicy::enabled`] is `true`, which is the
    /// intended emergency throttle for operators.
    pub max_parallel_streams: usize,
}

impl Default for PeerTransfer {
    fn default() -> Self {
        Self {
            max_parallel_streams: 2,
        }
    }
}

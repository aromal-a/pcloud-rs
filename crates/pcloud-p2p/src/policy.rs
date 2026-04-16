// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Global on/off policy for the P2P subsystem.
///
/// This is the single kill-switch for every planned LAN-acceleration
/// behavior (mDNS announcements, peer discovery, UDP hole-punch, direct
/// transfers). When `enabled = false` the subsystem is expected to
/// perform **no** outbound multicast, **no** inbound listener bind, and
/// **no** peer state-keeping.
///
/// In the current scaffolded state the flag is observed only by the
/// `summary` helpers — flipping it does not activate any runtime
/// behavior because none is wired. The field is preserved so that
/// configuration files can reserve the namespace and so the eventual
/// implementation can ship without a config migration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct P2pPolicy {
    /// When `false`, the P2P shell is inert (no discovery, no peer transfers).
    pub enabled: bool,
}

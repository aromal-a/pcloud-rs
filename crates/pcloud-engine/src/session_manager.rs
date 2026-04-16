// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Per-sync-root session state. Tracks session lifetime and cached
/// tokens so that the engine can refresh credentials before they
/// expire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionManagerActor {
    /// Proactive refresh margin, in seconds, before credential
    /// expiry. Values below this threshold trigger an eager refresh.
    pub refresh_margin_secs: u64,
}

impl Default for SessionManagerActor {
    fn default() -> Self {
        Self {
            refresh_margin_secs: 300,
        }
    }
}

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Coarse-grained client health classification.
///
/// Aggregated by the daemon from the health of individual subsystems
/// (auth, sync, crypto, transport). Intended for top-level dashboards
/// and the `/health` IPC endpoint.
///
/// # Example
///
/// ```
/// use pcloud_model::health::OverallHealth;
///
/// let h = OverallHealth::Healthy;
/// let j = serde_json::to_string(&h).unwrap();
/// let back: OverallHealth = serde_json::from_str(&j).unwrap();
/// assert_eq!(h, back);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverallHealth {
    /// All subsystems are green.
    Healthy,
    /// Some subsystems are reporting soft failures but core sync still
    /// makes forward progress.
    Degraded,
    /// The client is not making forward progress (e.g. no network,
    /// auth failed, store locked).
    Unavailable,
}

//! Power-source awareness for the sync engine (audit-06 M-4.1).
//!
//! ## Purpose
//!
//! Provides a small, dependency-free `PowerSource` trait that the sync
//! loop consumer (currently `pcloud_daemon::sync_loop`) can poll to skip
//! a cycle while the host is running on battery. This addresses audit
//! finding M-4.1 — the sync engine had `pause_sync_root`/`resume_sync_root`
//! manual hooks but no automatic battery-aware pause.
//!
//! ## Security and behavioural posture
//!
//! - **Opt-in.** This module is pure plumbing. Nothing here pauses the
//!   engine on its own. The scheduler / sync-loop consumer reads the
//!   `[sync_loop].pause_on_battery` config field (default `false`) and
//!   only consults a `PowerSource` when that flag is set, so existing
//!   deployments are unaffected.
//! - **No network or privileged I/O.** The platform reader only opens
//!   files under `/sys/class/power_supply/*` (Linux) or returns
//!   `PowerState::Unknown` (other platforms). Servers, VMs, and
//!   containers without a battery facade are treated as
//!   `Unknown` → "do not pause" so a missing battery never freezes a
//!   headless deployment.
//! - **Cancellation-safe.** Reads are synchronous filesystem polls
//!   bounded to a few small files; there is no async state to cancel.
//!
//! ## Cross-platform note
//!
//! The integrity-sweeper service in `pcloud_daemon` already wires the
//! `battery` (a.k.a. `starship-battery`) crate to read macOS / Windows
//! battery state. To keep `pcloud-engine` dependency-light (the engine
//! currently has zero platform-specific deps) this module does not pull
//! that crate in. macOS and Windows return `Unknown` here; if a richer
//! reading is required on those platforms the daemon-side wiring can
//! inject a custom `PowerSource` implementation that delegates to the
//! `battery` crate already present in the daemon dependency tree.

// **PLATFORM:** all
// **GATING:** none (portable; Linux uses sysfs, others return Unknown).

use std::sync::atomic::{AtomicBool, Ordering};

/// Observed power-state for the host.
///
/// Returned by [`PowerSource::read`]. The sync-loop consumer treats
/// `OnAc` and `Unknown` as "do not pause" so an absent battery facade
/// never blocks sync; `OnBattery` is the only state that causes a
/// configured pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    /// At least one power supply reports `Charging` or `Full`. Sync
    /// proceeds normally.
    OnAc,
    /// At least one power supply reports `Discharging`. Sync-loop
    /// consumers configured with `pause_on_battery = true` skip the
    /// cycle.
    OnBattery,
    /// No supply, no facade, or no consensus. Treated as `OnAc` for
    /// pause-decision purposes — a missing facade must never freeze a
    /// headless deployment.
    Unknown,
}

/// Trait abstracting the host's "am I on battery?" reader.
///
/// Production callers obtain the platform default via
/// [`default_power_source`]. Tests inject a stub that returns whichever
/// state they need.
pub trait PowerSource: Send + Sync {
    /// Read the current power state. Implementations should be cheap
    /// (no network, no privileged I/O) since the sync loop calls this
    /// once per cycle (every few seconds at default settings).
    fn read(&self) -> PowerState;
}

/// Build the platform-default reader.
///
/// - Linux: scans `/sys/class/power_supply/*/status` (no extra deps).
/// - macOS / Windows / BSD: returns `Unknown` (treated as "do not
///   pause"). The daemon-side integrity-sweeper service wires the
///   `battery` crate for those platforms; a richer `PowerSource` impl
///   can delegate to that service if and when sync-engine battery
///   pausing is enabled outside Linux.
#[must_use]
pub fn default_power_source() -> Box<dyn PowerSource> {
    Box::new(PlatformPowerSource::new())
}

/// Decide whether the sync loop should skip its next cycle.
///
/// Pure helper that lets the sync-loop consumer keep the gating logic
/// in one place. Returns `true` only when both `pause_on_battery` is
/// enabled in config and the source reports `OnBattery`.
#[must_use]
pub fn should_pause(power: &dyn PowerSource, pause_on_battery: bool) -> bool {
    if !pause_on_battery {
        return false;
    }
    matches!(power.read(), PowerState::OnBattery)
}

/// Default platform power-source reader. On Linux, scans sysfs; on
/// other platforms, returns `Unknown`.
pub struct PlatformPowerSource {
    unknown_logged: AtomicBool,
}

impl PlatformPowerSource {
    /// Construct a fresh reader. The `unknown_logged` latch ensures the
    /// "no battery facade" warning fires at most once per process.
    #[must_use]
    pub fn new() -> Self {
        Self {
            unknown_logged: AtomicBool::new(false),
        }
    }
}

impl Default for PlatformPowerSource {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerSource for PlatformPowerSource {
    fn read(&self) -> PowerState {
        #[cfg(target_os = "linux")]
        {
            read_linux_sysfs(&self.unknown_logged)
        }
        #[cfg(not(target_os = "linux"))]
        {
            log_unknown_once(&self.unknown_logged, "no battery facade for this platform");
            PowerState::Unknown
        }
    }
}

#[cfg(target_os = "linux")]
fn read_linux_sysfs(unknown_logged: &AtomicBool) -> PowerState {
    use std::fs;
    let root = std::path::Path::new("/sys/class/power_supply");
    let entries = match fs::read_dir(root) {
        Ok(it) => it,
        Err(_) => {
            log_unknown_once(unknown_logged, "/sys/class/power_supply unavailable");
            return PowerState::Unknown;
        }
    };
    let mut saw_ac = false;
    let mut saw_battery = false;
    for entry in entries.flatten() {
        let status_path = entry.path().join("status");
        let Ok(raw) = fs::read_to_string(&status_path) else {
            continue;
        };
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("Discharging") {
            saw_battery = true;
        } else if trimmed.eq_ignore_ascii_case("Charging") || trimmed.eq_ignore_ascii_case("Full") {
            saw_ac = true;
        }
    }
    if saw_battery && !saw_ac {
        PowerState::OnBattery
    } else if saw_ac {
        PowerState::OnAc
    } else {
        PowerState::Unknown
    }
}

fn log_unknown_once(latch: &AtomicBool, _msg: &str) {
    if latch
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        log::warn!(
            "pcloud-engine power: {}; pause_on_battery will be a no-op on this host",
            _msg
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub power source for tests.
    struct StubPower(PowerState);
    impl PowerSource for StubPower {
        fn read(&self) -> PowerState {
            self.0
        }
    }

    #[test]
    fn should_pause_returns_false_when_config_disabled() {
        let p = StubPower(PowerState::OnBattery);
        assert!(!should_pause(&p, false));
    }

    #[test]
    fn should_pause_returns_true_only_when_on_battery() {
        let on_battery = StubPower(PowerState::OnBattery);
        let on_ac = StubPower(PowerState::OnAc);
        let unknown = StubPower(PowerState::Unknown);
        assert!(should_pause(&on_battery, true));
        assert!(!should_pause(&on_ac, true));
        // Unknown is treated as "do not pause" so VMs / containers /
        // servers without a battery facade don't freeze.
        assert!(!should_pause(&unknown, true));
    }

    #[test]
    fn default_power_source_constructs() {
        // Smoke test: just ensure the trait object is dispatchable.
        let p = default_power_source();
        let _ = p.read();
    }

    #[test]
    fn platform_power_source_is_default_constructible() {
        let p = PlatformPowerSource::default();
        let _ = p.read();
    }
}

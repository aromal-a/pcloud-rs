//! Daemon-grade [`PowerSource`] reader.
//!
//! `pcloud-engine::power` ships a dependency-light reader that handles
//! Linux via sysfs and returns [`PowerState::Unknown`] on every other
//! platform (per the cross-platform note at
//! `pcloud-engine::power` line 27-36). This module provides the
//! daemon-side complement: a [`BatteryCratePowerSource`] that uses the
//! `battery` (a.k.a. `starship-battery`) crate already in the daemon's
//! dependency tree (originally for the integrity-sweeper service) to
//! return a real reading on macOS and Windows. Linux falls back to the
//! engine's own sysfs reader.
//!
//! Closes CLAUDEREV iter-1 SYNC-H-04-3 (fire 21, 2026-04-30): the
//! `pause_on_battery` config flag was a silent no-op on macOS and
//! Windows because the engine's `PlatformPowerSource` returned
//! `Unknown` on those platforms. With this reader injected at daemon
//! bootstrap, `pause_on_battery = true` is honoured everywhere the
//! `battery` crate has a backend.
//!
//! # Architecture
//!
//! - **Linux**: delegate to `pcloud_engine::power::PlatformPowerSource`
//!   (sysfs scan; no extra deps).
//! - **macOS / Windows**: delegate to the `battery` crate. Same logic
//!   as `integrity_sweeper_service::read_battery_crate` — returns
//!   `OnBattery` if any battery reports `Discharging`, `OnAc` if all
//!   batteries report not-discharging, `Unknown` if `battery::Manager`
//!   itself fails (no facade installed).
//! - **BSD / DragonFly**: delegate to the engine's `PlatformPowerSource`
//!   which returns `Unknown` (same fall-through as the engine note).
//!
//! # Why not in `pcloud-engine`
//!
//! The engine intentionally has zero platform-specific deps; pulling
//! the `battery` crate in there would push it into the embeddable SDK
//! surface. The daemon already pays that dep cost (integrity-sweeper)
//! so the platform-specific code lives here.
//!
//! # Cancellation-safety
//!
//! All reads are synchronous, bounded, and call neither network nor
//! privileged I/O.

// **PLATFORM:** all
// **GATING:** none (portable; macOS / Windows arm guarded by
// `cfg(any(target_os = "macos", windows))`).

use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "macos", windows))]
use std::sync::atomic::Ordering;

#[cfg(not(any(target_os = "macos", windows)))]
use pcloud_engine::power::PlatformPowerSource;
use pcloud_engine::power::{PowerSource, PowerState};

/// Daemon-grade [`PowerSource`] that uses the `battery` crate on
/// macOS / Windows and falls back to `pcloud-engine`'s sysfs reader
/// on every other platform.
///
/// CLAUDEREV iter-1 SYNC-H-04-3 fix.
pub struct BatteryCratePowerSource {
    /// Linux fall-through (sysfs reader).
    #[cfg(not(any(target_os = "macos", windows)))]
    engine: PlatformPowerSource,
    /// One-shot warning latch for the `battery::Manager` error path on
    /// macOS / Windows. Same shape as the engine's `unknown_logged`
    /// latch; ensures we don't spam the log on a host without a
    /// battery facade installed.
    #[cfg(any(target_os = "macos", windows))]
    unknown_logged: AtomicBool,
    /// Keep the type identical on every platform so the trait object
    /// path is `cfg`-clean. On non-macOS/Windows targets this is dead
    /// but cheap (one `AtomicBool`).
    #[cfg(not(any(target_os = "macos", windows)))]
    _phantom_unknown_logged: AtomicBool,
}

impl BatteryCratePowerSource {
    /// Construct a fresh reader.
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(not(any(target_os = "macos", windows)))]
            engine: PlatformPowerSource::new(),
            #[cfg(any(target_os = "macos", windows))]
            unknown_logged: AtomicBool::new(false),
            #[cfg(not(any(target_os = "macos", windows)))]
            _phantom_unknown_logged: AtomicBool::new(false),
        }
    }
}

impl Default for BatteryCratePowerSource {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerSource for BatteryCratePowerSource {
    fn read(&self) -> PowerState {
        #[cfg(target_os = "linux")]
        {
            // Linux: engine reader (sysfs). Already returns OnAc /
            // OnBattery / Unknown correctly.
            self.engine.read()
        }
        #[cfg(any(target_os = "macos", windows))]
        {
            read_battery_crate(&self.unknown_logged)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            // BSD / DragonFly / etc.: same as engine default — Unknown.
            self.engine.read()
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
fn read_battery_crate(unknown_logged: &AtomicBool) -> PowerState {
    // Mirrors `integrity_sweeper_service::read_battery_crate` (the
    // existing daemon-side battery consumer). Kept private to this
    // module so future telemetry / config surfaces touching the battery
    // crate stay in one place. CLAUDEREV iter-1 SYNC-H-04-3 fix.
    let manager = match battery::Manager::new() {
        Ok(m) => m,
        Err(_) => {
            log_unknown_once(unknown_logged, "battery::Manager::new failed");
            return PowerState::Unknown;
        }
    };
    let iter = match manager.batteries() {
        Ok(i) => i,
        Err(_) => {
            log_unknown_once(unknown_logged, "battery::Manager::batteries failed");
            return PowerState::Unknown;
        }
    };
    let mut saw_any = false;
    let mut saw_discharging = false;
    for b in iter.flatten() {
        saw_any = true;
        if matches!(b.state(), battery::State::Discharging) {
            saw_discharging = true;
        }
    }
    if !saw_any {
        // No battery → desktop / server / VM. Treat as on-AC so a
        // Mac mini doesn't accidentally pause sync forever.
        return PowerState::OnAc;
    }
    if saw_discharging {
        PowerState::OnBattery
    } else {
        PowerState::OnAc
    }
}

#[cfg(any(target_os = "macos", windows))]
fn log_unknown_once(latch: &AtomicBool, detail: &str) {
    if latch
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        log::warn!(
            "pcloud-daemon power: {detail}; pause_on_battery will fall back to Unknown on this host"
        );
    }
}

/// Build the daemon-grade default reader. Use this from
/// `bootstrap.rs` instead of `pcloud_engine::power::default_power_source`
/// so macOS / Windows `pause_on_battery` is no longer silent.
#[must_use]
pub fn default_daemon_power_source() -> Box<dyn PowerSource> {
    Box::new(BatteryCratePowerSource::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_crate_power_source_constructs_and_reads() {
        // Smoke test: trait object is dispatchable on every supported
        // target. The actual returned state depends on the host (sysfs
        // on Linux, battery crate on macOS / Windows, Unknown on BSD).
        let p: Box<dyn PowerSource> = default_daemon_power_source();
        let _ = p.read();
    }

    #[test]
    fn battery_crate_power_source_does_not_panic_under_repeated_reads() {
        // The macOS / Windows path constructs a `battery::Manager`
        // once per `read()` — confirm that's still cheap and panic-free
        // when called many times.
        let p = BatteryCratePowerSource::new();
        for _ in 0..5 {
            let _ = p.read();
        }
    }

    /// Linux-only sanity: BatteryCratePowerSource and the engine's
    /// PlatformPowerSource MUST agree on the current host's reading.
    /// (On macOS / Windows the readings legitimately differ because
    /// the engine returns `Unknown` while the daemon-grade reader
    /// returns the real state — which is the whole point of this
    /// module.)
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_battery_crate_source_matches_engine_default() {
        let daemon = BatteryCratePowerSource::new();
        let engine = PlatformPowerSource::new();
        assert_eq!(daemon.read(), engine.read());
    }
}

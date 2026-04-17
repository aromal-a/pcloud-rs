//! Best-effort metered-network detection using only the standard library.
//!
//! This module inspects well-known Linux paths to make a heuristic guess
//! about whether the default network connection is metered (i.e. capped,
//! mobile tethering, or otherwise billed by byte). It never panics, never
//! fails loudly, and returns `false` when detection is inconclusive.
//!
//! # Detection strategy ([`is_metered_network`])
//!
//! Sources are tried in priority order, short-circuiting on the first
//! positive signal:
//!
//! 1. **NetworkManager hint** — scans
//!    `/run/NetworkManager/dnsmasq/nm-dns-*` files. NetworkManager writes
//!    a drop-in per active connection. If any file contains the literal
//!    token `metered`, the network is reported as metered. This catches
//!    explicit NM metered-connection settings (`connection.metered=1`)
//!    and Wi-Fi networks the user has flagged metered in GNOME / KDE.
//! 2. **Default-route WAN prefix** — parses `/proc/net/route` to locate
//!    the row whose destination is `00000000` (the default gateway), then
//!    confirms that interface is `up` via
//!    `/sys/class/net/<iface>/operstate`. Interfaces whose names start
//!    with `wwan` (cellular), `ppp` (dial-up / tethering), or `rmnet`
//!    (Qualcomm modem) are treated as metered.
//!
//! Both sources are cheap and read-only. Errors are swallowed into
//! `Ok(false)` / `Err(())` and the public entry point returns a plain
//! `bool`.
//!
//! # Recommended limit ([`recommended_limit`])
//!
//! When the network is metered, returns a conservative byte-per-second
//! ceiling (512 KiB/s) suitable for feeding to
//! [`crate::pacing::BandwidthPacer::set_limit`]. Returns `None`
//! otherwise. The invariant `recommended_limit().is_some() == is_metered_network()`
//! is asserted in the crate's test suite.
//!
//! # Platform
//!
//! Linux-specific — see the `TODO(bd-xplat)` marker inside the private
//! `default_route_is_wan` helper. On non-Linux targets detection always
//! returns `false`.

// **PLATFORM:** Linux
// **GATING:** Linux-specific helpers gated with #[cfg(target_os = "linux")].

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::path::Path;

/// Recommended byte-per-second ceiling for metered links (512 KB/s).
const METERED_LIMIT_BPS: u64 = 512 * 1024;

/// Return `true` if the current default network is likely metered.
///
/// Returns `false` when detection is inconclusive or unsupported. This
/// function never panics.
///
/// # Example
///
/// ```
/// // Never panics — returns a deterministic bool for the CI host.
/// let _metered: bool = pcloud_resilience::is_metered_network();
/// ```
pub fn is_metered_network() -> bool {
    if nm_reports_metered().unwrap_or(false) {
        return true;
    }
    if default_route_is_wan().unwrap_or(false) {
        return true;
    }
    false
}

/// If the network is metered, return a recommended bandwidth cap in bytes
/// per second (currently 512 KB/s). Returns [`None`] otherwise.
///
/// # Example
///
/// ```
/// // Invariant: presence of a limit mirrors the metered predicate.
/// assert_eq!(
///     pcloud_resilience::recommended_limit().is_some(),
///     pcloud_resilience::is_metered_network(),
/// );
/// ```
pub fn recommended_limit() -> Option<u64> {
    if is_metered_network() {
        Some(METERED_LIMIT_BPS)
    } else {
        None
    }
}

/// Probe NetworkManager's dnsmasq drop-in directory for a metered hint.
///
/// Returns `Ok(true)` if any `nm-dns-*` file contains the literal token
/// `metered`. Returns `Ok(false)` otherwise, and `Err(())` if the directory
/// cannot be read.
///
/// Linux-only: `/run/NetworkManager` is a Linux-specific path.
#[cfg(target_os = "linux")]
fn nm_reports_metered() -> Result<bool, ()> {
    let dir = Path::new("/run/NetworkManager/dnsmasq");
    let entries = fs::read_dir(dir).map_err(|_| ())?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if !name_s.starts_with("nm-dns-") {
            continue;
        }
        if let Ok(contents) = fs::read_to_string(entry.path())
            && contents.contains("metered")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(target_os = "linux"))]
fn nm_reports_metered() -> Result<bool, ()> {
    Err(())
}

/// Determine whether the default route interface looks like a WAN (cellular
/// / tethered) link based on its name prefix.
///
/// Linux-only: uses `/proc/net/route` and `/sys/class/net/`.
/// TODO(bd-xplat): macOS equivalent uses `SCNetworkReachability` or
/// `NWPathMonitor`; tracked under PLAN_CROSSPLATFORM.md §2.
#[cfg(target_os = "linux")]
fn default_route_is_wan() -> Result<bool, ()> {
    let route = fs::read_to_string("/proc/net/route").map_err(|_| ())?;
    // /proc/net/route format:
    //   Iface  Destination  Gateway  Flags  ...
    // The default route has Destination == "00000000".
    for line in route.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let (Some(iface), Some(dest)) = (fields.next(), fields.next()) else {
            continue;
        };
        if dest != "00000000" {
            continue;
        }
        // Confirm interface is up.
        let state_path = format!("/sys/class/net/{iface}/operstate");
        let up = fs::read_to_string(&state_path)
            .map(|s| s.trim() == "up")
            .unwrap_or(false);
        if !up {
            continue;
        }
        if iface.starts_with("wwan") || iface.starts_with("ppp") || iface.starts_with("rmnet") {
            return Ok(true);
        }
        return Ok(false);
    }
    Ok(false)
}

#[cfg(not(target_os = "linux"))]
fn default_route_is_wan() -> Result<bool, ()> {
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metered_detection_returns_bool_without_panic() {
        // Must not panic on whatever the CI host has.
        let _ = is_metered_network();
        let limit = recommended_limit();
        // Invariant: recommended_limit mirrors the metered predicate.
        assert_eq!(limit.is_some(), is_metered_network());
    }
}

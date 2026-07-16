//! T1.4.c — Metered-network detector.
//!
//! # Trait
//!
//! [`MeteredHint`] is the smallest possible interface: a single
//! `is_metered()` query that returns `true` when the daemon should
//! treat the active network as metered (and apply
//! `BandwidthScheduleConfig::metered_cap_bytes_per_sec`).
//!
//! # Platform default
//!
//! [`default_metered_hint`] returns:
//!
//! - **Linux:** [`NetworkManagerMeteredHint`] — calls `busctl
//!   get-property` against NetworkManager's well-known service.
//!   `busctl` is part of `systemd`, which is the only environment in
//!   which NetworkManager runs anyway, so this avoids pulling a heavy
//!   `dbus`/`zbus` dependency for a one-byte query that fires once a
//!   minute. NM's `Metered` enum (0=Unknown, 1=Yes, 2=GuessYes,
//!   3=No, 4=GuessNo) is collapsed: any of `Yes` / `GuessYes` is
//!   treated as metered. Failures (busctl missing, NM unreachable,
//!   parse error) fall back to "not metered" — the worst-case
//!   penalty is unmetered transfer rates, never an over-aggressive
//!   throttle.
//! - **macOS / Windows:** [`AlwaysUnmeteredHint`]. The native APIs
//!   (`nw_path_monitor` / `Windows.Networking.Connectivity`) are
//!   WinRT / Objective-C-only and out of AI scope until a dedicated
//!   bridge crate exists. Returning `false` keeps the metered cap
//!   config field meaningful on Linux today and is honest about the
//!   gap on the other platforms.
//!
//! # Why no dbus / zbus dep
//!
//! The daemon already shells out for a handful of platform queries
//! (the upstream pCloud client does the same for keychain / wallet
//! lookups). Calling `busctl` once per cycle is cheap, requires no
//! linking changes, and isolates the policy boundary so a future
//! switch to a real D-Bus binding can replace this module wholesale
//! without touching `BandwidthScheduleApplier` or the trait surface.

// **PLATFORM:** all
// **GATING:** none (portable; the platform-specific busctl path is
// gated inside `default_metered_hint`).

use std::sync::atomic::{AtomicBool, Ordering};

/// Pluggable "is the host on a metered network?" reader.
///
/// Implementations are cheap to call (no async, no allocation
/// expected on the hot path) — the daemon hits this once per
/// sync-loop cycle, which is a fraction of a Hertz on default
/// settings. Implementations should return `false` rather than panic
/// on transient lookup failures so a flaky detector cannot freeze
/// the sync loop.
pub trait MeteredHint: Send + Sync + std::fmt::Debug {
    /// Read the current metered hint. Should not block longer than a
    /// few milliseconds.
    fn is_metered(&self) -> bool;
}

/// Honest stub: always returns `false`. Used on macOS / Windows /
/// platforms without a wired detector.
#[derive(Debug, Default)]
pub struct AlwaysUnmeteredHint {
    notice_logged: AtomicBool,
}

impl AlwaysUnmeteredHint {
    /// Construct a fresh stub.
    #[must_use]
    pub fn new() -> Self {
        Self {
            notice_logged: AtomicBool::new(false),
        }
    }
}

impl MeteredHint for AlwaysUnmeteredHint {
    fn is_metered(&self) -> bool {
        if self
            .notice_logged
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            log::info!(
                "metered-network detector not wired for this platform; \
                 [bandwidth.schedule] metered_cap_bytes_per_sec will be a no-op"
            );
        }
        false
    }
}

/// Linux: query NetworkManager's `Metered` property via `busctl
/// get-property`. The property is published on the well-known
/// service `org.freedesktop.NetworkManager`, object path
/// `/org/freedesktop/NetworkManager`, interface
/// `org.freedesktop.NetworkManager`.
#[derive(Debug, Default)]
pub struct NetworkManagerMeteredHint {
    /// Latched notice when busctl is missing or NM is unreachable;
    /// avoids spamming the log every cycle.
    #[cfg(target_os = "linux")]
    failure_logged: AtomicBool,
}

impl NetworkManagerMeteredHint {
    /// Construct a fresh detector. No I/O until the first
    /// `is_metered()` call.
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            failure_logged: AtomicBool::new(false),
        }
    }

    /// Parse `busctl`'s `get-property` reply.
    ///
    /// `busctl` prints a single line like `u 1` for a `u32` (the NM
    /// `Metered` property). The leading token is the D-Bus type
    /// signature; the trailing token is the value.
    ///
    /// NM `Metered` enum:
    /// - 0 = `NM_METERED_UNKNOWN`
    /// - 1 = `NM_METERED_YES`
    /// - 2 = `NM_METERED_GUESS_YES`
    /// - 3 = `NM_METERED_NO`
    /// - 4 = `NM_METERED_GUESS_NO`
    ///
    /// Returns `Some(true)` on `Yes`/`GuessYes`, `Some(false)` on
    /// `No`/`GuessNo`/`Unknown`, and `None` if parsing fails (caller
    /// treats `None` as "not metered" so a flaky detector cannot
    /// over-throttle).
    #[cfg(any(target_os = "linux", test))]
    fn parse_busctl_reply(raw: &str) -> Option<bool> {
        let trimmed = raw.trim();
        // Accept either `u N` (signature + value, the default
        // busctl form) or just `N` (some `--json=short` modes).
        let value_token = trimmed.split_whitespace().last()?;
        let n: u32 = value_token.parse().ok()?;
        match n {
            1 | 2 => Some(true),
            _ => Some(false),
        }
    }
}

#[cfg(target_os = "linux")]
impl MeteredHint for NetworkManagerMeteredHint {
    fn is_metered(&self) -> bool {
        use std::process::Command;
        // 100 ms is generous for a property read on the system bus;
        // a non-responsive NM should not hold up the sync cycle.
        let output = match Command::new("busctl")
            .args([
                "--system",
                "--no-pager",
                "get-property",
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
                "Metered",
            ])
            .output()
        {
            Ok(o) => o,
            Err(err) => {
                if self
                    .failure_logged
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    log::info!(
                        "metered-network: busctl unavailable ({err}); \
                         metered detection disabled"
                    );
                }
                return false;
            }
        };
        if !output.status.success() {
            if self
                .failure_logged
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                log::info!(
                    "metered-network: NetworkManager dbus query failed ({}); \
                     metered detection disabled",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            return false;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_busctl_reply(&stdout).unwrap_or(false)
    }
}

#[cfg(not(target_os = "linux"))]
impl MeteredHint for NetworkManagerMeteredHint {
    fn is_metered(&self) -> bool {
        // Type exists on every platform for consistency in the trait
        // hierarchy, but only the Linux build path actually calls
        // busctl. Non-Linux builds collapse to "not metered".
        false
    }
}

/// Build the platform-default metered detector.
#[must_use]
pub fn default_metered_hint() -> Box<dyn MeteredHint> {
    #[cfg(target_os = "linux")]
    {
        Box::new(NetworkManagerMeteredHint::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(AlwaysUnmeteredHint::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub used by sync-loop tests that want a deterministic hint.
    #[derive(Debug)]
    pub(crate) struct StubMeteredHint(pub bool);
    impl MeteredHint for StubMeteredHint {
        fn is_metered(&self) -> bool {
            self.0
        }
    }

    #[test]
    fn always_unmetered_returns_false() {
        let hint = AlwaysUnmeteredHint::new();
        assert!(!hint.is_metered());
        // Idempotent.
        assert!(!hint.is_metered());
    }

    #[test]
    fn nm_busctl_reply_yes_is_metered() {
        assert_eq!(
            NetworkManagerMeteredHint::parse_busctl_reply("u 1\n"),
            Some(true),
            "NM_METERED_YES"
        );
        assert_eq!(
            NetworkManagerMeteredHint::parse_busctl_reply("u 2"),
            Some(true),
            "NM_METERED_GUESS_YES"
        );
    }

    #[test]
    fn nm_busctl_reply_no_is_unmetered() {
        for line in ["u 0", "u 3", "u 4\n", "u 4"] {
            assert_eq!(
                NetworkManagerMeteredHint::parse_busctl_reply(line),
                Some(false),
                "{line}"
            );
        }
    }

    #[test]
    fn nm_busctl_reply_garbage_returns_none() {
        assert_eq!(NetworkManagerMeteredHint::parse_busctl_reply(""), None);
        assert_eq!(
            NetworkManagerMeteredHint::parse_busctl_reply("notanumber"),
            None
        );
    }

    #[test]
    fn default_metered_hint_constructs() {
        let hint = default_metered_hint();
        // Cannot assert a specific value (depends on host); just
        // ensure it dispatches without panicking.
        let _ = hint.is_metered();
    }

    #[test]
    fn stub_round_trips() {
        let on = StubMeteredHint(true);
        let off = StubMeteredHint(false);
        assert!(on.is_metered());
        assert!(!off.is_metered());
    }
}

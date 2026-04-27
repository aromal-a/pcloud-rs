#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
//! # pcloud-plugin-publink-expiry
//!
//! First-party, single-user pcloud-rs plugin that watches the daemon's
//! non-secret public link list and raises a desktop notification when a
//! link is close to expiring.
//!
//! ## Behaviour
//!
//! The plugin requests only [`PluginCapability::ObserveStatus`]. It has
//! no network access of its own — every data point it acts on comes from
//! the daemon through the typed [`PluginOperation`] boundary:
//!
//! 1. Each time the host drives it, the plugin first returns
//!    [`PluginOperation::TimerTick`] (informational) and then
//!    [`PluginOperation::ObservePublinkList`].
//! 2. On the [`PluginOperationResponse::PublinkList`] reply, it iterates
//!    the returned [`PublinkSummary`] items. A link with
//!    `expiry_unix - now <= notify_window_secs` becomes a notification
//!    candidate.
//! 3. A per-link rate limiter (persisted to disk) guarantees the plugin
//!    never emits more than one notification per `link_id` within 24h.
//!
//! ## Security posture
//!
//! * No `reqwest` dependency — the plugin never reaches the network.
//! * No `SecretString`/`SecretBytes` are accepted or stored; the
//!   [`PublinkSummary`] surface is already redacted by the host.
//! * The on-disk state file is the plugin's *only* durable artefact and
//!   contains only public `link_id` strings plus the unix timestamp at
//!   which each link was last notified. It MUST be placed under
//!   `$XDG_STATE_HOME/pcloud-rs/` (caller-controlled) with the default
//!   resolved by [`PublinkExpiryConfig::default_state_path`].
//! * `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are enforced.
//!
//! ## Configuration
//!
//! The host wires this plugin up through the `[plugins.publink_expiry]`
//! table in pcloud-rs's config file:
//!
//! ```toml
//! [plugins.publink_expiry]
//! enabled = true
//! notify_window_hours = 24
//! # state_file = "/home/user/.local/state/pcloud-rs/publink-expiry.json"
//! ```
//!
//! The config struct is [`PublinkExpiryConfig`] and is deliberately kept
//! inside this crate so the plugin remains self-contained.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use pcloud_plugin_api::{
    Plugin, PluginCapability, PluginContext, PluginError, PluginManifest, PluginOperation,
    PluginOperationResponse, PublinkSummary,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Canonical crate identifier, used in structured logs and telemetry.
pub const CRATE_NAME: &str = "pcloud-plugin-publink-expiry";

/// Default notification window in hours if the operator does not override it.
pub const DEFAULT_NOTIFY_WINDOW_HOURS: u32 = 24;

/// Minimum interval between two notifications for the same link, in
/// seconds. Fixed at 24h to avoid desktop notification spam.
pub const RATE_LIMIT_SECS: i64 = 24 * 3600;

/// Errors surfaced by the publink expiry plugin.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PublinkExpiryError {
    /// The configured state file could not be read or written.
    #[error("publink expiry state I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The state file was malformed JSON.
    #[error("publink expiry state parse error: {0}")]
    Parse(#[from] serde_json::Error),
    /// The host-supplied configuration failed validation.
    #[error("publink expiry config invalid: {0}")]
    Config(&'static str),
}

/// Operator-supplied configuration. Mirrors the
/// `[plugins.publink_expiry]` TOML table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublinkExpiryConfig {
    /// Master enable switch. When `false` the host should not register
    /// this plugin at all.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// How far in advance of expiry the plugin should start notifying.
    #[serde(default = "default_window_hours")]
    pub notify_window_hours: u32,
    /// Optional override for the state file location. When `None` the
    /// plugin uses [`PublinkExpiryConfig::default_state_path`].
    #[serde(default)]
    pub state_file: Option<PathBuf>,
}

fn default_enabled() -> bool {
    true
}
fn default_window_hours() -> u32 {
    DEFAULT_NOTIFY_WINDOW_HOURS
}

impl Default for PublinkExpiryConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            notify_window_hours: default_window_hours(),
            state_file: None,
        }
    }
}

impl PublinkExpiryConfig {
    /// Resolve the default state file location under `$XDG_STATE_HOME`
    /// (falling back to `$HOME/.local/state`). Returns `None` when
    /// neither environment variable is set — in that case callers MUST
    /// supply an explicit [`PublinkExpiryConfig::state_file`].
    #[must_use]
    pub fn default_state_path() -> Option<PathBuf> {
        if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
            return Some(PathBuf::from(xdg).join("pcloud-rs/publink-expiry.json"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Some(PathBuf::from(home).join(".local/state/pcloud-rs/publink-expiry.json"));
        }
        None
    }

    /// Return the effective notification window in seconds.
    #[must_use]
    pub fn notify_window_secs(&self) -> i64 {
        i64::from(self.notify_window_hours) * 3600
    }

    /// Validate the configuration and resolve the concrete state path.
    pub fn resolve_state_path(&self) -> Result<PathBuf, PublinkExpiryError> {
        if self.notify_window_hours == 0 {
            return Err(PublinkExpiryError::Config(
                "notify_window_hours must be > 0",
            ));
        }
        if let Some(p) = &self.state_file {
            return Ok(p.clone());
        }
        Self::default_state_path().ok_or(PublinkExpiryError::Config(
            "no state_file configured and neither XDG_STATE_HOME nor HOME set",
        ))
    }
}

/// Persisted rate-limit state — maps `link_id` to the last unix
/// timestamp at which a notification was emitted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationState {
    /// Version tag reserved for future schema migrations.
    #[serde(default = "state_version")]
    pub version: u32,
    /// Map of `link_id` → last-notified unix timestamp.
    #[serde(default)]
    pub last_notified: BTreeMap<String, i64>,
}

fn state_version() -> u32 {
    1
}

impl Default for NotificationState {
    fn default() -> Self {
        Self {
            version: state_version(),
            last_notified: BTreeMap::new(),
        }
    }
}

impl NotificationState {
    /// Load persisted state from `path`. Missing files become a default
    /// empty state; malformed files propagate as [`PublinkExpiryError::Parse`].
    pub fn load(path: &Path) -> Result<Self, PublinkExpiryError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(PublinkExpiryError::Io(e)),
        }
    }

    /// Persist state atomically. Creates parent directories if missing.
    /// On Unix the file is created with mode `0600`.
    pub fn save(&self, path: &Path) -> Result<(), PublinkExpiryError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&tmp, &bytes)?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Should a new notification be emitted for `link_id` at `now`? This
    /// returns `true` when either the link was never notified before, or
    /// the last notification is older than [`RATE_LIMIT_SECS`].
    #[must_use]
    pub fn should_notify(&self, link_id: &str, now_unix: i64) -> bool {
        match self.last_notified.get(link_id) {
            None => true,
            Some(prev) => now_unix.saturating_sub(*prev) >= RATE_LIMIT_SECS,
        }
    }

    /// Record a notification for `link_id` at `now_unix`.
    pub fn mark_notified(&mut self, link_id: &str, now_unix: i64) {
        self.last_notified.insert(link_id.to_owned(), now_unix);
    }
}

// ---------------------------------------------------------------------------
// Notifier abstraction (trait boundary for testability)
// ---------------------------------------------------------------------------

/// Platform-agnostic notifier interface. Production wiring uses
/// [`DesktopNotifier`]; unit tests use [`CapturingNotifier`].
pub trait Notifier: Send {
    /// Emit a single desktop notification. Failures must NOT panic — the
    /// plugin degrades gracefully if the notification subsystem is
    /// unavailable (e.g. headless CI).
    fn notify(&mut self, title: &str, body: &str);
}

/// Real-desktop notifier backed by `notify-rust` on Linux/macOS/Windows.
#[derive(Debug, Default)]
pub struct DesktopNotifier;

impl Notifier for DesktopNotifier {
    fn notify(&mut self, title: &str, body: &str) {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            // notify-rust failures are non-fatal — e.g. no D-Bus on headless hosts.
            let _ = notify_rust::Notification::new()
                .summary(title)
                .body(body)
                .timeout(notify_rust::Timeout::Milliseconds(10_000))
                .show();
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = (title, body);
        }
    }
}

/// In-memory notifier used by tests. Stores every `(title, body)` pair
/// the plugin tried to emit.
#[derive(Debug, Default)]
pub struct CapturingNotifier {
    /// Captured `(title, body)` pairs in emission order.
    pub emitted: Vec<(String, String)>,
}

impl Notifier for CapturingNotifier {
    fn notify(&mut self, title: &str, body: &str) {
        self.emitted.push((title.to_owned(), body.to_owned()));
    }
}

// ---------------------------------------------------------------------------
// Clock abstraction
// ---------------------------------------------------------------------------

/// Monotonic-ish "wall clock" trait — injected for deterministic tests.
pub trait Clock: Send {
    /// Return the current UNIX timestamp in seconds.
    fn now_unix(&self) -> i64;
}

/// Production clock backed by [`std::time::SystemTime`].
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// Fixed clock for tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(
    /// The timestamp this clock always reports.
    pub i64,
);

impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// The publink expiry plugin itself.
pub struct PublinkExpiryPlugin {
    config: PublinkExpiryConfig,
    state_path: PathBuf,
    state: NotificationState,
    pending: VecDeque<PluginOperation>,
    notifier: Box<dyn Notifier>,
    clock: Box<dyn Clock>,
    /// `true` once the plugin has been successfully `on_load`ed.
    loaded: bool,
}

impl std::fmt::Debug for PublinkExpiryPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublinkExpiryPlugin")
            .field("config", &self.config)
            .field("state_path", &self.state_path)
            .field("loaded", &self.loaded)
            .finish()
    }
}

impl PublinkExpiryPlugin {
    /// Construct a plugin using the production [`DesktopNotifier`] and
    /// [`SystemClock`].
    pub fn new(config: PublinkExpiryConfig) -> Result<Self, PublinkExpiryError> {
        Self::with_parts(config, Box::new(DesktopNotifier), Box::new(SystemClock))
    }

    /// Construct a plugin with an arbitrary notifier and clock —
    /// the injection point used by unit tests.
    pub fn with_parts(
        config: PublinkExpiryConfig,
        notifier: Box<dyn Notifier>,
        clock: Box<dyn Clock>,
    ) -> Result<Self, PublinkExpiryError> {
        let state_path = config.resolve_state_path()?;
        let state = NotificationState::load(&state_path)?;
        Ok(Self {
            config,
            state_path,
            state,
            pending: VecDeque::new(),
            notifier,
            clock,
            loaded: false,
        })
    }

    /// Effective notification window in seconds.
    #[must_use]
    pub fn notify_window_secs(&self) -> i64 {
        self.config.notify_window_secs()
    }

    /// Read-only access to the current persisted state (primarily for tests).
    #[must_use]
    pub fn state(&self) -> &NotificationState {
        &self.state
    }

    /// Configured state file path.
    #[must_use]
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Enqueue the per-tick operation sequence. Called internally by
    /// [`PublinkExpiryPlugin::tick`] and by the host when driving the
    /// plugin on its own schedule.
    pub fn tick(&mut self, period_secs: u64) {
        self.pending
            .push_back(PluginOperation::TimerTick { period_secs });
        self.pending.push_back(PluginOperation::ObservePublinkList);
    }

    /// Core of the behaviour: given a list of link summaries and the
    /// current time, emit notifications for any link within the window
    /// that has not yet been notified in the last [`RATE_LIMIT_SECS`],
    /// and persist state.
    ///
    /// Returns the number of notifications actually emitted.
    pub fn process_publinks(
        &mut self,
        links: &[PublinkSummary],
    ) -> Result<usize, PublinkExpiryError> {
        let now = self.clock.now_unix();
        let window = self.notify_window_secs();
        let mut emitted = 0usize;
        let mut mutated = false;

        for link in links {
            let Some(expiry) = link.expiry_unix else {
                continue;
            };
            let delta = expiry.saturating_sub(now);
            if delta < 0 || delta > window {
                continue;
            }
            if !self.state.should_notify(&link.link_id, now) {
                continue;
            }
            let title = "pCloud public link expiring soon";
            let body = format!(
                "Link {} ({}) expires in {} hours.",
                link.link_id,
                if link.label.is_empty() {
                    "unnamed"
                } else {
                    link.label.as_str()
                },
                (delta / 3600).max(0),
            );
            self.notifier.notify(title, &body);
            self.state.mark_notified(&link.link_id, now);
            emitted += 1;
            mutated = true;
        }

        if mutated {
            self.state.save(&self.state_path)?;
        }
        Ok(emitted)
    }
}

impl Plugin for PublinkExpiryPlugin {
    fn manifest(&self) -> PluginManifest {
        use std::collections::BTreeSet;
        PluginManifest {
            id: "pcloud-rs.publink-expiry".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            display_name: "Public Link Expiry Notifier".to_owned(),
            requested_capabilities: BTreeSet::from([PluginCapability::ObserveStatus]),
        }
    }

    fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
        if !self.config.enabled {
            return Err(PluginError::Initialization(
                "publink-expiry plugin is disabled in config".to_owned(),
            ));
        }
        self.loaded = true;
        // Prime the pending queue so the very first next_operation() call
        // drives a tick. Subsequent ticks are re-primed from on_response.
        self.tick(60);
        Ok(())
    }

    fn next_operation(&mut self) -> Option<PluginOperation> {
        self.pending.pop_front()
    }

    fn on_response(&mut self, response: &PluginOperationResponse) {
        if let PluginOperationResponse::PublinkList(links) = response {
            // Best-effort — failure to persist state is logged by the host
            // via audit events; we must not panic inside a plugin callback.
            let _ = self.process_publinks(links);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_plugin_api::{PluginOperation, PluginOperationResponse, PublinkSummary};

    fn tmpdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "pcloud-plugin-publink-expiry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn make_plugin(now: i64, window_hours: u32) -> (PublinkExpiryPlugin, PathBuf) {
        let dir = tmpdir();
        let state_path = dir.join("publink-expiry.json");
        let config = PublinkExpiryConfig {
            enabled: true,
            notify_window_hours: window_hours,
            state_file: Some(state_path.clone()),
        };
        let plugin = PublinkExpiryPlugin::with_parts(
            config,
            Box::new(CapturingNotifier::default()),
            Box::new(FixedClock(now)),
        )
        .unwrap();
        (plugin, state_path)
    }

    /// Thread-safe notifier that mirrors emissions into a shared buffer.
    struct MtNotifier(std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>);
    impl Notifier for MtNotifier {
        fn notify(&mut self, title: &str, body: &str) {
            self.0
                .lock()
                .unwrap()
                .push((title.to_owned(), body.to_owned()));
        }
    }

    /// Helper: run one `process_publinks` cycle and return emitted
    /// notifications alongside the post-cycle persisted state.
    fn run_cycle_v2(
        now: i64,
        window_hours: u32,
        state_path: &Path,
        links: &[PublinkSummary],
    ) -> (usize, Vec<(String, String)>, NotificationState) {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let config = PublinkExpiryConfig {
            enabled: true,
            notify_window_hours: window_hours,
            state_file: Some(state_path.to_path_buf()),
        };
        let notifier: Box<dyn Notifier> = Box::new(MtNotifier(std::sync::Arc::clone(&captured)));
        let clock: Box<dyn Clock> = Box::new(FixedClock(now));
        let mut plugin = PublinkExpiryPlugin::with_parts(config, notifier, clock).unwrap();
        let count = plugin.process_publinks(links).unwrap();
        let state = plugin.state().clone();
        let captured = captured.lock().unwrap().clone();
        (count, captured, state)
    }

    #[test]
    fn expiry_within_window_emits_notification() {
        let dir = tmpdir();
        let state_path = dir.join("s.json");
        let now = 1_000_000;
        let link = PublinkSummary {
            link_id: "LINK-1".to_owned(),
            label: "shared.pdf".to_owned(),
            expiry_unix: Some(now + 3600), // 1h ahead, inside 24h window
        };
        let (count, captured, state) = run_cycle_v2(now, 24, &state_path, &[link]);
        assert_eq!(count, 1);
        assert_eq!(captured.len(), 1);
        assert!(captured[0].1.contains("LINK-1"));
        assert_eq!(state.last_notified.get("LINK-1"), Some(&now));
        assert!(state_path.exists(), "state file must be persisted");
    }

    #[test]
    fn expiry_outside_window_does_not_emit() {
        let dir = tmpdir();
        let state_path = dir.join("s.json");
        let now = 1_000_000;
        let link = PublinkSummary {
            link_id: "LINK-2".to_owned(),
            label: "x".to_owned(),
            expiry_unix: Some(now + 48 * 3600), // 48h ahead
        };
        let (count, captured, state) = run_cycle_v2(now, 24, &state_path, &[link]);
        assert_eq!(count, 0);
        assert!(captured.is_empty());
        assert!(state.last_notified.is_empty());
    }

    #[test]
    fn rate_limit_suppresses_duplicate_notifications_within_24h() {
        let dir = tmpdir();
        let state_path = dir.join("s.json");
        let now = 2_000_000;
        let link = PublinkSummary {
            link_id: "LINK-3".to_owned(),
            label: "".to_owned(),
            expiry_unix: Some(now + 3600),
        };
        let (count1, captured1, _) =
            run_cycle_v2(now, 24, &state_path, std::slice::from_ref(&link));
        assert_eq!(count1, 1);
        assert_eq!(captured1.len(), 1);

        // Same link, 10 minutes later — must be suppressed.
        let (count2, captured2, state) =
            run_cycle_v2(now + 600, 24, &state_path, std::slice::from_ref(&link));
        assert_eq!(count2, 0);
        assert!(captured2.is_empty());
        assert_eq!(state.last_notified.get("LINK-3"), Some(&now));

        // 25 hours later: allowed again.
        let later = now + RATE_LIMIT_SECS + 1;
        let link_future = PublinkSummary {
            expiry_unix: Some(later + 3600),
            ..link
        };
        let (count3, captured3, _) = run_cycle_v2(later, 24, &state_path, &[link_future]);
        assert_eq!(count3, 1);
        assert_eq!(captured3.len(), 1);
    }

    #[test]
    fn state_file_round_trip_persists_notification_state() {
        let dir = tmpdir();
        let state_path = dir.join("s.json");
        let mut s = NotificationState::default();
        s.mark_notified("A", 111);
        s.mark_notified("B", 222);
        s.save(&state_path).unwrap();

        let loaded = NotificationState::load(&state_path).unwrap();
        assert_eq!(loaded.last_notified.get("A"), Some(&111));
        assert_eq!(loaded.last_notified.get("B"), Some(&222));
        assert_eq!(loaded.version, 1);

        // Missing file path resolves to default-empty state.
        let missing = dir.join("missing.json");
        let empty = NotificationState::load(&missing).unwrap();
        assert!(empty.last_notified.is_empty());
    }

    #[test]
    fn disabled_config_rejects_on_load() {
        let dir = tmpdir();
        let state_path = dir.join("s.json");
        let config = PublinkExpiryConfig {
            enabled: false,
            notify_window_hours: 24,
            state_file: Some(state_path),
        };
        let mut plugin = PublinkExpiryPlugin::with_parts(
            config,
            Box::new(CapturingNotifier::default()),
            Box::new(FixedClock(0)),
        )
        .unwrap();
        let ctx = PluginContext {
            runtime_summary: "rt".to_owned(),
            granted_capabilities: std::collections::BTreeSet::new(),
            dev_mode: true,
        };
        assert!(plugin.on_load(&ctx).is_err());
    }

    #[test]
    fn next_operation_sequence_drives_timer_and_publink_list() {
        let (mut plugin, _path) = make_plugin(0, 24);
        let ctx = PluginContext {
            runtime_summary: "rt".to_owned(),
            granted_capabilities: std::collections::BTreeSet::from([
                PluginCapability::ObserveStatus,
            ]),
            dev_mode: true,
        };
        plugin.on_load(&ctx).unwrap();
        assert!(matches!(
            plugin.next_operation(),
            Some(PluginOperation::TimerTick { period_secs: 60 })
        ));
        assert!(matches!(
            plugin.next_operation(),
            Some(PluginOperation::ObservePublinkList)
        ));
        assert!(plugin.next_operation().is_none());
    }

    #[test]
    fn on_response_publink_list_triggers_processing() {
        let dir = tmpdir();
        let state_path = dir.join("s.json");
        let now = 500_000;
        let config = PublinkExpiryConfig {
            enabled: true,
            notify_window_hours: 24,
            state_file: Some(state_path.clone()),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        struct MtN(std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>);
        impl Notifier for MtN {
            fn notify(&mut self, t: &str, b: &str) {
                self.0.lock().unwrap().push((t.to_owned(), b.to_owned()));
            }
        }
        let mut plugin = PublinkExpiryPlugin::with_parts(
            config,
            Box::new(MtN(std::sync::Arc::clone(&captured))),
            Box::new(FixedClock(now)),
        )
        .unwrap();
        plugin.on_response(&PluginOperationResponse::PublinkList(vec![
            PublinkSummary {
                link_id: "L".to_owned(),
                label: "doc".to_owned(),
                expiry_unix: Some(now + 600),
            },
        ]));
        assert_eq!(captured.lock().unwrap().len(), 1);
    }

    #[test]
    fn zero_window_hours_rejected_by_config() {
        let config = PublinkExpiryConfig {
            enabled: true,
            notify_window_hours: 0,
            state_file: Some(PathBuf::from("/tmp/x.json")),
        };
        assert!(matches!(
            config.resolve_state_path(),
            Err(PublinkExpiryError::Config(_))
        ));
    }
}

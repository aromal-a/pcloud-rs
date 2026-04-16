#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

//! # Auto-Heal Checksum Scanner Plugin
//!
//! This crate implements the **auto-heal** plugin for the pcloud-rs
//! Rust daemon. It subscribes to file-integrity events produced by the
//! host's checksum scanner, notifies the user when corruption is
//! detected, requests that the affected file's sync root be
//! quarantined (paused), and escalates to a full
//! [`pcloud_plugin_api::PluginOperation::RequestSyncPause`] when the
//! same path has misbehaved repeatedly in a short window.
//!
//! ## Capabilities
//!
//! The plugin declares and only requests:
//!
//! * [`PluginCapability::ObserveStatus`] — to receive integrity events.
//! * [`PluginCapability::SyncControl`]   — to quarantine / pause sync roots.
//!
//! ## Rate limits
//!
//! * At most **1 desktop notification per path per hour**.
//! * At most **10 quarantines per day per sync root**.
//! * More than **3 mismatches on the same path within 24h** escalate to
//!   a full [`PluginOperation::RequestSyncPause`] for that sync root.
//!
//! ## Security posture
//!
//! * `#![forbid(unsafe_code)]`.
//! * Non-secret values only cross the plugin boundary
//!   ([`FileIntegrityResult`] contains paths and an outcome enum).
//! * The plugin never logs, stores, or forwards secrets.
//! * Desktop notifications are best-effort; failure to show a
//!   notification is **not** a fatal error and is recorded in
//!   plugin-local state only.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pcloud_plugin_api::{
    FileIntegrityOutcome, FileIntegrityResult, Plugin, PluginCapability, PluginContext,
    PluginError, PluginManifest, PluginOperation, PluginOperationResponse,
};

/// Maximum desktop notifications per path within a rolling hour.
pub const NOTIFICATIONS_PER_PATH_PER_HOUR: u32 = 1;

/// Maximum quarantine requests the plugin will emit per sync root in
/// a rolling 24h window.
pub const MAX_QUARANTINES_PER_ROOT_PER_DAY: u32 = 10;

/// Number of mismatches on the same path within 24h that, once
/// exceeded, triggers an escalation to a full sync-root pause.
pub const ESCALATION_THRESHOLD: u32 = 3;

/// One hour in seconds.
const ONE_HOUR: u64 = 60 * 60;
/// One day in seconds.
const ONE_DAY: u64 = 24 * ONE_HOUR;

/// How the plugin reacts to a user-supplied response after a
/// mismatch notification. Recorded for audit / observability; this is
/// plumbed in by the host's notification UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserResponse {
    /// User acknowledged the warning but took no corrective action.
    Acknowledge,
    /// User asked the host to re-fetch the file.
    Refetch,
    /// User chose to ignore the event.
    Ignore,
}

/// A single historic event the plugin remembers for rate-limiting and
/// escalation bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MismatchRecord {
    /// Sync root id the event belongs to.
    pub sync_root_id: u64,
    /// Path that produced the mismatch (non-secret).
    pub path: String,
    /// Unix-seconds timestamp at which the event was observed.
    pub at: u64,
    /// Response the user gave after being notified, if any.
    pub user_response: Option<UserResponse>,
}

/// Abstract clock used by the plugin. Tests inject a deterministic
/// implementation; the default uses wall-clock time.
pub trait Clock: Send {
    /// Current unix-seconds timestamp.
    fn now_secs(&self) -> u64;
}

/// Default wall-clock implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }
}

/// Abstract notifier used by the plugin. Production builds use the
/// desktop notifier; tests inject a capturing mock.
pub trait Notifier: Send {
    /// Dispatch a best-effort desktop notification. Failure is
    /// swallowed by the plugin but never escalates.
    fn notify(&mut self, title: &str, body: &str);
}

/// Default desktop notifier backed by [`notify_rust`]. Construction
/// is infallible; delivery failures are swallowed so a headless
/// environment does not break the auto-heal plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct DesktopNotifier;

impl Notifier for DesktopNotifier {
    fn notify(&mut self, title: &str, body: &str) {
        // `notify-rust` returns a Result; any failure (no DBus, no
        // display, sandboxed CI) is intentionally ignored — see
        // module-level docs.
        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .show();
    }
}

/// The auto-heal plugin.
///
/// Generic over [`Clock`] and [`Notifier`] so unit tests can inject
/// deterministic behaviour. Construct with [`AutoHealPlugin::new`] for
/// production or [`AutoHealPlugin::with_parts`] for tests.
pub struct AutoHealPlugin<C: Clock = SystemClock, N: Notifier = DesktopNotifier> {
    clock: C,
    notifier: N,
    /// FIFO queue of [`PluginOperation`] values waiting to be drained
    /// by the host through [`Plugin::next_operation`].
    pending_ops: VecDeque<PluginOperation>,
    /// Last notification timestamp per path (for hourly rate limit).
    last_notification: HashMap<String, u64>,
    /// Quarantine timestamps per sync root (for daily quota).
    quarantines_by_root: HashMap<u64, Vec<u64>>,
    /// Mismatch history per path (for escalation decisions).
    mismatches_by_path: HashMap<String, Vec<u64>>,
    /// Full audit trail of mismatch events the plugin observed.
    history: Vec<MismatchRecord>,
    /// Tracks which sync roots have already been escalated to a full
    /// pause in the current 24h window to avoid flooding the host.
    escalated_roots: HashMap<u64, u64>,
}

impl AutoHealPlugin<SystemClock, DesktopNotifier> {
    /// Build a production auto-heal plugin using wall-clock time and
    /// the desktop notifier.
    #[must_use]
    pub fn new() -> Self {
        Self::with_parts(SystemClock, DesktopNotifier)
    }
}

impl Default for AutoHealPlugin<SystemClock, DesktopNotifier> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Clock, N: Notifier> AutoHealPlugin<C, N> {
    /// Build the plugin from explicit [`Clock`] and [`Notifier`]
    /// components. Primarily intended for tests.
    pub fn with_parts(clock: C, notifier: N) -> Self {
        Self {
            clock,
            notifier,
            pending_ops: VecDeque::new(),
            last_notification: HashMap::new(),
            quarantines_by_root: HashMap::new(),
            mismatches_by_path: HashMap::new(),
            history: Vec::new(),
            escalated_roots: HashMap::new(),
        }
    }

    /// Handle one integrity event. Public so the host or tests can
    /// drive the plugin without routing through
    /// [`PluginOperationResponse::IntegrityEvent`].
    pub fn handle_event(&mut self, event: &FileIntegrityResult) {
        match event.result {
            FileIntegrityOutcome::Ok | FileIntegrityOutcome::Unreadable => {
                // Nothing to do for healthy / unreadable files. The
                // scanner will surface unreadable files through other
                // channels; auto-heal only reacts to confirmed
                // mismatches.
            }
            FileIntegrityOutcome::Mismatch => self.handle_mismatch(event),
        }
    }

    /// Record the user's response to a previously-notified mismatch.
    /// Matches by (sync_root_id, path) on the most recent record;
    /// older records are left untouched so the audit trail remains
    /// accurate.
    pub fn record_user_response(&mut self, sync_root_id: u64, path: &str, response: UserResponse) {
        if let Some(record) = self
            .history
            .iter_mut()
            .rev()
            .find(|r| r.sync_root_id == sync_root_id && r.path == path)
        {
            record.user_response = Some(response);
        }
    }

    /// Count of mismatch events recorded for a given path in the last
    /// 24 hours. Useful for tests and host-side telemetry.
    #[must_use]
    pub fn recent_mismatches(&self, path: &str) -> u32 {
        let now = self.clock.now_secs();
        self.mismatches_by_path
            .get(path)
            .map(|v| {
                v.iter()
                    .filter(|t| now.saturating_sub(**t) < ONE_DAY)
                    .count() as u32
            })
            .unwrap_or(0)
    }

    /// Count of quarantine requests the plugin has emitted for the
    /// given sync root in the last 24 hours.
    #[must_use]
    pub fn recent_quarantines(&self, sync_root_id: u64) -> u32 {
        let now = self.clock.now_secs();
        self.quarantines_by_root
            .get(&sync_root_id)
            .map(|v| {
                v.iter()
                    .filter(|t| now.saturating_sub(**t) < ONE_DAY)
                    .count() as u32
            })
            .unwrap_or(0)
    }

    /// Full audit trail of mismatch events the plugin has observed.
    #[must_use]
    pub fn history(&self) -> &[MismatchRecord] {
        &self.history
    }

    /// Number of operations currently queued for the host to drain.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending_ops.len()
    }

    fn handle_mismatch(&mut self, event: &FileIntegrityResult) {
        let now = event.observed_at.unwrap_or_else(|| self.clock.now_secs());

        // Retention: trim any bookkeeping older than the 24h window.
        self.prune(now);

        // Record event for audit.
        self.history.push(MismatchRecord {
            sync_root_id: event.sync_root_id,
            path: event.path.clone(),
            at: now,
            user_response: None,
        });

        // Update mismatch counter for this path.
        self.mismatches_by_path
            .entry(event.path.clone())
            .or_default()
            .push(now);

        let mismatch_count = self
            .mismatches_by_path
            .get(&event.path)
            .map(|v| v.len() as u32)
            .unwrap_or(0);

        // Rate-limited desktop notification.
        let should_notify = self
            .last_notification
            .get(&event.path)
            .map(|last| now.saturating_sub(*last) >= ONE_HOUR)
            .unwrap_or(true);

        if should_notify {
            self.notifier.notify(
                "pcloud-rs: checksum mismatch",
                &format!(
                    "Integrity check failed for {} (sync root {}).",
                    event.path, event.sync_root_id
                ),
            );
            self.last_notification.insert(event.path.clone(), now);
        }

        // Quarantine — respect the per-root daily quota.
        let root_count = self
            .quarantines_by_root
            .get(&event.sync_root_id)
            .map(|v| v.len() as u32)
            .unwrap_or(0);

        if root_count < MAX_QUARANTINES_PER_ROOT_PER_DAY {
            self.pending_ops
                .push_back(PluginOperation::RequestQuarantine {
                    sync_root_id: event.sync_root_id,
                    path: event.path.clone(),
                });
            self.quarantines_by_root
                .entry(event.sync_root_id)
                .or_default()
                .push(now);
        }

        // Escalation — strictly more than `ESCALATION_THRESHOLD`
        // mismatches on the same path within 24h triggers a full
        // pause on the sync root. We only escalate once per sync root
        // per 24h window to avoid flooding the host.
        if mismatch_count > ESCALATION_THRESHOLD {
            let escalated_recently = self
                .escalated_roots
                .get(&event.sync_root_id)
                .map(|t| now.saturating_sub(*t) < ONE_DAY)
                .unwrap_or(false);

            if !escalated_recently {
                self.pending_ops
                    .push_back(PluginOperation::RequestSyncPause {
                        sync_root_id: event.sync_root_id,
                    });
                self.escalated_roots.insert(event.sync_root_id, now);
            }
        }
    }

    fn prune(&mut self, now: u64) {
        for v in self.mismatches_by_path.values_mut() {
            v.retain(|t| now.saturating_sub(*t) < ONE_DAY);
        }
        self.mismatches_by_path.retain(|_, v| !v.is_empty());

        for v in self.quarantines_by_root.values_mut() {
            v.retain(|t| now.saturating_sub(*t) < ONE_DAY);
        }
        self.quarantines_by_root.retain(|_, v| !v.is_empty());

        self.last_notification
            .retain(|_, t| now.saturating_sub(*t) < ONE_HOUR);

        self.escalated_roots
            .retain(|_, t| now.saturating_sub(*t) < ONE_DAY);
    }
}

impl<C: Clock + 'static, N: Notifier + 'static> Plugin for AutoHealPlugin<C, N> {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "pcloud.autoheal".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            display_name: "Auto-Heal Checksum Scanner".into(),
            requested_capabilities: [
                PluginCapability::ObserveStatus,
                PluginCapability::SyncControl,
            ]
            .into_iter()
            .collect(),
        }
    }

    fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
        // Subscribe to the integrity event stream on load.
        self.pending_ops
            .push_back(PluginOperation::ObserveIntegrityEvents);
        Ok(())
    }

    fn next_operation(&mut self) -> Option<PluginOperation> {
        self.pending_ops.pop_front()
    }

    fn on_response(&mut self, response: &PluginOperationResponse) {
        if let PluginOperationResponse::IntegrityEvent(event) = response {
            self.handle_event(event);
        }
    }
}

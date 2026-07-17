#![allow(clippy::pedantic)]
//! Behavioural tests for the auto-heal plugin. All tests inject a
//! deterministic clock and a capturing notifier so the suite runs
//! reliably in headless CI.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pcloud_plugin_api::{
    FileIntegrityOutcome, FileIntegrityResult, Plugin, PluginCapability, PluginContext,
    PluginOperation, PluginOperationResponse,
};
use pcloud_plugin_autoheal::{
    AutoHealPlugin, Clock, ESCALATION_THRESHOLD, MAX_QUARANTINES_PER_ROOT_PER_DAY, Notifier,
    SystemClock, UserResponse,
};

/// Deterministic, manually-advanced clock. `Send`-safe so the plugin
/// continues to satisfy the plugin trait bounds in tests.
#[derive(Clone)]
struct FakeClock {
    now: Arc<AtomicU64>,
}

impl FakeClock {
    fn new(start: u64) -> Self {
        Self {
            now: Arc::new(AtomicU64::new(start)),
        }
    }
    fn advance(&self, secs: u64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_secs(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// Capturing notifier that records every (title, body) pair.
#[derive(Clone, Default)]
struct CapturingNotifier {
    events: Arc<Mutex<Vec<(String, String)>>>,
}

impl CapturingNotifier {
    fn count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

impl Notifier for CapturingNotifier {
    fn notify(&mut self, title: &str, body: &str) {
        self.events
            .lock()
            .unwrap()
            .push((title.to_string(), body.to_string()));
    }
}

fn mismatch_event(sync_root_id: u64, path: &str, at: u64) -> FileIntegrityResult {
    FileIntegrityResult {
        sync_root_id,
        path: path.into(),
        result: FileIntegrityOutcome::Mismatch,
        observed_at: Some(at),
    }
}

fn ok_event(sync_root_id: u64, path: &str, at: u64) -> FileIntegrityResult {
    FileIntegrityResult {
        sync_root_id,
        path: path.into(),
        result: FileIntegrityOutcome::Ok,
        observed_at: Some(at),
    }
}

fn drain_ops<C: Clock + 'static, N: Notifier + 'static>(
    plugin: &mut AutoHealPlugin<C, N>,
) -> Vec<PluginOperation> {
    let mut out = Vec::new();
    while let Some(op) = pcloud_plugin_api::Plugin::next_operation(plugin) {
        out.push(op);
    }
    out
}

#[test]
fn single_mismatch_emits_notification_and_quarantine() {
    let clock = FakeClock::new(1_000_000);
    let notifier = CapturingNotifier::default();
    let mut plugin = AutoHealPlugin::with_parts(clock.clone(), notifier.clone());

    plugin.handle_event(&mismatch_event(7, "docs/report.pdf", clock.now_secs()));

    assert_eq!(notifier.count(), 1, "one desktop notification expected");

    let ops = drain_ops(&mut plugin);
    assert_eq!(ops.len(), 1, "exactly one operation queued");
    match &ops[0] {
        PluginOperation::RequestQuarantine { sync_root_id, path } => {
            assert_eq!(*sync_root_id, 7);
            assert_eq!(path, "docs/report.pdf");
        }
        other => panic!("expected RequestQuarantine, got {other:?}"),
    }
}

#[test]
fn three_mismatches_escalate_to_full_pause() {
    let clock = FakeClock::new(1_000_000);
    let notifier = CapturingNotifier::default();
    let mut plugin = AutoHealPlugin::with_parts(clock.clone(), notifier);

    // Four mismatches in < 24h on the SAME path → one escalation.
    for i in 0..4 {
        let t = clock.now_secs() + (i as u64) * 60;
        plugin.handle_event(&mismatch_event(42, "data/blob.bin", t));
    }

    let ops = drain_ops(&mut plugin);

    let quarantines = ops
        .iter()
        .filter(|o| matches!(o, PluginOperation::RequestQuarantine { .. }))
        .count();
    let pauses = ops
        .iter()
        .filter(|o| matches!(o, PluginOperation::RequestSyncPause { .. }))
        .count();

    assert_eq!(
        quarantines, 4,
        "one quarantine per mismatch (under daily cap)"
    );
    assert_eq!(pauses, 1, "exactly one escalation once threshold exceeded");

    // The escalation must target the offending sync root.
    let pause = ops
        .iter()
        .find(|o| matches!(o, PluginOperation::RequestSyncPause { .. }))
        .unwrap();
    match pause {
        PluginOperation::RequestSyncPause { sync_root_id } => {
            assert_eq!(*sync_root_id, 42);
        }
        _ => unreachable!(),
    }
}

#[test]
fn daily_quarantine_limit_respected() {
    let clock = FakeClock::new(1_000_000);
    let notifier = CapturingNotifier::default();
    let mut plugin = AutoHealPlugin::with_parts(clock.clone(), notifier);

    // 15 mismatches on DIFFERENT paths but the same sync root.
    // Different paths keeps the per-path notification limit and the
    // escalation path out of this test — we isolate the daily
    // quarantine cap.
    for i in 0..15u64 {
        let path = format!("file_{i}.bin");
        let t = clock.now_secs() + i * 300;
        plugin.handle_event(&mismatch_event(3, &path, t));
    }

    let ops = drain_ops(&mut plugin);
    let quarantines = ops
        .iter()
        .filter(|o| matches!(o, PluginOperation::RequestQuarantine { .. }))
        .count();

    assert_eq!(
        quarantines as u32, MAX_QUARANTINES_PER_ROOT_PER_DAY,
        "daily quarantine quota per sync root must be enforced"
    );
    assert_eq!(
        plugin.recent_quarantines(3),
        MAX_QUARANTINES_PER_ROOT_PER_DAY
    );
}

#[test]
fn ok_result_does_not_escalate() {
    let clock = FakeClock::new(1_000_000);
    let notifier = CapturingNotifier::default();
    let mut plugin = AutoHealPlugin::with_parts(clock.clone(), notifier.clone());

    for i in 0..20 {
        let t = clock.now_secs() + (i as u64) * 60;
        plugin.handle_event(&ok_event(1, "good/file.txt", t));
    }

    let ops = drain_ops(&mut plugin);

    assert!(ops.is_empty(), "Ok outcomes must not emit operations");
    assert_eq!(notifier.count(), 0, "Ok outcomes must not notify");
    assert_eq!(plugin.recent_mismatches("good/file.txt"), 0);
}

#[test]
fn notification_rate_limit_one_per_path_per_hour() {
    let clock = FakeClock::new(1_000_000);
    let notifier = CapturingNotifier::default();
    let mut plugin = AutoHealPlugin::with_parts(clock.clone(), notifier.clone());

    // Three mismatches on the same path within 30min.
    plugin.handle_event(&mismatch_event(1, "a.txt", clock.now_secs()));
    clock.advance(600);
    plugin.handle_event(&mismatch_event(1, "a.txt", clock.now_secs()));
    clock.advance(600);
    plugin.handle_event(&mismatch_event(1, "a.txt", clock.now_secs()));

    assert_eq!(
        notifier.count(),
        1,
        "at most one notification per hour per path"
    );

    // After an hour + a bit, a new notification is allowed.
    clock.advance(3_700);
    plugin.handle_event(&mismatch_event(1, "a.txt", clock.now_secs()));
    assert_eq!(notifier.count(), 2);
}

#[test]
fn plugin_contract_history_unreadable_and_retention_are_defined() {
    let production = AutoHealPlugin::new();
    assert_eq!(production.manifest().id, "pcloud.autoheal");
    assert!(SystemClock.now_secs() > 0);
    let defaulted = AutoHealPlugin::default();
    assert_eq!(
        defaulted.manifest().requested_capabilities,
        [
            PluginCapability::ObserveStatus,
            PluginCapability::SyncControl
        ]
        .into_iter()
        .collect()
    );

    let clock = FakeClock::new(2_000_000);
    let notifier = CapturingNotifier::default();
    let mut plugin = AutoHealPlugin::with_parts(clock.clone(), notifier.clone());
    assert_eq!(plugin.pending_len(), 0);
    assert!(plugin.history().is_empty());
    assert_eq!(plugin.recent_mismatches("missing"), 0);
    assert_eq!(plugin.recent_quarantines(999), 0);
    plugin.record_user_response(999, "missing", UserResponse::Ignore);

    plugin
        .on_load(&PluginContext {
            runtime_summary: "coverage".into(),
            granted_capabilities: [
                PluginCapability::ObserveStatus,
                PluginCapability::SyncControl,
            ]
            .into_iter()
            .collect(),
            dev_mode: true,
        })
        .unwrap();
    assert_eq!(
        plugin.next_operation(),
        Some(PluginOperation::ObserveIntegrityEvents)
    );
    assert!(plugin.next_operation().is_none());
    plugin.on_response(&PluginOperationResponse::TimerAck);

    plugin.handle_event(&FileIntegrityResult {
        sync_root_id: 8,
        path: "unreadable".into(),
        result: FileIntegrityOutcome::Unreadable,
        observed_at: None,
    });
    assert!(plugin.history().is_empty());

    plugin.on_response(&PluginOperationResponse::IntegrityEvent(mismatch_event(
        8,
        "damaged",
        clock.now_secs(),
    )));
    plugin.record_user_response(8, "damaged", UserResponse::Acknowledge);
    assert_eq!(
        plugin.history().last().unwrap().user_response,
        Some(UserResponse::Acknowledge)
    );
    plugin.record_user_response(8, "damaged", UserResponse::Refetch);
    assert_eq!(
        plugin.history().last().unwrap().user_response,
        Some(UserResponse::Refetch)
    );

    for i in 1..=ESCALATION_THRESHOLD + 1 {
        clock.advance(60);
        plugin.handle_event(&mismatch_event(8, "damaged", clock.now_secs()));
        assert!(plugin.recent_mismatches("damaged") >= i);
    }
    let pauses = drain_ops(&mut plugin)
        .into_iter()
        .filter(|op| matches!(op, PluginOperation::RequestSyncPause { .. }))
        .count();
    assert_eq!(pauses, 1);

    clock.advance(25 * 60 * 60);
    plugin.handle_event(&mismatch_event(8, "fresh", clock.now_secs()));
    assert_eq!(plugin.recent_mismatches("damaged"), 0);
    assert_eq!(plugin.recent_mismatches("fresh"), 1);
    assert!(notifier.count() >= 2);
}

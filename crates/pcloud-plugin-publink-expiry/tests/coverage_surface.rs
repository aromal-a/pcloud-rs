use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_plugin_api::{
    Plugin, PluginCapability, PluginContext, PluginOperation, PluginOperationResponse,
    PublinkSummary,
};
use pcloud_plugin_publink_expiry::{
    CapturingNotifier, Clock, DEFAULT_NOTIFY_WINDOW_HOURS, FixedClock, NotificationState, Notifier,
    PublinkExpiryConfig, PublinkExpiryError, PublinkExpiryPlugin, RATE_LIMIT_SECS, SystemClock,
};

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pcloud-publink-coverage-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn configuration_state_and_default_components_cover_public_contract() {
    let default = PublinkExpiryConfig::default();
    assert!(default.enabled);
    assert_eq!(default.notify_window_hours, DEFAULT_NOTIFY_WINDOW_HOURS);
    assert_eq!(default.notify_window_secs(), 24 * 3_600);
    assert!(PublinkExpiryConfig::default_state_path().is_some());
    assert!(
        PublinkExpiryConfig {
            notify_window_hours: 0,
            ..default.clone()
        }
        .resolve_state_path()
        .is_err()
    );

    let state_path = temp_path("nested").join("state.json");
    let explicit = PublinkExpiryConfig {
        enabled: true,
        notify_window_hours: 2,
        state_file: Some(state_path.clone()),
    };
    assert_eq!(explicit.resolve_state_path().unwrap(), state_path);
    let mut state = NotificationState::default();
    assert!(state.should_notify("new", 100));
    state.mark_notified("new", 100);
    assert!(!state.should_notify("new", 100 + RATE_LIMIT_SECS - 1));
    assert!(state.should_notify("new", 100 + RATE_LIMIT_SECS));
    state.save(&state_path).unwrap();
    assert_eq!(NotificationState::load(&state_path).unwrap(), state);

    let malformed = temp_path("malformed.json");
    std::fs::write(&malformed, b"{").unwrap();
    assert!(matches!(
        NotificationState::load(&malformed),
        Err(PublinkExpiryError::Parse(_))
    ));
    let directory = temp_path("directory");
    std::fs::create_dir_all(&directory).unwrap();
    assert!(matches!(
        NotificationState::load(&directory),
        Err(PublinkExpiryError::Io(_))
    ));
    let parent_file = temp_path("parent-file");
    std::fs::write(&parent_file, b"x").unwrap();
    assert!(matches!(
        state.save(&parent_file.join("state.json")),
        Err(PublinkExpiryError::Io(_))
    ));

    let mut capturing = CapturingNotifier::default();
    capturing.notify("title", "body");
    assert_eq!(capturing.emitted.len(), 1);
    assert!(SystemClock.now_unix() > 0);
    assert_eq!(FixedClock(42).now_unix(), 42);

    let production_state = temp_path("production.json");
    let plugin = PublinkExpiryPlugin::new(PublinkExpiryConfig {
        state_file: Some(production_state.clone()),
        ..default
    })
    .unwrap();
    assert!(format!("{plugin:?}").contains("PublinkExpiryPlugin"));
    assert_eq!(plugin.state_path(), production_state);
    assert_eq!(plugin.notify_window_secs(), 24 * 3_600);
    assert!(plugin.state().last_notified.is_empty());

    let _ = std::fs::remove_file(state_path);
    let _ = std::fs::remove_file(malformed);
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_file(parent_file);
}

#[test]
fn plugin_lifecycle_and_link_filtering_cover_all_public_branches() {
    let now = 1_000_000;
    let state_path = temp_path("plugin.json");
    let mut plugin = PublinkExpiryPlugin::with_parts(
        PublinkExpiryConfig {
            enabled: true,
            notify_window_hours: 24,
            state_file: Some(state_path.clone()),
        },
        Box::new(CapturingNotifier::default()),
        Box::new(FixedClock(now)),
    )
    .unwrap();
    let manifest = plugin.manifest();
    assert_eq!(manifest.id, "pcloud-rs.publink-expiry");
    assert_eq!(
        manifest.requested_capabilities,
        BTreeSet::from([PluginCapability::ObserveStatus])
    );
    assert!(plugin.signature().is_none());
    plugin
        .on_load(&PluginContext {
            runtime_summary: "coverage".into(),
            granted_capabilities: BTreeSet::from([PluginCapability::ObserveStatus]),
            dev_mode: true,
        })
        .unwrap();
    assert!(matches!(
        plugin.next_operation(),
        Some(PluginOperation::TimerTick { period_secs: 60 })
    ));
    assert!(matches!(
        plugin.next_operation(),
        Some(PluginOperation::ObservePublinkList)
    ));
    assert!(plugin.next_operation().is_none());
    plugin.tick(5);
    assert_eq!(
        plugin.next_operation(),
        Some(PluginOperation::TimerTick { period_secs: 5 })
    );

    let links = vec![
        PublinkSummary {
            link_id: "no-expiry".into(),
            label: "ignored".into(),
            expiry_unix: None,
        },
        PublinkSummary {
            link_id: "expired".into(),
            label: "ignored".into(),
            expiry_unix: Some(now - 1),
        },
        PublinkSummary {
            link_id: "far".into(),
            label: "ignored".into(),
            expiry_unix: Some(now + 25 * 3_600),
        },
        PublinkSummary {
            link_id: "unnamed".into(),
            label: String::new(),
            expiry_unix: Some(now + 3_600),
        },
    ];
    assert_eq!(plugin.process_publinks(&links).unwrap(), 1);
    assert_eq!(plugin.process_publinks(&links).unwrap(), 0);
    plugin.on_response(&PluginOperationResponse::TimerAck);
    plugin.on_response(&PluginOperationResponse::PublinkList(Vec::new()));

    let mut disabled = PublinkExpiryPlugin::with_parts(
        PublinkExpiryConfig {
            enabled: false,
            notify_window_hours: 24,
            state_file: Some(temp_path("disabled.json")),
        },
        Box::new(CapturingNotifier::default()),
        Box::new(FixedClock(now)),
    )
    .unwrap();
    assert!(
        disabled
            .on_load(&PluginContext {
                runtime_summary: String::new(),
                granted_capabilities: BTreeSet::new(),
                dev_mode: true,
            })
            .is_err()
    );
    let _ = std::fs::remove_file(state_path);
}

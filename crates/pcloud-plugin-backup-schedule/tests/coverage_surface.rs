use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use pcloud_plugin_api::{
    Plugin, PluginCapability, PluginContext, PluginOperation, PluginOperationResponse,
};
use pcloud_plugin_backup_schedule::{
    BackupScheduleCliCommand, BackupScheduleCliReply, BackupScheduleConfig, BackupScheduleError,
    BackupSchedulePlugin, Clock, MAX_SCHEDULES, ManualClock, ScheduleEntry, SystemClock, apply_cli,
    parse_schedule,
};

#[derive(Clone)]
struct SharedClock(Arc<Mutex<ManualClock>>);

impl Clock for SharedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0.lock().expect("clock").now()
    }
}

fn entry(name: impl Into<String>, schedule: impl Into<String>, enabled: bool) -> ScheduleEntry {
    ScheduleEntry {
        name: name.into(),
        schedule: schedule.into(),
        sync_root_id: 7,
        enabled,
    }
}

#[test]
fn public_schedule_parser_covers_canonical_and_rejection_surface() {
    for (input, fragment) in [
        ("0 18 * * 5", "0 0 18 * * 5 *"),
        ("0 0 18 * * 5", "0 0 18 * * 5 *"),
        ("0 0 18 * * FRI *", "0 0 18 * * FRI *"),
        ("hourly", "0 0 * * * * *"),
        ("daily", "0 0 0 * * * *"),
        ("daily at 23:59", "0 59 23 * * * *"),
        ("weekly", "MON"),
        ("weekly on sun", "SUN"),
        ("weekly on tues at 01:02", "TUE"),
        ("monthly", "0 0 0 1 * * *"),
        ("monthly on 31 at 12:34", "0 34 12 31 * * *"),
        ("every wed", "WED"),
        ("every thurs at 04:05", "THU"),
        ("every saturday 06:07", "SAT"),
    ] {
        let parsed = parse_schedule(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert!(
            parsed.as_cron().contains(fragment),
            "{input}: {}",
            parsed.as_cron()
        );
        assert!(
            parsed
                .next_after(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap())
                .is_some()
        );
    }

    for input in [
        "",
        " ",
        "0 0 0 0 0",
        "0 0 x * *",
        "0 0 0 * * * * extra",
        "hourly now",
        "daily at",
        "daily at noon",
        "daily at xx:01",
        "daily at 01:xx",
        "daily at 24:00",
        "daily at 00:60",
        "weekly on",
        "weekly on someday",
        "monthly on",
        "monthly on xx",
        "monthly on 0",
        "monthly on 32",
        "every",
        "every nonsense",
        "every friday at",
        "every friday 12:00 extra",
        "run every minute",
    ] {
        assert!(
            parse_schedule(input).is_err(),
            "{input:?} unexpectedly parsed"
        );
    }

    for day in ["sun", "mon", "tue", "wed", "thu", "fri", "sat"] {
        assert!(parse_schedule(&format!("every {day} 00:00")).is_ok());
    }
}

#[test]
fn config_clock_cli_and_plugin_contract_cover_success_and_failures() {
    let mut config = BackupScheduleConfig::default();
    assert!(config.validate().is_ok());
    config.add(entry("hourly", "hourly", true)).unwrap();
    assert_eq!(config.iter().count(), 1);
    assert!(matches!(
        config.add(entry("hourly", "daily", true)),
        Err(BackupScheduleError::DuplicateName(_))
    ));
    assert!(matches!(
        config.add(entry("bad", "not a schedule", true)),
        Err(BackupScheduleError::InvalidSchedule(_))
    ));
    assert!(matches!(
        config.remove("missing"),
        Err(BackupScheduleError::NotFound(_))
    ));
    assert_eq!(config.remove("hourly").unwrap().name, "hourly");

    let duplicate = BackupScheduleConfig {
        entries: vec![entry("same", "hourly", true), entry("same", "daily", true)],
    };
    assert!(matches!(
        duplicate.validate(),
        Err(BackupScheduleError::DuplicateName(_))
    ));
    assert!(BackupSchedulePlugin::new(duplicate).is_err());

    let mut full = BackupScheduleConfig {
        entries: (0..MAX_SCHEDULES)
            .map(|i| entry(format!("job-{i}"), "hourly", true))
            .collect(),
    };
    assert!(matches!(
        full.add(entry("overflow", "hourly", true)),
        Err(BackupScheduleError::TooMany { .. })
    ));
    full.entries.push(entry("forced-overflow", "hourly", true));
    assert!(matches!(
        full.validate(),
        Err(BackupScheduleError::TooMany { .. })
    ));

    let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let mut manual = ManualClock::new(start);
    manual.advance_secs(1);
    assert_eq!(manual.now(), start + chrono::Duration::seconds(1));
    manual.set(start);
    assert_eq!(manual.now(), start);
    assert!(SystemClock.now() >= start);

    let mut cli = BackupScheduleConfig::default();
    assert_eq!(
        apply_cli(
            &mut cli,
            BackupScheduleCliCommand::Add {
                name: "nightly".into(),
                schedule: "daily at 02:00".into(),
                sync_root_id: 19,
            }
        ),
        BackupScheduleCliReply::Ok
    );
    assert!(matches!(
        apply_cli(
            &mut cli,
            BackupScheduleCliCommand::Add {
                name: "invalid".into(),
                schedule: "never".into(),
                sync_root_id: 20,
            }
        ),
        BackupScheduleCliReply::Error { .. }
    ));
    assert!(matches!(
        apply_cli(&mut cli, BackupScheduleCliCommand::List),
        BackupScheduleCliReply::List { entries } if entries.len() == 1
    ));
    assert!(matches!(
        apply_cli(
            &mut cli,
            BackupScheduleCliCommand::Remove {
                name: "absent".into()
            }
        ),
        BackupScheduleCliReply::Error { .. }
    ));
    assert_eq!(
        apply_cli(
            &mut cli,
            BackupScheduleCliCommand::Remove {
                name: "nightly".into()
            }
        ),
        BackupScheduleCliReply::Ok
    );

    let shared = Arc::new(Mutex::new(ManualClock::new(start)));
    let config = BackupScheduleConfig {
        entries: vec![
            entry("active", "hourly", true),
            entry("disabled", "hourly", false),
        ],
    };
    let mut plugin =
        BackupSchedulePlugin::new_with_clock(config, Box::new(SharedClock(shared.clone())))
            .unwrap();
    assert!(format!("{plugin:?}").contains("BackupSchedulePlugin"));
    assert_eq!(plugin.entries().count(), 2);
    let manifest = plugin.manifest();
    assert_eq!(manifest.id, "pcloud-plugin-backup-schedule");
    assert_eq!(
        manifest.requested_capabilities,
        BTreeSet::from([PluginCapability::SyncControl])
    );
    assert!(plugin.signature().is_none());
    plugin
        .on_load(&PluginContext {
            runtime_summary: "test".into(),
            granted_capabilities: BTreeSet::from([PluginCapability::SyncControl]),
            dev_mode: true,
        })
        .unwrap();
    plugin.on_response(&PluginOperationResponse::SyncControlAck);
    assert!(plugin.next_operation().is_none());

    shared.lock().unwrap().advance_secs(1_100 * 3_600);
    plugin.tick();
    assert_eq!(plugin.pending_len(), 1_025);
    assert!(matches!(
        plugin.next_operation(),
        Some(PluginOperation::RequestSyncResume { sync_root_id: 7 })
    ));

    assert!(
        serde_json::from_str::<ScheduleEntry>(
            r#"{"name":"default-enabled","schedule":"hourly","sync_root_id":1}"#
        )
        .unwrap()
        .enabled
    );
    assert!(
        BackupScheduleError::Initialization("boom".into())
            .to_string()
            .contains("boom")
    );
}

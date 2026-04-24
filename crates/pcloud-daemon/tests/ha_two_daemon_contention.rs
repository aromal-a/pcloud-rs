#![allow(clippy::pedantic)]
//! Integration test: Tier-2 HA file-lock contention between two
//! bootstrap-assembled `RuntimeShell` instances pointing at the same
//! `state_dir`.
//!
//! Scenarios covered:
//!
//! 1. **Primary wins.** First `bootstrap_with_config` under
//!    `[ha].enabled = true` acquires the lease and reports
//!    `mode = "primary"` via `Method::HaStatus`.
//! 2. **Refuse mode.** A second bootstrap against the same `state_dir`
//!    with `mode = "refuse"` returns a `BootstrapError` whose message
//!    names the primary.
//! 3. **Passive mode.** A second bootstrap with `mode = "passive"`
//!    succeeds, reports `mode = "passive"` from the HA probe, and
//!    rejects non-probe requests with `ResponseStatus::Unavailable`.
//! 4. **Promotion.** Dropping the primary's `RuntimeShell` releases
//!    the lease; a fresh secondary bootstrap subsequently acquires it.
//!
//! The test does **not** bind any real IPC socket — it drives the
//! dispatch path through `RuntimeShell::handle_request` directly,
//! which is the same entry point `serve::accept_loop` uses per
//! accepted client.
//!
//! **PLATFORM:** Linux + Windows. On Unix the lease relies on
//! `flock(2)` advisory locking; on Windows on `LockFileEx` over a
//! single reserved sentinel byte at a fixed offset far past any
//! realistic metadata size (see `ha_lease::win_lock`). Both provide
//! the same "auto-release on process exit" semantic, so the takeover
//! test below is cross-platform.


use pcloud_config::ha::{HaContendedMode, HaPolicy};
use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::bootstrap_with_config;
use pcloud_ipc::{Method, Request, ResponseStatus};

fn unique_root(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("pcloud-ha-{tag}-{}-{nonce}", std::process::id()))
}

fn base_profile(root: &std::path::Path, ha: HaPolicy) -> ConfigProfile {
    let mut cfg = ConfigProfile::secure_defaults(root.to_path_buf(), Environment::Development);
    cfg.ha = ha;
    cfg
}

#[test]
fn primary_acquires_and_reports_primary_status() {
    let root = unique_root("primary");
    let ha = HaPolicy {
        enabled: true,
        mode: HaContendedMode::Refuse,
        heartbeat_interval_secs: 30,
        passive_poll_interval_secs: 10,
    };
    let mut primary = bootstrap_with_config(base_profile(&root, ha)).expect("primary boots");

    let response = primary.handle_request(Request::Plain {
        method: Method::HaStatus,
    });
    assert_eq!(response.status, ResponseStatus::Ok);
    assert!(
        response.message.contains("\"primary\""),
        "expected primary status, got: {}",
        response.message
    );
    // `mode` field is lower-cased per HaMode serde rename_all.
    assert!(
        response.message.contains("\"mode\":\"primary\""),
        "mode key missing: {}",
        response.message
    );
}

#[test]
fn refuse_mode_blocks_second_daemon_with_diagnostic() {
    let root = unique_root("refuse");
    let ha = HaPolicy {
        enabled: true,
        mode: HaContendedMode::Refuse,
        heartbeat_interval_secs: 30,
        passive_poll_interval_secs: 10,
    };
    let _primary = bootstrap_with_config(base_profile(&root, ha)).expect("primary boots");

    // Second bootstrap against the same state_dir must fail with a
    // diagnostic that identifies the primary (pid=… / instance=…).
    let err = bootstrap_with_config(base_profile(
        &root,
        HaPolicy {
            enabled: true,
            mode: HaContendedMode::Refuse,
            heartbeat_interval_secs: 30,
            passive_poll_interval_secs: 10,
        },
    ))
    .expect_err("second bootstrap must refuse");
    let msg = format!("{err}");
    assert!(
        msg.contains("Tier-2 HA lease already held"),
        "error not HA-flavoured: {msg}"
    );
    assert!(msg.contains("pid="), "error must name primary pid: {msg}");
}

#[test]
fn passive_mode_rejects_non_probe_requests_with_unavailable() {
    let root = unique_root("passive");
    let _primary = bootstrap_with_config(base_profile(
        &root,
        HaPolicy {
            enabled: true,
            mode: HaContendedMode::Refuse,
            heartbeat_interval_secs: 30,
            passive_poll_interval_secs: 10,
        },
    ))
    .expect("primary boots");

    let mut passive = bootstrap_with_config(base_profile(
        &root,
        HaPolicy {
            enabled: true,
            mode: HaContendedMode::Passive,
            heartbeat_interval_secs: 30,
            passive_poll_interval_secs: 10,
        },
    ))
    .expect("passive boots");

    // HaStatus probe is allow-listed — works, and reports passive.
    let probe = passive.handle_request(Request::Plain {
        method: Method::HaStatus,
    });
    assert_eq!(probe.status, ResponseStatus::Ok);
    assert!(
        probe.message.contains("\"mode\":\"passive\""),
        "passive probe missing mode: {}",
        probe.message
    );

    // A non-probe request (`GetStatus`) is rejected with a message
    // that names the primary.
    let blocked = passive.handle_request(Request::Plain {
        method: Method::GetStatus,
    });
    assert_eq!(blocked.status, ResponseStatus::Unavailable);
    assert!(
        blocked.message.contains("passive"),
        "rejection missing 'passive': {}",
        blocked.message
    );
    assert!(
        blocked.message.contains("pid="),
        "rejection missing primary pid: {}",
        blocked.message
    );

    // Health probes stay available — supervisors must still reach us.
    let health = passive.handle_request(Request::Plain {
        method: Method::GetHealth,
    });
    assert_eq!(health.status, ResponseStatus::Ok);
}

#[test]
fn passive_can_take_over_after_primary_releases() {
    let root = unique_root("takeover");

    // Acquire then drop the primary. On drop the kernel releases the
    // advisory flock, so a fresh bootstrap on the same state_dir
    // should succeed as the new primary.
    {
        let _primary = bootstrap_with_config(base_profile(
            &root,
            HaPolicy {
                enabled: true,
                mode: HaContendedMode::Refuse,
                heartbeat_interval_secs: 30,
                passive_poll_interval_secs: 10,
            },
        ))
        .expect("primary boots");
        // `_primary` drops here → lease is released.
    }

    let mut new_primary = bootstrap_with_config(base_profile(
        &root,
        HaPolicy {
            enabled: true,
            mode: HaContendedMode::Refuse,
            heartbeat_interval_secs: 30,
            passive_poll_interval_secs: 10,
        },
    ))
    .expect("takeover boots");

    let response = new_primary.handle_request(Request::Plain {
        method: Method::HaStatus,
    });
    assert_eq!(response.status, ResponseStatus::Ok);
    assert!(
        response.message.contains("\"mode\":\"primary\""),
        "takeover mode: {}",
        response.message
    );
}

#[test]
fn disabled_ha_reports_disabled_mode() {
    let root = unique_root("disabled");
    let mut shell = bootstrap_with_config(base_profile(&root, HaPolicy::default()))
        .expect("shell boots with HA disabled");

    let probe = shell.handle_request(Request::Plain {
        method: Method::HaStatus,
    });
    assert_eq!(probe.status, ResponseStatus::Ok);
    assert!(
        probe.message.contains("\"mode\":\"disabled\""),
        "disabled probe: {}",
        probe.message
    );

    // Non-probe requests still work normally.
    let status = shell.handle_request(Request::Plain {
        method: Method::GetStatus,
    });
    assert_eq!(status.status, ResponseStatus::Ok);
}

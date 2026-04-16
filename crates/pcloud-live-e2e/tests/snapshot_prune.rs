#![allow(clippy::pedantic)]
//! Live snapshot-prune coverage: seed a directory with 10 fake snapshot
//! archives spanning ~8 weeks, dispatch
//! `SnapshotAction::Prune { retention_days: 7 }` through the daemon IPC,
//! and assert the Grandfather-Father-Son semantics hold:
//!
//! * The freshest daily entry survives.
//! * Each distinct weekly bucket beyond the daily window keeps exactly
//!   one representative.
//! * The >8-week ancient file is removed.
//!
//! This binary does **not** contact the pCloud backend: `snapshot prune`
//! is a local filesystem operation. We still gate the run on
//! `PCLOUD_LIVE_E2E=1` so the binary is part of the opt-in suite.
//!
//! Pre-alpha honesty: the GFS bucketer is covered by unit tests inside
//! `pcloud-backends`. This binary re-proves the IPC → backend plumb
//! actually wires through with `yes=true` gating enforced.

#![forbid(unsafe_code)]

// **PLATFORM:** all (uses only stdlib mtime APIs).
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use pcloud_ipc::{Request, ResponseStatus, SnapshotAction};

use crate::common::{TestDaemon, assert_no_secret_leak, skip_if_not_live, status_label};

const SECS_PER_DAY: u64 = 86_400;

/// Create an empty `pcloud-rs-<tag>.tar.zst` file and set its mtime to
/// `days_ago` days before "now". We use [`std::fs::File::set_modified`]
/// (stable since Rust 1.75) so we don't need the `filetime` crate.
fn seed_snapshot(dir: &Path, tag: &str, days_ago: u64) -> PathBuf {
    let path = dir.join(format!("pcloud-rs-{tag}.tar.zst"));
    let mut file = fs::File::create(&path).expect("create fake snapshot");
    // Non-empty payload so the file is unambiguously present on disk.
    use std::io::Write as _;
    file.write_all(b"fake-snapshot-payload-for-prune-test")
        .expect("write fake payload");
    let mtime = SystemTime::now()
        .checked_sub(Duration::from_secs(days_ago * SECS_PER_DAY + 600))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    file.set_modified(mtime).expect("set snapshot mtime");
    drop(file);
    path
}

#[test]
#[ignore = "live-e2e: gated on PCLOUD_LIVE_E2E=1"]
fn live_snapshot_prune_gfs_semantics() {
    if skip_if_not_live(&[]) {
        return;
    }

    let mut daemon = TestDaemon::new("snapshot-prune");

    // Build a scratch directory under the daemon's own temp-owned tree
    // so cleanup comes for free when `TestDaemon` drops.
    let dir = daemon
        .config
        .paths
        .config_dir
        .parent()
        .expect("temp parent exists")
        .join("snapshot-prune-fixtures");
    fs::create_dir_all(&dir).expect("mkdir fixtures");

    // Seed 10 snapshots covering: 3 same-day, 3 in the daily window, 2 in
    // weekly buckets, 1 in a monthly bucket, and 1 well beyond all
    // buckets. Expected keep-set with retention_days=7 (from the unit
    // tests in pcloud-backends):
    // keep: d0-new, d1, d2, d5, w0 (age 10), w1 (age 20), m0 (age 70)
    // drop: d0-old, d0-mid, ancient (age 400).
    let d0_old = seed_snapshot(&dir, "d0-old", 0);
    let d0_mid = seed_snapshot(&dir, "d0-mid", 0);
    let d0_new = seed_snapshot(&dir, "d0-new", 0);
    // Make d0_new clearly fresher than the other two by advancing its
    // mtime to the absolute latest value within day 0.
    {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&d0_new)
            .expect("reopen d0_new");
        f.set_modified(SystemTime::now() - Duration::from_secs(60))
            .expect("bump d0_new mtime");
    }
    let d1 = seed_snapshot(&dir, "d1", 1);
    let d2 = seed_snapshot(&dir, "d2", 2);
    let d5 = seed_snapshot(&dir, "d5", 5);
    let w0 = seed_snapshot(&dir, "w0", 10);
    let w1 = seed_snapshot(&dir, "w1", 20);
    let m0 = seed_snapshot(&dir, "m0", 70);
    let ancient = seed_snapshot(&dir, "ancient", 400);

    // Also drop an unrelated file that must be untouched.
    let unrelated = dir.join("not-a-snapshot.txt");
    fs::write(&unrelated, b"ignore me").expect("write unrelated");

    // 1) Refuse to prune without --yes — wire safety gate.
    let no_confirm = daemon.dispatch(Request::BackupSnapshot {
        action: SnapshotAction::Prune,
        path: dir.clone(),
        gpg_recipient: None,
        yes: false,
        retention_days: Some(7),
        zstd_level: None,
    });
    assert_no_secret_leak(&no_confirm);
    assert_eq!(
        no_confirm.status,
        ResponseStatus::InvalidRequest,
        "prune without --yes must be rejected: status={} message={}",
        status_label(&no_confirm.status),
        no_confirm.message
    );

    // 2) Refuse to prune without retention_days.
    let no_retention = daemon.dispatch(Request::BackupSnapshot {
        action: SnapshotAction::Prune,
        path: dir.clone(),
        gpg_recipient: None,
        yes: true,
        retention_days: None,
        zstd_level: None,
    });
    assert_no_secret_leak(&no_retention);
    assert_eq!(
        no_retention.status,
        ResponseStatus::InvalidRequest,
        "prune without retention_days must be rejected: status={} message={}",
        status_label(&no_retention.status),
        no_retention.message
    );

    // 3) Prune with retention_days=7. All ten seeded files still there.
    let prune = daemon.dispatch(Request::BackupSnapshot {
        action: SnapshotAction::Prune,
        path: dir.clone(),
        gpg_recipient: None,
        yes: true,
        retention_days: Some(7),
        zstd_level: None,
    });
    assert_no_secret_leak(&prune);
    assert_eq!(
        prune.status,
        ResponseStatus::Ok,
        "prune failed: status={} message={}",
        status_label(&prune.status),
        prune.message
    );

    // 4) Post-prune: d0_new / d1 / d2 / d5 / w0 / w1 / m0 must still
    //    exist; d0_old / d0_mid / ancient must be gone. The unrelated
    //    file must still exist.
    for (path, must_exist, label) in [
        (&d0_new, true, "d0_new"),
        (&d1, true, "d1"),
        (&d2, true, "d2"),
        (&d5, true, "d5"),
        (&w0, true, "w0"),
        (&w1, true, "w1"),
        (&m0, true, "m0"),
        (&d0_old, false, "d0_old"),
        (&d0_mid, false, "d0_mid"),
        (&ancient, false, "ancient"),
        (&unrelated, true, "unrelated"),
    ] {
        let exists = path.exists();
        assert_eq!(
            exists,
            must_exist,
            "GFS prune broke expected keep-set for {label}: \
             exists={exists} expected={must_exist} path={}",
            path.display()
        );
    }

    // 5) Response message is JSON with `removed_count` >= 3 and `ok:true`.
    let payload: serde_json::Value =
        serde_json::from_str(&prune.message).expect("prune message must be JSON");
    assert_eq!(payload.get("ok").and_then(|v| v.as_bool()), Some(true));
    let removed_count = payload
        .get("removed_count")
        .and_then(|v| v.as_u64())
        .expect("removed_count present");
    assert!(
        removed_count >= 3,
        "expected at least 3 removed (d0_old, d0_mid, ancient); got {removed_count}"
    );
}

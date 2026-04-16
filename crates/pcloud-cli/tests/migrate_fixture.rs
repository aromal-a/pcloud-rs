#![allow(clippy::pedantic)]
//! Integration coverage for the `pcloudc migrate-from-c` helper.
//!
//! The migrate module lives in the `pcloudc` binary crate (there is no
//! `pcloud-cli` library target today), so we pull it in via `#[path]`
//! for the duration of this test. No other crates are touched.
//!
//! **PLATFORM:** Unix only. The C `pcloud-rs` client never ran on
//! Windows, so the module and its tests are gated at compile time.

#![cfg(unix)]

#[path = "../src/migrate.rs"]
mod migrate;

use std::fs;
use std::path::Path;

use migrate::{MigrateError, MigrationPlan};
use pcloud_model::sync::SyncType;
use rusqlite::{Connection, params};
use tempfile::TempDir;

fn build_legacy_fixture(legacy_home: &Path) {
    fs::create_dir_all(legacy_home).unwrap();
    let conn = Connection::open(legacy_home.join(".pclouddb")).unwrap();
    conn.execute_batch(
        "CREATE TABLE setting (id TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE syncfolder (
             id INTEGER PRIMARY KEY,
             folderid INTEGER,
             localpath TEXT,
             synctype INTEGER,
             flags INTEGER,
             inode INTEGER,
             deviceid INTEGER
         );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO setting (id, value) VALUES ('auth', 'legacy-token-abc')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO setting (id, value) VALUES ('user', 'alice@example.com')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO setting (id, value) VALUES ('pass', 'secret-plaintext')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO setting (id, value) VALUES ('usessl', '1')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO syncfolder (id, folderid, localpath, synctype, flags, inode, deviceid) \
         VALUES (?1, ?2, ?3, ?4, 0, 0, 0)",
        params![1i64, 42i64, "/home/alice/Docs", 3i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO syncfolder (id, folderid, localpath, synctype, flags, inode, deviceid) \
         VALUES (?1, ?2, ?3, ?4, 0, 0, 0)",
        params![2i64, 43i64, "/home/alice/Photos", 1i64],
    )
    .unwrap();
}

#[test]
fn migrate_fixture_seeds_store_and_vault() {
    let tmp = TempDir::new().unwrap();
    let legacy_home = tmp.path().join("legacy");
    let target_config = tmp.path().join("cfg");
    let target_data = tmp.path().join("data");
    build_legacy_fixture(&legacy_home);

    let plan = MigrationPlan::detect_with_targets(
        Some(legacy_home.clone()),
        false,
        false,
        Some(target_config.clone()),
        Some(target_data.clone()),
    )
    .unwrap()
    .expect("plan should be present when legacy db exists");

    // Preview must never include the raw token.
    let preview = plan.render_preview();
    assert!(!preview.contains("legacy-token-abc"));
    assert!(preview.contains("auth token present : yes"));

    let report = plan.execute().expect("execute should succeed");
    assert_eq!(report.sync_roots_seeded, 2);
    assert!(report.auth_token_carried);
    assert!(report.preferences_carried >= 2); // user + usessl at minimum

    // Side-car DB + seeded store exist; legacy DB untouched.
    assert!(report.side_car_db.exists());
    assert!(report.seeded_store.exists());
    assert!(legacy_home.join(".pclouddb").exists());

    // Vault carries the token bytes verbatim (still 0600).
    let vault_path = report.vault_path.clone().expect("vault populated");
    let body = fs::read_to_string(&vault_path).unwrap();
    assert_eq!(body.trim(), "legacy-token-abc");
    let mode = fs::metadata(&vault_path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "vault must be 0600");

    // Seeded store has the two sync roots.
    let store_conn = Connection::open(&report.seeded_store).unwrap();
    let mut stmt = store_conn
        .prepare("SELECT local_path, sync_type FROM sync_root_records ORDER BY sync_id")
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "/home/alice/Docs".to_string(),
                i64::from(SyncType::Full.as_u8())
            ),
            (
                "/home/alice/Photos".to_string(),
                i64::from(SyncType::DownloadOnly.as_u8()),
            ),
        ]
    );

    // `pass` must NOT have been carried into the preferences KV.
    // Look it up via the `setting` key-value table that `pcloud-store`
    // exposes; the schema has `setting(name, value_*)` rows — any value
    // containing the plaintext password would be a leak.
    let leak_check: i64 = store_conn
        .query_row(
            "SELECT COUNT(*) FROM setting WHERE name = 'pass'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    assert_eq!(leak_check, 0, "pass must never be persisted on Rust path");
}

#[test]
fn migrate_is_idempotent_without_force() {
    let tmp = TempDir::new().unwrap();
    let legacy_home = tmp.path().join("legacy");
    let target_config = tmp.path().join("cfg");
    let target_data = tmp.path().join("data");
    build_legacy_fixture(&legacy_home);

    // First run: succeeds.
    let plan = MigrationPlan::detect_with_targets(
        Some(legacy_home.clone()),
        false,
        false,
        Some(target_config.clone()),
        Some(target_data.clone()),
    )
    .unwrap()
    .unwrap();
    plan.execute().unwrap();

    // Second run without --force-overwrite: must refuse cleanly.
    let plan2 = MigrationPlan::detect_with_targets(
        Some(legacy_home.clone()),
        false,
        false,
        Some(target_config.clone()),
        Some(target_data.clone()),
    )
    .unwrap()
    .unwrap();
    let err = plan2.execute().expect_err("second run must refuse");
    match err {
        MigrateError::RustStateAlreadyPresent { .. } => {}
        other => panic!("expected RustStateAlreadyPresent, got {other:?}"),
    }

    // Third run WITH --force-overwrite: succeeds.
    let plan3 = MigrationPlan::detect_with_targets(
        Some(legacy_home),
        false,
        true,
        Some(target_config),
        Some(target_data),
    )
    .unwrap()
    .unwrap();
    plan3
        .execute()
        .expect("force-overwrite must succeed on populated target");
}

#[test]
fn detect_returns_none_when_no_legacy_db_present() {
    let tmp = TempDir::new().unwrap();
    let plan = MigrationPlan::detect_with_targets(
        Some(tmp.path().join("nothing-here")),
        true,
        false,
        Some(tmp.path().join("cfg")),
        Some(tmp.path().join("data")),
    )
    .unwrap();
    assert!(plan.is_none());
}

// Force-use PermissionsExt on Unix so `mode()` resolves in the test
// module above without an explicit `use` at item scope.
use std::os::unix::fs::PermissionsExt;

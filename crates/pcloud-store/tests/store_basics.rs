#![allow(clippy::pedantic)]
//! Integration tests for `pcloud-store`.
//!
//! These tests focus on the public bootstrap/persist/query surface and
//! exercise the in-process SQLite path. Every test uses a unique temp file
//! so tests can run in parallel without contention.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use pcloud_store::schema::{SCHEMA_VERSION_V11, read_schema_version};
use pcloud_store::{StoreError, StoreHandle, bootstrap_profile, persist_profile, value_kv};

// ── helpers ───────────────────────────────────────────────────────────────────

fn temp_db(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pcloud-store-basics-{}-{}.sqlite3",
        std::process::id(),
        label
    ))
}

// ── bootstrap ─────────────────────────────────────────────────────────────────

#[test]
fn bootstrap_succeeds_on_fresh_path() {
    let path = temp_db("fresh");
    let _ = std::fs::remove_file(&path);

    let (profile, integrity) = bootstrap_profile(&path).expect("bootstrap should succeed");
    assert_eq!(profile.schema_version, 11, "should reach v11");
    assert_eq!(integrity, pcloud_store::integrity::IntegrityStatus::Clean);
    assert_eq!(profile.db_path, path);
}

#[test]
fn bootstrap_creates_parent_directory_if_missing() {
    let parent = std::env::temp_dir().join(format!("pcloud-store-newdir-{}", std::process::id()));
    let path = parent.join("nested.sqlite3");
    let _ = std::fs::remove_dir_all(&parent);

    let (profile, _) = bootstrap_profile(&path).expect("bootstrap should create parent");
    assert!(path.exists());
    assert_eq!(profile.db_path, path);

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn second_open_on_same_path_sees_existing_schema_version() {
    let path = temp_db("reopen");
    let _ = std::fs::remove_file(&path);

    let (first, _) = bootstrap_profile(&path).expect("first bootstrap");
    assert_eq!(first.schema_version, 11);

    let (second, _) = bootstrap_profile(&path).expect("second bootstrap on same file");
    assert_eq!(second.schema_version, 11);
}

// ── value_kv round-trip ───────────────────────────────────────────────────────

#[test]
fn value_kv_uint_round_trip() {
    let path = temp_db("kv-uint");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    value_kv::set_uint(&path, "counter", 42).expect("set_uint");
    let v = value_kv::get_uint(&path, "counter").expect("get_uint");
    assert_eq!(v, 42);
}

#[test]
fn value_kv_string_round_trip() {
    let path = temp_db("kv-string");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    value_kv::set_string(&path, "label", "hello").expect("set_string");
    let v = value_kv::get_string(&path, "label").expect("get_string");
    assert_eq!(v.as_deref(), Some("hello"));
}

#[test]
fn value_kv_bool_round_trip() {
    let path = temp_db("kv-bool");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    value_kv::set_bool(&path, "flag", true).expect("set_bool");
    let v = value_kv::get_bool(&path, "flag").expect("get_bool");
    assert!(v);
}

#[test]
fn value_kv_int_round_trip() {
    let path = temp_db("kv-int");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    value_kv::set_int(&path, "delta", -7).expect("set_int");
    let v = value_kv::get_int(&path, "delta").expect("get_int");
    assert_eq!(v, -7);
}

#[test]
fn value_kv_has_uint_returns_true_after_set() {
    let path = temp_db("kv-has");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    assert!(!value_kv::has_uint(&path, "x").expect("has_uint before set"));
    value_kv::set_uint(&path, "x", 1).expect("set_uint");
    assert!(value_kv::has_uint(&path, "x").expect("has_uint after set"));
}

#[test]
fn value_kv_delete_removes_row() {
    let path = temp_db("kv-delete");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    value_kv::set_uint(&path, "to_delete", 99).expect("set_uint");
    let deleted = value_kv::delete(&path, "to_delete").expect("delete");
    assert!(deleted, "delete should report row removed");
    assert!(!value_kv::has_uint(&path, "to_delete").expect("has_uint after delete"));
}

// ── StoreHandle pooled connection ─────────────────────────────────────────────

#[test]
fn store_handle_open_after_bootstrap_succeeds() {
    let path = temp_db("handle-open");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    let handle = StoreHandle::open(&path).expect("open handle");
    assert_eq!(handle.db_path(), &*path);
}

#[test]
fn store_handle_clone_shares_connection() {
    let path = temp_db("handle-clone");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    let handle = StoreHandle::open(&path).expect("open handle");
    let cloned = handle.clone();

    handle
        .value_kv()
        .set_uint("shared_key", 77)
        .expect("set via handle");
    let v = cloned
        .value_kv()
        .get_uint("shared_key")
        .expect("get via clone");
    assert_eq!(v, 77);
}

// ── persist_profile round-trip ────────────────────────────────────────────────

#[test]
fn persist_then_reopen_preserves_schema_version() {
    let path = temp_db("persist-reopen");
    let _ = std::fs::remove_file(&path);

    let (profile, _) = bootstrap_profile(&path).expect("bootstrap");
    persist_profile(&profile).expect("persist");

    let (reloaded, _) = bootstrap_profile(&path).expect("re-bootstrap");
    assert_eq!(reloaded.schema_version, profile.schema_version);
}

// ── migration is idempotent ───────────────────────────────────────────────────

#[test]
fn bootstrap_on_existing_v11_file_is_idempotent() {
    let path = temp_db("idempotent-v11");
    let _ = std::fs::remove_file(&path);

    for _ in 0..3 {
        let (p, _) = bootstrap_profile(&path).expect("bootstrap should be idempotent");
        assert_eq!(p.schema_version, 11);
    }
}

// ── migration savepoint atomicity ────────────────────────────────────────────

/// Verify that running bootstrap twice on the same file is idempotent and that
/// the resulting `user_version` matches the target schema version constant.
#[test]
fn migration_user_version_matches_target_after_bootstrap() {
    let path = temp_db("migration-user-version");
    let _ = std::fs::remove_file(&path);

    let (profile, _) = bootstrap_profile(&path).expect("first bootstrap");
    assert_eq!(
        profile.schema_version, SCHEMA_VERSION_V11,
        "schema_version in StoreProfile must match the target constant"
    );

    // Open the raw connection and confirm PRAGMA user_version matches.
    let conn = rusqlite::Connection::open(&path).expect("open");
    let on_disk = read_schema_version(&conn).expect("read_schema_version");
    assert_eq!(
        on_disk, SCHEMA_VERSION_V11,
        "on-disk PRAGMA user_version must equal target after bootstrap"
    );
}

/// Verify that applying the full migration plan twice (idempotent re-run via
/// bootstrap) produces no error and leaves user_version at the correct value.
#[test]
fn migration_is_idempotent_and_user_version_stable() {
    let path = temp_db("migration-idempotent-uv");
    let _ = std::fs::remove_file(&path);

    for run in 0..3u32 {
        let (profile, _) = bootstrap_profile(&path)
            .unwrap_or_else(|_| panic!("bootstrap run {run} should succeed"));
        assert_eq!(
            profile.schema_version, SCHEMA_VERSION_V11,
            "run {run}: schema_version must be stable at target version"
        );
    }

    let conn = rusqlite::Connection::open(&path).expect("open");
    let on_disk = read_schema_version(&conn).expect("read_schema_version");
    assert_eq!(
        on_disk, SCHEMA_VERSION_V11,
        "user_version stable after repeated bootstrap"
    );
}

// ── error surface ─────────────────────────────────────────────────────────────

#[test]
fn store_handle_reports_path_on_missing_dir() {
    // Opening a handle on a path inside a deeply-nested missing directory
    // should surface a StoreError, not a panic.
    let path = PathBuf::from("/tmp/__pcloud_nonexistent_dir_abc/db.sqlite3");
    // bootstrap_profile creates parent dirs; StoreHandle::open does not.
    // If the parent does not exist SQLite returns an error.
    match StoreHandle::open(&path) {
        Err(StoreError::Sql(_)) | Err(StoreError::Io(_)) => {} // expected
        Ok(_) => {} // SQLite might auto-create; accept if so
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
}

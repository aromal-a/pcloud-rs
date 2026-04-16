#![allow(clippy::pedantic)]
//! Shared integration smoke tests for the per-backend `mock` submodules
//! promoted from the `pcloud-fs` mock-backend pattern (R18 wave-01
//! audit ask).
//!
//! Each test instantiates the corresponding backend's `mock::Fixture`,
//! drives the representative protocol call, and asserts that the
//! shared [`pcloud_backends::mock::MockProto`] recorder captured the
//! expected command name and payload marker. These tests intentionally
//! do not exercise live transports, stores, or audit sinks — they
//! prove the shared recording primitives are wired into every
//! backend's `mock` submodule.

use pcloud_backends::{
    account_backend, auth_backend, backup_backend, crypto_backend, folder_backend,
    notifications_backend, public_link_backend, shares_backend, sync_backend, transfer_backend,
};

fn assert_representative<F>(fixture: F, expected: &str)
where
    F: FnOnce() -> (
        pcloud_backends::mock::MockEvent,
        Vec<pcloud_backends::mock::MockEvent>,
    ),
{
    let (recorded, proto_events) = fixture();
    assert_eq!(recorded.category, "proto");
    assert_eq!(recorded.name, expected);
    assert_eq!(recorded.payload.as_deref(), Some("mock"));
    assert_eq!(proto_events.len(), 1);
    assert_eq!(proto_events[0], recorded);
}

#[test]
fn auth_backend_mock_records_userinfo() {
    assert_representative(
        || {
            let fx = auth_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        auth_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn account_backend_mock_records_setlanguage() {
    assert_representative(
        || {
            let fx = account_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        account_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn sync_backend_mock_records_listfolder() {
    assert_representative(
        || {
            let fx = sync_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        sync_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn transfer_backend_mock_records_getfilelink() {
    assert_representative(
        || {
            let fx = transfer_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        transfer_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn shares_backend_mock_records_listshares() {
    assert_representative(
        || {
            let fx = shares_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        shares_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn public_link_backend_mock_records_listpublinks() {
    assert_representative(
        || {
            let fx = public_link_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        public_link_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn crypto_backend_mock_records_getuserkeys() {
    assert_representative(
        || {
            let fx = crypto_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        crypto_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn folder_backend_mock_records_listfolder() {
    assert_representative(
        || {
            let fx = folder_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        folder_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn backup_backend_mock_records_backup_list() {
    assert_representative(
        || {
            let fx = backup_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        backup_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

#[test]
fn notifications_backend_mock_records_listnotifications() {
    assert_representative(
        || {
            let fx = notifications_backend::mock::Fixture::new();
            let rec = fx.record_representative_call();
            (rec, fx.fixture.proto.records())
        },
        notifications_backend::mock::REPRESENTATIVE_COMMAND,
    );
}

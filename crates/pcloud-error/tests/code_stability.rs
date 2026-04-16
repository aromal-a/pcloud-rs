#![allow(clippy::pedantic)]
//! Snapshot test for stable numeric error codes.
//!
//! If this test fails, **do not blindly update the expected table**. A code
//! change is a breaking change for any script consuming pcloud-rs CLI exit
//! codes / IPC status. Bump a major version and update
//! `ERROR-TAXONOMY.md` instead.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_error::{Category, Error};

fn sample(cat: Category) -> Error {
    cat.build("sample")
}

#[test]
fn numeric_codes_snapshot() {
    let table: [(Category, u32, &str); 15] = [
        (Category::Auth, 1000, "auth"),
        (Category::Permission, 1100, "permission"),
        (Category::Api, 1200, "api"),
        (Category::Transport, 1300, "transport"),
        (Category::Ipc, 1400, "ipc"),
        (Category::Protocol, 1500, "protocol"),
        (Category::Crypto, 1600, "crypto"),
        (Category::Storage, 1700, "storage"),
        (Category::Config, 1800, "config"),
        (Category::LocalIo, 1900, "local_io"),
        (Category::NotFound, 2000, "not_found"),
        (Category::InvalidInput, 2100, "invalid_input"),
        (Category::Busy, 2200, "busy"),
        (Category::Plugin, 2300, "plugin"),
        (Category::Internal, 9000, "internal"),
    ];
    for (cat, expected_code, expected_slug) in table {
        let err = sample(cat);
        assert_eq!(
            err.code(),
            expected_code,
            "numeric code for {cat} drifted; update ERROR-TAXONOMY.md deliberately"
        );
        assert_eq!(err.category(), expected_slug);
        assert_eq!(cat.to_string(), expected_slug);
    }
}

#[test]
fn roundtrip_from_std_io_is_local_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let unified: Error = io_err.into();
    assert_eq!(unified.code(), 1900);
    assert_eq!(unified.category(), "local_io");
}

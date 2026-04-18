#![allow(clippy::pedantic)]
//! Live stub: backup create / delete / stop_device lifecycle.
//!
//! Verifies that the backup-backend IPC surface (create backup, list,
//! stop device, delete) round-trips against a real pCloud account without
//! leaving residue.
//!
//! **Status:** stub — body is `todo!()` pending fixture design.  The
//! backup APIs require a real device registration that ties to the
//! hardware identity; creating and tearing down a device id in CI without
//! polluting a personal account needs a dedicated CI-only account and a
//! cleanup contract.
//!
//! Tracking: bd-1du.10 / pcloud-rs-s1p.57 / audit-04 P2-8.
//!
//! Gate: `PCLOUD_LIVE_E2E=1` + valid credentials.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use crate::common::{ENV_PASSWORD, ENV_TOKEN, ENV_USER, skip_if_not_live};

/// Live end-to-end: create a backup entry, list it, stop the device, and
/// delete the backup.
///
/// TODO(bd-1du.10): Implement once a CI-only pCloud account with backup
/// permissions is available and cleanup semantics are defined.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials; body is todo!() — CI account fixture not yet available"]
fn live_backup_create_delete() {
    if skip_if_not_live(&[]) {
        return;
    }
    let _has_creds = common::optional_env(ENV_TOKEN).is_some()
        || (common::optional_env(ENV_USER).is_some()
            && common::optional_env(ENV_PASSWORD).is_some());

    // TODO(bd-1du.10): Replace todo!() with real test body:
    //   1. Authenticate.
    //   2. Dispatch `Request::BackupCreate { ... }` with a unique device name.
    //   3. Assert ResponseStatus::Ok.
    //   4. Dispatch `Request::BackupList` and assert the new backup appears.
    //   5. Dispatch `Request::StopDevice { ... }` for the device.
    //   6. Dispatch `Request::BackupDelete { ... }` and assert Ok.
    //   7. Assert backup no longer appears in `Request::BackupList`.
    todo!("CI account fixture not yet defined — see bd-1du.10 and pcloud-rs-s1p.57")
}

/// Live end-to-end: stop device stops the associated backup without
/// deleting local sync roots (intentional non-parity with C client).
///
/// TODO(bd-1du.10): Implement alongside `live_backup_create_delete`.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials; body is todo!()"]
fn live_stop_device() {
    if skip_if_not_live(&[]) {
        return;
    }

    // TODO(bd-1du.10): Replace todo!() with real test body once
    // `live_backup_create_delete` fixture is established.
    todo!("CI account fixture not yet defined — see bd-1du.10 and pcloud-rs-s1p.57")
}

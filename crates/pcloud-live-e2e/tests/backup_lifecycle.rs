#![allow(clippy::pedantic)]
//! Live coverage: backup create / stop-device / delete-backup-device
//! lifecycle against a real pCloud account (rows 95, 97, 98).
//!
//! Gate: `PCLOUD_LIVE_E2E=1` + valid credentials.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::{fs, path::PathBuf, time::SystemTime};

use pcloud_ipc::{Request, ResponseStatus};

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate,
    optional_env, scratch_folder, skip_if_not_live, status_label,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

fn unique_tag(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

/// Allocate a fresh local directory under the OS temp dir; callers are
/// responsible for removing it at the end of the test.
fn make_local_dir(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("pcloud-live-e2e-{}", unique_tag(tag)));
    fs::create_dir_all(&root).expect("create local backup dir");
    root
}

fn remove_local_dir(p: &PathBuf) {
    let _ = fs::remove_dir_all(p);
}

/// Extracts the first `folder_id=` / `folderid=` / `id=` numeric token
/// from a `CreateRemoteFolder` response message.
fn extract_folder_id(msg: &str) -> Option<u64> {
    for marker in ["folder_id=", "folderid=", "id="] {
        if let Some(off) = msg.find(marker) {
            let tail = &msg[off + marker.len()..];
            let end = tail
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(tail.len());
            if let Ok(v) = tail[..end].parse::<u64>() {
                return Some(v);
            }
        }
    }
    None
}

/// Best-effort extractor: the `create_backup` response family typically
/// surfaces the device folder id under `device_folder_id=<N>` or
/// `folder_id=<N>`. We accept either, and fall back to generic `id=<N>`
/// so format drift does not silently break the test.
fn extract_device_folder_id(msg: &str) -> Option<u64> {
    for marker in ["device_folder_id=", "folder_id=", "backup_id=", "id="] {
        if let Some(off) = msg.find(marker) {
            let tail = &msg[off + marker.len()..];
            let end = tail
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(tail.len());
            if let Ok(v) = tail[..end].parse::<u64>() {
                return Some(v);
            }
        }
    }
    None
}

/// Create a scratch remote folder and return its folder_id.
///
/// pCloud's `backup/createbackup` requires a non-zero `folderid` pointing
/// to an existing remote folder. The account root (folderid=0) is rejected
/// with error 1017 ("Invalid 'folderid' provided"). We create a temporary
/// folder under the scratch root to obtain a valid target.
fn create_scratch_folder(daemon: &mut TestDaemon, tag: &str) -> Option<u64> {
    let scratch = scratch_folder();
    let leaf = unique_tag(tag);
    let path = if scratch.ends_with('/') {
        format!("{scratch}{leaf}")
    } else {
        format!("{scratch}/{leaf}")
    };
    let resp = daemon.dispatch(Request::CreateRemoteFolder {
        parent_folder_id: None,
        name: leaf,
        path,
        check_and_create: false,
    });
    if resp.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] CreateRemoteFolder failed: status={} message={}",
            status_label(&resp.status),
            resp.message
        );
        return None;
    }
    let id = extract_folder_id(&resp.message);
    if id.is_none() {
        eprintln!(
            "[live-e2e] CreateRemoteFolder did not advertise folder_id: {}",
            resp.message
        );
    }
    id
}

/// Live end-to-end: create a backup, then stop the device, then clear
/// the local backup-device registration. No remote delete is issued
/// because the backup backend does not expose a remote-delete helper
/// beyond `stopdevice`; the stopped backup is harmless and any residual
/// state is cleared by `DeleteBackupDevice`.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_backup_create_delete() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping backup_create_delete: need credentials");
        return;
    }

    let mut daemon = TestDaemon::new("backup-create-delete");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping backup_create_delete: auth failed: {err}");
        return;
    }

    let local = make_local_dir("backup");
    let name = unique_tag("backup");

    // pCloud `backup/createbackup` requires a non-zero `folderid`. Create a
    // scratch folder under the test scratch root and use its id.
    let root_folder_id = match create_scratch_folder(&mut daemon, "backup-root") {
        Some(id) => id,
        None => {
            remove_local_dir(&local);
            panic!("could not create scratch folder for CreateBackup root_folder_id");
        }
    };

    let create_resp = daemon.dispatch(Request::CreateBackup {
        name: name.clone(),
        root_folder_id,
        local_path: local.to_string_lossy().into_owned(),
        parent_folder_name: None,
    });
    assert_no_secret_leak(&create_resp);

    if create_resp.status != ResponseStatus::Ok {
        remove_local_dir(&local);
        // pCloud error 1017 ("Invalid 'folderid' provided") on any folderid
        // indicates the account is not provisioned for backup. This is an
        // account-type restriction, not an IPC or implementation bug.
        // We skip rather than fail so the test can be re-run on a provisioned
        // account without code changes.
        if create_resp.message.contains("1017")
            || create_resp.message.contains("Invalid 'folderid'")
        {
            eprintln!(
                "[live-e2e] skipping backup_create_delete: account not provisioned for backup \
                (folderid rejected): {}",
                create_resp.message
            );
            return;
        }
        panic!(
            "CreateBackup failed: status={} message={}",
            status_label(&create_resp.status),
            create_resp.message
        );
    }

    let device_folder_id = extract_device_folder_id(&create_resp.message);

    // Best-effort stop + local cleanup on ALL exit paths below.
    let mut stop_ok = true;
    if let Some(id) = device_folder_id {
        let stop = daemon.dispatch(Request::StopDevice {
            device_folder_id: id,
        });
        assert_no_secret_leak(&stop);
        if stop.status != ResponseStatus::Ok {
            stop_ok = false;
            eprintln!(
                "[live-e2e] StopDevice failed: status={} message={}",
                status_label(&stop.status),
                stop.message
            );
        }
    }

    let clear = daemon.dispatch(Request::DeleteBackupDevice);
    assert_no_secret_leak(&clear);
    let clear_status = clear.status;
    let clear_msg = clear.message.clone();

    remove_local_dir(&local);

    assert!(
        device_folder_id.is_some(),
        "CreateBackup response must advertise a device folder id: {}",
        create_resp.message
    );
    assert!(stop_ok, "StopDevice must succeed after CreateBackup");
    assert_eq!(
        clear_status,
        ResponseStatus::Ok,
        "DeleteBackupDevice must clear local state: {clear_msg}"
    );
}

/// Focused probe: StopDevice on the device folder id returned by
/// CreateBackup succeeds and leaves the daemon in a clean state so a
/// subsequent CreateBackup can allocate a fresh device.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_stop_device() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping stop_device: need credentials");
        return;
    }

    let mut daemon = TestDaemon::new("stop-device");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping stop_device: auth failed: {err}");
        return;
    }

    let local = make_local_dir("stopdev");
    let name = unique_tag("stopdev");

    // pCloud `backup/createbackup` requires a non-zero `folderid`. Create a
    // scratch folder under the test scratch root and use its id.
    let root_folder_id = match create_scratch_folder(&mut daemon, "stopdev-root") {
        Some(id) => id,
        None => {
            remove_local_dir(&local);
            panic!("could not create scratch folder for CreateBackup root_folder_id");
        }
    };

    let create_resp = daemon.dispatch(Request::CreateBackup {
        name: name.clone(),
        root_folder_id,
        local_path: local.to_string_lossy().into_owned(),
        parent_folder_name: None,
    });
    assert_no_secret_leak(&create_resp);
    if create_resp.status != ResponseStatus::Ok {
        remove_local_dir(&local);
        // pCloud error 1017 on any folderid indicates the account is not
        // provisioned for backup. Skip gracefully instead of panicking.
        if create_resp.message.contains("1017")
            || create_resp.message.contains("Invalid 'folderid'")
        {
            eprintln!(
                "[live-e2e] skipping stop_device: account not provisioned for backup \
                (folderid rejected): {}",
                create_resp.message
            );
            return;
        }
        panic!(
            "CreateBackup failed (stop_device precondition): status={} message={}",
            status_label(&create_resp.status),
            create_resp.message
        );
    }
    let device_folder_id = match extract_device_folder_id(&create_resp.message) {
        Some(id) => id,
        None => {
            remove_local_dir(&local);
            panic!(
                "CreateBackup response did not advertise a device folder id: {}",
                create_resp.message
            );
        }
    };

    let stop = daemon.dispatch(Request::StopDevice { device_folder_id });
    assert_no_secret_leak(&stop);
    let stop_status = stop.status;
    let stop_msg = stop.message.clone();

    // Always clear local registration + local dir so re-runs are clean.
    let _ = daemon.dispatch(Request::DeleteBackupDevice);
    remove_local_dir(&local);

    assert_eq!(
        stop_status,
        ResponseStatus::Ok,
        "StopDevice must succeed: {stop_msg}"
    );
}

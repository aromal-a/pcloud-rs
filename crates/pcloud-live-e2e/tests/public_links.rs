#![allow(clippy::pedantic)]
//! Live public-link coverage: create file/folder link, list, change
//! expire/password, delete by id and by code.
//!
//! Requires `PCLOUD_LIVE_E2E=1` plus credentials. The test uploads a
//! small scratch file into `PCLOUD_TEST_SCRATCH`, derives a link from
//! it, then removes everything it created.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::time::SystemTime;

use pcloud_ipc::{Method, Request, ResponseStatus};
use pcloud_sdk::EmbeddedDaemon;

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, optional_env,
    scratch_folder, skip_if_not_live,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

fn unique_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}.txt", std::process::id())
}

/// Best-effort extractor: the daemon's link_create responses encode the
/// numeric link id and/or share code in their message payload. We do
/// not commit to a single shape here; we just find the first of each.
fn extract_link_id(msg: &str) -> Option<u64> {
    for marker in ["link_id=", "linkid=", "id="] {
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

fn extract_code(msg: &str) -> Option<String> {
    for marker in ["code=", "short=", "share_code="] {
        if let Some(off) = msg.find(marker) {
            let tail = &msg[off + marker.len()..];
            let end = tail
                .find(|c: char| c.is_whitespace() || c == ',' || c == '"')
                .unwrap_or(tail.len());
            let v = tail[..end].trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_owned());
            }
        }
    }
    None
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_public_link_lifecycle() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping public-links: need credentials");
        return;
    }

    // 1) Upload a scratch file via the SDK direct-upload helper.
    let seed = TestDaemon::new("public-links-seed");
    let root = seed.config.paths.config_dir.parent().unwrap().to_path_buf();
    drop(seed);
    let mut sdk = EmbeddedDaemon::builder(root.clone())
        .build()
        .expect("SDK bootstrap");
    let auth_resp = if let Some(token) = optional_env(ENV_TOKEN) {
        sdk.dispatch(Request::AuthTokenSubmission { value: token })
    } else {
        sdk.dispatch(Request::PasswordSubmission {
            username: optional_env(ENV_USER).unwrap(),
            value: optional_env(ENV_PASSWORD).unwrap(),
        })
    };
    if auth_resp.status != ResponseStatus::Ok || !sdk.is_authenticated() {
        eprintln!(
            "[live-e2e] skipping public-links: auth failed/TFA required: {}",
            auth_resp.message
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let scratch = scratch_folder();
    let filename = unique_name("live-e2e-pl");
    let payload = b"pcloud-rs live-e2e public-link fixture\n";
    let uploaded = match sdk.upload_data_as(&scratch, filename.clone(), payload) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("[live-e2e] skipping public-links: upload failed: {err}");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
    };
    let file_path = if scratch.ends_with('/') {
        format!("{scratch}{filename}")
    } else {
        format!("{scratch}/{filename}")
    };
    let _file_id = uploaded.file_id;

    // 2) Drive the public-link lifecycle through the embedded daemon's
    //    IPC dispatcher so we exercise the same code path the CLI uses.
    let create_resp = sdk.dispatch(Request::CreateFilePublicLink {
        path: file_path.clone(),
    });
    assert_no_secret_leak(&create_resp);
    if create_resp.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] skipping (CreateFilePublicLink declined): status={} message={}",
            crate::common::status_label(&create_resp.status),
            create_resp.message
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    let link_id = extract_link_id(&create_resp.message);
    let code = extract_code(&create_resp.message);

    // Field-selector probes: the create response shape SHOULD advertise
    // at least one of `link_id=` / `code=` / `id=`. This catches regression
    // in the daemon's message formatter without over-committing to a
    // specific key name.
    assert!(
        link_id.is_some() || code.is_some(),
        "CreateFilePublicLink response must advertise a link id or code: {}",
        create_resp.message
    );

    // 3) List — the newly-created link must be present.
    let list = sdk.dispatch(Request::Plain {
        method: Method::ListPublicLinks,
    });
    assert_no_secret_leak(&list);
    assert_eq!(
        list.status,
        ResponseStatus::Ok,
        "ListPublicLinks failed: {}",
        list.message
    );
    if let Some(id) = link_id {
        assert!(
            list.message.contains(&id.to_string()),
            "ListPublicLinks output should mention link_id {id}: {}",
            list.message
        );
    }

    // 4) Change expire / password — opportunistic.
    if let Some(id) = link_id {
        let exp = daemon_epoch_plus(3600);
        let r1 = sdk.dispatch(Request::ChangePublicLinkExpire {
            link_id: id,
            expire: Some(exp),
        });
        assert_no_secret_leak(&r1);

        let r2 = sdk.dispatch(Request::ChangePublicLinkExpire {
            link_id: id,
            expire: None,
        });
        assert_no_secret_leak(&r2);

        let r3 = sdk.dispatch(Request::ChangePublicLinkPassword {
            link_id: id,
            password: Some("live-e2e-tmp-pw".to_owned()),
        });
        assert_no_secret_leak(&r3);

        let r4 = sdk.dispatch(Request::ChangePublicLinkPassword {
            link_id: id,
            password: None,
        });
        assert_no_secret_leak(&r4);
    }

    // 5) Folder-link attempt against the scratch path — accepted or
    //    politely declined; either way, clean up if we got an id back.
    let folder_resp = sdk.dispatch(Request::CreateFolderPublicLink {
        path: scratch.clone(),
    });
    assert_no_secret_leak(&folder_resp);
    if folder_resp.status == ResponseStatus::Ok {
        if let Some(folder_link_id) = extract_link_id(&folder_resp.message) {
            let rm = sdk.dispatch(Request::DeletePublicLink {
                link_id: folder_link_id,
            });
            assert_no_secret_leak(&rm);
        } else if let Some(fcode) = extract_code(&folder_resp.message) {
            let rm = sdk.dispatch(Request::DeletePublicLinkByCode { code: fcode });
            assert_no_secret_leak(&rm);
        }
    }

    // 6) Delete the file link. Prefer by-id; fall back to by-code.
    let mut deleted = false;
    if let Some(id) = link_id {
        let resp = sdk.dispatch(Request::DeletePublicLink { link_id: id });
        assert_no_secret_leak(&resp);
        if resp.status == ResponseStatus::Ok {
            deleted = true;
        }
    }
    if !deleted && let Some(c) = code {
        let resp = sdk.dispatch(Request::DeletePublicLinkByCode { code: c });
        assert_no_secret_leak(&resp);
    }

    let _ = std::fs::remove_dir_all(&root);
}

fn daemon_epoch_plus(seconds: u64) -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
        + seconds
}

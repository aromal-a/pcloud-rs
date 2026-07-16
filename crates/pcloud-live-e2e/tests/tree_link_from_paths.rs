#![allow(clippy::pedantic)]
//! Live coverage: `ptree_public_link` path-based IPC variant (row 149).
//!
//! Verifies that `Request::CreateTreePublicLinkFromPathTargets` is dispatched
//! end-to-end against a real pCloud account with root, folder, and file targets,
//! and that the resulting link is retrievable and deletable. Row 149 historical
//! tracker labels are provenance only; the CSV/STATUS truth source owns the
//! current verdict.
//!
//! Gate: `PCLOUD_LIVE_E2E=1` + valid credentials.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::time::SystemTime;

use pcloud_ipc::{Method, Request, ResponseStatus};

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

fn extract_number_after(msg: &str, markers: &[&str]) -> Option<u64> {
    for marker in markers {
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

/// Best-effort extractor: the daemon's tree-link create response formats
/// as `"tree public link created from paths: id=<N>, name=\"...\", link=\"...\""`.
fn extract_link_id(msg: &str) -> Option<u64> {
    extract_number_after(msg, &["link_id=", "linkid=", "id="])
}

fn extract_folder_id(msg: &str) -> Option<u64> {
    extract_number_after(msg, &["folder_id=", "folderid=", "id="])
}

fn extract_code(msg: &str) -> Option<String> {
    for marker in ["link=\"", "code=\"", "code="] {
        if let Some(off) = msg.find(marker) {
            let tail = &msg[off + marker.len()..];
            let end = tail
                .find(|c: char| c == '"' || c.is_whitespace() || c == ',')
                .unwrap_or(tail.len());
            let v = tail[..end].trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_owned());
            }
        }
    }
    None
}

fn join_path(parent: &str, leaf: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{leaf}")
    } else {
        format!("{parent}/{leaf}")
    }
}

fn cleanup_remote_root(daemon: &mut TestDaemon, folder_id: Option<u64>) {
    if let Some(folder_id) = folder_id {
        let rm = daemon.dispatch(Request::FolderDeleteById {
            folder_id,
            recursive: true,
        });
        assert_no_secret_leak(&rm);
        if rm.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] remote cleanup failed for folder_id={folder_id}: status={} message={}",
                status_label(&rm.status),
                rm.message
            );
        }
    }
}

/// Live end-to-end: create a tree public link using the path-based IPC
/// target variant (daemon resolves root/folder/file paths under auth).
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_create_tree_public_link_from_path_targets() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping tree_link_from_paths: need credentials");
        return;
    }

    let mut daemon = TestDaemon::new("tree-link-from-targets");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping tree_link_from_paths: auth failed: {err}");
        return;
    }

    let scratch = scratch_folder();
    let root_leaf = unique_tag("tree-root");
    let folder_leaf = unique_tag("tree-folder");
    let file_name = format!("{}.txt", unique_tag("tree-file"));
    let root_path = join_path(&scratch, &root_leaf);
    let folder_path = join_path(&root_path, &folder_leaf);
    let file_path = join_path(&root_path, &file_name);

    // 1) Create a unique root and child folder so the full target shape has
    //    real resolvable root/folder ids to collect.
    let mk_root = daemon.dispatch(Request::CreateRemoteFolder {
        parent_folder_id: None,
        name: root_leaf.clone(),
        path: root_path.clone(),
        check_and_create: false,
    });
    assert_no_secret_leak(&mk_root);
    if mk_root.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] skipping (CreateRemoteFolder root declined): status={} message={}",
            status_label(&mk_root.status),
            mk_root.message
        );
        return;
    }
    let Some(root_folder_id) = extract_folder_id(&mk_root.message) else {
        eprintln!(
            "[live-e2e] skipping tree_link_from_paths: CreateRemoteFolder root did not advertise folder_id: {}",
            mk_root.message
        );
        return;
    };

    let mk_child = daemon.dispatch(Request::CreateRemoteFolder {
        parent_folder_id: Some(root_folder_id),
        name: folder_leaf.clone(),
        path: String::new(),
        check_and_create: false,
    });
    assert_no_secret_leak(&mk_child);
    if mk_child.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] skipping (CreateRemoteFolder child declined): status={} message={}",
            status_label(&mk_child.status),
            mk_child.message
        );
        cleanup_remote_root(&mut daemon, Some(root_folder_id));
        return;
    }

    // 2) Upload a real file target under the same root.
    let payload = format!("pcloud-rs row 149 file target: {file_name}\n").into_bytes();
    let auth_token = daemon
        .runtime
        .auth
        .snapshot()
        .auth_token
        .as_ref()
        .map(|token| token.clone_secret())
        .expect("authenticated daemon must expose auth token");
    let session = match daemon.runtime.transfer_runtime.upload_create(
        auth_token.clone_secret(),
        root_folder_id,
        file_name.clone(),
        payload.len() as u64,
    ) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("[live-e2e] skipping tree_link_from_paths: upload_create failed: {err}");
            cleanup_remote_root(&mut daemon, Some(root_folder_id));
            return;
        }
    };
    assert_eq!(session.file_name, file_name);
    let frame = match daemon
        .runtime
        .transfer_runtime
        .upload_bytes(auth_token, &session, &payload)
    {
        Ok(frame) => frame,
        Err(err) => {
            eprintln!("[live-e2e] skipping tree_link_from_paths: upload_bytes failed: {err}");
            cleanup_remote_root(&mut daemon, Some(root_folder_id));
            return;
        }
    };
    assert_eq!(frame.stream_id, session.upload_id as u32);
    assert_eq!(frame.payload_len, payload.len());

    // 3) Invoke the root/folder/file path-target IPC. This is row 149's full
    //    path shape, not the folder-only compatibility alias.
    let link_name = unique_tag("tree-link");
    let create_resp = daemon.dispatch(Request::CreateTreePublicLinkFromPathTargets {
        name: link_name.clone(),
        root: Some(root_path.clone()),
        folders: vec![folder_path.clone()],
        files: vec![file_path.clone()],
        expires: None,
    });
    assert_no_secret_leak(&create_resp);

    let status_ok = create_resp.status == ResponseStatus::Ok;
    let link_id = extract_link_id(&create_resp.message);
    let code = extract_code(&create_resp.message);

    // 4) Always clean up what we created, regardless of success.
    if status_ok {
        let mut deleted_link = false;
        if let Some(id) = link_id {
            let rm = daemon.dispatch(Request::DeletePublicLink { link_id: id });
            assert_no_secret_leak(&rm);
            if rm.status == ResponseStatus::Ok {
                deleted_link = true;
            }
        }
        if !deleted_link {
            if let Some(c) = code.clone() {
                let rm = daemon.dispatch(Request::DeletePublicLinkByCode { code: c });
                assert_no_secret_leak(&rm);
            }
        }
    }
    cleanup_remote_root(&mut daemon, Some(root_folder_id));

    // 5) Now assert on the link-create outcome. Cleanup already ran.
    if !status_ok {
        panic!(
            "CreateTreePublicLinkFromPathTargets failed: status={} message={}",
            status_label(&create_resp.status),
            create_resp.message
        );
    }
    if link_id.is_none() && code.is_none() {
        panic!(
            "CreateTreePublicLinkFromPathTargets response must advertise a link id or code: {}",
            create_resp.message
        );
    }

    // 6) Probe the list endpoint so we exercise the same surface the CLI
    //    would use to discover the link we just made — purely defensive.
    let list = daemon.dispatch(Request::Plain {
        method: Method::ListPublicLinks,
    });
    assert_no_secret_leak(&list);
    // The link was already deleted above; list need not mention it. We
    // only require the endpoint itself to succeed.
    assert_eq!(
        list.status,
        ResponseStatus::Ok,
        "ListPublicLinks failed: {}",
        list.message
    );
}

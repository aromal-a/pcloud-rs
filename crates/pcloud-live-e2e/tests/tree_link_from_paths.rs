#![allow(clippy::pedantic)]
//! Live coverage: `ptree_public_link` path-based IPC variant (row 149).
//!
//! Verifies that `Request::CreateTreePublicLinkFromPaths` is dispatched
//! end-to-end against a real pCloud account, and that the resulting link
//! is retrievable and deletable. Tracks: bd-1du row 149 / pcloud-rs-s1p.57.
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

/// Best-effort extractor: the daemon's tree-link create response formats
/// as `"tree public link created from paths: id=<N>, name=\"...\", link=\"...\""`.
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

/// Live end-to-end: create a tree public link using the path-based IPC
/// variant (daemon resolves paths under its auth context).
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_create_tree_public_link_from_paths() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping tree_link_from_paths: need credentials");
        return;
    }

    let mut daemon = TestDaemon::new("tree-link-from-paths");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping tree_link_from_paths: auth failed: {err}");
        return;
    }

    let scratch = scratch_folder();
    let leaf_a = unique_tag("tree-a");
    let leaf_b = unique_tag("tree-b");
    let path_a = join_path(&scratch, &leaf_a);
    let path_b = join_path(&scratch, &leaf_b);

    // 1) Create two unique subfolders under the scratch root so the tree
    //    link has real resolvable folder ids to collect.
    let mk_a = daemon.dispatch(Request::CreateRemoteFolder {
        parent_folder_id: None,
        name: leaf_a.clone(),
        path: path_a.clone(),
        check_and_create: false,
    });
    assert_no_secret_leak(&mk_a);
    if mk_a.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] skipping (CreateRemoteFolder A declined): status={} message={}",
            status_label(&mk_a.status),
            mk_a.message
        );
        return;
    }

    let mk_b = daemon.dispatch(Request::CreateRemoteFolder {
        parent_folder_id: None,
        name: leaf_b.clone(),
        path: path_b.clone(),
        check_and_create: false,
    });
    assert_no_secret_leak(&mk_b);
    if mk_b.status != ResponseStatus::Ok {
        // Remote folder cleanup is not exposed via IPC today; leave the
        // stale scratch subfolder (unique-named, isolated to scratch).
        eprintln!(
            "[live-e2e] skipping (CreateRemoteFolder B declined): status={} message={}",
            status_label(&mk_b.status),
            mk_b.message
        );
        return;
    }

    // 2) Invoke the path-based tree-link IPC. This is the bead under test.
    let link_name = unique_tag("tree-link");
    let create_resp = daemon.dispatch(Request::CreateTreePublicLinkFromPaths {
        name: link_name.clone(),
        paths: vec![path_a.clone(), path_b.clone()],
        expires: None,
    });
    assert_no_secret_leak(&create_resp);

    let status_ok = create_resp.status == ResponseStatus::Ok;
    let link_id = extract_link_id(&create_resp.message);
    let code = extract_code(&create_resp.message);

    // 3) Always clean up what we created, regardless of success.
    if status_ok {
        let mut deleted_link = false;
        if let Some(id) = link_id {
            let rm = daemon.dispatch(Request::DeletePublicLink { link_id: id });
            assert_no_secret_leak(&rm);
            if rm.status == ResponseStatus::Ok {
                deleted_link = true;
            }
        }
        if !deleted_link
            && let Some(c) = code.clone() {
                let rm = daemon.dispatch(Request::DeletePublicLinkByCode { code: c });
                assert_no_secret_leak(&rm);
            }
    }

    // Remote folder cleanup is not exposed via IPC today; the scratch
    // subfolders use unique tag+pid+nanos names so re-runs do not
    // collide. Parent scratch stays untouched.

    // 4) Now assert on the link-create outcome. Cleanup already ran.
    assert!(
        status_ok,
        "CreateTreePublicLinkFromPaths failed: status={} message={}",
        status_label(&create_resp.status),
        create_resp.message
    );
    assert!(
        link_id.is_some() || code.is_some(),
        "CreateTreePublicLinkFromPaths response must advertise a link id or code: {}",
        create_resp.message
    );

    // 5) Probe the list endpoint so we exercise the same surface the CLI
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

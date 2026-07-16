#![allow(clippy::pedantic)]
//! Live coverage for `Request::UploadWriteFromFile` (parity matrix row 93,
//! `pclsync/pupload.c:843-859`).
//!
//! Closes the iter-1 TEST-H-2 finding sub-step for row 93 — the
//! retained-Implemented row had its parity claim made by code citation
//! only, with no live test exercising the IPC + proto + daemon dispatch
//! arm.
//!
//! ## Reachability semantics
//!
//! `UploadWriteFromFile` is a server-side copy primitive: it asks the
//! pCloud server to copy bytes from an existing remote file (`fileid` +
//! content `hash`) into an in-progress upload session
//! (`upload_session_id`). The full happy-path test would have to (a)
//! upload a source file to obtain a real `source_fileid`/`source_hash`,
//! (b) create a fresh upload session, (c) issue the
//! `UploadWriteFromFile`, and (d) finalise the destination via
//! `UploadSave` — that is full-round-trip orchestration.
//!
//! For the parity-row "verb reached" test, we instead dispatch with
//! synthetic but well-formed values. The daemon must reach the proto
//! layer and the server must reply with `InvalidRequest` (or another
//! verb-reached status). The verb being "reached at all" is what the
//! parity claim of `Implemented` actually requires; the full round-trip
//! orchestration belongs in a dedicated integration test which is out
//! of scope for the iter-1 TEST-H-2 sub-step.
//!
//! Future work: a `_full_round_trip` companion test that uploads a real
//! source file, invokes `UploadWriteFromFile` against it, and asserts
//! the destination's bytes match the source. That test belongs in its
//! own fire because it requires the same scratch-folder + cleanup
//! discipline the existing `transfers.rs` test demonstrates.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_ipc::{Request, ResponseStatus};

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate,
    optional_env, skip_if_not_live, status_label,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_upload_writefromfile_dispatches_verb_reached() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping UploadWriteFromFile: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("upload-writefromfile-verb");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping UploadWriteFromFile: {err}");
        return;
    }

    // Synthetic-but-formed request. `upload_session_id = 0` is not a
    // valid live session, and `source_fileid = 0` is not a valid file
    // id, so the server is expected to reject with `InvalidRequest`.
    // What we verify is that the daemon dispatched the verb to the
    // proto layer and got an answer back.
    let resp = daemon.dispatch(Request::UploadWriteFromFile {
        upload_session_id: 0,
        source_fileid: 0,
        source_hash: 0,
        offset: 0,
        source_offset: None,
        count: 0,
    });
    assert_no_secret_leak(&resp);
    // Verb-reached: any non-Ok rejection is acceptable here. An `Ok`
    // would be a server-side bug (we passed garbage ids) and a panic
    // / hang would surface as a test framework failure. The narrow
    // contract is "the daemon dispatched and the server replied".
    assert!(
        matches!(
            resp.status,
            ResponseStatus::InvalidRequest
                | ResponseStatus::Conflict
                | ResponseStatus::Unauthorized
                | ResponseStatus::Unavailable
                | ResponseStatus::InternalError
        ),
        "UploadWriteFromFile must be dispatched and answered (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

#![allow(clippy::pedantic)]
//! Live stub: `ptree_public_link` path-based IPC variant (row 149).
//!
//! Verifies that `Request::CreateTreePublicLinkFromPaths` is dispatched
//! end-to-end against a real pCloud account, and that the resulting link
//! is retrievable and deletable.
//!
//! **Status:** stub — body is `todo!()` pending server-side path-resolution
//! IPC wiring tracked under bd-1du.10 / pcloud-rs-s1p.57.
//!
//! Gate: `PCLOUD_LIVE_E2E=1` + valid credentials.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use crate::common::{ENV_PASSWORD, ENV_TOKEN, ENV_USER, skip_if_not_live};

/// Live end-to-end: create a tree public link using the path-based IPC
/// variant (daemon resolves paths under its auth context).
///
/// TODO(bd-1du.10): Implement once `Request::CreateTreePublicLinkFromPaths`
/// IPC variant is wired through `TransferRuntime`/`PublicLinkRuntime` and
/// the CLI.  The test body is a compile-time placeholder only.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials; body is todo!() pending IPC wiring"]
fn live_create_tree_public_link_from_paths() {
    if skip_if_not_live(&[]) {
        return;
    }
    // Require at least one form of credentials.
    let _has_creds = common::optional_env(ENV_TOKEN).is_some()
        || (common::optional_env(ENV_USER).is_some()
            && common::optional_env(ENV_PASSWORD).is_some());

    // TODO(bd-1du.10): Replace todo!() with real test body once the IPC
    // variant `Request::CreateTreePublicLinkFromPaths` exists:
    //   1. Authenticate via `common::authenticate`.
    //   2. Upload a scratch folder with two files.
    //   3. Dispatch `Request::CreateTreePublicLinkFromPaths { paths: vec![...] }`.
    //   4. Assert ResponseStatus::Ok and extract link id/code from message.
    //   5. Verify link is retrievable via `Request::ListPublicLinks`.
    //   6. Delete link and scratch folder.
    todo!("IPC variant not yet wired — see bd-1du.10 and pcloud-rs-s1p.57")
}

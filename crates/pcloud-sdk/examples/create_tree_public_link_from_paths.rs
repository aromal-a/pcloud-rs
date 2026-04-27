#![allow(clippy::pedantic)]
//! Example: create a tree public link by resolving pCloud-drive paths
//! daemon-side via `EmbeddedDaemon::create_tree_public_link_from_paths`.
//!
//! This demonstrates the typed SDK wrapper for the `ptree_public_link`
//! path-based variant (row 149, bd-1du). The daemon resolves each absolute
//! pCloud-drive path to a remote folder id under the authenticated session
//! before issuing the tree-link create call — no manual id lookup required.
//!
//! Without `PCLOUD_LIVE=1` this example prints a usage hint and exits
//! immediately — it never touches the network and is safe in CI.
//!
//! Run (dry):
//!   cargo run -p pcloud-sdk --example create_tree_public_link_from_paths
//!
//! Run (live):
//!   PCLOUD_LIVE=1 \
//!   PCLOUD_EMAIL=user@example.com \
//!   PCLOUD_PASSWORD=secret \
//!   PCLOUD_PATHS=/Documents,/Photos \
//!   PCLOUD_LINK_NAME="My shared bundle" \
//!   cargo run -p pcloud-sdk --example create_tree_public_link_from_paths

// **PLATFORM:** all
// **GATING:** PCLOUD_LIVE=1 required for network calls.

use std::path::PathBuf;

use pcloud_ipc::{Request, ResponseStatus, redacted::RedactedString};
use pcloud_sdk::EmbeddedDaemon;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("PCLOUD_LIVE").is_err() {
        println!("Set PCLOUD_LIVE=1 to run this example against a real pCloud account.");
        println!(
            "It demonstrates: sdk.create_tree_public_link_from_paths(\
             name, paths, expires)"
        );
        println!(
            "Required env vars: PCLOUD_EMAIL, PCLOUD_PASSWORD, PCLOUD_PATHS (comma-separated),"
        );
        println!(
            "                   PCLOUD_LINK_NAME (optional, defaults to 'SDK example bundle')"
        );
        return Ok(());
    }

    let root = PathBuf::from(
        std::env::var("PCLOUD_ROOT").unwrap_or_else(|_| "/tmp/pcloud-sdk-example".to_owned()),
    );
    std::fs::create_dir_all(&root)?;

    let mut daemon = EmbeddedDaemon::builder(root).build()?;

    // ── Authenticate ────────────────────────────────────────────────────────
    let email = std::env::var("PCLOUD_EMAIL")?;
    let password = std::env::var("PCLOUD_PASSWORD")?;

    let auth_resp = daemon.dispatch(Request::PasswordSubmission {
        username: email,
        value: RedactedString::new(password),
    });
    if auth_resp.status != ResponseStatus::Ok {
        return Err(format!("auth failed: {}", auth_resp.message).into());
    }
    println!("authenticated.");

    // ── Parse paths from env ─────────────────────────────────────────────────
    let paths_raw = std::env::var("PCLOUD_PATHS").unwrap_or_else(|_| "/Documents".to_owned());
    let paths: Vec<String> = paths_raw
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let name =
        std::env::var("PCLOUD_LINK_NAME").unwrap_or_else(|_| "SDK example bundle".to_owned());

    println!(
        "creating tree public link '{}' for paths: {:?}",
        name, paths
    );

    // ── Create tree public link from paths ────────────────────────────────────
    let result = daemon.create_tree_public_link_from_paths(name, paths, None)?;

    println!("tree public link created:");
    println!("  link_id : {}", result.link_id);
    println!("  name    : {}", result.name);
    println!("  link    : {}", result.link);

    Ok(())
}

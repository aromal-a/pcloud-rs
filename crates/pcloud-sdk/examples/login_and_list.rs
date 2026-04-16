#![allow(clippy::pedantic)]
//! Boots an in-process `EmbeddedDaemon`, dispatches a small set of typed IPC
//! requests against it, and prints the responses. Representative of how a
//! host application would embed the Rust runtime instead of talking to a
//! separate daemon process.
//!
//! Live auth against real pCloud servers is gated behind the
//! `PCLOUD_LIVE=1` environment variable AND credentials in
//! `PCLOUD_USERNAME` / `PCLOUD_PASSWORD`. Without that gate, the example
//! only exercises the local in-process dispatch surface (status, sync-root
//! listing) so it never touches the network in CI.
//!
//! Run with: `cargo run -p pcloud-sdk --example login_and_list`
//! Live mode: `PCLOUD_LIVE=1 PCLOUD_USERNAME=... PCLOUD_PASSWORD=... \
//!             cargo run -p pcloud-sdk --example login_and_list`

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use pcloud_config::Environment;
use pcloud_ipc::{Method, Request};
use pcloud_sdk::EmbeddedDaemon;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use a scratch root under the temp dir so the example never stamps on
    // real user state. Production use would point at a persistent runtime root.
    let root: PathBuf =
        std::env::temp_dir().join(format!("pcloud-sdk-example-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;

    let mut daemon = EmbeddedDaemon::builder(root.clone())
        .environment(Environment::Development)
        .build()?;

    println!("summary: {}", daemon.runtime_summary());

    // Offline dispatch: always safe, never hits the network.
    let status = daemon.dispatch(Request::Plain {
        method: Method::GetStatus,
    });
    println!("status:  {:?} :: {}", status.status, status.message);

    let roots = daemon.dispatch(Request::Plain {
        method: Method::GetSyncRoots,
    });
    println!("roots:   {:?} :: {}", roots.status, roots.message);

    // Live mode is opt-in. We intentionally do not construct a
    // PasswordSubmission here because that requires a network round-trip to
    // pCloud and must only run when the operator has set PCLOUD_LIVE=1.
    if std::env::var("PCLOUD_LIVE").ok().as_deref() == Some("1") {
        let user = std::env::var("PCLOUD_USERNAME").unwrap_or_default();
        let pass = std::env::var("PCLOUD_PASSWORD").unwrap_or_default();
        if user.is_empty() || pass.is_empty() {
            eprintln!(
                "PCLOUD_LIVE=1 set but PCLOUD_USERNAME/PCLOUD_PASSWORD are missing; skipping live login"
            );
        } else {
            let resp = daemon.dispatch(Request::PasswordSubmission {
                username: user,
                value: pass,
            });
            println!("login:   {:?} :: {}", resp.status, resp.message);
        }
    } else {
        println!("live:    skipped (set PCLOUD_LIVE=1 to enable)");
    }

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

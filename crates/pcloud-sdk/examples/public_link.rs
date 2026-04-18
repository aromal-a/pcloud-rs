#![allow(clippy::pedantic)]
//! Example: create a public link, list it, then delete it.
//!
//! Without `PCLOUD_LIVE=1` this example prints a usage hint and exits
//! immediately — it never touches the network and is safe in CI.
//!
//! Run (dry):
//!   cargo run -p pcloud-sdk --example public_link
//!
//! Run (live):
//!   PCLOUD_LIVE=1 \
//!   PCLOUD_EMAIL=user@example.com \
//!   PCLOUD_PASSWORD=secret \
//!   PCLOUD_FILE_PATH=/MyFolder/MyFile.txt \
//!   cargo run -p pcloud-sdk --example public_link

// **PLATFORM:** all
// **GATING:** PCLOUD_LIVE=1 required for network calls.

use std::path::PathBuf;

use pcloud_ipc::{Method, Request, ResponseStatus};
use pcloud_sdk::EmbeddedDaemon;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("PCLOUD_LIVE").is_err() {
        println!("Set PCLOUD_LIVE=1 to run this example against a real pCloud account.");
        println!(
            "It demonstrates: sdk.dispatch(CreateFilePublicLink) \
             -> sdk.dispatch(ListPublicLinks) -> sdk.dispatch(DeletePublicLink)"
        );
        return Ok(());
    }

    let email = std::env::var("PCLOUD_EMAIL")
        .unwrap_or_else(|_| std::env::var("PCLOUD_USERNAME").unwrap_or_default());
    let password = std::env::var("PCLOUD_PASSWORD").unwrap_or_default();
    let file_path =
        std::env::var("PCLOUD_FILE_PATH").unwrap_or_else(|_| "/Public/example.txt".to_owned());

    if email.is_empty() || password.is_empty() {
        eprintln!("PCLOUD_LIVE=1 set but PCLOUD_EMAIL/PCLOUD_PASSWORD are missing; aborting");
        std::process::exit(1);
    }

    let root: PathBuf =
        std::env::temp_dir().join(format!("pcloud-sdk-publink-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;

    let mut daemon = EmbeddedDaemon::builder(root.clone())
        .environment(pcloud_config::Environment::Production)
        .build()?;

    // Authenticate.
    let login_resp = daemon.dispatch(Request::PasswordSubmission {
        username: email,
        value: password.into(),
    });
    if login_resp.status != ResponseStatus::Ok {
        eprintln!(
            "Login failed: {:?} — {}",
            login_resp.status, login_resp.message
        );
        std::process::exit(3);
    }
    println!("login: ok");

    // Create a file public link.
    let create_resp = daemon.dispatch(Request::CreateFilePublicLink {
        path: file_path.clone(),
    });
    println!(
        "create-file-link: {:?} — {}",
        create_resp.status, create_resp.message
    );

    // List public links.
    let list_resp = daemon.dispatch(Request::Plain {
        method: Method::ListPublicLinks,
    });
    println!("list-links: {:?} — {}", list_resp.status, list_resp.message);

    // Parse the link id from the create response payload to demonstrate delete.
    // In production code you would deserialise the JSON payload into a typed
    // struct; here we extract the id heuristically for brevity.
    if let Some(id_str) = create_resp.payload.as_ref().and_then(|p| {
        // Payload is JSON; look for `"link_id":NNN` or `"id":NNN`.
        let s = p.to_string();
        s.split('"')
            .zip(s.split('"').skip(1))
            .find(|(k, _)| *k == "link_id" || *k == "id")
            .and_then(|(_, v)| v.trim_start_matches(':').trim().split([',', '}']).next())
            .map(|v| v.trim().to_owned())
    }) {
        if let Ok(link_id) = id_str.parse::<u64>() {
            let del_resp = daemon.dispatch(Request::DeletePublicLink { link_id });
            println!(
                "delete-link({link_id}): {:?} — {}",
                del_resp.status, del_resp.message
            );
        }
    }

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

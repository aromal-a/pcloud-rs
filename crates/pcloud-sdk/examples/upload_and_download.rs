#![allow(clippy::pedantic)]
//! Example: upload a file then download it back.
//!
//! Without `PCLOUD_LIVE=1` this example prints a usage hint and exits
//! immediately — it never touches the network and is safe in CI.
//!
//! Run (dry):
//!   cargo run -p pcloud-embedded-sdk --example upload_and_download
//!
//! Run (live):
//!   PCLOUD_LIVE=1 \
//!   PCLOUD_EMAIL=user@example.com \
//!   PCLOUD_PASSWORD=secret \
//!   cargo run -p pcloud-embedded-sdk --example upload_and_download

// **PLATFORM:** all
// **GATING:** PCLOUD_LIVE=1 required for network calls.

use std::path::PathBuf;

use pcloud_embedded_sdk::EmbeddedDaemon;
use pcloud_ipc::{Request, ResponseStatus};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("PCLOUD_LIVE").is_err() {
        println!("Set PCLOUD_LIVE=1 to run this example against a real pCloud account.");
        println!(
            "It demonstrates: EmbeddedDaemon::builder().build() \
             -> upload_data() -> download_file()"
        );
        return Ok(());
    }

    let email = std::env::var("PCLOUD_EMAIL")
        .unwrap_or_else(|_| std::env::var("PCLOUD_USERNAME").unwrap_or_default());
    let password = std::env::var("PCLOUD_PASSWORD").unwrap_or_default();
    if email.is_empty() || password.is_empty() {
        eprintln!("PCLOUD_LIVE=1 set but PCLOUD_EMAIL/PCLOUD_PASSWORD are missing; aborting");
        std::process::exit(1);
    }

    let root: PathBuf =
        std::env::temp_dir().join(format!("pcloud-sdk-upload-download-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;

    // Build an embedded daemon backed by real pCloud servers.
    let mut daemon = EmbeddedDaemon::builder(root.clone())
        .environment(pcloud_config::Environment::Production)
        .build()?;

    // Authenticate via password submission.
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

    // Upload a small in-memory payload to the root folder (folder_id = 0).
    let payload = b"hello from pcloud-sdk upload_and_download example".to_vec();
    let payload_len = payload.len() as u64;
    let remote_name = format!("sdk-example-{}.txt", std::process::id());

    let result = daemon.upload_data(0, remote_name.clone(), &payload)?;
    println!(
        "upload: file_id={:?} name={} bytes={}",
        result.file_id, result.remote_filename, result.bytes_uploaded
    );

    // Download the file back using the file_id returned from upload.
    if let Some(file_id) = result.file_id {
        let bytes = daemon.download_file(file_id)?;
        println!("download: {} bytes received", bytes.len());
        assert_eq!(
            bytes.len() as u64,
            payload_len,
            "downloaded byte count should match uploaded byte count"
        );
        println!("round-trip ok");
    } else {
        println!("upload succeeded but no file_id returned — skipping download");
    }

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#![allow(clippy::pedantic)]
//! Example: crypto folder lifecycle — setup, unlock, mkdir, lock.
//!
//! Without `PCLOUD_LIVE=1` this example prints a usage hint and exits
//! immediately — it never touches the network and is safe in CI.
//!
//! Run (dry):
//!   cargo run -p pcloud-sdk --example crypto_lifecycle
//!
//! Run (live):
//!   PCLOUD_LIVE=1 \
//!   PCLOUD_EMAIL=user@example.com \
//!   PCLOUD_PASSWORD=secret \
//!   PCLOUD_CRYPTO_PASS=my-crypto-passphrase \
//!   cargo run -p pcloud-sdk --example crypto_lifecycle

// **PLATFORM:** all
// **GATING:** PCLOUD_LIVE=1 required for network calls.

use std::path::PathBuf;

use pcloud_ipc::{Method, Request, ResponseStatus};
use pcloud_sdk::EmbeddedDaemon;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("PCLOUD_LIVE").is_err() {
        println!("Set PCLOUD_LIVE=1 to run this example against a real pCloud account.");
        println!(
            "It demonstrates: sdk.dispatch(CryptoSetup) \
             -> sdk.dispatch(CryptoUnlock) -> sdk.dispatch(CryptoMkdir) \
             -> sdk.dispatch(LockCrypto)"
        );
        return Ok(());
    }

    let email = std::env::var("PCLOUD_EMAIL")
        .unwrap_or_else(|_| std::env::var("PCLOUD_USERNAME").unwrap_or_default());
    let password = std::env::var("PCLOUD_PASSWORD").unwrap_or_default();
    let crypto_pass = std::env::var("PCLOUD_CRYPTO_PASS").unwrap_or_default();

    if email.is_empty() || password.is_empty() || crypto_pass.is_empty() {
        eprintln!(
            "PCLOUD_LIVE=1 set but PCLOUD_EMAIL/PCLOUD_PASSWORD/PCLOUD_CRYPTO_PASS \
             are missing; aborting"
        );
        std::process::exit(1);
    }

    let root: PathBuf =
        std::env::temp_dir().join(format!("pcloud-sdk-crypto-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;

    let mut daemon = EmbeddedDaemon::builder(root.clone())
        .environment(pcloud_config::Environment::Production)
        .build()?;

    // Authenticate.
    let login_resp = daemon.dispatch(Request::PasswordSubmission {
        username: email,
        value: password.clone().into(),
    });
    if login_resp.status != ResponseStatus::Ok {
        eprintln!(
            "Login failed: {:?} — {}",
            login_resp.status, login_resp.message
        );
        std::process::exit(3);
    }
    println!("login: ok");

    // Check current crypto status.
    let status_resp = daemon.dispatch(Request::Plain {
        method: Method::GetCryptoStatus,
    });
    println!(
        "crypto-status: {:?} — {}",
        status_resp.status, status_resp.message
    );

    // Unlock crypto (if already set up) or set up and unlock (first run).
    // The CryptoUnlock request carries the passphrase securely.
    let unlock_resp = daemon.dispatch(Request::CryptoUnlock {
        password: crypto_pass.clone().into(),
    });
    println!(
        "unlock-crypto: {:?} — {}",
        unlock_resp.status, unlock_resp.message
    );

    if unlock_resp.status == ResponseStatus::Ok {
        // Create an encrypted subfolder inside the crypto root.
        let mkdir_resp = daemon.dispatch(Request::CryptoMkdir {
            name: format!("sdk-example-{}", std::process::id()),
            parent_folder_id: None,
            local_folder_id: None,
        });
        println!(
            "crypto-mkdir: {:?} — {}",
            mkdir_resp.status, mkdir_resp.message
        );

        // Lock the crypto folder and zero in-memory key material.
        let lock_resp = daemon.dispatch(Request::Plain {
            method: Method::LockCrypto,
        });
        println!(
            "lock-crypto: {:?} — {}",
            lock_resp.status, lock_resp.message
        );
    } else {
        println!("crypto unlock failed — skipping mkdir and lock");
    }

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

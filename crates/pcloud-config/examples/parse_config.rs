#![allow(clippy::pedantic)]
//! Parses an inline JSON config envelope (the real on-disk format) and prints
//! the resulting typed `ConfigProfile`. The task brief mentions TOML, but the
//! workspace config loader is JSON-only (see
//! `crates/pcloud-config/src/loader.rs`), so this example stays true to the
//! actual format rather than inventing a parallel one.
//!
//! Run with: `cargo run -p pcloud-config --example parse_config`

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use pcloud_config::{ConfigProfile, Environment};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build a valid envelope by serializing a known-good secure-defaults
    // profile. This guarantees the example stays in sync with the current
    // schema version without hand-writing every field.
    let reference = ConfigProfile::secure_defaults(
        PathBuf::from("/tmp/pcloud-example-root"),
        Environment::Development,
    );

    let envelope = serde_json::json!({
        "version": pcloud_config::migrate::CURRENT_VERSION,
        "profile": reference,
    });
    let serialized = serde_json::to_string_pretty(&envelope)?;
    println!("--- input envelope ---\n{serialized}\n");

    // Round-trip: decode the envelope, extract the profile, and validate.
    let decoded: serde_json::Value = serde_json::from_str(&serialized)?;
    let profile_value = decoded
        .get("profile")
        .cloned()
        .ok_or("envelope missing 'profile' field")?;
    let profile: ConfigProfile = serde_json::from_value(profile_value)?;
    profile.validate()?;

    println!("--- parsed profile ---");
    println!("environment:    {:?}", profile.environment);
    println!("config_dir:     {}", profile.paths.config_dir.display());
    println!("runtime_dir:    {}", profile.paths.runtime_dir.display());
    println!("crypto_enabled: {}", profile.features.crypto_enabled);
    println!("max_uploads:    {}", profile.limits.max_concurrent_uploads);
    println!("ok: validated");
    Ok(())
}

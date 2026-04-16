#![allow(clippy::pedantic)]
//! Build script: embed the short git commit hash into the binary as
//! `env!("GIT_HASH")` so `pcloud-rs --version` can report
//! `pcloud-rs <pkg-version> (<git-hash>, <profile>)`.
//!
//! Failure modes are intentionally soft:
//! - not a git checkout (tarball release, vendored build)  → skip
//! - git not installed                                      → skip
//! - git command fails or returns non-UTF-8                 → skip
//!
//! In every skip case we simply don't set `GIT_HASH`, which makes
//! `option_env!("GIT_HASH")` evaluate to `None` at runtime and the CLI
//! falls back to `"unknown"`. No build failures from missing git.
//!
//! We also emit `cargo:rerun-if-changed=.git/HEAD` so a branch switch
//! or new commit triggers a rebuild of this crate only.

use std::process::Command;

fn main() {
    // Propagate the build profile (debug/release) to the runtime.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=BUILD_PROFILE={profile}");

    // Trigger a rebuild when HEAD moves.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=GIT_HASH");

    // Allow overriding via env (CI / reproducible builds).
    if let Ok(h) = std::env::var("GIT_HASH")
        && !h.is_empty()
    {
        println!("cargo:rustc-env=GIT_HASH={h}");
        return;
    }

    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            if let Ok(s) = String::from_utf8(o.stdout) {
                let hash = s.trim();
                if !hash.is_empty() {
                    println!("cargo:rustc-env=GIT_HASH={hash}");
                }
            }
        }
        _ => {
            // Silent soft-failure: no GIT_HASH set → "unknown" at runtime.
        }
    }
}

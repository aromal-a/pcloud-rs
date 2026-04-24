//! Stub replacement for `msvc_spectre_libs`.
//!
//! The upstream crate's build.rs panics when the MSVC Spectre-mitigated
//! runtime libraries aren't present, which requires a specific optional
//! VS Build Tools component (`VC.14.x.x.Spectre.x86.x64`). For pcloud-rs's
//! Windows Tier-3 compile-testing goal we don't need the Spectre
//! hardening yet (the whole FUSE-on-Windows surface is
//! scaffolded-only, not shipped). This stub exposes the crate as a
//! no-op so regorus's transitive dep resolves without installing the
//! extra component.
//!
//! Once Windows moves to Tier-1 (bd-xplat-windows), drop the
//! `[patch.crates-io]` override in the workspace root Cargo.toml and
//! install the real Spectre libs in CI via the VS installer's
//! `--add Microsoft.VisualStudio.Component.VC.<version>.Spectre.x86.x64`
//! flag.

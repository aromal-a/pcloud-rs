#![forbid(unsafe_code)]

//! # pcloud-session
//!
//! Session lifecycle primitives extracted from `pcloud-daemon` (PLAN_A_PLUS
//! P6.1 follow-up). This crate holds the deterministic, clock-driven
//! [`session_lifecycle::SessionSupervisor`] and the synchronous
//! [`refresh_loop::tick`] primitive so the daemon composition root stays
//! small and so downstream crates (SDK, CLI, tests) can pull in session
//! lifecycle behaviour without depending on the full daemon.
//!
//! **Architecture:** see `docs/book/src/architecture/crate-map.md`. This
//! crate depends on `pcloud-auth`, `pcloud-backends` (for
//! `AuthRuntime`), `pcloud-config`, `pcloud-proto`, `pcloud-secret`, and
//! `pcloud-store`.
//!
//! **Stability:** T1 internal — public API is not semver-stable across
//! workspace revisions. External consumers should go through
//! `pcloud-sdk` or the back-compat re-exports in `pcloud_daemon`.
//!
//! **MSRV:** Rust 1.89 for the portable crate; full workspace and release
//! validation use the repository-pinned Rust 1.96.1 toolchain.
//!
//! **Platform:** portable.
//!
//! # Back-compat
//!
//! Historical paths `pcloud_daemon::session_lifecycle::*` and
//! `pcloud_daemon::refresh_loop::*` continue to work via `pub use`
//! re-exports in `pcloud-daemon::lib.rs`.
//!
//! `auth_vault` intentionally still lives inside `pcloud-daemon` because
//! it is a thin shim over the daemon-owned `vault::file::*` surface and
//! moving it without the whole `vault/` subtree would sever the
//! `crate::vault::file::{load_token,store_token,clear_token}`
//! re-exports the daemon depends on.

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

pub mod refresh_loop;
pub mod session_lifecycle;

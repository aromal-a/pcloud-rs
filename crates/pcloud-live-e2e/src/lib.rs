#![forbid(unsafe_code)]
//! `pcloud-live-e2e` — integration-test-only crate that exercises the Rust
//! client against the real pCloud backend.
//!
//! # Why this crate has no `pub` items
//!
//! The `lib.rs` is intentionally empty (no `pub` types, no `pub` functions,
//! no re-exports). This crate is a pure test target: all verification
//! logic lives under `tests/`, compiles as its own integration binaries,
//! and automatically inherits the `dev-dependencies` declared in
//! `Cargo.toml` without any public surface here.
//!
//! Keeping the library empty is a deliberate security property:
//!
//! * No other crate in the workspace can accidentally add
//!   `pcloud-live-e2e` as a dependency and pull in live-network helpers,
//!   test credentials plumbing, or backend fixtures into a production
//!   compilation graph.
//! * There is no API surface to misuse; the only way to run anything in
//!   this crate is `cargo test -p pcloud-live-e2e`, and even then the
//!   tag-gated runtime checks documented below must pass.
//!
//! # Tag-gated runtime requirements
//!
//! Every integration test in this crate short-circuits (passing with a
//! skip banner) unless **all** of the following environment variables are
//! set at test invocation time:
//!
//! * `PCLOUD_LIVE=1` — the master opt-in flag. Without this, tests return
//!   immediately. This prevents live network calls from happening by
//!   accident in a developer's local `cargo test` or in CI jobs that
//!   don't explicitly request live coverage.
//! * `PCLOUD_USERNAME` — pCloud account email used for password auth.
//! * `PCLOUD_PASSWORD` — pCloud account password; consumed once at test
//!   start to acquire an auth token, never written to disk, never logged.
//!
//! Optional variables recognised by individual tests:
//!
//! * `PCLOUD_TFA_CODE` — prepopulated TFA response for accounts with
//!   two-factor enabled.
//! * `PCLOUD_API_HOST` — override the default API host (still required to
//!   be TLS; plaintext overrides are rejected by production policy).
//!
//! Some older tests in this crate also accept `PCLOUD_LIVE_E2E=1` as an
//! alias for `PCLOUD_LIVE=1` for backwards compatibility; new tests should
//! consult `PCLOUD_LIVE=1` only.
//!
//! # No-secrets-in-logs policy
//!
//! Tests in this crate MUST follow these rules:
//!
//! * Never print `PCLOUD_PASSWORD`, TFA codes, recovery codes, or any
//!   derived auth token to stdout, stderr, or panic messages.
//! * Never include secret material in assertion failure messages — if an
//!   assertion needs to compare a secret-bearing value, compare hashes or
//!   lengths instead.
//! * Prefer the `SecretString` / `SecretBytes` wrappers from
//!   `pcloud-secret` for any value that holds a credential, so `Debug`
//!   output is automatically redacted and the buffer is zeroized on drop.
//! * Route any diagnostic output through the test harness's normal logger
//!   with redaction on; do not add ad-hoc `println!` / `eprintln!` calls
//!   that might echo request bodies containing auth fields.
//! * If a backend response includes a token (e.g. on login), treat it as
//!   secret for the rest of the test: do not embed it in URLs that will
//!   be logged, do not write it to temporary files.
//!
//! These rules mirror the workspace-wide secret handling policy in
//! `CLAUDE.md` and the enterprise defaults enforced by the daemon's auth
//! vault (`crates/pcloud-daemon/src/auth_vault.rs`).
//!
//! # What this crate proves
//!
//! The live tests here are the final verification layer for parity claims:
//! they demonstrate that the Rust path actually authenticates, lists
//! folders, performs transfers, and manages account metadata against the
//! real backend, not just against [`pcloud-mockserver`]. They are the
//! proof artefacts referenced by `bd-1du.10` (the final parity gate).
//!
//! [`pcloud-mockserver`]: ../pcloud_mockserver/index.html
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
// **PLATFORM:** all
// **GATING:** runtime-only (env vars PCLOUD_LIVE=1 + credentials).
// **SECURITY:** no secrets are logged; credentials read once from env.

# pcloud-rs GPT Review Turn 4 Fix Summary

Generated: 2026-04-30

## Fix Agent Split

- Worker 1: security/KMS/proto transport.
- Worker 2: web auth, IPC framing, daemon config loading, systemd.
- Worker 3: sync runtime, FUSE write path/journal, watcher, integrity sweeper.
- Worker 4: parity/API/CLI/SDK reachability and parity truth docs.
- Worker 5: CI, release, packaging, Docker, Nix, docs truthfulness.
- Worker 6: dependency/MSRV/code quality and auth refresh poison behavior.

## Major Fixes Landed

- Vault KMS now rejects non-HTTPS URLs and the Vault reqwest client is configured with HTTPS-only enforcement.
- Binary API request/debug surfaces and signed download debug output are redacted; encoded request bytes now zeroize on drop.
- Plaintext binary/download transport paths are restricted to loopback/test-style use.
- Web daemon-backed read routes now require `X-PCloud-Web-Token`, and web-token files are written with owner-only, no-follow, atomic file discipline on Unix.
- IPC request/response decoders reject wrong message kinds before deserialization, and clients validate response header kind/version/size before allocating payloads.
- `PCLOUD_CONFIG` is loaded through the strict config loader during daemon bootstrap, and the resolved config path is preserved for reload.
- `SetApiServer` invalid hints now return `InvalidRequest` and do not mutate or persist state.
- systemd service now uses `Type=simple`, anchors `PCLOUD_ROOT`, and the socket unit is disabled until socket activation is implemented.
- Sync planning now combines remote/local observations in one planner pass, delays diff cursor commit until combined ingestion, and executes transfers once per cycle.
- Local sync uploads can read safe source files from sync roots; retryable failures requeue via `requeue_for_retry`.
- Watcher overflow records full-rescan markers; FUSE write journal checkpointing is path-scoped and truncate records are journaled before mutation.
- Integrity sweeper bootstraps non-paused roots and fails closed when enabled without roots or a real checksum fetcher.
- Row 93 `upload_writefromfile` now carries distinct upload and source offsets through IPC/CLI/daemon/SDK; CLI preserves legacy same-offset behavior.
- Row 149 tree public links now expose root/folder/file targets through IPC/CLI/daemon/SDK.
- Row 94 SDK `UploadSession` was downgraded to `Partial` consistently because the public SDK path is still synchronous and conflict policy is not fully wired.
- Parity count surfaces were reconciled to `149 Implemented / 7 Partial / 0 Missing / 30 Rejected`.
- Release workflows now run source gates before publishing and manual tag dispatch validates/checks out the requested tag.
- Invalid cargo-deny `--all-features` invocations were removed.
- Docker/Nix/package docs were corrected to match actual outputs; unsupported signing/provenance/platform release claims were downgraded.
- Every workspace crate now has `rust-version.workspace = true`.
- Auth refresh single-flight state no longer relies on a poisoned mutex; it uses an atomic guard and has an unwind regression test.
- Missing CLI `SAFETY:` comments were added for audited unsafe blocks.

## Verification

- `python3` YAML parse over `.github/workflows/*.yml`: passed.
- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo test --workspace --locked`: passed, including slow RSA-heavy tests.
- `cargo deny check`: passed with existing duplicate-dependency and unmatched-license warnings.
- `cargo audit --deny warnings --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2025-0134 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141`: passed.
- `cargo +1.85.0 check --workspace --all-targets --locked`: passed.
- Stale packaging grep for `pcloudcom/console-client`, `GPL-3.0-or-later`, and `cargo deny.*--all-features`: clean.
- Workspace crate MSRV metadata check: `0` missing `rust-version`.

## Remaining Documented Blockers

- RSA `RUSTSEC-2023-0071` remains a reviewed risk acceptance because no fixed upstream `rsa` crate release exists; the exception is explicit and time-boxed.
- `EncodedRequest.params` still retains plaintext parameters for legacy dev/mock transport compatibility, but Debug is redacted and serialized bytes zeroize.
- Live Vault roundtrip, Docker build, Nix build, mdBook build, and platform signing/notarization were not run in this environment.
- Host/Origin enforcement and no-JS CSRF form usability remain open web-hardening items.
- A production remote checksum fetcher is still not implemented; enabled integrity sweeper now fails closed instead of silently using the no-op fetcher.
- SDK row 94 daemon-backed public upload session, crypto share/team-share IPC/CLI/SDK reachability, and public-link specialty rows 147/148/168 remain intentionally tracked as Partial rather than overclaimed.

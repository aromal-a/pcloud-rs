# Turn 5 Fix Summary

Date: 2026-04-30

## Completed Fix Areas

- Saved all six review reports plus this fix summary under `GPTREV/turn5/`.
- Added a runnable `pcloud-web` binary, documented it, and verified `cargo run -p pcloud-web -- --help`.
- Hardened the web surface with Host allowlisting, mutating `Origin`/`Referer` enforcement, session-cookie form submission, hidden CSRF form support, and browser-like POST tests.
- Corrected parity/status documentation to the current truth: `149 Implemented / 7 Partial / 0 Missing / 30 Rejected (186 rows)`.
- Added live-gated row 149 coverage for `CreateTreePublicLinkFromPathTargets` with root, folder, and file targets.
- Changed FUSE mount startup to fail closed on unreplayed write journal records instead of silently mounting writable.
- Journaled `O_TRUNC` before mutating staging and ensured truncate-only opens are tracked for drain/upload.
- Added WinFSP dirty-write flush propagation on flush/close paths.
- Executed local sync mkdir/delete operations with containment checks and made unsupported remote pending operations fail visibly instead of staying silently pending.
- Hardened IPC debug/serialized frame handling, limited release `EncodedRequest.params` plaintext retention, and added zeroizing frame buffers.
- Rejected configured TLS revocation modes until transport-level revocation enforcement exists.
- Strengthened file-vault parent ownership/mode validation.
- Hardened WinFSP DLL loading away from search-order `LoadLibraryW("winfsp-x64.dll")`.
- Fixed no-default clippy in `pcloud-idp` and updated stale crate-level MSRV rustdoc to Rust 1.85.
- Improved daemon watchdog cadence, health-port parsing, metrics loop lifecycle, metrics connection caps, and Windows service failure reporting.
- Updated CI/release/package/deployment docs and packaging assets for locked Cargo commands, release gate truth, systemd user/system split, macOS LaunchAgent config, Linux platform paths, fuzz commands, credential-file env vars, and reproducibility wording.

## Final Verification

- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo clippy --workspace --all-targets --no-default-features --locked -- -D warnings`: passed.
- `cargo check -p pcloud-daemon --features metrics --locked`: passed.
- `cargo test --workspace --locked --no-fail-fast`: passed, including doctests and slow RSA suites.
- `cargo deny check`: passed with existing duplicate-dependency and unmatched-license warnings.
- `cargo audit --deny warnings --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2025-0134 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141`: passed.
- `cargo run -p pcloud-web -- --help`: passed.
- `cargo +1.85.0 check --workspace --all-targets --locked`: passed.
- `git diff --check`: passed.

## Remaining Risks

- RSA Marvin (`RUSTSEC-2023-0071`) remains accepted through explicit audit/deny policy because no safe constant-time drop-in replacement was introduced in this turn.
- Seven parity rows remain `Partial` by design until the missing user-facing/live-verified flows are implemented and proven: rows `94`, `124`, `138`, `142`, `147`, `148`, and `168`.
- Live pCloud, live Vault, WinFSP-on-Windows, macOS launchd, Docker, Nix, and mdBook validation were not run in this environment because required credentials or tools were unavailable.
- Privileged IPC operations are better audited and transport-limited, but a full per-request admin-capability protocol remains a larger design item and was not completed in this turn.

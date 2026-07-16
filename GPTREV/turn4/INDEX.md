# pcloud-rs GPT Review Turn 4 Index

Generated: 2026-04-30

Master prompt: `pcloud_rev.md`

Output policy: six explorer subagents used the master audit prompt with write target overridden from `AUDIT_REPORT.md` to `GPTREV/turn4/`.

## Reports

- `01_parity_api_cli_sdk.md` - parity matrix/API truth, CLI/SDK reachability, public-link, upload-session, and crypto-share surfaces.
- `02_security_crypto_transport.md` - security, crypto, transport, secret buffers, Vault KMS, TLS/downgrade, web auth, and WinFSP loading.
- `03_sync_fuse_data_integrity.md` - sync planner/runtime, FUSE journal replay/checkpointing, local uploads, retry, and integrity sweeper.
- `04_ipc_daemon_web_config_ops.md` - IPC, daemon lifecycle, config loading, web management, systemd/macOS/Windows service behavior.
- `05_testing_ci_deployment_docs.md` - testing/CI/live E2E, release/signing/provenance, Docker/Nix, packaging, and docs truthfulness.
- `06_code_quality_dependency_inventory.md` - build health, dependency advisory state, cargo-deny/audit, MSRV, unsafe, unwrap, lock poison, and inventory.

## Highest-Risk Turn 4 Findings

- HashiCorp Vault KMS can use plaintext HTTP and expose Vault token plus plaintext DEK.
- FUSE journal replay still does not execute recovered writes, and shared journal checkpointing can erase unrelated dirty-file recovery records.
- Release publishing is disconnected from the documented release gauntlet, and signing/provenance claims exceed actual workflow outputs.
- Web management read routes expose daemon state without web-token, Host, or Origin enforcement.
- Daemon startup does not load operator config files, making config validation and SIGHUP reload dead in the normal path.
- Local sync uploads still do not read ordinary source files, and retryable sync failures are classified but not requeued.
- `cargo audit` fails on default-enabled `rsa 0.9.10` / `RUSTSEC-2023-0071`, and security workflow cargo-deny flags are invalid for cargo-deny 0.19.
- Parity truth remains stale for SDK UploadSession, `upload_writefromfile`, tree public links, specialty public links, and crypto share/team-share reachability.

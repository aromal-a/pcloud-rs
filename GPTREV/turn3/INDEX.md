# pcloud-rs GPT Review Turn 3 Index

Generated: 2026-04-30

Master prompt: `pcloud_rev.md`

Output policy: subagents used the master audit prompt with write target overridden from `AUDIT_REPORT.md` to `GPTREV/turn3/`.

## Reports

- `01_parity_api_cli_sdk.md` - parity matrix/API truth, CLI/SDK reachability, row status/count drift.
- `02_security_crypto_transport.md` - security, secret discipline, crypto/KMS, auth vault, TLS/transport.
- `03_sync_fuse_data_integrity.md` - sync planner/runtime, FUSE write path, journal recovery, platform mount integrity.
- `04_ipc_daemon_web_config_ops.md` - IPC, daemon lifecycle, web management, config mutation, service behavior.
- `05_testing_ci_deployment_docs.md` - testing/CI/live E2E, deployment, packaging, observability, docs truthfulness.
- `06_code_quality_dependency_inventory.md` - fmt/clippy/check/deny/audit/MSRV, unsafe/panic/silent-drop/dependency posture.

## Highest-Risk Turn 3 Findings

- Production config can select development API mode, bypassing real TLS/auth paths.
- HashiCorp Vault KMS accepts `http://` URLs while carrying Vault tokens and plaintext DEKs.
- Live E2E can pass by skipping required live behavior, including crypto rotation and FUSE gates.
- Docker packaging is not buildable/runnable as described due to stale Rust version, args, env vars, and labels.
- Sync planning replaces remote/local work in separate passes, and background uploads do not read source files.
- FUSE write-journal replay only logs records, and checkpointing can erase records for other dirty files.
- The RSA Marvin RustSec advisory remains reachable in production crypto paths and `cargo audit` fails.
- Parity truth surfaces disagree on current counts and row 93 status/shape.

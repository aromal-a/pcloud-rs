# pcloud-rs GPT Review Index

Generated: 2026-04-29

Master prompt: `pcloud_rev.md`

Output policy: subagents treated the master audit instructions as authoritative, with the lead-agent write target overridden from `AUDIT_REPORT.md` to `GPTREV/`.

## Reports

- `01_parity_api_coverage.md` - C-to-Rust parity, matrix truth, API reachability, rejected/partial row coverage.
- `02_security_secret_transport.md` - secret handling, auth vault, transport policy, IPC timeout/security, logging/redaction risks.
- `03_crypto_subsystem.md` - crypto/KMS/interoperability, password rotation, PclsyncCompat, temppass/share flows, FIPS posture.
- `04_sync_engine_store_resilience.md` - sync engine, store/cache, retry/resilience, durable queue, conflict handling.
- `05_fuse_mount_drive.md` - mounted-drive/FUSE parity, platform implementations, write path, journal, orphan/signal handling.
- `06_ipc_daemon_web_config.md` - IPC, daemon lifecycle, web management surface, config/session/auth integration.
- `07_cli_sdk_public_surface.md` - CLI parser/help/completions, SDK API surface, examples/tests/features, semver exposure.
- `08_testing_ci_qa.md` - tests, live E2E, fuzz/bench coverage, CI matrix, weak/skipped tests.
- `09_deployment_ops_docs.md` - packaging, service definitions, observability, ops docs, platform claims, release docs.
- `10_code_quality_inventory.md` - fmt/clippy/deny/MSRV, unwrap/expect, unsafe, silent drops, stubs, raw IDs.

## Highest-Risk Themes

- Deployment readiness is blocked by invalid Linux user-service guidance and overstated packaging/signing documentation.
- Sync and FUSE have data-integrity blockers around local upload payloads, unexecuted directory/delete ops, and non-crash-safe write journaling.
- Crypto orchestration has lockout/desync risk because password rotation mutates local state before durable server success.
- Local management surfaces need stronger authorization and web-token enforcement beyond same-UID or loopback assumptions.
- QA gates are not strong enough for release readiness: live E2E, FUSE, fuzzing, cross-platform, and optional-feature jobs are advisory or incomplete.
- Current quality gates fail: `cargo fmt --all --check` and clippy with `-D warnings` are not clean in the audited worktree.

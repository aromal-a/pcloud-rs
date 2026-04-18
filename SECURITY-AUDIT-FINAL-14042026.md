# Security Audit — Final Wave 9 Record (14 April 2026)

> **Status**: This file is the wave 9 audit summary stub. The full per-section
> findings have been superseded and extended by the structured audit records in
> [`.audits/02/`](./.audits/02/). For the most complete and up-to-date security
> findings, consult those files directly.

## Overall Posture (Wave 9 Assessment)

The overall posture was assessed **top-of-the-line for the retained and
implemented surface**, with two open findings at close of wave 9:

- **H6-1 (High, Carried)** — `fuser 0.15.1` RUSTSEC-2021-0154 (unsound).
  No patched upstream `fuser` release exists. Mitigated via time-boxed ignore
  in `audit.toml`, scoped to `bd-1du.4`. Details in
  [`.audits/02/section-05-fuse.md`](./.audits/02/section-05-fuse.md).

- **L1 (Low, New wave 9)** — `dwltag` cookie value not CRLF/whitespace-validated.
  File: `crates/pcloud-fs/src/http_download.rs` (`build_request`). Planned fix:
  reject any byte outside the token-safe range (`0x21..=0x7E`) for `dwltag`,
  `host`, `path` before embedding. Details in
  [`.audits/02/section-06-transport.md`](./.audits/02/section-06-transport.md).

## Carried-Open Findings

See H6-1 above.

## New Findings (Wave 9)

See L1 above.

## Supersession Notice

This stub exists to satisfy references from `SECURITY.md`, `CONTRIBUTING.md`,
`README.md`, `CHANGELOG.md`, `AUDIT_REPORT.md`, and archive docs. The
per-section detail has moved to `.audits/02/`:

| Section | File |
|---------|------|
| Parity | [`.audits/02/section-01-parity.md`](./.audits/02/section-01-parity.md) |
| Security | [`.audits/02/section-02-security.md`](./.audits/02/section-02-security.md) |
| Crypto | [`.audits/02/section-03-crypto.md`](./.audits/02/section-03-crypto.md) |
| Sync engine | [`.audits/02/section-04-sync-engine.md`](./.audits/02/section-04-sync-engine.md) |
| FUSE | [`.audits/02/section-05-fuse.md`](./.audits/02/section-05-fuse.md) |
| Transport | [`.audits/02/section-06-transport.md`](./.audits/02/section-06-transport.md) |
| IPC / daemon | [`.audits/02/section-07-ipc-daemon.md`](./.audits/02/section-07-ipc-daemon.md) |
| CLI / SDK | [`.audits/02/section-08-cli-sdk.md`](./.audits/02/section-08-cli-sdk.md) |
| Quality / testing | [`.audits/02/section-09-10-quality-testing.md`](./.audits/02/section-09-10-quality-testing.md) |
| Deployment / docs | [`.audits/02/section-11-12-deployment-docs.md`](./.audits/02/section-11-12-deployment-docs.md) |

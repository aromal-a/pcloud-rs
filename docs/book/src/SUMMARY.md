# Table of Contents

[Introduction](./introduction.md)

# Getting Started

- [Installation](./getting-started/install.md)
- [First Login](./getting-started/first-login.md)
- [First Sync](./getting-started/first-sync.md)

# Architecture

- [Overview](./architecture/overview.md)
- [Crate Map](./architecture/crate-map.md)
- [Request Lifecycle](./architecture/request-lifecycle.md)
- [Performance](./architecture/performance.md)
- [Platform Support](./architecture/platform-support.md)
- [Security Model (Architecture Slice)](./architecture/security-model.md)
- [Decision Records](./adr/index.md)
  - [0001 — Record Format](./adr/0001.md)
  - [0002 — IPC Socket Framing](./adr/0002.md)
  - [0003 — Sync Mutex Choice](./adr/0003.md)
  - [0004 — Panic Guard Default-On](./adr/0004.md)
  - [0005 — Token Vault Layout](./adr/0005.md)
  - [0006 — No Update Check](./adr/0006.md)
  - [0007 — Crypto Password Not Persisted](./adr/0007.md)
  - [0008 — Streaming Download Buffer Size](./adr/0008.md)
  - [0009 — Parity Matrix Truth Source](./adr/0009.md)
  - [0010 — FUSE Write-Path Daemon Wiring Pending](./adr/0010.md)

# Security

- [Security Model](./security/model.md)
- [Secrets](./security/secrets.md)
- [Threat Model](./security/threat-model.md)
- [External Audit Dossier](./security/audit-dossier.md)

# Operations

- [Deployment](./operations/deployment.md)
- [Runbook](./operations/runbook.md)
- [Upgrade](./operations/upgrade.md)
- [RC Soak (30-day)](./operations/rc-soak.md)
- [Partial Transfers (Resume)](./operations/partial-transfers.md)
- [Web UI](./operations/web-ui.md)
- [Integrity Sweeper](./parity/integrity-sweeper.md)
- [Backup Snapshots](./operations/backup-snapshots.md)
- [Packaging Matrix](./operations/packaging-matrix.md)
- [Platforms]()
  - [Linux](./operations/platforms/linux.md)
  - [macOS](./operations/platforms/macos.md)
  - [Windows](./operations/platforms/windows.md)
  - [FreeBSD](./operations/platforms/freebsd.md)
  - [OpenBSD](./operations/platforms/openbsd.md)
  - [NetBSD](./operations/platforms/netbsd.md)

# Development

- [Contributing](./development/contributing.md)
- [Adding a Command](./development/adding-a-command.md)
- [Testing](./development/testing.md)
- [Reproducible Builds](./development/reproducible-builds.md)
- [Release Checklist](./development/release-checklist.md)
- [Adding a Plugin](./development/adding-a-plugin.md)

# Reference

- [CLI](./reference/cli.md)
- [Configuration](./reference/config.md)
- [IPC Protocol](./reference/ipc-protocol.md)
- [Exit Codes](./reference/exit-codes.md)
- [Packaging](./reference/packaging.md)

# Enterprise

- [Overview](../../enterprise/README.md)
- [OIDC Identity Broker](../../enterprise/oidc-broker.md)
- [Policy Layer (OPA/Rego)](../../enterprise/policy.md)
- [Fleet Management Agent](../../enterprise/fleet.md)
- [Data Residency](../../enterprise/data-residency.md)
- [Disaster Recovery](../../enterprise/disaster-recovery.md)
- [Data Loss Prevention](../../enterprise/dlp.md)
- [High Availability](../../enterprise/ha.md)
- [Key Management Service](../../enterprise/kms.md)
- [Distributed Tracing](../../enterprise/tracing.md)

# Plugins

- [Plugin Overview](../../plugins/README.md)
- [Autoheal](../../plugins/autoheal.md)
- [Backup Schedule](../../plugins/backup-schedule.md)
- [DLP Built-in](../../plugins/dlp-builtin.md)
- [Public-Link Expiry](../../plugins/publink-expiry.md)

# Parity

- [C-to-Rust Status](./parity/status.md)
- [bd-1du.10 Closure Checklist](../../parity/bd-1du-10-closure-checklist.md)
- [Roadmap (Complete Wave Summary)](../../roadmap-complete.md)

# Archive

- [Historical Reviews](./archive/index.md)

# FAQ

- [Frequently Asked Questions](./faq.md)

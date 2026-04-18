# pcloud-rs-rust-dev

Developer entry point for the Rust rewrite of the pCloud client. Cross-platform
target (Linux Tier 1; macOS + Windows Tier 2; FreeBSD/NetBSD/OpenBSD Tier 3).
The legacy C/C++ client has been removed from this fork; its sources remain
available upstream at
[`github.com/pcloudcom/pcloudcc`](https://github.com/pcloudcom/pcloudcc)
for historical reference only. Parity-matrix citations to `pclsync/*.c` paths
in this repo point at that upstream tree.

Single source of truth for parity counts: [`STATUS.md`](./STATUS.md).
Shipped history: [`CHANGELOG.md`](./CHANGELOG.md). Disclosure policy:
[`SECURITY.md`](./SECURITY.md). Dev rules: [`CONTRIBUTING.md`](./CONTRIBUTING.md).

> The rewrite explicitly does **not** claim "full parity", "production
> ready", "enterprise ready", or "drop-in replacement" until `bd-1du.10`
> is satisfied by code, tests, docs, and matrix evidence. See `CLAUDE.md`
> at the repo root for the honesty rules.

## Workspace Layout

```

├── crates/            # 27 crates (see Crate Map below)
├── docs/book/         # mdBook — developer + operator handbook
├── packaging/         # per-platform packaging (deb, rpm, homebrew, …)
├── fuzz/              # aggregate fuzz harness entry
├── tests/             # cross-crate integration tests
├── C_FEATURE_PARITY_MATRIX.csv
├── C_FEATURE_PARITY_REVIEW.md
├── STATUS.md
├── CHANGELOG.md
├── SECURITY.md
├── CONTRIBUTING.md
├── ARCHITECTURE.md
├── SECURITY-MODEL.md
├── API-REFERENCE.md
├── OPERATIONS-RUNBOOK.md
├── ERROR-TAXONOMY.md
├── TESTING-FUZZ-STRESS.md
└── PLAN_CROSSPLATFORM.md
```

## Build, Test, Docs

```bash
# Release build (locked dependencies)
cargo build --release --workspace --locked

# Full test suite
cargo test --workspace

# Clippy + format gate
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

# Supply-chain gates
cargo deny --manifest-path Cargo.toml check
cargo audit

# Generated rustdoc
cargo doc --workspace --no-deps --open
```

All of the above must be green before a PR merges. See
[`CONTRIBUTING.md`](./CONTRIBUTING.md) for the full daily workflow.

## Serve the mdBook

```bash
cd docs/book
mdbook serve           # http://localhost:3000
# or, for a static build:
mdbook build
```

The book covers architecture, per-platform integration chapters, ADRs,
operations runbook, security model, and the request lifecycle.

## Run the Daemon + CLI

```bash
# Long-running daemon (binds an owner-only UNIX socket)
cargo run -p pcloud-daemon -- serve

# Local CLI — health and auth
cargo run -p pcloud-cli -- health
cargo run -p pcloud-cli -- login --user alice@example.com --password-stdin

# Sync root management
cargo run -p pcloud-cli -- sync add ~/Documents /Drive/Documents
cargo run -p pcloud-cli -- sync list
cargo run -p pcloud-cli -- sync suggest ~/              # candidate directories
cargo run -p pcloud-cli -- sync is-syncable ~/Projects  # check before add

# Crypto folder
cargo run -p pcloud-cli -- crypto start
cargo run -p pcloud-cli -- crypto status
cargo run -p pcloud-cli -- crypto hint
cargo run -p pcloud-cli -- crypto change-password       # interactive prompts

# Account management
cargo run -p pcloud-cli -- account api-servers
cargo run -p pcloud-cli -- account set-language en
cargo run -p pcloud-cli -- account change-password

# Downloads
cargo run -p pcloud-cli -- download link 123456         # print signed URL
cargo run -p pcloud-cli -- download file 123456 ~/Downloads/report.pdf

# Backup
cargo run -p pcloud-cli -- backup list
cargo run -p pcloud-cli -- backup delete 42

# Cross-platform migration assistant
cargo run -p pcloud-cli -- migrate-from-c
```

Full command reference: [`docs/book/src/reference/cli.md`](./docs/book/src/reference/cli.md) and
`man pcloudc` (source: `packaging/man/pcloudc.1`).

## Run the Web UI

```bash
cargo run -p pcloud-web
# default: http://127.0.0.1:8080 (localhost bind only)
```

The Web UI is an MVP scaffold tracked under PLAN_A_PLUS §P4.5 and is not
exposed on non-loopback interfaces by default.

## Crate Map

27 crates, grouped by layer. Full per-crate purpose and public API lives in
the [mdBook crate-map chapter](./docs/book/src/architecture/crate-map.md);
each crate also carries its own `README.md`.

### Domain & primitives

- [`pcloud-model`](./crates/pcloud-model/) — shared domain types and newtype IDs.
- [`pcloud-error`](./crates/pcloud-error/) — shared error taxonomy.
- [`pcloud-secret`](./crates/pcloud-secret/) — zeroize-on-drop secret wrappers.
- [`pcloud-config`](./crates/pcloud-config/) — typed config loader/validator.

### Protocol & transport

- [`pcloud-proto`](./crates/pcloud-proto/) — typed API clients (TLS-only).
- [`pcloud-ipc`](./crates/pcloud-ipc/) — owner-only local IPC codec.
- [`pcloud-resilience`](./crates/pcloud-resilience/) — retry, circuit breaker, bandwidth pacer.

### State & persistence

- [`pcloud-store`](./crates/pcloud-store/) — SQLite store (`0600`).
- [`pcloud-cache`](./crates/pcloud-cache/) — in-memory cache primitives.
- [`pcloud-auth`](./crates/pcloud-auth/) — session state machine.

### Engines & backends

- [`pcloud-engine`](./crates/pcloud-engine/) — sync engine.
- [`pcloud-crypto`](./crates/pcloud-crypto/) — AES-256-GCM crypto folder.
- [`pcloud-fs`](./crates/pcloud-fs/) — FUSE adapter + cross-platform mount traits.
- [`pcloud-backends`](./crates/pcloud-backends/) — backend modules extracted from the daemon.
- [`pcloud-p2p`](./crates/pcloud-p2p/) — P2P LAN scaffolding (disabled by default).

### Runtime & interfaces

- [`pcloud-daemon`](./crates/pcloud-daemon/) — `pcloudd` binary + runtime.
- [`pcloud-daemon-win`](./crates/pcloud-daemon-win/) — Windows Service wrapper.
- [`pcloud-cli`](./crates/pcloud-cli/) — `pcloudc` CLI.
- [`pcloud-sdk`](./crates/pcloud-sdk/) — embeddable SDK facade.
- [`pcloud-web`](./crates/pcloud-web/) — MVP Web UI.
- [`pcloud-plugin-api`](./crates/pcloud-plugin-api/) — plugin manifest + signature API.

### Observability, testing, compat

- [`pcloud-observability`](./crates/pcloud-observability/) — metrics/health/audit.
- [`pcloud-chaos`](./crates/pcloud-chaos/) — fault-injection harness.
- [`pcloud-live-e2e`](./crates/pcloud-live-e2e/) — opt-in live-API tests.
- [`pcloud-mockserver`](./crates/pcloud-mockserver/) — in-process API mock.
- [`pcloud-compat`](./crates/pcloud-compat/) — legacy C-CLI RPC/shm compat shim.

## Live Verification

Live auth tests are `#[ignore]` and require explicit environment opt-in so
secrets never leak into CI:

```bash
PCLOUD_LIVE_AUTH_TOKEN=... \
  cargo test -p pcloud-daemon --test live_auth -- --ignored
```

See [`crates/pcloud-live-e2e/README.md`](./crates/pcloud-live-e2e/README.md)
for the full env var matrix and transport-override rules.

## Security Posture (short form)

- Secrets wrapped in `SecretString` / `SecretBytes` (zeroize on drop, redacted `Debug`).
- Auth vault: `0600` file / `0700` parent; passwords **never** persisted.
- IPC: UNIX socket `0600`, `SO_PEERCRED` UID check, per-connection timeouts.
- Transport: central `ApiEndpoint::validate(environment)` rejects plaintext
  in production; no TLS-bypass flag anywhere.
- Mount: `allow_other && !read_only` rejected; no `allow_root` / `setuid`.
- Telemetry / auto-update: **none**. See root README.

Full model: [`SECURITY-MODEL.md`](./SECURITY-MODEL.md). Latest audit:
[`SECURITY-AUDIT-FINAL-14042026.md`](./SECURITY-AUDIT-FINAL-14042026.md).

## Licence

Dual-licensed under **MIT OR Apache-2.0** —
[`LICENSE-MIT`](./LICENSE-MIT), [`LICENSE-APACHE`](./LICENSE-APACHE).

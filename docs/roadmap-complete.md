# Roadmap — Complete Wave Summary

_Last updated: 2026-04-15._

This document is the **chronological, landed-work** summary of every wave
that shipped into the Rust rewrite, from the P0 hardening wave through the
H-phase-2 enterprise expansion. It is deliberately written after-the-fact:
everything listed here is already in-tree, in-test, and in-CI.

Parity-matrix counts are **not** affected by anything described in this
document. See [`../STATUS.md`](../STATUS.md) for current counts. The
honesty rules in [`CLAUDE.md`](../../CLAUDE.md) still apply: no "full
parity", no "production ready", no "enterprise ready", no "drop-in
replacement" until `bd-1du.10` is gated shut.

---

## Wave P0 — Safety Hardening

Focus: eliminate silent failures, stop heap corruption classes, raise the
floor on cryptographic verification of downloads, wire the first kernel
FUSE end-to-end, and scaffold the mdBook.

Why this wave first: the prior C heap-corruption findings (commits
`7595c48`, `9773ed2`) set the bar for "don't regress on memory safety"
and we wanted a mechanical proof that the Rust side cannot silently
free-and-use-after. The circuit breaker also formalises the rule that
a transient backend failure degrades the daemon rather than panicking
through it.

Landed (with file-level references):

- RAII circuit-breaker with `parking_lot` primitives
  (`crates/pcloud-resilience/src/`), removing poison-panic mutex
  semantics workspace-wide (ADR 0003).
- Page-cache lock discipline in `crates/pcloud-fs/src/page_cache.rs`
  — eliminates the "hold lock across I/O" patterns the C code had.
- `fetch_download_verified` in `crates/pcloud-proto/src/transfer_api.rs`
  with SHA-256 tail verification before the file is made visible to
  the caller; raises the floor for every download consumer.
- FUSE kernel e2e proof on Linux via
  `crates/pcloud-fs/tests/fuse_mount_integration.rs` (gated behind
  `PCLOUD_FUSE_LIVE=1` so non-libfuse CI stays green).
- IPC 1 MiB request cap in `crates/pcloud-ipc/src/frame.rs`
  (DoS surface reduction; ADR 0002 framing stays, the cap is new).
- mdBook scaffold (`docs/book/src/SUMMARY.md` +
  `getting-started/*.md`) — first shippable developer docs surface.

**Test-count delta:** +84 (unit + integration + first doctests).

---

## Wave P1 — Throughput & Observability

Focus: make the daemon fast under load, expose enough telemetry to run it
in anger, and bolt on the first chaos harness.

Landed:

- O(1) LRU eviction in the page cache
- Upload journal: NDJSON + fsync crash-safe append
- `/proc/self/mountinfo` orphan detection + `pcloudc mount --force-umount`
- Streaming download path (`fetch_download_verified_streaming`)
- `pcloud-chaos` crate (fault injection primitives)
- SLO registry + `/slo` HTTP endpoint

**Test-count delta:** +142 (chaos harness, SLO endpoint, journal
property tests).

---

## Wave P2 — Test Substrate

Focus: make regressions visible. No new runtime features; pure coverage,
property testing, fuzzing, and doctests.

Landed:

- Coverage CI (`cargo llvm-cov` with per-crate gates)
- Property tests across protocol, crypto, store, engine crates
- Nightly fuzz sweep (libFuzzer + cargo-fuzz)
- +136 doctests across public APIs
- Weekly `cargo-mutants` mutation run

**Test-count delta:** +136 doctests, ~+300 property-test cases.

---

## Wave P3 — Operator Trust

Focus: make the daemon explainable. Lifecycle walkthroughs, manpages,
rustdoc enforcement, and the first ADRs.

Landed:

- Request-lifecycle walkthrough (`architecture/request-lifecycle.md`)
- Full manpages (`pcloudc.1`, `pcloudd.1`, `pcloud-cli.1`) + `manpage-lint`
- `#![deny(missing_docs)]` on 9 public crates
- First 10 ADRs (0001 record format through 0010 FUSE write-path pending)
- Runbook playbooks (deployment, upgrade, emergency, RC soak)

**Test-count delta:** +58 (doctest gates, manpage-lint harness).

---

## Wave P4 — Self-Diagnostics & Web UI MVP

Focus: let the binary tell its own operator what's wrong, and ship a
minimal browser surface.

Landed:

- `pcloudc doctor` (config sanity, vault perms, socket perms, version
  drift, endpoint reachability, clock skew)
- `pcloud-web` MVP (static assets, auth bridge, partial-transfer
  dashboard scaffolding)
- Selective-sync path filters
- `Arc<Vec<u8>>` page-cache zero-copy handoff
- LTO profile split (`release` vs `release-lto`)
- `BandwidthPacer` per-direction token bucket
- Hot-path clone sweep (reduced allocations in transfer path)

**Test-count delta:** +92.

---

## Wave P5 — Packaging Matrix

Focus: make installation boring on every platform the tier policy cares
about.

Landed:

- `flake.nix` for Nix/NixOS
- Debian `nfpm.yaml`
- Homebrew formula stub
- RPM spec
- Docker image
- AppImage
- Flatpak manifest
- Snap / Chocolatey / winget / Scoop manifest stubs
- WiX installer for Windows
- BSD `rc.d` scripts (FreeBSD, NetBSD, OpenBSD)

**Test-count delta:** +18 (packaging smoke tests).

---

## Wave P6 (partial) — Cross-Platform Lifecycle

Focus: extend the trait abstractions so macOS, Windows, and the BSDs
compile and unit-test.

Landed:

- Trait abstractions X1–X6
- macOS `fuse-t` adapter (16 callbacks)
- Windows WinFSP adapter (17 callbacks)
- Windows Service wrapper
- `pcloudc migrate-from-c` helper
- Six mdBook platform chapters

**Test-count delta:** +64 unit tests (per-platform adapters).

---

## Wave H-phase-1 — Enterprise Foundation

Focus: stand up the crates enterprise deployments need before they will
even evaluate a sync client.

Why this shape: enterprise buyers don't look at a sync client as a
single unit. They evaluate the identity story (OIDC), the policy
story (OPA), the fleet story (mTLS + device identity), the key
material story (KMS), and the observability story (OTel) as separate
gates. Each landed crate targets exactly one of those gates and can
be reasoned about independently.

Landed crates (with design ADRs):

- `pcloud-idp` — OIDC identity broker. Hand-rolled PKCE S256 (ADR
  0014), RS256-only JWKS with 1-hour TTL cache, pCloud trusted-issuer
  exchange stubbed until the upstream endpoint ships. See
  `docs/enterprise/oidc-broker.md`.
- `pcloud-policy` — OPA/Rego evaluation via `regorus` (ADR 0013).
  Default-deny, file-perm guard, transactional hot-reload. Example
  bundles in `crates/pcloud-policy/examples/policies/`. See
  `docs/enterprise/policy.md`.
- `pcloud-fleet` — fleet agent + enrolment. ed25519 device identity
  under `SecretBytes` with the `0600`/`0700` rule (ADR 0015), explicit
  rustls `RootCertStore` (no system CAs), canonical-JSON body
  signatures. See `docs/enterprise/fleet.md`.
- `pcloud-kms` — AWS KMS + HashiCorp Vault Transit providers with an
  in-memory unwrap cache; Pkcs11Hsm remains a stub. See
  `docs/enterprise/kms.md`.
- `pcloud-session` — session reauth coordinator bridging TFA
  re-challenge across SDK consumers.

Landed features:

- OpenTelemetry distributed tracing via the `RequestEnvelope` wrapper
  (ADR 0012), with the daemon opening `pcloudd.dispatch` and
  `pcloudd.backend.<name>` spans.
- Data-residency enforcement in `pcloud-config` + `pcloud-daemon`:
  `[data_residency]` block, `PolicyViolation { kind: "data_residency" }`
  IPC error, 1h region-resolver TTL cache.
- Disaster-recovery + high-availability playbooks under
  `docs/enterprise/disaster-recovery.md` and `docs/enterprise/ha.md`.
- External audit dossier at `docs/book/src/security/audit-dossier.md`.
- 30-day RC soak playbook at `docs/book/src/operations/rc-soak.md`.

**Test-count delta:** +210 (new crates carry their own unit + property
tests; tracing spans cross-checked by integration tests).

---

## Wave H-phase-2 — Enterprise Expansion

Focus: bolt on the plugin surface, partial-transfer resume, backup
snapshots, and the integrity sweeper scaffolding.

Landed:

- Four first-party plugins: `autoheal`, `backup-schedule`,
  `dlp-builtin`, `publink-expiry`
- `RequestEnvelope` unified across daemon + SDK
- Backup snapshot CLI + scheduler integration
- Integrity sweeper (config block, skip-list parser, rate limiter;
  background scrub wiring deliberately left for `bd-1du.4.6.1`)
- Partial-transfer resume (download + upload; CLI `pcloudc resume`)
- `pcloud-web` expansion (admin panel, partial-transfer dashboard)
- DLP enforcement module (`docs/enterprise/dlp.md`)

**Test-count delta:** +176 (plugin registry tests, envelope round-trips,
resume state-machine properties, snapshot lifecycle tests).

---

## What this roadmap explicitly does **not** change

- The parity-matrix totals. See [`../STATUS.md`](../STATUS.md).
- The open beads `bd-1du`, `bd-1du.4`, `bd-1du.4.6.1`, `bd-1du.10`.
- The honesty claims in `CLAUDE.md`.

When the mounted-drive runtime (`bd-1du.4`) lands with a live host-run
proof, **then** `fs,mounted pcloud filesystem` may flip. When the
chunked-upload state machine lands in the daemon, **then** the upload
wire-method + `SDK UploadSession` rows may flip. Until those code paths
exist and are live-verified, the matrix stays as it is and the claims in
`CLAUDE.md` stand.

## Related documents

- [`../STATUS.md`](../STATUS.md) — authoritative parity counts
- [`../C_FEATURE_PARITY_REVIEW.md`](../C_FEATURE_PARITY_REVIEW.md) — narrative
- [`../C_FEATURE_PARITY_MATRIX.csv`](../C_FEATURE_PARITY_MATRIX.csv) — matrix
- [`./parity/bd-1du-10-closure-checklist.md`](./parity/bd-1du-10-closure-checklist.md)
- [`./parity/integrity-sweeper.md`](./parity/integrity-sweeper.md)
- [`./enterprise/README.md`](./enterprise/README.md)
- [`./plugins/README.md`](./plugins/README.md)

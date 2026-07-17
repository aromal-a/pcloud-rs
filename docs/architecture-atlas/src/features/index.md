# Complete feature encyclopedia

This encyclopedia explains the entire current pcloud-rs feature surface: the
ordinary single-account drive, every client interface, synchronization and
mounting, all cryptography, sharing and multi-account work, enterprise
controls, platform adapters, internal runtime services, developer helpers,
and verification infrastructure.

It deliberately distinguishes four questions that product lists often mix:

1. **Does source exist?** The source-unit and declaration catalogs answer this.
2. **Is it wired into a real entrypoint?** The curated chapters and API catalog
   identify the daemon, CLI, SDK, protocol, plugin, or test path.
3. **Is it intended for users?** Internal helpers, compatibility adapters, and
   test harnesses are labeled rather than advertised as end-user features.
4. **Is it release-qualified?** Native/live evidence is separate from an
   implementation claim. See [Truth, maturity, and scope](../truth-and-scope.md).

## Feature map

```text
single user
  ├── login / TFA / token vault / account utilities
  ├── browse / stat / mkdir / move / copy / delete
  ├── upload / download / public links / notifications
  ├── sync / backup / snapshots / mounted drive
  └── Crypto folders and local secret protection
             │
collaboration│
  ├── contacts / invitations / incoming and outgoing shares
  ├── share permissions / team shares / public-link access lists
  └── multi-account supervisor scaffold
             │
enterprise   │
  ├── policy / DLP / residency / KMS-HSM / IdP
  ├── fleet / HA / audit chain / tracing / SLOs
  └── plugins / disaster recovery / controlled deployment
             │
internal foundation
  ├── typed model / error / config / protocol / IPC
  ├── daemon runtime / backends / store / cache / journals
  ├── resilience / observability / platform adapters
  └── mock / live E2E / chaos / fuzz / benchmarks / xtask
```

The layers are additive. Enterprise features do not replace the personal
drive; they add controls around the same daemon-owned operations. Null/default
implementors keep optional management systems out of the single-user path.

## Curated tours

| Area | What is explained |
|---|---|
| [Personal cloud and account](personal-cloud.md) | Login, sessions, account lifecycle, remote namespace, settings, notifications, links, and everyday file management |
| [Transfers, sync, backup, and mount](sync-mount-transfer.md) | Byte movement, resume/integrity, reconciliation, conflict policy, backups, snapshots, cache, and every native filesystem path |
| [Cryptography, secrets, and key custody](crypto.md) | Both Crypto backends, every primitive/helper, password and key lifecycle, vaults, sharing, KMS/HSM, and FIPS posture |
| [Sharing, multi-user, and enterprise](collaboration-enterprise.md) | People/teams, public access, multi-account isolation, OIDC, policy, DLP, residency, fleet, HA, DR, and plugins |
| [Interfaces and automation](interfaces-automation.md) | CLI, public and embedded SDKs, web, WebDAV, typed IPC, protocol clients, compatibility, and scripted use |
| [Runtime and internals](runtime-internals.md) | Models, errors, configuration, daemon composition, storage, cache, engines, resilience, telemetry, and helper ownership |
| [Platforms and operations](platform-operations.md) | Linux, macOS, Windows, BSD, Solaris-family, NAS intent, packaging, services, upgrades, and operational controls |
| [Verification and developer features](verification-helpers.md) | Unit/integration/live/chaos/fuzz/benchmark layers, mock server, CI orchestration, reproducibility, and generated indexes |

## Exhaustive machine-checked catalogs

The curated chapters group related behavior so a human can understand it. The
generated catalogs close the exhaustiveness gap:

| Catalog | Completeness contract |
|---|---|
| [Package feature families](../generated/features/package-families.md) | Every package returned by current Cargo metadata, with rationale, best use, strengths, maturity, and package entrypoint |
| [API and compatibility capabilities](../generated/features/api-capabilities.md) | Every row in `C_FEATURE_PARITY_MATRIX.csv`, including implemented operations and deliberate rejections |
| [Current CLI, IPC, SDK, and binary surfaces](../generated/features/current-surfaces.md) | Every live `Command`, `Method`, `Request`, Cargo binary, direct client construction site, daemon handler match, owner, side effect, rationale, and best use |
| [Cargo feature flags](../generated/features/cargo-flags.md) | Every declared package feature, including empty markers and compile-fail seams |
| [Internal modules and helpers](../generated/features/source-units.md) | Every Rust runtime module, binary, build helper, example, test, benchmark, and fuzz unit owned by a workspace package |
| [Crate declaration indexes](../generated/crates/index.md) | Every detected Rust declaration, including private helpers, with file and line entrypoint |
| [Complete file inventory](../generated/inventory/index.md) | Every tracked or unignored project file, including packaging, operations, scripts, docs, and vendored material |

Together these views prevent a polished overview from silently omitting a
small feature flag, internal service, platform shim, test-only tool, or
compatibility adapter.

## How each feature is described

Every curated feature family answers the same questions:

- **What:** the behavior and owner.
- **Why it exists:** the problem or risk that justified it.
- **Good for:** the user, operator, implementer, or integration scenario.
- **Why it is good at that job:** the concrete design properties and
  invariants, not marketing adjectives.
- **Entrypoint:** the crate/module or executable where implementation begins.
- **Maturity:** public, internal, evolving, experimental, verification-only,
  or externally unqualified.

## Important honesty rules

- “Implemented” does not mean a release package has been installed and tested
  on every target.
- A Cargo flag may be a dependency switch, test helper, empty marker, or
  intentional compile-time refusal; its name alone is not capability proof.
- The stable third-party Rust contract is `pcloud-sdk`. The broader
  `pcloud-embedded-sdk` is a first-party internal compatibility surface.
- WebDAV, P2P, the Windows SCM host, multi-account supervisor, and much of the
  enterprise/plugin track are bounded or experimental unless their chapter
  says otherwise.
- pcloud-rs is not FIPS 140 validated. The FIPS-named Cargo seam intentionally
  fails until a validated provider is actually integrated.
- The cloud remains authoritative. Caches, LAN peers, and local indexes are
  accelerators, never independent truth sources.

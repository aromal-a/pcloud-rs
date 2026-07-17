# pcloud-rs Architecture & Feature Atlas

This is the source-derived map and complete feature encyclopedia of the
pcloud-rs workspace: every user-facing capability, enterprise and multi-user
surface, cryptographic mechanism, compile-time switch, internal runtime unit,
test/helper, entrypoint, state owner, and platform qualification boundary.

## Explore every feature

Start with the [complete feature encyclopedia](features/index.md). It combines
plain-language, rationale-led chapters with four generated completeness views:

- [all API and compatibility capabilities](generated/features/api-capabilities.md),
  projected from every row of the canonical parity matrix;
- [all current CLI, IPC, SDK, and binary surfaces](generated/features/current-surfaces.md),
  generated from the live Rust enums and Cargo targets with routing and reachability;
- [all Cargo package feature families](generated/features/package-families.md)
  and [all feature flags](generated/features/cargo-flags.md);
- [all internal modules and helpers](generated/features/source-units.md), with
  their role, rationale, best use, and links to declaration-level references;
- curated tours from [personal cloud](features/personal-cloud.md),
  [sync/mount/transfer](features/sync-mount-transfer.md), and
  [cryptography](features/crypto.md) through
  [multi-user and enterprise](features/collaboration-enterprise.md),
  [interfaces](features/interfaces-automation.md),
  [runtime internals](features/runtime-internals.md),
  [platform operations](features/platform-operations.md), and
  [verification infrastructure](features/verification-helpers.md).

The atlas is written for four audiences:

| Audience | Start here | What you should leave with |
|---|---|---|
| Product evaluator | [Feature encyclopedia](features/index.md) | What exists, why it exists, where it excels, and what is not yet qualified |
| CLI/API user | [Standalone use](standalone-library.md) | Which process to start and which interface to call |
| Library implementer | [Entrypoints](entrypoints.md) and [RemoteFs](remote-fs.md) | The stable SDK boundary and canonical remote-drive contract |
| Core developer | [System overview](system-overview.md) and [Request paths](request-paths.md) | Crate ownership, call paths, and extension seams |
| Sysop/package maintainer | [Operations and platforms](operations-platforms.md) | Runtime state, lifecycle, packaging, and qualification gates |

## The shortest accurate mental model

```text
 human / program / mounted filesystem
          │
          ├── pcloudc ───────────────┐
          ├── pcloud-sdk 1.x ────────┤ owner-authenticated local IPC
          ├── pcloud-web ────────────┤
          └── experimental WebDAV ───┘
                                      ▼
                                 pcloudd
                      policy, auth, state, scheduling
                                      │
          ┌───────────────────────────┼───────────────────────────┐
          ▼                           ▼                           ▼
   canonical RemoteFs           sync / mount engines       SQLite + journals
          │                           │
          └──────────────┬────────────┘
                         ▼
                 typed pCloud protocol
                         │ TLS
                         ▼
                    pCloud service
```

`pcloudd` is the composition root and trust boundary. `RemoteFs` is the
canonical, live, ID-first remote namespace used by drive-like consumers.
The public `pcloud-sdk` is a focused blocking IPC client. The broader
`pcloud-embedded-sdk` is an unpublished first-party compatibility surface.
Kernel mounts are platform adapters over the same daemon-owned remote
semantics; they are not a second source of truth.

## Truth labels used throughout

<span class="atlas-supported">Implemented path</span>
means that source and tests demonstrate an implementation path.

<span class="atlas-experimental">Experimental / unshipped</span>
means the component exists but is not part of the supported public product.

<span class="atlas-unqualified">Externally unqualified</span>
means a native runner, real credentials, hardware, signing identity, package
install, or registry publication is still required. Source presence is not
release evidence.

The repository currently states that there is no public release and makes no
“production ready”, “full parity”, or “drop-in replacement” claim. See
[Truth, maturity, and scope](truth-and-scope.md) before treating any component
or platform table as a release promise.

## Atlas coverage

The generated reference contains:

- every Cargo package and target in the current workspace;
- every non-ignored file visible to Git, including untracked development
  files, with role classification and a short source-derived description;
- every Rust file in each crate;
- named Rust declarations found in each crate, including internal functions
  and methods, with visibility, source file, and line;
- workspace dependency and feature summaries;
- dedicated operator, implementation, security, durability, and platform
  views.

Regenerate after source changes:

```bash
python3 docs/architecture-atlas/tools/generate.py
mdbook build docs/architecture-atlas
```

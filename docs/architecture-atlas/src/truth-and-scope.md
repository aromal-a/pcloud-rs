# Truth, maturity, and scope

This atlas describes the current working tree, not an imagined final product.
That distinction matters because pcloud-rs contains mature core paths,
experimental integrations, release-candidate packaging, test harnesses, and
historical audit material in the same repository.

## Evidence hierarchy

Use this order when evaluating a claim:

1. Current implementation and its executable tests.
2. Current release-commit CI or native qualification result.
3. `STATUS.md`, the parity matrix, and current platform handbook.
4. Root and crate READMEs.
5. Historical reviews and audit archives.

Historical review folders are evidence of past analysis, not current product
state. Generated inventory pages identify them as historical material.

## Maturity classes

| Class | Meaning | Representative surfaces |
|---|---|---|
| Stable public contract | SemVer boundary intended for third-party consumers | `pcloud-sdk` 1.0 source contract |
| Internal stable | Shared workspace contract; use public SDK externally | model, IPC, protocol, store, secrets, resilience, backends |
| Evolving product surface | Product code that may still change before release | daemon, CLI, filesystem, web, embedded SDK |
| Experimental / bounded | Optional, test, enterprise, plugin, or unshipped work | WebDAV, Windows SCM host, P2P, plugin hosts, fleet, KMS/IDP/policy |
| Verification support | Proves or exercises behavior; not shipped | mock server, chaos, live E2E, fuzz and DR harnesses |

The generated [crate index](generated/crates/index.md) applies one of these
labels to every package.

## Supported path versus release-qualified platform

An implementation can be portable in source and still lack public support
evidence. The release bar additionally needs the relevant combination of:

- clean, reproducible release commit;
- native build and tests;
- real package install, upgrade, start/stop, and uninstall;
- credentialed pCloud smoke tests;
- kernel mount tests where mounting is claimed;
- signed/notarized artifacts on macOS and Windows;
- hardware tests for NAS packages;
- published and installation-tested registry packages for the public SDK.

The [platform chapter](operations-platforms.md) keeps target intent and
verified evidence separate.

## Canonical versus compatibility surfaces

The architecture deliberately narrows authoritative paths:

- remote-drive behavior belongs to `pcloud_backends::RemoteFs`;
- external Rust consumers should use `pcloud-sdk`;
- the CLI, web UI, and WebDAV adapter call daemon IPC;
- sync and mount use daemon-owned `RemoteFs` composition;
- `pcloud-embedded-sdk` retains broader first-party compatibility helpers but
  is unpublished;
- the Windows SCM wrapper and WebDAV implementation remain experimental and
  unshipped.

When documentation and code disagree, follow the current implementation and
open a documentation correction. Do not silently turn an experimental path
into a support claim.

## Explicit non-goals of this atlas

This website does not:

- certify release readiness;
- replace rustdoc for exact generic signatures and trait bounds;
- publish crates or packages;
- claim live pCloud behavior without credentials;
- treat ignored build outputs as project files;
- treat vendored upstream code as pcloud-rs-owned architecture.

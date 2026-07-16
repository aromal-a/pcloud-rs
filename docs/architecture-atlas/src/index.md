# pcloud-rs Architecture Atlas

This is the source-derived map of the pcloud-rs workspace: what runs, what
calls what, where state lives, which entrypoints are intended for external
use, and which platform claims still require native qualification.

The atlas is written for four audiences:

| Audience | Start here | What you should leave with |
|---|---|---|
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

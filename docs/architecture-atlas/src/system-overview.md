# System overview

## Process architecture

```text
┌──────────────────────── client processes ────────────────────────┐
│                                                                  │
│  pcloudc          public pcloud-sdk          pcloud-web           │
│  short lived      blocking Rust library      optional HTTP UI     │
│      │                     │                       │               │
│      └─────────────────────┴───────────────────────┘               │
│                    typed pcloud-ipc requests                       │
└──────────────────────────────┬───────────────────────────────────┘
                               │
              AF_UNIX + peer UID / Windows named pipe + SID
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│ pcloudd — pcloud-daemon composition root                         │
│                                                                  │
│ bootstrap → native IPC accept → decode → dispatch → RuntimeShell │
│                                      │                           │
│      ┌───────────────┬───────────────┼──────────────┐            │
│      ▼               ▼               ▼              ▼            │
│   RemoteFs       auth/crypto      sync engine     policy/plugins │
│      │               │               │              │            │
│      ├──── folder / transfer / share backends ──────┘            │
│      │                       │                                   │
│      ▼                       ▼                                   │
│ pcloud-proto           store/cache/journals                      │
└──────┬────────────────────────┬──────────────────────────────────┘
       │ TLS                    │ local durable state
       ▼                        ▼
 pCloud APIs          SQLite, vault, upload and mount journals
```

`pcloud-daemon` owns process lifecycle and wires the lower-level crates
together. Business logic that is reusable without the process shell lives in
`pcloud-backends`. Typed remote API calls live in `pcloud-proto`. Long-lived
state lives in `pcloud-store`, while secrets stay behind `pcloud-secret` and
the daemon vault boundary.

## Layer ownership

| Layer | Owner | Responsibilities | Must not own |
|---|---|---|---|
| User presentation | CLI, web | parse/display, input UX, exit/HTTP mapping | remote credentials or authoritative state |
| Public library | `pcloud-sdk` | stable SDK-owned types and blocking RemoteDrive calls | raw daemon internals |
| Local protocol | `pcloud-ipc` | requests, responses, framing, peer identity, client/server transport | business policy |
| Composition/runtime | `pcloud-daemon` | bootstrap, dispatch, auth session, background loops, lifecycle | duplicate remote namespace semantics |
| Business backends | `pcloud-backends` | auth/account/folder/transfer/share/backup logic and `RemoteFs` | UI formatting |
| Engines | engine, fs, crypto | synchronization, mounted filesystem, content crypto | process-global configuration |
| Remote protocol | `pcloud-proto` | typed pCloud commands, binary encoding, TLS transport | CLI or persistence policy |
| Persistence | store/cache | migrations, repositories, bounded caches | remote truth |
| Foundations | model/error/secret/config | types, errors, secret hygiene, configuration | orchestration |

## Portable core and native seams

The portable core includes model, protocol, backends, RemoteFs, daemon
dispatch, CLI semantics, SDK, transfer and sharing behavior. Native seams are
kept narrow:

```text
portable daemon
    ├── pcloud-ipc::platform
    │     ├── Unix peer credentials
    │     ├── BSD/macOS getpeereid
    │     ├── Solaris getpeerucred
    │     └── Windows named pipe + TokenUser SID
    ├── pcloud-daemon::vault
    │     ├── Secret Service / owner-only file
    │     ├── macOS Keychain
    │     └── Windows DPAPI
    └── pcloud-fs::platform
          ├── Linux FUSE
          ├── macOS fuse-t
          ├── Windows WinFSP
          ├── BSD FUSE
          └── unsupported mount adapter on Solaris-family systems
```

This shape permits CLI/API/copy/share operation on a platform even when no
kernel mount adapter exists.

## Runtime construction

At a high level:

```text
pcloudd main
   │
   ├── parse serve / control mode
   └── serve_with_shutdown
          ├── install shutdown handlers
          ├── bootstrap configuration and directories
          ├── open store and repositories
          ├── initialize vault and restore allowed token state
          ├── compose protocol clients and backend runtimes
          ├── start sync/mount/metrics support as configured
          └── run native IPC server
                 └── each accepted request → RuntimeShell::handle_request
```

Use the generated [`pcloud-daemon` page](generated/crates/pcloud-daemon.md)
for its complete file and public-item index.

# Request and data paths

## CLI or SDK remote operation

Example: `pcloudc remote cp /A/x /B/x`.

```text
pcloudc
  app/command parser
      │ Request::RemoteCopy { from, to }
      ▼
pcloud-ipc client
  encode bounded typed frame
      │
      ▼ native owner-authenticated connection
pcloud-ipc server
  peer UID/SID check → decode
      │
      ▼
pcloud-daemon serve/dispatch
      │ RuntimeShell remote operation
      ▼
pcloud-backends::RemoteFs
  resolve source and destination using live folder listings
  reject recursive self-copy
  create destination folders / stream file ranges
      │
      ├── FolderRuntime → pcloud-proto folder API
      └── TransferRuntime → pcloud-proto transfer API
                              │
                              ▼ TLS
                          pCloud service
```

The SDK joins the same path at `pcloud-ipc client`; it does not link
`RemoteFs` directly.

## Authentication path

```text
CLI password/token/TFA input
        │ SecretString-bearing IPC variant
        ▼
owner-authenticated local transport
        ▼
daemon auth runtime
        ├── pcloud-auth state machine
        ├── pcloud-proto auth API
        └── optional token vault persistence
                 ├── owner-only file / Secret Service
                 ├── Keychain
                 └── user-scope DPAPI
```

Passwords are not persisted. Token persistence is explicit and subject to
vault policy. Secret wrappers redact debug output and zeroize their owned
buffers on drop.

## Sync path

```text
local filesystem events ─┐
                         ├── pcloud-engine planner/conflict logic
remote diff polling ─────┘                 │
                                          ▼
                                daemon sync loop runtime
                                          │
                        ┌─────────────────┴──────────────────┐
                        ▼                                    ▼
                  RemoteFs upload/download            pcloud-store
                        │                         cursor, roots, pending state
                        ▼
                    pCloud API
```

The engine decides *what should happen*. RemoteFs and transfer backends
perform remote byte and namespace operations. The store makes progress and
recovery explicit.

## Mounted filesystem path

```text
application read/write syscall
       │
       ▼
OS kernel filesystem layer
       │ FUSE / fuse-t / WinFSP
       ▼
pcloud-fs platform adapter
       │ portable filesystem traits
       ▼
daemon mount runtime
       ├── metadata/read → RemoteFs live stat/list/range read
       └── write → bounded local staging + journal
                         │ flush/fsync/replay
                         ▼
                  RemoteFs durable upload
```

The platform adapter translates OS callbacks. It does not define remote
namespace semantics. Mount success must include native discovery; unmount
must not report success while the kernel mount remains.

## Public-link and sharing path

Public-link APIs have dedicated pCloud protocol families and backend
runtimes. Folder sharing through the stable remote-drive contract attaches a
`SharesRuntime` to RemoteFs and resolves the folder to an ID before sending
the share mutation. More specialized business/team/crypto-share surfaces may
remain internal, partial, or outside the stable SDK.

## Error boundaries

Each layer translates errors instead of leaking one giant enum:

```text
socket/TLS error
   → protocol-family error
      → backend/RemoteFs error
         → daemon ResponseStatus + safe message
            → SDK Error / CLI exit code / HTTP response
```

When debugging, begin at the first boundary whose observable behavior is
wrong. A CLI exit-code issue is not automatically a protocol bug; a remote
result code is not automatically an IPC error.

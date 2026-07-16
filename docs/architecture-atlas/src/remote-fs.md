# RemoteFs: the canonical remote-drive boundary

`pcloud_backends::remote_fs::RemoteFs` exists to prevent each consumer from
inventing its own interpretation of paths, metadata caches, transfers, and
mutation semantics.

## Role

RemoteFs is a borrowed service object composed from:

```text
FolderRuntime ─────────┐
                      ├── RemoteFs(auth token)
TransferRuntime ───────┤       │
                      │       ├── live path resolution
optional SharesRuntime┤       ├── ID-first mutations
                      │       ├── bounded reads
durability context ───┘       └── resumable transfers
```

It accepts human-friendly absolute paths at the edge. It then resolves those
paths from live pCloud folder metadata and carries typed IDs internally:
`RemoteId::Folder(u64)` or `RemoteId::File(u64)`. A local metadata-cache miss
is never treated as proof that a remote entry is absent.

## Why it is an object

The object captures the already-configured runtimes and current secret token
without making them global. Its lifetime makes ownership explicit:

- folder operations reuse the daemon's folder transport and error policy;
- byte operations reuse the transfer runtime;
- sharing is only enabled when a `SharesRuntime` is attached;
- resumable upload/download methods require an explicit durability context;
- callers cannot accidentally bypass authentication or construct a second
  cache-backed namespace.

This is composition, not persistence: RemoteFs borrows its dependencies and
does not become a second runtime.

## Operation families

| Family | Public operations | Important invariant |
|---|---|---|
| Resolution | `resolve`, `stat`, `list` | live traversal, canonical absolute paths, typed IDs |
| Namespace mutation | `mkdir`, `delete`, `move_path`, `copy_path` | resolve first, mutate by ID, reject recursive self-copy |
| Bounded reads | `read_range`, `read_range_by_id` | one allocation is capped at 16 MiB |
| Generic streaming upload | `write_stream` | declared length is enforced; short and overlong readers fail |
| Explicit upload session | `begin_streaming_write`, `write_streaming_chunk`, `streaming_write_status`, `commit_streaming_write`, `abort_streaming_write` | session ID and acknowledged offsets remain visible |
| Durable upload | `upload_file_resumable`, `upload_file_resumable_to_parent` | resume record + journal, SHA-1 verification, conflict policy |
| Durable download | `download_to_path`, `download_by_id_streaming_to_path`, `download_by_id_to_path` | sidecar resume state, SHA-256, sync-before-publication |
| Sharing | `share_folder`, `share_folder_by_id` | only folders; optional sharing composition |

## Resolution algorithm

```text
input "/A/B/report.pdf"
       │
       ├── validate absolute canonical path
       ├── root metadata = Folder(0)
       ├── list Folder(0), find exactly one "A"
       ├── require Folder(A)
       ├── list Folder(A), find exactly one "B"
       ├── require Folder(B)
       ├── list Folder(B), find exactly one "report.pdf"
       └── return RemoteMetadata { id: File(...), canonical path, flags... }
```

Zero matches produces `NotFound`; multiple same-name matches produces
`Ambiguous` rather than guessing. File/folder mismatches are typed errors.

## Upload durability

```text
local file
   │ open + size + hash identity
   ▼
resume repository / upload journal lookup
   │
   ├── no compatible state → create pCloud upload session
   └── compatible state → query/continue acknowledged offset
                         │
                         ▼
                bounded sequential chunks
                         │ each acknowledgement persisted
                         ▼
                  verify complete SHA-1
                         │
                         ▼
                  commit remote file
                         │
                         └── clear durable resume state
```

Conflict behavior is explicit: overwrite, conditional overwrite by hash, or
create-if-new. The service retries classified transient failures without
turning non-idempotent operations into blind duplicates.

## Download durability

Downloads go to a partial path first. Resume metadata binds the partial file
to remote path, file ID, and expected size. Completion computes SHA-256,
flushes and syncs the local file, and only then publishes the final path.

## Consumers

```text
pcloudc ───────────┐
pcloud-sdk ────────┼── pcloud-ipc ── daemon RuntimeShell ── RemoteFs
pcloud-webdav* ────┘

sync loop ───────────────────────── daemon composition ───── RemoteFs
mount adapter ───────────────────── daemon composition ───── RemoteFs
```

`*` WebDAV is experimental and unshipped. The important point is that its
implemented operations use the same IPC path rather than a private backend.

## Empty-cache behavior

RemoteFs is intentionally cache-independent. Tests in the backend, daemon,
mount, and embedded-SDK layers exercise operations with an empty local cache.
When adding a drive-like feature, use RemoteFs or extend it; do not resolve a
path solely from cached SQLite rows.

## Source entrypoints

- `crates/pcloud-backends/src/remote_fs.rs` — service, types, retry and
  durability logic.
- `crates/pcloud-daemon/src/runtime.rs` — IPC-visible operation mapping.
- `crates/pcloud-daemon/src/mount_runtime.rs` — mount adapter composition.
- `crates/pcloud-daemon/src/sync_loop_runtime.rs` — sync transfer composition.
- `crates/pcloud-sdk-public/src/lib.rs` — stable SDK facade.
- `crates/pcloud-webdav/src/ipc_backend.rs` — experimental IPC adapter.

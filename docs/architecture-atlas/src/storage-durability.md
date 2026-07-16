# State, transfers, and durability

## State ownership

| State | Owner | Durability expectation |
|---|---|---|
| Configuration profile | `pcloud-config` | validated at startup; layered defaults/system/user/env |
| Auth token | daemon vault | optional persistence; passwords never persisted |
| Sync roots and cursors | `pcloud-store` repositories | transactional SQLite with migrations |
| Pending/domain records | `pcloud-store` | typed repositories, explicit schema version |
| Read/cache data | `pcloud-cache` and filesystem caches | bounded optimization; never remote truth |
| Upload progress | upload resume repository + upload journal | acknowledged offset survives crash |
| Mounted writeback | `pcloud-fs` staging + write journal | replay before accepting new work |
| Download progress | partial file + resume sidecar | bound to remote ID/path/size |
| Audit chain | observability/audit storage | tamper-evident event sequence |

## Authority rules

```text
pCloud service       authoritative remote namespace/content
SQLite store         authoritative local durable intent/progress
cache                optimization only
staging/journals     incomplete operation recovery state
CLI/web process      no authoritative state
```

An empty cache must produce a live query, not `NotFound`. Deleting a cache
must not delete remote data. A successful mutation updates or invalidates
local state only after the relevant remote outcome is known, except where an
explicit idempotent ordering is documented.

## Durable upload state machine

```text
Absent
  │ create session + persist descriptor
  ▼
Uploading ── transient failure/crash ──► Recoverable
  │ acknowledged chunk                    │ validate source identity
  ├── persist offset/hash state            └── query/continue offset
  │
  ├── conflict ──► explicit overwrite / if-hash / create-if-new decision
  │
  ▼
Verifying
  │ complete length + SHA-1
  ▼
Committing
  │ remote save succeeds
  ▼
Committed ──► clear resume/journal state
```

Generic readers can stream without whole-file buffering, but durable resume
requires a stable local file identity. Callers must not claim resumability
when the durability context is absent.

## Durable download state machine

```text
destination absent
  │ create partial + sidecar
  ▼
Downloading
  │ bounded range reads, persist bytes
  ├── crash ──► validate partial + sidecar → resume
  ▼
Verifying
  │ expected size + SHA-256
  ▼
Syncing
  │ flush + fsync
  ▼
Publishing
  │ atomic final-path publication
  ▼
Complete
```

Replacement policy is explicit. An existing destination is not silently
destroyed unless the caller selected replacement.

## Shutdown and drain

Graceful shutdown is expected to:

1. stop accepting new work;
2. report and wait for in-flight work within a bounded drain policy;
3. persist transferable progress;
4. flush stores and journals;
5. release mounts and local IPC;
6. zeroize resident secrets.

“Drain complete” must mean the tracked work reached a safe terminal or
recoverable state; it must not simply mean a timer expired.

## Recovery test surfaces

Look in:

- `crates/pcloud-backends` for RemoteFs and upload-journal tests;
- `crates/pcloud-daemon/tests` for journal replay and runtime recovery;
- `crates/pcloud-fs/tests` for mount/writeback/crash behavior;
- `tests/dr_drill` for operator-level disaster scenarios;
- `crates/pcloud-chaos` for deterministic injected failures;
- `crates/pcloud-live-e2e` for credentialed, opt-in remote verification.

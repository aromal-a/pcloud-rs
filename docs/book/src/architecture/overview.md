# Architecture Overview

This chapter is the 30,000-foot view of the Rust pCloud client. Read it before
the crate map, before the request lifecycle, and before any deep dive. Its job
is to answer four questions: what processes exist, how they talk, what state
they touch on disk, and which abstractions make the whole thing portable.

All file paths are relative to the repository root unless otherwise stated.

## Processes

The deployment unit is three cooperating binaries plus an optional web UI:

- **`pcloudc`** — the command-line client. Short-lived. It parses arguments,
  serializes a request, opens a local IPC connection to the daemon, waits for
  a response, prints a human-readable line, and exits with a status code. It
  ships as `crates/pcloud-cli`.
- **`pcloudd`** — the long-running daemon. It owns every remote connection,
  every cached credential, the SQLite store, the upload journal, the audit
  hash-chain, and the sync engine. All policy decisions happen here. It ships
  from `crates/pcloud-daemon` on every supported platform. The separate
  `pcloud-daemon-win` crate is an experimental, unshipped SCM host around this
  same daemon runtime; the supported Windows package starts `pcloudd.exe`
  per-user.
- **`pcloud-web`** — an optional Axum-based HTTP facade that talks to the
  daemon over the same local IPC channel as the CLI and exposes a small
  browser/JSON API for operators. It ships as `crates/pcloud-web`.

The CLI and the web UI are *clients*. They never hit `binapi.pcloud.com`
directly. Every remote call flows through the daemon so that credential
material, retry state, cache invalidation, and audit records live in exactly
one process.

## Transport between processes

Local IPC uses the host's native named local transport:

- On Linux and macOS: a Unix domain socket inside the user runtime directory.
  The path is `$XDG_RUNTIME_DIR/pcloud/ipc.sock`. The parent directory is
  `0700`, the socket is `0600`. Peer identity is established with
  `SO_PEERCRED` on Linux and `getpeereid` on macOS. There is no fallback to
  TCP.
- On Windows: a named pipe under `\\.\pipe\pcloud-<sid>`. The pipe DACL is
  explicitly built to grant access only to the creating user's SID. Peer
  identity is verified via the pipe client token. There is no fallback to
  TCP.

Frames are size-prefixed: an 8-byte little-endian header followed by a JSON
body. The daemon enforces a bounded maximum body size before allocating,
which prevents oversized or streaming-attack bodies from exhausting memory
on the server side.

## End-to-end data flow

A single pCloud operation (for example, `pcloudc sync add /local /remote`)
traverses five layers:

```
┌────────────────────────────────────────────────────────────┐
│ user shell                                                 │
│   │ argv                                                   │
│   ▼                                                        │
│ pcloudc (pcloud-cli)                                       │
│   parses argv → Command                                    │
│   builds Request enum (pcloud-ipc::protocol)               │
│   encodes frame, writes to socket                          │
│   │                                                        │
│   ▼                                                        │
│ local IPC (Unix socket / named pipe)                       │
│   │                                                        │
│   ▼                                                        │
│ pcloudd (pcloud-daemon)                                    │
│   accept → peer check → decode → dispatch                  │
│   per-command backend (auth, sync, transfer, public links, │
│   crypto, shares, backup, filesystem)                      │
│   │ persists state: store.sqlite3, auth_token vault,       │
│   │ upload journal, audit hash-chain                       │
│   │                                                        │
│   ▼                                                        │
│ pcloud-proto (typed HTTPS client)                          │
│   TLS to binapi.pcloud.com:8398 (or EU/US equivalent)      │
│   │                                                        │
│   ▼                                                        │
│ pCloud backend                                             │
└────────────────────────────────────────────────────────────┘
```

The reverse path is symmetric: proto decodes a typed response, the backend
updates local state, the runtime serializes a `Response`, the daemon writes
a framed reply, the CLI maps it to an exit code and a human-readable line.

## Background sync loop

In addition to responding to IPC requests, the daemon runs an autonomous
background sync loop on a dedicated `pcloud-sync-loop` thread. The loop:

1. Polls the remote diff API for each active (non-paused) sync root.
2. Walks the local directory tree to detect new, modified, and deleted files.
3. Feeds both remote and local changes through the engine's planner/scheduler.
4. Advances queued transfers (downloads via `get_file_link` + `download_bytes`).
5. Persists the diff cursor per root so the daemon resumes from the last
   successfully-processed position after a restart.

The loop's poll interval, batch size, and concurrency limit are configured
via the `[sync]` section of the config profile (`SyncLoopConfig`). The
loop cooperates with the IPC thread through:

- A `SharedAuthToken` (`Arc<Mutex<Option<SecretString>>>`) written by the
  IPC thread on login/logout and read by the loop each cycle.
- A `SyncLoopShared` structure carrying a condvar for wake signals
  (sync-add, resume, shutdown), an atomic pause flag, and a status
  snapshot that the IPC thread reads for `GetSyncStatus`.

The loop's backend instances (`SyncRuntime`, `TransferRuntime`,
`EngineShell`) are separate from the `RuntimeShell` owned by the IPC
thread, because `RuntimeShell` is `!Sync`. The SQLite connection is a
second WAL-mode reader that does not contend with the writer.

## State on disk

The daemon owns every persistent artifact. Nothing is written by the CLI.

- **`store.sqlite3`** — the primary relational store. It lives in the user
  data directory resolved by `PcloudDirs`, and it holds sync-root records,
  public-link metadata, share state, backup-device records, transfer
  bookkeeping, and account metadata. It is opened with WAL journaling so
  readers never block the writer. Schema migrations are explicit and
  additive, never destructive.
- **`auth_token` vault** — durable auth storage. Its backing differs by
  platform (see `PlatformVault` below), but the invariants are identical:
  owner-only permissions, integrity-checked envelope, opt-in persistence,
  never a cleartext password. Passwords are *not* mirrored from the legacy
  C client; only bearer tokens are stored, and only when the user has
  explicitly opted into durable sessions.
- **upload journal** — an append-only log of in-flight uploads. A chunked
  upload that crashes mid-flight resumes from the journal on next start. The
  journal is fsync'd at segment boundaries, so a kernel panic cannot lose a
  completed chunk. Stale entries are reaped when their backing upload
  completes, aborts, or expires.
- **audit hash-chain** — a tamper-evident log of privileged operations
  (auth, sync-root add/remove, crypto unlock, share modify, public-link
  create). Each record chains into the previous record's BLAKE3 hash, and
  the chain head is written atomically. Audit persistence failures surface
  as errors on the control path — they are never silently swallowed.

All four artifacts live together under the user data directory. On a
factory reset the daemon deletes the whole directory; there is no hidden
state outside it.

## Threading model

The daemon is deliberately *synchronous* in the request pipeline. Each
accepted IPC connection is served on a dedicated OS thread. Shared state is
guarded by `parking_lot::Mutex` and `parking_lot::RwLock`, chosen for lower
overhead and poisoning semantics that match our error model.

There is no `tokio` executor in the hot request path. The proto crate uses
blocking `rustls`/`ureq` for HTTPS, because:

- request volume per daemon is low (tens to hundreds of in-flight ops),
- synchronous stacks are dramatically easier to audit for cancellation and
  panic safety,
- deterministic teardown at shutdown is trivial (joins, not aborts),
- `catch_unwind` at the dispatch boundary contains any backend panic
  without poisoning the executor.

Background work that *is* inherently concurrent — the sync engine's folder
walkers, the upload writeback pool, the journal reaper — runs on fixed-size
thread pools with explicit shutdown signalling. Nothing is spawned unbounded.
The web facade, which must speak HTTP, uses a small tokio runtime but that
runtime never leaks into the daemon request path; it translates into the
same synchronous IPC calls the CLI makes.

## Five core platform abstractions

Portability is concentrated in five traits. Anything that differs between
Linux, macOS, and Windows is expressed through one of these. The rest of the
code sees the same surface on every platform.

1. **`PlatformMount`** — mount and unmount the virtual drive. Linux backs
   this with FUSE, macOS with `fuse-t`, Windows with WinFSP. Each returns a
   RAII mount handle that unmounts on drop and handles signal-driven
   cleanup. The trait exposes `mount`, `unmount`, `is_mounted`, and
   `drain_journal_on_stop` so the runtime does not care which backend it is
   driving.
2. **`PlatformIpc`** — the local transport. Unix-domain socket on Linux and
   macOS; named pipe on Windows. Peer identity, permissions, and frame
   limits are part of the trait contract, not of the caller. The CLI uses
   the same trait via the `pcloud-ipc` client helpers.
3. **`PlatformVault`** — durable storage for the auth-token envelope.
   Linux uses an owner-only file with integrity metadata; macOS uses the
   Keychain via Security.framework; Windows uses DPAPI with
   `CryptProtectData`. All three expose the same get/put/erase API, and
   all three refuse to persist a cleartext password.
4. **`MountinfoReader`** — enumerates existing mount points so the daemon
   can refuse to mount over a busy path and can reap stale mounts on
   startup. Linux parses `/proc/self/mountinfo`; macOS uses `getfsstat`;
   Windows enumerates drive letters and junctions.
5. **`PcloudDirs`** — resolves per-user directories. It picks
   `$XDG_DATA_HOME`, `$XDG_RUNTIME_DIR`, and `$XDG_CONFIG_HOME` on Linux;
   `~/Library/Application Support/pcloud` and
   `~/Library/Caches/pcloud` on macOS; `%LOCALAPPDATA%\pcloud` and
   `%APPDATA%\pcloud` on Windows. All directory creation goes through this
   trait so permissions stay consistent.

These five traits are the reason the rest of the codebase compiles the same
on every target. If you are writing new functionality and find yourself
reaching for a `#[cfg(target_os = …)]` block outside of these abstractions,
stop and add to the trait instead. The goal is that the daemon's dispatch,
backends, proto, store, and sync engine are completely platform-neutral —
the platform lives in five named boxes.

## Performance posture

The daemon's hot paths have been intentionally optimised in **wave-1** of the
performance pass: the page cache evicts in O(1), cache hits clone an
`Arc<Vec<u8>>` instead of the buffer, downloads stream through a 64 KiB
window with a rolling SHA256, write-flush is chunked with back-pressure, and
every chunk flush is observed by the `flush_latency_seconds` Prometheus
histogram. Numbers, micro-benchmarks, and reproduction steps live in
[Performance](./performance.md); treat that chapter as the source of truth
for regression-gating the release.

## If you're new to this codebase

The **thing to know** before anything else on the supported public path: there
is exactly one process that ever speaks to pCloud's servers — `pcloudd`. Everything else (the `pcloudc`
CLI, the `pcloud-web` browser UI, any third-party consumer linking the
`pcloud-sdk` crate) is a *client*. Clients construct a typed `Request`, hand
it to a local IPC channel, and wait for a typed `Response`. They never hold
credentials, never hit `binapi.pcloud.com`, never touch the SQLite store,
never touch the audit hash-chain. The trust boundary is the IPC socket.

If you internalise that one rule, the rest of the architecture falls out:

- **Why is there a `pcloud-backends` crate?** Because the daemon's platform
  compositions, mount/sync adapters, and the internal
  `pcloud-embedded-sdk` compatibility path share a canonical implementation.
  The public `pcloud-sdk` does not link backends; it reaches them through IPC.
- **Why are there five and only five platform abstractions?** Because we
  promised ourselves that platform differences get named explicitly and
  boxed, not scattered as `cfg` blocks. If a new platform-dependent concept
  appears, it becomes a sixth trait, not a new `cfg` branch.
- **Why are there separate `pcloud-secret`, `pcloud-auth`, `pcloud-crypto`
  crates?** Because secret-bearing types zeroize on drop and that discipline
  is easier to enforce when the types live in a small, grep-able crate. Any
  use of a raw `String` for a password is visible in one `rg` command.
- **Why is the hot path synchronous, not tokio?** Because request volume is
  low, auditability is high, and `catch_unwind` at the dispatch boundary is
  trivial in a thread-per-connection model. Tokio exists in `pcloud-web`
  only because it must speak HTTP; it does not leak into the daemon.

## State machines at a glance

Five stateful subsystems run concurrently in the daemon. Each has its own
explicit state machine; this section summarises them. Full transition tables
live in the per-subsystem documentation.

### Auth state

```
+-----------+  login ok     +---------------+  tfa code ok  +-------------+
| LoggedOut |-------------->| AwaitingTFA   |-------------->| Authenticated|
+-----------+               +---------------+               +-------------+
     ^                              |                              |
     |                              | user cancel / timeout        | logout
     |                              v                              |
     |                      +---------------+                      |
     +----------------------|   LoggedOut   |<---------------------+
                            +---------------+
```

Implemented in `crates/pcloud-daemon/src/auth_backend.rs`. Transitions are
logged to the audit hash-chain; the vault is written only on entering
`Authenticated` with `persist = true`.

### Sync-root state

```
+----------+ add  +----------+ validate ok  +--------+ engine +---------+
| Absent   |----->| Pending  |------------->| Active |------->| Running |
+----------+      +----------+              +--------+        +---------+
                        |                       ^                  |
                        | validate fail         | resume           | pause
                        v                       |                  v
                  +----------+                  |            +----------+
                  | Rejected |                  +------------| Paused   |
                  +----------+                               +----------+
```

Implemented in `crates/pcloud-daemon/src/sync_backend.rs` and
`crates/pcloud-engine/src/lib.rs`.

### Crypto state

```
+-----------+ setup  +-----------+ unlock  +------------+ lock  +-----------+
| NotActive |------->| Active    |-------->| Unlocked   |------>| Active    |
+-----------+        | (locked)  |         | (session)  |       | (locked)  |
     ^               +-----------+         +------------+       +-----------+
     |                     ^                     |                    ^
     |                     | reset/rotate        | expire/idle        |
     +---------------------+                     +--------------------+
```

Implemented in `crates/pcloud-crypto/src/lib.rs`. Unlock holds
`SecretBytes` key material for the session lifetime; zeroize fires on every
downward transition.

### Mount state

```
+--------+ mount  +--------+ ready  +--------+ stop  +------------+
| Absent |------->| Mount  |------->| Online |------>| Unmounting |
+--------+        | ing    |        +--------+       +------------+
                  +--------+            |                   |
                      |                 | error             v
                      | error           v             +--------+
                      +------->  +-----------+        | Absent |
                                 | Failed    |        +--------+
                                 +-----------+
```

Implemented in `crates/pcloud-fs/src/mount.rs`. Signal-aware RAII handles
guarantee the `Online → Unmounting → Absent` transition even on `SIGKILL`-
less shutdowns; see [ADR-0010](../adr/0010.md) for the current write-path
wiring caveats.

### Integrity-sweeper state

```
+------+ start  +---------+ page  +---------+ chunk  +---------+ done +--------+
| Idle |------->| Listing |------>| Hashing |------->| Compare |----->| Report |
+------+        +---------+       +---------+        +---------+      +--------+
```

Implemented across `pcloud-engine` and `pcloud-plugin-autoheal`. The sweeper
advances one chunk at a time so cancellation is always possible at a page
boundary.

## Tradeoffs and design decisions (ADR-style)

This section summarises decisions that are load-bearing for the architecture.
Each decision has a full ADR under `docs/book/src/adr/`; only the headline
rationale lives here.

- **Thread-per-connection over tokio in the daemon request path**
  ([ADR-0003](../adr/0003.md), [ADR-0004](../adr/0004.md)). We rejected a
  tokio-first daemon because:
  - cancellation semantics for blocking system calls in tokio require
    `spawn_blocking` at every syscall site;
  - `catch_unwind` across `.await` points is not sound;
  - the OS is already a decent scheduler for ~hundreds of concurrent local
    IPC clients.
- **JSON IPC bodies over CBOR** ([ADR-0002](../adr/0002.md)). CBOR was
  rejected for now because JSON makes `tcpdump`-style debugging of the
  socket trivial and the 1 MiB cap keeps the cost bounded.
- **Opt-in durable auth, never opt-in durable passwords**
  ([ADR-0005](../adr/0005.md), [ADR-0007](../adr/0007.md)). The legacy C
  client persisted a cleartext password; we explicitly refuse to mirror that.
- **No background update checker** ([ADR-0006](../adr/0006.md)). The legacy
  update-check surface is a ghost declaration; carrying it would add an
  outbound network egress that leaks a deployment fingerprint.
- **Five platform traits, no `cfg` outside them** (this page). Any new
  platform-specific concept becomes a sixth trait; `cfg` outside the
  abstractions is a review-block.

## Concurrency model

The daemon has exactly three concurrency domains:

1. **IPC acceptor domain** — one dedicated OS thread per accepted connection,
   bounded by a soft cap on concurrent connections. Each thread reads a
   framed request, invokes `Runtime::handle_request` (which catches panics),
   writes a framed response, and exits.
2. **Engine domain** — a fixed-size thread pool inside `pcloud-engine` that
   drives folder walks, diffs, and transfer-queue scheduling. Shutdown is
   cooperative via an `AtomicBool` flag plus a channel.
3. **Writeback/journal domain** — a small dedicated pool for chunked flush
   and journal replay. Back-pressure is a tokio-free `parking_lot::Condvar`
   bounded semaphore, sized at four in-flight chunks by default.

Locks are chosen per-site:

- `parking_lot::Mutex` / `parking_lot::RwLock` for short critical sections
  inside the daemon (no poisoning overhead; faster `try_lock`);
- `std::sync::RwLock` is avoided because its poisoning semantics do not
  match our `catch_unwind` policy;
- channels are `crossbeam_channel::bounded` for work handoff and
  `std::sync::mpsc` only inside small isolated helpers.

`pcloud-web` runs a minimal tokio runtime for its HTTP serving needs and
issues blocking IPC calls from a `spawn_blocking` boundary so a slow daemon
response does not starve its reactor.

## Security invariants (summary)

Every invariant below is backed by at least one test in the workspace. See
[Security Model](./security-model.md) for the detailed list with test
citations.

- Every secret-bearing long-lived field uses `SecretString` or
  `SecretBytes`.
- Secrets never appear in `Debug`, `Display`, `tracing::event!`, or any
  error path visible to the user.
- The IPC socket is `0600` on a `0700` parent directory; permissions are
  asserted at startup and again on each accept.
- Peer identity is verified with `SO_PEERCRED` / `LOCAL_PEERCRED` /
  SID-DACL before the first body byte is read.
- IPC body size is capped at 1 MiB; oversize frames abort without allocation.
- Production configs reject plaintext downgrade from TLS.
- Audit persistence failures surface as errors on the control path; they
  are never silently swallowed.
- The auth vault envelope is integrity-checked with a BLAKE3 MAC; a
  tampered vault is treated as absent, not recovered.

## Extension points

- **Plugin API** (`pcloud-plugin-api`). Versioned trait surface with an
  explicit semver contract; current plugins are `autoheal`, `backup-schedule`,
  `publink-expiry`, and `dlp`.
- **Backend trait** (`pcloud-backends::Backend`). Per-feature backends
  implement this trait; the daemon and SDK both consume it.
- **`KmsProvider` trait** (`pcloud-kms`). Abstraction over local secret
  materials and optional enterprise KMS. Pkcs11 stub lives here; no live
  HSM interop yet.
- **`PolicyProvider` trait** (`pcloud-policy`). Declarative policy checks
  for mount location, share recipient domains, and DLP rules; enterprise
  deployments bind their own provider at daemon start.
- Five platform traits (`PlatformMount`, `PlatformIpc`, `PlatformVault`,
  `MountinfoReader`, `PcloudDirs`). New platforms implement these; no
  other file in the workspace should name the OS directly.

## Open `bd` trackers

The architecture is pre-alpha and the matrix is honest about that. The
open trackers that affect architecture claims are:

- **`bd-1du`** — the umbrella parity epic.
- **`bd-1du.4`** — mounted-drive / FUSE parity. Linux is the only
  live-tested mount backend today.
- **`bd-1du.4.6.1`** — write-path daemon wiring caveats; see
  [ADR-0010](../adr/0010.md).
- **`bd-1du.10`** — final parity proof. Until it closes, do not assert
  "full parity", "production-ready", or "drop-in replacement" in docs,
  release notes, or marketing.

## Cross-references

- [Crate Map](./crate-map.md) for every crate, its role, and its public
  surface.
- [Request Lifecycle](./request-lifecycle.md) for the end-to-end trace of
  a single request.
- [Performance](./performance.md) for the optimisation wave-1 wins and the
  release-gate numbers.
- [Platform Support](./platform-support.md) for the per-platform capability
  matrix.
- [Security Model](./security-model.md) for architecture-scoped security
  invariants and their test citations.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv`
  for the authoritative parity rows; see
  [`STATUS.md`](../../../../STATUS.md) for current counts.

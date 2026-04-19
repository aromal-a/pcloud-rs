# ADR 0019: IPC Serve Loop Is Single-Threaded Per Daemon Process

- Status: Accepted
- Date: 2026-04-19
- Audit reference: audit-06 §7-sonnet M2 (`pcloud-rs-ncx.56`)

## Context

The production IPC accept loop in `pcloud-daemon::serve::serve_until_shutdown`
calls `BoundIpcServer::serve_once_with_peer` in a single thread. Each
accepted connection is handled to completion (peer-credential check,
decode, dispatch through `RuntimeShell`, encode, write, close) before the
next `accept(2)` is invoked. A slow backend call (auth round trip,
crypto unlock, RSA-4096 OAEP on the big `CryptoGetFolderKey` path) blocks
every subsequent client until it returns.

`BoundIpcServer::accept_and_spawn` exists (`crates/pcloud-ipc/src/transport.rs`)
and implements thread-per-connection dispatch, but it is **not** used in
production. This ADR records why, when that choice may change, and what
operators should expect today.

## Decision

The daemon serves IPC requests one at a time per process. The serialization
is intentional; it is not a "TODO" item that needs a fix.

### Root cause

`RuntimeShell` is deliberately `!Send` and `!Sync`:

1. It owns raw pointers into SQLite connections (the store handle is
   built on `rusqlite::Connection`, which is `!Send`).
2. Several sub-shells (`MountControl`, `EngineShell`, `CacheShell`)
   hold platform-specific file descriptors and `RefCell`-backed
   mutable state that is not safe to cross thread boundaries.
3. The crypto shell caches unlocked key material in memory and
   validates single-thread access as part of its zeroise discipline.

`accept_and_spawn` requires the handler to be `Clone + Send + 'static`.
Migrating the production dispatcher to it would require either:

* Wrapping `RuntimeShell` in `Arc<Mutex<...>>`. This introduces lock
  contention on every IPC call — the lock would be held for the full
  duration of an auth or crypto round trip, which is worse than the
  current single-threaded loop for latency-sensitive probes (a health
  check behind a slow `Crypto` request would still block).
* Migrating to a channel-based dispatch model (accept loop sends
  `(Request, oneshot::Sender<Response>)` to a dispatcher actor). This
  is the cleanest long-term design but requires refactoring every
  `handle_request` call site and rewriting the panic-guard and SLO
  instrumentation around it.

### Current mitigations

* **Per-connection read timeout**: `IPC_REQUEST_READ_TIMEOUT` (5 s) caps
  the time a slow or malicious client can hold the accept thread hostage
  waiting for framed input.
* **Per-connection write timeout**: `IPC_RESPONSE_WRITE_TIMEOUT` (30 s)
  caps the time a slow reader can block the accept thread after dispatch
  has already completed.
* **Global and per-peer connection caps**: `MAX_IPC_CONNECTIONS` (128)
  and `MAX_IPC_CONNECTIONS_PER_PEER` (32), now runtime-configurable per
  ncx.59, bound worst-case thread/fd consumption. Since the production
  loop is single-threaded, the *global* cap is primarily useful as a
  backstop for the rare embedder that does use `accept_and_spawn`.
* **Cooperative drain**: during shutdown, the accept loop transitions to
  `Draining` and rejects non-status requests with
  `ResponseStatus::Unavailable("daemon draining, retry")`, so clients
  receive a clean signal rather than hanging.

### What we explicitly do **not** promise

* We do not promise IPC throughput scales with CPU count.
* We do not promise a slow `CryptoGetFolderKey` will not delay a
  concurrent `GetHealth` probe.
* We do not promise a buggy backend that blocks forever (e.g. awaiting
  a never-arriving network response) will be killed by any mechanism
  other than the per-request read/write timeout on the peer stream.

### When this ADR is re-opened

Trigger a reconsideration when any of the following holds:

1. Real-world deployment telemetry shows IPC p99 latency correlated with
   long-tail backend calls.
2. The multi-user server scenario (single daemon, many local accounts,
   one per service) becomes a supported deployment mode. The per-peer
   connection cap is the first line of defence, but under that
   scenario a channel-based dispatcher is the right primitive.
3. `RuntimeShell` is restructured so the non-`Send` bits (SQLite handle,
   mount descriptors, zeroised crypto caches) live behind `Send`-safe
   handles. At that point, migrating to `accept_and_spawn` is a one-line
   change in the serve loop.

## Consequences

### Positive

* No lock contention; the fast path (`GetHealth`, `GetStatus`,
  `Method::ListPublicLinks`) runs with zero synchronization overhead.
* Panic containment is trivial: the single dispatch thread's
  `catch_unwind` boundary (ADR 0004) cannot leave half-completed state
  in another thread's local cache.
* Crash recovery is simple: each request is logically atomic (accept →
  decode → dispatch → reply → close), so a process crash at any point
  cannot produce partial daemon state.

### Negative

* A slow backend call (auth RTT > 1 s) delays every queued IPC caller
  by the same amount. Operators must not assume concurrent CLI callers
  see parallel service.
* `accept_and_spawn` is only exercised by non-production embedders,
  which reduces production-observed coverage of that code path.

## References

- `crates/pcloud-ipc/src/transport.rs` — see the module-level "Why
  `accept_and_spawn` is not used in production" doc block.
- `crates/pcloud-daemon/src/serve.rs::serve_until_shutdown_with_flag` —
  the production accept loop.
- Audit finding: `audits/06/section-07-ipc-daemon-sonnet.md` M2.
- Bead: `pcloud-rs-ncx.56` — P3-E3 serve_once production loop
  single-threaded.

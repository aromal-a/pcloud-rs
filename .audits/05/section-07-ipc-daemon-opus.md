# Audit 05 — Section 7: IPC & Daemon (Opus)

Scope: `crates/pcloud-ipc/`, `crates/pcloud-daemon/{serve,dispatch,rate_limit,signals,health_server}.rs`, and the new Wave-2 crypto IPC variants (`CryptoSetupV2`, `CryptoGetFolderKey`, `CryptoGetFileKey`).

## Executive summary

Framing, peer auth, connection caps, drain, and sd_notify wiring are sound. The three new crypto IPC variants are fully dispatched, privileged-logged, and rate-classed per the audit-04 plan. Two real issues remain (one HIGH, one MEDIUM) plus a handful of LOW polish items.

## Findings

### HIGH

- **H1. Privileged-IPC log reports daemon-owner uid, not peer uid.**
  `crates/pcloud-daemon/src/serve.rs:204-211` logs `from uid={}` using `current_effective_uid()` (the daemon's own euid), with a comment asserting "peer uid == daemon owner uid". That is true today because `authorize_peer` (`crates/pcloud-ipc/src/server.rs:130`) rejects non-owner peers, but the audit line is still misleading (every privileged event says `uid=<daemon-uid>`) and it defeats the stated M-2 purpose of letting operators correlate *who* invoked a privileged op. It also breaks the moment anyone enables the multi-uid path (e.g. a future admin role, or a shared-runtime deployment). Fix: thread the real `PeerIdentity.uid` from the transport layer (`transport.rs:319, 502`) into the handler closure and pass it to `dispatch_with_drain_gate`. Log `peer_uid` and `peer_pid`.

### MEDIUM

- **M1. "Per-session" rate limiter is actually process-global.**
  `crates/pcloud-daemon/src/rate_limit.rs:19` docs claim "per-session" (each peer gets its own bucket), but `RuntimeShell` owns a single `SessionRateLimiter` (`crates/pcloud-daemon/src/runtime.rs:210`; instantiated once in `bootstrap.rs:834`) and `dispatch.rs:346` calls `runtime.rate_limiter.check(&request)` for every caller. Every connection from the owning uid shares the same buckets, so one chatty caller can starve another. With `MAX_IPC_CONNECTIONS_PER_PEER=32` and owner-only enforcement all peers are the same uid anyway, so the bucket is *de facto* global. Either (a) key the limiter by `peer.pid` inside a `DashMap<u32, SessionRateLimiter>` on the runtime, or (b) rewrite the rate_limit module docs and the audit-04 narrative to say "daemon-wide, per-category". Currently docs ≠ code.

- **M2. `CryptoGetFolderKey` / `CryptoGetFileKey` are `Medium` category but are not privileged-logged.**
  `serve.rs:75-99` excludes both from `is_privileged_request`. Folder/file key fetches touch the unlocked RSA private key and yield plaintext AES keys; an attacker with local IPC access (already rejected by owner-uid, but still) should leave an audit trail per fetch. Audit-04 Wave 2 explicitly added `CryptoSetupV2` to the privileged set — the two `Get*Key` fetches are equally sensitive and should join it. At minimum, emit a structured log at `info!` on every successful unwrap (already done inside `runtime.rs:3218-3222` via `audited_response`, so the gap is only in the pre-dispatch audit line; acceptable to close by adding them to `is_privileged_request`).

### LOW

- **L1. `UploadWriteFromFile` and `CreateTreePublicLinkFromPaths` are in `is_privileged_request` (`serve.rs:91-92`) but — per CLAUDE.md — the daemon-side IPC variants don't exist yet (bd-1du.10 row 93/149 Partial). These arms are dead code until the wiring lands. Not a bug, but the audit log will stay empty; flag the pattern so the matrix stays honest.

- **L2. `sd_notify` is Linux-only (`serve.rs:40-55`). On macOS/BSD the drain transitions (`STOPPING=1`, `RELOADING=1`, `READY=1`) become no-ops silently, which is correct for unsupervised runs but means a launchd-based macOS deployment gets no lifecycle signal. Document or add launchd KeepAlive signalling if macOS parity is in scope.

- **L3. Health endpoint is HTTP/1.0 plaintext on 127.0.0.1 only (`health_server.rs:213-228`) — correct posture. However the readiness check re-enters a global `drain_state()` atomic per request with no rate limit; a local attacker (already owner-uid) can trivially fan-out `/readyz` threads (`run_listener` spawns one OS thread per connection, `health_server.rs:134-142`). Add `accept_and_spawn` cap mirroring `MAX_IPC_CONNECTIONS` on the HTTP listener, or move to a bounded thread pool.

- **L4. `MAX_IPC_CONNECTIONS_PER_PEER=32` (`transport.rs:54`). With owner-only binding, only one uid ever connects, so this cap degenerates to a re-statement of `MAX_IPC_CONNECTIONS=128 / 4`. Either lower to 8 (typical ≤ 2 CLI callers) to leave headroom for slot attacks, or document that the per-peer cap is defense-in-depth for a future multi-uid surface.

- **L5. `BoundIpcServer::Drop` (`transport.rs:534-538`) unlinks the socket, but on abnormal termination (SIGKILL, panic in the serve thread before `Drop` runs) the socket file persists at `0600`. `IpcServer::bind` (`transport.rs:630-632`) does remove stale sockets, so cold restart is fine, but a hot-restart IPC client racing the bind could `connect(2)` the stale socket. Consider `flock(2)` on a `.pid` sibling to make the race deterministic.

- **L6. Wave-2 proptest coverage (`pcloud-ipc/tests/proptest_methods_roundtrip.rs:644-665`) encodes/decodes the three new variants, but there is no daemon-level proptest that feeds arbitrary `CryptoSetupV2 { backend: Enhanced, acknowledge_not_interop: false, .. }` and asserts `InvalidRequest`. Add one to lock in the gate at `runtime.rs:3042-3050`.

## Positives

- `CryptoSetupV2` gate (`runtime.rs:3042`) correctly rejects `Enhanced` without `acknowledge_not_interop`.
- All three new variants: rate-classed (`rate_limit.rs:221-228`), privileged-labelled (`serve.rs:85,115-117`), backend-labelled (`dispatch.rs:144-150`), CLI-wired (`app.rs:2792-2858`).
- Framing: 1 MiB `MAX_REQUEST_BYTES` check *precedes* allocation (`transport.rs:732-737`). Version pinned to 1 (`protocol.rs:39`).
- Socket permissions: `0700` parent + `0600` socket (`transport.rs:626,635`), enforced in `security_invariants.rs:159-169`.
- Connection caps: atomic global + per-peer with TOCTOU-safe ordering under a single mutex (`transport.rs:84-125`).
- Graceful drain: 3-state machine with `InFlightGuard`, drain-gate admits `DrainStatus|Shutdown|GetHealth|Health` only (`serve.rs:176-184`).
- sd_notify: `READY=1` post-bind, `RELOADING=1` / `STOPPING=1` / `WATCHDOG=1` at correct transitions (`serve.rs:264,309,330,352,449`).
- `/livez` always ok; `/readyz` 503 during drain (`health_server.rs:184-207`).

## Verdict

Section 7 is in good shape. Land H1 (peer-uid logging) and M1 (either fix or re-document per-session rate limiting) before closing audit-05; M2/L1–L6 can defer to a polish pass.

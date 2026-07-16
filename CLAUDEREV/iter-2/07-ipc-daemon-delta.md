# Dimension 7 (IPC & Daemon) — Iter-2 Delta

**Auditor:** Claude (read-only)
**Date:** 2026-04-29
**Method:** Re-verification of iter-1 findings against current source.

## Convergence verdict

**Converged on findings.** No new severity-level findings. All five
re-verification items confirm iter-1's claims; one minor source-citation
correction (the prompt referred to `dispatch.rs` but
`is_privileged_request` lives in `serve.rs` — iter-1 cited the right file).

## Re-verification matrix

### 1. CLAUDE.md correction — Windows IPC accept loop wiring (M-7.3) — CONFIRMED

- `crates/pcloud-ipc/src/transport.rs:439-477` — `serve_once_with_peer`
  has a fully-wired `#[cfg(windows)]` arm: calls `listener.accept()`,
  builds `PeerIdentity { uid: 0, pid: stream.peer_pid() }`, runs
  `ConnectionGuard::acquire(peer.uid)`, then dispatches through
  `serve_stream_once_with_peer` with the same `IpcServer::new(self.owner_uid)`
  context as Unix.
- `crates/pcloud-ipc/src/transport.rs:486-592` — `accept_and_spawn`
  mirrors the same end-to-end Windows wiring (accept → cap → spawn
  thread → `serve_stream_standalone_with_peer`).
- `crates/pcloud-daemon/src/serve.rs:533-604` — `serve_with_shutdown`
  is **identical** on Unix and Windows in its serve loop body. It
  bootstraps the runtime, spawns the sync loop, optionally spawns
  the health server, calls `bind(&socket_path)` (which on Windows
  resolves to a `\\.\pipe\pcloud-rs-<SID>` named pipe), and ends with
  `serve_until_shutdown_with_flag`. There is no `Unsupported` return
  on Windows.

CLAUDE.md's claim "`pcloud_daemon::serve_with_shutdown` on Windows
currently returns `Unsupported`" is **stale and contradicted by the
source tree**. Iter-1's correction stands.

### 2. H-7.1 — `is_privileged_request` is audit-only — CONFIRMED

- Function lives at `crates/pcloud-daemon/src/serve.rs:109-135` (the
  prompt referred to `dispatch.rs`; that's a citation slip in the prompt,
  not in iter-1's report — iter-1 cited `serve.rs:109-135` correctly).
- Call site at `serve.rs:245-260` (`dispatch_with_drain_gate`) is the
  only consumer. The body is exactly:

  ```rust
  if is_privileged_request(&request) {
      log::info!(
          "privileged IPC request: {} from uid={} pid={}",
          request_kind_name(&request),
          peer_uid,
          peer_pid,
      );
  }
  let _guard = signals::InFlightGuard::new();
  crate::dispatch::dispatch_with_peer_creds(runtime, peer_uid, peer_pid, request)
  ```

  No early return on `is_privileged_request == true`. No
  `ResponseStatus::Unauthorized`. No allow-list, no extra check beyond
  the owner-uid match `IpcServer::authorize_peer` already enforced.
  `dispatch::dispatch_with_peer_creds` is invoked unconditionally.
- The single `metrics_server.rs:155` re-invocation routes the metrics
  endpoint through the same `dispatch_with_drain_gate`, so the
  audit-only posture is symmetric across both transports.

H-7.1 is verbatim correct: privileged requests get a `log::info!` event
and **proceed**. No per-request capability tier exists.

### 3. M-7.4 / M-7.5 — pcloud-web rate-limit boundary — CONFIRMED

- Grepped `crates/pcloud-web/src` for any of `rate.?limit`, `RateLimit`,
  `governor`, `throttle`, `bucket`, `too_many` (case-insensitive). **Zero
  matches.** No `tower::limit::RateLimitLayer`, no `tower-governor`, no
  in-crate token bucket.
- Router built at `routes.rs:72-87` adds no rate-limit layer. Mutating
  routes (`POST /sync`, `DELETE /sync/:id`, `POST /publinks`,
  `DELETE /publinks/:code`) gate only on `require_web_token` +
  `require_csrf`.
- Daemon-side limiter at `crates/pcloud-daemon/src/rate_limit.rs:138-160`
  keys per `peer_uid`. Every `pcloud-web` HTTP request reaches the
  daemon over the same Unix socket as the daemon owner uid, so all web
  traffic shares a **single** `SessionRateLimiter` bucket regardless of
  HTTP-side concurrency. M-7.5 therefore stands: the per-peer bucket is
  shared across all web traffic and confers no per-route or per-token
  isolation. M-7.4's same-trust-boundary claim also stands — the web
  token file lives next to the IPC socket under the same `0700` runtime
  dir at `0600`.

### 4. Stress test `#[ignore]` reasoning (L-7.10) — REASONED

`crates/pcloud-ipc/tests/stress_concurrent_clients.rs:63-65`:

```rust
#[test]
#[ignore = "stress: 50 clients x 500 reqs, run with --release --ignored"]
fn stress_concurrent_ipc_clients_do_not_leak_or_panic() {
```

Reasoned: 50×500 = 25 000 round-trips, only meaningful as a load-shaped
regression check, debug-mode runtimes would be misleading. The fd-leak
half is also `#[cfg(target_os = "linux")]`-gated (lines 47-60 read
`/proc/self/fd`); the file documents that BSD/macOS fd-leak detection
is intentionally not wired and references `bd-1du.4` cross-platform
hardware verification (audit-06 LOW IPC L-7.3 / ncx.84). The `#[ignore]`
is justified; the L-7.10 remediation (nightly `--ignored` job +
cross-platform fd-leak harness) still applies.

### 5. pcloud-mockserver — NOT RELEVANT TO IPC POSTURE

- `crates/pcloud-mockserver/src/lib.rs:1-52` is unambiguous: the crate
  is an offline HTTP/1.1 stub of the **pCloud REST API** (`/userinfo`,
  `/listfolder`, `/upload_create`, …). It is not an IPC mock and does
  not exercise the daemon's local-socket transport.
- Module-level disclaimer at `lib.rs:47-52`: "test-only crate. It MUST
  NOT be depended on by any production/runtime code path."
- Iter-1 was correct to scope the IPC review to `pcloud-ipc` +
  `pcloud-daemon` + `pcloud-web`. `pcloud-mockserver` belongs in the
  pCloud REST/transport audit dimension, not the IPC & daemon
  dimension.

### 6. Proptest `#[non_exhaustive]` enumeration — CONFIRMED HONOR-SYSTEM

`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:118-123`:

```rust
        | Method::VerifyEmail => 0,
        // Required by #[non_exhaustive]; must remain last. Every currently-known
        // variant is listed above — a new variant NOT in the list above will
        // land here and must be added before merging.
        _ => 0,
    }
}
```

The `_ => 0` arm collapses unknown variants to zero — the file documents
this as honor-system maintenance discipline, but a forgotten enumeration
will not cause a compile-time failure. `arb_method()` (line 138-141)
similarly draws from `every_method().to_vec()`, so any variant missing
from the manual list is invisible to the property generator. M-7.7
stands as written.

## What is unchanged from iter-1

- 0 CRITICAL, 2 HIGH (H-7.1, H-7.2), 5 MEDIUM (M-7.3..M-7.7),
  4 LOW (L-7.8..L-7.11). No new findings, no upgraded severities, no
  retracted findings.

## What is new in iter-2

Nothing of audit substance. Two minor cosmetic notes for the parent's
consumption:

1. The prompt asked to inspect `dispatch.rs` for `is_privileged_request`
   — the function is actually in `serve.rs` (iter-1 cited correctly).
2. `pcloud-mockserver` audit relevance is null for this dimension; it
   should be picked up under whichever dimension covers the pCloud REST
   client / transport layer.

---

delta count: 0

# Audit 06 §7 — IPC & Daemon
**Date:** 2026-04-18  
**Auditor:** Sonnet (independent cross-validation of Opus audit-05)  
**Scope:** `crates/pcloud-ipc/`, `crates/pcloud-daemon/src/`, `crates/pcloud-web/`

---

## Verification of Audit-05 Claims

The following audit-05 improvements were verified as **held**:

- Privileged audit logging with real peer uid/pid threading — confirmed in `serve.rs:233–262`
- `PerPeerRateLimiter` — confirmed in `rate_limit.rs` (per-uid `HashMap<u32, SessionRateLimiter>`)
- IPC bind `chmod 0600` + parent dir `0700` — confirmed in `transport.rs:664–694`
- Health server connection cap (32) + loopback-only bind — confirmed in `health_server.rs:55,116`
- macOS `getpeereid` stub via `platform::unix` — confirmed in `transport.rs:869–878`

---

## Findings

### MEDIUM — M1: Version negotiation is hard-reject with no backward path
**File:** `crates/pcloud-ipc/src/protocol.rs:255–260`

`IPC_PROTOCOL_VERSION = 1` is checked by exact equality. A version bump to `2` immediately disconnects every pre-upgrade CLI/SDK client with `ProtocolError::VersionMismatch` — there is no negotiation, capability downgrade, or dual-version compatibility window. For an enterprise deployment where daemon and CLI may be upgraded independently this is operationally brittle.

**Remediation:** Add a minimum/maximum version range (`MIN_SUPPORTED_VERSION..=IPC_PROTOCOL_VERSION`) in the decoder so old clients still work for one release cycle. Document the deprecation/drop policy.

---

### MEDIUM — M2: `serve_once` is the production loop path; single-request-per-accept blocks concurrent callers
**File:** `crates/pcloud-ipc/src/transport.rs:310–371`, `crates/pcloud-daemon/src/serve.rs:380–397`

`serve.rs` calls `bound.serve_once_with_peer(…)` in a tight loop. The `accept_and_spawn` (threaded) path exists but is **not used** in the production daemon. The code comment on `BoundIpcServer` explicitly states the single-threaded loop is "the deliberate production path" because `RuntimeShell` is `!Send`. This means every CLI call (auth RTT, crypto unlock, large listing) blocks all other callers for its full duration. The connection cap (`MAX_IPC_CONNECTIONS=128`) and per-peer cap (`MAX_IPC_CONNECTIONS_PER_PEER=32`) are enforced at the transport layer but provide no concurrency benefit when only one request can run at a time.

**Remediation:** Track this as a known architectural constraint in ADR or CLAUDE.md. For short-term mitigation, document the serialization guarantee so operators know CLI latency is bounded by the slowest in-flight request. Longer term, a channel-based dispatch model (or Arc<Mutex<RuntimeShell>>) would allow `accept_and_spawn` to be the production path.

---

### MEDIUM — M3: Windows named-pipe backend is compile-only; peer SID check not integration-tested
**File:** `crates/pcloud-ipc/src/platform/windows.rs`

The Windows named-pipe backend is present and correctly designed (per-user SID in pipe name, DACL grants only current-user SID, `GetNamedPipeClientProcessId` → `TokenUser` SID comparison). However, the stress test (`stress_concurrent_clients.rs`) explicitly calls out `#[cfg(target_os = "linux")]` for fd-leak detection and the `peer_and_protocol.rs` tests are Unix-only. No integration test exercises the Windows peer-auth path. The `transport.rs` `peer_identity()` function has no `#[cfg(windows)]` branch — on Windows the Unix-socket path is dead and the named-pipe path is in a separate module but not wired into the shared `serve_once_with_peer` accept loop.

**Remediation:** Wire the Windows named-pipe accept loop into the production serve path (currently only the Unix socket path is wired). Add at minimum a compile-test that builds the Windows peer-auth path and a stub integration test that can be run in CI on a Windows runner.

---

### MEDIUM — M4: `health_server.rs` spawn-thread counter increment/decrement is not atomic relative to cap check
**File:** `crates/pcloud-daemon/src/health_server.rs:146–171`

The connection cap check (`current >= MAX_CONCURRENT_HEALTH_CONNECTIONS`) uses `Ordering::Relaxed` on the load and the increment uses a separate `fetch_add`. Between the check and the increment, another thread could increment past the cap (classic TOCTOU). Under the expected load (health-check probes) this is low-risk but a simple fix (`compare_exchange` loop) would be cleaner and matches the correct pattern already used in `transport.rs:ConnectionGuard::acquire`.

**Remediation:** Replace the load-then-fetch_add sequence with a `compare_exchange` loop as done in `ConnectionGuard::acquire`, or use `fetch_add` first and if the result exceeds the cap immediately `fetch_sub` and drop the connection.

---

### MEDIUM — M5: `proptest_methods_roundtrip.rs` — `every_method()` exhaustiveness is comment-only, not compile-enforced
**File:** `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:18–51`

The comment acknowledges that `Method` is `#[non_exhaustive]` and the exhaustiveness guard is therefore advisory only — a new `Method` variant introduced without updating `every_method()` will not fail tests. The compile-time guard function referenced in the comment exists but uses a `_ => ()` catch-all that silently passes for any new variant. New `Request` variants with payload fields (e.g. `UploadWriteFromFile`, `CreateTreePublicLinkFromPaths`) are present in the dispatch label map (`dispatch.rs:210–215`) but are not exercised in the proptest suite.

**Remediation:** Add proptest strategies for the two Partial-row variants (`UploadWriteFromFile`, `CreateTreePublicLinkFromPaths`) and any other payload-bearing `Request` variants not currently in the roundtrip suite. Consider a `#[test]` that calls a `const fn` over the full variant list to enforce coverage.

---

### LOW — L1: Crash recovery for IPC socket is stale-file cleanup only; no re-adoption of in-flight state
**File:** `crates/pcloud-ipc/src/transport.rs:689–691`, `crates/pcloud-daemon/src/bootstrap.rs`

On restart after SIGKILL, `IpcServer::bind` removes any stale socket file (documented in the `Drop` impl comment). Orphaned FUSE mounts are detected and handled (bootstrap.rs:745–805). However, no crash-recovery path re-hydrates in-flight IPC requests that were decoded but not responded to before the crash. This is acceptable for the current single-request-per-accept model (each request is atomic), but should be explicitly documented so future concurrent dispatch does not inadvertently leave half-responded IPC frames.

**Remediation:** Document the "one request, one atomic response" invariant in `serve.rs` so future concurrent-dispatch work knows it cannot break this atomicity guarantee without adding a WAL-style journal for in-flight IPC state.

---

### LOW — L2: `pcloud-web` management surface has no authentication beyond same-user IPC
**File:** `crates/pcloud-web/src/lib.rs:44–48`

The web UI correctly binds loopback-only and has no credential handling. However, any local process (not just the daemon owner) can send HTTP requests to `127.0.0.1:<port>` and trigger IPC round-trips. The IPC layer enforces owner-uid, so the daemon itself remains safe, but a lower-privileged process could probe the HTTP surface to determine daemon health/state via `/livez` and `/readyz` without going through the IPC auth gate. This is a minor information-disclosure concern.

**Remediation:** Document the information-disclosure surface. If the management UI ever exposes anything beyond health state, add a session token or local-socket upgrade so HTTP access is auth-gated.

---

### LOW — L3: `pcloud-ipc/tests/stress_concurrent_clients.rs` is `#[ignore]`-gated and not in CI
**File:** `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:1–8`

The stress test (50 clients × 500 sequential requests) is explicitly gated by `#[ignore]` and must be run manually with `--release`. No evidence it is included in any CI job. The test exercises the fd-leak and connection-cap correctness paths that are critical for a production daemon.

**Remediation:** Add a CI job step (or a separate stress-test workflow) that runs the ignored stress test on merges to `development`. At minimum, document in the test file which CI job owns it.

---

## Summary Table

| ID | Severity | Area | File:line |
|----|----------|------|-----------|
| M1 | MEDIUM | Version negotiation | `protocol.rs:255` |
| M2 | MEDIUM | Single-threaded dispatch | `transport.rs:310`, `serve.rs:380` |
| M3 | MEDIUM | Windows peer-auth not wired | `platform/windows.rs`, `transport.rs:865` |
| M4 | MEDIUM | Health-server cap TOCTOU | `health_server.rs:146–171` |
| M5 | MEDIUM | Proptest coverage gap | `proptest_methods_roundtrip.rs:18` |
| L1 | LOW | Crash-recovery documentation | `transport.rs:689`, `bootstrap.rs:745` |
| L2 | LOW | Web UI auth surface | `pcloud-web/src/lib.rs:44` |
| L3 | LOW | Stress test not in CI | `stress_concurrent_clients.rs:1` |

**CRITICAL:** 0  **HIGH:** 0  **MEDIUM:** 5  **LOW:** 3

---

## Held Claims (Audit-05 Improvements Verified)

All five audit-05 security improvements to this section are confirmed present and correctly implemented:
- Privileged-request audit logging with real peer uid/pid (`serve.rs:233–262`)
- Per-peer rate limiter keyed by uid (`rate_limit.rs`)
- IPC socket bind chmod to 0600 / parent 0700 (`transport.rs:664–694`)
- Health server connection cap + loopback binding (`health_server.rs:55,116`)
- macOS `getpeereid` peer-credential path (`platform/unix.rs`)

# Audit 06 — Section 7: IPC & Daemon (Opus)

**Date:** 2026-04-18
**Scope:** `crates/pcloud-ipc/`, `crates/pcloud-daemon/src/`
**Baseline:** Post audit-05 remediations (see header). Verify-held pass.

## Audit-05 Remediation Verification

| Claim | Evidence | Status |
|---|---|---|
| CryptoGet{Folder,File}Key privileged-audited | `crates/pcloud-daemon/src/serve.rs:120-121,152-153` (added to `is_privileged_request` + `request_kind_name`) | HELD |
| peer uid threaded via `serve_once_with_peer` | `crates/pcloud-daemon/src/serve.rs:380-382` (`bound.serve_once_with_peer(|peer, req| dispatch_with_drain_gate(runtime, peer.uid, peer.pid, req))`); comment at `serve.rs:245-253` documents rationale | HELD |
| `PerPeerRateLimiter<u32, SessionRateLimiter>` | `crates/pcloud-daemon/src/rate_limit.rs:156-200` (`peers: Mutex<HashMap<u32, SessionRateLimiter>>`); wired at `bootstrap.rs:834` | HELD |
| `IpcServer::bind` re-chmods pre-existing parent dir | `crates/pcloud-ipc/src/transport.rs:668-686` — `parent_missing` fast path + owner-check re-chmod on existing dir | HELD |
| Stress test Linux-gated | `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:44-57` (`cfg(target_os="linux")` for fd counting); `#[ignore]`-gated (line 5) | HELD |
| Health-server conn cap 32 | `crates/pcloud-daemon/src/health_server.rs:55` (`MAX_CONCURRENT_HEALTH_CONNECTIONS = 32`), enforced at `:148` | HELD |
| macOS `sd_notify` stub | `crates/pcloud-daemon/src/serve.rs:59-73` (cfg `target_os="macos"`, doc-linked TODO(L-2)) | HELD |
| launchd FD SAFETY comments | `crates/pcloud-ipc/src/transport.rs:612-642` — three SAFETY blocks: `launch_activate_socket`, `libc::free`, `UnixListener::from_raw_fd` | HELD |

All eight audit-05 remediations are present in the tree and substantively correct.

## New Findings (Section 7)

### CRITICAL
*(none)*

### HIGH
*(none)*

### MEDIUM

**M-7.1 — `metrics_server::serve_with_metrics` bypasses privileged-request audit logging.**
File: `crates/pcloud-daemon/src/metrics_server.rs:145-170`.
Finding: The Prometheus-enabled serve loop uses `bound.serve_once(|request| ...)` (not `serve_once_with_peer`) and calls `crate::dispatch(runtime, request)` (not `dispatch_with_peer`). Consequently, when the daemon is started under `pcloud-observability`, privileged IPC requests (`CryptoReset`, `Shutdown`, `AccountChangePassword`, etc.) are **not** emitted to the audit log, and peer uid is not threaded into dispatch for future authz checks. Only `serve::serve_until_shutdown_with_flag` carries the audit-05 improvement.
Impact: Audit-05 M-2 regression surface — privileged operations invisible in operator log when metrics are on. Peer-uid-aware rate limiting also loses its keying.
Remediation: Replace with `serve_once_with_peer`; route through `dispatch_with_drain_gate` (or a shared helper) so audit + `dispatch_with_peer` are invoked uniformly across both serve loops. Open a bead under `bd-1du` for the parity fix.

**M-7.2 — `dispatch_with_drain_gate` drops `peer_pid` before dispatch.**
File: `crates/pcloud-daemon/src/serve.rs:245-262`.
Finding: `peer_pid` is logged in the privileged-request line but is not passed to `dispatch_with_peer` (signature at `dispatch.rs:322` only accepts `peer_uid`). If a future authz check wants to correlate with pid (e.g. kill-switch a misbehaving client), it is not available at dispatch time.
Impact: Deferred capability, not a live defect. Low operational risk today.
Remediation: Extend `dispatch_with_peer` signature to accept a `PeerCreds` struct, or stash pid on a per-request context object; noted for the next IPC hardening wave.

### LOW

**L-7.1 — `MAX_IPC_CONNECTIONS = 128` and `MAX_IPC_CONNECTIONS_PER_PEER = 32` are compile-time constants.**
File: `crates/pcloud-ipc/src/transport.rs:44,54`.
Finding: Both caps are `pub const` with no config override. Enterprise deployments with elevated concurrent-CLI use (e.g. automated backup farms) cannot raise the cap without a rebuild.
Remediation: Plumb through `pcloud-config` with validated bounds; default stays 128/32.

**L-7.2 — `serve_with_metrics` uses a divergent drain-admit list.**
File: `crates/pcloud-daemon/src/metrics_server.rs:148-155` vs `serve.rs:212-220` (`should_reject_during_drain`).
Finding: Two near-identical but independent definitions of "which methods survive drain". Drift risk on future `Method::` additions.
Remediation: Extract to a single `drain_admits(&Request) -> bool` helper in `serve.rs`, re-use from both loops.

**L-7.3 — Stress test portability TODO unfiled.**
File: `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:41-42`.
Finding: `TODO(bd-xplat)` without a concrete bead ID; CLAUDE.md policy flags such TODOs as MEDIUM but the capability (fd-leak detection on non-Linux) is purely aspirational. Downgraded to LOW here since the test itself is correctly Linux-gated.
Remediation: File under `bd-1du.4` cross-platform hardware verification if the capability is in scope, or rewrite the comment to say "Linux-only by design".

## Scope Items Confirmed Sound

- **Wire framing** (`protocol.rs:13-100`): explicit `u32 payload_len | u16 version | u16 message_kind`; `IPC_PROTOCOL_VERSION=1` with decoder rejection on mismatch; oversized-frame early-close at `transport.rs:787-812`.
- **Proptest coverage**: `proptest_methods_roundtrip.rs`, `envelope.rs`, `peer_and_protocol.rs`, `security_invariants.rs`, `request_size_cap.rs` collectively cover framing, size caps, peer-cred plumbing, and method roundtrip.
- **Graceful drain**: `crates/pcloud-daemon/tests/graceful_drain.rs` present; drain state machine in `serve.rs:296-344` sound (fresh-drain flag, deadline, in-flight counter, DrainStatus exemption).
- **Runtime dir hygiene**: `transport.rs:668-694` enforces parent `0700` (owner-only re-chmod) + socket `0600`.
- **Connection cap enforcement**: `transport.rs:94-102,356-357` — both global and per-peer slots gated before spawn.

## Summary

Audit-05 remediations all verify held with correct, reviewable code. Two new MEDIUM findings concern the `metrics_server` serve loop being out of step with `serve.rs` on privileged-audit + peer-uid plumbing (M-7.1) and a minor peer_pid plumbing gap (M-7.2). Three LOW items address config-ability and code-reuse polish. No CRITICAL/HIGH issues. Section 7 posture is enterprise-grade on the core `serve.rs` path; metrics serve loop needs one focused pass to reach parity.

# Section 7: IPC & Daemon — Audit 05 (Sonnet independent review)
**Date:** 2026-04-18  
**Auditor:** Claude Sonnet (independent, cross-validating with Opus)

---

## Summary

The IPC/daemon stack is the strongest part of the codebase. Peer-credential
enforcement, framing caps, rate limiting, graceful drain, sd_notify, and the
new crypto dispatch surface are all substantively implemented. Three issues
were found: one MEDIUM and two LOWs.

---

## Findings

### MEDIUM

**M-1 — `CryptoGetFolderKey` / `CryptoGetFileKey` absent from `is_privileged_request` audit list**

File: `crates/pcloud-daemon/src/serve.rs:75–98`

`is_privileged_request()` gates privileged audit-log emission. `CryptoSetupV2`
is correctly listed. However `CryptoGetFolderKey` and `CryptoGetFileKey` are
**not** in the list even though they: (a) require an unlocked crypto shell,
(b) drive a live server round-trip to fetch RSA-OAEP-wrapped symmetric keys,
and (c) are classified `Medium` in the rate limiter (not `Cheap`). A hostile
but owner-uid process probing folder/file IDs via these requests generates no
audit trail. These are not credential mutations, but they leak which
folder/file IDs the attacker is targeting against the crypto backend.

`request_kind_name()` at line 116–117 does classify them correctly by name —
the gap is only in the `is_privileged_request` predicate. Adding them there
costs nothing and closes the observability gap.

**Remediation:** Add `Request::CryptoGetFolderKey { .. } | Request::CryptoGetFileKey { .. }` to the `matches!` block in `is_privileged_request` (serve.rs line ~89).

---

### LOW

**L-1 — `acknowledge_not_interop` gate is request-field only; no CLI warning on Enhanced path**

File: `crates/pcloud-daemon/src/runtime.rs:3042–3051`  
File: `crates/pcloud-cli/src/app.rs:2792–2817`

The Enhanced backend gate (`acknowledge_not_interop == false → InvalidRequest`)
is correctly enforced in dispatch. However the CLI at `app.rs:2792–2817`
silently passes `acknowledge_not_interop: true` when the user specifies
`--backend enhanced` without displaying any confirmation to the user that
they are choosing a non-interoperable format. Enterprise operators may not
understand the implication until they attempt cross-client file access.

**Remediation:** Add a `warn!` or interactive confirmation step in the CLI
before issuing `CryptoSetupV2` with `Enhanced`, or at minimum echo the
non-interop consequence to stderr.

**L-2 — Stress test gated `#[ignore]` with a Linux-specific `/proc/self/fd` call; no non-Linux path**

File: `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:36–41`

The `open_fd_count()` helper reads `/proc/self/fd` directly without a
`#[cfg(target_os = "linux")]` guard. On macOS/FreeBSD this will compile but
panic at runtime if the test is ever un-ignored. There is a TODO comment
(`TODO(bd-xplat)`) acknowledging this, but it is untracked.

**Remediation:** Gate the fd-count helper with `#[cfg(target_os = "linux")]`
and provide a `None` fallback on other platforms, or open a bead for the
cross-platform stress test path.

---

## Verified-correct items (positive findings)

| Area | File | Status |
|---|---|---|
| Peer-UID enforcement (SO_PEERCRED / getpeereid) | `pcloud-ipc/src/server.rs:84–88` | Owner-only socket, per-connection check |
| Frame size cap (1 MiB OOM guard) | `pcloud-ipc/src/server.rs:18–42` | Pre-allocation cap, documented |
| Global + per-peer connection caps (128 / 32) | `pcloud-ipc/src/transport.rs:44–54` | Both caps under mutex, TOCTOU-safe |
| IPC write timeout (30 s) | `pcloud-ipc/src/transport.rs:150,522,719` | Applied to every accepted connection |
| Rate limiting — Expensive/Medium/AuthAttempt buckets | `pcloud-daemon/src/rate_limit.rs` | Token buckets with retry-after hint; fail-open on config error |
| `CryptoSetupV2` dispatch + `acknowledge_not_interop` gate | `pcloud-daemon/src/runtime.rs:3042–3051` | Gate enforced before any local state mutation |
| `CryptoGetFolderKey` / `CryptoGetFileKey` auth gating | `pcloud-daemon/src/runtime.rs:3183,3248` | Requires `is_started()` + live auth token |
| Privileged audit logging | `pcloud-daemon/src/serve.rs:75–214` | Logs before dispatch; covers crypto, shutdown, password lifecycle |
| `sd_notify` (READY/STOPPING/WATCHDOG/RELOADING) | `pcloud-daemon/src/serve.rs:41–55,310,449` | Linux-only, silently no-ops if not supervised |
| `/livez` / `/readyz` health endpoints | `pcloud-daemon/src/health_server.rs` | Loopback-only, disabled by default, privileged-port guard |
| Graceful drain (3-state machine) | `pcloud-daemon/src/serve.rs:140–297` | DrainStatus/Shutdown/GetHealth pass through; everything else 503 |
| Drain test | `crates/pcloud-daemon/tests/graceful_drain.rs` | Present |
| Proptest roundtrip — `CryptoSetupV2`, `CryptoGetFolderKey`, `CryptoGetFileKey` | `pcloud-ipc/tests/proptest_methods_roundtrip.rs:644–665` | All three variants exercised |
| Shutdown propagated from external `Arc<AtomicBool>` (SCM shim) | `pcloud-daemon/src/serve.rs:224–290` | Tested in `serve_with_shutdown_exits_when_flag_set` |

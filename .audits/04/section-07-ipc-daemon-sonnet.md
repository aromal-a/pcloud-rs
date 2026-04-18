# Section 7: IPC & Daemon — Independent Audit
**Auditor:** Sonnet (cross-validate with Opus)
**Date:** 2026-04-18
**Scope:** `crates/pcloud-ipc/`, `crates/pcloud-daemon/src/`

---

## Summary

The IPC and daemon layer is substantially well-engineered. Peer credentials, socket mode, connection caps, write timeouts, drain logic, sd_notify, privileged logging, and proptest coverage are all present and correctly implemented. Two MEDIUM gaps stand out: the new `UploadWriteFromFile` and `CreateTreePublicLinkFromPaths` variants land in the wrong rate-limit bucket (Medium instead of Expensive), and `livez`/`readyz` HTTP probes exist only in `pcloud-web` (no IPC-level equivalents for non-web deployments).

---

## Findings

### MEDIUM — `UploadWriteFromFile` miscategorized as Medium in rate limiter

**File:** `crates/pcloud-daemon/src/rate_limit.rs:203` (wildcard `_ => RateCategory::Medium`)

`Request::UploadWriteFromFile` is handled by the catch-all `_ => RateCategory::Medium` arm in `categorize()`. The handler at `runtime.rs:2483` performs a synchronous `std::fs::read` of a local file into a `Vec<u8>`, then calls `upload_create` + `upload_bytes` — a potentially expensive, blocking, multi-step I/O operation that includes a network round-trip. A hostile or chatty client can fan-out this call under the Medium bucket (which is intentionally more permissive than Expensive). It should be assigned `RateCategory::Expensive` alongside `BackupSnapshot` and `AuditVerifyChain`.

**Remediation:** Add `Request::UploadWriteFromFile { .. } => RateCategory::Expensive` explicitly in `categorize()` before the wildcard.

---

### MEDIUM — `CreateTreePublicLinkFromPaths` miscategorized as Medium in rate limiter

**File:** `crates/pcloud-daemon/src/rate_limit.rs:203` (wildcard `_ => RateCategory::Medium`)

`Request::CreateTreePublicLinkFromPaths` iterates over N paths, issues N `get_folder_id_by_path` network calls, and then calls `create_tree_public_link`. Arbitrarily large `paths` vecs make this unboundedly expensive yet it falls through to Medium. The existing `Request::CreateTreePublicLink { .. } => RateCategory::Expensive` arm at line 194 covers only the pre-resolved-ids variant. A client supplying paths rather than ids bypasses the Expensive bucket.

Note: there is no `paths` length cap validation in `create_tree_public_link_from_paths_ipc` (`runtime.rs:2581`); a 1000-element `paths` slice will issue 1000 sequential HTTP calls under a Medium rate budget.

**Remediation:** (1) Add `Request::CreateTreePublicLinkFromPaths { .. } => RateCategory::Expensive` in `categorize()`. (2) Add a paths-length cap (e.g. 64) with an `InvalidRequest` response before the resolver loop.

---

### MEDIUM — `livez`/`readyz` endpoints only in `pcloud-web`, not IPC-native

**File:** `crates/pcloud-web/src/routes.rs:76-77`

`GET /livez` and `GET /readyz` exist on the optional web surface. Operators deploying the daemon without `pcloud-web` (headless, systemd-only) have no `/livez` or `/readyz` probe. The IPC layer exposes `Method::GetHealth` and `Method::Health` (drain-gate-admitted) that could serve as a basis for these probes, but there is no HTTP shim at the daemon's metrics server (`crates/pcloud-daemon/src/metrics_server.rs`) that would expose them over HTTP for container orchestration platforms (k8s readiness gates, etc.).

**Remediation:** Add `/livez` and `/readyz` GET handlers to `metrics_server.rs` (or document that operators must use `pcloud-web` for these probes). The IPC `Method::GetHealth` response is already drain-gate-admitted and could back both handlers.

---

### LOW — Proptest `every_method()` list missing newer Method variants

**File:** `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:18-51`

The `every_method()` runtime list (used by `every_method_variant_round_trips`) contains 30 entries. Several variants added after the initial list (`Method::Health`, `Method::HaStatus`, `Method::DrainStatus`, `Method::GetSlo`, `Method::GetAuditVerifierStatus`, `Method::GetSyncStatus`, `Method::ListConflicts`, `Method::StatPath`, `Method::GetApiServers`, `Method::GetPromo`, `Method::GetCryptoHint`, `Method::VerifyEmail`, `Method::SessionStatus`, `Method::FileHistory`, `Method::IntegrityStatus`) appear only in the compile-time exhaustiveness guard (`must_match_every_method_variant`), not in the runtime round-trip loop. The file note correctly documents this as a maintenance obligation but the gap means these variants are not actually exercised by `every_method_variant_round_trips`.

The two new variants (`UploadWriteFromFile`, `CreateTreePublicLinkFromPaths`) are correctly covered in the proptest `arb_request()` generator (lines 588-606) and round-trip via `prop_request_round_trips`.

**Remediation:** Extend `every_method()` to include all variants that appear in `must_match_every_method_variant` so the deterministic round-trip test exercises the full surface.

---

### LOW — `sd_notify` sends `WATCHDOG=1` per-request without a watchdog interval guard

**File:** `crates/pcloud-daemon/src/serve.rs:313-314`

`sd_notify("WATCHDOG=1\n")` is sent after every `serve_once` iteration, including accept-timeout wakeups. This is correct (the daemon is alive) but does not check `WATCHDOG_USEC` from the environment to confirm whether watchdog support is even configured. If the unit is deployed without `WatchdogSec=`, the datagram is silently discarded by the non-existing `NOTIFY_SOCKET` path (the `if let Ok(path)` guard already handles this). No correctness concern, but the comment `// A no-op when not supervised by systemd` is imprecise — it is also a no-op when `WATCHDOG_USEC` is absent from an otherwise-systemd environment.

**Remediation:** Low priority. Optionally check `WATCHDOG_USEC` before sending `WATCHDOG=1` and log a one-time debug message if the unit lacks it, to ease operator misconfiguration diagnosis.

---

## What Checks Out (No Findings)

- **Peer credentials:** `SO_PEERCRED` on Linux (`platform/linux.rs`), `getpeereid` on BSD/macOS (`platform/unix.rs`). Windows named-pipe path is documented as scaffolding, not a silent gap.
- **Socket mode:** `0600` socket + `0700` parent directory enforced in `transport::BoundIpcServer::bind`.
- **Connection cap:** `MAX_IPC_CONNECTIONS = 128` with CAS-loop RAII guard (`transport.rs:34-80`).
- **Write timeout:** `IPC_RESPONSE_WRITE_TIMEOUT = 30s` applied on response stream (`transport.rs:86`). Read timeout `5s` on request stream.
- **Privileged logging:** `is_privileged_request()` + `request_kind_name()` correctly enumerate all high-sensitivity operations (shutdown, crypto lifecycle, auth persistence, sync-remove, backup-delete) and log them at `info!` without leaking secret field values (`serve.rs:68-107`).
- **Graceful shutdown:** Three-state drain machine (`Running → Draining → Stopped`); drain gate admits `DrainStatus`, `Shutdown`, `GetHealth`, `Health`; rejects all others; `InFlightGuard` RAII correctly tracks in-flight count. Tested in `tests/graceful_drain.rs`.
- **Auth gating on new variants:** Both `UploadWriteFromFile` and `CreateTreePublicLinkFromPaths` check `auth.snapshot().auth_token` and return `Conflict`/`Unauthorized` when unauthenticated before any I/O (`runtime.rs:2505-2518`, `2600-2614`).
- **Dispatch wiring:** Both new variants are dispatched in `runtime.rs::handle_request` (`lines 820-829`) and have `backend_label` entries in `dispatch.rs` (`lines 207, 209`). No dead-dispatch.
- **Proptest coverage for new variants:** `Request::UploadWriteFromFile` and `Request::CreateTreePublicLinkFromPaths` are both present in `arb_request()` with realistic field generators and exercise `prop_request_round_trips` (`proptest_methods_roundtrip.rs:588-606`).
- **sd_notify READY:** Sent after socket bind before first `accept` iteration (`serve.rs:384`), correctly unblocking systemd `Type=notify` dependents.

# Audit 06 — Transport (HTTP API) & Network Resilience

**Date:** 2026-04-29
**Auditor:** Claude Agent (Opus 4.7, 1M context)
**Scope:** `crates/pcloud-proto/`, `crates/pcloud-resilience/`, `crates/pcloud-config/src/api.rs`, `crates/pcloud-daemon/src/transport_factory.rs`, `crates/pcloud-daemon/src/bootstrap.rs`, plus per-backend transport composition under `crates/pcloud-backends/`. Read-only.

## Summary

The transport layer is in good shape and clearly the most-iterated subsystem in the workspace. TLS 1.3 is pinned at builder time, `danger_accept_invalid_certs` does not exist anywhere in `crates/**/src/`, the production profile rejects plaintext at `ApiEndpoint::validate()`, the API-server hint allowlist refuses non-`*.pcloud.com`/`*.pcloud.link` redirects, and idempotency keys (audit-06 H-4.2) are now threaded end-to-end through `upload_create` → `upload_write` → `upload_save` with a stable per-driver key generated from the OS CSPRNG. `Retry-After` (RFC 7231 IMF-fixdate + delta-seconds, 300 s cap) is honored, retries do not burn budget on `Retry-After`-driven waits (M-1), and a `GlobalRetryBudget` of 100 tokens caps cross-call retries in production.

That said, four real gaps remain that are reasonable to flag for an enterprise readiness gate:

1. **Per-backend transport composition bypasses `ResilientTransport`.** Ten backends (`auth`, `account`, `transfer`, `folder`, `sync`, `shares`, `crypto`, `backup`, `public_link`, `notifications`) instantiate `BinaryApiTransport` directly via `TransportConfig::with_tls(...)` and never go through `TransportFactory::wrap_binary`. The factory is wired in the bootstrap test scaffolding, but the production hot path still uses bare transports — meaning circuit-breaker, rate-limit, and global-retry-budget protections are effectively unreachable in production.
2. **Per-host metric label is dropped.** `pcloud_transport_latency_seconds` and `pcloud_transport_errors_total` are global histograms/counters; the `host` parameter is accepted but discarded (`fn observe_latency(_host: &str, ...)`). An operator cannot break out latency/error rate per API endpoint, which defeats one of the explicit asks of this dimension.
3. **No write-timeout or total-request-timeout knob in `ApiEndpoint`.** The schema only persists `connect_timeout_ms` and `read_timeout_ms`; `write_timeout` and `total_request_timeout` are hard-coded constants on `TransportConfig` (`DEFAULT_WRITE_TIMEOUT = 30s`, `DEFAULT_TOTAL_REQUEST_TIMEOUT = 300s`) and cannot be tuned without a code change.
4. **No `diff` / WebSocket reconnect-with-resume layer.** `DiffApi::poll_diff` is a one-shot RPC; the `diffid` cursor is opaque and persisted by the sync engine, but there is no streaming subscription, no long-poll loop with reconnect-on-disconnect, and no documented reconnect semantics for the diff stream.

No CRITICAL findings.

## Findings by Severity

- CRITICAL: 0
- HIGH: 1
- MEDIUM: 3
- LOW: 3

---

## HIGH

### H-1. Production backends bypass `ResilientTransport` — circuit breaker, rate limit, and global retry budget are dead code in production

- **Severity:** HIGH
- **File:line:**
  - `crates/pcloud-backends/src/auth_backend.rs:283`
  - `crates/pcloud-backends/src/account_backend.rs:235`
  - `crates/pcloud-backends/src/transfer_backend.rs:318`
  - `crates/pcloud-backends/src/folder_backend.rs:263`
  - `crates/pcloud-backends/src/sync_backend.rs:426`
  - `crates/pcloud-backends/src/shares_backend.rs:277`
  - `crates/pcloud-backends/src/crypto_backend.rs:211`
  - `crates/pcloud-backends/src/backup_backend.rs:434`
  - `crates/pcloud-backends/src/public_link_backend.rs:641`
  - `crates/pcloud-backends/src/notifications_backend.rs:179`
  - Factory definition: `crates/pcloud-daemon/src/transport_factory.rs:151-179`
- **Evidence:** Every production backend constructs `BinaryApiTransport::new(TransportConfig::with_tls(...))` inline and uses it directly via `transport.execute(&encoded)` / `transport.execute_with_body(...)`. None of the ten backend modules import `pcloud_proto::resilient_transport::ResilientTransport`, and `grep -rn "resilient_transport\|ResilientTransport\|set_host_label\|with_budget" crates/pcloud-backends/` returns zero matches. The `TransportFactory::wrap_binary` API exists (`crates/pcloud-daemon/src/transport_factory.rs:151`) and the production constructor allocates a `GlobalRetryBudget(100)`, but no caller threads the wrapped transport into the backends. The factory comment at line 26-31 admits this: *"Each feature-domain backend still constructs its own transport locally; touching those call sites is out of scope for this change."*
- **Risk:** In production:
  - The circuit breaker (`CircuitBreaker` in `pcloud-resilience`) never trips on cascading 5xx failures from the API.
  - The token-bucket rate limiter (`TokenBucket`) does not throttle local burst traffic to the API.
  - The shared `GlobalRetryBudget(100)` is allocated by `TransportFactory::new` but no transport ever consumes a token from it.
  - The retry-with-jitter schedule (`BackoffSchedule::ExponentialJittered`) is unreachable; backends only see the inner per-syscall `EINTR`/`EAGAIN` loop in `transport.rs::send_and_receive`.
  - Transport-level metrics (`observe_transport_latency`, `observe_transport_error`) are emitted by `ResilientTransport::execute` only; bypassing the wrapper means the per-attempt latency histogram and error counter are never populated in production.
- **Remediation:** Either (a) thread `TransportFactory` (or a pre-wrapped `Arc<dyn ProtocolTransport>`) into each backend's `Network` constructor and have every backend's `execute` go through the wrapper, or (b) move the `BinaryApiTransport` instantiation up one layer (into the daemon runtime) and pass the resilient wrapper down. Option (a) is a smaller blast radius. Track under a new bead such as `pcloud-rs-ncx.transport-wrap-backends`. Until this lands, claims that production has "circuit breaker + retry budget + rate limit" are not actually observable in the hot path.

---

## MEDIUM

### M-1. Transport metrics drop the `host` dimension; per-endpoint break-down impossible

- **Severity:** MEDIUM
- **File:line:**
  - `crates/pcloud-resilience/src/transport.rs:121` — `pub fn observe_latency(_host: &str, _outcome: TransportOutcomeLabel, latency_secs: f64)` — `_host` is intentionally unused.
  - `crates/pcloud-resilience/src/transport.rs:153` — `pub fn increment_error(_host: &str, class: TransportErrorClass)` — same pattern.
  - `crates/pcloud-resilience/src/transport.rs:107-112` — `latency_histogram()` is a single process-wide handle keyed by name only.
  - `crates/pcloud-resilience/src/transport.rs:117-120` — comment acknowledges the limitation: *"per-host sub-histograms will be wired once the `pcloud-observability` histogram API gains a label dimension."*
- **Evidence:** `register_histogram` in `crates/pcloud-observability/src/metrics.rs:313` is name-keyed only and there is no per-label histogram family API. The transport emits one global latency histogram and one fixed-size 6-element error counter. An SRE looking at "p99 latency for `bineapi-eu.pcloud.com` vs `bineapi.pcloud.com`" cannot do so today.
- **Risk:** Cross-region failover diagnostics, per-endpoint SLO dashboards, and noisy-neighbor isolation are blocked. Hides regressions that affect only one regional API server.
- **Remediation:** Extend `pcloud_observability::metrics::register_histogram` to support a label dimension (or accept a `&[(&str, &str)]` label set at observation time), then wire the actual `host` label through `observe_latency`/`increment_error`. Until then, document this as a known gap and have callers append the host into a tagged metric name (e.g. `pcloud_transport_latency_bineapi_eu_seconds`).

### M-2. `ApiEndpoint` lacks `write_timeout_ms` and `total_request_timeout_ms` knobs

- **Severity:** MEDIUM
- **File:line:**
  - `crates/pcloud-config/src/api.rs:133-189` — `ApiEndpoint` struct only carries `connect_timeout_ms` and `read_timeout_ms`.
  - `crates/pcloud-proto/src/transport.rs:140-147` — `DEFAULT_WRITE_TIMEOUT = 30 s`, `DEFAULT_TOTAL_REQUEST_TIMEOUT = 300 s`, `DEFAULT_INTERRUPT_RETRY_DELAY = 10 ms`, `DEFAULT_MAX_RESPONSE_BYTES = 64 MiB` are all compile-time constants.
  - `crates/pcloud-config/src/api.rs:271-275` — `validate_timeout_composition` is invoked with `total = None` because the config layer cannot supply a total budget.
- **Evidence:** Every backend that builds a transport via `TransportConfig::with_tls(...)` (e.g. `auth_backend.rs:283`) inherits the hard-coded write/total defaults. The `validate_timeout_composition` function (line 354) is wired to enforce `connect ≤ read ≤ total`, but in practice the `total` arm is unreachable through the public config surface.
- **Risk:** Operators on slow links (residential ADSL, satellite) cannot raise the 300 s ceiling for very large uploads. Operators on hardened links (corporate WAF) cannot lower it for tighter SLOs. The defaults are reasonable, but inflexible.
- **Remediation:** Add `write_timeout_ms` and `total_request_timeout_ms` fields to `ApiEndpoint`, wire them through `apply_env_overrides`, validate them with the existing `validate_timeout_composition(connect, read, Some(total))`, and use them at every `BinaryApiTransport::new(TransportConfig::with_tls(...))` call site (10 sites listed under H-1).

### M-3. No `diff` reconnect-with-resume / streaming layer

- **Severity:** MEDIUM
- **File:line:**
  - `crates/pcloud-proto/src/diff_api.rs:174-235` — `DiffApi::poll_diff` is a single-shot RPC.
  - `crates/pcloud-proto/src/diff_api.rs:249` — there is a "resume-safe wrapper" but inspection shows it stores the cursor and re-issues a one-shot poll; it does not maintain a long-lived connection.
- **Evidence:** No WebSocket client is present in the workspace (`grep -rn "tungstenite\|tokio-tungstenite\|websocket" crates/` returns zero matches in production code paths). The pCloud server supports a long-poll `diff` mode (`timeout=N` in the binary protocol) but the Rust client does not pass it; every poll is a fresh TCP+TLS handshake.
- **Risk:** Latency on remote-change propagation = poll interval (often tens of seconds). Cost: every poll re-handshakes TLS, which is wasteful at scale and hostile to mobile / metered networks (the workspace has a `pcloud_resilience::metered::is_metered_network` helper but it does not gate diff polling).
- **Remediation:** Implement a long-poll wrapper that reuses a single TCP+TLS connection (or, if the server adds a WebSocket endpoint, switch to that), with explicit reconnect-with-cursor semantics on disconnect. Track under a new bead under `bd-1du`.

---

## LOW

### L-1. `TlsRevocationCheck::Disabled` is the production default

- **Severity:** LOW (already tracked under `pcloud-rs-t9o`)
- **File:line:** `crates/pcloud-config/src/api.rs:74-81`, `crates/pcloud-proto/src/tls.rs:52-90`.
- **Evidence:** `TlsRevocationCheck::default() = Disabled` and the placeholder hook `_t9o_revocation_placeholder` is documented as a no-op. No CRL or stapled-OCSP enforcement runs even when `StapledStrict` is selected — the implementation has not landed yet.
- **Risk:** A revoked-but-not-yet-rotated server certificate would still be accepted. Acceptable for commercial deployment; non-compliant for FedRAMP-class environments.
- **Remediation:** Already tracked under bead `pcloud-rs-t9o`. No new action.

### L-2. `Connect` errors are unconditionally classified `Transient`

- **Severity:** LOW
- **File:line:** `crates/pcloud-proto/src/resilient_transport.rs:528` — `TransportError::Connect(_) => ErrorClass::Transient`.
- **Evidence:** A `Connect` error covers everything from transient DNS flap to permanent ECONNREFUSED on a typo'd port. Treating all connect failures as transient means a typoed `host` will burn the full retry budget before failing.
- **Risk:** Wasted retry budget on configuration mistakes that will recur.
- **Remediation:** Inspect the underlying `io::ErrorKind` (`AddrNotAvailable`, `PermissionDenied` → Permanent; `TimedOut`, `ConnectionRefused`, `HostUnreachable` → Transient).

### L-3. Auth-token plaintext leakage in `PCLOUD_WIRE_CAPTURE_DIR` dumps

- **Severity:** LOW (already documented in code)
- **File:line:** `crates/pcloud-proto/src/transport.rs:560-563`, `:632-642` — comment explicitly warns *"The captured request bytes contain the auth token verbatim."*
- **Evidence:** The wire-capture directory is created `0o700` and files `0o600`, but the auth token is written in plaintext. Diagnostic-only (off by default), but worth mentioning.
- **Risk:** A misuse of `PCLOUD_WIRE_CAPTURE_DIR` (e.g. by attaching a capture bundle to a bug report) leaks the token.
- **Remediation:** Already documented; consider an additional redaction pass on the captured bytes to scrub the `auth=` parameter before write. Strictly best-effort because the capture's whole point is byte-fidelity diagnostics.

---

## TLS-Enforcement Audit: Where Is the `http://` Gate?

Three independent gates exist; all three reject plaintext in production. **Bypass requires source modification.**

| Gate | File:line | Mechanism |
|---|---|---|
| Config validation | `crates/pcloud-config/src/api.rs:237-241` | `ApiEndpoint::validate` returns `ConfigError::InvalidApiEndpoint` when `environment == Production && mode == Plaintext`. |
| Constructor visibility | `crates/pcloud-proto/src/transport.rs:101` | `TransportConfig::use_tls` is a **private** field. The only constructors are `production()` (TLS=true) and `dev_plaintext()` (explicitly named). Struct-literal construction of a TLS-off prod transport is impossible. |
| TLS client config | `crates/pcloud-proto/src/tls.rs:100-106` | rustls builder pinned to `&[&rustls::version::TLS13]`; TLS 1.2 categorically rejected. Regression test at line 141-176 source-scans the build_config body for `TLS12`. |

**Cert validation:** `grep -rn "danger_accept_invalid_certs\|accept_invalid_hostnames\|InsecureSkipVerify" crates/**/src/` returns **zero matches**. The shared rustls config (`crates/pcloud-proto/src/tls.rs`) uses the Mozilla `webpki-roots` bundle and `with_no_client_auth()`. Not bypassable from `--release`.

**API-server hint allowlist:** `crates/pcloud-proto/src/transport.rs:418-453` and `crates/pcloud-config/src/api.rs:381-383`. Only `*.pcloud.com` and `*.pcloud.link` subdomains are accepted — apex domains and IP literals rejected. Stored persistently in the SQLite preferences row `api_server_binapi` (`crates/pcloud-store/src/repositories/preferences.rs:7,22,40,99`) and re-applied by `bootstrap.rs:483-491` on every daemon start, which gives sticky selection across restarts (re-validated against the allowlist on apply).

---

## Retry / Backoff Policy Table

| Call site | File:line | Schedule | Max attempts | `Retry-After` honored | Budget | Notes |
|---|---|---|---|---|---|---|
| `ResilientTransport` (binary API wrapper, opt-in) | `crates/pcloud-proto/src/resilient_transport.rs:267,400` | `ExponentialJittered { base, factor, max, seed }` from `ResiliencePolicy` | `policy.retry_max_attempts` | n/a (binary protocol; classifier-only) | `GlobalRetryBudget` | Excludes `upload_write`/`upload_save`/`upload_writefromfile` from transport-layer retries (line 391, 456-461). Active **only when** the factory wraps the transport (see H-1). |
| `pcloud-resilience::ResilientTransport` (HTTP layer) | `crates/pcloud-resilience/src/transport.rs:815-` | `MethodRetryPolicy` + global `max_total_attempts` (default 10) | configurable | YES (RFC 7231 IMF-fixdate + delta-seconds, capped 300 s, line 253-277); `Retry-After` waits do **not** burn budget tokens (M-1 fix line 808-814) | `max_total_attempts` cap | Used by HTTP-side callers; not by the binary transport. |
| `BinaryApiTransport::send_and_receive` (inner loop) | `crates/pcloud-proto/src/transport.rs:774-869` | Fixed `interrupt_retry_delay = 10 ms` (line 142-143) | bounded only by `total_request_timeout = 300 s` | NO | none | Only retries `EINTR`/`WouldBlock` per-syscall. Other I/O kinds escape immediately. |
| `UploadStateMachine` (per-chunk upload) | `crates/pcloud-backends/src/upload_state.rs:60-65,262-269` | Fixed 2000 ms | 5 attempts (`DEFAULT_MAX_ATTEMPTS`) | NO | n/a | Spec §6.2; classifier from `pcloud-proto::methods::upload::UploadErrorClass`. Fixed delay, no jitter (matches legacy C). |
| `http_download::fetch_download_resumable` | `crates/pcloud-proto/src/http_download.rs:638-647,623-630` | Server-mandated `Retry-After` (line 386-387 delegates to canonical parser) then single retry | 1 retry then surface | YES | none | Only retries once on 429/503; not exponential. |
| `http_download` per-syscall deadline | `crates/pcloud-proto/src/http_download.rs:347-364` | n/a | n/a | n/a | `total_request_timeout` (10 min default) | Whole-request deadline; bounded slowloris guard. |

**Backoff jitter:** equal-jitter (AWS) variant (`crates/pcloud-resilience/src/retry.rs:230-238`), deterministic per `(seed, attempt)` so tests are reproducible. Production seed lives in `ResiliencePolicy.retry_jitter_seed`.

---

## Idempotency Round-Trip Audit

Audit-06 H-4.2 is **landed** and verified:

- Wire support: `crates/pcloud-proto/src/methods/upload.rs:175,236,308,431` — every upload request struct (`UploadCreateRequest`, `UploadWriteRequest`, `UploadWriteFromFileRequest`, `UploadSaveRequest`) carries an `idempotency_key: Option<String>` and emits a `key=...` parameter when present.
- API surface: `crates/pcloud-proto/src/transfer_api.rs:271-289` — `upload_create_idempotent` is the public threading point; `encode_upload_write_from_file_idempotent` exists at line 535-571.
- Driver scope: `crates/pcloud-backends/src/transfer_backend.rs:1204,1227` — `ChunkedUploadDriver` generates **one** key at construction (`new_idempotency_key()`) and reuses it for all three calls (`create:1261`, `write:1307`, `save:1385`).
- Key generation: `crates/pcloud-backends/src/transfer_backend.rs:1165-1182` — 16 bytes from `getrandom` → 32-hex string (128 bits of entropy). On RNG failure, falls back to a `rngfail-<nanos>` sentinel — visible in logs but still functional.
- Retry safety: `crates/pcloud-proto/src/resilient_transport.rs:391,456-461` — `is_upload_mutation` blocks transport-layer retries for `upload_write`, `upload_writefromfile`, `upload_save`; the `UploadStateMachine` is the authoritative offset-aware retry owner. Combined with the stable key, a server-committed-but-client-timeout cycle is replay-safe.

---

## Observability Matrix: Per-Endpoint Latency / Error Histograms

| Metric | File:line | Cardinality | Per-host? | Per-endpoint (RPC method)? |
|---|---|---|---|---|
| `pcloud_transport_latency_seconds{outcome}` | `crates/pcloud-resilience/src/transport.rs:107-122,195-202` | 1 histogram, 3 outcome labels (`success`/`retry`/`give_up`) | **NO** (`_host: &str` is unused — see M-1) | NO |
| `pcloud_transport_errors_total` | `crates/pcloud-resilience/src/transport.rs:132-183,207-214` | Fixed array of 6 classes (`connect`, `tls`, `io`, `response`, `budget_exhausted`, `circuit_open`) | **NO** | NO |
| Per-method client metrics (e.g. `auth.login`, `upload.create`) | (none observed) | n/a | n/a | n/a |

**Key gap:** the transport metrics exist only when `ResilientTransport` is on the call path, which is currently **not** the production hot path (see H-1). And even when reached, the `host` label is dropped (M-1). An SRE cannot today answer "what is the p99 latency of `upload_save` against `bineapi-eu.pcloud.com`?" from Prometheus alone.

The general `pcloud-observability::metrics` crate exposes a per-method auth/transfer/crypto counter family (`MetricFamilies` in `crates/pcloud-observability/src/metrics.rs:399`), but those are wired by feature backends for high-level outcomes (auth result, transfer direction, crypto lock-state), not by the transport layer for RPC method × endpoint × latency.

**Remediation summary:**
- Wire `TransportFactory::wrap_binary` into all 10 backends (closes H-1).
- Extend `register_histogram` to accept label sets; thread `host` through (closes M-1).
- Add per-RPC-method labels (low-cardinality: ~50 named methods) so observability matches the IPC `Method` taxonomy.

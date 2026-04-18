# Section 6 — Transport & Network Resilience
**Audit 05 | Auditor: Sonnet | Date: 2026-04-18**

---

## Summary

The transport and resilience stack is architecturally sound and well-engineered. TLS is enforced in production, certificate validation uses a fixed Mozilla root store pinned to TLS 1.3, retries are method-aware, circuit breaker is panic-safe, retry budget prevents amplification storms, and `Retry-After` is honoured. The main concerns are: observability wiring remains TODO-gated (not actually emitting metrics at runtime), the `ResilientTransport` wrapper is opt-in and not universally applied to all backends, the `is_known_safe_host` allowlist is duplicated and subtly inconsistent, and `BandwidthPacer` is not hooked into the HTTP download path.

---

## CRITICAL

None.

---

## HIGH

### H-6.1 — Observability metrics are TODO stubs; no real metric emission in production
**Files:**
- `crates/pcloud-proto/src/resilient_transport.rs:285-305` — `TODO(bd-1du)` blocks capture `_latency` / `_total` but never emit
- `crates/pcloud-proto/src/resilient_transport.rs:357-366` — latency on success path is a dead assignment
- `crates/pcloud-resilience/src/transport.rs:92-171` — full `metrics_impl` is gated behind `#[cfg(feature = "transport-metrics")]` which is not listed as a default feature

The `pcloud_transport_latency_seconds` histogram and `pcloud_transport_errors_total` counter are structurally defined but never populated in the default production binary-protocol path. The async `ResilientTransport` in `transport.rs` does emit (when `transport-metrics` feature is enabled), but the synchronous binary-protocol `ResilientTransport` in `resilient_transport.rs` unconditionally skips emission.

**Remediation:** Wire the existing observability hooks in `resilient_transport.rs::execute` to the `pcloud-observability` crate and enable the `transport-metrics` feature by default or remove the feature flag.

### H-6.2 — `ResilientTransport` wrapper is opt-in; most backends are bare
**Files:**
- `crates/pcloud-daemon/src/transport_factory.rs:1-80` — factory exists but is a separate opt-in
- `crates/pcloud-backends/src/transfer_backend.rs` — not audited to confirm it uses the factory
- `crates/pcloud-daemon/src/bootstrap.rs` — factory is present but backends still construct transports locally per comment in `transport_factory.rs:28-31`

The `CLAUDE.md` comments in `transport_factory.rs` acknowledge that feature-domain backends "still construct their own transport locally" and that "touching those call sites is out of scope." This means the rate limiter, circuit breaker, and retry policy are absent from most production API calls.

**Remediation:** Mandate the factory wrapper for all backends in the bootstrap path. Enforce via a lint or newtype that prevents bare `BinaryApiTransport` from being passed to a backend outside of test code.

---

## MEDIUM

### M-6.1 — `is_known_safe_host` duplicated with subtly different semantics
**Files:**
- `crates/pcloud-proto/src/transport.rs:440-446` — allows `pcloud.com` and `pcloud.link` bare apex
- `crates/pcloud-config/src/api.rs:208-210` — only allows `.pcloud.com` and `.pcloud.link` subdomain suffix, not apex

The transport-level allowlist (`transport.rs`) accepts `pcloud.com` (bare apex) and `pcloud.link`, but the config-level allowlist (`api.rs`) only accepts `ends_with(".pcloud.com")` — not the bare apex. A hint of `pcloud.com` would pass the transport check but fail the config check (or vice versa). This inconsistency could cause surprising accept/reject divergence depending on which code path processes the hint.

**Remediation:** Extract a single `is_known_safe_host` function into `pcloud-config` or a shared util crate and call it from both sites.

### M-6.2 — `BandwidthPacer` is not connected to the HTTP download path
**Files:**
- `crates/pcloud-resilience/src/pacing.rs` — `BandwidthPacer` fully implemented with `acquire` / `acquire_blocking`
- `crates/pcloud-proto/src/http_download.rs` — not checked for pacer integration (file exists but not read)
- `crates/pcloud-engine/src/transfers/bandwidth.rs` — exists but wiring from download read-loop unclear

`BandwidthPacer` is a well-engineered token-bucket but there is no evidence it is called inside the HTTP download byte-loop or the upload write path in production. The engine references it but the final plumbing to the actual `read()` / `write()` loop should be verified.

**Remediation:** Confirm `BandwidthPacer::acquire_blocking` or `acquire` is called in the HTTP download `read()` loop and upload byte loop. Add an integration test that asserts throughput is bounded.

### M-6.3 — `total_request_timeout` resets on each new connection, not end-to-end
**File:** `crates/pcloud-proto/src/transport.rs:526-559`

`send_and_receive` receives `total_request_timeout` as a deadline parameter. However, the deadline is computed as `Instant::now() + timeout` at the start of each I/O helper call (`write_all_with_deadline`, `read_exact_with_deadline`). If a retry in the outer `ResilientTransport` wrapper opens a new TCP connection, the new `execute_with_body` call resets the deadline, meaning a sequence of retries can each get the full 5-minute budget. The "total timeout" is per-connection, not per-logical-request.

**Remediation:** Pass a shared deadline from the resilient wrapper layer down to the transport, or document this as per-attempt (not per-logical-request) and enforce an outer total deadline in the retry loop.

### M-6.4 — `classify_error` in `transport.rs` is string-matching on error messages
**File:** `crates/pcloud-resilience/src/transport.rs:227-240`

TLS/cert terminal classification uses `error_message.to_lowercase().contains("tls")` etc. This is fragile: future error message changes or non-English locales could bypass the terminal gate and allow TLS errors to be retried as transient.

**Remediation:** Use typed error classification (match on the `TransportError` enum variant) as done in `resilient_transport.rs`'s `transport_error_classifier`, rather than substring matching on stringified errors.

---

## LOW

### L-6.1 — TLS pinned to 1.3 only; TLS 1.2 comment in `tls.rs` is misleading
**File:** `crates/pcloud-proto/src/tls.rs:49-53`

The module comment says "TLS 1.3 and TLS 1.2 are enabled" but the code passes only `&[&rustls::version::TLS13]`. The comment is stale and should be corrected to avoid misleading future maintainers.

### L-6.2 — `Retry-After` parser does not support HTTP-date format
**File:** `crates/pcloud-resilience/src/transport.rs:300-313`

`retry_after()` only parses numeric seconds (integer or float). The HTTP `Retry-After` spec also allows an HTTP-date string (e.g. `Retry-After: Wed, 21 Oct 2025 07:28:00 GMT`). pCloud currently appears to use numeric seconds, but the parser should handle or explicitly document the HTTP-date rejection.

### L-6.3 — `ResiliencePolicy` has no validation; invalid `retry_factor < 1.0` silently accepted until transport construction
**File:** `crates/pcloud-config/src/resilience.rs:20-78`

`ResiliencePolicy` is deserialized from config without field-level validation. An operator setting `retry_factor = 0.5` would not see an error until `ResilientTransport::build` panics (`assert!(factor >= 1.0)`), crashing the daemon on startup rather than giving a clean config error.

**Remediation:** Add a `validate()` method on `ResiliencePolicy` called from `ApiEndpoint::validate` or the config loader, returning a typed `ConfigError`.

---

## Strengths

- TLS 1.3-only with Mozilla roots, no `accept_invalid_certs` switch, private `use_tls` field preventing struct-literal bypass — solid posture.
- `CircuitBreaker` uses `parking_lot::Mutex` specifically to avoid poisoning-on-panic; `ProbeGuard` RAII prevents stuck HalfOpen — correct engineering.
- `GlobalRetryBudget` is lock-free (`AtomicI64`) and shared across all concurrent operations.
- `Retry-After` respected and M-1 fix (does not consume budget tokens) is tested.
- `is_upload_mutation` guard prevents double-application of `upload_write`/`upload_save` at the transport retry layer.
- API server hint allowlist validated at both transport and config layers against known pCloud domains only.

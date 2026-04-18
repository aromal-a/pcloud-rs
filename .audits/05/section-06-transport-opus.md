# Section 6 — Transport & Network Resilience (Opus)

Audit date: 2026-04-18. Scope: `crates/pcloud-proto/src/{transport.rs,resilient_transport.rs,http_download.rs,tls.rs}` and `crates/pcloud-resilience/`.

## Summary

The transport layer is in good shape overall. TLS is mandated by a private `use_tls` field and a shared rustls config; a circuit breaker, rate limiter, retry policy with jitter, global retry budget, `Retry-After` honouring, error classifier, total-request timeout, max-response cap, and a bandwidth pacer are all implemented and tested. Two parallel resilience stacks exist (binary path under `pcloud-proto/src/resilient_transport.rs`, HTTP path under `pcloud-resilience/src/transport.rs`) which creates inconsistency risk. Observability is wired only in the HTTP stack (feature-gated); the binary stack still has `TODO(bd-1du)` stubs.

---

## CRITICAL

None observed.

---

## HIGH

### H1 — Doc/code drift in TLS version pin (defense-in-depth)
File: `crates/pcloud-proto/src/tls.rs:12` vs `:49–53`.
The rustdoc says "Only TLS 1.3 and TLS 1.2 are enabled"; the code pins only `&[&rustls::version::TLS13]`. Behaviour is correct (TLS1.3-only is stronger, matches audit-04), but the comment is stale and will mislead operators/auditors verifying enforcement. Fix: update the doc to "TLS 1.3 only" and add a compile-time assertion / test that rejects TLS1.2 suites.

### H2 — Error-class classifier is message-substring based in the HTTP stack
File: `crates/pcloud-resilience/src/transport.rs:227–240`.
`classify_error` substrings over `"tls" / "ssl" / "certificate" / "handshake"`. A non-English rustls error, a wrapped/chained error, or a server-produced body containing "SSL" in a legitimate 5xx surface can be miscategorised in either direction. Replace with typed error classification (e.g. a `TransportErrorClass` passed by the caller) — the binary stack already does this correctly via `transport_error_classifier()` (`pcloud-proto/src/resilient_transport.rs:465–490`).

### H3 — Observability TODOs still unlanded on the binary path
File: `crates/pcloud-proto/src/resilient_transport.rs:302–305, 356–365`.
`pcloud_transport_latency_seconds` and `pcloud_transport_errors_total` are only emitted by the HTTP `pcloud-resilience` stack (feature `transport-metrics`). The binary path used by every daemon command is unmetered, so SLO dashboards silently underreport. Wire the existing `pcloud-observability` primitives here as the TODO already describes.

### H4 — Upload idempotency relies on command-name string match
File: `crates/pcloud-proto/src/resilient_transport.rs:416–421`.
`is_upload_mutation` matches only `"upload_write"` / `"upload_save"`. `upload_writefromfile` (the row-93 Partial in CLAUDE.md) and any future mutating variant will be retried by the resilient wrapper and may double-apply bytes. Either introduce a `MethodClass::Idempotent | Mutation` tag on `EncodedRequest` (already done in `pcloud-resilience::retry::RetryClass` — use it), or extend the allowlist and add a compile-time enum so new commands force a classification decision.

---

## MEDIUM

### M1 — Two divergent resilience implementations
`pcloud-proto/src/resilient_transport.rs` (sync, binary-protocol) and `pcloud-resilience/src/transport.rs` (async, HTTP). They have different classifiers, different metric surfaces, different budget semantics (the proto stack's budget replenishes on success, the resilience stack's is attempt-count based), and different `Retry-After` handling (proto stack only honours it for `HttpDownloadError::RetryAfter` on the download path; the binary layer never sees HTTP status). This is a maintainability and correctness risk. Collapse onto a single engine exposing sync and async facades, or document the split explicitly in both files.

### M2 — `Retry-After` cap enforced inconsistently
Files: `pcloud-proto/src/http_download.rs:371–378` (cap 300 s, integer only) vs `pcloud-resilience/src/transport.rs:292–314` (cap 300 s, float-accepting) vs `pcloud-proto/src/resilient_transport.rs` (no `Retry-After` honouring in the binary retry path). Align on a single parser (float, capped, clock-date form rejected with a typed error) and reuse.

### M3 — Circuit breaker state not surfaced as a metric
File: `crates/pcloud-resilience/src/circuit_breaker.rs` (whole file).
Breaker `state()` is read only by tests (`resilient_transport.rs:273`); no Prometheus gauge / event is emitted on open/close transitions. Operators cannot alert on "breaker open > 30 s". Add a callback or gauge.

### M4 — `connect_socket` lacks happy-eyeballs parallelism
Files: `pcloud-proto/src/transport.rs:453–487` and `pcloud-proto/src/http_download.rs:383–421`.
Both iterate resolved addresses sequentially with full `connect_timeout` per address. An IPv6-first host whose AAAA record is black-holed will burn `connect_timeout` before falling back to v4. RFC 8305 happy-eyeballs (start v4 after a 300 ms v6 head start) materially improves user-visible latency.

### M5 — `total_request_timeout` not propagated to `ResilientTransport`
File: `pcloud-proto/src/resilient_transport.rs:316–401`.
The outer retry loop has no wall-clock deadline; per-attempt timeouts apply (via `TransportConfig::total_request_timeout`, `transport.rs:119`), but N retries × 5 min each = 25+ min before `BudgetExhausted` trips. Add a `call_start + total_call_timeout` check at `attempt_start`.

### M6 — BandwidthPacer uses `std::sync::Mutex` with blocking sleep outside
File: `crates/pcloud-resilience/src/pacing.rs:49, 69–80`. Correct, but `Mutex` (vs `parking_lot`) can poison on panic and silently wedge pacing for the daemon lifetime. The circuit breaker doc (`circuit_breaker.rs:23–33`) already calls this out; apply the same reasoning here.

---

## LOW

### L1 — HTTP request line lacks `User-Agent` / `X-Client-Version`
File: `pcloud-proto/src/http_download.rs:1039–1041, 748`. Requests send only `Host` + `Connection: close` (+ `Range`). Server-side diagnostics / abuse controls can't attribute traffic to pcloud-rs.

### L2 — `parse_api_server_hint` accepts only IPv4-style `host:port` via `rsplit_once(':')`
File: `pcloud-proto/src/transport.rs:665–673`. An IPv6 literal hint would be mis-parsed. Hints currently come only from the server (`*.pcloud.com`), so low-risk, but defensive rejection is nicer than silent truncation.

### L3 — `ResilientError` does not implement `Retryable` / carry the underlying `ErrorClass`
File: `pcloud-proto/src/resilient_transport.rs:84–107`. Upstream callers must re-classify after the wrapper returns. Expose the class on the error.

### L4 — `webpki-roots` bundle is statically linked with no CRL / OCSP
File: `tls.rs:46–47`. Acceptable for current scope but worth tracking for FedRAMP-style deployments; document the rationale in `tls.rs` module docs.

### L5 — Tests for `ResilientTransport` use `unsafe` raw-pointer trick
File: `pcloud-proto/src/resilient_transport.rs:668–685`. The comment concedes it; replace with the `Arc<Forward>` pattern already used in `no_retry_on_permfail`.

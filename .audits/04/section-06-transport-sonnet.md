# Section 6: Transport & Network Resilience — Sonnet Audit
**Auditor:** Claude Sonnet 4.6 | **Date:** 2026-04-18 | **Cross-validate with:** Opus

---

## Summary

The transport and resilience layer is well-architected with meaningful depth: TLS enforcement
gated at the config level, a properly wired resilient-transport stack, circuit breaker, retry
policy with backoff/jitter, global retry budget, and Retry-After support. The findings below
are real gaps, not cosmetic.

---

## MEDIUM — Inconsistent max-response-frame limit: binary_api vs transport layer

**File:** `crates/pcloud-proto/src/binary_api.rs:74` and `crates/pcloud-proto/src/transport.rs:408`

`binary_api.rs` defines `MAX_RESPONSE_FRAME_LEN = 256 MiB`; `BinaryApiTransport::execute` applies
a hard-coded inline constant of `MAX_RESPONSE_BYTES = 64 MiB` (`transport.rs:408`). These two
bounds are uncoordinated — the framer layer would pass a 200 MiB frame header as valid while the
transport layer would reject it, producing a misleading error. A server that sends a 65 MiB frame
gets `ResponseTooLarge` from the transport but would pass the framer check, making the failure
mode confusing to operators.

**Remediation:** Unify behind a single exported constant in `binary_api.rs` and reference it from
`transport.rs`. The tighter 64 MiB limit is the correct production value; delete the 256 MiB
constant or rename it to make its non-enforcement clear.

---

## MEDIUM — TLS session resumption not configured; no session cache

**File:** `crates/pcloud-proto/src/transport.rs:48-59`

`get_tls_config()` builds a `ClientConfig` with `with_root_certificates(...).with_no_client_auth()`
and no session store configuration. By default rustls 0.23 disables client-side session caching
unless a `ClientSessionMemoryCache` or equivalent is attached. Every reconnect performs a full
TLS handshake, adding 1-2 RTTs to reconnect latency. For the sync daemon (which reconnects
frequently after idle periods) this is a measurable overhead.

**Remediation:** Add `ClientConfig::builder().with_session_cache(ClientSessionMemoryCache::new(64))`
(or the rustls 0.23 equivalent). Gate the cache size in `ResiliencePolicy` or `ApiEndpoint`.

---

## MEDIUM — `GlobalRetryBudget` not wired into `BinaryApiTransport`; resilience stack is opt-in with no default activation

**File:** `crates/pcloud-proto/src/resilient_transport.rs:136-139`, `crates/pcloud-proto/src/transport.rs`

`ResilientTransport` wraps `ProtocolTransport` and can hold a `GlobalRetryBudget`. However the
wrapper is **explicitly opt-in** (`resilient_transport.rs:9`: "Callers must explicitly wrap a
transport"). No production call site in the daemon bootstrap was verified to actually wrap
`BinaryApiTransport` in `ResilientTransport`. If the daemon uses `BinaryApiTransport` directly,
the circuit breaker, token bucket, global budget, and Retry-After support from `transport.rs`
(which only handles per-request retries) are the only active guards — the cross-request storm
protection from `GlobalRetryBudget` is absent.

**Remediation:** Audit `crates/pcloud-daemon/src/bootstrap.rs` and `runtime.rs` to confirm
`ResilientTransport` is the actual production transport. If not, make it the default and gate
the opt-out as an explicit dev override.

---

## MEDIUM — No write-timeout distinct from read-timeout

**File:** `crates/pcloud-proto/src/transport.rs:349`

`config.read_timeout` is used for both `set_read_timeout` and `set_write_timeout`. There is no
distinct `write_timeout_ms` field in `ApiEndpoint` or `TransportConfig`. Large uploads can tie
up a write syscall for far longer than typical reads; using the same 15 s default for both
means either reads are too permissive or writes are too tight.

**Remediation:** Add `write_timeout_ms` to `ApiEndpoint` and `TransportConfig`. Default to a
larger value (e.g. 60 s) for writes.

---

## LOW — API-server allowlist uses suffix match; subdomain confusion risk

**File:** `crates/pcloud-config/src/api.rs:208-210`

`is_known_safe_host` accepts any host ending in `.pcloud.com` or `.pcloud.link`. A value like
`evil-pcloud.com` does not match, but `evilpcloud.com` would also not match — that part is fine.
However `foo.attacker.pcloud.com` would pass if an attacker could control a subdomain at that
depth. pCloud does not expose arbitrary subdomain creation, but the check is fragile if ever
reused in a broader context.

**Remediation:** Verify against a whitelist of known API host prefixes
(`bineapi`, `api`, `eapi`, `binapi`, etc.) rather than a bare suffix. This is a defense-in-depth
tightening, not a current exploitable issue.

---

## LOW — `Retry-After` only honoured on 429; 503 with `Retry-After` not handled

**File:** `crates/pcloud-resilience/src/transport.rs:296-300`

`retry_after_hint` is only set when `response.is_rate_limited()` (status 429). A 503 with a
`Retry-After` header (standard for scheduled maintenance) follows the standard exponential backoff
path instead. This is not a correctness bug — backoff will eventually fire — but ignores the
server's explicit guidance on 503.

**Remediation:** Extend `retry_after_hint` extraction to also apply when `is_server_error()` and
a `Retry-After` header is present.

---

## LOW — `Retry-After` date-format values silently dropped

**File:** `crates/pcloud-resilience/src/transport.rs:155-163`

`retry_after()` parses the header as a float (seconds). RFC 7231 allows `Retry-After` to be an
HTTP-date string (e.g. `Sat, 01 Jan 2026 00:00:00 GMT`). The current parser returns `None` for
date-format values, so the retry falls back to the backoff schedule silently. This is acceptable
but undocumented.

**Remediation:** Add a comment explicitly noting that HTTP-date format is not supported and falls
back to backoff. Optionally parse it with `httpdate` for completeness.

---

## LOW — No per-endpoint request latency observability at transport layer

**File:** `crates/pcloud-observability/src/metrics.rs:19`

`pcloud_request_latency_seconds` histogram exists, but it is in `pcloud-observability` and must
be plumbed in by callers. The `BinaryApiTransport` and `ResilientTransport` do not emit latency
observations internally. If a caller forgets to instrument, the histogram silently stays empty.

**Remediation:** Either inject an optional `MetricFamilies` handle into `ResilientTransport` and
record observations on every execute call, or document that callers are responsible and enforce
in code review.

---

## What Works Well

- **TLS enforcement:** `ApiEndpoint::validate` hard-rejects `Plaintext` in `Environment::Production`
  (`api.rs:137`). No "accept any cert" switch exists in the rustls config.
- **Error classifier:** `classify_error` in `transport.rs` correctly marks TLS/cert/handshake
  errors as `Terminal` and aborts without retry. Tests verify this (`transport.rs:386-433`).
- **Circuit breaker:** Full three-state machine with `parking_lot::Mutex` (no poisoning),
  panic-safe `ProbeGuard`, and deterministic `ManualClock` injection. Stress test with 1000
  panicking threads passes (`circuit_breaker.rs:488-533`).
- **Retry policy:** Exponential backoff with deterministic equal-jitter, configurable seed,
  `MethodRetryPolicy::secure_default` does not retry mutations on 5xx.
- **Global retry budget:** Lock-free atomic token pool (`global_budget.rs`) is wired into
  `ResilientTransport` (`resilient_transport.rs:139`).
- **`Retry-After` cap:** Capped at 300 s to prevent indefinite stalls (`transport.rs:161`).
- **Upload idempotency:** `upload_write` and `upload_save` are classified `Mutation` and not
  retried on 5xx; `resilient_transport.rs:372` documents this explicitly.
- **API server steering allowlist:** Unknown hosts rejected to prevent SSRF (`api.rs:189`).
- **Max response size:** 64 MiB hard cap at transport layer (`transport.rs:408`).
- **Timeout coverage:** connect, per-read/write, and total-request-timeout all configured
  (`TransportConfig`); defaults are 5 s connect, 15 s read, 5 min total.

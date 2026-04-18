# Section 6 Audit — Transport & Network Resilience

Scope: `pcloud-proto/{transport.rs, resilient_transport.rs, http_download.rs}`,
`pcloud-resilience/*`. Auditor: Opus, 2026-04-18.

## Summary

TLS enforcement is real but policy lives outside the transport struct;
resilience primitives are solid (deterministic clock, parking_lot, panic-safe
ProbeGuard, global budget, upload-idempotency guard). The main weaknesses are
(1) production TLS policy not enforced at the transport constructor,
(2) TLS ClientConfig duplicated between `transport.rs` and `http_download.rs`
with no TLS1.2-minimum / ALPN / revocation configuration, (3) Retry-After is
parsed but not honored by the resilient wrapper, (4) HTTP download is missing
total-request deadline and uses lowest-resolved socket address only,
(5) observability hooks are TODO-only, and (6) API-server steering has no
host allowlist / schema validation.

## Findings

### CRITICAL

None. TLS is mandatory in practice (bootstrap rejects plaintext per
`bootstrap.rs:339,471`) and cert verification uses `webpki-roots` with no
"accept any" switch.

### HIGH

**H1. TLS policy not co-located with `TransportConfig`.**
`transport.rs:86-106` keeps `use_tls: bool` public and explicitly notes
"this field remains public only so that local test harnesses can exercise the
plaintext code path" — but there is no type-level distinction between a
production transport and a test one. A future caller that constructs
`BinaryApiTransport::new(TransportConfig{use_tls:false,..})` bypasses the
bootstrap check entirely (e.g. from `pcloud-sdk` or a plugin). Fix: introduce
`TransportProfile::{Production,Test}` or make `BinaryApiTransport::new`
private with a `::production(host,port)` constructor that hard-codes TLS.

**H2. TLS ClientConfig: no min-version pin, no ALPN, no SNI hardening.**
`transport.rs:48-60` and `http_download.rs:49-61` both build a default
rustls config with only `webpki-roots`. rustls defaults to TLS1.2+ but the
minimum is not pinned by policy; there is no `ALPN` restriction (the server
could negotiate HTTP/2 upgrade on the HTTPS download path unexpectedly), and
no revocation / OCSP stapling verification. Fix: pin `protocol_versions`
to TLS1.2+TLS1.3, set ALPN `["http/1.1"]` on `http_download`, and
centralize one `get_tls_config()` in a shared module to avoid drift.

**H3. Duplicate TLS config cache.** `transport.rs:46` and
`http_download.rs:47` each hold their own `OnceLock<Arc<ClientConfig>>`.
A security fix to one will silently miss the other. Fix: extract to
`pcloud-proto::tls` or a `pcloud-transport-tls` shared crate.

**H4. Retry-After parsed but not honored by the resilient wrapper.**
`http_download.rs:281-288,337-339` parse `Retry-After` and surface
`HttpDownloadError::RetryAfter(Duration)`, but `resilient_transport.rs`
(a) operates on binary-protocol errors only (`TransportError`), and
(b) never inspects a server-sent retry hint — the `RetryPolicy` schedule
wins unconditionally. The binary protocol itself doesn't carry
`Retry-After`, but the `TransportErrorClassifier`
(`resilient_transport.rs:445-462`) has no way to propagate a server-requested
delay. HTTP download callers likewise do not consult `retry_after()`.
Fix: add `RetryDecision::RetryAfter{wait}` override and have the HTTP
download retry path honor it; expose `ResiliencePolicy::max_retry_after`.

**H5. HTTP download lacks total-request deadline.** `transport.rs:124`
enforces `total_request_timeout`; `http_download.rs:83-106` only applies
per-syscall `connect_timeout` and `read_timeout`. A slow-loris-style
server drip-feeding one byte per `read_timeout` window can wedge a
download indefinitely (bounded only by `max_body_bytes`). Fix: add a
`total_request_timeout` field mirroring `transport.rs`, apply it in
`request_and_stream` and `stream_chunked_body`.

### MEDIUM

**M1. `connect_socket` uses first resolved address only.**
`transport.rs:337-342` and `http_download.rs:298-303` call
`to_socket_addrs().next()`. On dual-stack hosts a stale IPv6 record causes
a full connect-timeout wait even when IPv4 would succeed immediately. Fix:
iterate all addresses with a happy-eyeballs short circuit, or at minimum
retry the second address on `ConnectionRefused`.

**M2. API-server steering: no host validation.**
`transport.rs:314-331,522-530` applies any string the server hands back as
the new host/port. A malicious or compromised API response can redirect
the client to an attacker-controlled endpoint; cert verification would
still catch a mismatched SAN, but only if the server name matches exactly.
Fix: validate against an allowlist suffix (e.g. `.pcloud.com`,
`.pcloud.eu`) and reject hints containing schemes, paths, or IPs.

**M3. Observability is TODO-only.** `resilient_transport.rs:302-305,356,
363-365` contain `TODO(bd-1du)` comments for `pcloud_transport_latency_seconds`
and `pcloud_transport_errors_total`. Without metrics the circuit breaker
and retry budget cannot be tuned in production. Per CLAUDE.md's "stricter
than C" posture, missing telemetry is a real gap.

**M4. `TransportError::Io(io::Error)` collapses permanent errors to
transient.** `resilient_transport.rs:454-460` treats every non-TLS,
non-address error as transient. `io::ErrorKind::PermissionDenied`,
`AddrNotAvailable`, and `HostUnreachable` are not retryable and waste
budget. Fix: granular match on `io::ErrorKind`.

**M5. `parse_retry_after` is case-insensitive on prefix but does not
handle HTTP-date form** (`Retry-After: Wed, 21 Oct 2026 07:28:00 GMT`).
`http_download.rs:281-288`. Servers commonly use date form.

**M6. Budget semantics: token consumed only on `Retry` not on
initial failure.** `resilient_transport.rs:381-388` consumes a budget
token before each retry, which is correct, but `replenish(1)` on *every*
success (line 353) allows a high-QPS client with a single slow backend to
keep the pool full while still hammering it. Consider replenishing on a
rate or leaky-bucket cadence.

### LOW

**L1. `is_retryable_io` includes `BrokenPipe` and `ConnectionReset`
inside the deadline loop.** `transport.rs:504-514`. These usually mean the
socket is dead; looping will never succeed. The outer wrapper will retry
anyway; tighten the inner loop to `Interrupted|WouldBlock` only.

**L2. `ResponseTooLarge` limit is hard-coded.** `transport.rs:408`. Make
configurable via `TransportConfig` for future forward-compat.

**L3. `MAX_RESPONSE_BYTES = 64 MiB`** is allocated eagerly with
`vec![0u8; frame_len]` (`transport.rs:415`). A server lying about
`frame_len` near 64 MiB causes a single giant allocation. Prefer
streaming parse or bounded pool.

**L4. `fetch_download` retry-after advertised as retryable but
`fetch_download_resumable` never honors it.** `http_download.rs:172-181,
461`. No caller sleeps `retry_after()` before retry.

**L5. No `Keep-Alive` / connection reuse.** Every `execute` opens a new
TCP+TLS session (`transport.rs:305`). Acceptable for correctness but
costly; document explicitly.

**L6. Classifier returns `Arc<dyn Fn>`** (`resilient_transport.rs:119`).
Fine, but `default_classifier` marks everything transient including
`std::io::ErrorKind::InvalidInput` — dangerous default; prefer "everything
permanent unless explicitly listed".

## Recommended P0 actions

1. Fix H1 (type-gated TLS), H2 (pin TLS1.2+, ALPN), H3 (single config).
2. Fix H4 (honor Retry-After) and H5 (HTTP total-deadline).
3. Wire observability (M3) and host allowlist for API-server hints (M2).

No CRITICAL findings; the direct-shim path as shipped meets the
"TLS mandatory, cert-verified, bounded memory" bar. The HIGH findings
are pre-release polish, not parity blockers.

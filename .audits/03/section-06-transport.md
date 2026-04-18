# Section 6: Transport & Network Resilience
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 6)

## Findings

### CRITICAL [3]
### HIGH [6]
### MEDIUM [6]
### LOW [4]

Scope audited:
- `crates/pcloud-proto/src/transport.rs` (sync binary transport)
- `crates/pcloud-proto/src/resilient_transport.rs` (sync wrapper)
- `crates/pcloud-proto/src/http_download.rs` (HTTP signed-download path)
- `crates/pcloud-resilience/{retry,circuit_breaker,global_budget,transport}.rs`
- `crates/pcloud-resilience/src/lib.rs` (module registration / re-exports)
- `crates/pcloud-daemon/src/transport_factory.rs`
- `crates/pcloud-daemon/src/bootstrap.rs` (api_server hint replay)
- `crates/pcloud-config/src/api.rs` (ApiMode / TLS gate)

Tooling used: `cargo check -p pcloud-resilience` (passes), grep audits for dangerous flags and missing module registrations.

---

## CRITICAL Findings

### C-1. `pcloud-resilience/src/transport.rs` is orphan dead code and references a non-existent `RetryDecision::Exhausted` variant

**Files:**
- `crates/pcloud-resilience/src/transport.rs:312, 322, 331`
- `crates/pcloud-resilience/src/retry.rs:51-59` (enum defines only `Retry { wait }` and `GiveUp`)
- `crates/pcloud-resilience/src/lib.rs:48-57` (module list — `transport` NOT listed)

The file `crates/pcloud-resilience/src/transport.rs` defines an async `ResilientTransport` that carries all four advertised "Fix" mitigations (Terminal TLS gate — Fix 1; MethodRetryPolicy gating — Fix 2; Retry-After honouring — Fix 3; global attempt cap — Fix 4). Three match arms reference `RetryDecision::Exhausted`, which is not a real variant. The crate still builds because `mod transport;` is **absent** from `lib.rs` (confirmed via `Grep "pub mod transport"` in `crates/pcloud-resilience/src`). Workspace-wide grep finds no importer of `pcloud_resilience::transport::*`.

**Impact:**
- The entire async transport executor is unreachable code; the `MethodRetryPolicy::next` path, the 429 Retry-After handling, the configurable `max_total_attempts`, and the TLS-terminal classifier are all dead.
- Operators and reviewers who read `retry.rs` (line 215-316 defines `MethodRetryPolicy::secure_default` which "only retries idempotent methods") reasonably believe the transport enforces that rule. It does not, because the only live transport wrapper is `pcloud_proto::resilient_transport::ResilientTransport`, which ignores method class entirely.
- Any documentation or release note claiming "Retry-After honouring" or "method-aware retry" is materially false for every live request path.

**Remediation:**
1. Add `Exhausted` to `RetryDecision` in `retry.rs:51-59` (semantics: "budget fully consumed, do not retry again"), or rewrite lines 312/322/331 of `transport.rs` to only match `GiveUp`.
2. Register the module: add `pub mod transport;` to `pcloud-resilience/src/lib.rs`.
3. Re-export `TransportOutcome`, `ResilientTransport`, `ResilientTransportConfig`, `classify_error` from `lib.rs` so it is reachable.
4. Wire `http_download.rs` and/or the binary wrapper through this executor so the MethodRetryPolicy, Retry-After, and terminal-TLS gates actually run.

---

### C-2. `TransportConfig` has no total-request timeout; per-stage deadlines allow unbounded slow-drip attacks

**Files:**
- `crates/pcloud-proto/src/transport.rs:70-101` (`TransportConfig` — only `connect_timeout`, `read_timeout`, `interrupt_retry_delay`; no outer/total deadline)
- `crates/pcloud-proto/src/transport.rs:321-323` (`execute_plain` hard-codes `Duration::from_secs(15)` ignoring config)
- `crates/pcloud-proto/src/transport.rs:342` (`execute_tls` reuses `read_timeout` as the stage deadline)
- `crates/pcloud-proto/src/transport.rs:374-451` (`write_all_with_deadline`, `read_exact_with_deadline` deadlines reset per helper)

Each helper (`write_all_with_deadline`, `flush_with_deadline`, `read_exact_with_deadline`) computes its own `deadline = Instant::now() + timeout` at entry. A malicious or saturated server can send exactly one byte just before each stage's deadline expires. The outer call loops over write request, optional body write, flush, read 4-byte header, read frame body — five stages, each refreshing its own timer. With `read_timeout = 15 s` the effective ceiling per request is 75+ seconds *plus* connect + flush budget, and the code never checks a call-wide wall-clock. For `execute_plain` the hard-coded 15 s is doubly problematic since it overrides the user-configured `read_timeout` entirely.

**Impact:**
- Slowloris / slow-drip attacks on the binary API channel can pin worker threads well past any SLO.
- DoS surface: a compromised or flaky endpoint can trickle data just fast enough to renew the per-stage timer and hold resources indefinitely as long as each stage's progress continues below per-read granularity.
- Audit claim in comment (line 25-26: "Timeouts bound every read and write via a deadline loop so that a stuck server cannot wedge a caller indefinitely") is misleading.

**Remediation:**
1. Add `total_request_timeout: Duration` to `TransportConfig`.
2. Compute `let deadline = Instant::now() + config.total_request_timeout;` inside `execute_with_body` (line 262-273) and pass `deadline: Instant` (not `timeout: Duration`) to `send_and_receive` and each helper.
3. Each helper must return `TransportError::TotalTimeoutExceeded` the moment `Instant::now() >= deadline`, independent of whether the per-stage loop is still making small progress.
4. Remove the `Duration::from_secs(15)` hard-code in `execute_plain` and use the shared deadline.

---

### C-3. No response-size cap on the binary protocol; 4 GiB heap allocation from a single untrusted frame header

**File:** `crates/pcloud-proto/src/transport.rs:363-365`

```
let frame_len = parse_response_frame_len(&header)? as usize;
let mut body = vec![0u8; frame_len];
```

`parse_response_frame_len` returns a `u32` derived from the 4-byte little-endian length prefix the remote sent. `as usize` widens to `usize` and then `vec![0u8; frame_len]` allocates that many bytes *before* any body is read. `ParseLimits::default()` (line 190-200 of `response.rs`) does enforce `max_frame_len = 1_048_576` *inside* `parse_response_frame`, but by that point the allocation has already succeeded (or the kernel OOM-killed the daemon).

**Impact:**
- A hostile or compromised endpoint returns a 4-byte prefix of `0xFFFFFFFF` and the daemon immediately requests `Vec<u8>` of 4 GiB, irrespective of session, auth state, or parse limits.
- On memory-constrained machines (Raspberry Pi, containers with cgroup limits, embedded installs) this is an instant crash.
- This vector is reachable pre-parse, so `ParseLimits` cannot protect against it as currently sequenced.

**Remediation:**
1. Add `max_response_bytes: usize` to `TransportConfig` (default 16 MiB or similar, but ≥ `ParseLimits::default().max_frame_len`).
2. In `send_and_receive` (line 345-372) compare `frame_len > config.max_response_bytes` **before** `vec![0u8; frame_len]` and return a new `TransportError::ResponseTooLarge { len: frame_len }`.
3. `HttpDownloadConfig` already enforces `max_body_bytes` correctly (line 77, 332-336 in `http_download.rs`); port the same discipline here.

---

## HIGH Findings

### H-1. Production transport uses `default_classifier` — ALL errors treated as Transient

**Files:**
- `crates/pcloud-daemon/src/transport_factory.rs:118` (`default_classifier::<TransportError>()`)
- `crates/pcloud-proto/src/resilient_transport.rs:376-381` (`default_classifier` — "every inner error is treated as transient")
- `crates/pcloud-proto/src/resilient_transport.rs:394-411` (`transport_error_classifier` — a smart classifier EXISTS)

A smart classifier `transport_error_classifier()` that correctly maps `InvalidAddress`, `InvalidServerName`, and `Tls(_)` to `Permanent` is defined at line 394 but **never used**. `TransportFactory::wrap_binary` calls `ResilientTransport::new(..., default_classifier::<TransportError>(), ...)` (factory.rs:118) which returns `ErrorClass::Transient` for every variant.

**Impact:**
- Permanent TLS misconfiguration (expired cert, name mismatch, no SAN) burns the full retry budget before failing — N × exponential backoff of pure waste.
- Invalid DNS names are retried as if they might resolve next time.
- Security events (certificate rejection) are masked behind retry noise instead of failing fast and surfacing to operators.
- Operationally: a single bad config triggers an N-attempt delay before the user gets feedback.

**Remediation:** In `transport_factory.rs:118` replace `default_classifier::<TransportError>()` with `pcloud_proto::resilient_transport::transport_error_classifier()`. Add a unit test that constructs a transport with `InvalidServerName` and asserts it fails after exactly one attempt.

---

### H-2. `GlobalRetryBudget` exists and is wire-able but production factory does NOT attach one

**Files:**
- `crates/pcloud-resilience/src/global_budget.rs` (full module — defined, tested, re-exported at `lib.rs:63`)
- `crates/pcloud-proto/src/resilient_transport.rs:207-231` (`ResilientTransport::with_budget` exists)
- `crates/pcloud-daemon/src/transport_factory.rs:112-121` (`TransportFactory::wrap_binary` calls `ResilientTransport::new`, NOT `with_budget`; `budget: None`)

The previous audit (`.audits/02/section-06-transport.md`, C-2) reported that `GlobalRetryBudget` was entirely un-wired. That has been *partially* corrected — the wrapper now supports budgets via `with_budget`, and the budget is checked in `execute` at line 352-356. But the production factory still never attaches a budget.

**Impact:** Cross-operation retry-storm protection is available but not enabled. Under a broad outage with many concurrent calls, aggregate retry amplification is unbounded (still per-call capped by `retry_max_attempts`, but across N concurrent in-flight requests you get `N × retry_max_attempts` attempts hitting the struggling backend).

**Remediation:**
1. Store an `Arc<GlobalRetryBudget>` on `TransportFactory` (one per process, sized from `ResiliencePolicy::global_retry_budget` — add the knob if absent).
2. Change `wrap_binary` (line 112-121) to call `ResilientTransport::with_budget(..., budget.clone())`.
3. Consider replenishing tokens periodically (e.g., a tokio interval refilling `cap / 10` tokens per second) so transient exhaustion is self-healing; the current code only replenishes on per-call success.

---

### H-3. `rustls::ClientConfig` + `RootCertStore` rebuilt on every request (no session resumption, heavy alloc)

**Files:**
- `crates/pcloud-proto/src/transport.rs:325-343` (`execute_tls`)
- `crates/pcloud-proto/src/http_download.rs:222-233` (`fetch_download_verified_streaming`)
- `crates/pcloud-proto/src/http_download.rs:608-618` (`range_stream`)

Each `execute_tls` call constructs `RootCertStore::empty()`, copies all `webpki_roots::TLS_SERVER_ROOTS` entries, builds `ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()`, then wraps it in `Arc::new`. This happens twice per HTTP download (main + range path) and once per binary request. No caching, no TLS session resumption, no session tickets.

**Impact:**
- Measurable per-request CPU + heap cost (the root store copy plus rustls config allocation) — typically single-digit ms on server-class CPUs, higher on embedded.
- No TLS 1.3 session resumption across requests on the same host → every request pays the full handshake RTT and CPU.
- Under bulk operations (listing many folders, resumable upload loop) this dominates wall time.

**Remediation:** Add `static TLS_CONFIG: OnceLock<Arc<ClientConfig>>` (or cache inside the transport struct itself) and initialize once per process. Rustls's `ClientConfig` is designed to be shared across connections.

---

### H-4. `execute_with_body` lacks transport-layer observability; no per-endpoint latency or error-class metrics

**Files:**
- `crates/pcloud-proto/src/transport.rs:261-273`
- `crates/pcloud-proto/src/resilient_transport.rs:282-294` (documents the intent; has TODO(H-3) comments; no instrumentation emitted)
- `crates/pcloud-proto/src/http_download.rs` (entire module; no spans, no counters)

The resilient wrapper captures `let _latency = attempt_start.elapsed();` but discards it (lines 337, 344). The TODO comments at lines 291-294 explicitly note `pcloud_transport_latency_seconds{host, outcome}` histogram and `pcloud_transport_errors_total{host, class}` counter are planned but not wired because `pcloud-observability` is not a dependency of `pcloud-proto`.

**Impact:**
- Operators cannot SLO-monitor network latency or error rate per pCloud API endpoint.
- Cannot alert on TLS error spikes vs. IO timeouts vs. parse errors separately.
- Post-incident forensics lack per-endpoint timing.

**Remediation:** Add `pcloud-observability` as a (possibly feature-gated) dependency; instrument `BinaryApiTransport::execute_with_body` and `fetch_download_verified_streaming` with a histogram and a counter keyed by `{host, outcome}` and `{host, class}` respectively. Classes: `invalid_address | connect | tls | io | response_header | response_body`.

---

### H-5. Upload mutations are auto-retried without method-class awareness

**Files:**
- `crates/pcloud-proto/src/resilient_transport.rs:295-369` (`execute` — retries every `Transient` error irrespective of command)
- `crates/pcloud-resilience/src/retry.rs:215-316` (`MethodRetryPolicy` / `RetryClass` defined but not consumed here)
- `crates/pcloud-proto/src/transfer_api.rs` (upload_write / upload_save not marked non-retryable)

`ResilientTransport::execute` has no `RetryClass` parameter; every request is treated identically. Upload commits (`upload_save`) and especially `upload_write` to a chunk that partially succeeded on the server but timed out on the client can double-apply bytes on retry. The `UploadStateMachine` in `pcloud-backends/src/upload_state.rs` mitigates by tracking `acked_offset`, but nothing prevents the outer transport wrapper from concurrently re-issuing the same call.

**Impact:** Data corruption risk under timeout-induced retries of upload mutations. Idempotency comment in the doc header (line 27-30: "any embedded auth tokens pass through by reference — no extra cloning and no leakage surface") only addresses tokens, not write semantics.

**Remediation:** Either (a) plumb `RetryClass` into `execute` so callers can mark `upload_write`/`upload_save` as `Mutation` and rely on `MethodRetryPolicy::secure_default` to refuse those retries at the transport layer, or (b) explicitly disable the outer retry for mutation endpoints and let `UploadStateMachine` own all retry semantics.

---

### H-6. `is_retryable_io` in binary transport is overly narrow; misses common transient kinds

**File:** `crates/pcloud-proto/src/transport.rs:453-458`

```
fn is_retryable_io(err: &io::Error) -> bool {
    matches!(err.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock)
}
```

Compare to `pcloud-resilience/src/transport.rs:50-57` (`is_retryable_io_kind`) which correctly includes `TimedOut | ConnectionReset | BrokenPipe | ConnectionAborted | Interrupted | WouldBlock`. The binary transport layer's deadline loops will NOT re-enter on `ConnectionReset` or `BrokenPipe`, immediately surfacing a transient network blip to the caller.

**Impact:** More aggressive than needed error surfacing for transient in-progress network drops; the outer `ResilientTransport::execute` will retry them anyway, but with an added full-connection teardown/reconnect cost per blip.

**Remediation:** Copy the correct set from `pcloud_resilience::transport::is_retryable_io_kind`. Better: export that function from `pcloud-resilience` (once C-1 is fixed) and call it from both places.

---

## MEDIUM Findings

### M-1. `is_known_safe_host` gate not present for bootstrap-replay of `api_server_binapi`

**Files:**
- `crates/pcloud-daemon/src/bootstrap.rs:447-449`
- `crates/pcloud-config/src/api.rs:178-190` (`apply_api_server_hint` — no validation)

Grep confirms: `is_known_safe_host` does not exist anywhere in the workspace (only referenced in the audit question itself). On bootstrap, if the preferences DB contains a stored `api_server_binapi` value, it is replayed into `config.api` via `apply_api_server_hint` without any allow-list check. The hint parser (line 205-213 of `api.rs`) accepts any string that loosely resembles `host[:port]`.

**Impact:**
- A compromised or tampered preferences DB (SQLite file on the user's filesystem) can redirect every subsequent API request to an attacker-controlled endpoint. TLS still validates the cert name against `server_name`, which `apply_api_server_hint` also sets from the attacker value — so TLS validation passes if the attacker controls a valid cert for the chosen name, or the attacker picks a domain they own.
- Under the project's own rule ("do not reintroduce silent persistence-driven behaviour that weakens transport policy") this is a soft violation: transport re-target is driven by on-disk state with no operator confirmation.

**Remediation:**
1. Define a hard allow-list of known pCloud binapi host suffixes (`*.pcloud.com`, `*.pcloud.link`, etc.) plus any operator-configured allow-list via env var.
2. In production (`Environment::Production`), reject persisted hints that fail the allow-list check — emit a hard bootstrap error, not `log::warn`.
3. Development/Test can keep the permissive behaviour.

### M-2. HTTP download path silently accepts plaintext when `use_tls = false`

**File:** `crates/pcloud-proto/src/http_download.rs:219, 234-236`

```
let port = download.port.unwrap_or(if config.use_tls { 443 } else { 80 });
...
} else {
    let mut plain_stream = stream;
    request_and_stream(&mut plain_stream, ...)
}
```

`HttpDownloadConfig` has `use_tls: bool` with no environment-dependent enforcement. No caller-side check gates plaintext in production. Signed-download URLs returned by `getfilelink` do carry a TLS host by design, but nothing in `fetch_download_verified_streaming` prevents a caller from passing `use_tls=false` in production profile.

**Remediation:** Either add an `Environment` parameter and refuse plaintext in `Production`, or make `use_tls` non-configurable (always true) and remove the plaintext code path from production builds behind `#[cfg(test)]`.

### M-3. `Retry-After` honouring exists only in the orphan module and HTTP download path

**Files:**
- `crates/pcloud-proto/src/http_download.rs:268-275, 325-329` (parses `Retry-After` for 429/503, caps at 300 s) — correct
- `crates/pcloud-proto/src/resilient_transport.rs` — binary wrapper has no HTTP headers concept; no `Retry-After` support
- `crates/pcloud-resilience/src/transport.rs:145-159` — has Retry-After parsing, but orphan (see C-1)

The synchronous binary-protocol `ResilientTransport::execute` ignores server pacing signals entirely. Backoff is always computed from the local schedule regardless of what the server asks.

**Remediation:** Since the binary protocol does not carry HTTP-style headers, the pCloud API would need an in-band backoff hint (e.g., `result=X + x-retry-after-seconds` field). If one exists, plumb it into the classifier return so it can override the backoff schedule. If not, accept this as a protocol limitation and document it.

### M-4. `http_download.rs` caps `Retry-After` at 300 s but `resilient_transport::TransportResponse::retry_after` (orphan) caps at 300 s too — no operator override

**Files:**
- `crates/pcloud-proto/src/http_download.rs:275` (`Duration::from_secs(secs.min(300))`)
- `crates/pcloud-resilience/src/transport.rs:156-158` (`capped = secs.min(300.0)`)

A server doing scheduled maintenance and returning `Retry-After: 1800` (30 min) gets clamped to 300 s and the client hammers back 6× faster than requested. Fine for DoS protection, surprising for legitimate maintenance.

**Remediation:** Make the cap a configuration field on `ResilientTransportConfig` / `HttpDownloadConfig` with a secure default of 300 s but overridable by operators.

### M-5. Circuit breaker is wired in the sync binary wrapper but not in the async HTTP download path

**Files:**
- `crates/pcloud-proto/src/resilient_transport.rs:317-324` — breaker wired
- `crates/pcloud-proto/src/http_download.rs` — no breaker

The HTTP download path (`fetch_download_verified_streaming`, `fetch_download_resumable`) has no circuit breaker. A pCloud CDN endpoint going down causes every download attempt to wait the full connect+TLS handshake budget.

**Remediation:** Wrap `fetch_download_*` with a `CircuitBreaker` + `ProbeGuard`, keyed on `(host, port)`. The existing breaker implementation (`pcloud-resilience/src/circuit_breaker.rs`) is well-tested and usable as-is.

### M-6. `backoff()` thread::sleep busy-loop floor

**File:** `crates/pcloud-proto/src/transport.rs:460-464`

On every `Interrupted`/`WouldBlock` in the write/read loop, the helper calls `thread::sleep(delay)` where `delay = config.interrupt_retry_delay` (default 10 ms). Under SIGALRM or other signal storms (e.g., when the daemon is under heavy VMM scheduler pressure), this is 100 Hz wakeups per blocked syscall. Not catastrophic but worth noting as a power / battery cost on laptops.

**Remediation:** Implement exponential backoff inside `backoff()` (e.g., start at `interrupt_retry_delay`, double up to 1 s, reset on progress) instead of a fixed floor.

---

## LOW Findings

### L-1. Hex-encoding re-implementation in `http_download.rs`

**File:** `crates/pcloud-proto/src/http_download.rs:256-263`

```
let mut s = String::with_capacity(bytes.len() * 2);
for b in bytes {
    write!(s, "{:02x}", b).unwrap();
}
```

Should use a workspace `hex` or `base16ct` dependency (already present for other subsystems) or a lookup-table variant. Minor.

### L-2. `connect_socket` only tries the first resolved address

**Files:**
- `crates/pcloud-proto/src/transport.rs:295-314`
- `crates/pcloud-proto/src/http_download.rs:278-301`

```
let mut addresses = (host, port).to_socket_addrs()...;
let address = addresses.next().ok_or(...)?;
```

If DNS returns multiple A/AAAA records and the first is dead (e.g., an IPv6 route blackhole while IPv4 works), the connect fails immediately. No happy-eyeballs, no multi-address retry.

**Remediation:** Iterate all resolved addresses (or at least the first N) with short per-address timeouts; return a combined error if every address fails.

### L-3. No WebSocket / push notification transport

`listnotifications` + `diff` are poll-based. No real-time push. Acceptable design, but should be explicitly documented as a known limitation so operators don't expect sub-second change propagation.

### L-4. `TransportConfig::interrupt_retry_delay` is publicly mutable but undocumented

**File:** `crates/pcloud-proto/src/transport.rs:96-101`

Tests set it to `Duration::ZERO`; production leaves it at 10 ms. No user-facing docs describe the knob. Either remove the public API (hide behind a `#[cfg(test)]` helper) or document the production guidance.

---

## Summary Table

| Area | Status | Top Finding |
|------|--------|-------------|
| TLS production gate | Enforced in config | OK |
| Cert validation (no dangerous flags) | Clean | H-3 (rebuild-per-request) |
| Timeouts (connect/read) | Partial | C-2 (no total deadline) |
| Retry policy | Wired for binary only | H-5 (no method-class awareness) |
| Retry-After honoring | HTTP only | M-3 |
| Global retry budget | Plumbed but not attached | H-2 |
| RetryDecision::Exhausted | References non-existent variant | C-1 |
| Error classification | `default_classifier` in production | H-1 |
| Upload idempotency | Mitigated by state machine only | H-5 |
| Circuit breaker (binary) | Wired | OK |
| Circuit breaker (HTTP) | Missing | M-5 |
| TLS config caching | Rebuilt per request | H-3 |
| Observability | Missing at transport layer | H-4 |
| API-server steering validation | No allow-list on persisted hints | M-1 |
| Response size cap (binary) | Missing | C-3 |
| Response size cap (HTTP) | Enforced | OK |

---

## File Citations Recap

- `crates/pcloud-proto/src/transport.rs` — C-2, C-3, H-3, H-6, M-6, L-2, L-4
- `crates/pcloud-proto/src/resilient_transport.rs` — H-1, H-4, H-5, M-3, M-4
- `crates/pcloud-proto/src/http_download.rs` — H-3, M-2, M-5, L-1, L-2
- `crates/pcloud-resilience/src/transport.rs` — C-1 (orphan)
- `crates/pcloud-resilience/src/retry.rs` — C-1 (missing Exhausted), H-5
- `crates/pcloud-resilience/src/global_budget.rs` — H-2
- `crates/pcloud-resilience/src/lib.rs` — C-1 (missing `mod transport`)
- `crates/pcloud-resilience/src/circuit_breaker.rs` — OK (well-tested; unused in HTTP path — M-5)
- `crates/pcloud-daemon/src/transport_factory.rs` — H-1, H-2
- `crates/pcloud-daemon/src/bootstrap.rs` — M-1
- `crates/pcloud-config/src/api.rs` — M-1 (production TLS gate at line 137-141 is correct)

## Delta vs `.audits/02/section-06-transport.md`

- Previously-reported C-2 (GlobalRetryBudget not re-exported / never wired) is **partially remediated** — the module is now re-exported at `lib.rs:63` and `ResilientTransport::with_budget` exists. The production factory still does not *attach* a budget. Re-tiered to H-2.
- Previously-reported H-1 (`total_request_timeout` dead) was framed as "the field exists but is ignored." In current code the field does not exist at all. Re-tiered to C-2.
- M-5 (binary-protocol `frame_len` unbounded allocation) — promoted to CRITICAL (C-3) because the size cap inside `ParseLimits` runs after the allocation already occurred; this is a pre-parse DoS vector.
- Added H-6 (`is_retryable_io` narrower in binary path than in resilience crate).
- Added M-2 (HTTP download path accepts plaintext in production).
- Added L-2 (happy-eyeballs / multi-address retry missing).

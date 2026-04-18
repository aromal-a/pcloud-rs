# Section 6: Transport & Network Resilience
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 6)

## Findings

### CRITICAL [2]
### HIGH [5]
### MEDIUM [6]
### LOW [3]

---

## CRITICAL Findings

### C-1. `pcloud-resilience/src/transport.rs` references non-existent `RetryDecision::Exhausted`

**File:** `crates/pcloud-resilience/src/transport.rs:289,299,308`

Module pattern-matches `RetryDecision::GiveUp | RetryDecision::Exhausted`, but the `RetryDecision` enum in `retry.rs:51-59` only defines `Retry { wait }` and `GiveUp`. No `Exhausted` variant exists.

**Impact:** The entire async `ResilientTransport` (HTTP retry executor) is dead/broken code. All transport-level fixes — Terminal TLS gate, MethodRetryPolicy gating, Retry-After honoring, global budget — are inaccessible. The crate only compiles because this module is never imported by any other crate (confirmed by grep: no external users of `pcloud_resilience::transport::ResilientTransport`).

**Fix:** Either add an `Exhausted` variant to `RetryDecision` in `retry.rs` (matching its intended semantics: budget spent, stop retrying), or change lines 289/299/308 to match only `GiveUp`. Then wire the module into the HTTP download path.

---

### C-2. `GlobalRetryBudget` is defined but NEVER wired into any request path

**File:** `crates/pcloud-resilience/src/global_budget.rs` (full module)

`GlobalRetryBudget` is not re-exported from `lib.rs` (line 58-66 exports list omits it), not used in `pcloud-proto/src/resilient_transport.rs`, and not touched by `transport_factory`. Sync-engine retry paths also do not consume it.

**Impact:** Cross-operation retry storm protection is absent. A crashing pCloud endpoint with many concurrent in-flight operations produces unbounded aggregate retries. Any documentation or code comment claiming "global retry budget" is enforced is false.

**Fix:** Export from `lib.rs`; wire into `ResilientTransport::execute` via a shared `Arc<GlobalRetryBudget>`. Or remove the module entirely and update all references.

---

## HIGH Findings

### H-1. `TransportConfig::total_request_timeout` declared but never enforced

**File:** `crates/pcloud-proto/src/transport.rs:99` (field), `:261-274` (`execute_with_body`), `:324-342` (`execute_tls`)

Every caller populates `total_request_timeout` and the doc comment promises it is "enforced as an outer deadline shared across all stages." But `execute_tls` at line 341 uses `config.read_timeout` — the per-read timeout — as the loop deadline, not the total timeout. `execute_plain` at line 321 hard-codes `Duration::from_secs(15)`, ignoring config entirely.

**Impact:** A slow-drip server can extend a request far beyond the configured wall-clock budget by sending bytes slower than `read_timeout` but never finishing. No outer deadline exists.

**Fix:** Compute `let deadline = Instant::now() + config.total_request_timeout;` before `connect_socket`, thread it through `send_and_receive`, and abort at each loop iteration when the deadline is past. Remove the hard-coded 15s in `execute_plain`.

---

### H-2. `rustls::ClientConfig` and `RootCertStore` rebuilt on every request

**File:** `crates/pcloud-proto/src/transport.rs:330-335`, `http_download.rs:208-212`, `http_download.rs:572-573`

`execute_tls` constructs `RootCertStore::empty()`, copies all webpki root CAs, and builds a fresh `ClientConfig` for every request. No `Arc<OnceLock<ClientConfig>>` caching exists.

**Impact:** Expensive heap allocation and CA-store copy on every HTTP request. No TLS session resumption across requests.

**Fix:** `static TLS_CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();` with a `fn tls_config() -> Arc<ClientConfig>` initializer that runs once per process.

---

### H-3. No per-endpoint latency/error metrics exported from transport layer

**File:** `crates/pcloud-proto/src/transport.rs`, `resilient_transport.rs`, `http_download.rs`

Grep for `pcloud_observability::|counter\.|histogram\.|record(` inside `pcloud-proto` yields no matches. `BinaryApiTransport::execute_with_body` emits no spans, counters, or timing. Only IPC-layer metrics exist in `pcloud-observability/src/metrics.rs`.

**Impact:** Operators cannot SLO-monitor network latency or error rate per pCloud API endpoint.

**Fix:** Instrument `execute_with_body` to record `pcloud_transport_latency_seconds{host, outcome}` histogram and `pcloud_transport_errors_total{host, class}` counter via `pcloud-observability`. Classes: `invalid_address|connect|tls|io|response`.

---

### H-4. Production transport uses `default_classifier` — treats EVERY error as Transient

**File:** `crates/pcloud-proto/src/resilient_transport.rs:301-310`, `crates/pcloud-daemon/src/transport_factory.rs:118`

Production `TransportFactory::wrap_binary` passes `default_classifier::<TransportError>()`, documented as "every inner error is treated as transient." No `smart_transport_classifier` exists anywhere (grep confirmed). This means `TransportError::InvalidAddress`, `InvalidServerName`, and `Tls(_)` (all permanent) are all retried.

**Impact:** A permanent TLS misconfiguration wastes the entire retry budget and produces N × backoff wait instead of failing fast. Certificate-rejection security events are masked behind retry noise.

**Fix:** Provide a `smart_transport_classifier()` that maps `InvalidAddress | InvalidServerName | Tls(_) → Permanent`; `Connect(_) | Io(_) → Transient`; `ResponseHeader(_) | ResponseBody(_) → Permanent`. Use it in `TransportFactory::wrap_binary`.

---

### H-5. Upload mutations can be auto-retried without method-class awareness

**File:** `crates/pcloud-proto/src/resilient_transport.rs:244-298`

The binary-protocol `ResilientTransport::execute` retries every `Transient` error regardless of whether the command is `upload_write` (not fully idempotent without explicit offset) or `upload_save` (commit). The `MethodRetryPolicy` exists in `pcloud-resilience/src/retry.rs:264-316` but is only consumed in the broken async transport (C-1), not in the binary-protocol wrapper.

**Impact:** Transport-level retry of `upload_write` on timeout could apply bytes twice if server committed before the client received the response. The `UploadStateMachine` partially mitigates this but cannot prevent a second concurrent transport-level retry.

**Fix:** Either disable the outer `ResilientTransport` retry for upload mutations (let the state machine own retry), or plumb `RetryClass` into `execute` so callers can mark `upload_write`/`upload_save` as non-retryable at this layer.

---

## MEDIUM Findings

### M-1. No circuit breaker in async HTTP transport

**File:** `crates/pcloud-resilience/src/transport.rs:196-198`

The async `ResilientTransport` has only retry + Retry-After + global cap. No circuit breaker or token bucket. The binary-protocol `resilient_transport.rs` does have a `CircuitBreaker`.

**Fix:** Extend the async transport with `CircuitBreaker` wiring, or merge the two implementations.

---

### M-2. `is_retryable_io` is overly narrow

**File:** `crates/pcloud-proto/src/transport.rs:439-444`

`fn is_retryable_io(err)` only matches `Interrupted | WouldBlock`. `ConnectionReset`, `BrokenPipe`, `TimedOut`, `ConnectionAborted` are not retried at this layer.

**Fix:** Add `TimedOut`, `ConnectionReset`, `BrokenPipe`, `ConnectionAborted` to the retryable set.

---

### M-3. `Retry-After` header honoring only exists in the broken async transport

**File:** `crates/pcloud-resilience/src/transport.rs:114-136`

The binary-protocol wrapper has no HTTP headers concept. `http_download.rs` retry path does not parse Retry-After.

**Fix:** Once C-1 is fixed, route HTTP download retries through the fixed async executor, or add a lightweight Retry-After parser directly in `http_download.rs`.

---

### M-4. `retry_after()` cap at 60s may mask server maintenance signals

**File:** `crates/pcloud-resilience/src/transport.rs:133-135`

`let capped = secs.min(60.0);` — server-requested backoffs longer than 60s (e.g., scheduled maintenance windows) are silently clamped.

**Fix:** Make the cap configurable; consider returning `RetryDecision::GiveUp` when server requests more than the remaining budget.

---

### M-5. No max-response-body-size enforcement on binary protocol

**File:** `crates/pcloud-proto/src/transport.rs:361-363`

`let frame_len = parse_response_frame_len(&header)? as usize; let mut body = vec![0u8; frame_len];` — server-supplied `frame_len` is trusted without a cap, allowing up to `u32::MAX` (4 GiB) allocation from a single request. HTTP download path enforces `max_body_bytes`; binary protocol does not.

**Fix:** Add `max_response_bytes: usize` to `TransportConfig`, check `frame_len` before `vec![0u8; ...]`.

---

### M-6. `is_known_safe_host` check is advisory only for persisted api-server hints

**File:** `crates/pcloud-daemon/src/bootstrap.rs:447-465`, `crates/pcloud-config/src/api.rs:200-202`

Bootstrap replays `api_server_binapi` from the preferences table and only emits `log::warn!` if the host is not known-safe. In production environment, a compromised preferences DB can redirect requests to an attacker-controlled endpoint (TLS hostname validation blocks active MitM but not DNS-matched server impersonation).

**Fix:** In production, reject unknown hosts on bootstrap (error, not warn) unless `--trust-custom-api-server` operator opt-in is set.

---

## LOW Findings

### L-1. No WebSocket / push-notification transport

Notifications are polled via `listnotifications`; diff uses cursor-based polling. No real-time push stream exists. Acceptable for the current transport design but should be documented.

### L-2. `backoff()` sleep is hard-coded at 10ms

**File:** `crates/pcloud-proto/src/transport.rs:446-448`

A high-churn `Interrupted` loop spins at 100 Hz. Make the delay configurable via `TransportConfig`.

### L-3. `hex_encode` re-implemented in `http_download.rs:240-246`

Loops with `format!("{:02x}", byte)` — performance pitfall. Replace with a workspace `hex` dependency or `write!` with `fmt::Write`.

---

## Summary by Area

| Area | Status | Top Finding |
|------|--------|-------------|
| TLS enforcement | ✓ Production enforced | — |
| Cert validation | ✓ No dangerous flags | TLS config not cached (H-2) |
| Timeouts | ✗ total_request_timeout dead | H-1 CRITICAL|
| Retry policy | ✗ async module broken | C-1 CRITICAL |
| Global budget | ✗ Not wired | C-2 CRITICAL |
| Error classification | ✗ All treated Transient | H-4 |
| Upload idempotency | Partial (state machine mitigates) | H-5 |
| API server steering | Partial | M-6 |
| Observability | ✗ No transport metrics | H-3 |
| WebSocket/diff | Not present (poll-based) | L-1 |
| Payload size caps | HTTP ✓ / Binary ✗ | M-5 |

---

## File Citations Recap

- `crates/pcloud-proto/src/transport.rs` — H-1, H-2, M-2, M-5
- `crates/pcloud-proto/src/resilient_transport.rs` — H-4, H-5
- `crates/pcloud-proto/src/http_download.rs` — H-2 (duplicate rustls build), M-3
- `crates/pcloud-resilience/src/transport.rs` — C-1, M-1, M-3, M-4
- `crates/pcloud-resilience/src/global_budget.rs` — C-2
- `crates/pcloud-resilience/src/retry.rs` — C-1 (no Exhausted variant)
- `crates/pcloud-resilience/src/lib.rs` — C-2 (missing re-export)
- `crates/pcloud-daemon/src/transport_factory.rs` — H-4
- `crates/pcloud-daemon/src/bootstrap.rs` — M-6
- `crates/pcloud-config/src/api.rs` — TLS production enforcement confirmed at line 137-141
- `crates/pcloud-backends/src/upload_state.rs` — H-5 partial mitigation

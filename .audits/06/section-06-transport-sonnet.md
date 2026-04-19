# Section 6 Transport Audit — Sonnet (Audit 06)

**Auditor:** claude-sonnet-4-6 (independent cross-validation of opus audit-05 §6)
**Date:** 2026-04-18
**Scope:** `crates/pcloud-proto/src/transport.rs`, `tls.rs`, `resilient_transport.rs`,
`crates/pcloud-resilience/src/transport.rs`, `pacing.rs`, `retry.rs`,
`crates/pcloud-config/src/api.rs`

---

## Post-Audit-05 Claims Verified

Four claims from the audit-05 remediation were independently checked:

### 1. Typed TransportError / TlsError classifier — HELD

`crates/pcloud-resilience/src/transport.rs:227–274` defines a typed
`TransportError` enum and a `TlsError` sub-enum (`InvalidCertificate`,
`AlertReceived`, `NoVersionOrCipher`, `InvalidServerName`, `Other`). The
`classify_error` path in `execute()` dispatches on typed variants, not on
`err.to_string().contains(...)`. The `pcloud-proto`-layer `TransportError`
(`transport.rs:258–312`) independently uses typed variant matching in
`transport_error_classifier()` (`resilient_transport.rs:470–494`). Both
layers hold. No string-fragility regressions found.

### 2. `is_known_safe_host` dedup — HELD

`pcloud-config/src/api.rs:208–210` is the single canonical implementation.
`pcloud-proto/src/transport.rs:438–440` delegates to it by calling
`pcloud_config::api::is_known_safe_host(host)` — no inline copy.
Tests at `transport.rs:767–780` and `api.rs:253–261` both exercise the
canonical function. Dedup is clean.

### 3. `parking_lot` BandwidthPacer — HELD

`crates/pcloud-resilience/src/pacing.rs:49` imports `parking_lot::Mutex`.
`BandwidthPacer.state` is `Mutex<PacerState>` (`pacing.rs:70`). The sleep
happens outside the lock (`pacing.rs:183–189`). Both `pace()` and `acquire()`
are implemented and tested. Claim holds.

### 4. `upload_writefromfile` idempotency guard — PARTIALLY HELD

`resilient_transport.rs:421–425` excludes `"upload_write"`,
`"upload_writefromfile"`, and `"upload_save"` from transport-layer retry, which
is correct safety behaviour. However, **`upload_writefromfile` IPC wiring
remains unimplemented** (`transfer_backend.rs:601–609`): no
`Request::UploadWriteFromFile` variant, no CLI caller. The idempotency guard
protects a code path that does not yet exist. The guard itself is not harmful,
but the claim that `upload_writefromfile` idempotency is "landed" overstates
reality; it is guarded against double-fire but the feature is still `Partial`
(row 93 in the parity matrix, consistent with existing matrix).

---

## Findings

### HIGH

**H-1 — Observability gap in primary binary-protocol path**
`crates/pcloud-proto/src/resilient_transport.rs:302–365`
The `ResilientTransport` that wraps `BinaryApiTransport` has two explicit
`TODO(bd-1du)` comments where `pcloud_transport_latency_seconds` and
`pcloud_transport_errors_total` should be emitted. Latency is captured into
`_latency` (line 358) but discarded. The HTTP-path
`pcloud-resilience/src/transport.rs` does wire metrics behind the
`transport-metrics` feature flag (line 676+), but the binary-protocol path
(the primary production path for all pCloud API calls) has zero observability
export. Per §6 of the audit spec: "per-endpoint latency/error histogram
exported via `pcloud-observability`" is a requirement.
**Remediation:** Wire `pcloud-observability` into `pcloud-proto` as a workspace
dep and replace the two `_latency` discards with `observe_latency()` calls,
mirroring the pattern already used in `pcloud-resilience/src/transport.rs`.

**H-2 — `transport-metrics` feature is opt-in, not enabled by default**
`crates/pcloud-resilience/Cargo.toml:9–18`
`default = []` — the `transport-metrics` feature is disabled in every
consumer that does not explicitly opt in. Even the HTTP-path observability
wiring is silently dead at the default feature set. Prometheus metrics are a
stated enterprise requirement; a feature flag that is off by default means
production deployments have no transport telemetry unless the integrator
knows to enable it.
**Remediation:** Add `transport-metrics` to `default` features in
`pcloud-resilience/Cargo.toml`, or make the observability calls unconditional
once `pcloud-observability` becomes a hard dependency (already the case for
the daemon crate).

### MEDIUM

**M-1 — `write_timeout` uses `read_timeout` value (copy-paste bug)**
`crates/pcloud-proto/src/transport.rs:468`
`crates/pcloud-proto/src/http_download.rs:408`

```rust
stream.set_write_timeout(Some(config.read_timeout))
```

Both sites pass `config.read_timeout` to `set_write_timeout`. There is no
separate `write_timeout` field in `TransportConfig`. For large upload bodies
this means a 30 s per-syscall deadline governs writes, which is correct in
practice, but it also means write-budget configuration cannot be tuned
independently. If a future operator needs asymmetric read/write timeouts
(e.g., slow-upload environments) there is no knob. Low severity for correctness
now, but a latent API gap.
**Remediation:** Add `write_timeout: Duration` field to `TransportConfig`
defaulting to `DEFAULT_READ_TIMEOUT`, and use it in both `set_write_timeout`
calls.

**M-2 — `observe_latency` drops the `_host` label**
`crates/pcloud-resilience/src/transport.rs:122–123`
The `_host` parameter is accepted but unused; the histogram emits a single
global bucket with no per-host dimension. Per §6 the audit requires
"per-endpoint latency/error histogram". The metric is present but loses the
endpoint dimension needed for per-host SLO alerting.
**Remediation:** Once the observability crate supports label dimensions,
route the `host` label through; document the limitation in the meantime.

**M-3 — `apply_api_server_hint` in `pcloud-config` returns `Result` but in `pcloud-proto/transport.rs` it silently drops errors**
`crates/pcloud-proto/src/transport.rs:403–427`
`ApiServerHintConsumer::apply_api_server_hint` in the proto transport silently
returns `()` on unknown hosts (the guard `if !is_known_safe_host` just returns).
The config-layer `ApiEndpoint::apply_api_server_hint` returns a `Result` which
the call site at `api.rs:189` actually uses. The proto-layer wrapper has no
error surface — a rejected hint disappears silently. This is safe but
unobservable; a misconfigured hint that should fail is invisible in logs.
**Remediation:** Log a `warn!` on rejected hints in `apply_api_server_hint`
so operators can detect server-side misconfiguration.

### LOW

**L-1 — TLS 1.2 exclusion not verified at runtime on the HTTP download path**
`crates/pcloud-proto/src/tls.rs:60`
The shared `OnceLock` config pins TLS 1.3 for the binary protocol. The HTTP
download path (`http_download.rs`) uses the same `shared_config()` call, so
this is consistent. However, there is no integration test that asserts a TLS
1.2 handshake is rejected. If `webpki-roots` or `rustls` introduces a
compatibility layer that accepts 1.2 in a future release, there is no
regression guard.
**Remediation:** Add a `#[test]` that constructs a TLS 1.2-only mock server
and asserts `execute()` returns `TransportError::Tls`.

**L-2 — `BackoffSchedule::ExponentialJittered` uses a fixed seed**
`crates/pcloud-proto/src/resilient_transport.rs:252`
`retry_jitter_seed: 7` (from test-policy) feeds into the policy. In production
all daemon instances with the same config file will generate identical jitter
sequences — this defeats jitter's purpose of spreading retry thundering-herds
across multiple clients. The seed should default to a per-process entropy
source (e.g., `rand::random::<u64>()` at config parse time).
**Remediation:** In `ResiliencePolicy`, derive `retry_jitter_seed` from
`rand::random()` at struct construction rather than zero-defaulting it.

---

## Summary Table

| ID  | Severity | Finding |
|-----|----------|---------|
| H-1 | HIGH | Binary-protocol `ResilientTransport` has no observability export (TODOs, discarded latency) |
| H-2 | HIGH | `transport-metrics` feature off by default; no production transport telemetry without opt-in |
| M-1 | MEDIUM | `write_timeout` uses `read_timeout` value in both TCP transports |
| M-2 | MEDIUM | `observe_latency` drops `host` label; no per-endpoint histogram dimension |
| M-3 | MEDIUM | Rejected `apply_api_server_hint` silently dropped, no log warning |
| L-1 | LOW | No regression test asserting TLS 1.2 handshakes are refused |
| L-2 | LOW | Fixed `retry_jitter_seed` defeats thundering-herd protection in multi-instance deployments |

---

## Audit-05 Claims Status

| Claim | Status |
|-------|--------|
| Typed TransportError/TlsError classifier | **HELD** |
| `is_known_safe_host` dedup | **HELD** |
| `parking_lot` BandwidthPacer | **HELD** |
| `upload_writefromfile` idempotency | **PARTIALLY HELD** — guard exists, feature still Partial (row 93) |

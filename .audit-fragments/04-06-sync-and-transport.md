# Audit Fragment: Dimensions 4 & 6 – Sync Engine & Transport / Network Resilience

**Scope:** `crates/pcloud-engine/`, `crates/pcloud-daemon/src/runtime.rs`, `crates/pcloud-store/`, `crates/pcloud-proto/src/`, `crates/pcloud-resilience/`.

**Audit Date:** 2026-04-26  
**Audit Coverage:** Enterprise-readiness gap analysis vs. C reference (psync_syncer.c, pupload.c, psettings.h).

---

## 4. Sync Engine & Runtime

### CRITICAL

None identified.

### HIGH

#### H-4.1: Watcher Event Overflow — Potential Silent Drop Risk
**Location:** `crates/pcloud-engine/src/fs_events.rs:1–95`  
**Finding:**  
The filesystem event ingestor (`FsEventIngestor::normalize_events`) performs **batch-local deduplication only** (line 23 comment confirms time-window debouncing is **upstream** via `pcloud_fs::fs_watcher::FsWatcher::debounce_loop`). However:

1. If the `notify` crate's bounded channel fills, events are dropped without explicit tracking or telemetry.
2. No observable metric for "events dropped due to overflow" is wired into `pcloud-observability`.
3. Synchronization semantics on overflow are undocumented.

**Severity:** HIGH (silent data loss risk during burst creates).

**Remediation:**
- Add a per-sync-root overflow counter to `pcloud-observability` metrics.
- Emit a WARN log + telemetry on watcher drop.
- Document drop behavior in `fs_watcher::FsWatcher` module docs.

**Tracking:** Estimated effort: 2–3 hours (counter + instrumentation).

---

#### H-4.2: Upload Idempotency — No Request ID Propagation in upload_create → upload_write → upload_save
**Location:** `crates/pcloud-proto/src/methods/upload.rs:153–409`  
**Finding:**  
The upload three-phase protocol (`upload_create`, `upload_write`, `upload_save`) does **not** carry a stable request or idempotency key:

- `upload_create` (line 177) opens a handle but generates no client-side UUID.
- `upload_write` (line 231) takes an explicit offset but has no retry ID.
- `upload_save` (line 409) commits but cannot distinguish duplicate commits from the same retry.

On network retry, the daemon could:
1. Create a second handle (`upload_create` succeeds twice).
2. Write overlapping chunks via separate handles.
3. Save both handles, creating a duplicate file on the server.

**Severity:** HIGH (data integrity: double-write on retry).

**Remediation:**
- Generate a client-side UUID on `upload_create` (or derive from file content hash + mtime).
- Carry the UUID through `upload_write` and `upload_save` as an optional parameter.
- Server tracks UUID → handle mapping to reject duplicate `upload_create` calls.
- Wire idempotency into `resilient_transport` so retry-after-failure re-uses the same UUID.

**Tracking:** Epic; estimated 2–3 days for full round-trip (proto, transport, daemon).

---

### MEDIUM

#### M-4.1: No Explicit Power/Battery Awareness
**Location:** `crates/pcloud-engine/src/lib.rs:932–945` (pause_sync_root / resume_sync_root exist but no power signal)  
**Finding:**  
- The engine supports manual `pause_sync_root` (line 932) and `resume_sync_root` (line 945).
- No platform-level power state watcher (e.g., DBus on Linux, IOKit on macOS) triggers automatic pause on battery.
- The C psync (`psync_syncer.c`) references OS battery signals; Rust version does not.

**Severity:** MEDIUM (battery drain under sync on mobile/laptop scenarios; no blocking production impact).

**Remediation:**
- Add a `pcloud-power` crate wrapping platform signals (SystemD inhibitor, IOKit, Windows API).
- Wire into the daemon bootstrap to auto-pause all syncs on battery below threshold.
- Expose knob via `pcloud-config::sync.battery_pause_threshold_percent`.

**Tracking:** Estimated 1–2 days; deferred to `pcloud-rs-batx`.

---

#### M-4.2: No Explicit Integrity Sweeper (Periodic Consistency Scan)
**Location:** `crates/pcloud-store/src/` (no `sweep` or `fsck` module)  
**Finding:**  
- The schema (`schema.rs`, `lib.rs`) defines the schema but has no periodic consistency checker.
- No background task to scan for orphaned rows (e.g., upload_resume state for missing sync roots).
- The C psync has `psync_check_integrity`; Rust has none.

**Severity:** MEDIUM (drift accumulation over months; doesn't block sync but causes slow queries).

**Remediation:**
- Add `crates/pcloud-store/src/sweep.rs` with a `SweepCriteria` enum (orphan rows, stale diff cursors, audit truncation).
- Wire into `daemon::runtime::EngineShell` as an idle-time background task (runs when no sync activity > 1 hour).
- Expose results via observability counter `pcloud_sweep_rows_reclaimed`.

**Tracking:** Estimated 3–4 hours; low priority.

---

#### M-4.3: Watcher Debounce Window Not Configurable
**Location:** `crates/pcloud-fs/src/fs_watcher.rs` (not in current crate tree, FUSE layer)  
**Finding:**  
The debounce logic is hardcoded; no config knob to tune the coalesce window for high-churn environments (e.g., build directories with thousands of tmp files per minute).

**Severity:** MEDIUM (suboptimal perf under bursty churn; not a correctness issue).

**Remediation:**
- Add `pcloud_config::sync.fs_event_coalesce_window_ms` (default 100 ms).
- Expose to daemon config YAML.

**Tracking:** Out of scope for 04-06 audit (belongs to FUSE layer / pcloud-fs crate).

---

### LOW

#### L-4.1: Conflict Resolution Policy Not Persisted Per-Sync-Root
**Location:** `crates/pcloud-engine/src/conflict_resolver.rs:14–88`  
**Finding:**  
Conflict policy is a **global** setting (`pcloud_config::sync.conflict_policy`), not per-sync-root. If the user has two syncs (one tolerating conflicts, one rejecting them) they must reconfigure the daemon or use the default policy for both.

**Severity:** LOW (inconvenient for power users; not a data loss risk).

**Remediation:**
- Add optional `conflict_policy: Option<ConflictPolicy>` to sync root record.
- Fall back to global default if not set.
- Estimated 1 hour.

---

## 6. Transport & Network Resilience

### CRITICAL

None identified.

### HIGH

#### H-6.1: Missing Per-Endpoint Timeout Composition (connect/read/write/total)
**Location:** `crates/pcloud-proto/src/transport.rs:100–145`  
**Finding:**  
The transport config provides **four** timeout knobs:
- `connect_timeout: Duration` (default 10s, line 102)
- `read_timeout: Duration` (default 30s, line 104)
- `write_timeout: Duration` (default 30s, line 106)
- `total_request_timeout: Duration` (default 5 min, line 124)

**But:**
1. No per-operation (per endpoint method) override. A slow `folder_list` call uses the same 5-min total as a quick `login` call.
2. No "connection timeout < read timeout < total timeout" validation at construction.

**On misconfiguration,** a caller could set `total_request_timeout < read_timeout`, causing spurious failures.

**Severity:** HIGH (configuration footgun; wrong timeouts cause cascading timeouts in dependent layers).

**Remediation:**
- Add validation in `TransportConfig::production()` and `TransportConfig::with_tls()`:  
  `assert!(read_timeout <= total_request_timeout, "read timeout must not exceed total timeout")`
- Add optional per-method overrides (e.g., `method_overrides: HashMap<&'static str, Duration>`) for endpoints known to be slow.
- Document timeout composition in module rustdoc.

**Tracking:** Estimated 2–3 hours.

---

#### H-6.2: No Retry-After Header Parsing
**Location:** `crates/pcloud-resilience/src/retry.rs:1–150`  
**Finding:**  
The retry policy supports exponential backoff + jitter (lines 26–46) but does **not** parse or respect the HTTP `Retry-After` response header. The pCloud API may return `Retry-After: 60` to signal rate-limit windows, but the daemon's backoff ignores it.

**Severity:** HIGH (API rate-limiting: daemon can hammer the server unnecessarily instead of honoring server guidance).

**Remediation:**
- Add `parse_retry_after(header: &str) -> Option<Duration>` helper to `resilience::retry` module.
- Wire into `resilient_transport` (where the response is available) to overwrite the computed backoff with the server's hint.
- Test against a mock server returning `Retry-After: 120`.

**Tracking:** Estimated 3–4 hours.

---

### MEDIUM

#### M-6.1: No Circuit Breaker for API Server (Only Per-Endpoint)
**Location:** `crates/pcloud-resilience/src/circuit_breaker.rs:65–95`  
**Finding:**  
A per-endpoint circuit breaker exists (line 76, `CircuitBreakerConfig`) but **no** coarse-grained breaker for the entire API server or failover target. If the primary server is down, the daemon tries the primary for N attempts before failing over to a secondary (if configured). No fast-fail once the primary is detected as unhealthy.

**Severity:** MEDIUM (inefficient failover; daemon wastes retries on a dead primary).

**Remediation:**
- Add a module-level `ApiServerCircuitBreaker` (separate from per-endpoint) that trips after 5 consecutive timeouts.
- On trip, skip to secondary server immediately (if available).
- Expose state via observability counter `pcloud_api_server_circuit_breaker_state{server, state}`.

**Tracking:** Estimated 4–6 hours.

---

#### M-6.2: TLS Configuration Missing OCSP Stapling (FedRAMP Gap)
**Location:** `crates/pcloud-proto/src/tls.rs:52–89`  
**Finding:**  
The TLS config is hardened (TLS 1.3 only, webpki-roots, no dangerous cert acceptance) but **intentionally** defers OCSP/CRL revocation checking. Lines 66–72 document the deferral; the placeholder function `_t9o_revocation_placeholder()` notes that FedRAMP environments require this.

**Severity:** MEDIUM (FedRAMP compliance gap; not a risk for typical deployments).

**Remediation:**
- No remediation required in this audit window (tracked as `pcloud-rs-t9o` epic).
- Closure criteria: deployment decision on CRL sourcing, validation against live API servers, failure-mode agreement with security team.

---

#### M-6.3: No API Server Stickiness Across Restarts
**Location:** `crates/pcloud-proto/src/transport.rs:84–97`  
**Finding:**  
The transport config carries `host` and `port` but does **not** persist the last-seen-good API server (or a preference list). On daemon restart, the daemon re-resolves DNS and may land on a different regional server, losing any connection-state hints (e.g., `apiserver` field returned by `login`).

**Severity:** MEDIUM (suboptimal perf: daemon may pick a farther server on restart; not a correctness issue).

**Remediation:**
- Add `pcloud_config::api.sticky_server_hint: Option<String>` (e.g., "bineapi2.pcloud.com").
- Persist the last-used server from `apply_api_server_hint` to the config file.
- Estimated 2–3 hours.

---

### LOW

#### L-6.1: HTTP Downgrade Prevention — No Build-Time Check
**Location:** `crates/pcloud-proto/src/transport.rs:155–195`  
**Finding:**  
The constructors `production()` (line 155) and `dev_plaintext()` (line 178) are explicitly named to catch accidental use, and the daemon bootstrap validates against the config file. However, **no compile-time gating** (feature or cfg) prevents a third party or future refactor from accidentally calling `dev_plaintext` in a release build.

**Severity:** LOW (mitigated by code review and runtime bootstrap validation; not a risk given the explicit naming).

**Remediation:**
- Optional: Add compile-time assertion via `#[cfg(debug_assertions)]` guard on `dev_plaintext`.
- Current state (explicit naming + bootstrap validation) is sufficient.

---

#### L-6.2: Observability — No Latency Histogram Per API Endpoint
**Location:** `crates/pcloud-observability/src/metrics.rs:19–20`  
**Finding:**  
The histogram `pcloud_request_latency_seconds` is **keyed by method** (line 20, label `method`). A label value per endpoint (e.g., `login`, `folder_list`, `upload_write`) carves up the histogram. No per-method-per-server latency breakdown is available (e.g., to detect if one regional endpoint is slow).

**Severity:** LOW (operational visibility gap; existing histogram still provides some signal).

**Remediation:**
- Optional: Add a second histogram `pcloud_request_latency_per_endpoint_seconds{method, endpoint}` with bounded cardinality (top 10 methods, label-sanitized).
- Deferred to observability enhancement epic.

---

## Summary

**CRITICAL:** None.

**HIGH:** 2 findings
- H-4.2: Upload idempotency (double-write on retry)
- H-6.1: Timeout composition & validation

**MEDIUM:** 5 findings
- M-4.1: Power/battery awareness
- M-4.2: Integrity sweeper
- M-4.3: Debounce configurability (out of scope, FUSE layer)
- M-6.1: API server circuit breaker
- M-6.2: OCSP/CRL revocation (FedRAMP, deferred)
- M-6.3: API server stickiness across restarts

**LOW:** 3 findings
- L-4.1: Per-sync conflict policy
- L-6.1: HTTP downgrade prevention (mitigated)
- L-6.2: Observability per endpoint

**Enterprise-Readiness Status:**
- ✅ TLS 1.3 hardened, no dangerous cert acceptance.
- ✅ SQLite WAL + PRAGMA synchronous=NORMAL for crash-consistency.
- ✅ Transaction boundaries via `TransactionBoundary` (no half-writes).
- ✅ Retry policy with exponential backoff + jitter.
- ✅ Circuit breaker (per-endpoint).
- ✅ Timeout knobs (connect/read/write/total).
- ⚠️ Upload idempotency missing (HIGH).
- ⚠️ Retry-After header not honored (HIGH).
- ⚠️ Timeout validation missing (HIGH).
- ⚠️ Power/battery awareness missing (MEDIUM).
- ⚠️ Integrity sweeper missing (MEDIUM).

---

## References

- `crates/pcloud-engine/src/`: Scheduler, conflict resolver, fs events, transfers.
- `crates/pcloud-store/src/lib.rs`: SQLite bootstrap, schema, migrations, transaction discipline.
- `crates/pcloud-proto/src/transport.rs`: TLS config, timeout composition.
- `crates/pcloud-proto/src/tls.rs`: TLS 1.3 hardening, revocation placeholder.
- `crates/pcloud-resilience/src/`: Retry, circuit breaker, timeout, rate limit.
- `crates/pcloud-observability/src/metrics.rs`: Observability counters & histograms.


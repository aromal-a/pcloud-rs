> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# OpenTelemetry Distributed Tracing

> **Status:** **LANDED** (H13a–H13d), **live in-process collector
> test**. Enabled by opting into the `tracing-otlp` Cargo feature at
> build time and setting `[observability.tracing].enabled = true` at
> runtime. End-to-end OTLP delivery is now exercised by an in-process
> OTLP/HTTP collector integration test
> (`crates/pcloud-observability/tests/otlp_live_interop.rs`) that
> spins up `axum`, decodes the exported protobuf payloads via
> `opentelemetry-proto`, and asserts the span hierarchy, the
> [`ALLOWED_ATTRS`] allow-list contract, and W3C `traceparent`
> propagation. Live interop against a managed third-party OTLP
> backend (Datadog, Honeycomb, New Relic, Tempo UI) is still
> unverified in CI; `Pkcs11Hsm` KMS paths and FUSE callbacks are
> **not** instrumented yet.

## 1. Purpose

Distributed tracing turns a CLI-to-API call chain into a single,
queryable object in an OTLP backend (Jaeger, Tempo, Datadog,
Honeycomb, New Relic). For a user hitting a failure in
`pcloudc sync add /some/path`, the operator wants one trace id that
covers:

- CLI parsing and the IPC round-trip,
- daemon dispatch routing,
- the responsible backend (`transfer`, `sync`, `crypto`, `shares`,
  `public_link`, `backup`, `account`),
- the outgoing HTTPS call to the pCloud API.

Without tracing, the operator is stuck correlating three separate
log streams by wall-clock and hoping the clocks are synced. With
tracing, the user pastes one trace id into a ticket and the
operator sees the whole chain end-to-end in the collector UI.

This is the beginner-friendly, non-negotiable story: **one id,
whole trace, no PII.** Everything below is the enterprise-grade
shape of that story.

## 2. Threat model

Tracing adds an outbound TLS channel to a collector and attaches a
header (`traceparent`) to local IPC requests. The relevant threats
and their mitigations:

| Threat | Mitigation |
| --- | --- |
| **PII leakage** via span attributes (paths, emails, tokens) | `attr_redact` five-key allow-list; debug builds panic on unknown keys (§6) |
| **PII leakage** via span *names* | Names are fixed string literals drawn from §5; callers cannot inject user data |
| **Malicious `traceparent` injection** on the wire | Invalid traceparent values are dropped; daemon synthesises a fresh root (§4) |
| **Collector endpoint downgrade to plaintext** | Non-loopback plaintext endpoints refused at startup (§8) |
| **Secret leakage via collector auth headers in config file** | Headers MUST use `${env:VAR}` form; literal values refused at startup (§8) |
| **Always-on performance cost in disabled builds** | Feature-gated at compile time (`tracing-otlp`) plus opt-in at runtime |

Tracing is explicitly not an auth channel, not a DLP channel, and
not a security boundary. It is an observability channel only.

## 3. Scope

In scope, landed:

- W3C Trace Context `traceparent` propagation between CLI and
  daemon,
- server span on the dispatch boundary,
- internal spans for each backend (`transfer`, `sync`, `crypto`,
  `shares`, `public_link`, `backup`, `account`),
- OTLP/HTTP protobuf export,
- strict attribute allow-list,
- head sampling with error bias.

Out of scope for H13:

- FUSE/mount callbacks instrumentation (uninstrumented by design
  in this wave; follow-up),
- `Pkcs11Hsm` KMS provider call paths (stub surface, no spans),
- TraceState (W3C multi-vendor extension) propagation,
- metrics and logs pipelines — only **tracing** landed here;
  OpenTelemetry metrics and logs are separate roadmap items.

## 4. Design

### 4.1 Wire shape

The CLI wraps each IPC `Request` in `RequestEnvelope`:

```rust
// crates/pcloud-ipc/src/methods.rs
#[derive(Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub request: Request,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}

impl RequestEnvelope {
    /// Accept either the new envelope shape or a legacy bare `Request`.
    pub fn try_from_wire(bytes: &[u8]) -> Result<Self, ProtocolError> { /* ... */ }
}
```

Design choice: **single envelope wrapper, not per-variant
ripple.** Threading a `traceparent` field into every
`Request::*` variant would have touched ~485 call sites and
required breaking `#[non_exhaustive]`. The envelope localises the
change, preserves byte compatibility with untraced peers
(Serde omits `traceparent` when `None`), and keeps legacy bare-
`Request` peers working via `try_from_wire`.

### 4.2 Propagation

The wire carrier is the W3C Trace Context `traceparent`:

```
traceparent = "00-" <trace_id:32-hex> "-" <parent_id:16-hex> "-" <flags:2-hex>
```

- `00` is the version byte,
- `flags = 01` means sampled; `00` means recorded-but-not-sampled,
- malformed traceparents are dropped at the boundary and the
  daemon synthesises a fresh root — never trust a broken id.

### 4.3 Span hierarchy

```
pcloudc.command                kind=client   (CLI root; --trace-id or fresh)
└── pcloudd.dispatch           kind=server   (IPC receive; extracts traceparent)
    └── pcloudd.backend.<name> kind=internal (transfer | sync | crypto |
                                              public_link | shares | backup |
                                              account)
        └── pcloud.proto.<method> kind=client (HTTPS call to pCloud API)
```

Background work (sync engine ticks, writeback flushes) is emitted
under `pcloudd.background.<job>` with a stable `job` attribute.
These spans are **not** parented to any CLI invocation — they are
trace roots in their own right.

### 4.4 Sampling

Head-based **1% default**, error-biased to **100% on error**:

- The CLI rolls a random sample at `sample_rate`
  (default `0.01`). The decision is encoded in the `flags` byte of
  the outgoing `traceparent`, so the daemon honours it without
  re-rolling.
- Any span ending with a non-`Ok` status is force-exported
  together with its full ancestor chain. Success is cheap;
  failure is fully traced.
- Passing `--trace-id` implicitly force-samples that invocation
  regardless of `sample_rate`.

## 5. Interfaces

### 5.1 Library surface (`pcloud-observability`)

- `TracingHandle::init(cfg) -> Result<TracingHandle>` — installs
  the global subscriber, starts the OTLP exporter.
- `attr_redact(key, value) -> Option<Value>` — the allow-list
  filter.
- `parse_traceparent(&str) -> Option<W3cTraceparent>` — validator.
- `set_thread_traceparent(Option<&str>)` — sets the current
  dispatching thread's ambient parent context.
- `note_dispatch_panic(&Err)` — closes the current span with a
  non-Ok status and force-exports with the full ancestor chain.

All gated on the `tracing-otlp` Cargo feature; without the
feature these functions exist as no-op stubs that return `Ok`.

### 5.2 Attribute allow-list

`attr_redact` enforces a **five-key allow-list**, no exceptions:

- `command`
- `duration_ms`
- `error_category`
- `status_code`
- `trace_kind`

Any other key is **dropped** in release builds and **panics in
debug builds**, so accidental additions are caught in CI and
local dev. There is no path through the redaction layer that can
emit a filename, folder name, path, email, username, auth token,
or crypto material. Span *names* are fixed literals from §4.3.

### 5.3 CLI surface

```
pcloudc --trace-id=<32-HEX> <subcommand> ...
pcloudc --trace-id <32-HEX>  <subcommand> ...
```

- With `--trace-id`: the provided 32-hex id is validated and used
  as the root `trace_id`; the invocation is implicitly sampled.
- Without `--trace-id`: a fresh random 16-byte id is generated.
- Both paths emit a single stderr line before the command result:

  ```
  [trace: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01]
  ```

Operators copy this line into tickets; support pastes the
`trace_id` into the OTLP backend.

## 6. Configuration

```toml
[observability.tracing]
enabled     = false
endpoint    = "https://otlp.example.com:4318/v1/traces"
sample_rate = 0.01
headers     = { "x-honeycomb-team" = "${env:HONEYCOMB_API_KEY}" }
```

Validation rules (fail-closed at daemon start):

- `enabled = false` (default) — tracing is off; all library calls
  become no-ops.
- `endpoint` must be `https://` unless it resolves to loopback.
  Plaintext non-loopback endpoints are **refused** at startup.
- `sample_rate` is clamped to `[0.0, 1.0]`; out-of-range values
  cause refuse-to-start.
- `headers` values must use `${env:VAR}` form. Literal secrets
  in the config file are **refused** at startup, matching
  `CLAUDE.md` §Secrets: "do not persist auth tokens in clear".
- A daemon built **without** the `tracing-otlp` feature accepts
  the section but logs
  `observability.tracing.feature_disabled` once at startup and
  silently ignores the rest.

## 7. Onboarding

**Small-deployment happy path:**

1. Rebuild the daemon with the `tracing-otlp` feature:
   ```
   cargo build --release -p pcloud-daemon --features tracing-otlp
   ```
2. Run an OTLP collector somewhere reachable on HTTPS (Jaeger,
   Tempo, otel-collector-contrib in HTTP/protobuf mode).
3. Set `[observability.tracing]` in the daemon config per §6.
4. Restart the daemon. Confirm the absence of
   `observability.tracing.feature_disabled` in the daemon log.
5. Run `pcloudc --trace-id=0123...`, copy the stderr
   `[trace: ...]` line, paste the id into the collector UI.

**Incident triage:** the user runs `pcloudc <failing-command>`,
copies the stderr trace line into the ticket, and the operator
searches the `trace_id` in the backend. Every hop from CLI to the
outgoing HTTPS call appears in the same trace.

## 8. Verification

What has been verified in this release:

- **Live OTLP/HTTP end-to-end delivery.** The integration test
  `crates/pcloud-observability/tests/otlp_live_interop.rs`
  (feature-gated on `tracing-otlp`) spins up an in-process
  OTLP/HTTP collector with `axum` + `opentelemetry-proto`,
  initializes the daemon tracer against it, emits the
  `pcloudd.dispatch` + `pcloudd.backend.<name>` span pair, and
  asserts:
  - exactly one `pcloudd.dispatch` parent span arrives,
  - exactly one `pcloudd.backend.*` child span arrives, parented
    to the dispatch span and sharing the same `trace_id`,
  - every exported attribute key is drawn from the five-key
    [`ALLOWED_ATTRS`] allow-list (`command`, `duration_ms`,
    `error_category`, `status_code`, `trace_kind`) — no
    `code.filepath`, `thread.id`, `busy_ns`, or other
    auto-injected keys leak past the exporter,
  - inbound W3C `traceparent` round-trips into the exported
    `trace_id` bytes verbatim.
- Span hierarchy shape (server → internal → client) produced
  against a local OTLP sink.
- `attr_redact` panics in debug builds on any non-allow-listed
  key; release builds drop silently.
- Malformed `traceparent` values are dropped on receive.
- Legacy bare-`Request` peers still interoperate through
  `RequestEnvelope::try_from_wire`.
- `sample_rate` clamp and `https`-only endpoint rejection land at
  startup, not at first span.
- `feature_disabled` log line is emitted exactly once when the
  section is configured but the feature is off.

What has **not** been verified:

- Live interop against a managed third-party OTLP backend
  (Jaeger UI, Tempo, Datadog APM, Honeycomb, New Relic) —
  collector-side parser quirks are possible. The in-process
  test proves wire-format correctness against a reference
  decoder (`opentelemetry-proto`), not vendor-specific UI
  ingest.
- Performance overhead under sustained load above 1% sample
  rate — only smoke-tested.

## 9. Failure modes

| Failure | Behaviour |
| --- | --- |
| Collector unreachable | Exporter batches time out; spans are dropped; daemon logs a warning at most once per minute; no CLI-visible effect |
| Collector returns 5xx | Exporter retries with backoff; after retry budget, spans are dropped |
| Malformed `traceparent` on IPC | Dropped; daemon acts as trace root |
| Handler panic inside a dispatched request | `note_dispatch_panic` closes the span with non-Ok status and force-exports; the outer dispatch span carries the panic message in the `error_category` attribute (never the panic payload) |
| Debug build, disallowed span attribute | **Panic.** Intentional; caught in CI |
| Release build, disallowed span attribute | Attribute dropped silently, no other effect |
| Feature disabled at runtime | `enabled = false` → no OTLP exporter thread, no IPC envelope overhead beyond an `Option::None` |
| Feature not compiled in | Config parsed, `feature_disabled` log line emitted once, everything else no-op |

## 10. Honest limitations

pre-alpha reality check:

- **In-process collector test, no managed-backend run.** End-to-end
  OTLP delivery is now exercised against a live in-process
  `axum`-hosted collector (see §8), but no certified live run
  against a managed vendor backend (Datadog, Honeycomb, Tempo UI,
  New Relic) has landed.
- **FUSE uninstrumented.** Mount callbacks in `crates/pcloud-fs/`
  do not participate in `pcloudd.dispatch` traces. Closing this
  gap is blocked on `bd-1du.4` landing the mounted-drive runtime
  in the first place.
- **`Pkcs11Hsm` uninstrumented.** The KMS provider stub does not
  emit spans. Documented follow-up; low priority because the
  provider itself returns `KmsError::NotImplemented` today (see
  `kms.md`).
- **No metrics, no logs pipeline.** Only OpenTelemetry **tracing**
  landed here. Metrics and logs over OTLP are separate, not-yet-
  scoped roadmap items.
- **No TraceState propagation.** W3C `tracestate` multi-vendor
  chaining is not parsed, not propagated, not emitted.
- **Background traces are rootless.** Sync/writeback jobs emit
  their own trace roots; they cannot be correlated to a triggering
  CLI invocation via the wire format.

## 11. Extension points

- **Add a new backend** → register the handler in
  `dispatch.rs`, emit `pcloudd.backend.<name>` with `<name>` added
  to the canonical list in §4.3. No schema change.
- **Add a new attribute** → add the key to `attr_redact`'s
  allow-list **and** document it in §5.2. Without the allow-list
  addition, debug builds will panic.
- **Swap exporter protocol** → `opentelemetry-otlp` supports
  gRPC; the daemon currently pins HTTP/protobuf for firewall
  friendliness. Flipping to gRPC is a config-schema change, not a
  code change in the instrumentation.
- **Instrument FUSE** (follow-up) → needs `bd-1du.4` to land the
  mounted-drive runtime first; then wrap each FUSE callback in
  an `instrument` span under `pcloudd.fuse.<op>` and attach it to
  the dispatch parent via `set_thread_traceparent`.

## 12. Cross-refs

Code:

- `crates/pcloud-observability/src/tracing/` — library, exporter,
  `attr_redact`.
- `crates/pcloud-ipc/src/methods.rs` — `RequestEnvelope`,
  `try_from_wire`.
- `crates/pcloud-cli/src/globals.rs` — `--trace-id` parsing,
  generation, stderr emission.
- `crates/pcloud-daemon/src/dispatch.rs` — dispatch span,
  per-backend span opening, `set_thread_traceparent`,
  `note_dispatch_panic`.
- `packaging/man/pcloudc.1`, `packaging/man/pcloudd.1` — operator
  surface.

Related docs:

- `docs/book/src/reference/cli.md#global-flags` — CLI reference
  for `--trace-id`.
- `docs/book/src/reference/ipc-protocol.md#requestenvelope` — wire
  shape.
- `docs/book/src/operations/runbook.md` — triage workflow with
  traceparent.
- `docs/enterprise/disaster-recovery.md` — snapshot audit events
  carry trace ids when tracing is enabled.
- `docs/enterprise/ha.md` — restart paths preserve the ambient
  thread traceparent so post-crash replay traces are rooted at
  the right parent.

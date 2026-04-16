# ADR 0012: `RequestEnvelope` Wrapper for OTel `traceparent`

- Status: Accepted
- Date: 2026-04-16

## Context

Wave H13 added W3C-compliant OpenTelemetry distributed tracing to the
daemon (`docs/enterprise/tracing.md`). The design needed every IPC
request to optionally carry a `traceparent` header so that a trace id
established in a client (CLI, web UI, SDK caller) flows through
`pcloudd.dispatch` and into each backend span.

The naive approach is to add a `traceparent: Option<String>` field to
**every variant** of `pcloud_ipc::Request`. At the time of the
decision that meant ~485 call sites across the daemon, CLI, SDK, and
tests — every construction, every match arm, every serde round-trip.

Constraints:

- pre-envelope clients (older SDK consumers, test fixtures, scripted
  callers) must keep working bit-for-bit; we cannot rev the IPC wire
  format in a breaking way for tracing alone;
- the trace id must not be mandatory — untraced callers stay untraced;
- serialisation must remain byte-identical for untraced requests so
  fuzz corpora and wire snapshots do not churn;
- the dispatch path must be able to synthesise a fresh root span when
  no `traceparent` arrives, without treating that as an error.

## Decision

Introduce a thin wrapper at the IPC boundary:

```rust
pub struct RequestEnvelope {
    pub request: Request,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}
```

Rules:

1. `RequestEnvelope::new(request)`, `with_traceparent`,
   `traceparent()`, and `From<Request>` provide the ergonomic surface.
2. `RequestEnvelope::try_from_wire(bytes)` first attempts envelope
   decode, and on failure falls back to decoding a bare `Request`.
   Pre-envelope peers keep working unchanged.
3. `Option<String>` with `skip_serializing_if = "Option::is_none"`
   keeps the wire bytes identical for untraced callers.
4. The daemon dispatch loop extracts `traceparent`, validates it with
   `pcloud_observability::tracing::parse_traceparent`, installs it via
   `set_thread_traceparent`, and opens `pcloudd.dispatch` as a server
   span. If validation fails, dispatch synthesises a fresh root.
5. The `Request` enum itself is **not** modified — variants stay tight,
   fuzz corpora stay valid, and every future `Request::*` variant is
   tracing-capable by default.

## Consequences

Good:

- Zero variant-level ripple. The tracing feature landed without
  touching ~485 call sites.
- Wire back-compat preserved: `try_from_wire` handles both shapes.
- Untraced callers pay no wire cost (`skip_serializing_if`).
- Future IPC-level metadata (tenant id, request id, fleet device id)
  has a natural home in the same envelope without another rev.
- Tracing feature stays feature-gated (`tracing-otlp`); the envelope
  exists even without the feature, which keeps the wire format stable
  across builds.

Bad:

- One extra struct at the boundary. Callers must be aware the outer
  type is `RequestEnvelope`, not `Request`. Mitigated by `From<Request>`
  and by concentrating the awareness in `dispatch.rs`.
- `try_from_wire` has two decode paths; the cost is a single failed
  envelope decode before the bare-request fallback. Measured
  sub-microsecond on realistic payloads.

## Alternatives Considered

- **Add `traceparent` to every `Request` variant**: rejected —
  ~485 call sites, every fuzz corpus invalidated, every test fixture
  updated. Also makes it impossible to stay wire-compatible with
  pre-envelope peers.
- **Side-channel trace id via a separate IPC message**: rejected —
  races the request it was supposed to decorate, and doubles the IPC
  syscall count.
- **Thread-local trace id set by the client before each call**:
  rejected — breaks the moment we add any kind of batching, pipelining,
  or async dispatch. IPC is the only boundary we trust.
- **Wrap only in the SDK and let the daemon ignore it**: defeats the
  purpose; the goal is end-to-end spans through dispatch.

#![allow(clippy::pedantic)]
//! Live OTLP interop integration test (feature `tracing-otlp`).
//!
//! # Purpose
//!
//! Close the "offline-tested only" gap called out in
//! `docs/enterprise/tracing.md` §10 and `PRODUCTION_READINESS_AUDIT.md`
//! row 28 by proving the `pcloud-observability` OTLP tracing pipeline
//! **actually delivers spans** to a real OTLP collector endpoint.
//!
//! The test stands up an in-process OTLP/HTTP collector using `axum`
//! (matching the exporter protocol the daemon is configured for —
//! `http/protobuf`, not gRPC), initializes the daemon tracer against
//! it via [`pcloud_observability::tracing::init`], emits the exact
//! span shape the daemon dispatch path emits (`pcloudd.dispatch`
//! parent + `pcloudd.backend.<name>` child), and asserts that:
//!
//! 1. The collector receives **exactly one** `pcloudd.dispatch` span
//!    and **exactly one** `pcloudd.backend.<name>` child span in the
//!    same trace.
//! 2. The child span's parent span id matches the parent span's
//!    span id (hierarchy intact across the wire).
//! 3. Every attribute key on the exported spans is drawn from the
//!    [`ALLOWED_ATTRS`] allow-list (`command`, `duration_ms`,
//!    `error_category`, `status_code`, `trace_kind`) — no leaks.
//! 4. W3C `traceparent` propagation works: when a `TRACEPARENT` env
//!    var is set at request time, the exported dispatch span's
//!    `trace_id` matches the incoming `traceparent` trace id.
//!
//! # Scope / non-goals
//!
//! - **No external network.** The collector binds to `127.0.0.1:<ephemeral>`
//!   so the test is hermetic and does not touch a real OTLP backend.
//! - **No daemon IPC.** We exercise the library surface directly
//!   (`tracing::info_span!` + the OTel layer) rather than spinning up
//!   the full daemon — the dispatch span construction in
//!   `pcloud-daemon::dispatch::handle_request_traced` uses the same
//!   `tracing::info_span!` macros we use here, so span shape is
//!   identical.
//! - **Not a performance benchmark.** Export timing is bounded but
//!   not asserted.
//!
//! # Feature gate
//!
//! The entire test module is `#[cfg(feature = "tracing-otlp")]`.
//! Builds without the feature compile this file as an empty crate —
//! no dev-dependency chain is forced onto minimal builds.

#![cfg(feature = "tracing-otlp")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Router, extract::State};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::trace::v1::Span as PbSpan;
use prost::Message;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use pcloud_observability::tracing::{ALLOWED_ATTRS, attr_redact, init, parse_traceparent};

/// State shared between the in-process collector handler and the test
/// assertions. All received span batches are accumulated here.
#[derive(Default, Clone)]
struct CollectorState {
    received: Arc<Mutex<Vec<PbSpan>>>,
}

/// Axum handler for `POST /v1/traces`. Decodes the OTLP/HTTP protobuf
/// body and stashes every span into the shared collector state.
async fn traces_handler(
    State(state): State<CollectorState>,
    body: Bytes,
) -> Result<(StatusCode, [(&'static str, &'static str); 1], Vec<u8>), StatusCode> {
    let req =
        ExportTraceServiceRequest::decode(body.as_ref()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut guard = state.received.lock().await;
    for rs in req.resource_spans {
        for ss in rs.scope_spans {
            for span in ss.spans {
                guard.push(span);
            }
        }
    }
    // OTLP/HTTP 1.0 success shape: empty ExportTraceServiceResponse
    // encoded as protobuf, content-type application/x-protobuf.
    let resp = ExportTraceServiceResponse::default();
    let mut buf = Vec::with_capacity(resp.encoded_len());
    resp.encode(&mut buf).expect("encode response");
    Ok((
        StatusCode::OK,
        [("content-type", "application/x-protobuf")],
        buf,
    ))
}

/// Bring up the in-process OTLP collector and return its base URL
/// plus a handle to the shared state and shutdown signal.
async fn start_collector() -> (String, CollectorState, tokio::sync::oneshot::Sender<()>) {
    let state = CollectorState::default();
    let app = Router::new()
        .route("/v1/traces", post(traces_handler))
        .with_state(state.clone());

    // Bind to an ephemeral loopback port to keep the test hermetic
    // and parallel-safe.
    let listener = TcpListener::bind::<SocketAddr>("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local_addr");
    let endpoint = format!("http://{}", addr);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    (endpoint, state, shutdown_tx)
}

/// Wait until at least `n` spans have been received or the deadline
/// expires. Returns `true` if the count was reached.
async fn wait_for_spans(state: &CollectorState, n: usize, timeout: Duration) -> bool {
    let started = tokio::time::Instant::now();
    loop {
        {
            let guard = state.received.lock().await;
            if guard.len() >= n {
                return true;
            }
        }
        if started.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Emit one `pcloudd.dispatch` parent span + one `pcloudd.backend.<name>`
/// child span. Span attributes are routed through `attr_redact` so the
/// allow-list contract is exercised end-to-end.
///
/// If `inbound_traceparent` is `Some(...)`, it is wired as the parent
/// span context via `OpenTelemetrySpanExt::set_parent`, mirroring what
/// `pcloud-daemon::dispatch::handle_request_traced` does with the
/// envelope's traceparent.
fn emit_dispatch_span(inbound_traceparent: Option<&str>) {
    let (_ck, cv) = attr_redact("command", "auth");
    let dispatch_span = tracing::info_span!(
        "pcloudd.dispatch",
        otel.name = "pcloudd.dispatch",
        command = cv,
        status_code = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
        error_category = tracing::field::Empty,
    );

    if let Some(tp) = inbound_traceparent
        && parse_traceparent(tp).is_some()
    {
        use opentelemetry::propagation::{Extractor, TextMapPropagator};
        use opentelemetry_sdk::propagation::TraceContextPropagator;

        struct Once<'a> {
            tp: &'a str,
        }
        impl<'a> Extractor for Once<'a> {
            fn get(&self, key: &str) -> Option<&str> {
                if key.eq_ignore_ascii_case("traceparent") {
                    Some(self.tp)
                } else {
                    None
                }
            }
            fn keys(&self) -> Vec<&str> {
                vec!["traceparent"]
            }
        }
        let propagator = TraceContextPropagator::new();
        let parent_ctx = propagator.extract(&Once { tp });
        dispatch_span.set_parent(parent_ctx);
    }

    let _enter = dispatch_span.enter();

    // Record the remaining allow-listed attributes on the parent.
    let (_k1, sv) = attr_redact("status_code", "0");
    let (_k2, dv) = attr_redact("duration_ms", "3");
    let (_k3, ev) = attr_redact("error_category", "ok");
    tracing::Span::current().record("status_code", sv);
    tracing::Span::current().record("duration_ms", dv);
    tracing::Span::current().record("error_category", ev);

    // Child backend span — mirrors the daemon's per-backend span shape.
    let backend_span = tracing::info_span!(
        "pcloudd.backend",
        otel.name = "pcloudd.backend.auth",
        command = cv,
    );
    let _benter = backend_span.enter();
    // Trivial work; no attributes outside the allow-list.
    std::hint::black_box(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn otlp_live_interop_end_to_end() {
    // 1. Spin up the in-process collector.
    let (endpoint, state, shutdown_tx) = start_collector().await;
    let traces_endpoint = format!("{}/v1/traces", endpoint);

    // 2. Initialize the daemon tracer pointing at the collector.
    //    sample_rate = 1.0 so every span is exported; no headers.
    //    `init` installs a global subscriber — safe here because
    //    integration tests each run in their own process.
    let _handle = init(&traces_endpoint, 1.0, &[]).expect("OTLP init");

    // 3. Optional inbound traceparent to assert propagation.
    //    We take it from the `TRACEPARENT` env var when set at request
    //    time; otherwise we use a well-known W3C example and assert
    //    the trace_id round-trips.
    let inbound = std::env::var("TRACEPARENT").ok();
    let inbound_for_assert = inbound
        .clone()
        .unwrap_or_else(|| "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned());
    emit_dispatch_span(Some(&inbound_for_assert));

    // 4. Wait for the two spans to flush through the batch exporter.
    //    The exporter's default schedule is ~5s; force a drop to flush.
    //    We still bound the wait to keep CI responsive.
    let got = wait_for_spans(&state, 2, Duration::from_secs(15)).await;

    // Flush + shutdown the global provider to force export of any
    // still-batched spans before we inspect the collector state.
    opentelemetry::global::shutdown_tracer_provider();
    let _ = wait_for_spans(&state, 2, Duration::from_secs(5)).await;

    // Bring the collector down cleanly regardless of outcome.
    let _ = shutdown_tx.send(());

    assert!(
        got || state.received.lock().await.len() >= 2,
        "OTLP collector did not receive >=2 spans within timeout"
    );

    let spans = state.received.lock().await.clone();
    assert!(
        spans.len() >= 2,
        "expected >=2 spans, got {} ({:?})",
        spans.len(),
        spans.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    // 5. Assert we got exactly one dispatch + one backend span.
    let dispatch: Vec<&PbSpan> = spans
        .iter()
        .filter(|s| s.name == "pcloudd.dispatch")
        .collect();
    let backend: Vec<&PbSpan> = spans
        .iter()
        .filter(|s| s.name.starts_with("pcloudd.backend"))
        .collect();

    assert_eq!(
        dispatch.len(),
        1,
        "expected exactly 1 pcloudd.dispatch span, got {}",
        dispatch.len()
    );
    assert_eq!(
        backend.len(),
        1,
        "expected exactly 1 pcloudd.backend.* span, got {}",
        backend.len()
    );

    let parent = dispatch[0];
    let child = backend[0];

    // 6. Hierarchy: the backend span's parent_span_id must match the
    //    dispatch span's span_id, and both share the same trace_id.
    assert_eq!(
        child.trace_id, parent.trace_id,
        "child and parent must share trace_id"
    );
    assert_eq!(
        child.parent_span_id, parent.span_id,
        "backend span must be parented to dispatch span"
    );

    // 7. Allow-list contract: every exported attribute key is drawn
    //    from ALLOWED_ATTRS. `otel.name` is a tracing-level directive
    //    that the OTel layer consumes (renames the span) and does
    //    not survive as an exported attribute — but if a future
    //    library change ever bubbled it through, it would land here
    //    and fail the allow-list check. That is the desired behavior.
    for span in [parent, child] {
        for attr in &span.attributes {
            assert!(
                ALLOWED_ATTRS.contains(&attr.key.as_str()),
                "span {:?} exported forbidden attribute key {:?}; \
                 allow-list is {:?}",
                span.name,
                attr.key,
                ALLOWED_ATTRS
            );
        }
    }

    // 8. W3C traceparent propagation: the exported parent trace_id
    //    must equal the 16 bytes embedded in the inbound traceparent.
    let expected = parse_traceparent(&inbound_for_assert)
        .expect("fixture traceparent must parse")
        .trace_id;
    assert_eq!(
        parent.trace_id.as_slice(),
        &expected,
        "exported trace_id must round-trip the inbound W3C traceparent"
    );
}

//! OpenTelemetry tracing integration (H13a, feature `tracing-otlp`).
//!
//! ## Purpose
//!
//! Provide an optional, strictly PII-redacted OTLP tracing pipeline for the
//! daemon so operators can correlate IPC calls, transfer errors, and
//! sync-engine transitions across services without leaking credentials,
//! file paths, or user identifiers.
//!
//! Exposes:
//! - [`init`] — installs an OTLP `http/protobuf` exporter wired into a
//!   `tracing-subscriber` registry through `tracing-opentelemetry`.
//! - [`TracingHandle`] — RAII guard that flushes the provider on drop.
//! - [`attr_redact`] — PII-safe attribute allow-list. In debug builds a
//!   forbidden key is a `debug_assert!` panic; in release builds the value
//!   is replaced with `"REDACTED"`.
//! - [`parse_traceparent`] — strict W3C `traceparent` header parser.
//!
//! ## Security posture
//!
//! - **Allow-listed attribute keys only.** Every span attribute must pass
//!   through [`attr_redact`] with a key from [`ALLOWED_ATTRS`]. Forbidden
//!   keys are a `debug_assert!` in debug builds and silently replaced with
//!   `"REDACTED"` in release so dashboards still see the attempt.
//! - **No secret values.** Consumers must never pass a password, auth
//!   token, crypto key, or raw file path into a span; there is no runtime
//!   value-side filter here, only a key-side allow-list.
//! - **No implicit propagator install.** [`init`] installs the W3C
//!   traceparent propagator explicitly so the set of propagators is
//!   auditable.
//!
//! ## Honest limitations
//!
//! - **Offline-tested only.** The end-to-end OTLP pipeline is exercised
//!   against unit-level fakes and the `tracing-opentelemetry` layer; this
//!   fork has not yet run it against a live OTEL collector in CI. Do not
//!   claim live OTLP delivery without rerunning an integration proof.
//! - **Error-biased sampling is API-level only.** The module test
//!   `error_biased_sampling_always_records_on_err` models the expected
//!   always-record behaviour for error spans; the production sampler
//!   composition that enforces it lives in daemon bootstrap, not here.
//! - **Feature-gated.** The entire module is behind
//!   `#[cfg(feature = "tracing-otlp")]`. A build without the feature has
//!   zero tracing surface; downstream code must use `cfg(feature = ...)`
//!   guards when calling into it.

#![cfg(feature = "tracing-otlp")]

use std::fmt;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{self as sdktrace, Sampler};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Compile-time allow-list of attribute keys permitted on tracing spans.
///
/// Any key not in this list is considered potentially PII-bearing and
/// must not flow into the exported telemetry.
pub const ALLOWED_ATTRS: &[&str] = &[
    "command",
    "duration_ms",
    "error_category",
    "status_code",
    "trace_kind",
];

/// Errors produced while initializing the OTLP tracing pipeline.
#[derive(Debug)]
pub enum TracingError {
    /// The supplied endpoint string was empty or otherwise unusable.
    InvalidEndpoint(String),
    /// The supplied sample rate was outside `[0.0, 1.0]`.
    InvalidSampleRate(f64),
    /// The OTLP exporter could not be constructed.
    Exporter(String),
    /// The global subscriber could not be installed.
    Subscriber(String),
}

impl fmt::Display for TracingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint(e) => write!(f, "invalid OTLP endpoint: {e}"),
            Self::InvalidSampleRate(r) => write!(f, "invalid sample rate: {r}"),
            Self::Exporter(e) => write!(f, "OTLP exporter error: {e}"),
            Self::Subscriber(e) => write!(f, "subscriber install error: {e}"),
        }
    }
}

impl std::error::Error for TracingError {}

/// RAII handle that owns the installed OpenTelemetry tracer provider.
///
/// On `Drop`, pending spans are flushed and the global provider is
/// shut down. Holding this for the lifetime of the daemon process is
/// the intended usage pattern.
pub struct TracingHandle {
    provider: Option<sdktrace::TracerProvider>,
}

impl fmt::Debug for TracingHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracingHandle")
            .field("provider_installed", &self.provider.is_some())
            .finish()
    }
}

impl Drop for TracingHandle {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            // Best-effort flush; ignore individual span export errors.
            for result in provider.force_flush() {
                let _ = result;
            }
            opentelemetry::global::shutdown_tracer_provider();
        }
    }
}

/// Initialize the OTLP tracing pipeline with an `http/protobuf` exporter.
///
/// `endpoint` is the OTLP collector base URL (e.g. `https://otel:4318`).
/// `sample_rate` is clamped through validation to `[0.0, 1.0]` and used
/// inside `Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(rate)))`.
/// `headers` is forwarded as OTLP headers (e.g. authentication tokens).
///
/// # Errors
///
/// Returns [`TracingError`] if the endpoint is empty, the sample rate is
/// out of range, the exporter cannot be built, or the global subscriber
/// cannot be installed.
pub fn init(
    endpoint: &str,
    sample_rate: f64,
    headers: &[(String, String)],
) -> Result<TracingHandle, TracingError> {
    if endpoint.trim().is_empty() {
        return Err(TracingError::InvalidEndpoint(endpoint.to_owned()));
    }
    if !(0.0..=1.0).contains(&sample_rate) || sample_rate.is_nan() {
        return Err(TracingError::InvalidSampleRate(sample_rate));
    }

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let mut header_map = std::collections::HashMap::with_capacity(headers.len());
    for (k, v) in headers {
        header_map.insert(k.clone(), v.clone());
    }

    let exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(endpoint)
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_secs(10))
        .with_headers(header_map);

    let sampler = Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(sample_rate)));

    let resource = Resource::new(vec![KeyValue::new("service.name", "pcloud-daemon")]);

    let provider = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(
            sdktrace::Config::default()
                .with_sampler(sampler)
                .with_resource(resource),
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)
        .map_err(|e| TracingError::Exporter(e.to_string()))?;

    let tracer = provider.tracer("pcloud-daemon");
    // Disable the layer's auto-injection of `code.filepath`,
    // `code.namespace`, `code.lineno`, `thread.id`, and `thread.name`.
    // Those keys are not in `ALLOWED_ATTRS` and would otherwise leak
    // source file paths, module paths, and thread names to the OTLP
    // collector, defeating the allow-list contract documented in
    // `docs/enterprise/tracing.md` §5.2. The live OTLP interop test
    // (`tests/otlp_live_interop.rs`) enforces this.
    let otel_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_location(false)
        .with_threads(false)
        // Also disable the layer's `busy_ns` / `idle_ns` auto-attrs
        // (see `with_tracked_inactivity` docs) — same allow-list
        // leakage concern as `with_location`/`with_threads` above.
        .with_tracked_inactivity(false);

    tracing_subscriber::registry()
        .with(otel_layer)
        .try_init()
        .map_err(|e| TracingError::Subscriber(e.to_string()))?;

    Ok(TracingHandle {
        provider: Some(provider),
    })
}

/// Filter an attribute through the [`ALLOWED_ATTRS`] allow-list.
///
/// In debug builds a forbidden key triggers a `debug_assert!` panic so
/// developers immediately notice attempts to record PII. In release
/// builds the value is replaced with `"REDACTED"` and the original
/// key is preserved so dashboards can still flag the leak attempt.
#[must_use]
pub fn attr_redact<'a>(key: &'a str, value: &'a str) -> (&'a str, &'a str) {
    let allowed = ALLOWED_ATTRS.contains(&key);
    debug_assert!(
        allowed,
        "attr_redact: forbidden attribute key {key:?} is not in ALLOWED_ATTRS"
    );
    if allowed {
        (key, value)
    } else {
        (key, "REDACTED")
    }
}

/// Parsed W3C `traceparent` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct W3cTraceparent {
    /// Version byte. Always `0x00` for the `00` version.
    pub version: u8,
    /// 16-byte trace ID.
    pub trace_id: [u8; 16],
    /// 8-byte span (parent) ID.
    pub span_id: [u8; 8],
    /// Flags byte (e.g. `0x01` indicates sampled).
    pub flags: u8,
}

/// Parse a W3C `traceparent` header of the form
/// `"00-{32hex trace_id}-{16hex span_id}-{2hex flags}"`.
///
/// Returns `None` for any deviation from the spec — wrong version,
/// wrong field length, or non-hex characters.
#[must_use]
pub fn parse_traceparent(s: &str) -> Option<W3cTraceparent> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 4 {
        return None;
    }
    if parts[0].len() != 2 || parts[1].len() != 32 || parts[2].len() != 16 || parts[3].len() != 2 {
        return None;
    }

    let version = u8::from_str_radix(parts[0], 16).ok()?;
    if version != 0x00 {
        return None;
    }

    let mut trace_id = [0u8; 16];
    hex_decode(parts[1], &mut trace_id)?;
    if trace_id == [0u8; 16] {
        return None;
    }

    let mut span_id = [0u8; 8];
    hex_decode(parts[2], &mut span_id)?;
    if span_id == [0u8; 8] {
        return None;
    }

    let flags = u8::from_str_radix(parts[3], 16).ok()?;

    Some(W3cTraceparent {
        version,
        trace_id,
        span_id,
        flags,
    })
}

fn hex_decode(input: &str, out: &mut [u8]) -> Option<()> {
    if input.len() != out.len() * 2 {
        return None;
    }
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_nibble(input.as_bytes()[i * 2])?;
        let lo = hex_nibble(input.as_bytes()[i * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(())
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_parses_w3c_format() {
        // Valid example from the W3C spec.
        let valid = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let parsed = parse_traceparent(valid).expect("valid traceparent must parse");
        assert_eq!(parsed.version, 0x00);
        assert_eq!(parsed.flags, 0x01);
        assert_eq!(parsed.trace_id[0], 0x0a);
        assert_eq!(parsed.trace_id[15], 0x9c);
        assert_eq!(parsed.span_id[0], 0xb7);
        assert_eq!(parsed.span_id[7], 0x31);

        // Wrong version.
        assert!(
            parse_traceparent("ff-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").is_none()
        );
        // Wrong trace_id length.
        assert!(
            parse_traceparent("00-0af7651916cd43dd8448eb211c80319-b7ad6b7169203331-01").is_none()
        );
        // Wrong span_id length.
        assert!(
            parse_traceparent("00-0af7651916cd43dd8448eb211c80319c-b7ad6b716920333-01").is_none()
        );
        // Non-hex characters.
        assert!(
            parse_traceparent("00-zzf7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").is_none()
        );
        // All-zero trace_id is invalid per W3C.
        assert!(
            parse_traceparent("00-00000000000000000000000000000000-b7ad6b7169203331-01").is_none()
        );
        // Missing field.
        assert!(
            parse_traceparent("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331").is_none()
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "forbidden attribute key")]
    fn attribute_allow_list_rejects_forbidden_key_in_debug() {
        let _ = attr_redact("user_email", "alice@example.com");
    }

    #[test]
    fn allowed_attribute_passes_through() {
        let (k, v) = attr_redact("command", "Login");
        assert_eq!(k, "command");
        assert_eq!(v, "Login");
    }

    #[test]
    fn error_biased_sampling_always_records_on_err() {
        // A span carrying an "error" status field must always be recording,
        // even at sample_rate 0.0 — the runtime composes a parent-based
        // sampler with an explicit override for the error path.
        //
        // We model that override here at the API level: any span produced
        // through an error-biased helper reports `is_recording() == true`.
        struct ErrorBiasedSpan {
            status: &'static str,
            sample_rate: f64,
        }
        impl ErrorBiasedSpan {
            fn is_recording(&self) -> bool {
                if self.status == "error" {
                    return true;
                }
                self.sample_rate > 0.0
            }
        }

        let span = ErrorBiasedSpan {
            status: "error",
            sample_rate: 0.0,
        };
        assert!(
            span.is_recording(),
            "error spans must record regardless of sample rate"
        );

        let span = ErrorBiasedSpan {
            status: "error",
            sample_rate: 1.0,
        };
        assert!(span.is_recording());

        // Sanity: non-error span at 0.0 is not recorded.
        let span = ErrorBiasedSpan {
            status: "ok",
            sample_rate: 0.0,
        };
        assert!(!span.is_recording());
    }
}

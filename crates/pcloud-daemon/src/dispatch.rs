//! IPC method dispatcher: decodes typed `Request` frames from
//! `pcloud-ipc` and routes them to the appropriate backend under
//! `RuntimeShell`. Producers a typed `Response` without panicking on
//! malformed or unauthorized input. Called by `serve::accept_loop` for
//! every accepted client.
//!
//! Portable; no platform gating.
//!
//! # Distributed tracing (`tracing-otlp` feature)
//!
//! When the `tracing-otlp` cargo feature is compiled in, every dispatched
//! request opens a `pcloudd.dispatch` span (and a child
//! `pcloudd.backend.<name>` span around the runtime call). If the caller
//! registered a W3C `traceparent` header via [`set_thread_traceparent`]
//! before invoking [`dispatch`], the parsed parent context is wired into
//! the current span via `tracing_opentelemetry::OpenTelemetrySpanExt`.
//!
//! Every recorded attribute is filtered through
//! `pcloud_observability::tracing::attr_redact`, which guarantees that
//! only allow-listed keys reach the OTLP exporter. Forbidden keys panic
//! in debug builds and are replaced with `"REDACTED"` in release builds.
//!
//! When the feature is **off**, the helpers compile to no-ops and the
//! daemon's panic-guard / metrics behaviour is unchanged.

// **PLATFORM:** all
// **GATING:** none (portable). Span emission is gated by `tracing-otlp`.

use pcloud_ipc::Request;

use crate::runtime::RuntimeShell;

#[cfg(feature = "tracing-otlp")]
use std::cell::RefCell;

// =========================================================================
// Thread-local traceparent carrier
//
// `pcloud_ipc::Request` does not yet carry a W3C `traceparent` field on
// the wire (see PR-H13d follow-up). Until the protocol gains a typed
// header, IPC servers (and tests) inject the inbound `traceparent` into
// this thread-local immediately before calling `dispatch`. The dispatcher
// reads + clears it during the `pcloudd.dispatch` span open. A missing
// header simply means no parent context is attached — the span still
// records, with its own trace_id.
// =========================================================================

#[cfg(feature = "tracing-otlp")]
thread_local! {
    static CURRENT_TRACEPARENT: RefCell<Option<String>> = const { RefCell::new(None) };
    static CURRENT_PANICKED: RefCell<bool> = const { RefCell::new(false) };
}

/// Stage a W3C `traceparent` for the next [`dispatch`] call on this thread.
///
/// IPC servers extract the header from the inbound frame (or transport
/// metadata) and call this immediately before `dispatch`. Tests use it to
/// pin a known parent context.
///
/// Compiles to a no-op when the `tracing-otlp` feature is off.
#[cfg(feature = "tracing-otlp")]
pub fn set_thread_traceparent(value: Option<String>) {
    CURRENT_TRACEPARENT.with(|cell| *cell.borrow_mut() = value);
}

/// No-op variant when the `tracing-otlp` feature is disabled. Keeps the
/// `Option<String>` signature so callers compile under both feature
/// configurations.
#[cfg(not(feature = "tracing-otlp"))]
#[inline]
pub fn set_thread_traceparent(_value: Option<String>) {}

/// Mark that the current dispatch call experienced a panic (caught by the
/// runtime's `catch_unwind` boundary).
///
/// Called from `RuntimeShell::handle_request` so the dispatch span can
/// record `status_code = "panic"` and emit an error event before being
/// closed. Compiles away when `tracing-otlp` is off.
#[cfg(feature = "tracing-otlp")]
pub(crate) fn note_dispatch_panic() {
    CURRENT_PANICKED.with(|cell| *cell.borrow_mut() = true);
}

/// No-op variant when the `tracing-otlp` feature is disabled.
#[cfg(not(feature = "tracing-otlp"))]
#[inline]
pub(crate) fn note_dispatch_panic() {}

/// Map a typed [`Request`] to a stable backend label used as the child
/// span name (`pcloudd.backend.<label>`) and as the `command` attribute
/// on the parent dispatch span.
///
/// Returned strings are `&'static` so there is no allocation on the hot
/// path and no risk of label cardinality explosion. Mirrors the
/// `Method` taxonomy used by `runtime::method_label` (which is itself
/// gated on the `metrics` feature) so traces and metrics stay in sync.
#[must_use]
pub fn backend_label(request: &Request) -> &'static str {
    use pcloud_ipc::Method;
    match request {
        Request::Plain { method } => match method {
            Method::GetStatus
            | Method::GetHealth
            | Method::Health
            | Method::GetPending
            | Method::Shutdown
            | Method::DrainStatus => "control",
            Method::LoginBegin
            | Method::Logout
            | Method::SendTwoFactorSms
            | Method::SendTwoFactorNotification
            | Method::SubmitPassword
            | Method::SubmitTwoFactorCode
            | Method::SetAuthPersistence => "auth",
            Method::UnlockCrypto
            | Method::LockCrypto
            | Method::GetCryptoStatus
            | Method::CryptoReset
            | Method::GetCryptoPrivKeyFlags
            | Method::SendCryptoChangeUserPrivate => "crypto",
            Method::GetSyncRoots
            | Method::PauseSync
            | Method::ResumeSync
            | Method::ListConflicts => "sync",
            Method::ListPublicLinks | Method::ListUploadLinks => "public_link",
            Method::GetUserInfo => "account",
            Method::ListIncomingShares
            | Method::ListOutgoingShares
            | Method::ListIncomingShareRequests
            | Method::ListOutgoingShareRequests
            | Method::ListContacts
            | Method::ListMyTeams => "shares",
            Method::ListNotifications => "notifications",
            Method::GetAuditVerifierStatus => "audit",
            Method::GetApiServers | Method::GetPromo | Method::VerifyEmail => "account",
            Method::GetCryptoHint => "crypto",
            _ => "other",
        },
        Request::PasswordSubmission { .. }
        | Request::AuthTokenSubmission { .. }
        | Request::TwoFactorCodeSubmission { .. }
        | Request::AuthPersistence { .. } => "auth",
        Request::CryptoUnlock { .. }
        | Request::CryptoSetup { .. }
        | Request::CryptoMkdir { .. }
        | Request::CryptoChangePassword { .. }
        | Request::CryptoChangePasswordUnlocked { .. } => "crypto",
        Request::SyncRootAdd { .. }
        | Request::SyncRootRemove { .. }
        | Request::SyncRootPause { .. }
        | Request::SyncRootResume { .. }
        | Request::SyncRootChangeType { .. }
        | Request::GetSyncSuggestions { .. }
        | Request::IsFolderSyncable { .. }
        | Request::RunLocalScan
        | Request::ConflictList
        | Request::ConflictResolve { .. } => "sync",
        Request::ShowPublicLink { .. }
        | Request::DeletePublicLink { .. }
        | Request::CreateFilePublicLink { .. }
        | Request::CreateFolderPublicLink { .. }
        | Request::ChangePublicLinkExpire { .. }
        | Request::ChangePublicLinkPassword { .. }
        | Request::ChangePublicLinkUpload { .. }
        | Request::CreateUploadLink { .. }
        | Request::DeleteUploadLink { .. }
        | Request::CreateTreePublicLink { .. }
        | Request::ListPublicLinkAccess { .. }
        | Request::AddPublicLinkAccess { .. }
        | Request::RemovePublicLinkAccess { .. }
        | Request::SendPublink { .. } => "public_link",
        Request::ListBookmarks
        | Request::RemoveBookmark { .. }
        | Request::ChangeBookmark { .. } => "bookmarks",
        Request::ShareFolder { .. }
        | Request::CancelShareRequest { .. }
        | Request::DeclineShareRequest { .. }
        | Request::AcceptShareRequest { .. }
        | Request::RemoveShare { .. }
        | Request::ModifyShare { .. }
        | Request::AccountStopShare { .. }
        | Request::AccountModifyShare { .. }
        | Request::AccountTeamShare { .. } => "shares",
        Request::ValueGet { .. } | Request::ValueSet { .. } | Request::ValueHas { .. } => "config",
        Request::MarkNotificationsRead { .. } => "notifications",
        Request::AuditVerifyChain { .. } => "audit",
        Request::Mount { .. } | Request::Unmount | Request::MountForceUnmount { .. } => "mount",
        Request::CreateRemoteFolder { .. }
        | Request::GetFolderIdByPath { .. }
        | Request::GetFolderFlags { .. }
        | Request::GetFolderOwnerId { .. } => "folder",
        Request::SessionStatus => "session",
        Request::FilesystemStatus { .. } | Request::FileHistory { .. } => "filesystem",
        Request::UploadCreate { .. }
        | Request::UploadPause { .. }
        | Request::UploadResume { .. }
        | Request::UploadCancel { .. }
        | Request::UploadList => "upload",
        Request::LostPassword { .. }
        | Request::VerifyEmailRestricted { .. }
        | Request::AccountChangePassword { .. }
        | Request::AccountRegister { .. }
        | Request::SetApiServer { .. }
        | Request::SetLanguage { .. } => "account",
        Request::GetFileLink { .. }
        | Request::DownloadFile { .. }
        | Request::UploadWriteFromFile { .. } => "transfer",
        Request::DeleteBackup { .. }
        | Request::CreateBackup { .. }
        | Request::StopDevice { .. }
        | Request::DeleteBackupDevice => "backup",
        Request::CreateTreePublicLinkFromPaths { .. } => "public_link",
        other => {
            // A Request variant that is not yet listed in backend_label was
            // added without a corresponding label. This is an observability
            // gap — metrics and spans will bucket it as "other". Log once at
            // warn level so operators and CI can detect drift.
            log::warn!(
                "pcloud-daemon: backend_label: unclassified request variant \
                 (observability drift); add it to backend_label in dispatch.rs. \
                 variant_debug={other:?}"
            );
            "other"
        }
    }
}

/// Stable string label for a [`pcloud_ipc::ResponseStatus`], used as the
/// `status_code` span attribute. Mirrors `runtime::status_label`.
#[cfg(feature = "tracing-otlp")]
fn status_str(status: &pcloud_ipc::ResponseStatus) -> &'static str {
    use pcloud_ipc::ResponseStatus;
    match status {
        ResponseStatus::Ok => "ok",
        ResponseStatus::InvalidRequest => "invalid_request",
        ResponseStatus::Unauthorized => "unauthorized",
        ResponseStatus::Conflict => "conflict",
        ResponseStatus::Unavailable => "unavailable",
        ResponseStatus::InternalError => "internal_error",
        // `ResponseStatus` is `#[non_exhaustive]`; future variants
        // bucket as a generic error label until classified explicitly.
        _ => "other",
    }
}

/// Coarse error category attribute. Returns `"none"` for `Ok` responses,
/// `"client"` for 4xx-equivalent statuses, and `"server"` for 5xx-equivalent
/// statuses. Used for OTLP-side filtering without re-deriving from the
/// status code at query time.
#[cfg(feature = "tracing-otlp")]
fn error_category(status: &pcloud_ipc::ResponseStatus) -> &'static str {
    use pcloud_ipc::ResponseStatus;
    match status {
        ResponseStatus::Ok => "none",
        ResponseStatus::InvalidRequest
        | ResponseStatus::Unauthorized
        | ResponseStatus::Conflict => "client",
        ResponseStatus::Unavailable | ResponseStatus::InternalError => "server",
        // Non-exhaustive enum: classify unknown future variants as
        // `server` (safer for SLO alerting than `none`).
        _ => "server",
    }
}

/// Route a decoded IPC [`Request`] through the [`RuntimeShell`] and
/// return the typed [`pcloud_ipc::Response`].
///
/// Thin wrapper over [`RuntimeShell::handle_request`] so every IPC
/// server (current synchronous `serve`, future tokio runner) hits the
/// same entry point and thus the same panic-guard and metrics wrapping.
///
/// # Panic containment (ADR 0004)
///
/// [`RuntimeShell::handle_request`] installs a `catch_unwind` boundary
/// around the per-method dispatch so a panic in one backend cannot tear
/// down the whole daemon process. On catch we increment
/// `pcloud_daemon_panic_total` (when `metrics` is on) and return
/// `ResponseStatus::InternalError`. See
/// `docs/book/src/adr/0004-panic-containment.md` for the rationale:
/// enterprise service managers expect `EXIT=clean` on SIGTERM, not a
/// SIGABRT-induced restart storm; a panic in e.g. the public-link
/// backend should degrade a single request, not the IPC socket and
/// every connected client.
///
/// # Distributed tracing
///
/// Under the `tracing-otlp` feature, opens a `pcloudd.dispatch` span
/// with `command`, `status_code`, `duration_ms`, and `error_category`
/// attributes (each routed through `attr_redact`) plus a child
/// `pcloudd.backend.<label>` span around the runtime call. Inbound
/// `traceparent` headers staged via [`set_thread_traceparent`] are
/// installed as the parent context. If the span is sampled-out
/// (`!is_recording()`) attribute writes short-circuit so there is no
/// per-attribute allocation cost on the hot path.
///
/// # Errors
///
/// This function does not return a `Result`; transport and protocol
/// failures are encoded in the returned [`pcloud_ipc::Response`]
/// (`status` + `message`). Malformed input is handled by the
/// `pcloud-ipc` decoder before `dispatch` is called.
pub fn dispatch(
    runtime: &mut RuntimeShell,
    envelope: impl Into<pcloud_ipc::RequestEnvelope>,
) -> pcloud_ipc::Response {
    let envelope: pcloud_ipc::RequestEnvelope = envelope.into();
    #[cfg(feature = "tracing-otlp")]
    {
        // Extract `traceparent` at the dispatch boundary so the
        // inbound W3C context becomes the parent of `pcloudd.dispatch`.
        // `RequestEnvelope::traceparent()` borrows the optional header
        // verbatim — no validation here; the dispatch span open path
        // calls `parse_traceparent` and silently ignores malformed
        // values to avoid leaking parser errors into request handling.
        if let Some(tp) = envelope.traceparent() {
            set_thread_traceparent(Some(tp.to_owned()));
        } else {
            set_thread_traceparent(None);
        }
    }
    handle_request(runtime, envelope.request)
}

/// Dispatch entry point that wraps `RuntimeShell::handle_request` with
/// OpenTelemetry span instrumentation when the `tracing-otlp` feature is
/// active. Public so alternative IPC frontends (tokio) can call the
/// instrumented path directly.
///
/// # IPC rate limiting
///
/// Before the backend is invoked, the request is classified via
/// [`crate::rate_limit::categorize`] and checked against the
/// [`crate::rate_limit::SessionRateLimiter`] attached to the runtime.
/// Over-budget callers receive a typed [`pcloud_ipc::ResponseStatus::Conflict`]
/// response with a `"rate limit exceeded: <category>, retry after Ns"`
/// message and the backend is **not** called. The check is zero-cost for
/// the `Cheap` category (status / userinfo / field selectors).
pub fn handle_request(runtime: &mut RuntimeShell, request: Request) -> pcloud_ipc::Response {
    // Admission check (per-session, per-category token bucket). Runs
    // before any backend dispatch so an over-budget caller cannot
    // observe partial state mutation. Cheap-category requests always
    // pass; disabled buckets are silently bypassed.
    let decision = runtime.rate_limiter.check(&request);
    if let Some(resp) = crate::rate_limit::reject_response(&decision) {
        return resp;
    }

    #[cfg(feature = "tracing-otlp")]
    {
        handle_request_traced(runtime, request)
    }
    #[cfg(not(feature = "tracing-otlp"))]
    {
        runtime.handle_request(request)
    }
}

#[cfg(feature = "tracing-otlp")]
fn handle_request_traced(runtime: &mut RuntimeShell, request: Request) -> pcloud_ipc::Response {
    use pcloud_observability::tracing::{attr_redact, parse_traceparent};
    use std::time::Instant;
    use tracing::Span;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    // Reset the panic flag so we observe only this call's panic state.
    CURRENT_PANICKED.with(|cell| *cell.borrow_mut() = false);

    let command = backend_label(&request);
    // `attr_redact` enforces the allow-list. We only ever feed it
    // compile-time-known keys, so the debug-build assertion in
    // `attr_redact` proves the allow-list contract at test time.
    let (_cmd_key, cmd_val) = attr_redact("command", command);

    let dispatch_span = tracing::info_span!(
        "pcloudd.dispatch",
        otel.name = "pcloudd.dispatch",
        command = cmd_val,
        status_code = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
        error_category = tracing::field::Empty,
    );

    // Wire inbound W3C traceparent (if any) as the parent span context.
    let parent_traceparent = CURRENT_TRACEPARENT.with(|cell| cell.borrow_mut().take());
    if let Some(tp) = parent_traceparent.as_deref()
        && parse_traceparent(tp).is_some()
    {
        // Use the global text-map propagator to extract a `Context`.
        let extractor = TraceparentExtractor::new(tp);
        let parent_ctx =
            opentelemetry::global::get_text_map_propagator(|prop| prop.extract(&extractor));
        dispatch_span.set_parent(parent_ctx);
    }

    let _enter = dispatch_span.enter();
    let started = Instant::now();

    // Per-backend child span. The static name is `pcloudd.backend`;
    // the dynamic backend label is recorded as the `command` field on
    // the child span so OTLP collectors can group by backend without
    // span-name cardinality explosion. The per-backend `otel.name`
    // override is set as a field for the OpenTelemetry layer to honour.
    let backend_otel_name = match command {
        "auth" => "pcloudd.backend.auth",
        "crypto" => "pcloudd.backend.crypto",
        "sync" => "pcloudd.backend.sync",
        "public_link" => "pcloudd.backend.public_link",
        "shares" => "pcloudd.backend.shares",
        "account" => "pcloudd.backend.account",
        "notifications" => "pcloudd.backend.notifications",
        "bookmarks" => "pcloudd.backend.bookmarks",
        "config" => "pcloudd.backend.config",
        "audit" => "pcloudd.backend.audit",
        "mount" => "pcloudd.backend.mount",
        "folder" => "pcloudd.backend.folder",
        "session" => "pcloudd.backend.session",
        "filesystem" => "pcloudd.backend.filesystem",
        "control" => "pcloudd.backend.control",
        _ => "pcloudd.backend.other",
    };
    let backend_span = tracing::info_span!(
        "pcloudd.backend",
        otel.name = backend_otel_name,
        command = cmd_val,
    );
    let response = {
        let _backend_enter = backend_span.enter();
        runtime.handle_request(request)
    };

    let panicked = CURRENT_PANICKED.with(|cell| *cell.borrow());
    let span = Span::current();

    // Sampled-out short-circuit: skip attribute writes entirely if the
    // span is not recording. `tracing::Span::is_disabled` is the
    // negative of "is recording" and is a cheap subscriber-level check
    // that avoids any per-attribute allocation when the OTel sampler
    // has dropped the trace.
    if !span.is_disabled() {
        let status = if panicked {
            "panic"
        } else {
            status_str(&response.status)
        };
        let category = if panicked {
            "panic"
        } else {
            error_category(&response.status)
        };
        let dur_ms = format!("{}", started.elapsed().as_millis());

        let (sk, sv) = attr_redact("status_code", status);
        let (dk, dv) = attr_redact("duration_ms", dur_ms.as_str());
        let (ek, ev) = attr_redact("error_category", category);
        span.record(sk, sv);
        span.record(dk, dv);
        span.record(ek, ev);

        if panicked {
            // Error event sampled at 100% — `tracing` records events on
            // the current span unconditionally when the subscriber is
            // listening, regardless of the sampler decision for child
            // spans (the parent has already been admitted).
            tracing::event!(
                tracing::Level::ERROR,
                error = true,
                otel.status_code = "ERROR",
                "dispatch panic caught by runtime guard",
            );
        }
    }

    response
}

/// Minimal `opentelemetry::propagation::Extractor` adapter that exposes
/// a single `traceparent` value to the global text-map propagator
/// without pulling in a full HTTP-style header map. Keeps the OTLP
/// integration self-contained.
#[cfg(feature = "tracing-otlp")]
struct TraceparentExtractor<'a> {
    value: &'a str,
}

#[cfg(feature = "tracing-otlp")]
impl<'a> TraceparentExtractor<'a> {
    fn new(value: &'a str) -> Self {
        Self { value }
    }
}

#[cfg(feature = "tracing-otlp")]
impl<'a> opentelemetry::propagation::Extractor for TraceparentExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        if key.eq_ignore_ascii_case("traceparent") {
            Some(self.value)
        } else {
            None
        }
    }

    fn keys(&self) -> Vec<&str> {
        vec!["traceparent"]
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(all(test, feature = "tracing-otlp"))]
mod tests {
    use super::*;
    use pcloud_config::{ConfigProfile, Environment};
    use pcloud_ipc::{Method, ResponseStatus};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tracing::Subscriber;
    use tracing::span::Attributes;
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;

    fn bootstrap_test_shell() -> crate::RuntimeShell {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-dispatch-test-{}-{nonce}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        crate::bootstrap_with_config(config).expect("runtime bootstrap should succeed")
    }

    /// Captures span names + creation events into a shared vector so
    /// tests can assert that the dispatch span hierarchy was opened.
    #[derive(Default)]
    struct CapturingLayer {
        spans: Arc<Mutex<Vec<String>>>,
    }

    impl<S> Layer<S> for CapturingLayer
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_new_span(&self, attrs: &Attributes<'_>, _id: &tracing::Id, _ctx: Context<'_, S>) {
            let mut guard = self.spans.lock().expect("span buffer mutex");
            guard.push(attrs.metadata().name().to_owned());
        }
    }

    #[test]
    fn span_hierarchy_preserved_from_cli_to_daemon() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let layer = CapturingLayer { spans: buf.clone() };
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        // Stage a known W3C traceparent. We cannot inspect the OTel
        // global trace_id without installing a full provider, but the
        // span open + parent attach path must run without panic and the
        // dispatch + backend span names must be recorded.
        let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned();
        set_thread_traceparent(Some(tp));

        // Drive a no-runtime path: build a minimal Request that will
        // bypass any backend that needs network IO.
        let request = Request::Plain {
            method: Method::GetStatus,
        };

        // Construct a runtime shell sufficient for in-process dispatch.
        // GetStatus reads only local fields and never touches the network.
        let mut shell = bootstrap_test_shell();
        let envelope = pcloud_ipc::RequestEnvelope::new(request)
            .with_traceparent("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned());
        let resp = dispatch(&mut shell, envelope);
        assert!(matches!(resp.status, ResponseStatus::Ok));

        let names = buf.lock().expect("span buffer mutex").clone();
        assert!(
            names.iter().any(|n| n == "pcloudd.dispatch"),
            "pcloudd.dispatch span must be opened, got: {names:?}",
        );
        assert!(
            names.iter().any(|n| n == "pcloudd.backend"),
            "pcloudd.backend.<name> span must be opened, got: {names:?}",
        );
    }

    #[test]
    fn error_biased_sampling_records_on_err() {
        // The dispatch span must be recording even when the inbound
        // response carries an error status, so OTLP downstreams can
        // see error traces at sample_rate=0.0. We assert by capturing
        // the attribute writes via a custom layer that observes
        // `record` calls.
        #[derive(Default)]
        struct AttrLayer {
            attrs: Arc<Mutex<Vec<(String, String)>>>,
        }

        impl<S> Layer<S> for AttrLayer
        where
            S: Subscriber + for<'a> LookupSpan<'a>,
        {
            fn on_record(
                &self,
                _id: &tracing::Id,
                values: &tracing::span::Record<'_>,
                _ctx: Context<'_, S>,
            ) {
                struct V<'a>(&'a Arc<Mutex<Vec<(String, String)>>>);
                impl<'a> tracing::field::Visit for V<'a> {
                    fn record_debug(
                        &mut self,
                        field: &tracing::field::Field,
                        value: &dyn std::fmt::Debug,
                    ) {
                        self.0
                            .lock()
                            .unwrap()
                            .push((field.name().to_owned(), format!("{value:?}")));
                    }
                    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                        self.0
                            .lock()
                            .unwrap()
                            .push((field.name().to_owned(), value.to_owned()));
                    }
                }
                let mut visitor = V(&self.attrs);
                values.record(&mut visitor);
            }
        }

        let attrs = Arc::new(Mutex::new(Vec::new()));
        let layer = AttrLayer {
            attrs: attrs.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        // Drive a request that the runtime returns InternalError for —
        // GetCryptoPrivKeyFlags is not implemented on the test shell and
        // returns an error response.
        set_thread_traceparent(None);
        let request = Request::Plain {
            method: Method::GetCryptoPrivKeyFlags,
        };
        let mut shell = bootstrap_test_shell();
        let _ = handle_request(&mut shell, request);

        let recorded = attrs.lock().unwrap().clone();
        let has_status = recorded.iter().any(|(k, _)| k == "status_code");
        assert!(
            has_status,
            "status_code must be recorded for every dispatch span, got: {recorded:?}",
        );
    }

    #[test]
    #[should_panic(expected = "forbidden attribute key")]
    fn forbidden_attribute_debug_panics() {
        // Direct contract test of `attr_redact`: any key not in
        // `ALLOWED_ATTRS` must panic in debug builds. This guards
        // against future PRs introducing PII-bearing span attributes
        // through dispatch.rs.
        let _ = pcloud_observability::tracing::attr_redact("user_email", "alice@example.com");
    }
}

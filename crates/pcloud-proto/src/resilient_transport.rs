//! Resilient wrapper over any [`ProtocolTransport`].
//!
//! `ResilientTransport` composes three primitives from `pcloud-resilience`:
//!
//! - a per-endpoint [`TokenBucket`] rate limiter,
//! - a per-endpoint [`CircuitBreaker`],
//! - a [`RetryPolicy`] for transient errors.
//!
//! **Opt-in only.** Callers must explicitly wrap a transport; the existing
//! direct-dispatch path in [`crate::transport::BinaryApiTransport`] is not
//! touched. This preserves the current test and production call sites that
//! want to bypass the wrapper.
//!
//! ## Determinism
//!
//! All time-dependent behavior (rate-limit refill, breaker reset, retry
//! backoff) reads through an injected [`Clock`]. Tests use
//! [`pcloud_resilience::clock::ManualClock`] together with an injected
//! [`Waiter`] so that `cargo test` never blocks on a real sleep.
//!
//! ## Auth-token handling
//!
//! The wrapper never copies, logs, or inspects request bytes. It just
//! forwards the `&EncodedRequest` to the inner transport unchanged, so any
//! embedded auth tokens pass through by reference — no extra cloning and no
//! leakage surface beyond what the inner transport already provides.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use pcloud_config::resilience::ResiliencePolicy;
use pcloud_resilience::transport::{
    TransportErrorClass, TransportOutcomeLabel, observe_transport_error, observe_transport_latency,
};
use pcloud_resilience::{
    BackoffSchedule, BreakerState, CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError,
    Clock, GlobalRetryBudget, RateLimitError, RetryDecision, RetryPolicy, SystemClock, TokenBucket,
    TokenBucketConfig,
};
use thiserror::Error;

use crate::EncodedRequest;
use crate::auth_api::ProtocolTransport;
use crate::response::Value;

/// Classification an inner transport error can receive.
///
/// `Permanent` errors are returned to the caller immediately (and count as a
/// failure against the circuit breaker). `Transient` errors may be retried
/// according to the [`RetryPolicy`]; each attempt also counts as a failure
/// against the breaker until a success flips it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Retryable.
    Transient,
    /// Not retryable — fail fast.
    Permanent,
}

/// Pluggable blocking-wait primitive so tests can run deterministically.
///
/// Production code uses [`ThreadSleepWaiter`]; tests can supply a clock-
/// advancing no-op waiter.
pub trait Waiter: Send + Sync + 'static {
    /// Block the current thread (or advance test time) for `dur`.
    fn wait(&self, dur: Duration);
}

/// Default production waiter backed by [`std::thread::sleep`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ThreadSleepWaiter;

impl Waiter for ThreadSleepWaiter {
    fn wait(&self, dur: Duration) {
        if !dur.is_zero() {
            thread::sleep(dur);
        }
    }
}

/// Error returned from [`ResilientTransport::execute`].
#[derive(Debug, Error)]
pub enum ResilientError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    /// The circuit breaker is open.
    #[error("circuit breaker is open")]
    CircuitOpen,
    /// The circuit breaker is half-open and the single probe slot is in use.
    #[error("circuit breaker probe is already in flight")]
    ProbeInFlight,
    /// The rate limiter shed this call (bucket empty, shedding mode).
    #[error("rate limit exceeded")]
    RateLimited,
    /// The rate-limiter config itself is bad.
    #[error("rate limiter misconfigured: {0}")]
    RateLimitConfig(#[from] RateLimitError),
    /// The inner transport returned a permanent or exhausted-retry error.
    #[error("inner transport error: {0}")]
    Inner(#[source] E),
    /// The shared global retry budget was exhausted — no further retries allowed.
    #[error("global retry budget exhausted")]
    BudgetExhausted,
}

/// Behaviour when the rate limiter has no tokens available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitMode {
    /// Return [`ResilientError::RateLimited`] immediately (shedding).
    Shed,
    /// Block on the injected [`Waiter`] for the reservation duration.
    Wait,
}

/// A classifier closure turning an inner error into an [`ErrorClass`].
pub type Classifier<E> = Arc<dyn Fn(&E) -> ErrorClass + Send + Sync>;

/// Resilient wrapper around a [`ProtocolTransport`].
///
/// Construct with [`ResilientTransport::new`] or
/// [`ResilientTransport::from_policy`]. The wrapper is cheaply cloneable and
/// thread-safe: all state (bucket, breaker) lives behind `Arc`s.
pub struct ResilientTransport<T>
where
    T: ProtocolTransport + Send + Sync,
{
    inner: Arc<T>,
    bucket: TokenBucket,
    breaker: CircuitBreaker,
    retry: RetryPolicy,
    waiter: Arc<dyn Waiter>,
    classifier: Classifier<T::Error>,
    rate_mode: RateLimitMode,
    /// Shared global retry budget — limits total retries across all concurrent
    /// operations on this transport. `None` when no budget was configured.
    budget: Option<Arc<GlobalRetryBudget>>,
    /// Endpoint label used for metric dimensions (e.g. `"binapi.pcloud.com"`).
    /// Empty string when no label is configured; the metric is still emitted
    /// but the label value will be blank in Prometheus output.
    host: String,
}

impl<T> Clone for ResilientTransport<T>
where
    T: ProtocolTransport + Send + Sync,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            bucket: self.bucket.clone(),
            breaker: self.breaker.clone(),
            retry: self.retry.clone(),
            waiter: self.waiter.clone(),
            classifier: self.classifier.clone(),
            rate_mode: self.rate_mode,
            budget: self.budget.clone(),
            host: self.host.clone(),
        }
    }
}

impl<T> fmt::Debug for ResilientTransport<T>
where
    T: ProtocolTransport + Send + Sync,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResilientTransport")
            .field("host", &self.host)
            .field("bucket", &self.bucket)
            .field("breaker", &self.breaker)
            .field("retry", &self.retry)
            .field("rate_mode", &self.rate_mode)
            .finish()
    }
}

impl<T> ResilientTransport<T>
where
    T: ProtocolTransport + Send + Sync,
{
    /// Build a wrapper from a [`ResiliencePolicy`] using [`SystemClock`],
    /// [`ThreadSleepWaiter`], and a default classifier that treats every
    /// inner error as transient.
    pub fn from_policy(inner: T, policy: &ResiliencePolicy) -> Result<Self, RateLimitError> {
        Self::build(
            inner,
            policy,
            Arc::new(SystemClock),
            Arc::new(ThreadSleepWaiter),
            default_classifier::<T::Error>(),
            RateLimitMode::Wait,
            None,
        )
    }

    /// Build a wrapper with fully-injected dependencies. Used by tests for
    /// deterministic timing and by callers that want a custom classifier
    /// (e.g. to mark specific `TransportError` variants permanent).
    pub fn new(
        inner: T,
        policy: &ResiliencePolicy,
        clock: Arc<dyn Clock>,
        waiter: Arc<dyn Waiter>,
        classifier: Classifier<T::Error>,
        rate_mode: RateLimitMode,
    ) -> Result<Self, RateLimitError> {
        Self::build(inner, policy, clock, waiter, classifier, rate_mode, None)
    }

    /// Build a wrapper with a shared [`GlobalRetryBudget`].
    ///
    /// Use this variant in production to bound the total number of retry
    /// attempts across all concurrent operations on the same client. When
    /// the budget is exhausted, retries are refused immediately rather than
    /// amplifying load on an already-struggling backend.
    pub fn with_budget(
        inner: T,
        policy: &ResiliencePolicy,
        clock: Arc<dyn Clock>,
        waiter: Arc<dyn Waiter>,
        classifier: Classifier<T::Error>,
        rate_mode: RateLimitMode,
        budget: Arc<GlobalRetryBudget>,
    ) -> Result<Self, RateLimitError> {
        Self::build(
            inner,
            policy,
            clock,
            waiter,
            classifier,
            rate_mode,
            Some(budget),
        )
    }

    fn build(
        inner: T,
        policy: &ResiliencePolicy,
        clock: Arc<dyn Clock>,
        waiter: Arc<dyn Waiter>,
        classifier: Classifier<T::Error>,
        rate_mode: RateLimitMode,
        budget: Option<Arc<GlobalRetryBudget>>,
    ) -> Result<Self, RateLimitError> {
        let bucket_cfg =
            TokenBucketConfig::new(policy.rate_limit_capacity, policy.rate_limit_refill_per_sec)?;
        let bucket = TokenBucket::with_clock(bucket_cfg, clock.clone());

        let breaker_cfg = CircuitBreakerConfig::new(
            policy.breaker_failure_threshold,
            policy.breaker_reset_timeout(),
        );
        let breaker = CircuitBreaker::with_clock(breaker_cfg, clock.clone());

        let schedule = BackoffSchedule::ExponentialJittered {
            base: policy.retry_base_delay(),
            factor: policy.retry_factor,
            max: policy.retry_max_delay(),
            seed: policy.retry_jitter_seed,
        };
        let retry = RetryPolicy::with_clock(policy.retry_max_attempts, schedule, clock);

        Ok(Self {
            inner: Arc::new(inner),
            bucket,
            breaker,
            retry,
            waiter,
            classifier,
            rate_mode,
            budget,
            host: String::new(),
        })
    }

    /// Override the host label used for metric dimensions.
    ///
    /// By default the label is taken from
    /// `ResiliencePolicy::endpoint_label`. Call this after construction to
    /// set or override the label (e.g. when the API-server hint changes the
    /// active endpoint).
    pub fn set_host_label(&mut self, host: impl Into<String>) {
        self.host = host.into();
    }

    /// Observable breaker state. Primarily for diagnostics/tests.
    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.state()
    }

    /// Borrow a clone of the inner transport's `Arc` so callers can
    /// call methods on the inner type that are not part of
    /// [`ProtocolTransport`] (e.g.
    /// [`crate::auth_api::ApiServerHintConsumer::apply_api_server_hint`]).
    /// CLAUDEREV deferred-set D5.1 (fire 49): unblocks per-backend
    /// transport wrapping while keeping the underlying transport reachable
    /// for non-`ProtocolTransport` trait calls. The returned `Arc` is a
    /// cheap pointer bump; mutating the inner state on one clone is
    /// observable through every other clone.
    pub fn inner_arc(&self) -> std::sync::Arc<T> {
        std::sync::Arc::clone(&self.inner)
    }

    /// Execute a request with rate-limit, circuit-breaker, and retry logic
    /// applied. Auth tokens embedded in the request bytes are forwarded by
    /// reference — never cloned by this wrapper.
    ///
    /// ## Observability
    ///
    /// Per-attempt wall-clock latency is emitted to
    /// `pcloud_transport_latency_seconds{outcome}` via the
    /// `pcloud-resilience` `transport-metrics` feature (on by default).
    /// Errors increment `pcloud_transport_errors_total{class}`.
    /// The `host` label dimension is set via [`Self::set_host_label`] (defaults
    /// to an empty string when not configured).
    ///
    /// ## Upload mutation safety
    ///
    /// `upload_write` and `upload_save` are **not idempotent** at the
    /// transport layer. The `UploadStateMachine` tracks the write offset and
    /// retries with the correct offset after a failure. If the transport
    /// layer were to retry these methods independently it could double-apply
    /// bytes (the server may have committed the write before the client
    /// received the error response). Those commands are therefore excluded
    /// from transport-layer retries here.
    pub fn execute(&self, request: &EncodedRequest) -> Result<Value, ResilientError<T::Error>> {
        let call_start = std::time::Instant::now();
        let mut attempt: u32 = 0;
        let mut had_retry = false;
        loop {
            attempt = attempt.saturating_add(1);
            let attempt_start = std::time::Instant::now();

            // 1. Rate limit.
            match self.rate_mode {
                RateLimitMode::Shed => {
                    if !self.bucket.try_acquire(1)? {
                        return Err(ResilientError::RateLimited);
                    }
                }
                RateLimitMode::Wait => {
                    let wait = self.bucket.acquire(1)?;
                    if !wait.is_zero() {
                        self.waiter.wait(wait);
                    }
                }
            }

            // 2. Circuit breaker admission.
            match self.breaker.try_acquire() {
                Ok(()) => {}
                Err(CircuitBreakerError::Open) => return Err(ResilientError::CircuitOpen),
                Err(CircuitBreakerError::ProbeInFlight) => {
                    return Err(ResilientError::ProbeInFlight);
                }
            }

            // 3. Inner call.
            match self.inner.execute(request) {
                Ok(v) => {
                    self.breaker.record_success();
                    // Replenish one budget token on success so transient bursts
                    // don't permanently deplete the shared pool.
                    if let Some(ref budget) = self.budget {
                        budget.replenish(1);
                    }
                    let latency = attempt_start.elapsed();
                    observe_transport_latency(
                        &self.host,
                        if had_retry {
                            TransportOutcomeLabel::Retry
                        } else {
                            TransportOutcomeLabel::Success
                        },
                        latency.as_secs_f64(),
                    );
                    return Ok(v);
                }
                Err(err) => {
                    let latency = attempt_start.elapsed();
                    self.breaker.record_failure();
                    let class = (self.classifier)(&err);
                    if class == ErrorClass::Permanent {
                        observe_transport_error(&self.host, TransportErrorClass::Io);
                        observe_transport_latency(
                            &self.host,
                            TransportOutcomeLabel::GiveUp,
                            latency.as_secs_f64(),
                        );
                        return Err(ResilientError::Inner(err));
                    }
                    // SAFETY: upload_write and upload_save are NOT idempotent.
                    // The UploadStateMachine owns offset-aware retry for these
                    // commands. Retrying here could double-apply bytes if the
                    // server committed the write before the client received the
                    // error response. Return immediately so the state machine
                    // can retry with the correct offset.
                    if is_upload_mutation(request) {
                        observe_transport_error(&self.host, TransportErrorClass::Io);
                        observe_transport_latency(
                            &self.host,
                            TransportOutcomeLabel::GiveUp,
                            latency.as_secs_f64(),
                        );
                        return Err(ResilientError::Inner(err));
                    }
                    match self.retry.next(attempt) {
                        RetryDecision::Retry { wait } => {
                            // Check global budget before consuming a retry slot.
                            if let Some(ref budget) = self.budget {
                                if !budget.try_consume() {
                                    observe_transport_error(
                                        &self.host,
                                        TransportErrorClass::BudgetExhausted,
                                    );
                                    observe_transport_latency(
                                        &self.host,
                                        TransportOutcomeLabel::GiveUp,
                                        call_start.elapsed().as_secs_f64(),
                                    );
                                    return Err(ResilientError::BudgetExhausted);
                                }
                            }
                            had_retry = true;
                            if !wait.is_zero() {
                                self.waiter.wait(wait);
                            }
                            continue;
                        }
                        RetryDecision::GiveUp => {
                            observe_transport_error(&self.host, TransportErrorClass::Io);
                            observe_transport_latency(
                                &self.host,
                                TransportOutcomeLabel::GiveUp,
                                call_start.elapsed().as_secs_f64(),
                            );
                            return Err(ResilientError::Inner(err));
                        }
                    }
                }
            }
        }
    }
}

/// Returns `true` when the request command is an upload mutation that must
/// **not** be retried at the transport layer.
///
/// `upload_write`, `upload_writefromfile`, and `upload_save` are non-idempotent:
/// the server may have committed the write before the client received the error
/// response. The `UploadStateMachine` is the authoritative retry owner for these
/// methods; it tracks the write offset and retries with the correct position.
///
/// `upload_writefromfile` (server-side copy IPC, row 93 in the parity matrix) is
/// included now that its IPC variant is wired: retrying it independently at the
/// transport layer could double-apply the server-side copy if the server committed
/// before the client saw the error.
///
/// The check is a simple ASCII string comparison against the command name
/// encoded in [`EncodedRequest::frame`].  Command names are short, static,
/// lower-case ASCII tokens; no allocation or regex is needed.
#[inline]
fn is_upload_mutation(request: &EncodedRequest) -> bool {
    matches!(
        request.frame.command.as_str(),
        "upload_write" | "upload_writefromfile" | "upload_save"
    )
}

/// Default classifier: every inner error is treated as **permanent** (not
/// retried) unless a caller explicitly supplies a custom classifier.
///
/// The previous default treated every error as transient (retryable).
/// That was over-broad: protocol-level errors such as `InvalidInput` (bad
/// request parameters), `InvalidAddress` (bad hostname), and similar
/// configuration-time mistakes will recur on every retry and only waste the
/// global retry budget. Callers that know a specific error is transient
/// (e.g. `TransportError::Io` for a connection reset) should supply a
/// domain-aware classifier via [`ResilientTransport::new`].
///
/// For the binary transport specifically, use [`transport_error_classifier`]
/// which promotes the handful of genuinely transient I/O variants while
/// keeping all others permanent.
pub fn default_classifier<E>() -> Classifier<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    Arc::new(|_: &E| ErrorClass::Permanent)
}

/// A `TransportError`-specific error classifier.
///
/// Maps protocol-level permanent failures (bad address, invalid TLS name,
/// TLS handshake errors) to [`ErrorClass::Permanent`] so the
/// [`ResilientTransport`] does **not** waste retry budget on errors that
/// are guaranteed to recur.
///
/// `TransportError::Io` is split by the underlying `io::ErrorKind`:
/// - `TimedOut`, `ConnectionReset`, `BrokenPipe`, `ConnectionAborted`,
///   `Interrupted`, and `WouldBlock` are **transient** — a fresh
///   connection attempt may succeed.
/// - All other `Io` kinds (e.g. `PermissionDenied`, `NotFound`,
///   `AddrNotAvailable`) are **permanent** — retrying cannot help.
///
/// `Connect` (TCP handshake failure) and `ResponseTooLarge` are
/// transient and permanent respectively: a connect failure may clear
/// on the next attempt but an oversized frame is a protocol error that
/// will recur.
///
/// Use this instead of [`default_classifier`] when wrapping a
/// [`crate::transport::BinaryApiTransport`].
pub fn transport_error_classifier() -> Classifier<crate::transport::TransportError> {
    use crate::transport::TransportError;
    use std::io::ErrorKind as K;
    Arc::new(|err: &TransportError| match err {
        // Permanent: these errors will not resolve on retry.
        TransportError::InvalidAddress { .. } => ErrorClass::Permanent,
        TransportError::InvalidServerName(_) => ErrorClass::Permanent,
        TransportError::Tls(_) => ErrorClass::Permanent,
        TransportError::SocketConfig(_) => ErrorClass::Permanent,
        TransportError::ResponseTooLarge { .. } => ErrorClass::Permanent,
        TransportError::ResponseHeader(_) => ErrorClass::Permanent,
        TransportError::ResponseBody(_) => ErrorClass::Permanent,
        // Io: classify by underlying kind — only a small set is genuinely transient.
        TransportError::Io(io_err) => match io_err.kind() {
            K::TimedOut
            | K::ConnectionReset
            | K::BrokenPipe
            | K::ConnectionAborted
            | K::Interrupted
            | K::WouldBlock => ErrorClass::Transient,
            _ => ErrorClass::Permanent,
        },
        // Connect: TCP handshake failure is transient (DNS flap, transient unreachable).
        TransportError::Connect(_) => ErrorClass::Transient,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_resilience::clock::ManualClock;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    // --- A scriptable fake transport --------------------------------------

    #[derive(Debug, Error)]
    enum FakeError {
        #[error("tempfail")]
        Temp,
        #[error("permfail")]
        Perm,
    }

    struct FakeTransport {
        calls: AtomicU32,
        script: Mutex<Vec<Outcome>>,
    }

    #[derive(Clone, Copy, Debug)]
    enum Outcome {
        Ok,
        Temp,
        Perm,
    }

    impl FakeTransport {
        fn new(script: Vec<Outcome>) -> Self {
            Self {
                calls: AtomicU32::new(0),
                script: Mutex::new(script),
            }
        }

        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl ProtocolTransport for FakeTransport {
        type Error = FakeError;
        fn execute(&self, _request: &EncodedRequest) -> Result<Value, Self::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = {
                let mut g = self.script.lock().unwrap();
                if g.is_empty() {
                    Outcome::Temp
                } else {
                    g.remove(0)
                }
            };
            match next {
                Outcome::Ok => Ok(Value::Hash(Vec::new())),
                Outcome::Temp => Err(FakeError::Temp),
                Outcome::Perm => Err(FakeError::Perm),
            }
        }
    }

    // A waiter that records waits but never blocks, so tests are instant.
    struct RecordingWaiter {
        total: Mutex<Duration>,
    }

    impl RecordingWaiter {
        fn new() -> Self {
            Self {
                total: Mutex::new(Duration::ZERO),
            }
        }
    }

    impl Waiter for RecordingWaiter {
        fn wait(&self, dur: Duration) {
            let mut g = self.total.lock().unwrap();
            *g += dur;
        }
    }

    fn dummy_request() -> EncodedRequest {
        crate::encode_request("noop", &[], Some(0)).unwrap()
    }

    fn classifier_temp_is_transient() -> Classifier<FakeError> {
        Arc::new(|e: &FakeError| match e {
            FakeError::Temp => ErrorClass::Transient,
            FakeError::Perm => ErrorClass::Permanent,
        })
    }

    fn test_policy(retry_attempts: u32, cap: u32, refill: f64, thr: u32) -> ResiliencePolicy {
        ResiliencePolicy {
            enabled: true,
            rate_limit_capacity: cap,
            rate_limit_refill_per_sec: refill,
            breaker_failure_threshold: thr,
            breaker_reset_timeout_ms: 100,
            retry_max_attempts: retry_attempts,
            retry_base_delay_ms: 10,
            retry_factor: 2.0,
            retry_max_delay_ms: 100,
            retry_jitter_seed: 7,
        }
    }

    #[test]
    fn rate_limit_shed_returns_error_when_bucket_empty() {
        let clock = Arc::new(ManualClock::new());
        let waiter = Arc::new(RecordingWaiter::new());
        // Capacity=1, refill slow enough that two back-to-back calls shed.
        let policy = test_policy(1, 1, 0.01, 3);
        let fake = FakeTransport::new(vec![Outcome::Ok, Outcome::Ok]);
        let rt = ResilientTransport::new(
            fake,
            &policy,
            clock.clone(),
            waiter.clone(),
            classifier_temp_is_transient(),
            RateLimitMode::Shed,
        )
        .unwrap();

        // First call succeeds (token available).
        let req = dummy_request();
        rt.execute(&req).unwrap();
        // Second call must be shed — no real sleep.
        let err = rt.execute(&req).unwrap_err();
        assert!(matches!(err, ResilientError::RateLimited));
        // No waits should have been recorded.
        assert_eq!(*waiter.total.lock().unwrap(), Duration::ZERO);
    }

    #[test]
    fn circuit_breaker_open_half_open_closed_cycle() {
        let clock = Arc::new(ManualClock::new());
        let waiter = Arc::new(RecordingWaiter::new());
        // Threshold=2, 1 retry only (so 2 attempts -> 2 failures -> trip).
        let policy = test_policy(1, 100, 1000.0, 2);
        // Two temp failures to trip, then an Ok to close on probe.
        let fake = FakeTransport::new(vec![Outcome::Temp, Outcome::Temp, Outcome::Ok]);
        let rt = ResilientTransport::new(
            fake,
            &policy,
            clock.clone(),
            waiter,
            classifier_temp_is_transient(),
            RateLimitMode::Wait,
        )
        .unwrap();

        let req = dummy_request();
        // Attempt 1: Temp -> recorded failure (1). retry=GiveUp (max_attempts=1).
        let _ = rt.execute(&req).unwrap_err();
        // Attempt 2: Temp -> recorded failure (2). breaker trips Open.
        let _ = rt.execute(&req).unwrap_err();
        assert_eq!(rt.breaker_state(), BreakerState::Open);

        // While Open, every call rejects fast.
        let err = rt.execute(&req).unwrap_err();
        assert!(matches!(err, ResilientError::CircuitOpen));

        // Advance clock past reset_timeout -> HalfOpen probe admitted, Ok closes.
        clock.advance(Duration::from_millis(100));
        rt.execute(&req).unwrap();
        assert_eq!(rt.breaker_state(), BreakerState::Closed);
    }

    #[test]
    fn retry_with_backoff_succeeds_after_transient() {
        let clock = Arc::new(ManualClock::new());
        let waiter = Arc::new(RecordingWaiter::new());
        let policy = test_policy(3, 100, 1000.0, 10);
        let fake = FakeTransport::new(vec![Outcome::Temp, Outcome::Temp, Outcome::Ok]);
        let fake_ptr: *const FakeTransport = &fake;
        let rt = ResilientTransport::new(
            fake,
            &policy,
            clock,
            waiter.clone(),
            classifier_temp_is_transient(),
            RateLimitMode::Wait,
        )
        .unwrap();

        rt.execute(&dummy_request()).unwrap();
        // Safe: FakeTransport was moved into Arc inside rt; raw ptr above
        // is unused afterwards. We instead assert via script consumption.
        let _ = fake_ptr;
        // Two backoff waits were recorded (attempts 1 and 2 failed).
        assert!(*waiter.total.lock().unwrap() > Duration::ZERO);
    }

    #[test]
    fn no_retry_on_permfail() {
        let clock = Arc::new(ManualClock::new());
        let waiter = Arc::new(RecordingWaiter::new());
        let policy = test_policy(5, 100, 1000.0, 10);
        let fake = FakeTransport::new(vec![Outcome::Perm, Outcome::Ok]);
        // We need a handle on calls after move — wrap in Arc manually.
        let fake = Arc::new(fake);
        // Trick: clone Arc, then use a newtype that forwards to the inner.
        struct Forward(Arc<FakeTransport>);
        impl ProtocolTransport for Forward {
            type Error = FakeError;
            fn execute(&self, r: &EncodedRequest) -> Result<Value, Self::Error> {
                self.0.execute(r)
            }
        }
        let rt = ResilientTransport::new(
            Forward(fake.clone()),
            &policy,
            clock,
            waiter,
            classifier_temp_is_transient(),
            RateLimitMode::Wait,
        )
        .unwrap();

        let err = rt.execute(&dummy_request()).unwrap_err();
        assert!(matches!(err, ResilientError::Inner(FakeError::Perm)));
        // Only one underlying call — no retries on permanent failures.
        assert_eq!(fake.calls(), 1);
    }

    #[test]
    fn budget_exhaustion_stops_retries() {
        let clock = Arc::new(ManualClock::new());
        let waiter = Arc::new(RecordingWaiter::new());
        // 5 retry attempts allowed by policy, but budget only has 1 token.
        let policy = test_policy(5, 100, 1000.0, 10);
        // Script: all temp failures so retries would otherwise continue.
        let fake = FakeTransport::new(vec![
            Outcome::Temp,
            Outcome::Temp,
            Outcome::Temp,
            Outcome::Temp,
            Outcome::Ok,
        ]);
        let budget = Arc::new(GlobalRetryBudget::new(1));
        let rt = ResilientTransport::with_budget(
            fake,
            &policy,
            clock,
            waiter,
            classifier_temp_is_transient(),
            RateLimitMode::Wait,
            budget.clone(),
        )
        .unwrap();

        // First call fails, retries once (consuming the 1 token), then budget
        // is exhausted and the next retry attempt must be refused.
        let err = rt.execute(&dummy_request()).unwrap_err();
        assert!(
            matches!(err, ResilientError::BudgetExhausted),
            "expected BudgetExhausted, got {err:?}"
        );
    }
}

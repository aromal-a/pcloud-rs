//! Retry policy with pluggable backoff schedules.
//!
//! A [`RetryPolicy`] computes the wait duration for each attempt but does
//! **not** perform the wait itself; callers integrate the returned
//! [`RetryDecision`] with whatever async runtime they already use. This keeps
//! the crate runtime-agnostic and keeps all timing under the control of the
//! injected [`Clock`] so tests never rely on wall-clock sleeps.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::time::Duration;

use crate::clock::{Clock, SystemClock};
use std::sync::Arc;

/// Backoff strategy.
#[derive(Debug, Clone, Copy)]
pub enum BackoffSchedule {
    /// Constant delay between attempts.
    Fixed {
        /// Delay for every retry.
        delay: Duration,
    },
    /// Exponential backoff: `base * factor^(attempt-1)`, capped at `max`.
    Exponential {
        /// Delay for the first retry.
        base: Duration,
        /// Growth factor per attempt (must be >= 1.0).
        factor: f64,
        /// Upper bound on the computed delay.
        max: Duration,
    },
    /// Exponential backoff plus deterministic "equal jitter":
    /// `d/2 + rand_in_half(d/2)` where the random half is derived from a
    /// seeded stream so tests remain reproducible.
    ExponentialJittered {
        /// Delay for the first retry.
        base: Duration,
        /// Growth factor per attempt (must be >= 1.0).
        factor: f64,
        /// Upper bound on the computed delay.
        max: Duration,
        /// Seed for the deterministic jitter stream.
        seed: u64,
    },
}

/// Decision emitted by [`RetryPolicy::next`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Retry after the given delay.
    Retry {
        /// Duration to wait before the next attempt.
        wait: Duration,
    },
    /// No more retries permitted.
    GiveUp,
}

/// Retry policy. Cheap to clone; stateless aside from the injected clock.
#[derive(Clone)]
pub struct RetryPolicy {
    max_attempts: u32,
    schedule: BackoffSchedule,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_attempts", &self.max_attempts)
            .field("schedule", &self.schedule)
            .finish()
    }
}

impl RetryPolicy {
    /// Builds a policy with [`SystemClock`].
    ///
    /// `max_attempts` is the total number of attempts **including** the
    /// first call, so `max_attempts = 3` yields up to two retries.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::{BackoffSchedule, RetryDecision, RetryPolicy};
    ///
    /// let policy = RetryPolicy::new(
    ///     3,
    ///     BackoffSchedule::Fixed { delay: Duration::from_millis(100) },
    /// );
    /// assert_eq!(
    ///     policy.next(1),
    ///     RetryDecision::Retry { wait: Duration::from_millis(100) },
    /// );
    /// assert_eq!(policy.next(3), RetryDecision::GiveUp);
    /// ```
    pub fn new(max_attempts: u32, schedule: BackoffSchedule) -> Self {
        Self::with_clock(max_attempts, schedule, Arc::new(SystemClock))
    }

    /// Builds a policy with an injected [`Clock`].
    pub fn with_clock(max_attempts: u32, schedule: BackoffSchedule, clock: Arc<dyn Clock>) -> Self {
        assert!(max_attempts >= 1, "max_attempts must be >= 1");
        if let BackoffSchedule::Exponential { factor, .. }
        | BackoffSchedule::ExponentialJittered { factor, .. } = schedule
        {
            assert!(
                factor >= 1.0 && factor.is_finite(),
                "exponential factor must be finite and >= 1.0"
            );
        }
        Self {
            max_attempts,
            schedule,
            clock,
        }
    }

    /// Returns the configured [`Clock`], useful for callers that want to
    /// record an attempt start instant.
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    /// Computes the decision for the given attempt number. `attempt` is
    /// 1-indexed: `1` means "the first call just failed, should we retry?".
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::{BackoffSchedule, RetryDecision, RetryPolicy};
    ///
    /// let p = RetryPolicy::new(
    ///     4,
    ///     BackoffSchedule::Exponential {
    ///         base: Duration::from_millis(10),
    ///         factor: 2.0,
    ///         max: Duration::from_secs(1),
    ///     },
    /// );
    /// // first retry: 10ms, second: 20ms, third: 40ms.
    /// match p.next(1) {
    ///     RetryDecision::Retry { wait } => assert_eq!(wait, Duration::from_millis(10)),
    ///     RetryDecision::GiveUp => unreachable!(),
    /// }
    /// ```
    pub fn next(&self, attempt: u32) -> RetryDecision {
        self.next_wait(attempt, None)
    }

    /// Same as [`Self::next`] but accepts an optional explicit wait
    /// override that takes precedence over the computed backoff.
    ///
    /// Use this to honour a server-supplied `Retry-After` hint: when
    /// the caller parses a 429/503 response it passes the duration
    /// here, and the policy returns `RetryDecision::Retry { wait =
    /// override_wait }` (unless the attempt budget is already
    /// exhausted, in which case [`RetryDecision::GiveUp`] still wins).
    ///
    /// The override is respected as-is — callers are responsible for
    /// any upper-bound capping (the HTTP helpers cap at 300 s, see
    /// `http_download::parse_retry_after`).
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::{BackoffSchedule, RetryDecision, RetryPolicy};
    ///
    /// let p = RetryPolicy::new(
    ///     3,
    ///     BackoffSchedule::Fixed { delay: Duration::from_millis(10) },
    /// );
    /// // Server asked us to wait 5 seconds; policy-computed 10 ms is ignored.
    /// assert_eq!(
    ///     p.next_wait(1, Some(Duration::from_secs(5))),
    ///     RetryDecision::Retry { wait: Duration::from_secs(5) },
    /// );
    /// ```
    pub fn next_wait(&self, attempt: u32, override_wait: Option<Duration>) -> RetryDecision {
        if attempt >= self.max_attempts {
            return RetryDecision::GiveUp;
        }
        let wait = override_wait.unwrap_or_else(|| self.compute_wait(attempt));
        RetryDecision::Retry { wait }
    }

    fn compute_wait(&self, attempt: u32) -> Duration {
        match self.schedule {
            BackoffSchedule::Fixed { delay } => delay,
            BackoffSchedule::Exponential { base, factor, max } => {
                exp_delay(base, factor, max, attempt)
            }
            BackoffSchedule::ExponentialJittered {
                base,
                factor,
                max,
                seed,
            } => {
                let d = exp_delay(base, factor, max, attempt);
                apply_equal_jitter(d, seed, attempt)
            }
        }
    }
}

fn exp_delay(base: Duration, factor: f64, max: Duration, attempt: u32) -> Duration {
    // attempt is 1-indexed: first retry uses base.
    let exp = factor.powi(attempt.saturating_sub(1) as i32);
    let nanos = (base.as_nanos() as f64) * exp;
    // Guard against overflow / NaN.
    if !nanos.is_finite() || nanos < 0.0 {
        return max;
    }
    let max_nanos = max.as_nanos() as f64;
    let clamped = nanos.min(max_nanos);
    // Safe: clamped is finite, non-negative, and <= max_nanos which fits u64 range in practice.
    let as_u128 = clamped as u128;
    Duration::new(
        (as_u128 / 1_000_000_000) as u64,
        (as_u128 % 1_000_000_000) as u32,
    )
}

/// Equal-jitter per AWS: `d/2 + rand(0, d/2)`, deterministic from (seed, attempt).
fn apply_equal_jitter(d: Duration, seed: u64, attempt: u32) -> Duration {
    if d.is_zero() {
        return d;
    }
    let half_nanos = (d.as_nanos() / 2) as u64;
    let rnd = splitmix64(seed ^ (attempt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let extra = if half_nanos == 0 { 0 } else { rnd % half_nanos };
    Duration::from_nanos(half_nanos + extra)
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Method-level retry class.
///
/// `pcloud-resilience` stays free of cross-crate IPC types; callers on the
/// daemon / proto side map their own `Method` enum onto this coarse
/// classification. The contract is deliberately narrow:
///
/// - [`RetryClass::Idempotent`] — safe to retry without side effects.
///   Examples: `GetStatus`, `ListPublicLinks`, health probes, reads.
/// - [`RetryClass::Mutation`] — has observable side effects (create,
///   delete, snapshot, password change). Retry only when the policy
///   explicitly allows, and only for transport-layer failures where the
///   server is known not to have seen the request.
/// - [`RetryClass::Unknown`] — caller did not classify; treat as
///   non-retriable unless the policy overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetryClass {
    /// Read-only / idempotent method; always safe to retry.
    Idempotent,
    /// State-changing method; retry only when allowed by policy.
    Mutation,
    /// Unclassified; default to non-retriable.
    Unknown,
}

/// Method-aware wrapper over [`RetryPolicy`] that decides both *whether*
/// to retry a given operation and *how long* to wait.
///
/// The policy is intentionally simple so callers can compose it without
/// pulling in the full IPC method taxonomy: the caller classifies every
/// request via [`RetryClass`] before invoking [`MethodRetryPolicy::next`].
#[derive(Clone)]
pub struct MethodRetryPolicy {
    inner: RetryPolicy,
    retry_idempotent: bool,
    retry_mutations: bool,
    retry_unknown: bool,
}

impl std::fmt::Debug for MethodRetryPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MethodRetryPolicy")
            .field("inner", &self.inner)
            .field("retry_idempotent", &self.retry_idempotent)
            .field("retry_mutations", &self.retry_mutations)
            .field("retry_unknown", &self.retry_unknown)
            .finish()
    }
}

impl MethodRetryPolicy {
    /// Build a secure-default method policy: idempotent methods may
    /// retry, mutations and unknown classes do not.
    pub fn secure_default(inner: RetryPolicy) -> Self {
        Self {
            inner,
            retry_idempotent: true,
            retry_mutations: false,
            retry_unknown: false,
        }
    }

    /// Explicit constructor. Callers can opt into retrying mutations
    /// when the underlying transport guarantees exactly-once semantics
    /// (for example an idempotency-keyed upload).
    pub fn new(
        inner: RetryPolicy,
        retry_idempotent: bool,
        retry_mutations: bool,
        retry_unknown: bool,
    ) -> Self {
        Self {
            inner,
            retry_idempotent,
            retry_mutations,
            retry_unknown,
        }
    }

    /// Returns the wrapped clock-driven [`RetryPolicy`].
    #[must_use]
    pub fn inner(&self) -> &RetryPolicy {
        &self.inner
    }

    /// Decide the next step for the given method class and attempt number.
    ///
    /// Returns [`RetryDecision::GiveUp`] immediately if the class is not
    /// retriable under this policy; otherwise delegates to the underlying
    /// [`RetryPolicy`] for the wait-duration computation.
    #[must_use]
    pub fn next(&self, class: RetryClass, attempt: u32) -> RetryDecision {
        self.next_wait(class, attempt, None)
    }

    /// Same as [`Self::next`] but accepts an optional explicit wait
    /// override. Used to honour a server-supplied `Retry-After` hint
    /// (audit-04 H-4): the transport layer passes the parsed duration
    /// here and the policy returns it unchanged on `Retry`, so there
    /// is a single source of truth for "how long to wait".
    #[must_use]
    pub fn next_wait(
        &self,
        class: RetryClass,
        attempt: u32,
        override_wait: Option<Duration>,
    ) -> RetryDecision {
        let allowed = match class {
            RetryClass::Idempotent => self.retry_idempotent,
            RetryClass::Mutation => self.retry_mutations,
            RetryClass::Unknown => self.retry_unknown,
        };
        if !allowed {
            return RetryDecision::GiveUp;
        }
        self.inner.next_wait(attempt, override_wait)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_backoff_is_constant() {
        let p = RetryPolicy::new(
            5,
            BackoffSchedule::Fixed {
                delay: Duration::from_millis(25),
            },
        );
        for attempt in 1..=4 {
            assert_eq!(
                p.next(attempt),
                RetryDecision::Retry {
                    wait: Duration::from_millis(25)
                }
            );
        }
        assert_eq!(p.next(5), RetryDecision::GiveUp);
    }

    #[test]
    fn exponential_backoff_grows_and_caps() {
        let p = RetryPolicy::new(
            10,
            BackoffSchedule::Exponential {
                base: Duration::from_millis(10),
                factor: 2.0,
                max: Duration::from_millis(80),
            },
        );
        let RetryDecision::Retry { wait: w1 } = p.next(1) else {
            panic!()
        };
        let RetryDecision::Retry { wait: w2 } = p.next(2) else {
            panic!()
        };
        let RetryDecision::Retry { wait: w3 } = p.next(3) else {
            panic!()
        };
        let RetryDecision::Retry { wait: w4 } = p.next(4) else {
            panic!()
        };
        let RetryDecision::Retry { wait: w5 } = p.next(5) else {
            panic!()
        };
        assert_eq!(w1, Duration::from_millis(10));
        assert_eq!(w2, Duration::from_millis(20));
        assert_eq!(w3, Duration::from_millis(40));
        assert_eq!(w4, Duration::from_millis(80));
        assert_eq!(w5, Duration::from_millis(80)); // capped
    }

    #[test]
    fn jittered_backoff_is_deterministic_for_seed() {
        let p1 = RetryPolicy::new(
            5,
            BackoffSchedule::ExponentialJittered {
                base: Duration::from_millis(100),
                factor: 2.0,
                max: Duration::from_millis(1000),
                seed: 0xDEAD_BEEF,
            },
        );
        let p2 = RetryPolicy::new(
            5,
            BackoffSchedule::ExponentialJittered {
                base: Duration::from_millis(100),
                factor: 2.0,
                max: Duration::from_millis(1000),
                seed: 0xDEAD_BEEF,
            },
        );
        for attempt in 1..5 {
            assert_eq!(p1.next(attempt), p2.next(attempt));
        }
    }

    #[test]
    fn jittered_backoff_bounds() {
        let p = RetryPolicy::new(
            5,
            BackoffSchedule::ExponentialJittered {
                base: Duration::from_millis(100),
                factor: 2.0,
                max: Duration::from_millis(1000),
                seed: 42,
            },
        );
        for attempt in 1..5 {
            let RetryDecision::Retry { wait } = p.next(attempt) else {
                panic!()
            };
            let upper_unjittered = exp_delay(
                Duration::from_millis(100),
                2.0,
                Duration::from_millis(1000),
                attempt,
            );
            assert!(wait >= upper_unjittered / 2);
            assert!(wait <= upper_unjittered);
        }
    }

    #[test]
    fn gives_up_at_max_attempts() {
        let p = RetryPolicy::new(
            1,
            BackoffSchedule::Fixed {
                delay: Duration::from_millis(10),
            },
        );
        assert_eq!(p.next(1), RetryDecision::GiveUp);
    }

    #[test]
    fn method_policy_secure_default_only_retries_idempotent() {
        let inner = RetryPolicy::new(
            3,
            BackoffSchedule::Fixed {
                delay: Duration::from_millis(50),
            },
        );
        let policy = MethodRetryPolicy::secure_default(inner);
        assert_eq!(
            policy.next(RetryClass::Idempotent, 1),
            RetryDecision::Retry {
                wait: Duration::from_millis(50)
            }
        );
        assert_eq!(policy.next(RetryClass::Mutation, 1), RetryDecision::GiveUp);
        assert_eq!(policy.next(RetryClass::Unknown, 1), RetryDecision::GiveUp);
    }

    #[test]
    fn method_policy_respects_inner_max_attempts() {
        let inner = RetryPolicy::new(
            2,
            BackoffSchedule::Fixed {
                delay: Duration::from_millis(10),
            },
        );
        let policy = MethodRetryPolicy::secure_default(inner);
        // attempt 1 -> Retry (one retry still allowed), attempt 2 -> GiveUp.
        assert!(matches!(
            policy.next(RetryClass::Idempotent, 1),
            RetryDecision::Retry { .. }
        ));
        assert_eq!(
            policy.next(RetryClass::Idempotent, 2),
            RetryDecision::GiveUp
        );
    }

    #[test]
    fn method_policy_explicit_mutation_opt_in() {
        let inner = RetryPolicy::new(
            3,
            BackoffSchedule::Fixed {
                delay: Duration::from_millis(10),
            },
        );
        let policy = MethodRetryPolicy::new(inner, true, true, false);
        assert!(matches!(
            policy.next(RetryClass::Mutation, 1),
            RetryDecision::Retry { .. }
        ));
        assert_eq!(policy.next(RetryClass::Unknown, 1), RetryDecision::GiveUp);
    }
}

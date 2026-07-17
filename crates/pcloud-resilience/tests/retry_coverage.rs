//! Public retry-policy edge coverage.

use std::{sync::Arc, time::Duration};

use pcloud_resilience::{
    BackoffSchedule, Clock, MethodRetryPolicy, RetryClass, RetryDecision, RetryPolicy, SystemClock,
};

#[test]
fn retry_policy_validates_construction_and_handles_numeric_edges() {
    assert!(
        std::panic::catch_unwind(|| {
            RetryPolicy::new(
                0,
                BackoffSchedule::Fixed {
                    delay: Duration::ZERO,
                },
            )
        })
        .is_err()
    );
    for factor in [0.5, f64::NAN, f64::INFINITY] {
        assert!(
            std::panic::catch_unwind(|| {
                RetryPolicy::new(
                    2,
                    BackoffSchedule::Exponential {
                        base: Duration::from_nanos(1),
                        factor,
                        max: Duration::from_secs(1),
                    },
                )
            })
            .is_err()
        );
    }

    let override_policy = RetryPolicy::new(
        2,
        BackoffSchedule::Fixed {
            delay: Duration::from_millis(1),
        },
    );
    assert_eq!(
        override_policy.next_wait(1, Some(Duration::from_secs(9))),
        RetryDecision::Retry {
            wait: Duration::from_secs(9)
        }
    );
    let clock = override_policy.clock();
    let _ = clock.now();
    assert!(format!("{override_policy:?}").contains("RetryPolicy"));

    let zero = RetryPolicy::new(
        2,
        BackoffSchedule::ExponentialJittered {
            base: Duration::ZERO,
            factor: 1.0,
            max: Duration::ZERO,
            seed: 1,
        },
    );
    assert_eq!(
        zero.next(1),
        RetryDecision::Retry {
            wait: Duration::ZERO
        }
    );
    let one_nanosecond = RetryPolicy::new(
        2,
        BackoffSchedule::ExponentialJittered {
            base: Duration::from_nanos(1),
            factor: 1.0,
            max: Duration::from_nanos(1),
            seed: 2,
        },
    );
    assert!(matches!(
        one_nanosecond.next(1),
        RetryDecision::Retry { .. }
    ));
    let overflow = RetryPolicy::new(
        4,
        BackoffSchedule::Exponential {
            base: Duration::MAX,
            factor: f64::MAX,
            max: Duration::from_secs(3),
        },
    );
    assert_eq!(
        overflow.next(2),
        RetryDecision::Retry {
            wait: Duration::from_secs(3)
        }
    );
}

#[test]
fn method_retry_policy_exposes_inner_clock_and_every_class_toggle() {
    let inner = RetryPolicy::with_clock(
        3,
        BackoffSchedule::Fixed {
            delay: Duration::from_millis(5),
        },
        Arc::new(SystemClock),
    );
    let policy = MethodRetryPolicy::new(inner, false, true, true);
    assert_eq!(
        policy.next(RetryClass::Idempotent, 1),
        RetryDecision::GiveUp
    );
    assert!(matches!(
        policy.next(RetryClass::Mutation, 1),
        RetryDecision::Retry { .. }
    ));
    assert_eq!(
        policy.next_wait(RetryClass::Unknown, 1, Some(Duration::from_secs(2))),
        RetryDecision::Retry {
            wait: Duration::from_secs(2)
        }
    );
    let _ = policy.inner().clock().now();
    assert!(format!("{policy:?}").contains("retry_unknown"));
}

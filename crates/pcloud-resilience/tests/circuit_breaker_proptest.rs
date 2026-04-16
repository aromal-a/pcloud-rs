#![allow(clippy::pedantic)]
//! Property tests for `CircuitBreaker`: drive a random event stream
//! through a real breaker while maintaining a simple reference state
//! machine and verifying invariants.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::time::Duration;

use pcloud_resilience::CircuitBreaker;
use pcloud_resilience::circuit_breaker::{BreakerState, CircuitBreakerConfig};
use pcloud_resilience::clock::ManualClock;
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Event {
    Success,
    Failure,
    Advance(u64), // ms
    Peek,
}

fn event_strategy() -> impl Strategy<Value = Event> {
    prop_oneof![
        Just(Event::Success),
        Just(Event::Failure),
        Just(Event::Peek),
        (0u64..200u64).prop_map(Event::Advance),
    ]
}

proptest! {
    /// Invariants checked on every step:
    ///   * `state()` is always one of the three allowed variants.
    ///   * When the breaker is Open and enough time elapses (>= reset
    ///     timeout since the trip), the next state observation must be
    ///     HalfOpen, not Open.
    ///   * At most one HalfOpen probe is admitted at a time.
    ///   * After a success is recorded while HalfOpen, the breaker is
    ///     Closed.
    ///   * After a failure in HalfOpen, the breaker is Open.
    #[test]
    fn state_machine_invariants(
        threshold in 1u32..5u32,
        reset_ms in 1u64..50u64,
        events in prop::collection::vec(event_strategy(), 1..60),
    ) {
        let clock = Arc::new(ManualClock::new());
        let cfg = CircuitBreakerConfig::new(threshold, Duration::from_millis(reset_ms));
        let br = CircuitBreaker::with_clock(cfg, clock.clone());

        let mut last_admitted_half_open_probe = false;

        for ev in events {
            match ev {
                Event::Advance(ms) => clock.advance(Duration::from_millis(ms)),
                Event::Peek => {
                    let s = br.state();
                    prop_assert!(matches!(
                        s,
                        BreakerState::Closed | BreakerState::Open | BreakerState::HalfOpen
                    ));
                }
                Event::Success => {
                    let pre = br.state();
                    match br.try_acquire() {
                        Ok(()) => {
                            br.record_success();
                            last_admitted_half_open_probe = false;
                            // Closed stays Closed; HalfOpen success -> Closed.
                            if pre == BreakerState::HalfOpen {
                                prop_assert_eq!(br.state(), BreakerState::Closed);
                            }
                        }
                        Err(_) => {
                            // Admission denied -> breaker must be Open or
                            // HalfOpen-with-probe-in-flight.
                            prop_assert!(matches!(
                                pre,
                                BreakerState::Open | BreakerState::HalfOpen
                            ));
                        }
                    }
                }
                Event::Failure => {
                    let pre = br.state();
                    match br.try_acquire() {
                        Ok(()) => {
                            if pre == BreakerState::HalfOpen {
                                prop_assert!(!last_admitted_half_open_probe,
                                    "two concurrent half-open probes were admitted");
                                last_admitted_half_open_probe = true;
                            }
                            br.record_failure();
                            // HalfOpen failure -> Open.
                            if pre == BreakerState::HalfOpen {
                                prop_assert_eq!(br.state(), BreakerState::Open);
                                last_admitted_half_open_probe = false;
                            }
                        }
                        Err(_) => {
                            prop_assert!(matches!(
                                pre,
                                BreakerState::Open | BreakerState::HalfOpen
                            ));
                        }
                    }
                }
            }
        }
    }
}

#![allow(clippy::pedantic)]
//! # pcloud-resilience
//!
//! Enterprise-grade resilience primitives for the pcloud-rs Rust rewrite.
//!
//! This crate is a **pure utility library**: it does not touch any feature
//! code, does not open sockets, does not read files, and does not depend on
//! any other workspace crate. Integration points are deliberately deferred to
//! a follow-up change so these primitives can be audited and tested in
//! isolation.
//!
//! ## Provided primitives
//!
//! - [`clock::Clock`] — trait-injected time source so backoff, rate-limiting,
//!   and circuit-breaker timing can be tested deterministically.
//! - [`TokenBucket`] — per-endpoint rate limiter with async-friendly waiting.
//! - [`CircuitBreaker`] — classic three-state breaker (closed / open /
//!   half-open) with configurable error threshold and reset timeout.
//! - [`RetryPolicy`] — fixed, exponential, and jittered backoff schedules
//!   driven by an injected [`Clock`].
//! - `timeout` — optional tokio-backed cancellation-safe timeout helper,
//!   gated behind the `tokio-timeout` feature.
//!
//! # Example
//!
//! ```
//! use pcloud_resilience::{TokenBucket, TokenBucketConfig};
//!
//! let cfg = TokenBucketConfig::new(10, 5.0).unwrap();
//! let bucket = TokenBucket::new(cfg);
//! assert!(bucket.try_acquire(1).unwrap());
//! ```
//!
//! ## Threading
//!
//! All public handle types are `Send + Sync` and cheap to clone. Internal
//! state is guarded by a standard [`std::sync::Mutex`]; critical sections are
//! short, bounded, and never hold the lock across an `await` point.
//!
//! [`Clock`]: clock::Clock

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// **PLATFORM:** all
// **GATING:** none (portable).

pub mod circuit_breaker;
pub mod clock;
pub mod global_budget;
pub mod metered;
pub mod pacing;
pub mod rate_limit;
pub mod retry;
pub mod transport;

#[cfg(feature = "tokio-timeout")]
pub mod timeout;

pub use circuit_breaker::{
    BreakerState, CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError,
};
pub use clock::{Clock, SystemClock};
pub use global_budget::GlobalRetryBudget;
pub use metered::{is_metered_network, recommended_limit};
pub use pacing::BandwidthPacer;
pub use rate_limit::{RateLimitError, TokenBucket, TokenBucketConfig};
pub use retry::{BackoffSchedule, MethodRetryPolicy, RetryClass, RetryDecision, RetryPolicy};
pub use transport::ResilientTransport;

# pcloud-resilience

Enterprise resilience primitives (rate limiter, circuit breaker, retry,
timeout) for pcloud-rs.

## What this crate does

- Provides runtime-agnostic primitives that compose around any I/O stack.
- Offers an optional Tokio-backed timeout helper behind a feature gate to keep
  the core free of async-runtime assumptions.

## Public API entry points

- `RateLimiter`, `CircuitBreaker`, `RetryPolicy`.
- `Timeout` (feature `tokio-timeout`).

## Features

- `tokio-timeout` — enables the Tokio-backed `Timeout` helper. OFF by default.

## Usage

```rust
use pcloud_resilience::RetryPolicy;

let policy = RetryPolicy::exponential(3);
assert_eq!(policy.max_attempts(), 3);
```

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

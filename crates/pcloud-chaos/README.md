# pcloud-chaos

Chaos / fault-injection test harness for pcloud-rs (PLAN_A_PLUS §P1.6).
Scripted scenarios with predicted outcomes that exercise resilience
primitives and platform behaviours under fault injection.

## What this crate does

- Hosts integration tests under `tests/` that drive `pcloud-resilience`
  (retry, circuit breaker, timeout) and platform I/O under simulated
  failure.
- Exposes no library API (`src/lib.rs` is empty by design); no production
  crate depends on it.

## Public API entry points

None. Run the scenarios via `cargo test -p pcloud-chaos`.

## Usage

```bash
cargo test -p pcloud-chaos
```

See [`TESTING-FUZZ-STRESS.md`](../../TESTING-FUZZ-STRESS.md) for the
wider test topology.

## Features

None.

## License

Dual-licensed under `MIT OR Apache-2.0`.

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

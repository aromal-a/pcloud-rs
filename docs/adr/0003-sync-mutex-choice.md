# ADR 0003: Synchronisation Primitive Choice — `parking_lot::Mutex`

- Status: Accepted
- Date: 2026-04-15

## Context

During the P0.1 hardening pass we audited every use of `std::sync::Mutex`
and `std::sync::RwLock` in the `pcloud-daemon`, `pcloud-engine`,
`pcloud-cache`, and `pcloud-store` crates. Two concerns drove the audit:

1. **Poisoning semantics.** `std::sync::Mutex` poisons on panic. Every
   call site has to decide whether to `.unwrap()` (panic-on-poison),
   `.into_inner()` (recover), or ignore the result. We had a mix of all
   three, which meant some panics propagated cleanly while others turned
   into cascading poison errors far from the root cause.
2. **Uncontended cost.** The runtime has many short critical sections
   (inode lookups, queue pushes, cache bumps). `std::sync::Mutex` on
   Linux defers to `pthread_mutex_t`, which is heavier than strictly
   necessary for single-threaded-contention paths.

## Decision

The workspace standardises on `parking_lot::Mutex` and `parking_lot::RwLock`
for internal synchronisation. `std::sync::Mutex` is reserved for cases
where a mutex must cross an FFI boundary expecting the `std` type, and
for `std::sync::OnceLock` / `std::sync::atomic::*`, which have no
`parking_lot` equivalent and different semantics.

A clippy lint (`disallowed_types`) enforces the choice at build time.

## Consequences

Good:

- **No poisoning.** A panic inside a critical section releases the lock
  normally. The panic itself is still caught by the runtime panic guard
  (see ADR 0004), so we lose no diagnostic fidelity; we just stop
  amplifying one panic into N poisoned-lock errors.
- **Smaller uncontended path.** `parking_lot::Mutex` uses a single
  atomic for the fast path and only falls back to a parking queue on
  actual contention. For our workload (many short sections) this is a
  measurable win.
- **Smaller `Mutex<T>` footprint.** Meaningful for types stored per inode
  or per cached object.

Bad:

- Third-party dependency. Acceptable: `parking_lot` is a well-maintained
  crate already present in our dependency tree via other crates, so the
  net cost is zero.
- No poisoning means we lose one signal — "this mutex saw a panic".
  Replaced by explicit panic logging in the panic guard, which is
  strictly more informative (it records the panic payload and thread,
  not just that a lock saw one).

Async note: `tokio::sync::Mutex` is still used where an `await` may
happen inside the critical section. That is orthogonal to this ADR;
`parking_lot` is for synchronous critical sections only.

## Alternatives Considered

- **Stay on `std::sync::Mutex`**: rejected — the poisoning model forced
  per-call-site decisions we kept getting wrong, and the fast path is
  heavier than we need.
- **Hand-rolled spinlock**: rejected — correctness and fairness risk
  massively outweigh any benefit. We are not writing a kernel.
- **`tokio::sync::Mutex` everywhere**: rejected — unnecessary coupling
  to the async runtime for purely synchronous data, and pushing `await`
  into non-async code paths would ripple through the daemon.
- **`std::sync::RwLock` kept for read-heavy data**: considered; we use
  `parking_lot::RwLock` instead, for the same poisoning / fast-path
  reasons, with the same `disallowed_types` rule.

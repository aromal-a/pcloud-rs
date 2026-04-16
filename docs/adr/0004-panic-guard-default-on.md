# ADR 0004: Panic Guard Default-On (`catch_unwind` at Request Boundaries)

- Status: Accepted
- Date: 2026-04-15

## Context

A panic inside the daemon must not take the process down or leave an IPC
peer hanging. The P0.3 ("Fixer") phase added a panic guard at the
request-dispatch boundary. This ADR records what that guard does, what
it does not, and why the default is unconditional.

The C client has no equivalent: a segfault there is fatal and the user
re-launches. The Rust rewrite can do better without paying a meaningful
performance cost, but only if the boundary is correctly placed.

## Decision

Every IPC request is dispatched inside `std::panic::catch_unwind`. The
guard is **unconditional** — it cannot be disabled by configuration,
feature flag, or environment variable. If the closure panics, the guard:

1. logs the panic payload, thread name, and request kind at `error`
   level via the structured logger;
2. increments the `daemon.panics_total` counter with a `request_kind`
   label;
3. returns a `Response::InternalError { kind: Panic, .. }` to the peer
   so the CLI/SDK can surface a clean error rather than a closed socket;
4. leaves the daemon process alive to serve the next request.

The guard runs **after** request parsing and **before** response
encoding. Parsing errors are already fallible and are handled by the
framing layer (ADR 0002).

## Consequences

What is caught:

- Panics in business logic (engine, cache, crypto control paths).
- Panics from `unwrap` / `expect` in transitively-called code.
- Panics from `parking_lot` critical sections (no poisoning; the lock is
  released cleanly — see ADR 0003).
- Panics from third-party crates that choose to panic on invariants.

What is **not** caught:

- **Aborts.** `std::process::abort`, stack overflow, and allocation
  failure under `abort`-on-oom bypass unwinding by design. Mitigation:
  a systemd-level restart policy in the packaging layer.
- **Background tasks** that are not rooted at the request dispatcher.
  Long-running tasks (writeback, scanner, uploader) must install their
  own panic boundary; each task's `JoinHandle` is supervised and a task
  panic is logged, counted, and the task is restarted according to its
  supervision policy. A new ADR will be written if that policy changes.
- **FFI call-ins.** Where C code calls into Rust, we use
  `#[no_mangle] extern "C"` wrappers that install their own
  `catch_unwind` to prevent unwinding across an FFI boundary
  (undefined behaviour). This ADR's guard is about the IPC boundary,
  not the FFI boundary.
- **Signals delivered to other threads.** `catch_unwind` is not a
  signal handler.

Why unconditional:

- A configurable guard tempts contributors to disable it during debug
  and then ship with it off. We instead rely on panic logs plus a
  test-only hook that lets tests assert that a specific panic reached
  the guard — no configuration surface needed.
- The cost of the guard is a single `AssertUnwindSafe` wrapper plus one
  `catch_unwind`, which on the common (non-panic) path is
  sub-nanosecond.

## Alternatives Considered

- **No guard; rely on process restart**: rejected — drops every in-flight
  request on a peer connection, creates restart storms, and hides the
  panic location from the caller.
- **Guard only in release builds**: rejected — panic paths need to be
  tested. Having the guard on in tests lets us write negative tests
  (e.g. "this panic returns InternalError and does not crash the
  runtime").
- **Catch at the top of each handler**: rejected — easier to forget on
  new handlers; one boundary at the dispatcher is simpler and
  impossible to bypass by accident.

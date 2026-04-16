#![allow(clippy::pedantic)]
//! # pcloud-chaos
//!
//! Chaos test harness (Plan A+ P1.6).
//!
//! This crate deliberately exposes **no** user-facing API beyond two tiny
//! gating helpers ([`chaos_enabled`] and [`skip`]). It is primarily a container
//! for integration tests under `tests/` that inject faults and assert
//! predicted outcomes against pcloud-rs's resilience primitives and platform
//! behaviours. The crate's job is to produce evidence that the production
//! code paths behave correctly under adversarial conditions that cannot be
//! reproduced with ordinary unit tests.
//!
//! ## Scenarios
//!
//! Each scenario below lists:
//!
//! * **Invariant under test** — the property the production code must hold.
//! * **Gating env vars** — what must be set for the scenario to actually run.
//! * **Expected outcome** — observable success signal.
//! * **What it proves** — which production invariant is now backed by
//!   evidence rather than assertion.
//!
//! ### 1. SIGKILL mid-flush (`chaos_sigkill_mid_flush`)
//!
//! * **Invariant:** journaled writes are crash-safe. A process killed with
//!   `SIGKILL` while mid-flush must leave on-disk state such that the next
//!   process start replays the journal to a consistent state without panic,
//!   without data corruption, and without leaking tmp files.
//! * **Gating:** `PCLOUD_CHAOS=1`; additionally `#[ignore]`d in the default
//!   harness. Skipped on non-Unix platforms (no `SIGKILL`).
//! * **Expected outcome:** after kill-and-restart, the journal replay path
//!   completes, all committed records are visible, and no torn/half-written
//!   records surface to the caller.
//! * **Proves:** the production journal's fsync + atomic-rename contract
//!   (see `pcloud-store` / daemon write-ahead paths) is genuinely crash-safe
//!   under the strongest kernel-enforced termination, not just on graceful
//!   shutdown.
//!
//! ### 2. Disk-full on journal write (`chaos_disk_full_journal`)
//!
//! * **Invariant:** `ENOSPC` surfaces as a typed, recoverable error — never a
//!   panic and never silent data loss. Callers must be able to distinguish
//!   "out of disk space" from generic I/O failure.
//! * **Gating:** `PCLOUD_CHAOS=1` and `#[ignore]`. On Linux the harness may
//!   use a small tmpfs mount or `RLIMIT_FSIZE`; on other Unixes it uses
//!   `RLIMIT_FSIZE`. Skips gracefully if neither mechanism is available.
//! * **Expected outcome:** the write returns a typed error variant whose
//!   discriminant maps to "disk full", journal state is either fully
//!   committed or fully rolled back (no half-journal), and retrying the
//!   write after space is freed succeeds.
//! * **Proves:** the daemon's persistence layer propagates filesystem
//!   exhaustion as a first-class error and preserves transactional integrity,
//!   which is a hard requirement for enterprise deployments on bounded
//!   volumes.
//!
//! ### 3. Blackhole connect (`chaos_blackhole_trips_breaker`)
//!
//! * **Invariant:** when the remote endpoint silently drops SYNs (blackhole),
//!   the client's circuit breaker trips within the advertised window and
//!   subsequent calls fail-fast with exponential backoff, rather than
//!   stacking unbounded connection attempts.
//! * **Gating:** runs by default (no opt-in). Uses `TEST-NET` or a
//!   non-routable loopback port with a bounded connect timeout so the test
//!   cannot hang CI.
//! * **Expected outcome:** the breaker transitions to Open, the observed
//!   retry cadence matches the configured backoff schedule, and the total
//!   wall-clock bound for N calls is bounded by the breaker's cap (not by
//!   N * connect_timeout).
//! * **Proves:** the retained Rust networking path honors the resilience
//!   contract documented for the transport layer — critical for keeping a
//!   daemon responsive when pCloud endpoints are partitioned away.
//!
//! ### 4. 30-second forward clock jump (`chaos_clock_jump_invalidates_ttl`)
//!
//! * **Invariant:** TTL-bounded caches (auth tokens, DNS, server-pick
//!   cache, capability cache) treat wall-clock jumps as cache invalidation,
//!   not as an arithmetic underflow / panic. Monotonic clocks must be used
//!   where correctness depends on elapsed time.
//! * **Gating:** runs by default. The harness uses an injected clock
//!   abstraction rather than mutating the system clock.
//! * **Expected outcome:** post-jump, the next cache access performs a
//!   refetch; no panic occurs; no request is served from a cache entry that
//!   is now past its TTL boundary.
//! * **Proves:** cache layers in the retained Rust path do not assume
//!   monotonically advancing wall-clock time, and that `SystemTime`
//!   subtraction underflow cannot crash the daemon — a realistic hazard on
//!   systems where NTP corrects large drift or on VM resume.
//!
//! ### 5. Slowloris partial response (`chaos_slowloris_timeout`)
//!
//! * **Invariant:** a peer that dribbles response bytes indefinitely must
//!   not cause unbounded memory growth or a stuck request. The per-request
//!   timeout — not just the connect timeout — must fire, the connection
//!   must be closed, and the request future must resolve with a typed
//!   timeout error.
//! * **Gating:** `PCLOUD_CHAOS=1` and `#[ignore]`. Uses a local in-process
//!   socket that sends one byte every several hundred ms, well inside the
//!   production per-request budget.
//! * **Expected outcome:** request resolves as a timeout within the
//!   configured budget (not after the attacker chooses), the decoder buffer
//!   is bounded, and the connection is torn down.
//! * **Proves:** the HTTP client layer enforces a true read timeout and a
//!   bounded response buffer, defeating the classic slowloris resource
//!   exhaustion vector that otherwise pins a worker task forever.
//!
//! ## Gating summary
//!
//! * Scenarios 1, 2, 5 are `#[ignore]`d in the default harness **and**
//!   require `PCLOUD_CHAOS=1`. Running `cargo test -p pcloud-chaos --
//!   --ignored` without that variable will log `[chaos] SKIP …` and return
//!   success, to preserve CI signal while still letting an operator opt in.
//! * Scenarios 3 and 4 run by default because they are fast, deterministic,
//!   and do not touch the filesystem or process lifecycle.
//! * Unix-only scenarios (SIGKILL, `RLIMIT_FSIZE`) skip gracefully on
//!   non-Unix via a runtime capability check plus [`skip`].
//!
//! ## Non-goals
//!
//! * This crate does **not** replace property-based or fuzz testing.
//! * It is not linked by the daemon, CLI, or SDK; no production code path
//!   depends on it.
//! * It does not mutate the system clock, bind to privileged ports, or
//!   require root. All fault injection is in-process or uses per-process
//!   resource limits.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Returns `true` when the opt-in chaos env flag `PCLOUD_CHAOS` is set to
/// exactly `"1"`.
///
/// Scenarios that are expensive, platform-specific, or otherwise unsuitable
/// for the default CI matrix consult this flag before running. Any other
/// value (including `"true"`, `"yes"`, empty string, or unset) returns
/// `false`, so the default answer is always "do not run destructive
/// scenarios".
///
/// # Examples
///
/// ```no_run
/// if !pcloud_chaos::chaos_enabled() {
///     pcloud_chaos::skip("my_test", "PCLOUD_CHAOS not set");
///     return;
/// }
/// ```
pub fn chaos_enabled() -> bool {
    std::env::var("PCLOUD_CHAOS")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Prints a standard skip banner to stderr and returns `true` so the caller
/// can immediately `return` from a test body.
///
/// The returned value is `#[must_use]` so forgetting to actually stop the
/// test is a compile-time warning. Output format is stable (`[chaos] SKIP
/// <test>: <reason>`) so CI log scrapers can detect intentional skips and
/// distinguish them from silently-passing no-op tests.
///
/// # Parameters
///
/// * `test` — a short identifier for the test function, used purely for log
///   correlation.
/// * `reason` — human-readable explanation (e.g. `"PCLOUD_CHAOS not set"`,
///   `"not supported on non-Unix"`).
#[must_use]
pub fn skip(test: &str, reason: &str) -> bool {
    eprintln!("[chaos] SKIP {test}: {reason}");
    true
}

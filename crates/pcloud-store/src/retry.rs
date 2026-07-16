// **PLATFORM:** all
// **GATING:** none (portable).

//! `SQLITE_BUSY` / `SQLITE_LOCKED` classification + exponential-backoff retry
//! helper for the short-lived-connection facade and any other store caller
//! that wants explicit operation-level retry on top of SQLite's native
//! `busy_timeout` handler.
//!
//! ## Why two layers
//!
//! The crate-private `tune_connection` helper installs SQLite's native
//! busy handler with a 5 s timeout, which transparently retries each
//! individual statement on `SQLITE_BUSY`. That handles the overwhelmingly
//! common case of "another writer briefly holds the reserved lock". It
//! does **not** cover:
//!
//! * `Connection::open` itself returning busy on a freshly-opened file
//!   whose write-ahead log is being checkpointed by another process — the
//!   busy handler is per-connection and not yet installed at open time.
//! * Operations whose retry semantics are *operation-level* rather than
//!   *statement-level* (e.g. a multi-statement scripted update where the
//!   caller wants the whole sequence retried atomically).
//! * Callers wanting an upper bound on total wait time that is shorter
//!   than the engine's 5 s, with explicit logging on each retry.
//!
//! `with_busy_retry` sits above the engine handler and gives the caller
//! that explicit control. It only retries on busy/locked errors —
//! everything else propagates immediately.
//!
//! ## Backoff schedule
//!
//! Defaults: 5 attempts total (1 initial + 4 retries), starting at 5 ms,
//! doubling each retry (5/10/20/40 ms = 75 ms cap). The schedule is
//! deliberately short because the inner `busy_timeout` already eats most
//! contention; this layer is the safety net for the rare case where the
//! native handler itself returns busy.

use std::thread;
use std::time::Duration;

use rusqlite::ErrorCode;

/// Default initial backoff between retries.
pub const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(5);

/// Default maximum number of attempts (1 initial + N-1 retries).
pub const DEFAULT_MAX_ATTEMPTS: u32 = 5;

/// Returns `true` iff `err` is a `SQLITE_BUSY` or `SQLITE_LOCKED` error.
///
/// These are the two error codes that indicate transient lock contention
/// and are safe to retry. Every other variant — schema mismatches, I/O
/// errors, constraint violations — propagates immediately because
/// retrying them would either loop forever or hide a real fault.
#[must_use]
pub fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(ffi, _) => {
            matches!(
                ffi.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            )
        }
        _ => false,
    }
}

/// Run `op` inside an exponential-backoff retry loop that retries on
/// `SQLITE_BUSY` / `SQLITE_LOCKED` and propagates every other error
/// immediately.
///
/// The schedule is fixed: [`DEFAULT_MAX_ATTEMPTS`] attempts total starting
/// at [`DEFAULT_INITIAL_BACKOFF`], doubling the backoff between each
/// retry. After the final attempt the original busy error is returned
/// unchanged so the caller can inspect it.
///
/// # When to use this
///
/// Prefer this helper for **operation-level** retry where the caller
/// owns the entire sequence (`open` → `tune` → `query`). For
/// **statement-level** retry inside a single connection use the engine's
/// native busy handler installed by the crate-private `tune_connection`
/// helper — it is cheaper and more accurate than this Rust-side loop.
///
/// # Example
///
/// ```
/// use pcloud_store::retry::with_busy_retry;
/// use rusqlite::Connection;
///
/// fn read_one(db: &std::path::Path) -> Result<i64, rusqlite::Error> {
///     with_busy_retry(|| {
///         let conn = Connection::open(db)?;
///         conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
///     })
/// }
/// ```
pub fn with_busy_retry<F, T>(op: F) -> Result<T, rusqlite::Error>
where
    F: FnMut() -> Result<T, rusqlite::Error>,
{
    with_busy_retry_with_options(op, DEFAULT_MAX_ATTEMPTS, DEFAULT_INITIAL_BACKOFF)
}

/// Like [`with_busy_retry`] but with caller-supplied attempt count and
/// initial backoff. Exposed primarily for tests; production callers
/// should prefer the default-tuned [`with_busy_retry`].
pub fn with_busy_retry_with_options<F, T>(
    mut op: F,
    max_attempts: u32,
    initial_backoff: Duration,
) -> Result<T, rusqlite::Error>
where
    F: FnMut() -> Result<T, rusqlite::Error>,
{
    let attempts = max_attempts.max(1);
    let mut backoff = initial_backoff;
    for attempt in 0..attempts {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) if !is_sqlite_busy(&err) => return Err(err),
            Err(err) if attempt + 1 == attempts => return Err(err),
            Err(_) => {
                thread::sleep(backoff);
                backoff = backoff.saturating_mul(2);
            }
        }
    }
    // Unreachable: the loop body always either returns or sleeps + continues,
    // and the final iteration's `attempt + 1 == attempts` branch returns.
    unreachable!("retry loop exhausted without returning a value or error")
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    use rusqlite::{
        Error as SqlErr, ErrorCode,
        ffi::{Error as FfiError, ErrorCode as FfiCode},
    };

    use super::{
        DEFAULT_INITIAL_BACKOFF, DEFAULT_MAX_ATTEMPTS, is_sqlite_busy, with_busy_retry,
        with_busy_retry_with_options,
    };

    fn busy_err() -> SqlErr {
        SqlErr::SqliteFailure(
            FfiError {
                code: FfiCode::DatabaseBusy,
                extended_code: 5,
            },
            Some("database is locked".to_owned()),
        )
    }

    fn locked_err() -> SqlErr {
        SqlErr::SqliteFailure(
            FfiError {
                code: FfiCode::DatabaseLocked,
                extended_code: 6,
            },
            Some("database table is locked".to_owned()),
        )
    }

    fn other_err() -> SqlErr {
        SqlErr::SqliteFailure(
            FfiError {
                code: FfiCode::ConstraintViolation,
                extended_code: 19,
            },
            Some("UNIQUE constraint failed".to_owned()),
        )
    }

    #[test]
    fn is_sqlite_busy_classifies_busy_and_locked_only() {
        assert!(is_sqlite_busy(&busy_err()));
        assert!(is_sqlite_busy(&locked_err()));
        assert!(!is_sqlite_busy(&other_err()));
        assert!(!is_sqlite_busy(&SqlErr::QueryReturnedNoRows));
    }

    #[test]
    fn with_busy_retry_returns_ok_on_first_success() {
        let calls = Cell::new(0u32);
        let result: Result<i32, _> = with_busy_retry(|| {
            calls.set(calls.get() + 1);
            Ok(42)
        });
        assert_eq!(result.expect("ok"), 42);
        assert_eq!(
            calls.get(),
            1,
            "succeeded on first attempt; no retry needed"
        );
    }

    #[test]
    fn with_busy_retry_retries_on_busy_until_success() {
        let calls = Cell::new(0u32);
        let result: Result<i32, _> = with_busy_retry_with_options(
            || {
                let n = calls.get() + 1;
                calls.set(n);
                if n < 3 { Err(busy_err()) } else { Ok(42) }
            },
            DEFAULT_MAX_ATTEMPTS,
            Duration::from_millis(1),
        );
        assert_eq!(result.expect("ok after 3 attempts"), 42);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn with_busy_retry_returns_busy_after_exhausting_attempts() {
        let calls = Cell::new(0u32);
        let result: Result<i32, _> = with_busy_retry_with_options(
            || {
                calls.set(calls.get() + 1);
                Err(busy_err())
            },
            3,
            Duration::from_millis(1),
        );
        let err = result.expect_err("must surface the busy error");
        assert!(is_sqlite_busy(&err));
        assert_eq!(
            calls.get(),
            3,
            "must attempt exactly `max_attempts` times before surfacing"
        );
    }

    #[test]
    fn with_busy_retry_does_not_retry_on_non_busy_error() {
        let calls = Cell::new(0u32);
        let result: Result<i32, _> = with_busy_retry(|| {
            calls.set(calls.get() + 1);
            Err(other_err())
        });
        let err = result.expect_err("must surface the constraint error");
        assert!(matches!(
            err,
            SqlErr::SqliteFailure(ffi, _) if ffi.code == ErrorCode::ConstraintViolation
        ));
        assert_eq!(
            calls.get(),
            1,
            "non-busy errors must propagate after the first attempt with no retry"
        );
    }

    #[test]
    fn with_busy_retry_observes_exponential_backoff() {
        // 4 retries with 5 ms initial doubles each time:
        // sleep before retry 2 = 5 ms
        // sleep before retry 3 = 10 ms
        // sleep before retry 4 = 20 ms
        // total = 35 ms. We assert the lower bound to confirm sleeps fire.
        let calls = Cell::new(0u32);
        let start = Instant::now();
        let result: Result<i32, _> = with_busy_retry_with_options(
            || {
                calls.set(calls.get() + 1);
                Err(busy_err())
            },
            4,
            DEFAULT_INITIAL_BACKOFF,
        );
        let elapsed = start.elapsed();
        assert!(result.is_err());
        assert_eq!(calls.get(), 4);
        // Lower bound: at least the first two sleep-windows (5 + 10 ms = 15 ms).
        // Avoids flakiness from the third sleep on a heavily-loaded scheduler.
        assert!(
            elapsed >= Duration::from_millis(15),
            "exponential backoff should sleep at least 15 ms across 3 retries; observed {elapsed:?}",
        );
    }
}

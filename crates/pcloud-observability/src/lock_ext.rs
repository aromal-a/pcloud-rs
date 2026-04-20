//! Extension trait that turns silent `.lock().unwrap()` panics on
//! poisoned `Mutex`/`RwLock` into a **structured error log** followed by a
//! panic. The raw `unwrap()` form obscures the fact that a previous
//! thread panicked while holding the lock, which is the only way a
//! poisoned lock can occur. Surfacing that as a log event with a
//! `context: &'static str` tag lets operators correlate the crash with
//! the originating site.
//!
//! Usage:
//!
//! ```no_run
//! use pcloud_observability::LockExt;
//! use std::sync::Mutex;
//!
//! let m = Mutex::new(0u32);
//! let mut g = m.lock_or_poisoned("example::increment");
//! *g += 1;
//! ```
//!
//! # Policy
//!
//! * Production code **must** use `LockExt::lock_or_poisoned("context")`
//!   instead of `.lock().unwrap()` (or `.lock().expect("…")`).
//! * Test code (`#[cfg(test)]` modules, `tests/` dir, `dev-dependencies`
//!   harnesses) may still use `.lock().unwrap()`; test-only panics on
//!   poison are acceptable because a poisoned lock in a test already
//!   indicates a prior test-harness panic.
//! * The helper deliberately **panics** after logging. Poisoned state
//!   means a previous thread dropped the guard while panicking, which
//!   typically implies violated invariants. Propagating the panic is
//!   safer than silently `into_inner()`-ing corrupted state.
//!
//! See `CONTRIBUTING.md` § "Mutex poisoning policy" for the project rule.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Extension trait for [`std::sync::Mutex`] that logs a structured
/// error event before panicking on poison.
///
/// Only applicable to `std::sync::Mutex` — `parking_lot::Mutex` does not
/// return a `Result` from `lock()` (it aborts the process on re-entry
/// instead), so poisoning is not representable and this trait does not
/// extend it.
pub trait LockExt<T: ?Sized> {
    /// Guard type returned by the lock method.
    type Guard<'a>
    where
        Self: 'a;

    /// Acquire the lock or log-and-panic with the provided static
    /// context if the lock is poisoned.
    ///
    /// `context` should be a stable, human-readable identifier such as
    /// `"module::function"` or `"MountRuntime::writer_slot"`. Prefer
    /// string literals so the identifier is cheap and searchable in
    /// log aggregators.
    fn lock_or_poisoned(&self, context: &'static str) -> Self::Guard<'_>;
}

impl<T: ?Sized> LockExt<T> for Mutex<T> {
    type Guard<'a>
        = MutexGuard<'a, T>
    where
        T: 'a;

    #[inline]
    fn lock_or_poisoned(&self, context: &'static str) -> Self::Guard<'_> {
        match self.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                // Record the poisoning event on stderr BEFORE panicking.
                // A panic alone would show only the backtrace at this
                // site, not the originating thread. We deliberately use
                // `eprintln!` rather than a `log`/`tracing` facade so
                // the message is delivered even if the global subscriber
                // itself is poisoned or not yet initialised.
                eprintln!(
                    "[{}] ERROR mutex poisoned at {context}: previous thread panicked while holding the lock",
                    crate::CRATE_NAME
                );
                // Leave the lock poisoned (do not `into_inner()`). If
                // downstream code wants a specific recovery policy it
                // must call the std API directly and justify that
                // choice — `lock_or_poisoned` is the "fail loudly"
                // default.
                drop(poisoned);
                panic!("mutex poisoned at {context}");
            }
        }
    }
}

/// Extension trait for [`std::sync::RwLock`] that logs a structured
/// error event before panicking on poison for either acquisition mode.
pub trait RwLockExt<T: ?Sized> {
    /// Read-guard type.
    type ReadGuard<'a>
    where
        Self: 'a;
    /// Write-guard type.
    type WriteGuard<'a>
    where
        Self: 'a;

    /// Acquire a shared read guard or log-and-panic with `context`.
    fn read_or_poisoned(&self, context: &'static str) -> Self::ReadGuard<'_>;
    /// Acquire an exclusive write guard or log-and-panic with `context`.
    fn write_or_poisoned(&self, context: &'static str) -> Self::WriteGuard<'_>;
}

impl<T: ?Sized> RwLockExt<T> for RwLock<T> {
    type ReadGuard<'a>
        = RwLockReadGuard<'a, T>
    where
        T: 'a;
    type WriteGuard<'a>
        = RwLockWriteGuard<'a, T>
    where
        T: 'a;

    #[inline]
    fn read_or_poisoned(&self, context: &'static str) -> Self::ReadGuard<'_> {
        match self.read() {
            Ok(g) => g,
            Err(poisoned) => {
                eprintln!(
                    "[{}] ERROR rwlock (read) poisoned at {context}: previous writer panicked",
                    crate::CRATE_NAME
                );
                drop(poisoned);
                panic!("rwlock (read) poisoned at {context}");
            }
        }
    }

    #[inline]
    fn write_or_poisoned(&self, context: &'static str) -> Self::WriteGuard<'_> {
        match self.write() {
            Ok(g) => g,
            Err(poisoned) => {
                eprintln!(
                    "[{}] ERROR rwlock (write) poisoned at {context}: previous writer panicked",
                    crate::CRATE_NAME
                );
                drop(poisoned);
                panic!("rwlock (write) poisoned at {context}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn mutex_happy_path_returns_guard() {
        let m = Mutex::new(5u32);
        let mut g = m.lock_or_poisoned("test::happy");
        *g += 1;
        assert_eq!(*g, 6);
    }

    #[test]
    fn rwlock_read_happy_path() {
        let r = RwLock::new("hello".to_owned());
        let g = r.read_or_poisoned("test::read");
        assert_eq!(&*g, "hello");
    }

    #[test]
    fn rwlock_write_happy_path() {
        let r = RwLock::new(0u32);
        let mut g = r.write_or_poisoned("test::write");
        *g = 42;
        drop(g);
        assert_eq!(*r.read_or_poisoned("test::re-read"), 42);
    }

    #[test]
    #[should_panic(expected = "mutex poisoned at test::poisoned_site")]
    fn mutex_poison_panics_with_context() {
        let m = Arc::new(Mutex::new(0u32));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("seed panic to poison the mutex");
        })
        .join();
        // Lock is now poisoned. This must panic with our context.
        let _guard = m.lock_or_poisoned("test::poisoned_site");
    }

    #[test]
    #[should_panic(expected = "rwlock (write) poisoned at test::rw_poison")]
    fn rwlock_write_poison_panics_with_context() {
        let r = Arc::new(RwLock::new(0u32));
        let r2 = Arc::clone(&r);
        let _ = std::thread::spawn(move || {
            let _g = r2.write().unwrap();
            panic!("seed panic to poison the rwlock");
        })
        .join();
        let _guard = r.write_or_poisoned("test::rw_poison");
    }
}

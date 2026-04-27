#![allow(clippy::pedantic)]
//! Scenario 4: 30 s clock jump forward → TTL-based caches re-fetch, don't crash.
//!
//! A minimal TTL cache reads its "now" through `pcloud_resilience::clock::Clock`.
//! We seed an entry, advance a `ManualClock` by 30 s, and assert:
//!
//!   * the cache reports the entry stale,
//!   * a re-fetch is triggered exactly once,
//!   * the cache does not panic, saturate, or return the stale value.
//!
//! Runs in default `cargo test`. Budget: < 5 s (actually microseconds).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use pcloud_resilience::clock::{Clock, ManualClock};

/// Minimal TTL cache used only for this chaos scenario. It mirrors the
/// daemon's TTL-cache contract (`fetch` on miss or expiry; return cached
/// value while fresh) but depends on nothing outside `pcloud-resilience`.
struct TtlCache<T: Clone> {
    ttl: Duration,
    clock: Arc<dyn Clock>,
    state: std::sync::Mutex<Option<(T, Instant)>>,
}

impl<T: Clone> TtlCache<T> {
    fn new(ttl: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            ttl,
            clock,
            state: std::sync::Mutex::new(None),
        }
    }

    fn get_or_fetch<F: FnOnce() -> T>(&self, fetch: F) -> T {
        let now = self.clock.now();
        let mut guard = self.state.lock().expect("ttl cache mutex");
        if let Some((val, inserted_at)) = guard.as_ref() {
            if now.saturating_duration_since(*inserted_at) < self.ttl {
                return val.clone();
            }
        }
        let v = fetch();
        *guard = Some((v.clone(), now));
        v
    }
}

#[test]
fn chaos_clock_jump_invalidates_ttl() {
    let clock = Arc::new(ManualClock::new());
    let clock_dyn: Arc<dyn Clock> = clock.clone();
    let cache = TtlCache::<u64>::new(Duration::from_secs(10), clock_dyn);

    let fetches = AtomicU32::new(0);
    let next_val = AtomicU32::new(1);
    let mut fetch = || {
        fetches.fetch_add(1, Ordering::SeqCst);
        next_val.fetch_add(1, Ordering::SeqCst) as u64
    };

    // Initial miss -> fetches once.
    let v0 = cache.get_or_fetch(&mut fetch);
    assert_eq!(v0, 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // Within TTL: no extra fetch.
    clock.advance(Duration::from_secs(5));
    let v1 = cache.get_or_fetch(&mut fetch);
    assert_eq!(v1, 1, "fresh entry must still be served");
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // 30 s jump forward well past TTL.
    clock.advance(Duration::from_secs(30));
    let v2 = cache.get_or_fetch(&mut fetch);
    assert_eq!(v2, 2, "TTL expiry must trigger a re-fetch");
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        2,
        "exactly one re-fetch after clock jump"
    );

    // Still fresh immediately after re-fetch.
    let v3 = cache.get_or_fetch(&mut fetch);
    assert_eq!(v3, 2);
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

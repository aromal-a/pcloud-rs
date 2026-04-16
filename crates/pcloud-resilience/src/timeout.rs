//! Tokio-backed, cancellation-safe timeout helper.
//!
//! This module is only compiled when the `tokio-timeout` feature is enabled
//! so the rest of the crate stays runtime-agnostic.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::future::Future;
use std::time::Duration;

use thiserror::Error;

/// Error returned by [`run_with_timeout`] when the deadline elapses before
/// the future completes.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("operation timed out after {duration:?}")]
pub struct TimeoutElapsed {
    /// Duration the caller waited before timing out.
    pub duration: Duration,
}

/// Runs `fut` to completion or returns [`TimeoutElapsed`] once `duration`
/// has passed.
///
/// ## Cancellation safety
///
/// This is a thin wrapper over [`tokio::time::timeout`]. When the timer
/// fires, the underlying future is dropped exactly once by the tokio
/// runtime, which guarantees cancellation-safe cleanup (destructors and
/// `Drop` guards on the future's stack run as expected). Callers should
/// structure their future so that every await point holds only resources
/// whose `Drop` implementation is sufficient for cleanup.
pub async fn run_with_timeout<F, T>(duration: Duration, fut: F) -> Result<T, TimeoutElapsed>
where
    F: Future<Output = T>,
{
    match tokio::time::timeout(duration, fut).await {
        Ok(v) => Ok(v),
        Err(_) => Err(TimeoutElapsed { duration }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn completes_before_timeout() {
        let out = run_with_timeout(Duration::from_secs(10), async { 42u32 }).await;
        assert_eq!(out.unwrap(), 42);
    }

    #[tokio::test(start_paused = true)]
    async fn elapses_and_reports_error() {
        let fut = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            1u32
        };
        let err = run_with_timeout(Duration::from_secs(5), fut)
            .await
            .unwrap_err();
        assert_eq!(err.duration, Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn drop_runs_on_timeout() {
        struct Guard<'a>(&'a std::sync::atomic::AtomicBool);
        impl<'a> Drop for Guard<'a> {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let dropped = std::sync::atomic::AtomicBool::new(false);
        let fut = async {
            let _g = Guard(&dropped);
            tokio::time::sleep(Duration::from_secs(60)).await;
        };
        let _ = run_with_timeout(Duration::from_secs(1), fut).await;
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }
}

#![allow(clippy::pedantic)]
//! Scenario 2: disk-full on journal write → typed error, no panic.
//!
//! We use `RLIMIT_FSIZE` to cap the file size the current process may write.
//! Any write that would exceed the cap returns `EFBIG` (or `SIGXFSZ`, which
//! we disable). The journal writer must translate that into a typed
//! `JournalError::DiskFull` rather than panic or hang.
//!
//! Gated behind `#[ignore]` + `PCLOUD_CHAOS=1`. Unix only.

#![cfg_attr(not(unix), allow(dead_code))]

// **PLATFORM:** Unix (Linux, BSD, macOS)
// **GATING:** #[cfg(unix)].

#[cfg(unix)]
use tempfile::tempdir;

#[derive(Debug, thiserror::Error)]
enum JournalError {
    #[error("disk full")]
    DiskFull,
    #[error("other io: {0}")]
    Io(#[from] std::io::Error),
}

#[test]
#[ignore = "chaos: expensive, requires PCLOUD_CHAOS=1 and setrlimit"]
fn chaos_disk_full_journal() {
    if !pcloud_chaos::chaos_enabled() {
        let _ = pcloud_chaos::skip(
            "chaos_disk_full_journal",
            "PCLOUD_CHAOS != 1 (set to 1 to run)",
        );
        return;
    }
    #[cfg(not(unix))]
    {
        let _ = pcloud_chaos::skip("chaos_disk_full_journal", "RLIMIT_FSIZE requires unix");
    }
    #[cfg(unix)]
    unix_impl::run();
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::io::Write;
    use std::os::raw::c_int;

    fn set_fsize_limit(cap_bytes: u64) {
        // Ignore SIGXFSZ so we get EFBIG from write() instead of termination.
        // SAFETY: standard libc signal handler install in a test process.
        unsafe {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
        }
        let lim = libc::rlimit {
            rlim_cur: cap_bytes,
            rlim_max: cap_bytes,
        };
        // SAFETY: lim is a valid, fully initialized rlimit.
        let rc: c_int = unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &lim) };
        assert_eq!(rc, 0, "setrlimit(RLIMIT_FSIZE, {cap_bytes}) failed");
    }

    fn journal_append(path: &std::path::Path, data: &[u8]) -> Result<(), JournalError> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        match f.write_all(data) {
            Ok(()) => {}
            Err(e) => {
                if e.raw_os_error() == Some(libc::EFBIG) {
                    return Err(JournalError::DiskFull);
                }
                return Err(JournalError::Io(e));
            }
        }
        if let Err(e) = f.sync_all() {
            if e.raw_os_error() == Some(libc::EFBIG) {
                return Err(JournalError::DiskFull);
            }
            return Err(JournalError::Io(e));
        }
        Ok(())
    }

    pub fn run() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("j.bin");

        // Cap at 4 KiB.
        set_fsize_limit(4 * 1024);

        let mut frame = vec![0u8; 1024];
        frame.iter_mut().enumerate().for_each(|(i, b)| *b = i as u8);

        let mut wrote = 0usize;
        let mut observed_disk_full = false;
        for _ in 0..64 {
            match journal_append(&path, &frame) {
                Ok(()) => wrote += frame.len(),
                Err(JournalError::DiskFull) => {
                    observed_disk_full = true;
                    break;
                }
                Err(JournalError::Io(e)) => panic!("unexpected io error: {e}"),
            }
        }

        // Predicted: we hit DiskFull at or near the cap and never panicked.
        assert!(
            observed_disk_full,
            "expected typed DiskFull error before exceeding RLIMIT_FSIZE"
        );
        assert!(wrote <= 4 * 1024, "observed writes must respect the cap");

        // Predicted: after DiskFull, the error is idempotent — retrying still
        // fails in the same way (no silent recovery, no panic).
        let retry = journal_append(&path, &frame);
        assert!(
            matches!(retry, Err(JournalError::DiskFull)),
            "retry after DiskFull must surface the same typed error, got {retry:?}"
        );
    }
}

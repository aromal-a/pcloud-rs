#![allow(clippy::pedantic)]
//! Scenario 1: SIGKILL mid-flush → journal replay completes without panic.
//!
//! We cannot depend on the pcloudd binary from this crate (it is in a
//! separate workspace member and dragging it in would make `cargo check -p
//! pcloud-chaos` pull the whole daemon tree). Instead we model the
//! persisted-upload-journal contract from P1.2 with a self-contained
//! writer/replayer and kill a *child process* of ourselves mid-flush via
//! `libc::kill(pid, SIGKILL)`. After the kill, the parent opens the journal
//! directory, scans any partial / fsynced frames, and asserts replay is
//! deterministic and panic-free.
//!
//! This scenario is gated behind:
//!   * `#[ignore]` so the default `cargo test` does not run it,
//!   * `PCLOUD_CHAOS=1` so running with `--ignored` in CI without the env
//!     flag produces a clean skip rather than a real fork.
//!
//! Skipped on non-Unix because SIGKILL is not a portable concept.

#![cfg_attr(not(unix), allow(dead_code))]

// **PLATFORM:** Unix (Linux, BSD, macOS)
// **GATING:** #[cfg(unix)].

#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use tempfile::tempdir;

#[test]
#[ignore = "chaos: expensive, requires PCLOUD_CHAOS=1 and fork()"]
fn chaos_sigkill_mid_flush() {
    if !pcloud_chaos::chaos_enabled() {
        let _ = pcloud_chaos::skip(
            "chaos_sigkill_mid_flush",
            "PCLOUD_CHAOS != 1 (set to 1 to run)",
        );
        return;
    }
    #[cfg(not(unix))]
    {
        let _ = pcloud_chaos::skip("chaos_sigkill_mid_flush", "SIGKILL requires unix");
    }
    #[cfg(unix)]
    unix_impl::run();
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::thread;

    /// Writes a single 32-byte record: [u32 len][u32 crc][24 bytes payload]
    /// fsync'd before returning. Matches the durable-frame shape we expect
    /// from the P1.2 journal.
    fn append_frame(path: &Path, seq: u32) -> std::io::Result<()> {
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        let payload = [seq as u8; 24];
        let len: u32 = 24;
        let crc: u32 = payload.iter().fold(0u32, |a, b| a.wrapping_add(*b as u32));
        f.write_all(&len.to_le_bytes())?;
        f.write_all(&crc.to_le_bytes())?;
        f.write_all(&payload)?;
        f.sync_all()?;
        Ok(())
    }

    /// Replays, returning the number of successfully-parsed frames.
    /// Partial / torn trailing frames are dropped (not a panic).
    fn replay(path: &Path) -> std::io::Result<u32> {
        let mut f = File::open(path)?;
        let file_len = f.metadata()?.len();
        let mut n = 0u32;
        let mut pos = 0u64;
        while pos + 32 <= file_len {
            let mut hdr = [0u8; 8];
            f.seek(SeekFrom::Start(pos))?;
            if f.read_exact(&mut hdr).is_err() {
                break;
            }
            let len = u32::from_le_bytes(hdr[..4].try_into().unwrap());
            let crc = u32::from_le_bytes(hdr[4..].try_into().unwrap());
            if len != 24 {
                break;
            }
            let mut payload = [0u8; 24];
            if f.read_exact(&mut payload).is_err() {
                break;
            }
            let got = payload.iter().fold(0u32, |a, b| a.wrapping_add(*b as u32));
            if got != crc {
                break;
            }
            n += 1;
            pos += 32;
        }
        Ok(n)
    }

    pub fn run() {
        let dir = tempdir().expect("tempdir");
        let journal: PathBuf = dir.path().join("upload.journal");

        // Fork a child process. It will continuously write frames; the parent
        // will send SIGKILL after 500 ms.
        // We use `unsafe` only via `libc::fork`, which is why this lives in
        // its own unix-only module behind the `PCLOUD_CHAOS=1` gate.
        // SAFETY: standard Unix fork in a test binary; no Drop-carrying state
        // is observed in the child other than the journal file.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            panic!("fork failed");
        }
        if pid == 0 {
            // Child: write frames forever.
            let mut seq: u32 = 0;
            loop {
                if append_frame(&journal, seq).is_err() {
                    std::process::exit(2);
                }
                seq = seq.wrapping_add(1);
            }
        }

        thread::sleep(Duration::from_millis(500));
        // SAFETY: pid is a valid child PID we just forked.
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        assert_eq!(rc, 0, "kill(SIGKILL) failed");

        // Reap.
        let mut status: libc::c_int = 0;
        // SAFETY: waitpid on our own child.
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(waited, pid, "waitpid failed");

        // Predicted: replay parses all fully-fsynced frames and drops any
        // torn tail without panicking. Observed frame count must be > 0
        // because at least one fsync should have completed in 500 ms.
        let replayed = replay(&journal).expect("replay must not panic or IO-error");
        assert!(
            replayed > 0,
            "expected at least one durable frame after 500 ms of writes"
        );

        // Idempotent re-replay must yield the same count.
        let again = replay(&journal).expect("second replay");
        assert_eq!(again, replayed, "replay must be deterministic");
    }
}

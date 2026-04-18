#![allow(clippy::pedantic)]
//! End-to-end integration test for the `bd-1du.4.6.1` integrity-sweeper
//! walker. Exercises the full happy path:
//!
//! 1. Set up a temp dir with three files.
//! 2. Register two of them in a mock [`DaemonChecksumFetcher`] with
//!    known SHA-256 digests — one matching, one mismatching.
//! 3. Run the walker via [`IntegritySweeperShell::run_once_ndjson`]
//!    streaming into an in-memory `Vec<u8>` NDJSON buffer.
//! 4. Parse each line back into an [`IntegrityNdjsonRecord`] and assert
//!    the event stream contains the expected three entries with the
//!    `match`, `mismatch`, and `missing_remote` statuses.
//!
//! No network traffic. The mock fetcher is a trait object so this test
//! exercises the exact production code path end-to-end, minus only the
//! HTTP hop to the real pCloud `checksumfile` endpoint.

// **PLATFORM:** all
// **GATING:** none (portable).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pcloud_config::integrity_sweeper::IntegritySweeperConfig;
use pcloud_daemon::runtime::integrity_sweeper_service::{
    DaemonCheckError, DaemonChecksumFetcher, IntegrityNdjsonRecord, IntegritySweeperShell,
    SweepRoot, ndjson_status,
};
use sha2::{Digest, Sha256};

/// Mock fetcher that returns pre-configured SHA-256 hex digests or
/// `NotFound` per remote path. Used instead of a live `checksumfile`
/// HTTP call so the test is hermetic.
#[derive(Debug, Default)]
struct TestFetcher {
    responses: Mutex<HashMap<String, Result<String, DaemonCheckError>>>,
}

impl TestFetcher {
    fn set_ok(&self, remote_path: &str, sha_hex: &str) {
        self.responses
            .lock()
            .unwrap()
            .insert(remote_path.to_owned(), Ok(sha_hex.to_owned()));
    }

    fn set_not_found(&self, remote_path: &str) {
        self.responses
            .lock()
            .unwrap()
            .insert(remote_path.to_owned(), Err(DaemonCheckError::NotFound));
    }
}

impl DaemonChecksumFetcher for TestFetcher {
    fn fetch_sha256_hex(&self, remote_path: &str) -> Result<String, DaemonCheckError> {
        // `DaemonCheckError` does not implement `Clone`, so we clone
        // field-by-field after a `get()` inspection.
        let guard = self.responses.lock().unwrap();
        match guard.get(remote_path) {
            Some(Ok(hex)) => Ok(hex.clone()),
            Some(Err(DaemonCheckError::NotFound)) => Err(DaemonCheckError::NotFound),
            Some(Err(DaemonCheckError::Other(r))) => Err(DaemonCheckError::Other(r.clone())),
            None => Err(DaemonCheckError::NotFound),
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::new().chain_update(data).finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn enabled_cfg() -> IntegritySweeperConfig {
    IntegritySweeperConfig {
        enabled: true,
        schedule_cron: None,
        // Generous budget so the 3-file test never throttles.
        rate_files_per_minute: 6000,
        pause_on_battery: false,
        skip_list_path: None,
    }
}

/// `bd-1du.4.6.1` end-to-end walker proof.
///
/// Three files are written to a temp dir. The mock fetcher is
/// configured so:
/// - `match.txt`   — remote SHA-256 equals local (expect `match`)
/// - `drift.bin`   — remote SHA-256 differs from local (expect `mismatch`)
/// - `orphan.dat`  — mock reports `NotFound` (expect `missing_remote`)
///
/// The NDJSON stream must contain exactly three records with those
/// statuses (order-independent — walker uses a DFS but the walker's
/// direntry order is filesystem-dependent).
#[test]
fn walker_emits_match_mismatch_and_missing_remote_ndjson() {
    let tmp = tempfile::tempdir().expect("mkdtemp");
    let root = tmp.path().to_path_buf();

    let match_body = b"match-contents";
    let drift_body = b"local-version-of-drift";
    let orphan_body = b"only-exists-locally";

    std::fs::write(root.join("match.txt"), match_body).unwrap();
    std::fs::write(root.join("drift.bin"), drift_body).unwrap();
    std::fs::write(root.join("orphan.dat"), orphan_body).unwrap();

    let fetcher = Arc::new(TestFetcher::default());
    // `match.txt` matches.
    fetcher.set_ok("/remote/match.txt", &sha256_hex(match_body));
    // `drift.bin` — remote reports a different digest (matching the
    // bytes `"remote-version"` rather than the local file).
    fetcher.set_ok("/remote/drift.bin", &sha256_hex(b"remote-version"));
    // `orphan.dat` — server reports NotFound.
    fetcher.set_not_found("/remote/orphan.dat");

    let mut shell = IntegritySweeperShell::from_config(enabled_cfg()).expect("shell builds");
    // Spawn a worker with a no-op audit sink so the daemon channel does
    // not back up on the test process.
    shell.spawn_worker(|_ev| Ok(()));

    shell.set_sweep_roots(vec![SweepRoot {
        local_path: root.clone(),
        remote_prefix: "/remote".to_owned(),
    }]);
    shell.set_checksum_fetcher(fetcher);

    // Stream NDJSON events into an in-memory buffer.
    let mut ndjson_buf: Vec<u8> = Vec::new();
    shell.run_once_ndjson(&mut ndjson_buf);

    // Shutdown joins the worker thread, draining all channel events so that
    // files_hashed is stable before we read the progress snapshot.
    shell.shutdown();
    // Take snapshot AFTER shutdown so files_hashed reflects the drained worker.
    let snapshot = shell.progress_snapshot();

    // Parse each non-empty line as one IntegrityNdjsonRecord.
    let text = String::from_utf8(ndjson_buf).expect("ndjson must be utf-8");
    let records: Vec<IntegrityNdjsonRecord> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<IntegrityNdjsonRecord>(l)
                .unwrap_or_else(|e| panic!("ndjson parse failed for line {l:?}: {e}"))
        })
        .collect();

    // Spec: exactly three records for three files.
    assert_eq!(
        records.len(),
        3,
        "expected 3 NDJSON records for 3 files, got {}: {:?}",
        records.len(),
        records
    );

    // Every record carries ts + path_hash + remote_path.
    for r in &records {
        assert!(!r.ts.is_empty(), "ts must be non-empty RFC3339");
        assert_eq!(
            r.path_hash.len(),
            64,
            "path_hash must be 64 hex chars (SHA-256), got {}",
            r.path_hash.len()
        );
        assert_eq!(r.remote_path, "/remote", "remote_path carries root prefix");
    }

    // Exactly one `match`, one `mismatch`, one `missing_remote`.
    let statuses: Vec<&str> = records.iter().map(|r| r.status.as_str()).collect();
    let match_count = statuses
        .iter()
        .filter(|s| **s == ndjson_status::MATCH)
        .count();
    let mismatch_count = statuses
        .iter()
        .filter(|s| **s == ndjson_status::MISMATCH)
        .count();
    let missing_remote_count = statuses
        .iter()
        .filter(|s| **s == ndjson_status::MISSING_REMOTE)
        .count();

    assert_eq!(match_count, 1, "one `match` status, got {statuses:?}");
    assert_eq!(mismatch_count, 1, "one `mismatch` status, got {statuses:?}");
    assert_eq!(
        missing_remote_count, 1,
        "one `missing_remote` status, got {statuses:?}"
    );

    // The `match` record must carry both local_hash == remote_hash.
    let matched = records
        .iter()
        .find(|r| r.status == ndjson_status::MATCH)
        .unwrap();
    assert_eq!(
        matched.local_hash.as_deref(),
        Some(sha256_hex(match_body).as_str()),
        "match local_hash matches seeded body"
    );
    assert_eq!(
        matched.local_hash, matched.remote_hash,
        "match rows report equal local + remote hashes"
    );

    // The `mismatch` record must carry both hashes and they must differ.
    let mismatched = records
        .iter()
        .find(|r| r.status == ndjson_status::MISMATCH)
        .unwrap();
    assert_eq!(
        mismatched.local_hash.as_deref(),
        Some(sha256_hex(drift_body).as_str()),
        "mismatch local_hash reflects the local bytes"
    );
    assert_eq!(
        mismatched.remote_hash.as_deref(),
        Some(sha256_hex(b"remote-version").as_str()),
        "mismatch remote_hash reflects the server-reported bytes"
    );
    assert_ne!(
        mismatched.local_hash, mismatched.remote_hash,
        "mismatch rows must carry two distinct digests"
    );

    // The `missing_remote` record must have no remote_hash.
    let missing = records
        .iter()
        .find(|r| r.status == ndjson_status::MISSING_REMOTE)
        .unwrap();
    assert!(
        missing.remote_hash.is_none(),
        "missing_remote rows must not carry a remote_hash"
    );
    // Local hash is still present (we did hash the orphan file locally).
    assert!(
        missing.local_hash.is_some(),
        "missing_remote rows still report the local_hash"
    );

    // Progress snapshot cross-check: at least the two cross-checked
    // files (match + mismatch) should bump files_hashed. The
    // `missing_remote` arm translates to `None` at the daemon channel
    // (see `translate_fs_event`), so it does not increment this
    // counter — that's intentional: the daemon channel only surfaces
    // events that need local follow-up. The NDJSON stream is the
    // authoritative per-file record; the `SweepProgress` counter is
    // only the short-form IPC summary.
    assert!(
        snapshot.files_hashed >= 2,
        "files_hashed >= 2 (match + mismatch), got {}",
        snapshot.files_hashed
    );
    assert_eq!(snapshot.throttled, 0, "no throttling expected");

    // Secret-leak smoke check: the raw local path must never appear in
    // the NDJSON (path_hash only). This mirrors the project-wide audit-
    // redaction invariant.
    let raw_path_str = root.to_string_lossy().into_owned();
    assert!(
        !text.contains(&raw_path_str),
        "NDJSON leaked raw local path: {raw_path_str}"
    );
    // Neither should the individual file names appear.
    for name in ["match.txt", "drift.bin", "orphan.dat"] {
        assert!(
            !text.contains(name),
            "NDJSON leaked file name {name} — must only carry path_hash"
        );
    }
}

/// Disabled sweeper: run_once_ndjson must not write anything to the
/// sink, must not panic, and must return the zero-progress snapshot.
#[test]
fn disabled_walker_writes_nothing() {
    let shell = IntegritySweeperShell::disabled();
    let mut buf: Vec<u8> = Vec::new();
    let snapshot = shell.run_once_ndjson(&mut buf);
    assert!(buf.is_empty(), "disabled sweeper must not emit NDJSON");
    assert_eq!(snapshot.files_hashed, 0);
    assert_eq!(snapshot.mismatches_found, 0);
}

/// A `_unused` marker so rustdoc-included test modules do not warn. Not
/// a test.
#[allow(dead_code)]
fn _touch() -> PathBuf {
    PathBuf::new()
}

//! Integrity sweeper (H14b/c — PR3 of the H14 audit pipeline).
//!
//! ## Purpose
//!
//! Walk a directory tree, compute a per-file SHA-256 hash, apply a
//! glob-based skip list and a token-bucket rate limiter, and emit
//! [`IntegrityEvent`]s into a caller-provided MPSC channel. H14c wires
//! the **server-side** `checksumfile` cross-check on top of the offline
//! H14b walker via the [`ChecksumFetcher`] trait: after the local digest
//! completes, the sweeper asks the fetcher for the remote digest of the
//! corresponding remote path and compares.
//!
//! ## Security posture
//!
//! - **Path privacy.** Events carry only `path_hash` (SHA-256 of the
//!   absolute path bytes); the raw filesystem path never leaves the
//!   sweeper. Audit consumers cannot reconstruct the path from an event.
//! - **Deterministic under test.** Time, throttling sleeps, and
//!   rate-bucket refills all flow through the injected [`Clock`] so the
//!   unit tests are fully deterministic when paired with
//!   `ManualClock` (re-exported from [`pcloud_resilience`]).
//! - **Bounded I/O.** The rate limiter caps file-open bursts to the
//!   configured token-bucket capacity, and every throttle wait is bounded
//!   by [`SweeperConfig::max_throttle_wait`] so a misconfiguration cannot
//!   hang the sweep thread.
//! - **No secret material.** The sweeper never opens the vault, the
//!   store SQLite file, or the audit NDJSON; it is purely a content
//!   scanner for user data directories.
//!
//! ## Honest limitations
//!
//! - **Offline-triggered only.** This crate ships the `IntegritySweeper`
//!   engine and the [`ChecksumFetcher`] trait; it does not start a
//!   background scheduler. The daemon's
//!   `pcloud_daemon::integrity_sweeper_service` wires it into IPC as an
//!   **on-demand** verb. There is currently no cron-driven auto-start.
//! - **Mock fetcher used in unit tests.** Real backend wiring (the
//!   `transfer_backend` / `checksumfile` glue) lands in H14d; this
//!   crate's tests use a trait mock and do not prove end-to-end server
//!   cross-check behaviour.
//! - **Per-file open errors are swallowed** (as `LocalMissing` events)
//!   so one unreadable file does not abort a sweep. A higher-level
//!   consumer must count `LocalMissing` events if it needs a strict
//!   failure mode.

// **PLATFORM:** all (filesystem walk + SHA-256; no FUSE-specific code).
// **GATING:** none.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pcloud_resilience::clock::{Clock, SystemClock};
use pcloud_resilience::{RateLimitError, TokenBucket, TokenBucketConfig};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Size of the chunked read buffer used while streaming a file into
/// [`Sha256`]. Fixed at 4 KiB by the H14b spec.
pub const HASH_CHUNK_BYTES: usize = 4 * 1024;

/// Errors a [`ChecksumFetcher`] implementation may return.
///
/// PR3 only consumes two variants directly: [`CheckError::NotFound`]
/// (mapped to [`IntegrityResult::RemoteMissing`]) and
/// [`CheckError::Other`] (mapped to
/// [`IntegrityResult::FetchFailed`]). Real backend wiring in H14d will
/// surface richer error categories that this crate can fan out into
/// its own variants without breaking existing consumers.
#[derive(Debug, Error)]
pub enum CheckError {
    /// The remote object does not exist (server-side `2009`-class
    /// response, or any equivalent "no such file" signal).
    #[error("remote object not found")]
    NotFound,
    /// Any other failure (transport, auth, decode, server-side error).
    /// The string is already redacted of secrets by the caller.
    #[error("checksum fetch failed: {0}")]
    Other(String),
}

/// Trait the sweeper uses to obtain a server-side SHA-256 for a given
/// remote path.
///
/// Implementations are expected to be cheap to share across threads
/// (the sweeper holds a `&dyn ChecksumFetcher`). Real backend wiring
/// — wrapping the `transfer_backend::classify_file_hashes` helper or
/// the pCloud `checksumfile` endpoint — lands in H14d; this PR ships
/// only the trait plus a mock used by the unit tests.
pub trait ChecksumFetcher: Send + Sync {
    /// Resolve the remote SHA-256 for `remote_path`.
    fn fetch_sha256(&self, remote_path: &str) -> Result<[u8; 32], CheckError>;
}

/// Function mapping a local absolute filesystem path (relative to the
/// sweep root) to the corresponding remote pCloud path used when
/// asking a [`ChecksumFetcher`] for the server-side digest.
pub type RemotePathMapper<'a> = &'a (dyn Fn(&Path) -> String + Send + Sync);

/// Result variants for a single file visited by the sweeper.
///
/// PR3 augments the H14b-era `Hashed` outcome with the four
/// cross-check verdicts emitted **after** the server-side
/// `checksumfile` lookup: [`IntegrityResult::Ok`],
/// [`IntegrityResult::Mismatch`], [`IntegrityResult::RemoteMissing`]
/// and [`IntegrityResult::FetchFailed`]. The legacy `Hashed`
/// variant is retained for callers that bypass the cross-check
/// (e.g. tests or future offline-only modes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityResult {
    /// File was opened and fully hashed (no cross-check performed).
    Hashed,
    /// Local digest matched the server's digest.
    Ok,
    /// Local and server digests differ.
    Mismatch {
        /// Locally-computed SHA-256 of the file contents.
        local: [u8; 32],
        /// Server-reported SHA-256 of the corresponding remote object.
        remote: [u8; 32],
    },
    /// Server reports the remote object does not exist.
    RemoteMissing,
    /// The cross-check call itself failed for a non-`NotFound` reason.
    FetchFailed {
        /// Human-readable reason (already redacted of secrets).
        reason: String,
    },
    /// File disappeared (or was unreadable) between walk and open.
    LocalMissing,
    /// File matched a configured skip-list glob.
    Skipped,
    /// Rate limiter forced the sweeper to throttle before this file.
    /// The file *was* still hashed after the throttle wait completed; the
    /// event records that throttling occurred so the audit consumer can
    /// observe back-pressure.
    Throttled,
}

/// One observation emitted by the sweeper for each visited filesystem entry.
#[derive(Debug, Clone)]
pub struct IntegrityEvent {
    /// SHA-256 of the **absolute** path bytes (privacy: the consumer
    /// never receives the path itself).
    pub path_hash: [u8; 32],
    /// SHA-256 of the file contents, when available. `None` for
    /// `Skipped` / `LocalMissing` events.
    pub local_sha256: Option<[u8; 32]>,
    /// Outcome for this entry.
    pub result: IntegrityResult,
    /// `Clock`-supplied timestamp at which the event was generated.
    pub timestamp: Instant,
}

/// Aggregate counters returned at the end of a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    /// Files for which a SHA-256 was successfully computed.
    pub files_hashed: u64,
    /// Files matched by a skip-list glob.
    pub files_skipped: u64,
    /// Files for which the sweeper had to wait on the rate limiter at
    /// least once.
    pub files_throttled: u64,
    /// Sum of file sizes (bytes) actually hashed.
    pub bytes_hashed: u64,
    /// Wall time the sweep took, measured against the injected clock.
    pub elapsed: Duration,
}

/// Errors the sweeper can return.
#[derive(Debug, Error)]
pub enum SweepError {
    /// The supplied root could not be canonicalised or did not exist.
    #[error("invalid sweep root {path:?}: {source}")]
    InvalidRoot {
        /// The offending root path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Walking the tree failed irrecoverably (i.e. the root directory
    /// itself could not be enumerated). Per-file open failures are
    /// reported as `LocalMissing` events instead of bubbling up here.
    #[error("walk failure under {path:?}: {source}")]
    Walk {
        /// Directory whose enumeration failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The MPSC sink was dropped mid-sweep.
    #[error("event sink closed")]
    SinkClosed,
    /// Glob compilation failed during construction.
    #[error("invalid skip pattern {pattern:?}: {source}")]
    InvalidSkipPattern {
        /// The offending pattern.
        pattern: String,
        /// Underlying glob parse error.
        #[source]
        source: glob::PatternError,
    },
    /// Rate-limiter configuration was rejected.
    #[error("invalid rate limit: {0}")]
    InvalidRateLimit(#[from] RateLimitError),
}

/// Sweeper configuration. All knobs are explicit; none are read from
/// global state.
#[derive(Debug, Clone)]
pub struct SweeperConfig {
    /// Glob patterns (matched against the path **relative** to the
    /// sweep root) whose matches are emitted as `Skipped` and not hashed.
    pub skip_patterns: Vec<String>,
    /// Token-bucket capacity (max burst of files hashed before throttling).
    pub rate_capacity: u32,
    /// Token-bucket refill rate (files per second).
    pub rate_refill_per_sec: f64,
    /// Maximum time the sweeper is willing to sleep on a single throttle.
    /// Bounded to keep tests deterministic and prevent unbounded waits.
    pub max_throttle_wait: Duration,
}

impl Default for SweeperConfig {
    fn default() -> Self {
        Self {
            skip_patterns: Vec::new(),
            rate_capacity: 64,
            rate_refill_per_sec: 32.0,
            max_throttle_wait: Duration::from_secs(5),
        }
    }
}

/// Offline integrity sweeper. Stateless across sweeps; cheap to clone via
/// [`Arc`]-shared internals.
pub struct IntegritySweeper {
    config: SweeperConfig,
    skip_patterns: Vec<glob::Pattern>,
    rate_limiter: TokenBucket,
    clock: Arc<dyn Clock>,
    sleep: Arc<dyn Fn(Duration) + Send + Sync>,
}

impl std::fmt::Debug for IntegritySweeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntegritySweeper")
            .field("config", &self.config)
            .field("skip_pattern_count", &self.skip_patterns.len())
            .finish()
    }
}

impl IntegritySweeper {
    /// Build a sweeper using [`SystemClock`] and `std::thread::sleep`.
    pub fn new(config: SweeperConfig) -> Result<Self, SweepError> {
        Self::with_clock_and_sleep(config, Arc::new(SystemClock), Arc::new(std::thread::sleep))
    }

    /// Build a sweeper with an injected clock and sleep fn (used by tests
    /// to keep throttling deterministic).
    pub fn with_clock_and_sleep(
        config: SweeperConfig,
        clock: Arc<dyn Clock>,
        sleep: Arc<dyn Fn(Duration) + Send + Sync>,
    ) -> Result<Self, SweepError> {
        let mut compiled = Vec::with_capacity(config.skip_patterns.len());
        for raw in &config.skip_patterns {
            let pat = glob::Pattern::new(raw).map_err(|source| SweepError::InvalidSkipPattern {
                pattern: raw.clone(),
                source,
            })?;
            compiled.push(pat);
        }
        let bucket_cfg = TokenBucketConfig::new(config.rate_capacity, config.rate_refill_per_sec)?;
        let bucket = TokenBucket::with_clock(bucket_cfg, clock.clone());
        Ok(Self {
            config,
            skip_patterns: compiled,
            rate_limiter: bucket,
            clock,
            sleep,
        })
    }

    /// Walk `root` and emit one [`IntegrityEvent`] per visited file.
    ///
    /// `fetcher` is consulted after each successful local hash to
    /// produce one of [`IntegrityResult::Ok`],
    /// [`IntegrityResult::Mismatch`], [`IntegrityResult::RemoteMissing`]
    /// or [`IntegrityResult::FetchFailed`]. `remote_path_for` maps the
    /// **relative** path under the sweep root to the corresponding
    /// pCloud remote path used in the lookup.
    pub fn sweep<P: AsRef<Path>>(
        &self,
        root: P,
        sink: &mpsc::Sender<IntegrityEvent>,
        fetcher: &dyn ChecksumFetcher,
        remote_path_for: RemotePathMapper<'_>,
    ) -> Result<SweepReport, SweepError> {
        let root_ref = root.as_ref();
        let canonical_root = root_ref
            .canonicalize()
            .map_err(|source| SweepError::InvalidRoot {
                path: root_ref.to_path_buf(),
                source,
            })?;
        let started = self.clock.now();
        let mut report = SweepReport {
            files_hashed: 0,
            files_skipped: 0,
            files_throttled: 0,
            bytes_hashed: 0,
            elapsed: Duration::ZERO,
        };

        let mut stack: Vec<PathBuf> = vec![canonical_root.clone()];
        while let Some(dir) = stack.pop() {
            let read_dir = std::fs::read_dir(&dir).map_err(|source| SweepError::Walk {
                path: dir.clone(),
                source,
            })?;
            for entry in read_dir {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue, // best-effort: skip per-entry errors
                };
                let path = entry.path();
                let file_type = match entry.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                self.process_file(
                    &canonical_root,
                    &path,
                    sink,
                    &mut report,
                    Some((fetcher, remote_path_for)),
                )?;
            }
        }

        report.elapsed = self.clock.now().saturating_duration_since(started);
        Ok(report)
    }

    fn process_file(
        &self,
        root: &Path,
        path: &Path,
        sink: &mpsc::Sender<IntegrityEvent>,
        report: &mut SweepReport,
        cross_check: Option<(&dyn ChecksumFetcher, RemotePathMapper<'_>)>,
    ) -> Result<(), SweepError> {
        let path_hash = hash_absolute_path(path);
        let rel = path.strip_prefix(root).unwrap_or(path);

        // Skip-list match takes precedence over rate limiting.
        if self.matches_skip(rel) {
            report.files_skipped += 1;
            self.emit(
                sink,
                IntegrityEvent {
                    path_hash,
                    local_sha256: None,
                    result: IntegrityResult::Skipped,
                    timestamp: self.clock.now(),
                },
            )?;
            return Ok(());
        }

        // Rate limiter: try non-blocking; if denied, emit Throttled,
        // sleep until a token is available, then proceed to hash.
        let mut throttled = false;
        if !self
            .rate_limiter
            .try_acquire(1)
            .expect("rate-limit request size 1 ≤ capacity")
        {
            throttled = true;
            report.files_throttled += 1;
            self.emit(
                sink,
                IntegrityEvent {
                    path_hash,
                    local_sha256: None,
                    result: IntegrityResult::Throttled,
                    timestamp: self.clock.now(),
                },
            )?;
            // Reserving acquire returns the wait-duration.
            let wait = self
                .rate_limiter
                .acquire(1)
                .expect("rate-limit request size 1 ≤ capacity");
            let bounded = wait.min(self.config.max_throttle_wait);
            if bounded > Duration::ZERO {
                (self.sleep)(bounded);
            }
        }

        match hash_file_contents(path) {
            Ok((digest, bytes)) => {
                report.files_hashed += 1;
                report.bytes_hashed = report.bytes_hashed.saturating_add(bytes);
                let _ = throttled; // back-pressure already recorded above
                let result = match cross_check {
                    Some((fetcher, mapper)) => {
                        let remote_path = mapper(rel);
                        match fetcher.fetch_sha256(&remote_path) {
                            Ok(remote) if remote == digest => IntegrityResult::Ok,
                            Ok(remote) => IntegrityResult::Mismatch {
                                local: digest,
                                remote,
                            },
                            Err(CheckError::NotFound) => IntegrityResult::RemoteMissing,
                            Err(CheckError::Other(reason)) => {
                                IntegrityResult::FetchFailed { reason }
                            }
                        }
                    }
                    None => IntegrityResult::Hashed,
                };
                self.emit(
                    sink,
                    IntegrityEvent {
                        path_hash,
                        local_sha256: Some(digest),
                        result,
                        timestamp: self.clock.now(),
                    },
                )?;
            }
            Err(_) => {
                // Open / read failure: file vanished or is unreadable.
                self.emit(
                    sink,
                    IntegrityEvent {
                        path_hash,
                        local_sha256: None,
                        result: IntegrityResult::LocalMissing,
                        timestamp: self.clock.now(),
                    },
                )?;
            }
        }
        Ok(())
    }

    fn emit(
        &self,
        sink: &mpsc::Sender<IntegrityEvent>,
        event: IntegrityEvent,
    ) -> Result<(), SweepError> {
        sink.send(event).map_err(|_| SweepError::SinkClosed)
    }

    fn matches_skip(&self, rel: &Path) -> bool {
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };
        self.skip_patterns
            .iter()
            .any(|p| p.matches_path_with(rel, opts))
    }
}

fn hash_absolute_path(path: &Path) -> [u8; 32] {
    let mut hasher = Sha256::new();
    // Prefer raw OS bytes on Unix; fall back to lossy on other platforms
    // for portability. Either way the value is opaque to consumers.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hasher.update(path.as_os_str().as_bytes());
    }
    #[cfg(not(unix))]
    {
        hasher.update(path.to_string_lossy().as_bytes());
    }
    hasher.finalize().into()
}

fn hash_file_contents(path: &Path) -> io::Result<([u8; 32], u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; HASH_CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }
    Ok((hasher.finalize().into(), total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_resilience::clock::ManualClock;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Mutex;

    fn collect_events(rx: mpsc::Receiver<IntegrityEvent>) -> Vec<IntegrityEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    type SleepLog = Arc<Mutex<Vec<Duration>>>;
    type SleepFn = Arc<dyn Fn(Duration) + Send + Sync>;

    fn fast_sleep_recorder() -> (SleepLog, SleepFn) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        let sleep: Arc<dyn Fn(Duration) + Send + Sync> = Arc::new(move |d: Duration| {
            log2.lock().unwrap().push(d);
        });
        (log, sleep)
    }

    fn manual_clock_arc() -> (Arc<ManualClock>, Arc<dyn Clock>) {
        let mc = Arc::new(ManualClock::new());
        let dyn_clock: Arc<dyn Clock> = mc.clone();
        (mc, dyn_clock)
    }

    fn write_file(root: &Path, rel: &str, body: &[u8]) -> PathBuf {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, body).unwrap();
        p
    }

    fn sha256(body: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(body);
        h.finalize().into()
    }

    fn rel_to_remote(rel: &Path) -> String {
        // Tests use a `/` -prefixed POSIX-style remote path so the
        // mapping is unambiguous on every platform.
        let mut s = String::from("/");
        s.push_str(&rel.to_string_lossy().replace('\\', "/"));
        s
    }

    /// Test fixture implementing [`ChecksumFetcher`] from a fixed
    /// `remote_path → result` table. Real backend wiring lands in H14d.
    #[derive(Default)]
    struct MockChecksumFetcher {
        responses: Mutex<HashMap<String, Result<[u8; 32], String>>>,
        not_found: Mutex<Vec<String>>,
    }

    impl MockChecksumFetcher {
        fn new() -> Self {
            Self::default()
        }
        fn set_ok(&self, remote: &str, digest: [u8; 32]) {
            self.responses
                .lock()
                .unwrap()
                .insert(remote.to_string(), Ok(digest));
        }
        fn set_other(&self, remote: &str, reason: &str) {
            self.responses
                .lock()
                .unwrap()
                .insert(remote.to_string(), Err(reason.to_string()));
        }
        fn set_not_found(&self, remote: &str) {
            self.not_found.lock().unwrap().push(remote.to_string());
        }
    }

    impl ChecksumFetcher for MockChecksumFetcher {
        fn fetch_sha256(&self, remote_path: &str) -> Result<[u8; 32], CheckError> {
            if self
                .not_found
                .lock()
                .unwrap()
                .iter()
                .any(|p| p == remote_path)
            {
                return Err(CheckError::NotFound);
            }
            match self.responses.lock().unwrap().get(remote_path).cloned() {
                Some(Ok(d)) => Ok(d),
                Some(Err(reason)) => Err(CheckError::Other(reason)),
                None => Err(CheckError::Other(format!(
                    "no mock response configured for {remote_path}"
                ))),
            }
        }
    }

    fn make_sweeper(cfg: SweeperConfig) -> (IntegritySweeper, SleepLog) {
        let (_mc, clock) = manual_clock_arc();
        let (log, sleep) = fast_sleep_recorder();
        let sw = IntegritySweeper::with_clock_and_sleep(cfg, clock, sleep).unwrap();
        (sw, log)
    }

    fn fast_cfg() -> SweeperConfig {
        SweeperConfig {
            rate_capacity: 1024,
            rate_refill_per_sec: 1024.0,
            ..Default::default()
        }
    }

    #[test]
    fn sweeper_hashes_every_non_skipped_file() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "a.txt", b"alpha");
        write_file(tmp.path(), "sub/b.txt", b"beta-bytes");
        write_file(tmp.path(), "sub/deep/c.bin", b"\x00\x01\x02\x03");

        let (sw, _log) = make_sweeper(fast_cfg());
        let mock = MockChecksumFetcher::new();
        mock.set_ok("/a.txt", sha256(b"alpha"));
        mock.set_ok("/sub/b.txt", sha256(b"beta-bytes"));
        mock.set_ok("/sub/deep/c.bin", sha256(b"\x00\x01\x02\x03"));

        let (tx, rx) = mpsc::channel();
        let report = sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);

        let events = collect_events(rx);
        let ok_events: Vec<_> = events
            .iter()
            .filter(|e| e.result == IntegrityResult::Ok)
            .collect();
        assert_eq!(ok_events.len(), 3);
        assert_eq!(report.files_hashed, 3);
        assert_eq!(report.files_skipped, 0);
        assert_eq!(report.files_throttled, 0);
        for ev in &ok_events {
            assert!(ev.local_sha256.is_some());
            assert_ne!(ev.path_hash, [0u8; 32]);
        }
    }

    #[test]
    fn skip_list_excludes_matching_paths() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "keep.txt", b"keep");
        write_file(tmp.path(), "drop.tmp", b"drop");
        write_file(tmp.path(), "logs/server.log", b"log");

        let (sw, _log) = make_sweeper(SweeperConfig {
            skip_patterns: vec!["*.tmp".into(), "logs/*".into()],
            ..fast_cfg()
        });
        let mock = MockChecksumFetcher::new();
        mock.set_ok("/keep.txt", sha256(b"keep"));

        let (tx, rx) = mpsc::channel();
        let report = sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);
        let events = collect_events(rx);

        assert_eq!(report.files_hashed, 1);
        assert_eq!(report.files_skipped, 2);
        let skipped: Vec<_> = events
            .iter()
            .filter(|e| e.result == IntegrityResult::Skipped)
            .collect();
        assert_eq!(skipped.len(), 2);
        for ev in skipped {
            assert!(ev.local_sha256.is_none());
        }
    }

    #[test]
    fn rate_limiter_throttles_above_configured_rate() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write_file(tmp.path(), &format!("f{i}.dat"), b"x");
        }

        let (sw, sleep_log) = make_sweeper(SweeperConfig {
            rate_capacity: 2,
            rate_refill_per_sec: 1.0,
            max_throttle_wait: Duration::from_millis(10),
            ..Default::default()
        });
        let mock = MockChecksumFetcher::new();
        let x_digest = sha256(b"x");
        for i in 0..5 {
            mock.set_ok(&format!("/f{i}.dat"), x_digest);
        }

        let (tx, rx) = mpsc::channel();
        let report = sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);
        let events = collect_events(rx);

        assert_eq!(report.files_hashed, 5);
        // Capacity 2 then refill 1/s + max_throttle_wait 10ms: at least
        // 3 of the 5 files must trigger a throttle.
        assert!(
            report.files_throttled >= 3,
            "throttled={}",
            report.files_throttled
        );
        let throttle_events = events
            .iter()
            .filter(|e| e.result == IntegrityResult::Throttled)
            .count();
        assert_eq!(throttle_events as u64, report.files_throttled);
        // Sleep recorder must have at least one entry per throttle.
        assert!(sleep_log.lock().unwrap().len() >= report.files_throttled as usize);
    }

    #[test]
    fn local_missing_file_emits_event_and_continues() {
        // We can't easily race the walk on a tempdir, so we exercise the
        // open-failure branch directly via a nonexistent file fed into
        // process_file.
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "present.txt", b"hi");
        let (sw, _log) = make_sweeper(fast_cfg());
        let mock = MockChecksumFetcher::new();
        mock.set_ok("/present.txt", sha256(b"hi"));

        let (tx, rx) = mpsc::channel();
        let mut report = SweepReport {
            files_hashed: 0,
            files_skipped: 0,
            files_throttled: 0,
            bytes_hashed: 0,
            elapsed: Duration::ZERO,
        };
        let ghost = tmp.path().join("ghost.txt"); // never created
        sw.process_file(tmp.path(), &ghost, &tx, &mut report, None)
            .unwrap();
        // And run a real sweep so the present file is also covered.
        sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);

        let events = collect_events(rx);
        let missing: Vec<_> = events
            .iter()
            .filter(|e| e.result == IntegrityResult::LocalMissing)
            .collect();
        assert_eq!(missing.len(), 1);
        assert!(missing[0].local_sha256.is_none());
        // Sweep over `present.txt` still produced a cross-checked Ok event.
        assert!(events.iter().any(|e| e.result == IntegrityResult::Ok));
    }

    #[test]
    fn bytes_hashed_report_matches_sum_of_file_sizes() {
        let tmp = tempfile::tempdir().unwrap();
        let bodies: &[&[u8]] = &[
            b"one",
            b"twenty-bytes--padded",
            b"\x00\x00\x00\x00\x00\x00\x00\x00",
        ];
        let expected: u64 = bodies.iter().map(|b| b.len() as u64).sum();
        for (i, body) in bodies.iter().enumerate() {
            write_file(tmp.path(), &format!("file_{i}.bin"), body);
        }

        let (sw, _log) = make_sweeper(fast_cfg());
        let mock = MockChecksumFetcher::new();
        for (i, body) in bodies.iter().enumerate() {
            mock.set_ok(&format!("/file_{i}.bin"), sha256(body));
        }

        let (tx, rx) = mpsc::channel();
        let report = sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);
        let _ = collect_events(rx);

        assert_eq!(report.bytes_hashed, expected);
        assert_eq!(report.files_hashed, bodies.len() as u64);
    }

    // ---- PR3 cross-check verdicts ------------------------------------

    #[test]
    fn equal_hashes_emit_ok() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "match.bin", b"identical-bytes");

        let (sw, _log) = make_sweeper(fast_cfg());
        let mock = MockChecksumFetcher::new();
        mock.set_ok("/match.bin", sha256(b"identical-bytes"));

        let (tx, rx) = mpsc::channel();
        sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);
        let events = collect_events(rx);
        assert_eq!(
            events
                .iter()
                .filter(|e| e.result == IntegrityResult::Ok)
                .count(),
            1
        );
        let ok = events
            .iter()
            .find(|e| e.result == IntegrityResult::Ok)
            .unwrap();
        assert_eq!(ok.local_sha256, Some(sha256(b"identical-bytes")));
    }

    #[test]
    fn different_hashes_emit_mismatch_with_both_digests() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "drift.bin", b"local-version");
        let local = sha256(b"local-version");
        let remote = sha256(b"remote-version");

        let (sw, _log) = make_sweeper(fast_cfg());
        let mock = MockChecksumFetcher::new();
        mock.set_ok("/drift.bin", remote);

        let (tx, rx) = mpsc::channel();
        sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);
        let events = collect_events(rx);

        let mismatches: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.result {
                IntegrityResult::Mismatch { local, remote } => Some((*local, *remote)),
                _ => None,
            })
            .collect();
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].0, local);
        assert_eq!(mismatches[0].1, remote);
        assert_ne!(mismatches[0].0, mismatches[0].1);
    }

    #[test]
    fn fetcher_not_found_emits_remote_missing() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "orphan.bin", b"only-here-locally");

        let (sw, _log) = make_sweeper(fast_cfg());
        let mock = MockChecksumFetcher::new();
        mock.set_not_found("/orphan.bin");

        let (tx, rx) = mpsc::channel();
        sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);
        let events = collect_events(rx);

        assert_eq!(
            events
                .iter()
                .filter(|e| e.result == IntegrityResult::RemoteMissing)
                .count(),
            1
        );
    }

    #[test]
    fn fetcher_error_emits_fetch_failed_reason() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "boom.bin", b"data");

        let (sw, _log) = make_sweeper(fast_cfg());
        let mock = MockChecksumFetcher::new();
        mock.set_other("/boom.bin", "transport timeout");

        let (tx, rx) = mpsc::channel();
        sw.sweep(tmp.path(), &tx, &mock, &rel_to_remote).unwrap();
        drop(tx);
        let events = collect_events(rx);

        let fetch_failed: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.result {
                IntegrityResult::FetchFailed { reason } => Some(reason.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(fetch_failed.len(), 1);
        assert_eq!(fetch_failed[0], "transport timeout");
    }
}

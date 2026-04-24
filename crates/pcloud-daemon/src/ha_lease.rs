//! Tier-2 active-passive HA lease for `pcloudd`.
//!
//! Two daemons on the same host can coexist — one holds an **exclusive
//! advisory file-range lock** on `<state_dir>/daemon.lease`; the other
//! (when `[ha].mode = "passive"`) binds its IPC socket and rejects
//! every request with a helpful `primary is <owner>` message until the
//! primary releases the lock.
//!
//! # Design notes (see `docs/enterprise/ha.md` §4.4 and Tier 2)
//!
//! * The lease is a single regular file under the state directory;
//!   parent directory is already provisioned `0700` by
//!   `bootstrap::bootstrap_with_config`.
//! * On **Unix** the file is created/enforced at mode `0600` and the
//!   exclusive lock is taken with `flock(LOCK_EX | LOCK_NB)` —
//!   advisory, cooperative, and released by the kernel on process
//!   exit (so a crashed primary is automatically recoverable by the
//!   secondary on the next poll).
//! * On **Windows** the exclusive lock is taken with
//!   `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)`
//!   over a **single reserved byte far past any realistic metadata**
//!   (offset `LEASE_LOCK_BYTE_OFFSET`, one byte). Windows file-range
//!   locks are mandatory — other handles' `ReadFile` over a locked
//!   region fails with `ERROR_LOCK_VIOLATION`. Locking a single
//!   sentinel byte at a very high offset leaves the JSON metadata
//!   region at the start of the file fully readable by secondary
//!   daemons performing `read_lease_metadata`, while still providing
//!   the "only one holder at a time" guarantee. Windows auto-releases
//!   the lock when the owning handle closes, giving the same
//!   crash-recovery guarantee as Unix.
//! * The `0600` chmod step is skipped on Windows — filesystem ACL
//!   hardening there is deferred to `bd-xplat-windows`; the state
//!   directory itself is already protected by the Windows bootstrap
//!   DACL path.
//! * Lock acquisition on Unix is mediated through the safe `fs2`
//!   wrapper (`FileExt::try_lock_exclusive`) so the Unix path stays
//!   at `forbid(unsafe_code)`. On Windows the single-byte
//!   `LockFileEx` / `UnlockFileEx` calls require a tiny, tightly
//!   scoped `unsafe` surface (see `win_lock` below).
//! * The lease file carries human-readable metadata (hostname, pid,
//!   start_ts, instance_id, last_heartbeat) re-written on every
//!   heartbeat. Metadata is **advisory**: the authoritative
//!   "who-holds" signal is the kernel lock, not the file contents.
//! * Metadata is JSON with no platform-specific fields, so a Windows
//!   daemon reading a lease file previously written by a Unix daemon
//!   (or vice versa) works transparently — useful for operators
//!   debugging mixed-host logs.
//!
//! # Threat model
//!
//! The lease is **not** a security boundary. It is a co-ordination
//! primitive between two cooperating daemons running as the same UID
//! (or same Windows SID). Cross-UID / cross-SID isolation is enforced
//! by Tier 1 (XDG 0700 dirs on Unix, per-user DACL on Windows,
//! `SO_PEERCRED` / `GetNamedPipeClientProcessId` on IPC), unchanged
//! by this module.

// `forbid(unsafe_code)` on Unix keeps the production path safe-only
// (the `fs2` flock wrapper encapsulates all unsafe). On Windows the
// narrow `win_lock` module below needs a tightly scoped `unsafe` block
// for the `LockFileEx` / `UnlockFileEx` calls, so we downgrade to
// `deny(unsafe_code)` with a localised `allow` on that one module.
#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default file name under `state_dir`. Owner-only (`0600`).
pub const LEASE_FILE_NAME: &str = "daemon.lease";

/// Default heartbeat cadence (seconds). Primary re-writes the lease
/// metadata at this interval so observers can see a rolling
/// `last_heartbeat_unix`.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Default passive-mode poll cadence (seconds). Secondary attempts to
/// re-acquire at this interval after failing the initial `try_acquire`.
pub const PASSIVE_POLL_INTERVAL_SECS: u64 = 10;

/// Errors surfaced while acquiring or operating the HA lease.
#[derive(Debug, Error)]
pub enum LeaseError {
    /// I/O failure opening / writing the lease file (parent dir
    /// missing, permission error, ENOSPC, ...).
    #[error("lease i/o failure at {path}: {source}")]
    Io {
        /// Offending filesystem path.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// Serialising lease metadata to JSON failed. Should not happen in
    /// practice; surfaced for completeness.
    #[error("lease metadata encode failed: {0}")]
    Encode(#[from] serde_json::Error),
    /// Another process already holds the exclusive lock. The recorded
    /// metadata is returned so the caller can surface a helpful
    /// diagnostic in passive mode.
    #[error("lease held by {owner:?}")]
    HeldBy {
        /// Metadata read from the file at the time acquisition failed.
        /// May be `None` if the file was empty or parse-unfriendly
        /// (e.g. primary has just created it but not yet flushed).
        owner: Option<LeaseOwner>,
    },
}

impl LeaseError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Human-readable lease metadata — written to the lease file on every
/// heartbeat so observers (secondary daemons, operators, `pcloudc ha
/// status`) can identify the primary at a glance.
///
/// Persisted as JSON. All fields are non-secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseOwner {
    /// Hostname reported by `gethostname(3)`.
    pub hostname: String,
    /// Process id of the primary daemon.
    pub pid: u32,
    /// Unix time (seconds) when the primary acquired the lease.
    pub start_ts_unix: u64,
    /// Stable per-boot / per-config identifier (derived from config
    /// state_dir by default). Purely informational.
    pub instance_id: String,
    /// Unix time (seconds) of the last heartbeat write. Bumped every
    /// [`HEARTBEAT_INTERVAL_SECS`] while the primary is alive.
    pub last_heartbeat_unix: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn default_hostname() -> String {
    // Prefer `HOSTNAME` env when set (common on Linux user sessions);
    // fall back to a stable placeholder so downstream serde never
    // panics. We deliberately avoid pulling an extra crate for this.
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned())
}

/// Read-only snapshot of the lease file (without acquiring the lock).
/// Used by the secondary's status probe and by tests.
pub fn read_lease_metadata(path: &Path) -> Result<Option<LeaseOwner>, LeaseError> {
    match OpenOptions::new().read(true).open(path) {
        Ok(mut f) => {
            let mut buf = String::new();
            f.read_to_string(&mut buf)
                .map_err(|e| LeaseError::io(path, e))?;
            if buf.trim().is_empty() {
                return Ok(None);
            }
            match serde_json::from_str::<LeaseOwner>(&buf) {
                Ok(owner) => Ok(Some(owner)),
                Err(_) => Ok(None),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(LeaseError::io(path, e)),
    }
}

/// Active (primary) lease handle. Holds the exclusive `flock` for the
/// handle's lifetime; the kernel drops the lock if the process dies.
///
/// Dropping `LeaseHolder` also signals the heartbeat worker thread
/// (if any) to exit and releases the advisory lock.
pub struct LeaseHolder {
    path: PathBuf,
    // `Option<File>` so `Drop` can take the file and explicitly
    // `unlock_exclusive` it before close (belt-and-braces: the kernel
    // releases on close regardless).
    file: Option<File>,
    owner: LeaseOwner,
    // Heartbeat worker handle + shared stop flag. The worker re-writes
    // lease metadata every `HEARTBEAT_INTERVAL_SECS` and exits when
    // the flag is set or when `LeaseHolder` drops.
    stop: Arc<AtomicBool>,
    heartbeat_thread: Option<JoinHandle<()>>,
    shared_meta: Arc<Mutex<LeaseOwner>>,
}

impl std::fmt::Debug for LeaseHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseHolder")
            .field("path", &self.path)
            .field("owner", &self.owner)
            .field("heartbeat_running", &self.heartbeat_thread.is_some())
            .finish()
    }
}

impl LeaseHolder {
    /// Attempt to acquire the exclusive lease at `path`.
    ///
    /// * Creates the file with mode `0600` if absent.
    /// * Tries a non-blocking `flock(LOCK_EX | LOCK_NB)`.
    /// * On success, rewrites the file with fresh [`LeaseOwner`]
    ///   metadata and returns a [`LeaseHolder`] that will renew the
    ///   heartbeat every [`HEARTBEAT_INTERVAL_SECS`].
    /// * On contention, reads the existing metadata (if any) and
    ///   returns [`LeaseError::HeldBy { owner }`].
    ///
    /// The caller owns the `instance_id`; passing a stable value
    /// (e.g. a hash of `state_dir`) keeps log lines identifiable
    /// across restarts.
    pub fn try_acquire(
        path: impl AsRef<Path>,
        instance_id: impl Into<String>,
    ) -> Result<Self, LeaseError> {
        Self::try_acquire_inner(
            path.as_ref(),
            instance_id.into(),
            Duration::from_secs(HEARTBEAT_INTERVAL_SECS),
            true,
        )
    }

    /// Like [`Self::try_acquire`] but does **not** spawn the
    /// heartbeat worker — useful for unit tests that want
    /// deterministic timing, and for callers that manage their own
    /// scheduling.
    pub fn try_acquire_no_heartbeat(
        path: impl AsRef<Path>,
        instance_id: impl Into<String>,
    ) -> Result<Self, LeaseError> {
        Self::try_acquire_inner(
            path.as_ref(),
            instance_id.into(),
            Duration::from_secs(HEARTBEAT_INTERVAL_SECS),
            false,
        )
    }

    fn try_acquire_inner(
        path: &Path,
        instance_id: String,
        heartbeat_interval: Duration,
        spawn_heartbeat: bool,
    ) -> Result<Self, LeaseError> {
        // Open (or create) the lease file. On Unix we request
        // `mode = 0600` at creation time; on Windows we accept the
        // platform default (ACL hardening is tracked under
        // `bd-xplat-windows` — the state directory itself is already
        // owner-restricted by the bootstrap layer).
        #[cfg(unix)]
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|e| LeaseError::io(path, e))?;
        #[cfg(not(unix))]
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| LeaseError::io(path, e))?;

        // Defensive: fix the mode in case the file pre-existed with
        // relaxed perms (tempdir umask, restored backup, etc.). Unix
        // only — Windows inherits parent ACLs and has no direct
        // mode-bit analogue; see `bd-xplat-windows`.
        #[cfg(unix)]
        if let Ok(meta) = file.metadata() {
            let perms = meta.permissions();
            if perms.mode() & 0o777 != 0o600 {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }

        // Cross-platform non-blocking exclusive lock:
        // * Unix: `flock(LOCK_EX | LOCK_NB)` via fs2 (advisory).
        // * Windows: `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK |
        //            LOCKFILE_FAIL_IMMEDIATELY)` over a single reserved
        //            byte at `win_lock::LEASE_LOCK_BYTE_OFFSET` (past
        //            any realistic metadata size). Windows locks are
        //            mandatory — full-file locking would prevent
        //            observers from reading the JSON metadata, so we
        //            restrict the lock to one sentinel byte.
        match try_lock_exclusive_nb(&file) {
            Ok(()) => {
                // Primary wins. Write fresh metadata.
                let owner = LeaseOwner {
                    hostname: default_hostname(),
                    pid: std::process::id(),
                    start_ts_unix: now_unix(),
                    instance_id,
                    last_heartbeat_unix: now_unix(),
                };
                write_metadata(&file, path, &owner)?;

                let shared_meta = Arc::new(Mutex::new(owner.clone()));
                let stop = Arc::new(AtomicBool::new(false));

                let heartbeat_thread = if spawn_heartbeat {
                    let stop_worker = Arc::clone(&stop);
                    let path_worker = path.to_path_buf();
                    let meta_worker = Arc::clone(&shared_meta);
                    let file_worker = file.try_clone().map_err(|e| LeaseError::io(path, e))?;
                    Some(
                        thread::Builder::new()
                            .name("pcloudd-ha-lease".to_owned())
                            .spawn(move || {
                                heartbeat_loop(
                                    file_worker,
                                    path_worker,
                                    meta_worker,
                                    stop_worker,
                                    heartbeat_interval,
                                );
                            })
                            .map_err(|e| LeaseError::io(path, e))?,
                    )
                } else {
                    None
                };

                Ok(LeaseHolder {
                    path: path.to_path_buf(),
                    file: Some(file),
                    owner,
                    stop,
                    heartbeat_thread,
                    shared_meta,
                })
            }
            Err(_contended) => {
                // Contention. Return the metadata we can read so the
                // secondary can surface a helpful diagnostic.
                drop(file);
                let owner = read_lease_metadata(path).ok().flatten();
                Err(LeaseError::HeldBy { owner })
            }
        }
    }

    /// Return a clone of the owner metadata as written at acquisition
    /// time. The lease file's on-disk snapshot is authoritative for
    /// heartbeat freshness — use [`read_lease_metadata`] against the
    /// same path for a fresh read.
    #[must_use]
    pub fn owner(&self) -> LeaseOwner {
        self.shared_meta
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Path to the lease file — exposed for tests and diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Manually bump the `last_heartbeat_unix` field and re-write the
    /// lease file. Normally called by the heartbeat worker; exposed
    /// for deterministic tests.
    pub fn heartbeat(&self) -> Result<(), LeaseError> {
        let Some(ref file) = self.file else {
            return Ok(());
        };
        let mut owner = self.owner();
        owner.last_heartbeat_unix = now_unix();
        write_metadata(file, &self.path, &owner)?;
        if let Ok(mut g) = self.shared_meta.lock() {
            *g = owner;
        }
        Ok(())
    }

    /// Release the lease immediately. Normally callers just drop the
    /// handle; this method exists for tests that want deterministic
    /// cleanup ordering.
    pub fn release(mut self) {
        self.stop_heartbeat_and_unlock();
    }

    fn stop_heartbeat_and_unlock(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.heartbeat_thread.take() {
            let _ = handle.join();
        }
        if let Some(f) = self.file.take() {
            let _ = unlock_lease(&f);
        }
    }
}

impl Drop for LeaseHolder {
    fn drop(&mut self) {
        self.stop_heartbeat_and_unlock();
    }
}

fn write_metadata(file: &File, path: &Path, owner: &LeaseOwner) -> Result<(), LeaseError> {
    let json = serde_json::to_vec_pretty(owner)?;
    // Rewrite strategy:
    //   1. seek to 0,
    //   2. write_all the JSON,
    //   3. set_len to `json.len()` so no trailing garbage remains
    //      from a previous, longer write.
    //
    // On Windows we hold a single-byte `LockFileEx` far past any
    // realistic metadata size (see `win_lock::LEASE_LOCK_BYTE_OFFSET`).
    // Writing low-offset bytes then shrinking the file back to
    // `json.len()` never crosses the sentinel byte, so the kernel
    // lock remains unaffected. On Unix the `flock` is whole-file
    // advisory; the sequence is just truncate-and-rewrite.
    let mut f = file.try_clone().map_err(|e| LeaseError::io(path, e))?;
    f.seek(SeekFrom::Start(0))
        .map_err(|e| LeaseError::io(path, e))?;
    f.write_all(&json).map_err(|e| LeaseError::io(path, e))?;
    f.set_len(json.len() as u64)
        .map_err(|e| LeaseError::io(path, e))?;
    f.flush().map_err(|e| LeaseError::io(path, e))?;
    Ok(())
}

fn heartbeat_loop(
    file: File,
    path: PathBuf,
    shared_meta: Arc<Mutex<LeaseOwner>>,
    stop: Arc<AtomicBool>,
    interval: Duration,
) {
    // Use a short polling granularity so shutdown remains responsive.
    let tick = Duration::from_millis(200);
    let mut remaining = interval;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let step = std::cmp::min(tick, remaining);
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
        if remaining.is_zero() {
            // Fire heartbeat.
            let mut owner = match shared_meta.lock() {
                Ok(g) => g.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            };
            owner.last_heartbeat_unix = now_unix();
            if write_metadata(&file, &path, &owner).is_ok()
                && let Ok(mut g) = shared_meta.lock()
            {
                *g = owner;
            }
            remaining = interval;
        }
    }
}

// ---------------------------------------------------------------------
// Platform-specific lock primitives
// ---------------------------------------------------------------------
//
// Unix: advisory `flock(2)` via the safe `fs2` wrapper — whole-file
// lock, released on close or process exit. Reads from other handles
// are always allowed because flock is advisory.
//
// Windows: `LockFileEx` is *mandatory* — if we locked the whole file
// an observer doing `ReadFile` over any of the locked bytes would
// fail with `ERROR_LOCK_VIOLATION`, and `read_lease_metadata` would
// never see the JSON. We therefore lock a single sentinel byte past
// any realistic metadata size (`win_lock::LEASE_LOCK_BYTE_OFFSET`).
// `LockFileEx` explicitly permits locking byte ranges beyond EOF,
// and the phantom lock is just as effective at serialising holders.

/// Try to acquire the exclusive non-blocking lease lock on `file`.
/// Returns `Ok(())` on success; `Err(_)` on contention or OS failure.
#[cfg(unix)]
fn try_lock_exclusive_nb(file: &File) -> std::io::Result<()> {
    FileExt::try_lock_exclusive(file)
}

/// Release the exclusive lease lock on `file`.
#[cfg(unix)]
fn unlock_lease(file: &File) -> std::io::Result<()> {
    FileExt::unlock(file)
}

#[cfg(windows)]
fn try_lock_exclusive_nb(file: &File) -> std::io::Result<()> {
    win_lock::try_lock_exclusive_single_byte(file)
}

#[cfg(windows)]
fn unlock_lease(file: &File) -> std::io::Result<()> {
    win_lock::unlock_single_byte(file)
}

/// Windows `LockFileEx` / `UnlockFileEx` helpers.
///
/// The `unsafe` surface is strictly limited to the lock/unlock FFI
/// invocations and the matching `GetLastError` reads. The `OVERLAPPED`
/// structure is zero-initialised in safe code (via `Default`) before
/// being passed by reference.
///
/// **SAFETY discipline**:
/// * `file` is a valid, owned [`File`]; `as_raw_handle()` therefore
///   returns a live kernel handle for the duration of the FFI call.
/// * `OVERLAPPED` is zero-initialised, which is a valid representation
///   per MSDN. `hEvent` stays null; `Offset` / `OffsetHigh` are set to
///   the sentinel byte. Lifetimes of the mutable borrow passed to
///   `LockFileEx` / `UnlockFileEx` are bounded by the call itself —
///   no async completion is possible because we always pass
///   `LOCKFILE_FAIL_IMMEDIATELY`.
/// * The same `(offset, length) = (LEASE_LOCK_BYTE_OFFSET, 1)` tuple
///   is used for lock and unlock; Windows requires byte-for-byte
///   identity for a clean unlock.
#[cfg(windows)]
#[allow(unsafe_code)]
mod win_lock {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::{ERROR_LOCK_VIOLATION, GetLastError, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
    };
    use windows::Win32::System::IO::OVERLAPPED;

    /// Byte offset of the single sentinel byte we lock. Chosen at
    /// `1 << 62` so it lies far past any realistic lease-metadata
    /// size (JSON payloads are a few hundred bytes at most) while
    /// still being representable as `Offset`/`OffsetHigh`
    /// `u32 + u32 = u64`. Phantom locks past EOF are fully supported
    /// by `LockFileEx` per MSDN.
    pub(super) const LEASE_LOCK_BYTE_OFFSET: u64 = 1u64 << 62;

    /// One byte — the minimum useful lock range.
    const LEASE_LOCK_BYTE_LEN: u64 = 1;

    /// Non-blocking exclusive `LockFileEx` over the sentinel byte.
    pub(super) fn try_lock_exclusive_single_byte(file: &File) -> io::Result<()> {
        let handle = HANDLE(file.as_raw_handle() as _);
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = LEASE_LOCK_BYTE_OFFSET as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (LEASE_LOCK_BYTE_OFFSET >> 32) as u32;

        // SAFETY: `handle` is a live file handle owned by the caller;
        // `overlapped` is a locally-owned `OVERLAPPED` initialised to
        // zero except for the intended offset. `LOCKFILE_FAIL_IMMEDIATELY`
        // guarantees synchronous completion, so the borrow of
        // `overlapped` does not outlive this call.
        let ok = unsafe {
            LockFileEx(
                handle,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                LEASE_LOCK_BYTE_LEN as u32,
                (LEASE_LOCK_BYTE_LEN >> 32) as u32,
                &mut overlapped,
            )
        };
        if ok.is_err() {
            // SAFETY: `GetLastError` has no preconditions.
            let code = unsafe { GetLastError() };
            // Map contention to a stable `ErrorKind` so the caller's
            // `match Err(_)` behaves like the Unix `EWOULDBLOCK`
            // return.
            if code == ERROR_LOCK_VIOLATION {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "lease lock already held",
                ));
            }
            return Err(io::Error::from_raw_os_error(code.0 as i32));
        }
        Ok(())
    }

    /// Release the sentinel-byte lock. The lock tuple matches the
    /// one passed to `LockFileEx` byte-for-byte, as Windows requires.
    pub(super) fn unlock_single_byte(file: &File) -> io::Result<()> {
        let handle = HANDLE(file.as_raw_handle() as _);
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = LEASE_LOCK_BYTE_OFFSET as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (LEASE_LOCK_BYTE_OFFSET >> 32) as u32;

        // SAFETY: see `try_lock_exclusive_single_byte`. The unlock
        // tuple matches the lock tuple byte-for-byte.
        let ok = unsafe {
            UnlockFileEx(
                handle,
                0,
                LEASE_LOCK_BYTE_LEN as u32,
                (LEASE_LOCK_BYTE_LEN >> 32) as u32,
                &mut overlapped,
            )
        };
        if ok.is_err() {
            // SAFETY: `GetLastError` has no preconditions.
            let code = unsafe { GetLastError() };
            return Err(io::Error::from_raw_os_error(code.0 as i32));
        }
        Ok(())
    }
}

/// Daemon HA mode — snapshot of the runtime posture used by the
/// `pcloudc ha status` IPC probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HaMode {
    /// HA is **disabled** for this daemon (legacy single-instance
    /// behaviour — default).
    Disabled,
    /// This daemon holds the exclusive lease and is serving requests
    /// normally.
    Primary,
    /// This daemon failed to acquire the lease and is running in
    /// passive mode (socket bound; requests rejected with
    /// `Unavailable`).
    Passive,
}

/// JSON payload returned by the `Method::HaStatus` IPC call (and
/// rendered by `pcloudc ha status`). All fields are non-secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HaStatusPayload {
    /// Current mode for this daemon.
    pub mode: HaMode,
    /// Metadata of the lease holder as last read from the lease
    /// file, or `None` if HA is disabled / the file is absent.
    pub lease_owner: Option<LeaseOwner>,
    /// Seconds since the lease owner's last heartbeat, or `None` if
    /// no metadata is available.
    pub lease_age_s: Option<u64>,
    /// Path to the lease file (for operator troubleshooting).
    pub lease_path: Option<String>,
}

impl HaStatusPayload {
    /// Build a payload from a primary holder (includes lease path
    /// + fresh heartbeat age).
    #[must_use]
    pub fn from_primary(holder: &LeaseHolder) -> Self {
        let owner = holder.owner();
        let age = now_unix().saturating_sub(owner.last_heartbeat_unix);
        Self {
            mode: HaMode::Primary,
            lease_owner: Some(owner),
            lease_age_s: Some(age),
            lease_path: Some(holder.path().display().to_string()),
        }
    }

    /// Build a payload for a passive daemon polling the given lease
    /// file. Always reads the file anew so secondaries see fresh
    /// heartbeat ages.
    #[must_use]
    pub fn from_passive(path: &Path) -> Self {
        let owner = read_lease_metadata(path).ok().flatten();
        let age = owner
            .as_ref()
            .map(|o| now_unix().saturating_sub(o.last_heartbeat_unix));
        Self {
            mode: HaMode::Passive,
            lease_owner: owner,
            lease_age_s: age,
            lease_path: Some(path.display().to_string()),
        }
    }

    /// Build a payload for a daemon where HA is disabled entirely.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            mode: HaMode::Disabled,
            lease_owner: None,
            lease_age_s: None,
            lease_path: None,
        }
    }

    /// Render a short human-readable summary for the passive-mode
    /// Unavailable response message. Format:
    /// `this daemon is passive; primary is <host>/pid=<pid> (age=<s>s)`.
    #[must_use]
    pub fn passive_rejection_message(&self) -> String {
        match self.lease_owner.as_ref() {
            Some(o) => format!(
                "this daemon is passive; primary is {}/pid={} (age={}s, instance={})",
                o.hostname,
                o.pid,
                self.lease_age_s.unwrap_or(0),
                o.instance_id,
            ),
            None => "this daemon is passive; primary metadata is currently unavailable".to_owned(),
        }
    }
}

/// Composite runtime HA state owned by [`crate::runtime::RuntimeShell`].
///
/// There are three shapes:
///
/// * `Disabled` — `[ha].enabled = false`; behaves like the pre-HA daemon.
/// * `Primary { holder }` — we hold the lease; the heartbeat worker
///   keeps `last_heartbeat_unix` fresh. Dispatch proceeds normally.
/// * `Passive { lease_path }` — another daemon holds the lease.
///   `serve::accept_loop` should route every request to the
///   `Unavailable` rejection message built from
///   [`HaStatusPayload::passive_rejection_message`].
///
/// Transitioning from `Passive` to `Primary` is the promotion step
/// invoked by the passive poll loop once the lease file becomes
/// reacquirable.
#[derive(Debug, Default)]
pub enum HaRuntime {
    /// Tier-2 HA is not enabled for this daemon.
    #[default]
    Disabled,
    /// This daemon holds the exclusive lease.
    Primary {
        /// Owned lease handle. Dropping the runtime releases the lease.
        holder: LeaseHolder,
    },
    /// This daemon failed to acquire the lease at startup and is
    /// running in passive mode.
    Passive {
        /// Path of the lease file being polled.
        lease_path: PathBuf,
    },
}

impl HaRuntime {
    /// Explicit `disabled` constructor — makes call sites uniform with
    /// the `secure_defaults` helpers elsewhere in the workspace.
    #[must_use]
    pub fn disabled() -> Self {
        Self::Disabled
    }

    /// Is this daemon currently rejecting requests because another
    /// instance owns the lease?
    #[must_use]
    pub fn is_passive(&self) -> bool {
        matches!(self, Self::Passive { .. })
    }

    /// Render the current HA state as a JSON-serialisable payload.
    /// Fresh on every call (re-reads the lease file in passive mode).
    #[must_use]
    pub fn status_payload(&self) -> HaStatusPayload {
        match self {
            Self::Disabled => HaStatusPayload::disabled(),
            Self::Primary { holder } => HaStatusPayload::from_primary(holder),
            Self::Passive { lease_path } => HaStatusPayload::from_passive(lease_path),
        }
    }

    /// Try to promote a passive runtime to primary by re-acquiring the
    /// lease. Returns `Ok(true)` on successful promotion. Called by the
    /// passive poll loop (every
    /// `ha.passive_poll_interval_secs` seconds).
    pub fn try_promote(&mut self, instance_id: &str) -> Result<bool, LeaseError> {
        let Self::Passive { lease_path } = self else {
            return Ok(false);
        };
        match LeaseHolder::try_acquire(lease_path.clone(), instance_id.to_owned()) {
            Ok(holder) => {
                *self = Self::Primary { holder };
                Ok(true)
            }
            Err(LeaseError::HeldBy { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

// Unix-only tests (PermissionsExt + flock helpers).
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    fn lease_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join(LEASE_FILE_NAME)
    }

    #[test]
    fn acquire_writes_metadata_with_0600_mode() {
        let dir = tempdir().expect("tempdir");
        let path = lease_path(&dir);
        let holder = LeaseHolder::try_acquire_no_heartbeat(&path, "instance-A")
            .expect("first acquire succeeds");

        assert!(path.exists(), "lease file created");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "lease file is owner-only");

        let owner = holder.owner();
        assert_eq!(owner.pid, std::process::id());
        assert_eq!(owner.instance_id, "instance-A");
        assert!(owner.last_heartbeat_unix >= owner.start_ts_unix);

        // On-disk metadata is readable and matches.
        let disk = read_lease_metadata(&path).expect("read").expect("some");
        assert_eq!(disk.pid, owner.pid);
        assert_eq!(disk.instance_id, "instance-A");

        drop(holder);
    }

    #[test]
    fn second_acquire_returns_held_by_with_metadata() {
        let dir = tempdir().expect("tempdir");
        let path = lease_path(&dir);
        let first = LeaseHolder::try_acquire_no_heartbeat(&path, "instance-A")
            .expect("first acquire succeeds");

        match LeaseHolder::try_acquire_no_heartbeat(&path, "instance-B") {
            Err(LeaseError::HeldBy { owner }) => {
                let owner = owner.expect("metadata readable");
                assert_eq!(owner.instance_id, "instance-A");
                assert_eq!(owner.pid, std::process::id());
            }
            Ok(_) => panic!("second acquire should have failed"),
            Err(e) => panic!("unexpected error: {e}"),
        }

        drop(first);
    }

    #[test]
    fn steal_on_release_succeeds() {
        let dir = tempdir().expect("tempdir");
        let path = lease_path(&dir);

        let first = LeaseHolder::try_acquire_no_heartbeat(&path, "instance-A")
            .expect("first acquire succeeds");
        // Second attempt fails.
        assert!(matches!(
            LeaseHolder::try_acquire_no_heartbeat(&path, "instance-B"),
            Err(LeaseError::HeldBy { .. })
        ));

        // Release the first; the second now succeeds.
        first.release();

        let second = LeaseHolder::try_acquire_no_heartbeat(&path, "instance-B")
            .expect("second acquire after release succeeds");
        assert_eq!(second.owner().instance_id, "instance-B");
        drop(second);
    }

    #[test]
    fn heartbeat_bumps_last_heartbeat_unix() {
        let dir = tempdir().expect("tempdir");
        let path = lease_path(&dir);
        let holder = LeaseHolder::try_acquire_no_heartbeat(&path, "instance-A").expect("acquire");

        let before = holder.owner().last_heartbeat_unix;
        // Ensure at least 1s passes so the unix-second heartbeat is
        // observably different.
        thread::sleep(Duration::from_millis(1100));
        holder.heartbeat().expect("heartbeat");
        let after = holder.owner().last_heartbeat_unix;

        assert!(after >= before, "heartbeat is monotone");
        assert!(after > before, "heartbeat advanced at least one second");
    }

    #[test]
    fn read_metadata_missing_file_returns_none() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("no-such-lease");
        assert!(matches!(read_lease_metadata(&path), Ok(None)));
    }

    #[test]
    fn ha_status_payloads_serialize() {
        let disabled = HaStatusPayload::disabled();
        let s = serde_json::to_string(&disabled).expect("encode");
        assert!(s.contains("\"disabled\""));

        let dir = tempdir().expect("tempdir");
        let path = lease_path(&dir);
        let holder = LeaseHolder::try_acquire_no_heartbeat(&path, "instance-A").expect("acquire");
        let primary = HaStatusPayload::from_primary(&holder);
        assert_eq!(primary.mode, HaMode::Primary);
        assert!(primary.lease_owner.is_some());
        drop(holder);

        let passive = HaStatusPayload::from_passive(&path);
        assert_eq!(passive.mode, HaMode::Passive);
        // Lease file still exists on disk with stale metadata.
        assert!(passive.lease_owner.is_some());
    }

    #[test]
    fn passive_rejection_message_includes_primary() {
        let dir = tempdir().expect("tempdir");
        let path = lease_path(&dir);
        let holder = LeaseHolder::try_acquire_no_heartbeat(&path, "instance-A").expect("acquire");
        let p = HaStatusPayload::from_primary(&holder);
        let msg = p.passive_rejection_message();
        assert!(msg.contains("pid="), "msg mentions pid: {msg}");
        assert!(msg.contains("instance-A"), "msg mentions instance: {msg}");
    }
}

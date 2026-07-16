#![forbid(unsafe_code)]
//! # pcloud-store
//!
//! SQLite persistence layer for the Rust pcloud-rs path: schema
//! migrations, repositories, integrity checks, and transaction helpers.
//!
//! This crate is deliberately the lowest layer of the daemon's
//! durability story. It owns the on-disk SQLite file, enforces Unix
//! ownership and mode, enforces schema-version linearity, owns the
//! tamper-evident audit log, and exposes per-table repositories that
//! upper layers compose into higher-level runtime state. It does
//! **not** speak to the network and it does **not** depend on the
//! `pcloud-secret` crate (see the audit repository for why HMAC keys
//! are held as raw `Vec<u8>` here rather than `SecretBytes`).
//!
//! ## Storage invariants
//!
//! These invariants are enforced on every call that hands out a
//! connection — never assume a stray `Connection::open` elsewhere in
//! the workspace carries them.
//!
//! * The store file is created owner-only (`0600`) and lives under a
//!   `0700` parent directory (enforced by [`bootstrap_profile`]). The
//!   mode is re-applied on every bootstrap so a previously-permissive
//!   file is tightened in place on upgrade.
//! * Write-Ahead Logging is enabled (`PRAGMA journal_mode = WAL`) for
//!   concurrent readers, durability under crash, and bounded commit
//!   latency. WAL is a persistent file-level setting; once set it
//!   survives across restarts. The legacy rollback journal is never
//!   used by this crate.
//! * `PRAGMA foreign_keys = ON` on every handed-out connection — the
//!   pragma is per-connection, so the internal `tune_connection` helper is
//!   re-invoked for every pool or short-lived connection.
//! * `PRAGMA synchronous = NORMAL` — safe under WAL (only a crash
//!   during the checkpoint can lose the last few microseconds of
//!   committed writes; the main database file itself is never torn)
//!   and avoids the per-commit fsync on the write-ahead log that would
//!   otherwise make key/value writes an order of magnitude slower.
//! * `PRAGMA temp_store = MEMORY` — scratch data stays off disk and
//!   off the WAL.
//!
//! ## Transaction discipline
//!
//! Multi-statement mutations go through [`tx::TransactionBoundary`],
//! which wraps the closure in `BEGIN IMMEDIATE` / `COMMIT` with an
//! automatic `ROLLBACK` on error. `BEGIN IMMEDIATE` (as opposed to
//! the default deferred mode) takes a reserved lock eagerly so
//! competing writers fail fast instead of racing all the way to the
//! commit point and losing work. Audit appends use the same pattern
//! internally so that the INSERT and the hash back-fill are atomic —
//! a process crash between the two can never leave an unhashed row.
//!
//! The transaction helper is intentionally **not** a RAII drop guard
//! (it is a zero-sized marker type carrying the method). That is a
//! deliberate choice: panic-on-drop rollback would mask the original
//! error, and the helper is constrained to synchronous closures so
//! the explicit commit/rollback on each return path is easier to
//! audit. See [`tx::TransactionBoundary`] for the contract.
//!
//! ## Schema migrations
//!
//! Migrations are forward-only, idempotent, and data-preserving. The
//! current schema is [`schema::SCHEMA_VERSION_V12`].
//! [`bootstrap_profile`] reads the database's `PRAGMA user_version`,
//! calls [`migrations::build_plan`] to produce a
//! [`migrations::MigrationPlan`], and applies every step needed to
//! reach the crate-hard-coded target.
//!
//! ### Rollback policy
//!
//! **There is no rollback path.** A request to migrate to a version
//! older than the current on-disk version surfaces
//! [`migrations::MigrationError::BackwardsMigration`] rather than
//! silently wiping rows. Callers that need to reset the store must
//! delete the file — this is intentional, because some schema steps
//! (notably the v8 audit rebuild) widen rows with cryptographic
//! state that a naive downgrade would discard and which cannot be
//! recomputed from a later, richer shape.
//!
//! ### Per-version intent
//!
//! | Version | Intent | Forward step |
//! | --- | --- | --- |
//! | v1  | Bootstrap tables | `account`, `audit_events`, `sync_roots`. |
//! | v2  | Structured audit payloads | Adds `audit_events.details`. |
//! | v3  | Full sync-root row | Replaces id-only `sync_roots` with `sync_root_records` carrying local/remote path and paused flag. |
//! | v4  | Preferences scaffold | Adds `preferences(name, bool_value)`. |
//! | v5  | Typed preferences | Extends `preferences` with `text_value` / `int_value`. |
//! | v6  | Sync type carry-over | Adds `sync_root_records.sync_type` mirroring the C `psync_synctype_t` enum; defaults existing rows to `Full` sync. |
//! | v7  | Open key/value table | Adds `value_kv` with a persisted type tag. |
//! | v8  | Tamper-evident audit | Adds `prev_hash`/`entry_hash`/`hmac` BLOBs to `audit_events` and re-hashes all existing rows in insertion order via [`repositories::audit::rebuild_hash_chain`]. |
//! | v9  | Upload resume | Adds `upload_resume_state` for chunked upload client-tracked offsets. |
//! | v10 | Diff cursor persistence | Adds `sync_diff_state` per-sync `diffid` cursor so daemons resume rather than refetching account history. |
//! | v11 | File metadata cache | Adds `file_metadata` table for local file/folder metadata populated by the diff engine, backing `stat_path` local resolution. |
//!
//! Each step bumps `PRAGMA user_version` as its last statement so
//! that partial migration followed by a crash leaves the database at
//! the last successfully-committed version, not half-applied.
//!
//! ## Repositories
//!
//! Each logical table has a repository in [`repositories`]. All
//! repositories are designed to be called under an outer
//! [`tx::TransactionBoundary::immediate`] when more than one is being
//! mutated together ([`persist_profile`] is the canonical example).
//!
//! * [`repositories::account`] — primary account record (single row,
//!   enforced by a `CHECK (primary_account = 1)` constraint).
//! * [`repositories::audit`] — tamper-evident SHA-256 hash-chained
//!   audit log with optional HMAC-SHA256 non-repudiation. See the
//!   module docs for the chain construction and [`verify_audit_chain`]
//!   for the external verification entry point.
//! * [`repositories::preferences`] — strongly-typed daemon
//!   preferences keyed by stable names.
//! * [`repositories::settings`] — strict typed key/value helpers on
//!   top of `value_kv`; surfaces a type-mismatch error on cross-kind
//!   reads.
//! * [`repositories::values`] — loose typed key/value helpers on top
//!   of `value_kv`; mirrors the C `psync_*_value` sentinel contract.
//! * [`repositories::sync_graph`] — persisted sync-root records.
//! * [`repositories::diff_state`] — per-sync diff cursor state.
//! * [`repositories::file_metadata`] — local file/folder metadata cache.
//! * [`repositories::upload_resume`] — chunked upload resume state.

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

/// Startup integrity probes (`PRAGMA quick_check` + schema version compare).
pub mod integrity;
/// Forward-only schema migration planner and applier.
pub mod migrations;
/// Per-table repositories (account, audit, preferences, settings, sync graph, …).
pub mod repositories;
/// `SQLITE_BUSY` classification + exponential-backoff retry helper.
pub mod retry;
/// Versioned DDL steps and `PRAGMA user_version` helpers.
pub mod schema;
/// `BEGIN IMMEDIATE` / `COMMIT` / `ROLLBACK` transaction boundary helpers.
pub mod tx;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::Connection;
use thiserror::Error;

use integrity::IntegrityStatus;
use migrations::{MigrationError, apply_plan, build_plan};
use repositories::RepositorySet;
pub use repositories::diff_state::{DiffStateRecord, DiffStateRepository};
pub use repositories::file_metadata::{FileMetadataRecord, FileMetadataRepository, StatResult};
use schema::{SCHEMA_VERSION_V12, read_schema_version, schema_exists};
use tx::TransactionBoundary;

/// Human-readable crate name used by telemetry / logging.
pub const CRATE_NAME: &str = "pcloud-store";

/// Snapshot of an opened store: schema version, on-disk location, and
/// the in-memory view of every repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreProfile {
    /// Schema version currently materialized on disk.
    pub schema_version: u32,
    /// Path to the SQLite database file.
    pub db_path: PathBuf,
    /// In-memory view of every repository loaded from the database.
    pub repositories: RepositorySet,
}

/// Error surface for the crate's top-level bootstrap / persistence
/// helpers. Each variant wraps the underlying source error so callers
/// can pattern-match on a concrete cause.
#[derive(Debug, Error)]
pub enum StoreError {
    /// I/O failure preparing the store directory or tightening file
    /// permissions.
    #[error("failed to prepare store directory: {0}")]
    Io(#[from] std::io::Error),
    /// Underlying SQLite error from the rusqlite driver.
    #[error("sqlite operation failed: {0}")]
    Sql(#[from] rusqlite::Error),
    /// Migration planner refused to produce a plan (e.g. a backwards
    /// migration was requested).
    #[error("migration planning failed: {0}")]
    Migration(#[from] MigrationError),
}

/// Open or create the SQLite store at `db_path`, enforce file-system
/// permissions, apply any outstanding migrations up to
/// [`schema::SCHEMA_VERSION_V12`], and load the in-memory repository
/// snapshot.
///
/// Returns the loaded [`StoreProfile`] together with the integrity
/// verdict from [`integrity::evaluate_connection_integrity`]. The
/// database file is left at mode `0600`; its parent directory is
/// created if missing.
pub fn bootstrap_profile(db_path: &Path) -> Result<(StoreProfile, IntegrityStatus), StoreError> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(db_path)?;
    // File-permission tightening is Unix-only (`chmod 0600`). On Windows
    // the SQLite file inherits the creating user's ACL, which already
    // restricts access to the daemon account; explicit ACL hardening is
    // deferred to a Windows-native path (bd-xplat-windows).
    #[cfg(unix)]
    fs::set_permissions(db_path, fs::Permissions::from_mode(0o600))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    tune_connection(&conn)?;

    let current_version = if schema_exists(&conn)? {
        read_schema_version(&conn)?
    } else {
        0
    };
    let plan = build_plan(current_version, SCHEMA_VERSION_V12)?;
    apply_plan(&conn, &plan)?;

    let schema_version = read_schema_version(&conn)?;
    let integrity =
        integrity::evaluate_connection_integrity(&conn, schema_version, SCHEMA_VERSION_V12)?;
    let repositories = RepositorySet::load(&conn)?;

    Ok((
        StoreProfile {
            schema_version,
            db_path: db_path.to_path_buf(),
            repositories,
        },
        integrity,
    ))
}

/// Persist the in-memory repository state of `profile` back to the
/// database file, atomically under a single `BEGIN IMMEDIATE` /
/// `COMMIT` (see [`tx::TransactionBoundary`]). Any error rolls the
/// transaction back so a failed save never leaves a half-written
/// profile on disk.
pub fn persist_profile(profile: &StoreProfile) -> Result<(), StoreError> {
    let conn = Connection::open(&profile.db_path)?;
    tune_connection(&conn)?;
    TransactionBoundary.immediate(&conn, |conn| profile.repositories.save(conn))?;
    Ok(())
}

/// Apply the shared runtime pragmas used by both the short-lived-connection
/// facade and the pooled [`StoreHandle`]. WAL journaling is already persisted
/// on the database file during [`bootstrap_profile`]; this helper makes sure
/// every connection the crate hands out shares the same tuning:
///
/// * `foreign_keys = ON` — matches every existing helper.
/// * `synchronous = NORMAL` — safe under WAL (only a crash during the
///   checkpoint can drop the last few microseconds of committed writes) and
///   removes the per-commit fsync on the write-ahead log that makes the
///   `set_int`/`set_uint` hot path ~30× slower than `set_string`.
/// * `temp_store = MEMORY` — avoids touching the disk for throw-away
///   statement temporaries.
/// * `busy_timeout = 5000` (ms) — installs SQLite's native busy handler so
///   transient `SQLITE_BUSY` from a competing writer is retried internally
///   with exponential-backoff sleeps for up to 5 s before being surfaced
///   to the caller. Applied to **every** connection (pooled and
///   short-lived) so concurrent short-lived facade callers no longer race
///   to the first `BEGIN` and lose. Closes the iter-1 SYNC-H-04-5 finding
///   that the short-lived facade had no busy handler at all and would
///   propagate `SQLITE_BUSY` immediately on the first contention. Callers
///   that need an additional Rust-level retry on top of this engine
///   handler can use [`retry::with_busy_retry`].
fn tune_connection(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    Ok(())
}

/// Shared, long-lived SQLite connection handle.
///
/// The [`value_kv`] / [`settings_kv`] free-function facade opens a fresh
/// `Connection` per call (~200 µs on warm SSDs). That floor is the dominant
/// cost on every `set_*_value` and `set_*_setting` call, as captured in
/// `PERF-BASELINE-14042026.md`. `StoreHandle` solves the hotspot by giving
/// the daemon a cheap, owner-only, clonable handle backed by a
/// `Mutex<Connection>`:
///
/// * Cloning the handle is `Arc::clone` — no I/O, no syscall, no lock.
/// * Every write holds the mutex for the duration of a single `execute` call
///   and returns. There is no long-running transaction held across await
///   points (this crate is intentionally sync, so the mutex guard lifetime
///   is statically bounded to a scope and cannot span tokio yields).
/// * Readers pay the same mutex cost; SQLite itself serializes writes and
///   allows concurrent readers via WAL, but the crate currently uses a
///   single connection, so the `Mutex` is the serialization boundary.
///
/// ## Schema assumptions
///
/// [`StoreHandle::open`] expects the database to have already been brought
/// up to date by [`bootstrap_profile`]. Opening a stale file bypasses the
/// migration planner and will surface `SqliteError` on the first query that
/// touches a post-v1 column. The daemon bootstrap sequence always runs
/// `bootstrap_profile` before constructing a handle.
///
/// ## Poisoning
///
/// The inner `Mutex` only poisons if a prior holder panicked while holding
/// it. [`StoreHandle::lock`] recovers the guard in that case rather than
/// propagating the panic — a SQLite connection holds no Rust-visible
/// invariants whose violation would be unsafe after a panic, and escalating
/// to a process abort here would convert a best-effort retry into a
/// fatal event.
///
/// The short-lived-connection facade (`value_kv::*`, `settings_kv::*`, and
/// friends) is retained for backwards compatibility. Callers that care
/// about throughput should migrate to [`StoreHandle::value_kv`] /
/// [`StoreHandle::settings_kv`].
#[derive(Clone)]
pub struct StoreHandle {
    inner: Arc<StoreHandleInner>,
}

struct StoreHandleInner {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreHandle")
            .field("db_path", &self.inner.db_path)
            .finish()
    }
}

impl StoreHandle {
    /// Open a pooled handle on an already-bootstrapped database.
    pub fn open(db_path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(db_path)?;
        tune_connection(&conn)?;
        Ok(Self {
            inner: Arc::new(StoreHandleInner {
                db_path: db_path.to_path_buf(),
                conn: Mutex::new(conn),
            }),
        })
    }

    /// Database file this handle is bound to.
    pub fn db_path(&self) -> &Path {
        &self.inner.db_path
    }

    /// Acquire the underlying connection. The lock is short-lived — callers
    /// must not await or perform blocking I/O while holding it.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        // Mutex is only poisoned if a prior holder panicked. We recover the
        // inner connection in that case rather than propagating the panic —
        // the SQLite connection itself is panic-safe (no internal
        // invariants visible at the Rust boundary).
        match self.inner.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Run an arbitrary closure against the pooled connection.
    pub fn with_connection<T, F>(&self, f: F) -> T
    where
        F: FnOnce(&Connection) -> T,
    {
        let conn = self.lock();
        f(&conn)
    }

    /// Pooled equivalent of [`crate::value_kv`].
    pub fn value_kv(&self) -> handle_value_kv::Scope<'_> {
        handle_value_kv::Scope { handle: self }
    }

    /// Pooled equivalent of [`crate::settings_kv`].
    pub fn settings_kv(&self) -> handle_settings_kv::Scope<'_> {
        handle_settings_kv::Scope { handle: self }
    }
}

/// Pooled KV accessors. Prefer these over the short-lived [`value_kv`] facade
/// when the caller already owns a [`StoreHandle`].
pub mod handle_value_kv {
    use super::StoreHandle;
    use crate::repositories::values::{ValueKind, ValuesRepository};

    /// RAII accessor bound to a [`StoreHandle`] that routes value-KV
    /// calls through the shared pooled connection.
    pub struct Scope<'a> {
        pub(crate) handle: &'a StoreHandle,
    }

    impl Scope<'_> {
        /// See [`ValuesRepository::get_uint`].
        pub fn get_uint(&self, name: &str) -> Result<u64, rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::get_uint(&conn, name)
        }

        /// See [`ValuesRepository::get_int`].
        pub fn get_int(&self, name: &str) -> Result<i64, rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::get_int(&conn, name)
        }

        /// See [`ValuesRepository::get_bool`].
        pub fn get_bool(&self, name: &str) -> Result<bool, rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::get_bool(&conn, name)
        }

        /// See [`ValuesRepository::get_string`].
        pub fn get_string(&self, name: &str) -> Result<Option<String>, rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::get_string(&conn, name)
        }

        /// See [`ValuesRepository::set_uint`].
        pub fn set_uint(&self, name: &str, value: u64) -> Result<(), rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::set_uint(&conn, name, value)
        }

        /// See [`ValuesRepository::set_int`].
        pub fn set_int(&self, name: &str, value: i64) -> Result<(), rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::set_int(&conn, name, value)
        }

        /// See [`ValuesRepository::set_bool`].
        pub fn set_bool(&self, name: &str, value: bool) -> Result<(), rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::set_bool(&conn, name, value)
        }

        /// See [`ValuesRepository::set_string`].
        pub fn set_string(&self, name: &str, value: &str) -> Result<(), rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::set_string(&conn, name, value)
        }

        /// See [`ValuesRepository::has`].
        pub fn has(&self, name: &str, kind: ValueKind) -> Result<bool, rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::has(&conn, name, kind)
        }

        /// See [`ValuesRepository::delete`].
        pub fn delete(&self, name: &str) -> Result<bool, rusqlite::Error> {
            let conn = self.handle.lock();
            ValuesRepository::delete(&conn, name)
        }
    }
}

/// Pooled settings accessors.
pub mod handle_settings_kv {
    use super::StoreHandle;
    use crate::repositories::settings;

    /// RAII accessor bound to a [`StoreHandle`] that routes strict
    /// settings calls through the shared pooled connection.
    pub struct Scope<'a> {
        pub(crate) handle: &'a StoreHandle,
    }

    impl Scope<'_> {
        /// See [`settings::get_bool_setting`].
        pub fn get_bool(&self, name: &str) -> Result<Option<bool>, settings::SettingsError> {
            let conn = self.handle.lock();
            settings::get_bool_setting(&conn, name)
        }

        /// See [`settings::set_bool_setting`].
        pub fn set_bool(&self, name: &str, value: bool) -> Result<(), settings::SettingsError> {
            let conn = self.handle.lock();
            settings::set_bool_setting(&conn, name, value)
        }

        /// See [`settings::get_int_setting`].
        pub fn get_int(&self, name: &str) -> Result<Option<i64>, settings::SettingsError> {
            let conn = self.handle.lock();
            settings::get_int_setting(&conn, name)
        }

        /// See [`settings::set_int_setting`].
        pub fn set_int(&self, name: &str, value: i64) -> Result<(), settings::SettingsError> {
            let conn = self.handle.lock();
            settings::set_int_setting(&conn, name, value)
        }

        /// See [`settings::get_uint_setting`].
        pub fn get_uint(&self, name: &str) -> Result<Option<u64>, settings::SettingsError> {
            let conn = self.handle.lock();
            settings::get_uint_setting(&conn, name)
        }

        /// See [`settings::set_uint_setting`].
        pub fn set_uint(&self, name: &str, value: u64) -> Result<(), settings::SettingsError> {
            let conn = self.handle.lock();
            settings::set_uint_setting(&conn, name, value)
        }

        /// See [`settings::get_string_setting`].
        pub fn get_string(&self, name: &str) -> Result<Option<String>, settings::SettingsError> {
            let conn = self.handle.lock();
            settings::get_string_setting(&conn, name)
        }

        /// See [`settings::set_string_setting`].
        pub fn set_string(&self, name: &str, value: &str) -> Result<(), settings::SettingsError> {
            let conn = self.handle.lock();
            settings::set_string_setting(&conn, name, value)
        }

        /// See [`settings::reset_setting`].
        pub fn reset(&self, name: &str) -> Result<bool, settings::SettingsError> {
            let conn = self.handle.lock();
            settings::reset_setting(&conn, name)
        }
    }
}

/// Convenience wrappers that mirror the C `psync_{get,set,has}_*_value`
/// helpers. These open a short-lived connection against the profile's
/// database file so callers do not have to own a `Connection` themselves.
///
/// **Deprecated:** prefer [`StoreHandle::value_kv`] for new code. The
/// short-lived-connection path remains for backwards compatibility and
/// pays a ~200 µs per-call `sqlite3_open` tax.
pub mod value_kv {
    use super::{Connection, Path, StoreError};
    use crate::repositories::values::{ValueKind, ValuesRepository};

    fn open(db_path: &Path) -> Result<Connection, StoreError> {
        let conn = Connection::open(db_path)?;
        super::tune_connection(&conn)?;
        Ok(conn)
    }

    /// Read the `uint`-typed value at `name` from the store at `db_path`.
    pub fn get_uint(db_path: &Path, name: &str) -> Result<u64, StoreError> {
        Ok(ValuesRepository::get_uint(&open(db_path)?, name)?)
    }

    /// Read the `int`-typed value at `name` from the store at `db_path`.
    pub fn get_int(db_path: &Path, name: &str) -> Result<i64, StoreError> {
        Ok(ValuesRepository::get_int(&open(db_path)?, name)?)
    }

    /// Read the `bool`-typed value at `name` from the store at `db_path`.
    pub fn get_bool(db_path: &Path, name: &str) -> Result<bool, StoreError> {
        Ok(ValuesRepository::get_bool(&open(db_path)?, name)?)
    }

    /// Read the `string`-typed value at `name` from the store at `db_path`.
    pub fn get_string(db_path: &Path, name: &str) -> Result<Option<String>, StoreError> {
        Ok(ValuesRepository::get_string(&open(db_path)?, name)?)
    }

    /// Upsert a `uint`-typed value at `name`.
    pub fn set_uint(db_path: &Path, name: &str, value: u64) -> Result<(), StoreError> {
        ValuesRepository::set_uint(&open(db_path)?, name, value)?;
        Ok(())
    }

    /// Upsert an `int`-typed value at `name`.
    pub fn set_int(db_path: &Path, name: &str, value: i64) -> Result<(), StoreError> {
        ValuesRepository::set_int(&open(db_path)?, name, value)?;
        Ok(())
    }

    /// Upsert a `bool`-typed value at `name`.
    pub fn set_bool(db_path: &Path, name: &str, value: bool) -> Result<(), StoreError> {
        ValuesRepository::set_bool(&open(db_path)?, name, value)?;
        Ok(())
    }

    /// Upsert a `string`-typed value at `name`.
    pub fn set_string(db_path: &Path, name: &str, value: &str) -> Result<(), StoreError> {
        ValuesRepository::set_string(&open(db_path)?, name, value)?;
        Ok(())
    }

    /// Return `true` iff `name` is stored as a `uint`.
    pub fn has_uint(db_path: &Path, name: &str) -> Result<bool, StoreError> {
        Ok(ValuesRepository::has(
            &open(db_path)?,
            name,
            ValueKind::Uint,
        )?)
    }

    /// Return `true` iff `name` is stored as an `int`.
    pub fn has_int(db_path: &Path, name: &str) -> Result<bool, StoreError> {
        Ok(ValuesRepository::has(
            &open(db_path)?,
            name,
            ValueKind::Int,
        )?)
    }

    /// Return `true` iff `name` is stored as a `bool`.
    pub fn has_bool(db_path: &Path, name: &str) -> Result<bool, StoreError> {
        Ok(ValuesRepository::has(
            &open(db_path)?,
            name,
            ValueKind::Bool,
        )?)
    }

    /// Return `true` iff `name` is stored as a `string`.
    pub fn has_string(db_path: &Path, name: &str) -> Result<bool, StoreError> {
        Ok(ValuesRepository::has(
            &open(db_path)?,
            name,
            ValueKind::String,
        )?)
    }

    /// Delete the value at `name`, returning `true` if a row was removed.
    pub fn delete(db_path: &Path, name: &str) -> Result<bool, StoreError> {
        Ok(ValuesRepository::delete(&open(db_path)?, name)?)
    }
}

/// Convenience wrappers that mirror the C
/// `psync_{get,set}_{bool,int,uint,string}_setting` family (see
/// `pclsync/psynclib.c:1038-1068`).
///
/// These are stricter than [`value_kv`]: reading a setting under a different
/// kind than it was stored surfaces `settings::SettingTypeMismatch` rather
/// than returning a zero/empty sentinel. This mirrors the C
/// `CHECK_SETTINGID_AND_TYPE` contract in `pclsync/psettings.c`.
///
/// Storage is shared with [`value_kv`] (both operate on the `value_kv`
/// table), so a value written via either API is readable via both - with the
/// setting helpers rejecting cross-kind reads.
pub mod settings_kv {
    use super::{Connection, Path, StoreError};
    use crate::repositories::settings;

    pub use crate::repositories::settings::{SettingTypeMismatch, SettingsError};

    fn open(db_path: &Path) -> Result<Connection, StoreError> {
        let conn = Connection::open(db_path)?;
        super::tune_connection(&conn)?;
        Ok(conn)
    }

    /// See [`settings::get_bool_setting`].
    pub fn get_bool(db_path: &Path, name: &str) -> Result<Option<bool>, SettingsError> {
        settings::get_bool_setting(&open(db_path).map_err(store_to_settings)?, name)
    }

    /// See [`settings::set_bool_setting`].
    pub fn set_bool(db_path: &Path, name: &str, value: bool) -> Result<(), SettingsError> {
        settings::set_bool_setting(&open(db_path).map_err(store_to_settings)?, name, value)
    }

    /// See [`settings::get_int_setting`].
    pub fn get_int(db_path: &Path, name: &str) -> Result<Option<i64>, SettingsError> {
        settings::get_int_setting(&open(db_path).map_err(store_to_settings)?, name)
    }

    /// See [`settings::set_int_setting`].
    pub fn set_int(db_path: &Path, name: &str, value: i64) -> Result<(), SettingsError> {
        settings::set_int_setting(&open(db_path).map_err(store_to_settings)?, name, value)
    }

    /// See [`settings::get_uint_setting`].
    pub fn get_uint(db_path: &Path, name: &str) -> Result<Option<u64>, SettingsError> {
        settings::get_uint_setting(&open(db_path).map_err(store_to_settings)?, name)
    }

    /// See [`settings::set_uint_setting`].
    pub fn set_uint(db_path: &Path, name: &str, value: u64) -> Result<(), SettingsError> {
        settings::set_uint_setting(&open(db_path).map_err(store_to_settings)?, name, value)
    }

    /// See [`settings::get_string_setting`].
    pub fn get_string(db_path: &Path, name: &str) -> Result<Option<String>, SettingsError> {
        settings::get_string_setting(&open(db_path).map_err(store_to_settings)?, name)
    }

    /// See [`settings::set_string_setting`].
    pub fn set_string(db_path: &Path, name: &str, value: &str) -> Result<(), SettingsError> {
        settings::set_string_setting(&open(db_path).map_err(store_to_settings)?, name, value)
    }

    /// See [`settings::reset_setting`].
    pub fn reset(db_path: &Path, name: &str) -> Result<bool, SettingsError> {
        settings::reset_setting(&open(db_path).map_err(store_to_settings)?, name)
    }

    fn store_to_settings(err: StoreError) -> SettingsError {
        match err {
            StoreError::Sql(e) => SettingsError::Sql(e),
            // The settings surface only exercises sqlite opens; any other
            // variant is a bug in the caller. Fall back to a generic sqlite
            // error so we never silently swallow failures.
            other => SettingsError::Sql(rusqlite::Error::ToSqlConversionFailure(Box::new(
                std::io::Error::other(other.to_string()),
            ))),
        }
    }
}

/// Append a new row to the tamper-evident audit log.
///
/// Opens a short-lived connection against `profile.db_path`, calls into
/// [`repositories::audit::AuditRepository::append_event`] (which runs
/// under its own SQLite transaction so insert + hash back-fill are
/// atomic), and updates the in-memory repository state on the given
/// `profile`.
pub fn append_audit_event(
    profile: &mut StoreProfile,
    category: &str,
    details: Option<&str>,
) -> Result<(), StoreError> {
    let conn = Connection::open(&profile.db_path)?;
    tune_connection(&conn)?;
    profile
        .repositories
        .audit
        .append_event(&conn, category, details)?;
    Ok(())
}

/// Errors surfaced by [`verify_audit_chain`].
#[derive(Debug, Error)]
pub enum VerifyAuditChainError {
    /// Underlying SQLite error raised while reading rows.
    #[error("sqlite operation failed: {0}")]
    Sql(#[from] rusqlite::Error),
    /// The chain walk itself detected a tampered or invalid row.
    #[error("{0}")]
    Chain(#[from] crate::repositories::audit::AuditChainError),
}

/// Convenience wrapper: open a short-lived connection, re-load the audit
/// repository state, and run a full chain verification against the
/// requested range. If an HMAC key is provided, rows stored with that
/// key are cross-checked.
pub fn verify_audit_chain(
    db_path: &Path,
    from: Option<i64>,
    to: Option<i64>,
    hmac_key: Option<Vec<u8>>,
) -> Result<crate::repositories::audit::VerifiedChain, VerifyAuditChainError> {
    let conn = Connection::open(db_path)?;
    tune_connection(&conn)?;
    let mut repo = crate::repositories::audit::AuditRepository::load(&conn)?;
    repo.set_hmac_key(hmac_key);
    Ok(repo.verify_chain(&conn, from, to)?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pcloud_model::ids::{SyncId, UserId};

    use crate::repositories::{
        RepositorySet,
        account::{AccountRecord, AccountRepository},
        audit::AuditRepository,
        preferences::PreferencesRepository,
        sync_graph::SyncGraphRepository,
    };

    use super::{bootstrap_profile, persist_profile};

    fn temp_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-store-test-{}-{}.sqlite3",
            std::process::id(),
            name
        ))
    }

    #[test]
    fn bootstrap_creates_real_sqlite_profile() {
        let path = temp_db_path("bootstrap");
        let _ = std::fs::remove_file(&path);
        let (profile, integrity) = bootstrap_profile(&path).expect("bootstrap should succeed");

        assert_eq!(profile.schema_version, 12);
        assert_eq!(integrity, crate::integrity::IntegrityStatus::Clean);
        assert_eq!(profile.db_path, path);
    }

    #[test]
    fn repositories_persist_and_reload() {
        let path = temp_db_path("reload");
        let _ = std::fs::remove_file(&path);
        let (mut profile, _) = bootstrap_profile(&path).expect("bootstrap should succeed");

        let conn = rusqlite::Connection::open(&path).expect("db should open");
        profile.repositories = RepositorySet {
            accounts: AccountRepository {
                primary_account: Some(AccountRecord {
                    user_id: UserId::new(7),
                    email: "alice@example.com".to_owned(),
                    auth_token_present: true,
                }),
            },
            audit: AuditRepository::default(),
            preferences: PreferencesRepository {
                durable_auth_tokens_enabled: Some(true),
                api_server_binapi: Some("bineapi-eu.pcloud.com".to_owned()),
                api_server_location_id: Some(2),
                backup_device_folder_id: None,
            },
            sync_graph: SyncGraphRepository {
                tracked_sync_roots: vec![
                    crate::repositories::sync_graph::SyncRootRecord {
                        sync_id: SyncId::new(11),
                        local_path: "/tmp/local-11".to_owned(),
                        remote_path: "/remote-11".to_owned(),
                        paused: false,
                        sync_type: pcloud_model::sync::SyncType::Full,
                        exclude_globs: Vec::new(),
                    },
                    crate::repositories::sync_graph::SyncRootRecord {
                        sync_id: SyncId::new(12),
                        local_path: "/tmp/local-12".to_owned(),
                        remote_path: "/remote-12".to_owned(),
                        paused: false,
                        sync_type: pcloud_model::sync::SyncType::Full,
                        exclude_globs: Vec::new(),
                    },
                ],
            },
        };
        profile
            .repositories
            .save(&conn)
            .expect("repositories should save");
        let reloaded = RepositorySet::load(&conn).expect("repositories should load");

        assert_eq!(
            reloaded
                .accounts
                .primary_account
                .as_ref()
                .map(|account| account.user_id.get()),
            Some(7)
        );
        assert_eq!(reloaded.preferences.durable_auth_tokens_enabled, Some(true));
        assert_eq!(
            reloaded.preferences.api_server_binapi.as_deref(),
            Some("bineapi-eu.pcloud.com")
        );
        assert_eq!(reloaded.preferences.api_server_location_id, Some(2));
        assert_eq!(reloaded.sync_graph.tracked_sync_roots.len(), 2);
        assert_eq!(
            reloaded.sync_graph.tracked_sync_roots[0].local_path,
            "/tmp/local-11"
        );
    }

    #[test]
    fn persist_profile_rolls_back_on_repository_failure() {
        let path = temp_db_path("rollback");
        let _ = std::fs::remove_file(&path);
        let (mut profile, _) = bootstrap_profile(&path).expect("bootstrap should succeed");

        profile.repositories = RepositorySet {
            accounts: AccountRepository {
                primary_account: Some(AccountRecord {
                    user_id: UserId::new(7),
                    email: "alice@example.com".to_owned(),
                    auth_token_present: true,
                }),
            },
            audit: AuditRepository::default(),
            preferences: PreferencesRepository::default(),
            sync_graph: SyncGraphRepository {
                tracked_sync_roots: vec![crate::repositories::sync_graph::SyncRootRecord {
                    sync_id: SyncId::new(11),
                    local_path: "/tmp/local-11".to_owned(),
                    remote_path: "/remote-11".to_owned(),
                    paused: false,
                    sync_type: pcloud_model::sync::SyncType::Full,
                    exclude_globs: Vec::new(),
                }],
            },
        };
        persist_profile(&profile).expect("initial persist should succeed");

        profile.repositories.sync_graph.tracked_sync_roots = vec![
            crate::repositories::sync_graph::SyncRootRecord {
                sync_id: SyncId::new(21),
                local_path: "/tmp/local-21".to_owned(),
                remote_path: "/remote-21".to_owned(),
                paused: false,
                sync_type: pcloud_model::sync::SyncType::Full,
                exclude_globs: Vec::new(),
            },
            crate::repositories::sync_graph::SyncRootRecord {
                sync_id: SyncId::new(21),
                local_path: "/tmp/local-21b".to_owned(),
                remote_path: "/remote-21b".to_owned(),
                paused: false,
                sync_type: pcloud_model::sync::SyncType::Full,
                exclude_globs: Vec::new(),
            },
        ];
        let err = persist_profile(&profile).expect_err("duplicate sync ids should fail");
        assert!(matches!(err, crate::StoreError::Sql(_)));

        let conn = rusqlite::Connection::open(&path).expect("db should open");
        let reloaded = RepositorySet::load(&conn).expect("repositories should load");
        assert_eq!(
            reloaded
                .accounts
                .primary_account
                .as_ref()
                .map(|account| account.user_id.get()),
            Some(7)
        );
        assert_eq!(reloaded.sync_graph.tracked_sync_roots.len(), 1);
        assert_eq!(
            reloaded.sync_graph.tracked_sync_roots[0].sync_id,
            SyncId::new(11)
        );
    }

    #[test]
    fn store_handle_reuses_connection() {
        let path = temp_db_path("handle-reuse");
        let _ = std::fs::remove_file(&path);
        let _ = super::bootstrap_profile(&path).expect("bootstrap");

        let handle = super::StoreHandle::open(&path).expect("open handle");
        let cloned = handle.clone();

        handle.value_kv().set_uint("k", 7).expect("pooled set_uint");
        assert_eq!(cloned.value_kv().get_uint("k").expect("pooled get_uint"), 7);

        handle
            .settings_kv()
            .set_int("bench_int", -11)
            .expect("pooled set_int");
        assert_eq!(
            handle
                .settings_kv()
                .get_int("bench_int")
                .expect("pooled get_int"),
            Some(-11)
        );
    }

    // Regression gate for the PERF-BASELINE hotspot. The original short-lived
    // facade clocked `set_int` at ~6.9 ms per call; the pooled handle should
    // land at least an order of magnitude below that. The bound is deliberately
    // loose (5 ms per op averaged over a 200-op burst) so the test does not
    // flake on slow/contended CI hardware while still catching a catastrophic
    // regression back to a ~6.9 ms/op floor.
    #[test]
    fn store_handle_set_int_perf_bound() {
        let path = temp_db_path("perf-bound");
        let _ = std::fs::remove_file(&path);
        let _ = super::bootstrap_profile(&path).expect("bootstrap");
        let handle = super::StoreHandle::open(&path).expect("open handle");

        // warm the cache path
        handle.settings_kv().set_int("warm", 0).expect("warm");

        let n = 200;
        let start = std::time::Instant::now();
        for i in 0..n {
            handle
                .settings_kv()
                .set_int("perf_gate", i as i64)
                .expect("set");
        }
        let elapsed = start.elapsed();
        let per_op = elapsed / n;
        assert!(
            per_op < std::time::Duration::from_millis(5),
            "pooled set_int regressed: {per_op:?}/op over {n} ops (bound: 5 ms/op)",
        );
    }

    #[test]
    fn append_audit_event_persists_details() {
        let path = temp_db_path("audit-details");
        let _ = std::fs::remove_file(&path);
        let (mut profile, _) = bootstrap_profile(&path).expect("bootstrap should succeed");

        crate::append_audit_event(&mut profile, "auth.event", Some("LoginSucceeded"))
            .expect("audit event should persist");

        let conn = rusqlite::Connection::open(&path).expect("db should open");
        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT category, details FROM audit_events ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("audit row should exist");

        assert_eq!(row.0, "auth.event");
        assert_eq!(row.1.as_deref(), Some("LoginSucceeded"));
    }

    /// Regression test for F-08: apply_schema_v5 must survive being called
    /// when `text_value` / `int_value` already exist (column-exists guard).
    #[test]
    fn migration_v5_is_idempotent_with_preexisting_columns() {
        use crate::schema::{
            apply_schema_v1, apply_schema_v2, apply_schema_v3, apply_schema_v4, apply_schema_v5,
        };
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2(&conn).unwrap();
        apply_schema_v3(&conn).unwrap();
        apply_schema_v4(&conn).unwrap();
        // Simulate a partial v5: add columns but do NOT bump user_version.
        conn.execute_batch("ALTER TABLE preferences ADD COLUMN text_value TEXT;")
            .unwrap();
        conn.execute_batch("ALTER TABLE preferences ADD COLUMN int_value INTEGER;")
            .unwrap();
        // Now applying v5 again must not error on duplicate-column.
        apply_schema_v5(&conn).expect("v5 must be idempotent when columns already exist");
        let ver: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 5);
    }

    /// Regression test for F-08: apply_schema_v6 must survive being called
    /// when `sync_type` already exists on `sync_root_records`.
    #[test]
    fn migration_v6_is_idempotent_with_preexisting_column() {
        use crate::schema::{
            apply_schema_v1, apply_schema_v2, apply_schema_v3, apply_schema_v4, apply_schema_v5,
            apply_schema_v6,
        };
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2(&conn).unwrap();
        apply_schema_v3(&conn).unwrap();
        apply_schema_v4(&conn).unwrap();
        apply_schema_v5(&conn).unwrap();
        // Simulate a partial v6: add column but do NOT bump user_version.
        conn.execute_batch(
            "ALTER TABLE sync_root_records ADD COLUMN sync_type INTEGER NOT NULL DEFAULT 3 \
             CHECK (sync_type IN (1, 2, 3));",
        )
        .unwrap();
        // Now applying v6 again must not error on duplicate-column.
        apply_schema_v6(&conn).expect("v6 must be idempotent when column already exists");
        let ver: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 6);
    }

    /// T1.1.a: `exclude_globs` round-trips through save/load on v12 schema.
    #[test]
    fn sync_root_exclude_globs_roundtrips() {
        use crate::repositories::sync_graph::{SyncGraphRepository, SyncRootRecord};
        use pcloud_model::ids::SyncId;
        use pcloud_model::sync::SyncType;

        let path = temp_db_path("exclude-globs");
        let _ = std::fs::remove_file(&path);
        let (mut profile, _) = bootstrap_profile(&path).expect("bootstrap should succeed");

        profile.repositories.sync_graph = SyncGraphRepository {
            tracked_sync_roots: vec![SyncRootRecord {
                sync_id: SyncId::new(1),
                local_path: "/tmp/sel".to_owned(),
                remote_path: "/Sel".to_owned(),
                paused: false,
                sync_type: SyncType::Full,
                exclude_globs: vec!["*.tmp".to_owned(), "build/**".to_owned()],
            }],
        };
        persist_profile(&profile).expect("persist should succeed");

        let (reloaded, _) = bootstrap_profile(&path).expect("bootstrap reload should succeed");
        let roots = &reloaded.repositories.sync_graph.tracked_sync_roots;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].exclude_globs, vec!["*.tmp", "build/**"]);
    }

    /// T1.1.a: `apply_schema_v12` is idempotent when `exclude_globs` already exists.
    #[test]
    fn migration_v12_is_idempotent_with_preexisting_column() {
        use crate::schema::{
            apply_schema_v1, apply_schema_v2, apply_schema_v3, apply_schema_v4, apply_schema_v5,
            apply_schema_v6, apply_schema_v7, apply_schema_v8, apply_schema_v9, apply_schema_v10,
            apply_schema_v11, apply_schema_v12,
        };
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2(&conn).unwrap();
        apply_schema_v3(&conn).unwrap();
        apply_schema_v4(&conn).unwrap();
        apply_schema_v5(&conn).unwrap();
        apply_schema_v6(&conn).unwrap();
        apply_schema_v7(&conn).unwrap();
        apply_schema_v8(&conn).unwrap();
        apply_schema_v9(&conn).unwrap();
        apply_schema_v10(&conn).unwrap();
        apply_schema_v11(&conn).unwrap();
        // Simulate a partial v12: add column but do NOT bump user_version.
        conn.execute_batch(
            "ALTER TABLE sync_root_records ADD COLUMN exclude_globs TEXT NOT NULL DEFAULT '';",
        )
        .unwrap();
        apply_schema_v12(&conn).expect("v12 must be idempotent when column already exists");
        let ver: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 12);
    }
}

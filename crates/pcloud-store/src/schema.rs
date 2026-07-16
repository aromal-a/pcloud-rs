// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::{Connection, OptionalExtension};

/// Bootstrap schema version: creates `account`, `audit_events`, and `sync_roots`.
pub const SCHEMA_VERSION_V1: u32 = 1;
/// Adds the `details` column to `audit_events` for structured event payloads.
pub const SCHEMA_VERSION_V2: u32 = 2;
/// Replaces the legacy `sync_roots` id-only table with the full `sync_root_records` row.
pub const SCHEMA_VERSION_V3: u32 = 3;
/// Adds the `preferences` key/value table (initial bool-only shape).
pub const SCHEMA_VERSION_V4: u32 = 4;
/// Extends `preferences` with `text_value` and `int_value` typed columns.
pub const SCHEMA_VERSION_V5: u32 = 5;
/// Adds per-sync-root `sync_type` mirroring the C `psync_synctype_t` enum.
pub const SCHEMA_VERSION_V6: u32 = 6;
/// Adds the open `value_kv` schemaless typed key/value table.
pub const SCHEMA_VERSION_V7: u32 = 7;
/// Upgrades `audit_events` to a SHA-256 hash-chained, optionally HMAC-signed log.
pub const SCHEMA_VERSION_V8: u32 = 8;
/// Adds the `upload_resume_state` table for chunked upload client-tracked offsets.
pub const SCHEMA_VERSION_V9: u32 = 9;
/// Adds the `sync_diff_state` table for per-sync persisted `diffid` cursors.
pub const SCHEMA_VERSION_V10: u32 = 10;
/// Adds the `file_metadata` table for local file/folder metadata cache.
pub const SCHEMA_VERSION_V11: u32 = 11;
/// Adds `sync_root_records.exclude_globs` for selective sync (T1.1).
pub const SCHEMA_VERSION_V12: u32 = 12;

/// Returns a stable diagnostic name for the current schema target, used in logs.
#[must_use]
pub fn schema_name() -> &'static str {
    "store-schema-v12"
}

/// Apply schema version 1: creates `account`, `audit_events`, and `sync_roots`.
pub fn apply_schema_v1(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS account (
            primary_account INTEGER PRIMARY KEY CHECK (primary_account = 1),
            user_id INTEGER NOT NULL,
            email TEXT NOT NULL,
            auth_token_present INTEGER NOT NULL CHECK (auth_token_present IN (0, 1))
        );

        CREATE TABLE IF NOT EXISTS audit_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            category TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS sync_roots (
            sync_id INTEGER PRIMARY KEY
        );

        PRAGMA user_version = 1;
        ",
    )
}

/// Apply schema version 2: adds `audit_events.details TEXT` for structured payloads.
///
/// Idempotent: skips the `ALTER TABLE` if the column already exists (same
/// guard used by the v8 hash-chain migration).
pub fn apply_schema_v2(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "audit_events", "details")? {
        conn.execute_batch("ALTER TABLE audit_events ADD COLUMN details TEXT;")?;
    }
    conn.execute_batch("PRAGMA user_version = 2;")
}

/// Apply schema version 3: creates `sync_root_records` with `(sync_id, local_path, remote_path, paused)`.
pub fn apply_schema_v3(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sync_root_records (
            sync_id INTEGER PRIMARY KEY,
            local_path TEXT NOT NULL,
            remote_path TEXT NOT NULL,
            paused INTEGER NOT NULL CHECK (paused IN (0, 1))
        );

        PRAGMA user_version = 3;
        ",
    )
}

/// Apply schema version 4: creates the initial `preferences (name, bool_value)` table.
pub fn apply_schema_v4(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS preferences (
            name TEXT PRIMARY KEY,
            bool_value INTEGER CHECK (bool_value IN (0, 1))
        );

        PRAGMA user_version = 4;
        ",
    )
}

/// Apply schema version 5: extends `preferences` with `text_value` and `int_value` columns.
///
/// Idempotent: each `ALTER TABLE` is guarded by a column-existence check so a
/// partial migration that added a column but crashed before bumping
/// `user_version` does not brick the next startup with a duplicate-column error.
pub fn apply_schema_v5(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "preferences", "text_value")? {
        conn.execute_batch("ALTER TABLE preferences ADD COLUMN text_value TEXT;")?;
    }
    if !column_exists(conn, "preferences", "int_value")? {
        conn.execute_batch("ALTER TABLE preferences ADD COLUMN int_value INTEGER;")?;
    }
    conn.execute_batch("PRAGMA user_version = 5;")
}

/// Schema v6 carries per-sync-root `sync_type` (mirrors C `psync_synctype_t`).
///
/// The column is added with a default of `3` (full sync) so that pre-existing
/// sync roots keep their current behavior after migration.
///
/// Idempotent: the `ALTER TABLE` is guarded by a column-existence check so a
/// partial migration that added the column but crashed before bumping
/// `user_version` does not brick the next startup with a duplicate-column error.
pub fn apply_schema_v6(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "sync_root_records", "sync_type")? {
        conn.execute_batch(
            "ALTER TABLE sync_root_records ADD COLUMN sync_type INTEGER NOT NULL DEFAULT 3 \
             CHECK (sync_type IN (1, 2, 3));",
        )?;
    }
    conn.execute_batch("PRAGMA user_version = 6;")
}

/// Schema v7 adds a typed key/value table that mirrors the C `setting` table
/// consumed by the `psync_{get,set,has}_{bool,int,uint,string}_value` helper
/// family. Unlike the `preferences` table (which stores a small, fixed set of
/// strongly-named daemon preferences), `value_kv` is an open schemaless key
/// space used by arbitrary callers - so we keep it as its own repository to
/// avoid collisions with the preferences repository.
pub fn apply_schema_v7(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS value_kv (
            name TEXT PRIMARY KEY,
            kind INTEGER NOT NULL CHECK (kind IN (1, 2, 3, 4)),
            int_value INTEGER,
            text_value TEXT
        );

        PRAGMA user_version = 7;
        ",
    )
}

/// Schema v8 upgrades the `audit_events` table to a tamper-evident
/// hash chain:
///
/// * `prev_hash BLOB` – SHA-256 of the previous entry (`NULL` / zero-filled
///   for the genesis row).
/// * `entry_hash BLOB` – SHA-256(`prev_hash || serialize(event)`) for this
///   row.
/// * `hmac BLOB` – optional HMAC-SHA256 over `entry_hash` when the daemon
///   was started with `PCLOUD_AUDIT_HMAC_KEY` provisioned.
///
/// The migration is idempotent and data-preserving: any existing rows are
/// re-hashed in insertion order so the chain is valid after upgrade. If
/// the columns already exist (migration was previously applied), the
/// `ALTER TABLE` statements are skipped and we simply re-assert the
/// `PRAGMA user_version`.
///
/// Because re-computation only reads `id || category || created_at ||
/// details`, pre-existing audit rows retain their original semantics; only
/// the hash-chain columns change.
pub fn apply_schema_v8(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Idempotent column adds - detect whether the new columns already exist.
    let has_prev_hash = column_exists(conn, "audit_events", "prev_hash")?;
    let has_entry_hash = column_exists(conn, "audit_events", "entry_hash")?;
    let has_hmac = column_exists(conn, "audit_events", "hmac")?;

    if !has_prev_hash {
        conn.execute_batch("ALTER TABLE audit_events ADD COLUMN prev_hash BLOB;")?;
    }
    if !has_entry_hash {
        conn.execute_batch("ALTER TABLE audit_events ADD COLUMN entry_hash BLOB;")?;
    }
    if !has_hmac {
        conn.execute_batch("ALTER TABLE audit_events ADD COLUMN hmac BLOB;")?;
    }

    // Recompute hashes for every existing row, in insertion (id) order, so
    // the chain is internally consistent after migration. We deliberately
    // do NOT compute the HMAC here - HMAC is only emitted when a live
    // daemon has a key provisioned at runtime; recomputing it at migration
    // time without a key would be meaningless.
    crate::repositories::audit::rebuild_hash_chain(conn)?;

    conn.execute_batch("PRAGMA user_version = 8;")?;
    Ok(())
}

/// Schema v9 adds the `upload_resume_state` table used by the upload state
/// machine to persist client-tracked offsets for chunked uploads (mirrors
/// the legacy C `localfileupload` table — see
/// `pclsync/pupload.c:1017-1022`).
///
/// Columns:
///
/// * `local_path` — canonicalized absolute path of the local file being
///   uploaded. Primary key; one in-flight upload per local path.
/// * `parent_folder_id` — remote parent folder id (`folderid`).
/// * `file_name` — target remote file name.
/// * `upload_id` — server-assigned upload handle from `upload_create`.
/// * `offset` — last locally-confirmed uploaded byte count.
/// * `total_size` — total size in bytes of the local file at create time.
/// * `prefix_sha1` — lowercase hex SHA-1 of the prefix `[0, offset)` used
///   to prove the server prefix still matches the local file on resume.
///   Nullable when offset is 0.
/// * `if_hash` — optional numeric conflict hint (`ifhash` numeric param).
///   Mutually exclusive with `if_new` in practice.
/// * `if_new` — `1` when the conflict param was the `"new"` sentinel.
/// * `updated_at` — unix timestamp (seconds) of the last update.
pub fn apply_schema_v9(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS upload_resume_state (
            local_path TEXT PRIMARY KEY,
            parent_folder_id INTEGER NOT NULL,
            file_name TEXT NOT NULL,
            upload_id INTEGER NOT NULL,
            offset INTEGER NOT NULL CHECK (offset >= 0),
            total_size INTEGER NOT NULL CHECK (total_size >= 0),
            prefix_sha1 TEXT,
            if_hash INTEGER,
            if_new INTEGER NOT NULL DEFAULT 0 CHECK (if_new IN (0, 1)),
            updated_at INTEGER NOT NULL
        );

        PRAGMA user_version = 9;
        ",
    )
}

/// Schema v10 adds the `sync_diff_state` table used by the daemon's
/// `DiffWorker` to persist the current `diffid` cursor per sync root
/// across daemon restarts (mirrors `pclsync/pdiff.c:2540` which reads
/// `setting.diffid`).
///
/// Columns:
///
/// * `sync_id` — primary key. Foreign key in spirit to
///   `sync_root_records.sync_id`; we do not declare a real FK because
///   diff state can outlive a transient sync_root remove/re-add and the
///   row is truncated explicitly when a sync root is fully evicted.
/// * `diffid` — last successfully-processed diffid (`u64`). 0 means
///   "never processed; next diff is initial".
/// * `updated_at` — unix timestamp (seconds) of the last advance.
pub fn apply_schema_v10(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sync_diff_state (
            sync_id INTEGER PRIMARY KEY,
            diffid INTEGER NOT NULL CHECK (diffid >= 0),
            updated_at INTEGER NOT NULL
        );

        PRAGMA user_version = 10;
        ",
    )
}

/// Schema v11 adds the `file_metadata` table used as a local cache of
/// remote file/folder metadata populated by the diff engine. This mirrors
/// the C `pfolder.c` / `pfile.c` in-memory metadata cache that backs
/// `psync_stat_path` (`pclsync/psynclib.h:743`).
///
/// Columns:
///
/// * `file_id` — primary key. Remote file id (for files) or folder id
///   (for folders). This is the pCloud-assigned numeric id.
/// * `parent_folder_id` — parent folder's remote id. `0` for root.
/// * `name` — leaf entry name (not the full path).
/// * `size` — size in bytes. `0` for folders.
/// * `hash` — content hash hex string. Empty for folders.
/// * `modified` — last-modified timestamp (unix seconds).
/// * `created` — creation timestamp (unix seconds).
/// * `is_folder` — `1` for folders, `0` for files.
pub fn apply_schema_v11(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS file_metadata (
            file_id INTEGER PRIMARY KEY,
            parent_folder_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            size INTEGER NOT NULL DEFAULT 0,
            hash TEXT NOT NULL DEFAULT '',
            modified INTEGER NOT NULL DEFAULT 0,
            created INTEGER NOT NULL DEFAULT 0,
            is_folder INTEGER NOT NULL CHECK (is_folder IN (0, 1))
        );

        CREATE INDEX IF NOT EXISTS idx_file_metadata_parent
            ON file_metadata (parent_folder_id, name);

        PRAGMA user_version = 11;
        ",
    )
}

/// Schema v12 adds `sync_root_records.exclude_globs` (T1.1 selective sync).
///
/// Stores a newline-separated list of glob patterns. Empty string means
/// "no excludes" (the default for pre-v12 rows). Engine planners
/// consult the patterns and skip matching files on the next pass.
///
/// Idempotent: the `ALTER TABLE` is guarded by a column-existence check
/// so a partial migration that added the column but crashed before
/// bumping `user_version` does not brick startup with a duplicate-column
/// error.
pub fn apply_schema_v12(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "sync_root_records", "exclude_globs")? {
        conn.execute_batch(
            "ALTER TABLE sync_root_records ADD COLUMN exclude_globs TEXT NOT NULL DEFAULT '';",
        )?;
    }
    conn.execute_batch("PRAGMA user_version = 12;")
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Return the current `PRAGMA user_version` of the database.
pub fn read_schema_version(conn: &Connection) -> Result<u32, rusqlite::Error> {
    conn.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
}

/// Returns `true` if the expected bootstrap `account` table is already present.
///
/// Used by `bootstrap_profile` to distinguish a fresh database from a pre-existing one.
pub fn schema_exists(conn: &Connection) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='account' LIMIT 1",
        [],
        |row| row.get::<_, u8>(0),
    )
    .optional()
    .map(|value| value.is_some())
}

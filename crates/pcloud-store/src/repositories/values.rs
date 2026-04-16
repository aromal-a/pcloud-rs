//! Typed key/value helpers that mirror the C `psync_{get,set,has}_*_value`
//! family declared in `pclsync/psynclib.h` and implemented in
//! `pclsync/psynclib.c` (rows 1089-1151).
//!
//! The C layer stores everything in a single `setting(id TEXT, value BLOB)`
//! table and reads it back with loose casts (bool = !!uint, int = signed cast
//! of uint, string = TEXT). We keep the same contract at the API boundary but
//! persist with an explicit type tag so that a caller cannot read back a
//! different type than was stored without being told.
//!
//! Semantics preserved from C:
//!
//! * `get_uint`/`get_int`/`get_bool` return `0` / `false` when the key is
//!   missing. `get_string` returns `None` when the key is missing.
//! * `set_bool` stores `0` or `1` (C does `!!value`).
//! * `set_int` / `get_int` reinterpret the same underlying 64-bit slot as
//!   signed (C does `(int64_t)uint64_t`).
//! * `has_*_value` is true only when a value is present AND stored under the
//!   requested kind, which is stricter than C (C has no `has_*_value`
//!   helpers - these exist to give the Rust API the presence check the C
//!   caller emulates with `if (psync_get_..._value(k))`).

// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::{Connection, OptionalExtension};

/// Persisted type tag for a `value_kv` row.
///
/// Kept stable - values are part of the on-disk schema.
#[repr(i64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// Boolean value stored as `0` / `1` in `int_value`.
    Bool = 1,
    /// Unsigned 64-bit value stored in `int_value` (C bit-reinterpret).
    Uint = 2,
    /// Signed 64-bit value stored in `int_value` (C bit-reinterpret).
    Int = 3,
    /// UTF-8 string stored in `text_value`.
    String = 4,
}

impl ValueKind {
    fn as_i64(self) -> i64 {
        self as i64
    }

    fn from_i64(raw: i64) -> Option<Self> {
        match raw {
            1 => Some(ValueKind::Bool),
            2 => Some(ValueKind::Uint),
            3 => Some(ValueKind::Int),
            4 => Some(ValueKind::String),
            _ => None,
        }
    }
}

/// Stateless accessor for the `value_kv` table.
///
/// Unlike the `preferences` repository this has no in-memory cache: the key
/// space is open-ended and arbitrary callers (daemon, SDK, CLI) may mutate it
/// concurrently, so we always round-trip to SQLite just like the C `setting`
/// table does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ValuesRepository;

impl ValuesRepository {
    /// Read `name` as a `u64`. Returns `0` when the key is missing, matching the C behavior.
    pub fn get_uint(conn: &Connection, name: &str) -> Result<u64, rusqlite::Error> {
        let row: Option<(i64, i64)> = conn
            .query_row(
                "SELECT kind, int_value FROM value_kv WHERE name = ?1",
                [name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    ))
                },
            )
            .optional()?;
        match row {
            Some((kind, raw)) if ValueKind::from_i64(kind).is_some() => Ok(raw as u64),
            _ => Ok(0),
        }
    }

    /// Read `name` as an `i64`. Returns `0` when the key is missing.
    pub fn get_int(conn: &Connection, name: &str) -> Result<i64, rusqlite::Error> {
        Self::get_uint(conn, name).map(|value| value as i64)
    }

    /// Read `name` as a `bool`. Returns `false` when the key is missing or stored as `0`.
    pub fn get_bool(conn: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
        Self::get_uint(conn, name).map(|value| value != 0)
    }

    /// Read `name` as a `String`. Returns `None` if the key is missing or not a string.
    pub fn get_string(conn: &Connection, name: &str) -> Result<Option<String>, rusqlite::Error> {
        conn.query_row(
            "SELECT kind, text_value FROM value_kv WHERE name = ?1",
            [name],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map(|row| match row {
            Some((kind, text)) if ValueKind::from_i64(kind) == Some(ValueKind::String) => text,
            _ => None,
        })
    }

    /// Upsert `name` as an unsigned 64-bit value. Overwrites any existing kind and clears `text_value`.
    pub fn set_uint(conn: &Connection, name: &str, value: u64) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT INTO value_kv (name, kind, int_value, text_value)
             VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(name) DO UPDATE SET
                 kind = excluded.kind,
                 int_value = excluded.int_value,
                 text_value = NULL",
            (name, ValueKind::Uint.as_i64(), value as i64),
        )?;
        Ok(())
    }

    /// Upsert `name` as a signed 64-bit value. Overwrites any existing kind and clears `text_value`.
    pub fn set_int(conn: &Connection, name: &str, value: i64) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT INTO value_kv (name, kind, int_value, text_value)
             VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(name) DO UPDATE SET
                 kind = excluded.kind,
                 int_value = excluded.int_value,
                 text_value = NULL",
            (name, ValueKind::Int.as_i64(), value),
        )?;
        Ok(())
    }

    /// Upsert `name` as a boolean. C-compatible: stores `0` / `1`.
    pub fn set_bool(conn: &Connection, name: &str, value: bool) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT INTO value_kv (name, kind, int_value, text_value)
             VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(name) DO UPDATE SET
                 kind = excluded.kind,
                 int_value = excluded.int_value,
                 text_value = NULL",
            (name, ValueKind::Bool.as_i64(), i64::from(value)),
        )?;
        Ok(())
    }

    /// Upsert `name` as a UTF-8 string. Overwrites any existing kind and clears `int_value`.
    pub fn set_string(conn: &Connection, name: &str, value: &str) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT INTO value_kv (name, kind, int_value, text_value)
             VALUES (?1, ?2, NULL, ?3)
             ON CONFLICT(name) DO UPDATE SET
                 kind = excluded.kind,
                 int_value = NULL,
                 text_value = excluded.text_value",
            (name, ValueKind::String.as_i64(), value),
        )?;
        Ok(())
    }

    /// Returns `true` only if `name` exists AND is stored under `expected`.
    ///
    /// Stricter than the C loose contract: a present-but-wrong-kind row reports `false`.
    pub fn has(
        conn: &Connection,
        name: &str,
        expected: ValueKind,
    ) -> Result<bool, rusqlite::Error> {
        let row: Option<i64> = conn
            .query_row("SELECT kind FROM value_kv WHERE name = ?1", [name], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
        Ok(matches!(row, Some(raw) if ValueKind::from_i64(raw) == Some(expected)))
    }

    /// Delete `name` from `value_kv`. Returns whether a row was removed.
    pub fn delete(conn: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
        let affected = conn.execute("DELETE FROM value_kv WHERE name = ?1", [name])?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_profile;

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-store-values-{}-{}-{}.sqlite3",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[test]
    fn missing_keys_yield_zero_or_none() {
        let path = temp_db_path("missing");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");

        assert_eq!(ValuesRepository::get_uint(&conn, "nope").unwrap(), 0);
        assert_eq!(ValuesRepository::get_int(&conn, "nope").unwrap(), 0);
        assert!(!ValuesRepository::get_bool(&conn, "nope").unwrap());
        assert!(
            ValuesRepository::get_string(&conn, "nope")
                .unwrap()
                .is_none()
        );
        assert!(!ValuesRepository::has(&conn, "nope", ValueKind::Uint).unwrap());
    }

    #[test]
    fn set_and_get_roundtrip_per_kind() {
        let path = temp_db_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");

        ValuesRepository::set_uint(&conn, "quota", u64::MAX).unwrap();
        assert_eq!(
            ValuesRepository::get_uint(&conn, "quota").unwrap(),
            u64::MAX
        );
        assert!(ValuesRepository::has(&conn, "quota", ValueKind::Uint).unwrap());

        ValuesRepository::set_int(&conn, "offset", -42).unwrap();
        assert_eq!(ValuesRepository::get_int(&conn, "offset").unwrap(), -42);

        ValuesRepository::set_bool(&conn, "owner", true).unwrap();
        assert!(ValuesRepository::get_bool(&conn, "owner").unwrap());
        ValuesRepository::set_bool(&conn, "owner", false).unwrap();
        assert!(!ValuesRepository::get_bool(&conn, "owner").unwrap());

        ValuesRepository::set_string(&conn, "user", "alice@example.com").unwrap();
        assert_eq!(
            ValuesRepository::get_string(&conn, "user")
                .unwrap()
                .as_deref(),
            Some("alice@example.com")
        );
        assert!(ValuesRepository::has(&conn, "user", ValueKind::String).unwrap());
        assert!(!ValuesRepository::has(&conn, "user", ValueKind::Uint).unwrap());
    }

    #[test]
    fn overwrite_changes_kind_and_clears_other_slot() {
        let path = temp_db_path("overwrite");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");

        ValuesRepository::set_string(&conn, "k", "hello").unwrap();
        ValuesRepository::set_uint(&conn, "k", 7).unwrap();
        assert_eq!(ValuesRepository::get_uint(&conn, "k").unwrap(), 7);
        // string reader must not see the stale text
        assert!(ValuesRepository::get_string(&conn, "k").unwrap().is_none());
    }

    #[test]
    fn delete_removes_the_row() {
        let path = temp_db_path("delete");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");

        ValuesRepository::set_uint(&conn, "doomed", 1).unwrap();
        assert!(ValuesRepository::delete(&conn, "doomed").unwrap());
        assert!(!ValuesRepository::delete(&conn, "doomed").unwrap());
        assert_eq!(ValuesRepository::get_uint(&conn, "doomed").unwrap(), 0);
    }
}

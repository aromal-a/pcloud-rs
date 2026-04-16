//! Typed settings helpers mirroring the C
//! `psync_{get,set}_{bool,int,uint,string}_setting` family declared in
//! `pclsync/psynclib.h:887-894` and implemented in `pclsync/psynclib.c:1038-1068`
//! + `pclsync/psettings.c`.
//!
//! The C settings API differs from the looser `psync_*_value` API on two
//! fronts:
//!
//! 1. The C code maintains a static `settings[]` array with a declared type per
//!    setting name and hard-rejects mismatched reads/writes via
//!    `CHECK_SETTINGID_AND_TYPE` (`pclsync/psettings.c:247`).
//! 2. Some settings invoke change-callbacks (pfs_remount, ptimer notify, etc.)
//!    on mutation. Those callbacks live in feature subsystems - this crate
//!    only owns persistence, so callers needing notification hook their side
//!    effects around these helpers.
//!
//! Storage is shared with [`crate::repositories::values::ValuesRepository`] on the
//! `value_kv` table (schema v7). The kind tags are reused so a setting written
//! via this module can be read back via `ValuesRepository` and vice versa -
//! mirroring the single `setting` table the C code uses. The difference is
//! that this module surfaces a `SettingTypeMismatch` error when a caller
//! reads a different kind than was stored, whereas `ValuesRepository` returns
//! a zero sentinel (matching the looser C `psync_*_value` behavior).

// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::{Connection, OptionalExtension};
use thiserror::Error;

use super::values::ValueKind;

/// Error surface for the strict settings helpers.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// The caller supplied an empty setting name; names must be non-empty.
    #[error("setting name must not be empty")]
    EmptyName,
    /// The setting exists but was written with a different [`ValueKind`].
    #[error(transparent)]
    TypeMismatch(#[from] SettingTypeMismatch),
    /// Underlying SQLite failure.
    #[error("sqlite operation failed: {0}")]
    Sql(#[from] rusqlite::Error),
}

/// Returned when a setting exists under a different kind than the accessor
/// requested. Equivalent to the C
/// `pdbg_logf(D_BUG, "invalid setting type requested...")` path in
/// `pclsync/psettings.c:253-257`, but surfaced explicitly to callers instead
/// of degraded to a zero/empty sentinel.
#[derive(Debug, Error)]
#[error("setting {name:?} has kind {stored:?}, not {requested:?}")]
pub struct SettingTypeMismatch {
    /// Setting name the caller attempted to read.
    pub name: String,
    /// The kind actually stored in `value_kv`.
    pub stored: ValueKind,
    /// The kind the caller asked for.
    pub requested: ValueKind,
}

fn check_name(name: &str) -> Result<(), SettingsError> {
    if name.is_empty() {
        Err(SettingsError::EmptyName)
    } else {
        Ok(())
    }
}

fn lookup_kind(conn: &Connection, name: &str) -> Result<Option<ValueKind>, rusqlite::Error> {
    let raw: Option<i64> = conn
        .query_row("SELECT kind FROM value_kv WHERE name = ?1", [name], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?;
    Ok(raw.and_then(|value| match value {
        1 => Some(ValueKind::Bool),
        2 => Some(ValueKind::Uint),
        3 => Some(ValueKind::Int),
        4 => Some(ValueKind::String),
        _ => None,
    }))
}

fn require_kind(
    conn: &Connection,
    name: &str,
    requested: ValueKind,
) -> Result<Option<ValueKind>, SettingsError> {
    let stored = lookup_kind(conn, name)?;
    match stored {
        None => Ok(None),
        Some(actual) if actual == requested => Ok(Some(actual)),
        Some(actual) => Err(SettingsError::TypeMismatch(SettingTypeMismatch {
            name: name.to_owned(),
            stored: actual,
            requested,
        })),
    }
}

/// Mirrors `psync_get_bool_setting`. Returns `Ok(None)` when the setting is
/// unset (callers mirroring C behavior should fall back to `false`).
pub fn get_bool_setting(conn: &Connection, name: &str) -> Result<Option<bool>, SettingsError> {
    check_name(name)?;
    if require_kind(conn, name, ValueKind::Bool)?.is_none() {
        return Ok(None);
    }
    let raw: Option<i64> = conn
        .query_row(
            "SELECT int_value FROM value_kv WHERE name = ?1",
            [name],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    Ok(Some(raw.unwrap_or(0) != 0))
}

/// Mirrors `psync_set_bool_setting`. C reduces arbitrary `int` to `0|1`;
/// Rust's `bool` is already normalized. Stores under `ValueKind::Bool` so that
/// a subsequent `get_int_setting` / `get_string_setting` surfaces
/// [`SettingTypeMismatch`].
pub fn set_bool_setting(conn: &Connection, name: &str, value: bool) -> Result<(), SettingsError> {
    check_name(name)?;
    conn.execute(
        "INSERT INTO value_kv (name, kind, int_value, text_value)
         VALUES (?1, ?2, ?3, NULL)
         ON CONFLICT(name) DO UPDATE SET
             kind = excluded.kind,
             int_value = excluded.int_value,
             text_value = NULL",
        (name, ValueKind::Bool as i64, i64::from(value)),
    )?;
    Ok(())
}

/// Mirrors `psync_get_int_setting`.
pub fn get_int_setting(conn: &Connection, name: &str) -> Result<Option<i64>, SettingsError> {
    check_name(name)?;
    if require_kind(conn, name, ValueKind::Int)?.is_none() {
        return Ok(None);
    }
    let raw: Option<i64> = conn
        .query_row(
            "SELECT int_value FROM value_kv WHERE name = ?1",
            [name],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    Ok(Some(raw.unwrap_or(0)))
}

/// Mirrors `psync_set_int_setting`.
pub fn set_int_setting(conn: &Connection, name: &str, value: i64) -> Result<(), SettingsError> {
    check_name(name)?;
    conn.execute(
        "INSERT INTO value_kv (name, kind, int_value, text_value)
         VALUES (?1, ?2, ?3, NULL)
         ON CONFLICT(name) DO UPDATE SET
             kind = excluded.kind,
             int_value = excluded.int_value,
             text_value = NULL",
        (name, ValueKind::Int as i64, value),
    )?;
    Ok(())
}

/// Mirrors `psync_get_uint_setting`. SQLite integers are signed 64-bit;
/// values with the high bit set are round-tripped via `i64 as u64`.
pub fn get_uint_setting(conn: &Connection, name: &str) -> Result<Option<u64>, SettingsError> {
    check_name(name)?;
    if require_kind(conn, name, ValueKind::Uint)?.is_none() {
        return Ok(None);
    }
    let raw: Option<i64> = conn
        .query_row(
            "SELECT int_value FROM value_kv WHERE name = ?1",
            [name],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    Ok(Some(raw.unwrap_or(0) as u64))
}

/// Mirrors `psync_set_uint_setting`.
pub fn set_uint_setting(conn: &Connection, name: &str, value: u64) -> Result<(), SettingsError> {
    check_name(name)?;
    conn.execute(
        "INSERT INTO value_kv (name, kind, int_value, text_value)
         VALUES (?1, ?2, ?3, NULL)
         ON CONFLICT(name) DO UPDATE SET
             kind = excluded.kind,
             int_value = excluded.int_value,
             text_value = NULL",
        (name, ValueKind::Uint as i64, value as i64),
    )?;
    Ok(())
}

/// Mirrors `psync_get_string_setting`. Returns `Ok(None)` when the setting is
/// unset. The C helper returns an empty-string sentinel; the Rust helper
/// distinguishes "absent" from "present but empty" so callers can decide
/// which default to apply.
pub fn get_string_setting(conn: &Connection, name: &str) -> Result<Option<String>, SettingsError> {
    check_name(name)?;
    if require_kind(conn, name, ValueKind::String)?.is_none() {
        return Ok(None);
    }
    let raw: Option<Option<String>> = conn
        .query_row(
            "SELECT text_value FROM value_kv WHERE name = ?1",
            [name],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?;
    Ok(raw.flatten().or_else(|| Some(String::new())))
}

/// Mirrors `psync_set_string_setting`.
pub fn set_string_setting(conn: &Connection, name: &str, value: &str) -> Result<(), SettingsError> {
    check_name(name)?;
    conn.execute(
        "INSERT INTO value_kv (name, kind, int_value, text_value)
         VALUES (?1, ?2, NULL, ?3)
         ON CONFLICT(name) DO UPDATE SET
             kind = excluded.kind,
             int_value = NULL,
             text_value = excluded.text_value",
        (name, ValueKind::String as i64, value),
    )?;
    Ok(())
}

/// Mirrors `psync_reset_setting` - drop the row so subsequent `get_*_setting`
/// calls return `Ok(None)` (i.e. the caller's default applies).
pub fn reset_setting(conn: &Connection, name: &str) -> Result<bool, SettingsError> {
    check_name(name)?;
    let affected = conn.execute("DELETE FROM value_kv WHERE name = ?1", [name])?;
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_profile;

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-store-settings-{}-{}-{}.sqlite3",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    fn open(name: &str) -> (std::path::PathBuf, Connection) {
        let path = temp_db_path(name);
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");
        (path, conn)
    }

    #[test]
    fn bool_roundtrip() {
        let (_p, conn) = open("bool");
        assert_eq!(get_bool_setting(&conn, "usessl").unwrap(), None);
        set_bool_setting(&conn, "usessl", true).unwrap();
        assert_eq!(get_bool_setting(&conn, "usessl").unwrap(), Some(true));
        set_bool_setting(&conn, "usessl", false).unwrap();
        assert_eq!(get_bool_setting(&conn, "usessl").unwrap(), Some(false));
    }

    #[test]
    fn int_roundtrip_signed() {
        let (_p, conn) = open("int");
        set_int_setting(&conn, "offset", -9000).unwrap();
        assert_eq!(get_int_setting(&conn, "offset").unwrap(), Some(-9000));
    }

    #[test]
    fn uint_preserves_high_bit() {
        let (_p, conn) = open("uint");
        set_uint_setting(&conn, "big", u64::MAX).unwrap();
        assert_eq!(get_uint_setting(&conn, "big").unwrap(), Some(u64::MAX));
    }

    #[test]
    fn string_roundtrip_distinguishes_empty_from_absent() {
        let (_p, conn) = open("string");
        assert_eq!(get_string_setting(&conn, "api_server").unwrap(), None);
        set_string_setting(&conn, "api_server", "").unwrap();
        assert_eq!(
            get_string_setting(&conn, "api_server").unwrap().as_deref(),
            Some("")
        );
        set_string_setting(&conn, "api_server", "binapi.pcloud.com").unwrap();
        assert_eq!(
            get_string_setting(&conn, "api_server").unwrap().as_deref(),
            Some("binapi.pcloud.com")
        );
    }

    #[test]
    fn type_mismatch_is_surfaced() {
        let (_p, conn) = open("mismatch");
        set_bool_setting(&conn, "flag", true).unwrap();
        let err = get_int_setting(&conn, "flag").expect_err("should mismatch");
        match err {
            SettingsError::TypeMismatch(tm) => {
                assert_eq!(tm.name, "flag");
                assert_eq!(tm.stored, ValueKind::Bool);
                assert_eq!(tm.requested, ValueKind::Int);
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn overwrite_changes_kind() {
        let (_p, conn) = open("overwrite");
        set_bool_setting(&conn, "k", true).unwrap();
        set_string_setting(&conn, "k", "now-text").unwrap();
        assert_eq!(
            get_string_setting(&conn, "k").unwrap().as_deref(),
            Some("now-text")
        );
        // the previous bool reader should now surface a type mismatch
        assert!(matches!(
            get_bool_setting(&conn, "k"),
            Err(SettingsError::TypeMismatch(_))
        ));
    }

    #[test]
    fn reset_removes_entry() {
        let (_p, conn) = open("reset");
        set_int_setting(&conn, "x", 42).unwrap();
        assert!(reset_setting(&conn, "x").unwrap());
        assert_eq!(get_int_setting(&conn, "x").unwrap(), None);
        assert!(!reset_setting(&conn, "x").unwrap());
    }

    #[test]
    fn empty_name_rejected() {
        let (_p, conn) = open("empty");
        assert!(matches!(
            set_bool_setting(&conn, "", true),
            Err(SettingsError::EmptyName)
        ));
        assert!(matches!(
            get_string_setting(&conn, ""),
            Err(SettingsError::EmptyName)
        ));
    }
}

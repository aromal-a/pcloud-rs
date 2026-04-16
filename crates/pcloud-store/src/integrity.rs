// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::Connection;

/// Outcome of a startup/integrity probe against the backing SQLite store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityStatus {
    /// Schema version matches expected target and `PRAGMA quick_check` returns `ok`.
    Clean,
    /// Store is out of date or corrupt; caller must run migrations or repair before use.
    RepairRequired,
}

/// Classify store health based purely on `PRAGMA user_version` vs the expected schema version.
///
/// This is a pure helper used before a connection integrity check is available.
#[must_use]
pub fn evaluate_startup_integrity(schema_version: u32, expected_version: u32) -> IntegrityStatus {
    if schema_version == expected_version {
        IntegrityStatus::Clean
    } else {
        IntegrityStatus::RepairRequired
    }
}

/// Run `PRAGMA quick_check` against `conn` and combine it with the schema-version comparison.
///
/// Returns [`IntegrityStatus::RepairRequired`] if either the structural check fails or the schema
/// version is out of date. No transaction is opened; the pragma is read-only.
pub fn evaluate_connection_integrity(
    conn: &Connection,
    schema_version: u32,
    expected_version: u32,
) -> Result<IntegrityStatus, rusqlite::Error> {
    let quick_check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        return Ok(IntegrityStatus::RepairRequired);
    }

    Ok(evaluate_startup_integrity(schema_version, expected_version))
}

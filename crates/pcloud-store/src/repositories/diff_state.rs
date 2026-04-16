//! Per-sync-root diff cursor state (`sync_diff_state` table, schema v10).
//!
//! Mirrors the C `pclsync/pdiff.c` use of the `setting.diffid` row: the
//! daemon's `DiffWorker` advances the cursor as it processes diff event
//! batches. Persisted across restart so a freshly-launched daemon resumes
//! from the last successfully-processed `diffid` rather than re-fetching
//! the entire account history.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::SyncId;
use rusqlite::{Connection, OptionalExtension};

/// One row of the `sync_diff_state` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffStateRecord {
    /// Sync root this cursor belongs to.
    pub sync_id: SyncId,
    /// Last successfully-processed `diffid`. `0` means "never processed".
    pub diffid: u64,
    /// Unix timestamp (seconds) of the most recent advance.
    pub updated_at: i64,
}

/// Stateless helper bundle for reading / writing `sync_diff_state` rows.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiffStateRepository;

impl DiffStateRepository {
    /// Read the persisted diffid for `sync_id`, or `None` if never set.
    pub fn load(
        conn: &Connection,
        sync_id: SyncId,
    ) -> Result<Option<DiffStateRecord>, rusqlite::Error> {
        conn.query_row(
            "SELECT sync_id, diffid, updated_at FROM sync_diff_state WHERE sync_id = ?1",
            [sync_id.get()],
            |row| {
                Ok(DiffStateRecord {
                    sync_id: SyncId::new(row.get::<_, u64>(0)?),
                    diffid: row.get::<_, u64>(1)?,
                    updated_at: row.get::<_, i64>(2)?,
                })
            },
        )
        .optional()
    }

    /// Write or upsert the diffid for `sync_id`.
    pub fn save(
        conn: &Connection,
        sync_id: SyncId,
        diffid: u64,
        updated_at: i64,
    ) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT INTO sync_diff_state (sync_id, diffid, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(sync_id) DO UPDATE SET diffid = excluded.diffid,
                                                updated_at = excluded.updated_at",
            (sync_id.get(), diffid, updated_at),
        )?;
        Ok(())
    }

    /// Drop the cursor for `sync_id` (called when a sync root is removed).
    pub fn delete(conn: &Connection, sync_id: SyncId) -> Result<bool, rusqlite::Error> {
        let n = conn.execute(
            "DELETE FROM sync_diff_state WHERE sync_id = ?1",
            [sync_id.get()],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StoreError, bootstrap_profile};
    use std::path::PathBuf;

    fn temp_db(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-store-diff-{}-{}-{}.sqlite3",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn diff_state_round_trips() -> Result<(), StoreError> {
        let path = temp_db("roundtrip");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path)?;
        let conn = Connection::open(&path)?;

        assert!(DiffStateRepository::load(&conn, SyncId::new(1))?.is_none());

        DiffStateRepository::save(&conn, SyncId::new(1), 42, 1_700_000_000)?;
        let row = DiffStateRepository::load(&conn, SyncId::new(1))?.unwrap();
        assert_eq!(row.diffid, 42);
        assert_eq!(row.updated_at, 1_700_000_000);

        DiffStateRepository::save(&conn, SyncId::new(1), 99, 1_700_000_500)?;
        let row = DiffStateRepository::load(&conn, SyncId::new(1))?.unwrap();
        assert_eq!(row.diffid, 99);

        assert!(DiffStateRepository::delete(&conn, SyncId::new(1))?);
        assert!(DiffStateRepository::load(&conn, SyncId::new(1))?.is_none());
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}

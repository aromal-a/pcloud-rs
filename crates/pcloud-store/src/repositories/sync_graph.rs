// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::SyncId;
use pcloud_model::sync::SyncType;
use rusqlite::Connection;

/// One row of the `sync_root_records` table: a persisted sync-root definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRootRecord {
    /// Stable sync-root identifier allocated by the daemon.
    pub sync_id: SyncId,
    /// Canonicalized absolute local filesystem path.
    pub local_path: String,
    /// Remote pCloud path this root maps to.
    pub remote_path: String,
    /// True when the sync root is paused and the engine must not schedule work for it.
    pub paused: bool,
    /// Mirrors the three values of the C `psync_synctype_t` type.
    /// Defaults to `SyncType::Full` for freshly added roots, matching the
    /// historical CLI default.
    pub sync_type: SyncType,
}

/// In-memory snapshot of every tracked sync root.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncGraphRepository {
    /// All sync roots, ordered by `sync_id` ascending.
    pub tracked_sync_roots: Vec<SyncRootRecord>,
}

impl SyncGraphRepository {
    /// Load every `sync_root_records` row, ordered by `sync_id`.
    pub fn load(conn: &Connection) -> Result<Self, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT sync_id, local_path, remote_path, paused, sync_type FROM sync_root_records ORDER BY sync_id",
        )?;
        let tracked_sync_roots = stmt
            .query_map([], |row| {
                let raw_type: i64 = row.get(4)?;
                let sync_type = SyncType::from_u8(raw_type as u8).unwrap_or(SyncType::Full);
                Ok(SyncRootRecord {
                    sync_id: SyncId::new(row.get::<_, u64>(0)?),
                    local_path: row.get(1)?,
                    remote_path: row.get(2)?,
                    paused: row.get::<_, i64>(3)? != 0,
                    sync_type,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { tracked_sync_roots })
    }

    /// Replace the contents of `sync_root_records` with [`SyncGraphRepository::tracked_sync_roots`].
    ///
    /// The full-table rewrite must run inside a [`crate::tx::TransactionBoundary::immediate`]
    /// so concurrent readers never observe an empty sync graph mid-save.
    pub fn save(&self, conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute("DELETE FROM sync_root_records", [])?;
        let mut stmt = conn.prepare(
            "INSERT INTO sync_root_records (sync_id, local_path, remote_path, paused, sync_type) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for root in &self.tracked_sync_roots {
            stmt.execute((
                root.sync_id.get(),
                root.local_path.as_str(),
                root.remote_path.as_str(),
                i64::from(root.paused),
                i64::from(root.sync_type.as_u8()),
            ))?;
        }
        Ok(())
    }
}

// **PLATFORM:** all
// **GATING:** none (portable).

/// Single-row primary account repository (`account` table).
pub mod account;
/// Tamper-evident SHA-256 hash-chained audit log (`audit_events` table).
pub mod audit;
/// Per-sync-root `diffid` cursor persistence (`sync_diff_state` table).
pub mod diff_state;
/// Local file/folder metadata cache (`file_metadata` table, schema v11).
pub mod file_metadata;
/// Named daemon preferences repository (`preferences` table).
pub mod preferences;
/// Strict typed key/value helpers on top of the `value_kv` table.
pub mod settings;
/// Persisted sync-root records (`sync_root_records` table).
pub mod sync_graph;
/// Chunked upload resume state (`upload_resume_state` table).
pub mod upload_resume;
/// Loose typed key/value helpers on top of the `value_kv` table.
pub mod values;

use rusqlite::Connection;
pub use sync_graph::SyncRootRecord;

/// Aggregated in-memory snapshot of the repositories the daemon keeps hot.
///
/// Not every repository is mirrored here — only the ones whose state is cheap to
/// hold in memory and useful for startup diagnostics. Use the per-module repositories
/// directly for everything else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepositorySet {
    /// Primary account repository snapshot.
    pub accounts: account::AccountRepository,
    /// Audit log repository snapshot (counters only; the chain itself lives in SQL).
    pub audit: audit::AuditRepository,
    /// Preferences repository snapshot.
    pub preferences: preferences::PreferencesRepository,
    /// Sync graph repository snapshot.
    pub sync_graph: sync_graph::SyncGraphRepository,
}

impl RepositorySet {
    /// Human-readable diagnostic summary used in startup logs.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "repos(account_present={}, audit_events={}, sync_roots={})",
            self.accounts.primary_account.is_some(),
            self.audit.retained_event_count,
            self.sync_graph.tracked_sync_roots.len()
        )
    }

    /// Load every tracked repository from `conn` in read-only fashion.
    pub fn load(conn: &Connection) -> Result<Self, rusqlite::Error> {
        Ok(Self {
            accounts: account::AccountRepository::load(conn)?,
            audit: audit::AuditRepository::load(conn)?,
            preferences: preferences::PreferencesRepository::load(conn)?,
            sync_graph: sync_graph::SyncGraphRepository::load(conn)?,
        })
    }

    /// Persist every mutable repository back to `conn`. Callers are expected to
    /// wrap this in a [`crate::tx::TransactionBoundary::immediate`] so repositories
    /// stay mutually consistent.
    pub fn save(&self, conn: &Connection) -> Result<(), rusqlite::Error> {
        self.accounts.save(conn)?;
        self.preferences.save(conn)?;
        self.sync_graph.save(conn)?;
        Ok(())
    }
}

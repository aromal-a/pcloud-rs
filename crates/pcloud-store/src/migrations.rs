// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::Connection;
use thiserror::Error;

use crate::schema::{
    SCHEMA_VERSION_V1, SCHEMA_VERSION_V2, SCHEMA_VERSION_V3, SCHEMA_VERSION_V4, SCHEMA_VERSION_V5,
    SCHEMA_VERSION_V6, SCHEMA_VERSION_V7, SCHEMA_VERSION_V8, SCHEMA_VERSION_V9, SCHEMA_VERSION_V10,
    SCHEMA_VERSION_V11, SCHEMA_VERSION_V12, apply_schema_v1, apply_schema_v2, apply_schema_v3,
    apply_schema_v4, apply_schema_v5, apply_schema_v6, apply_schema_v7, apply_schema_v8,
    apply_schema_v9, apply_schema_v10, apply_schema_v11, apply_schema_v12,
};

/// Forward-only migration plan to bring the store up to a target schema version.
///
/// The plan is a single scalar today (the target version). [`apply_plan`]
/// reads the database's current `PRAGMA user_version` at execution time
/// and runs the per-version apply functions for every step in the
/// inclusive range `(current, target]`. Each apply function is
/// idempotent and commits its own `PRAGMA user_version` bump, so a crash
/// part-way through a multi-step migration leaves the database at the
/// last successfully-bumped version and the next launch resumes from
/// there.
///
/// ## Rollback policy
///
/// There is **no** rollback. Migrations are append-only in schema space:
/// a user that needs to revert to an older daemon build must either
/// keep a backup of the pre-migration database file or delete the store
/// and re-authenticate. This policy is load-bearing for the v8 audit
/// upgrade, which re-hashes historical rows — a silent rollback would
/// drop cryptographic state that cannot be reconstructed from a later
/// shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    /// Target `PRAGMA user_version` the store will end at after [`apply_plan`] completes.
    pub target_version: u32,
}

/// Errors produced while computing a [`MigrationPlan`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MigrationError {
    /// The requested `target` is older than the store's `current` version. Migrations are
    /// forward-only and the store intentionally refuses to downgrade; see the
    /// rollback policy documented on [`MigrationPlan`].
    #[error("cannot migrate backwards from {current} to {target}")]
    BackwardsMigration {
        /// Current `PRAGMA user_version` read from the database.
        current: u32,
        /// Version the caller attempted to step down to.
        target: u32,
    },
}

/// Build a forward-only [`MigrationPlan`] from `current` to `target`. Fails with
/// [`MigrationError::BackwardsMigration`] when `target < current`.
pub fn build_plan(current: u32, target: u32) -> Result<MigrationPlan, MigrationError> {
    if target < current {
        return Err(MigrationError::BackwardsMigration { current, target });
    }

    Ok(MigrationPlan {
        target_version: target,
    })
}

/// Apply each `apply_schema_vN` step needed to bring `conn` from its current
/// `PRAGMA user_version` up to `plan.target_version`.
///
/// Each step is idempotent and executed in its own inner transaction inside the
/// schema helpers; the caller is free to wrap the whole call in an outer
/// [`crate::tx::TransactionBoundary`] for atomic startup.
///
/// Crash safety: every `apply_schema_vN` ends with a `PRAGMA user_version = N`
/// statement inside the same `execute_batch` that ran its DDL. A crash between
/// two successive version steps therefore leaves the database at the last
/// fully-committed version, not in a half-applied state, and a subsequent
/// launch will resume the migration from there.
pub fn apply_plan(conn: &Connection, plan: &MigrationPlan) -> Result<(), rusqlite::Error> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if current < SCHEMA_VERSION_V1 && plan.target_version >= SCHEMA_VERSION_V1 {
        apply_schema_v1(conn)?;
    }
    if current < SCHEMA_VERSION_V2 && plan.target_version >= SCHEMA_VERSION_V2 {
        apply_schema_v2(conn)?;
    }
    if current < SCHEMA_VERSION_V3 && plan.target_version >= SCHEMA_VERSION_V3 {
        apply_schema_v3(conn)?;
    }
    if current < SCHEMA_VERSION_V4 && plan.target_version >= SCHEMA_VERSION_V4 {
        apply_schema_v4(conn)?;
    }
    if current < SCHEMA_VERSION_V5 && plan.target_version >= SCHEMA_VERSION_V5 {
        apply_schema_v5(conn)?;
    }
    if current < SCHEMA_VERSION_V6 && plan.target_version >= SCHEMA_VERSION_V6 {
        apply_schema_v6(conn)?;
    }
    if current < SCHEMA_VERSION_V7 && plan.target_version >= SCHEMA_VERSION_V7 {
        apply_schema_v7(conn)?;
    }
    if current < SCHEMA_VERSION_V8 && plan.target_version >= SCHEMA_VERSION_V8 {
        apply_schema_v8(conn)?;
    }
    if current < SCHEMA_VERSION_V9 && plan.target_version >= SCHEMA_VERSION_V9 {
        apply_schema_v9(conn)?;
    }
    if current < SCHEMA_VERSION_V10 && plan.target_version >= SCHEMA_VERSION_V10 {
        apply_schema_v10(conn)?;
    }
    if current < SCHEMA_VERSION_V11 && plan.target_version >= SCHEMA_VERSION_V11 {
        apply_schema_v11(conn)?;
    }
    if current < SCHEMA_VERSION_V12 && plan.target_version >= SCHEMA_VERSION_V12 {
        apply_schema_v12(conn)?;
    }

    Ok(())
}

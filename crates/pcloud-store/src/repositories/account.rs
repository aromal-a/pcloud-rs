// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::UserId;
use rusqlite::{Connection, OptionalExtension};

/// Primary account row read from / written to the `account` table.
///
/// Only a single row exists (enforced by `CHECK (primary_account = 1)` in schema v1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    /// pCloud numeric user id.
    pub user_id: UserId,
    /// Account email address (not treated as a secret).
    pub email: String,
    /// True when an auth token is held in the auth vault. The token itself is **never**
    /// stored in this table; only a presence flag.
    pub auth_token_present: bool,
}

/// Repository guarding the single `account` row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountRepository {
    /// The current primary account, or `None` when the daemon is unauthenticated.
    pub primary_account: Option<AccountRecord>,
}

impl AccountRepository {
    /// Load the primary account row (if any) from the `account` table.
    pub fn load(conn: &Connection) -> Result<Self, rusqlite::Error> {
        let primary_account = conn
            .query_row(
                "SELECT user_id, email, auth_token_present FROM account WHERE primary_account = 1",
                [],
                |row| {
                    Ok(AccountRecord {
                        user_id: UserId::new(row.get::<_, u64>(0)?),
                        email: row.get(1)?,
                        auth_token_present: row.get::<_, i64>(2)? != 0,
                    })
                },
            )
            .optional()?;

        Ok(Self { primary_account })
    }

    /// Replace the primary account row with [`AccountRepository::primary_account`].
    ///
    /// Implemented as `DELETE` + `INSERT` in the same transaction; callers should wrap
    /// this in [`crate::tx::TransactionBoundary::immediate`] so a partial failure never
    /// leaves the daemon with a wiped account row.
    pub fn save(&self, conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute("DELETE FROM account WHERE primary_account = 1", [])?;
        if let Some(account) = &self.primary_account {
            conn.execute(
                "INSERT INTO account (primary_account, user_id, email, auth_token_present) VALUES (1, ?1, ?2, ?3)",
                (
                    account.user_id.get(),
                    account.email.as_str(),
                    i64::from(account.auth_token_present),
                ),
            )?;
        }

        Ok(())
    }
}

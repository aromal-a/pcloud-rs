// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::{Connection, OptionalExtension};

const DURABLE_AUTH_TOKENS_KEY: &str = "durable_auth_tokens_enabled";
const API_SERVER_BINAPI_KEY: &str = "api_server_binapi";
const API_SERVER_LOCATION_ID_KEY: &str = "api_server_location_id";
/// Remote folder id for the per-device backup root. Mirrors the legacy
/// `BackupRootFoId` row in the C `setting` table consumed by
/// `psync_stop_device` and the backup create path.
const BACKUP_DEVICE_FOLDER_ID_KEY: &str = "backup_device_folder_id";

/// In-memory snapshot of the strongly-named daemon preferences row set.
///
/// Each field maps to one row of the `preferences` table keyed by a stable name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreferencesRepository {
    /// When `Some(true)`, the daemon persists auth tokens into the auth vault.
    pub durable_auth_tokens_enabled: Option<bool>,
    /// Optional override for the binary API server hostname.
    pub api_server_binapi: Option<String>,
    /// Optional numeric location id selecting which API server cluster to use.
    pub api_server_location_id: Option<u32>,
    /// Remote folder id for the per-device backup root (mirrors legacy `BackupRootFoId`).
    pub backup_device_folder_id: Option<u64>,
}

impl PreferencesRepository {
    /// Load every known preference row from the `preferences` table.
    pub fn load(conn: &Connection) -> Result<Self, rusqlite::Error> {
        let durable_auth_tokens_enabled = conn
            .query_row(
                "SELECT bool_value FROM preferences WHERE name = ?1",
                [DURABLE_AUTH_TOKENS_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|value| value != 0);
        let api_server_binapi = conn
            .query_row(
                "SELECT text_value FROM preferences WHERE name = ?1",
                [API_SERVER_BINAPI_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let api_server_location_id = conn
            .query_row(
                "SELECT int_value FROM preferences WHERE name = ?1",
                [API_SERVER_LOCATION_ID_KEY],
                |row| row.get::<_, u32>(0),
            )
            .optional()?;
        let backup_device_folder_id = conn
            .query_row(
                "SELECT int_value FROM preferences WHERE name = ?1",
                [BACKUP_DEVICE_FOLDER_ID_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .and_then(|value| u64::try_from(value).ok());

        Ok(Self {
            durable_auth_tokens_enabled,
            api_server_binapi,
            api_server_location_id,
            backup_device_folder_id,
        })
    }

    /// Replace every known preference row from the in-memory snapshot.
    ///
    /// Implemented as per-key `DELETE` + conditional `INSERT`; wrap in
    /// [`crate::tx::TransactionBoundary::immediate`] so partial failure cannot leave
    /// a preference partially cleared.
    pub fn save(&self, conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute(
            "DELETE FROM preferences WHERE name = ?1",
            [DURABLE_AUTH_TOKENS_KEY],
        )?;
        conn.execute(
            "DELETE FROM preferences WHERE name = ?1",
            [API_SERVER_BINAPI_KEY],
        )?;
        conn.execute(
            "DELETE FROM preferences WHERE name = ?1",
            [API_SERVER_LOCATION_ID_KEY],
        )?;
        conn.execute(
            "DELETE FROM preferences WHERE name = ?1",
            [BACKUP_DEVICE_FOLDER_ID_KEY],
        )?;
        if let Some(enabled) = self.durable_auth_tokens_enabled {
            conn.execute(
                "INSERT INTO preferences (name, bool_value) VALUES (?1, ?2)",
                (DURABLE_AUTH_TOKENS_KEY, i64::from(enabled)),
            )?;
        }
        if let Some(binapi) = self.api_server_binapi.as_deref() {
            conn.execute(
                "INSERT INTO preferences (name, text_value) VALUES (?1, ?2)",
                (API_SERVER_BINAPI_KEY, binapi),
            )?;
        }
        if let Some(location_id) = self.api_server_location_id {
            conn.execute(
                "INSERT INTO preferences (name, int_value) VALUES (?1, ?2)",
                (API_SERVER_LOCATION_ID_KEY, i64::from(location_id)),
            )?;
        }
        if let Some(folder_id) = self.backup_device_folder_id {
            let signed = i64::try_from(folder_id).unwrap_or(i64::MAX);
            conn.execute(
                "INSERT INTO preferences (name, int_value) VALUES (?1, ?2)",
                (BACKUP_DEVICE_FOLDER_ID_KEY, signed),
            )?;
        }
        Ok(())
    }
}

//! Persisted state for in-flight chunked uploads.
//!
//! Mirrors the role of the legacy C `localfileupload` table
//! (`pclsync/pupload.c:1017-1022`) but stores an explicit client-tracked
//! byte offset so the upload state machine can resume from exactly the
//! last confirmed write on restart.
//!
//! The table is defined by schema v9 (see
//! [`crate::schema::apply_schema_v9`]). Rows are keyed by canonicalized
//! local path: at most one in-flight upload per local file.

// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::{Connection, OptionalExtension};

/// Conflict hint persisted alongside the upload.
///
/// Maps to the C `ifhash` binparam selection (`pupload.c:1495-1509`):
///
/// * [`ConflictHint::None`] — no conflict param; server default overwrite.
/// * [`ConflictHint::IfHash`] — numeric `ifhash`: conditional overwrite
///   only when the remote hash matches the supplied value.
/// * [`ConflictHint::IfNew`] — string `ifhash = "new"`: create-if-absent;
///   server renames on conflict and flips `conflicted=true` in metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictHint {
    /// No conflict param emitted.
    None,
    /// Numeric `ifhash` hint.
    IfHash(u64),
    /// `ifhash = "new"` sentinel.
    IfNew,
}

/// Row-shaped view of a persisted upload resume state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadResumeRecord {
    /// Canonicalized absolute local path.
    pub local_path: String,
    /// Remote parent folder id (`folderid`).
    pub parent_folder_id: u64,
    /// Target remote file name.
    pub file_name: String,
    /// Server-assigned upload handle from `upload_create`.
    pub upload_id: u64,
    /// Last locally-confirmed uploaded byte count.
    pub offset: u64,
    /// Total size of the local file at create time.
    pub total_size: u64,
    /// Lowercase hex SHA-1 of the prefix `[0, offset)`.
    pub prefix_sha1: Option<String>,
    /// Conflict hint persisted with the upload.
    pub conflict: ConflictHint,
    /// Unix timestamp (seconds) of the last update.
    pub updated_at: i64,
}

/// Stateless accessor for the `upload_resume_state` table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UploadResumeRepository;

impl UploadResumeRepository {
    /// Inserts or replaces the resume row for `local_path`.
    pub fn put(conn: &Connection, record: &UploadResumeRecord) -> Result<(), rusqlite::Error> {
        let (if_hash, if_new) = match record.conflict {
            ConflictHint::None => (None, 0i64),
            ConflictHint::IfHash(hash) => (Some(hash as i64), 0i64),
            ConflictHint::IfNew => (None, 1i64),
        };
        conn.execute(
            "INSERT INTO upload_resume_state (
                local_path, parent_folder_id, file_name, upload_id,
                offset, total_size, prefix_sha1, if_hash, if_new, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(local_path) DO UPDATE SET
                 parent_folder_id = excluded.parent_folder_id,
                 file_name = excluded.file_name,
                 upload_id = excluded.upload_id,
                 offset = excluded.offset,
                 total_size = excluded.total_size,
                 prefix_sha1 = excluded.prefix_sha1,
                 if_hash = excluded.if_hash,
                 if_new = excluded.if_new,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                record.local_path,
                record.parent_folder_id as i64,
                record.file_name,
                record.upload_id as i64,
                record.offset as i64,
                record.total_size as i64,
                record.prefix_sha1,
                if_hash,
                if_new,
                record.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Updates just the offset + prefix hash for a known upload.
    pub fn update_offset(
        conn: &Connection,
        local_path: &str,
        offset: u64,
        prefix_sha1: Option<&str>,
        updated_at: i64,
    ) -> Result<bool, rusqlite::Error> {
        let affected = conn.execute(
            "UPDATE upload_resume_state
                SET offset = ?2, prefix_sha1 = ?3, updated_at = ?4
              WHERE local_path = ?1",
            rusqlite::params![local_path, offset as i64, prefix_sha1, updated_at],
        )?;
        Ok(affected > 0)
    }

    /// Returns the persisted resume row for `local_path`, if any.
    pub fn get(
        conn: &Connection,
        local_path: &str,
    ) -> Result<Option<UploadResumeRecord>, rusqlite::Error> {
        conn.query_row(
            "SELECT local_path, parent_folder_id, file_name, upload_id,
                    offset, total_size, prefix_sha1, if_hash, if_new, updated_at
               FROM upload_resume_state
              WHERE local_path = ?1",
            [local_path],
            row_to_record,
        )
        .optional()
    }

    /// Removes a resume row (e.g. after successful commit).
    pub fn delete(conn: &Connection, local_path: &str) -> Result<bool, rusqlite::Error> {
        let affected = conn.execute(
            "DELETE FROM upload_resume_state WHERE local_path = ?1",
            [local_path],
        )?;
        Ok(affected > 0)
    }

    /// Lists every persisted resume row.
    pub fn list_all(conn: &Connection) -> Result<Vec<UploadResumeRecord>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT local_path, parent_folder_id, file_name, upload_id,
                    offset, total_size, prefix_sha1, if_hash, if_new, updated_at
               FROM upload_resume_state
              ORDER BY local_path",
        )?;
        let rows = stmt.query_map([], row_to_record)?;
        rows.collect()
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> Result<UploadResumeRecord, rusqlite::Error> {
    let if_hash: Option<i64> = row.get(7)?;
    let if_new: i64 = row.get(8)?;
    let conflict = match (if_hash, if_new) {
        (Some(h), 0) => ConflictHint::IfHash(h as u64),
        (None, 1) => ConflictHint::IfNew,
        (None, 0) => ConflictHint::None,
        // Defensive: malformed combination → fall back to None.
        _ => ConflictHint::None,
    };
    Ok(UploadResumeRecord {
        local_path: row.get(0)?,
        parent_folder_id: row.get::<_, i64>(1)? as u64,
        file_name: row.get(2)?,
        upload_id: row.get::<_, i64>(3)? as u64,
        offset: row.get::<_, i64>(4)? as u64,
        total_size: row.get::<_, i64>(5)? as u64,
        prefix_sha1: row.get(6)?,
        conflict,
        updated_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_profile;

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-store-upload-resume-{}-{}-{}.sqlite3",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    fn sample_record(path: &str, offset: u64) -> UploadResumeRecord {
        UploadResumeRecord {
            local_path: path.to_owned(),
            parent_folder_id: 42,
            file_name: "report.txt".to_owned(),
            upload_id: 7,
            offset,
            total_size: 10 * 1024 * 1024,
            prefix_sha1: Some("a".repeat(40)),
            conflict: ConflictHint::IfHash(0xdead_beef),
            updated_at: 1_700_000_000,
        }
    }

    #[test]
    fn put_get_roundtrip() {
        let path = temp_db_path("put-get");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");
        let rec = sample_record("/tmp/a.bin", 4096);

        UploadResumeRepository::put(&conn, &rec).expect("put");
        let fetched = UploadResumeRepository::get(&conn, "/tmp/a.bin")
            .expect("get")
            .expect("row present");
        assert_eq!(fetched, rec);
    }

    #[test]
    fn update_offset_moves_forward_only_rows_that_exist() {
        let path = temp_db_path("update-offset");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");
        let rec = sample_record("/tmp/b.bin", 0);
        UploadResumeRepository::put(&conn, &rec).expect("put");

        let updated = UploadResumeRepository::update_offset(
            &conn,
            "/tmp/b.bin",
            65_536,
            Some("b".repeat(40).as_str()),
            1_700_000_100,
        )
        .expect("update");
        assert!(updated);
        let fetched = UploadResumeRepository::get(&conn, "/tmp/b.bin")
            .unwrap()
            .unwrap();
        assert_eq!(fetched.offset, 65_536);
        assert_eq!(
            fetched.prefix_sha1.as_deref(),
            Some("b".repeat(40).as_str())
        );

        let missing = UploadResumeRepository::update_offset(&conn, "/tmp/missing", 1, None, 0)
            .expect("update");
        assert!(!missing);
    }

    #[test]
    fn delete_and_list() {
        let path = temp_db_path("delete-list");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");
        UploadResumeRepository::put(&conn, &sample_record("/tmp/a.bin", 1)).unwrap();
        UploadResumeRepository::put(&conn, &sample_record("/tmp/c.bin", 2)).unwrap();

        let rows = UploadResumeRepository::list_all(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(UploadResumeRepository::delete(&conn, "/tmp/a.bin").unwrap());
        assert!(!UploadResumeRepository::delete(&conn, "/tmp/a.bin").unwrap());
        let rows = UploadResumeRepository::list_all(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].local_path, "/tmp/c.bin");
    }

    #[test]
    fn conflict_hint_roundtrip_all_variants() {
        let path = temp_db_path("conflict-variants");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path).expect("bootstrap");
        let conn = Connection::open(&path).expect("open");

        let mut rec = sample_record("/tmp/n.bin", 0);
        rec.conflict = ConflictHint::None;
        UploadResumeRepository::put(&conn, &rec).unwrap();
        assert_eq!(
            UploadResumeRepository::get(&conn, "/tmp/n.bin")
                .unwrap()
                .unwrap()
                .conflict,
            ConflictHint::None
        );

        let mut rec = sample_record("/tmp/new.bin", 0);
        rec.conflict = ConflictHint::IfNew;
        UploadResumeRepository::put(&conn, &rec).unwrap();
        assert_eq!(
            UploadResumeRepository::get(&conn, "/tmp/new.bin")
                .unwrap()
                .unwrap()
                .conflict,
            ConflictHint::IfNew
        );

        let mut rec = sample_record("/tmp/hash.bin", 0);
        rec.conflict = ConflictHint::IfHash(0xabcd);
        UploadResumeRepository::put(&conn, &rec).unwrap();
        assert_eq!(
            UploadResumeRepository::get(&conn, "/tmp/hash.bin")
                .unwrap()
                .unwrap()
                .conflict,
            ConflictHint::IfHash(0xabcd)
        );
    }
}

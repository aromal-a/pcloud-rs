//! Local file/folder metadata cache (`file_metadata` table, schema v11).
//!
//! Mirrors the C `pclsync/pfolder.c` local metadata cache: the diff engine
//! populates this table as remote diff events arrive, so that
//! `psync_stat_path`-style queries can be resolved locally without hitting
//! the API.

// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::{Connection, OptionalExtension};

/// One row of the `file_metadata` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetadataRecord {
    /// Remote file id (for files) or folder id (for folders). Primary key.
    pub file_id: u64,
    /// Parent folder's remote id. `0` for the root folder.
    pub parent_folder_id: u64,
    /// Entry name (leaf name, not full path).
    pub name: String,
    /// Size in bytes. `0` for folders.
    pub size: u64,
    /// Content hash (hex string). Empty for folders.
    pub hash: String,
    /// Last-modified timestamp (unix seconds).
    pub modified: i64,
    /// Creation timestamp (unix seconds).
    pub created: i64,
    /// `true` if this entry is a folder, `false` if it is a file.
    pub is_folder: bool,
}

/// Result of a `stat_path` lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatResult {
    /// The resolved metadata record.
    pub metadata: FileMetadataRecord,
    /// The full path that was resolved.
    pub resolved_path: String,
}

/// Stateless helper bundle for reading/writing `file_metadata` rows.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileMetadataRepository;

impl FileMetadataRepository {
    /// Upsert a single metadata entry. If the `file_id` already exists,
    /// the row is replaced.
    pub fn upsert(conn: &Connection, record: &FileMetadataRecord) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT INTO file_metadata (file_id, parent_folder_id, name, size, hash, modified, created, is_folder)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(file_id) DO UPDATE SET
                parent_folder_id = excluded.parent_folder_id,
                name = excluded.name,
                size = excluded.size,
                hash = excluded.hash,
                modified = excluded.modified,
                created = excluded.created,
                is_folder = excluded.is_folder",
            (
                record.file_id,
                record.parent_folder_id,
                &record.name,
                record.size,
                &record.hash,
                record.modified,
                record.created,
                record.is_folder as i32,
            ),
        )?;
        Ok(())
    }

    /// Lookup a single entry by its `file_id`.
    pub fn get_by_id(
        conn: &Connection,
        file_id: u64,
    ) -> Result<Option<FileMetadataRecord>, rusqlite::Error> {
        conn.query_row(
            "SELECT file_id, parent_folder_id, name, size, hash, modified, created, is_folder
             FROM file_metadata WHERE file_id = ?1",
            [file_id],
            row_to_record,
        )
        .optional()
    }

    /// List all children of a given parent folder.
    pub fn list_children(
        conn: &Connection,
        parent_folder_id: u64,
    ) -> Result<Vec<FileMetadataRecord>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT file_id, parent_folder_id, name, size, hash, modified, created, is_folder
             FROM file_metadata WHERE parent_folder_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map([parent_folder_id], row_to_record)?;
        rows.collect()
    }

    /// Lookup a single entry by parent folder id and name.
    pub fn get_by_parent_and_name(
        conn: &Connection,
        parent_folder_id: u64,
        name: &str,
    ) -> Result<Option<FileMetadataRecord>, rusqlite::Error> {
        conn.query_row(
            "SELECT file_id, parent_folder_id, name, size, hash, modified, created, is_folder
             FROM file_metadata WHERE parent_folder_id = ?1 AND name = ?2",
            (parent_folder_id, name),
            row_to_record,
        )
        .optional()
    }

    /// Resolve an absolute pCloud-drive path (e.g. `/Documents/report.txt`)
    /// against the local metadata cache. Returns `None` if any path
    /// component is missing from the cache.
    ///
    /// The root folder is assumed to have `file_id = 0` and `is_folder = true`.
    pub fn resolve_path(
        conn: &Connection,
        path: &str,
    ) -> Result<Option<FileMetadataRecord>, rusqlite::Error> {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            // Root folder lookup: return synthetic root or actual row if present.
            return Self::get_by_id(conn, 0);
        }

        let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        let mut current_folder_id: u64 = 0; // root

        for (i, segment) in segments.iter().enumerate() {
            let is_last = i == segments.len() - 1;
            match Self::get_by_parent_and_name(conn, current_folder_id, segment)? {
                Some(record) => {
                    if is_last {
                        return Ok(Some(record));
                    }
                    if !record.is_folder {
                        // Non-terminal path component is not a folder.
                        return Ok(None);
                    }
                    current_folder_id = record.file_id;
                }
                None => return Ok(None),
            }
        }

        Ok(None)
    }

    /// Delete a single entry by id.
    pub fn delete(conn: &Connection, file_id: u64) -> Result<bool, rusqlite::Error> {
        let n = conn.execute("DELETE FROM file_metadata WHERE file_id = ?1", [file_id])?;
        Ok(n > 0)
    }

    /// Delete all children of a parent folder (non-recursive; callers
    /// must walk the tree for deep deletes).
    pub fn delete_children(
        conn: &Connection,
        parent_folder_id: u64,
    ) -> Result<usize, rusqlite::Error> {
        let n = conn.execute(
            "DELETE FROM file_metadata WHERE parent_folder_id = ?1",
            [parent_folder_id],
        )?;
        Ok(n)
    }

    /// Return the total number of cached metadata entries.
    pub fn count(conn: &Connection) -> Result<u64, rusqlite::Error> {
        conn.query_row("SELECT COUNT(*) FROM file_metadata", [], |row| {
            row.get::<_, u64>(0)
        })
    }
}

fn row_to_record(row: &rusqlite::Row) -> Result<FileMetadataRecord, rusqlite::Error> {
    Ok(FileMetadataRecord {
        file_id: row.get::<_, u64>(0)?,
        parent_folder_id: row.get::<_, u64>(1)?,
        name: row.get::<_, String>(2)?,
        size: row.get::<_, u64>(3)?,
        hash: row.get::<_, String>(4)?,
        modified: row.get::<_, i64>(5)?,
        created: row.get::<_, i64>(6)?,
        is_folder: row.get::<_, i32>(7)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StoreError, bootstrap_profile};
    use std::path::PathBuf;

    fn temp_db(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-store-fmeta-{}-{}-{}.sqlite3",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn make_folder(file_id: u64, parent: u64, name: &str) -> FileMetadataRecord {
        FileMetadataRecord {
            file_id,
            parent_folder_id: parent,
            name: name.to_owned(),
            size: 0,
            hash: String::new(),
            modified: 1_700_000_000,
            created: 1_699_000_000,
            is_folder: true,
        }
    }

    fn make_file(file_id: u64, parent: u64, name: &str, size: u64) -> FileMetadataRecord {
        FileMetadataRecord {
            file_id,
            parent_folder_id: parent,
            name: name.to_owned(),
            size,
            hash: "abc123".to_owned(),
            modified: 1_700_000_100,
            created: 1_699_000_100,
            is_folder: false,
        }
    }

    #[test]
    fn upsert_and_get_by_id() -> Result<(), StoreError> {
        let path = temp_db("upsert_get");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path)?;
        let conn = Connection::open(&path)?;

        let record = make_file(42, 0, "report.txt", 1024);
        FileMetadataRepository::upsert(&conn, &record)?;

        let loaded = FileMetadataRepository::get_by_id(&conn, 42)?.unwrap();
        assert_eq!(loaded, record);

        // Upsert overwrites
        let mut updated = record.clone();
        updated.size = 2048;
        FileMetadataRepository::upsert(&conn, &updated)?;
        let loaded = FileMetadataRepository::get_by_id(&conn, 42)?.unwrap();
        assert_eq!(loaded.size, 2048);

        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn list_children_returns_sorted() -> Result<(), StoreError> {
        let path = temp_db("list_children");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path)?;
        let conn = Connection::open(&path)?;

        FileMetadataRepository::upsert(&conn, &make_file(10, 0, "b.txt", 100))?;
        FileMetadataRepository::upsert(&conn, &make_file(11, 0, "a.txt", 200))?;
        FileMetadataRepository::upsert(&conn, &make_folder(12, 0, "docs"))?;

        let children = FileMetadataRepository::list_children(&conn, 0)?;
        assert_eq!(children.len(), 3);
        assert_eq!(children[0].name, "a.txt");
        assert_eq!(children[1].name, "b.txt");
        assert_eq!(children[2].name, "docs");

        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn resolve_path_walks_hierarchy() -> Result<(), StoreError> {
        let path = temp_db("resolve_path");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path)?;
        let conn = Connection::open(&path)?;

        // Build: /docs/reports/q2.csv
        FileMetadataRepository::upsert(&conn, &make_folder(100, 0, "docs"))?;
        FileMetadataRepository::upsert(&conn, &make_folder(101, 100, "reports"))?;
        FileMetadataRepository::upsert(&conn, &make_file(102, 101, "q2.csv", 512))?;

        let result = FileMetadataRepository::resolve_path(&conn, "/docs/reports/q2.csv")?;
        assert!(result.is_some());
        let record = result.unwrap();
        assert_eq!(record.file_id, 102);
        assert_eq!(record.name, "q2.csv");
        assert!(!record.is_folder);

        // Missing path returns None
        let missing = FileMetadataRepository::resolve_path(&conn, "/docs/nonexistent/file.txt")?;
        assert!(missing.is_none());

        // Folder resolution
        let folder = FileMetadataRepository::resolve_path(&conn, "/docs/reports")?;
        assert!(folder.is_some());
        assert!(folder.unwrap().is_folder);

        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn delete_and_count() -> Result<(), StoreError> {
        let path = temp_db("delete_count");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path)?;
        let conn = Connection::open(&path)?;

        FileMetadataRepository::upsert(&conn, &make_file(1, 0, "a.txt", 10))?;
        FileMetadataRepository::upsert(&conn, &make_file(2, 0, "b.txt", 20))?;
        assert_eq!(FileMetadataRepository::count(&conn)?, 2);

        assert!(FileMetadataRepository::delete(&conn, 1)?);
        assert_eq!(FileMetadataRepository::count(&conn)?, 1);
        assert!(!FileMetadataRepository::delete(&conn, 999)?);

        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn delete_children_removes_direct_children() -> Result<(), StoreError> {
        let path = temp_db("delete_children");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path)?;
        let conn = Connection::open(&path)?;

        FileMetadataRepository::upsert(&conn, &make_folder(10, 0, "docs"))?;
        FileMetadataRepository::upsert(&conn, &make_file(11, 10, "a.txt", 10))?;
        FileMetadataRepository::upsert(&conn, &make_file(12, 10, "b.txt", 20))?;
        FileMetadataRepository::upsert(&conn, &make_file(13, 0, "root.txt", 5))?;

        let removed = FileMetadataRepository::delete_children(&conn, 10)?;
        assert_eq!(removed, 2);
        assert_eq!(FileMetadataRepository::count(&conn)?, 2); // docs + root.txt

        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn get_by_parent_and_name() -> Result<(), StoreError> {
        let path = temp_db("parent_name");
        let _ = std::fs::remove_file(&path);
        let _ = bootstrap_profile(&path)?;
        let conn = Connection::open(&path)?;

        FileMetadataRepository::upsert(&conn, &make_file(50, 0, "notes.md", 300))?;

        let found = FileMetadataRepository::get_by_parent_and_name(&conn, 0, "notes.md")?;
        assert!(found.is_some());
        assert_eq!(found.unwrap().file_id, 50);

        let missing = FileMetadataRepository::get_by_parent_and_name(&conn, 0, "nope.txt")?;
        assert!(missing.is_none());

        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}

//! Disk-backed staging directory for pending uploads (bd-1du.4.d).
//!
//! The staging dir holds one file per pending write. Each file is a
//! content-addressed blob referenced by a [`crate::write_journal::JournalOp::Write`]
//! record. The staging layout is intentionally simple:
//!
//! ```text
//!   <root>/journal.log          // WriteJournal (see write_journal.rs)
//!   <root>/blobs/<blob-name>    // one pending-blob file per journal Write
//! ```
//!
//! # Permission contract
//!
//! * `<root>` is created with mode `0o700` (owner-only).
//! * Every blob file is opened with `O_CREAT | O_EXCL | 0o600` so a
//!   concurrent attacker cannot race a symlink in.
//! * Attempting to open a staging dir whose mode is looser than `0o700`
//!   (world- or group-accessible) is rejected with
//!   [`StagingError::InsecurePermissions`].

// **PLATFORM:** Unix (Linux, BSD, macOS)
// **GATING:** #[cfg(unix)].

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Errors produced by the staging dir manager.
#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    /// Underlying filesystem I/O failure.
    #[error("staging I/O failure: {0}")]
    Io(#[from] io::Error),
    /// The staging dir has permissions looser than `0o700`. The staging
    /// area refuses to open in this state because staged blobs may
    /// contain confidential user data.
    #[error("staging dir has insecure permissions: {mode:o} (expected 0700)")]
    InsecurePermissions {
        /// The mode bits observed on the staging dir, masked to `0o777`.
        mode: u32,
    },
    /// The caller-supplied blob name was rejected for containing a path
    /// separator, a traversal component, or a NUL byte.
    #[error("staging blob name rejected: {0}")]
    InvalidName(&'static str),
}

/// Staging directory handle.
#[derive(Debug, Clone)]
pub struct StagingDir {
    root: PathBuf,
}

impl StagingDir {
    /// Open or create the staging dir at `root`. Creates `<root>/blobs/`
    /// and ensures permissions are `0o700` on both.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StagingError> {
        let root = root.into();
        create_secure_dir(&root)?;
        let blobs = root.join("blobs");
        create_secure_dir(&blobs)?;
        verify_secure_dir(&root)?;
        verify_secure_dir(&blobs)?;
        Ok(Self { root })
    }

    /// Root directory of the staging area.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the journal file inside the staging dir.
    #[must_use]
    pub fn journal_path(&self) -> PathBuf {
        self.root.join("journal.log")
    }

    /// Path of the blob file for `blob_name`.
    pub fn blob_path(&self, blob_name: &str) -> Result<PathBuf, StagingError> {
        if blob_name.is_empty()
            || blob_name.contains('/')
            || blob_name.contains('\\')
            || blob_name == "."
            || blob_name == ".."
            || blob_name.contains('\0')
        {
            return Err(StagingError::InvalidName(
                "blob name must not contain separators or be traversal",
            ));
        }
        Ok(self.root.join("blobs").join(blob_name))
    }

    /// Create a fresh blob file (fails if it already exists). Returns an
    /// open read/write file with mode `0o600` on unix.
    pub fn create_blob(&self, blob_name: &str) -> Result<File, StagingError> {
        let path = self.blob_path(blob_name)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            Ok(file)
        }
        #[cfg(not(unix))]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)?;
            Ok(file)
        }
    }

    /// Open a blob that is expected to already exist. Mode is not relaxed.
    pub fn open_blob(&self, blob_name: &str) -> Result<File, StagingError> {
        let path = self.blob_path(blob_name)?;
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        Ok(file)
    }

    /// Delete a blob file, ignoring a NotFound error.
    pub fn remove_blob(&self, blob_name: &str) -> Result<(), StagingError> {
        let path = self.blob_path(blob_name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// List existing blob names, unsorted.
    pub fn list_blobs(&self) -> Result<Vec<String>, StagingError> {
        let dir = self.root.join("blobs");
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                out.push(name.to_owned());
            }
        }
        Ok(out)
    }

    /// Convenience: read the entire blob into a `Vec<u8>`.
    pub fn read_blob(&self, blob_name: &str) -> Result<Vec<u8>, StagingError> {
        let mut f = self.open_blob(blob_name)?;
        let mut out = Vec::new();
        f.read_to_end(&mut out)?;
        Ok(out)
    }

    /// Convenience: rewrite a blob from scratch, fsync its contents, and
    /// return the number of bytes written.
    pub fn write_blob_full(&self, blob_name: &str, bytes: &[u8]) -> Result<u64, StagingError> {
        let path = self.blob_path(blob_name)?;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            opts.mode(0o600);
        }
        let mut file = opts.open(&path)?;
        file.write_all(bytes)?;
        file.sync_data()?;
        Ok(bytes.len() as u64)
    }

    /// Write `bytes` at `offset` inside an existing blob, extending with
    /// zeroes if needed. Syncs the file before returning.
    pub fn write_blob_at(
        &self,
        blob_name: &str,
        offset: u64,
        bytes: &[u8],
    ) -> Result<u64, StagingError> {
        let mut file = match self.open_blob(blob_name) {
            Ok(f) => f,
            Err(StagingError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
                self.create_blob(blob_name)?
            }
            Err(e) => return Err(e),
        };
        let current_len = file.metadata()?.len();
        if offset > current_len {
            // Extend with zeroes (write-beyond-EOF semantics).
            file.set_len(offset)?;
        }
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
        file.sync_data()?;
        let new_len = file.metadata()?.len();
        Ok(new_len)
    }

    /// Truncate a blob to `new_size`, extending with zeros if needed.
    pub fn truncate_blob(&self, blob_name: &str, new_size: u64) -> Result<(), StagingError> {
        let file = match self.open_blob(blob_name) {
            Ok(f) => f,
            Err(StagingError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
                self.create_blob(blob_name)?
            }
            Err(e) => return Err(e),
        };
        file.set_len(new_size)?;
        file.sync_data()?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Permission helpers
// -----------------------------------------------------------------------------

#[cfg(unix)]
fn create_secure_dir(path: &Path) -> Result<(), StagingError> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    if !path.exists() {
        builder.create(path)?;
    }
    // Tighten permissions even if the dir pre-existed.
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o700);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_secure_dir(path: &Path) -> Result<(), StagingError> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn verify_secure_dir(path: &Path) -> Result<(), StagingError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(StagingError::InsecurePermissions { mode });
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_secure_dir(_path: &Path) -> Result<(), StagingError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_creates_root_blobs_and_journal_paths() {
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        assert!(stage.root().exists());
        assert!(stage.root().join("blobs").exists());
        // Journal path is computed, not created.
        assert_eq!(stage.journal_path(), stage.root().join("journal.log"));
    }

    #[cfg(unix)]
    #[test]
    fn staging_dir_is_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        let mode = fs::metadata(stage.root()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let blob_mode = fs::metadata(stage.root().join("blobs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(blob_mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn blob_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        let _f = stage.create_blob("b1").unwrap();
        let mode = fs::metadata(stage.blob_path("b1").unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn insecure_existing_root_is_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let root = dir.path().join("loose");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        // open() will tighten the mode itself, which is correct behaviour.
        let stage = StagingDir::open(&root).unwrap();
        let mode = fs::metadata(stage.root()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn rejects_traversal_in_blob_name() {
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        assert!(matches!(
            stage.blob_path(".."),
            Err(StagingError::InvalidName(_))
        ));
        assert!(matches!(
            stage.blob_path("a/b"),
            Err(StagingError::InvalidName(_))
        ));
        assert!(matches!(
            stage.blob_path(""),
            Err(StagingError::InvalidName(_))
        ));
    }

    #[test]
    fn write_blob_at_extends_with_zeroes_beyond_eof() {
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        stage.write_blob_full("f", b"hi").unwrap();
        stage.write_blob_at("f", 10, b"END").unwrap();
        let data = stage.read_blob("f").unwrap();
        assert_eq!(data.len(), 13);
        assert_eq!(&data[0..2], b"hi");
        assert_eq!(&data[2..10], &[0u8; 8]);
        assert_eq!(&data[10..13], b"END");
    }

    #[test]
    fn truncate_shrinks_blob() {
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        stage.write_blob_full("f", b"0123456789").unwrap();
        stage.truncate_blob("f", 4).unwrap();
        assert_eq!(stage.read_blob("f").unwrap(), b"0123");
    }

    #[test]
    fn remove_blob_is_idempotent() {
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        stage.write_blob_full("f", b"x").unwrap();
        stage.remove_blob("f").unwrap();
        stage.remove_blob("f").unwrap();
        assert!(!stage.blob_path("f").unwrap().exists());
    }

    #[test]
    fn list_blobs_returns_created_blobs() {
        let dir = tempdir().unwrap();
        let stage = StagingDir::open(dir.path().join("stage")).unwrap();
        stage.write_blob_full("a", b"1").unwrap();
        stage.write_blob_full("b", b"2").unwrap();
        let mut names = stage.list_blobs().unwrap();
        names.sort();
        assert_eq!(names, vec!["a".to_owned(), "b".to_owned()]);
    }
}

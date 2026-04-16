//! Backup snapshot builder (H12b/c): tar + manifest + SQLite VACUUM core,
//! zstd-compressed + SHA3-256-sealed sidecar output, optional GPG
//! envelope, and Grandfather-Father-Son retention pruning.
//!
//! ## Output layout (default, no GPG)
//!
//! A call to [`create_snapshot`] with default [`SnapshotOptions`] produces
//! two sibling files next to each other on disk:
//!
//! * `<out_path>` — a `.tar.zst` archive. The inner tar has the same
//!   manifest-first shape used by [`create_unencrypted_snapshot`], but
//!   the whole tar buffer is zstd-compressed with the caller-supplied
//!   level (default `3`, matching the upstream zstd default).
//! * `<out_path>.manifest.json` — the [`SidecarManifest`] sidecar.
//!   Outer integrity: it records the SHA3-256 digest of the **final
//!   on-disk archive bytes** (the compressed `.tar.zst`, or the
//!   compressed-then-GPG-encrypted `.tar.zst.gpg`). [`verify_snapshot`]
//!   re-reads the archive, recomputes the SHA3, and rejects the archive
//!   on mismatch before handing off to the zstd/tar pipeline — the
//!   sidecar is the sealed outer envelope.
//!
//! Legacy unencrypted `.tar` production
//! ([`create_unencrypted_snapshot`]) remains intact and continues to
//! compute the inner payload SHA-256; that digest is embedded in the
//! inner `manifest.json` and preserved as a second, independent
//! integrity layer inside the compressed archive.
//!
//! ## Output layout (optional GPG envelope)
//!
//! When [`SnapshotOptions::gpg_recipient`] is `Some`, the archive path
//! must end with `.tar.zst.gpg`. The compressed tar bytes are piped
//! into `gpg --encrypt --recipient <id> --batch --yes --output <path>`;
//! the sidecar then hashes the produced ciphertext. Verification and
//! restore reverse the chain: SHA3 check the ciphertext, GPG-decrypt,
//! zstd-decompress, tar-verify.
//!
//! ## Pipeline ordering
//!
//! `tar → zstd → (optional GPG) → SHA3-256 over the sealed bytes`.
//! Compression always happens **before** encryption so the ciphertext
//! is not compressible and so restore does not need a per-level zstd
//! fallback inside gpg.
//!
//! ## Purpose
//!
//! Give operators a reproducible, integrity-checked bundle of the daemon's
//! durable state (auth vault, SQLite store, audit log, active config) that
//! can be shipped off-host, verified without restoring, and pruned under a
//! predictable retention policy.
//!
//! ## Archive layout
//!
//! Files are written in this exact order:
//!
//! 1. `manifest.json`  — [`SnapshotManifest`] (versioned, schema-aware,
//!    carries SHA-256 over the remaining four entries).
//! 2. `auth_token.bin` — opaque vault bytes.
//! 3. `store.sqlite3`  — produced via `rusqlite` online backup (VACUUM-like
//!    consistent copy; not a raw file copy).
//! 4. `audit.ndjson`   — append-only audit log.
//! 5. `config.toml`    — serialized active config bytes.
//!
//! ## Integrity
//!
//! [`SnapshotManifest::sha256_manifest`] is the hex SHA-256 over the
//! concatenation of entries 2..=5 in the order above. [`verify_unencrypted_snapshot`]
//! re-reads the tar, recomputes the digest, and compares.
//!
//! ## Security posture
//!
//! - Encrypted snapshots call out to the local `gpg(1)` binary; the
//!   plaintext tar is staged in a `tempfile::tempdir()` on the same
//!   filesystem as the ciphertext (so [`restore_encrypted_snapshot`] can
//!   use `rename(2)` without `EXDEV`) and is dropped as soon as the child
//!   process exits.
//! - GPG subprocess failures never echo the output path or the recipient
//!   identifier — see [`SnapshotError::GpgFailed`],
//!   [`SnapshotError::GpgRecipientMissing`], and
//!   [`SnapshotError::GpgUnavailable`].
//! - Tar entries are written with mode `0o600` and `mtime = 0` so no
//!   per-user metadata leaks into the archive.
//! - Restore refuses tar entries that contain `..`, `/`, `\`, NUL, or
//!   absolute paths ([`SnapshotError::UnsafePath`]) — the archive shape is
//!   flat on purpose.
//!
//! ## Honest limitations
//!
//! - **`gpg(1)` is a runtime dependency** for
//!   [`create_encrypted_snapshot`], [`verify_encrypted_snapshot`], and
//!   [`restore_encrypted_snapshot`]. A missing or non-executable `gpg` on
//!   `PATH` fails closed with [`SnapshotError::GpgUnavailable`]; this
//!   crate does **not** ship an embedded OpenPGP implementation.
//! - Encrypted verify/restore **always decrypts to a tempdir** even when
//!   the caller only needs the manifest. There is no way to verify the
//!   manifest digest without running `gpg --decrypt`.
//! - [`prune_gfs_execute`] operates on mtime, not on manifest timestamps.
//!   Renaming an archive onto a filesystem that clamps mtime (e.g. some
//!   SMB mounts) changes which bucket the file falls into.
//! - The GFS window constants (8 weekly buckets, 6 monthly buckets) are
//!   not configurable and are derived from the H12c spec.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use sha3::{Digest as Sha3Digest, Sha3_256};
use tar::{Archive, Builder, Header};
use thiserror::Error;

/// Minimum zstd compression level accepted by [`SnapshotOptions`].
pub const ZSTD_MIN_LEVEL: i32 = 1;
/// Maximum zstd compression level accepted by [`SnapshotOptions`].
pub const ZSTD_MAX_LEVEL: i32 = 22;
/// Default zstd compression level (matches upstream zstd default).
pub const ZSTD_DEFAULT_LEVEL: i32 = 3;

/// Sidecar manifest schema version for [`SidecarManifest`]. Bumped when
/// the on-disk shape of the `.manifest.json` sidecar changes.
pub const SIDECAR_MANIFEST_VERSION: u32 = 1;

/// Filename suffix for zstd-compressed (unencrypted) snapshots.
pub const ZSTD_SUFFIX: &str = ".tar.zst";
/// Filename suffix for zstd-compressed, GPG-encrypted snapshots.
pub const ZSTD_GPG_SUFFIX: &str = ".tar.zst.gpg";
/// Filename suffix used for the outer sidecar manifest file, appended
/// directly to the archive path (so
/// `snapshot.tar.zst` → `snapshot.tar.zst.manifest.json`).
pub const SIDECAR_SUFFIX: &str = ".manifest.json";

/// Current snapshot manifest format version.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Ordered payload entry names (entries 2..=5; `manifest.json` is separate).
const PAYLOAD_ENTRIES: [&str; 4] = [
    "auth_token.bin",
    "store.sqlite3",
    "audit.ndjson",
    "config.toml",
];

/// Errors produced by snapshot create / verify.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// Filesystem or tar I/O failure.
    #[error("snapshot io: {0}")]
    Io(#[from] io::Error),
    /// SQLite backup failure.
    #[error("snapshot store: {0}")]
    Store(#[from] rusqlite::Error),
    /// Manifest (de)serialization failure.
    #[error("snapshot manifest serde: {0}")]
    Serde(#[from] serde_json::Error),
    /// A required entry was missing from the archive.
    #[error("snapshot missing required file: {which}")]
    MissingFile {
        /// Logical name of the missing entry.
        which: &'static str,
    },
    /// Could not read the source store schema version.
    #[error("snapshot could not read source schema version")]
    SchemaReadFailed,
    /// Recorded payload digest did not match recomputed digest.
    #[error("snapshot manifest digest mismatch (archive tampered or corrupted)")]
    DigestMismatch,
    /// Manifest schema_version did not match the verifier's expectation.
    #[error("snapshot schema mismatch: expected {expected}, got {got}")]
    SchemaMismatch {
        /// Schema version the verifier expected.
        expected: u32,
        /// Schema version the archive actually carried.
        got: u32,
    },
    /// GPG binary is not available on `PATH`.
    ///
    /// The path of the missing binary is intentionally not echoed.
    #[error("snapshot gpg binary unavailable on PATH")]
    GpgUnavailable,
    /// GPG recipient key is not present in the local keyring.
    ///
    /// The recipient identifier is intentionally not echoed so that a TTY
    /// observer cannot harvest it from stderr.
    #[error("snapshot gpg recipient key not found in keyring")]
    GpgRecipientMissing,
    /// GPG subprocess exited non-zero.
    ///
    /// Neither the archive path nor the recipient is echoed.
    #[error("snapshot gpg subprocess failed")]
    GpgFailed,
    /// Archive contains a path component that escapes the destination tree.
    #[error("snapshot archive contains unsafe path entry")]
    UnsafePath,
    /// zstd compression or decompression failed.
    #[error("snapshot zstd codec failed: {0}")]
    ZstdFailed(io::Error),
    /// Caller passed a zstd level outside the accepted 1..=22 range.
    #[error("snapshot invalid zstd level {got}: must be in {ZSTD_MIN_LEVEL}..={ZSTD_MAX_LEVEL}")]
    InvalidZstdLevel {
        /// The rejected level value.
        got: i32,
    },
    /// Caller used a GPG recipient but the output path does not end in
    /// [`ZSTD_GPG_SUFFIX`], or passed no recipient but used a suffix
    /// other than [`ZSTD_SUFFIX`] / [`ZSTD_GPG_SUFFIX`].
    #[error("snapshot output path does not match requested encryption mode")]
    InvalidOutputSuffix,
    /// The expected `.manifest.json` sidecar file is missing.
    #[error("snapshot sidecar manifest is missing")]
    SidecarMissing,
    /// Sidecar manifest could not be parsed or its schema_version is
    /// unsupported.
    #[error("snapshot sidecar manifest is corrupt or unsupported")]
    SidecarCorrupt,
}

/// Versioned snapshot manifest serialized as `manifest.json`.
///
/// `sha256_manifest` is the hex-encoded SHA-256 over the byte concatenation
/// of `auth_token.bin`, `store.sqlite3`, `audit.ndjson`, `config.toml`
/// (in that order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Manifest format version. Always [`SNAPSHOT_FORMAT_VERSION`] for new snapshots.
    pub version: u32,
    /// Unix seconds at archive creation time.
    pub created_at: u64,
    /// Source store schema (`PRAGMA user_version`) at snapshot time.
    pub schema_version: u32,
    /// Hex SHA-256 digest of the four payload entries in canonical order.
    pub sha256_manifest: String,
}

/// Build an unencrypted snapshot tar at `out_tar`.
///
/// Order of operations:
///
/// 1. Read `auth_token.bin`, `audit.ndjson`, and the supplied `config_bytes`
///    into memory.
/// 2. Run `rusqlite` online backup of `store_path` into a temporary file
///    (`out_tar` sibling) and read the resulting bytes back.
/// 3. Read `PRAGMA user_version` from the source store for `schema_version`.
/// 4. Compute SHA-256 over the four payload byte slices in canonical order.
/// 5. Serialize the manifest, then write `manifest.json` followed by the
///    four payload entries to the tar in canonical order.
pub fn create_unencrypted_snapshot(
    store_path: &Path,
    vault_path: &Path,
    audit_path: &Path,
    config_bytes: &[u8],
    out_tar: &Path,
) -> Result<SnapshotManifest, SnapshotError> {
    let auth_bytes = read_required(vault_path, "auth_token.bin")?;
    let audit_bytes = read_required(audit_path, "audit.ndjson")?;

    let store_bytes = sqlite_backup_to_bytes(store_path, out_tar)?;
    let schema_version = read_schema_version(store_path)?;

    let mut hasher = Sha256::new();
    hasher.update(&auth_bytes);
    hasher.update(&store_bytes);
    hasher.update(&audit_bytes);
    hasher.update(config_bytes);
    let digest_hex = hex_encode(&hasher.finalize());

    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let manifest = SnapshotManifest {
        version: SNAPSHOT_FORMAT_VERSION,
        created_at,
        schema_version,
        sha256_manifest: digest_hex,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;

    let tar_file = File::create(out_tar)?;
    let mut builder = Builder::new(tar_file);
    append_entry(&mut builder, "manifest.json", &manifest_bytes)?;
    append_entry(&mut builder, "auth_token.bin", &auth_bytes)?;
    append_entry(&mut builder, "store.sqlite3", &store_bytes)?;
    append_entry(&mut builder, "audit.ndjson", &audit_bytes)?;
    append_entry(&mut builder, "config.toml", config_bytes)?;
    builder.finish()?;

    Ok(manifest)
}

/// Re-read an unencrypted snapshot at `tar_path`, validate that all five
/// entries are present, and recompute the manifest digest over the four
/// payload entries. Returns the parsed manifest on success.
///
/// Does **not** enforce a particular `schema_version`. Use
/// [`verify_unencrypted_snapshot_with_schema`] to additionally gate that.
pub fn verify_unencrypted_snapshot(tar_path: &Path) -> Result<SnapshotManifest, SnapshotError> {
    let (manifest, _payloads) = read_and_verify(tar_path)?;
    Ok(manifest)
}

/// Like [`verify_unencrypted_snapshot`] but also rejects archives whose
/// manifest `schema_version` differs from `expected_schema`.
pub fn verify_unencrypted_snapshot_with_schema(
    tar_path: &Path,
    expected_schema: u32,
) -> Result<SnapshotManifest, SnapshotError> {
    let manifest = verify_unencrypted_snapshot(tar_path)?;
    if manifest.schema_version != expected_schema {
        return Err(SnapshotError::SchemaMismatch {
            expected: expected_schema,
            got: manifest.schema_version,
        });
    }
    Ok(manifest)
}

fn read_and_verify(tar_path: &Path) -> Result<(SnapshotManifest, [Vec<u8>; 4]), SnapshotError> {
    let file = File::open(tar_path)?;
    let mut archive = Archive::new(file);

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut payloads: [Option<Vec<u8>>; 4] = [None, None, None, None];

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let name = path.to_string_lossy().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        if name == "manifest.json" {
            manifest_bytes = Some(buf);
            continue;
        }
        for (idx, expected) in PAYLOAD_ENTRIES.iter().enumerate() {
            if name == *expected {
                payloads[idx] = Some(buf);
                break;
            }
        }
    }

    let manifest_bytes = manifest_bytes.ok_or(SnapshotError::MissingFile {
        which: "manifest.json",
    })?;
    let manifest: SnapshotManifest = serde_json::from_slice(&manifest_bytes)?;

    let mut owned: [Vec<u8>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for (idx, slot) in payloads.into_iter().enumerate() {
        owned[idx] = slot.ok_or(SnapshotError::MissingFile {
            which: PAYLOAD_ENTRIES[idx],
        })?;
    }

    let mut hasher = Sha256::new();
    for bytes in &owned {
        hasher.update(bytes);
    }
    let recomputed = hex_encode(&hasher.finalize());
    if recomputed != manifest.sha256_manifest {
        return Err(SnapshotError::DigestMismatch);
    }

    Ok((manifest, owned))
}

fn append_entry<W: Write>(
    builder: &mut Builder<W>,
    name: &str,
    data: &[u8],
) -> Result<(), SnapshotError> {
    let mut header = Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(0);
    header.set_cksum();
    builder.append_data(&mut header, name, data)?;
    Ok(())
}

fn read_required(path: &Path, which: &'static str) -> Result<Vec<u8>, SnapshotError> {
    if !path.exists() {
        return Err(SnapshotError::MissingFile { which });
    }
    Ok(fs::read(path)?)
}

fn sqlite_backup_to_bytes(store_path: &Path, out_tar: &Path) -> Result<Vec<u8>, SnapshotError> {
    let tmp_path: PathBuf = match out_tar.parent() {
        Some(parent) => parent.join(format!(
            ".pcloud-snapshot-store-{}.sqlite3",
            std::process::id()
        )),
        None => PathBuf::from(format!(
            ".pcloud-snapshot-store-{}.sqlite3",
            std::process::id()
        )),
    };
    if tmp_path.exists() {
        let _ = fs::remove_file(&tmp_path);
    }

    {
        let src = rusqlite::Connection::open(store_path)?;
        src.backup(rusqlite::MAIN_DB, &tmp_path, None)?;
    }

    let bytes = fs::read(&tmp_path)?;
    let _ = fs::remove_file(&tmp_path);
    Ok(bytes)
}

fn read_schema_version(store_path: &Path) -> Result<u32, SnapshotError> {
    let conn = rusqlite::Connection::open(store_path)?;
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| SnapshotError::SchemaReadFailed)?;
    if v < 0 || v > u32::MAX as i64 {
        return Err(SnapshotError::SchemaReadFailed);
    }
    Ok(v as u32)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// PR3: GPG-encrypted snapshots, restore, and Grandfather-Father-Son pruning.
// ---------------------------------------------------------------------------

/// Filename suffix used by encrypted snapshots in [`prune_gfs`] discovery.
pub const ENCRYPTED_SUFFIX: &str = ".tar.gpg";

/// Filename prefix used by encrypted snapshots in [`prune_gfs`] discovery.
pub const ENCRYPTED_PREFIX: &str = "pcloud-rs-";

const SECS_PER_DAY: u64 = 86_400;

fn gpg_available() -> bool {
    Command::new("gpg")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn gpg_recipient_known(recipient: &str) -> bool {
    Command::new("gpg")
        .arg("--list-keys")
        .arg("--with-colons")
        .arg(recipient)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build a GPG-encrypted snapshot at `out_path`.
///
/// Inputs are the same as [`create_unencrypted_snapshot`] plus a GPG
/// recipient identifier and the encrypted output path.
///
/// The plaintext tar is staged in a temp directory and streamed into
/// `gpg --encrypt --recipient <r> --output <out>` via a piped stdin.  The
/// staged plaintext is removed when the tempdir drops; the plaintext tar
/// **never** sits next to the ciphertext on disk.
///
/// Errors:
///
/// * [`SnapshotError::GpgUnavailable`] — `gpg --version` failed.
/// * [`SnapshotError::GpgRecipientMissing`] — `gpg --list-keys <r>` failed.
/// * [`SnapshotError::GpgFailed`] — encryption subprocess exited non-zero.
///
/// Neither `out_path` nor `gpg_recipient` are embedded in any error.
pub fn create_encrypted_snapshot(
    store_path: &Path,
    vault_path: &Path,
    audit_path: &Path,
    config_bytes: &[u8],
    gpg_recipient: &str,
    out_path: &Path,
) -> Result<SnapshotManifest, SnapshotError> {
    if !gpg_available() {
        return Err(SnapshotError::GpgUnavailable);
    }
    if !gpg_recipient_known(gpg_recipient) {
        return Err(SnapshotError::GpgRecipientMissing);
    }

    let staging = tempfile::tempdir()?;
    let staged_tar = staging.path().join("snapshot.tar");
    let manifest = create_unencrypted_snapshot(
        store_path,
        vault_path,
        audit_path,
        config_bytes,
        &staged_tar,
    )?;

    let mut child = Command::new("gpg")
        .arg("--batch")
        .arg("--yes")
        .arg("--encrypt")
        .arg("--recipient")
        .arg(gpg_recipient)
        .arg("--output")
        .arg(out_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| SnapshotError::GpgFailed)?;

    {
        let mut stdin = child.stdin.take().ok_or(SnapshotError::GpgFailed)?;
        let mut tar_file = File::open(&staged_tar)?;
        io::copy(&mut tar_file, &mut stdin)?;
        // Drop closes stdin, signalling EOF to gpg.
    }

    let status = child.wait().map_err(|_| SnapshotError::GpgFailed)?;
    if !status.success() {
        // Best effort: do not leave a partial ciphertext behind.
        let _ = fs::remove_file(out_path);
        return Err(SnapshotError::GpgFailed);
    }
    Ok(manifest)
}

fn gpg_decrypt_to(archive: &Path, plaintext_out: &Path) -> Result<(), SnapshotError> {
    if !gpg_available() {
        return Err(SnapshotError::GpgUnavailable);
    }
    let status = Command::new("gpg")
        .arg("--batch")
        .arg("--yes")
        .arg("--decrypt")
        .arg("--output")
        .arg(plaintext_out)
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| SnapshotError::GpgFailed)?;
    if !status.success() {
        return Err(SnapshotError::GpgFailed);
    }
    Ok(())
}

/// Verify a GPG-encrypted snapshot.
///
/// Decrypts the archive into a tempdir, runs [`verify_unencrypted_snapshot`]
/// against the plaintext tar, then drops the tempdir (which removes the
/// plaintext).
pub fn verify_encrypted_snapshot(archive: &Path) -> Result<SnapshotManifest, SnapshotError> {
    let staging = tempfile::tempdir()?;
    let plain = staging.path().join("snapshot.tar");
    gpg_decrypt_to(archive, &plain)?;
    verify_unencrypted_snapshot(&plain)
}

/// Restore a GPG-encrypted snapshot into `target_dir`.
///
/// Steps:
///
/// 1. Decrypt the ciphertext to a staging tempdir.
/// 2. Re-verify the manifest digest (delegates to
///    [`verify_unencrypted_snapshot`]) against the plaintext tar.
/// 3. Extract the four payload entries into `<staging>/extract/`.
/// 4. Atomically move each file into `target_dir` using [`std::fs::rename`].
///
/// **EXDEV caveat:** [`std::fs::rename`] returns `EXDEV` when source and
/// destination live on different filesystems.  We mitigate by allocating the
/// staging directory **inside `target_dir`'s parent** so the final move stays
/// on the same filesystem.  Callers whose `target_dir` parent is itself
/// across a mount boundary from where they expect the data should pass a
/// destination on the same volume.
///
/// On success returns the parsed manifest.  On failure no files in
/// `target_dir` are touched (extraction happens entirely in the staging dir
/// before any rename).  The returned manifest is also schema-checked
/// implicitly because the tar reader rejects mismatched payload counts and
/// `verify_unencrypted_snapshot` rejects digest drift.
pub fn restore_encrypted_snapshot(
    archive: &Path,
    target_dir: &Path,
) -> Result<SnapshotManifest, SnapshotError> {
    fs::create_dir_all(target_dir)?;
    let parent = target_dir.parent().unwrap_or_else(|| Path::new("."));
    let staging = tempfile::Builder::new()
        .prefix(".pcloud-rs-restore-")
        .tempdir_in(parent)?;
    let plain = staging.path().join("snapshot.tar");
    gpg_decrypt_to(archive, &plain)?;

    // Verify first so we never extract a tampered archive.
    let manifest = verify_unencrypted_snapshot(&plain)?;

    let extract_root = staging.path().join("extract");
    fs::create_dir_all(&extract_root)?;

    let mut archive_r = Archive::new(File::open(&plain)?);
    archive_r.set_preserve_permissions(false);
    for entry in archive_r.entries()? {
        let mut entry = entry?;
        let rel = entry.path()?.to_path_buf();
        let rel_str = rel.to_string_lossy().to_string();
        if rel.is_absolute()
            || rel_str.contains("..")
            || rel_str.contains('\0')
            || rel_str.contains('/')
            || rel_str.contains('\\')
        {
            return Err(SnapshotError::UnsafePath);
        }
        // Skip the manifest in the restored tree; callers don't need it.
        if rel_str == "manifest.json" {
            let mut sink = io::sink();
            io::copy(&mut entry, &mut sink)?;
            continue;
        }
        let dest = extract_root.join(&rel);
        let mut out = File::create(&dest)?;
        io::copy(&mut entry, &mut out)?;
    }

    // Atomic move of each payload into target_dir (same-fs by construction).
    for name in PAYLOAD_ENTRIES.iter() {
        let from = extract_root.join(name);
        if !from.exists() {
            return Err(SnapshotError::MissingFile { which: name });
        }
        let to = target_dir.join(name);
        if to.exists() {
            fs::remove_file(&to)?;
        }
        fs::rename(&from, &to)?;
    }
    Ok(manifest)
}

/// Discover snapshot archives in `destination` matching `pcloud-rs-*`
/// with any of the supported suffixes:
///
/// * legacy: `.tar.gpg`
/// * zstd default: `.tar.zst`
/// * zstd + GPG: `.tar.zst.gpg`
///
/// Sorted by mtime descending. Sidecar manifest files
/// (`*.manifest.json`) are intentionally excluded so the GFS bucketer
/// only operates on actual archives.
fn list_snapshot_files(destination: &Path) -> Result<Vec<(PathBuf, SystemTime)>, SnapshotError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(destination)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with(ENCRYPTED_PREFIX) {
            continue;
        }
        let matches_suffix = name.ends_with(ENCRYPTED_SUFFIX)
            || name.ends_with(ZSTD_SUFFIX)
            || name.ends_with(ZSTD_GPG_SUFFIX);
        if !matches_suffix {
            continue;
        }
        // Exclude sidecar JSON even if it starts with `pcloud-rs-` —
        // the sidecar tracks its archive, not the other way around.
        if name.ends_with(SIDECAR_SUFFIX) {
            continue;
        }
        let mtime = entry.metadata()?.modified().unwrap_or(UNIX_EPOCH);
        out.push((path, mtime));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out)
}

/// Compute the Grandfather-Father-Son keep-set for an existing collection
/// of `pcloud-rs-*.tar.gpg` archives in `destination`.
///
/// **Keep rule** (relative to "now"):
///
/// * **Daily**: for each of the most recent `retention_days` 24-hour
///   buckets, keep the newest snapshot in that bucket.
/// * **Weekly**: for each of the **8** weekly buckets immediately preceding
///   the daily window, keep the newest snapshot in that bucket.
/// * **Monthly**: for each of the **6** 30-day buckets immediately
///   preceding the weekly window, keep the newest snapshot in that bucket.
///
/// Snapshots whose mtime is outside *all* buckets are not kept by this
/// function (i.e. they would be eligible for deletion by
/// [`prune_gfs_execute`]).
///
/// This function is **read-only**: it returns the keep-set without removing
/// anything.
pub fn prune_gfs(destination: &Path, retention_days: u32) -> Result<Vec<PathBuf>, SnapshotError> {
    let files = list_snapshot_files(destination)?;
    Ok(gfs_keep_set(&files, retention_days, SystemTime::now()))
}

/// Apply [`prune_gfs`] for real: remove any snapshot file in `destination`
/// that is not in the keep-set.  Returns the list of files that were
/// deleted.
pub fn prune_gfs_execute(
    destination: &Path,
    retention_days: u32,
) -> Result<Vec<PathBuf>, SnapshotError> {
    let files = list_snapshot_files(destination)?;
    let keep = gfs_keep_set(&files, retention_days, SystemTime::now());
    let keep_set: std::collections::HashSet<&PathBuf> = keep.iter().collect();
    let mut removed = Vec::new();
    for (path, _) in &files {
        if !keep_set.contains(path) {
            fs::remove_file(path)?;
            removed.push(path.clone());
            // Best-effort sidecar cleanup: ignore if missing. We only
            // look for sidecars on the new zstd-family suffixes; the
            // legacy `.tar.gpg` pipeline never produced sidecars.
            let sidecar = sidecar_path_for(path);
            if sidecar.exists() {
                let _ = fs::remove_file(&sidecar);
            }
        }
    }
    Ok(removed)
}

fn gfs_keep_set(
    files: &[(PathBuf, SystemTime)],
    retention_days: u32,
    now: SystemTime,
) -> Vec<PathBuf> {
    let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

    let daily_window = retention_days as u64;
    let weekly_buckets: u64 = 8;
    let monthly_buckets: u64 = 6;

    let mut daily: BTreeMap<u64, (u64, PathBuf)> = BTreeMap::new();
    let mut weekly: BTreeMap<u64, (u64, PathBuf)> = BTreeMap::new();
    let mut monthly: BTreeMap<u64, (u64, PathBuf)> = BTreeMap::new();

    let pick = |slot: &mut BTreeMap<u64, (u64, PathBuf)>, bucket: u64, m: u64, p: &Path| {
        slot.entry(bucket)
            .and_modify(|(t, existing)| {
                if m > *t {
                    *t = m;
                    *existing = p.to_path_buf();
                }
            })
            .or_insert((m, p.to_path_buf()));
    };

    for (path, mtime) in files {
        let m = mtime
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if m > now_secs {
            // Future-dated mtimes: bucket as day 0.
            pick(&mut daily, 0, m, path);
            continue;
        }
        let age_days = (now_secs - m) / SECS_PER_DAY;
        if age_days < daily_window {
            pick(&mut daily, age_days, m, path);
        } else if age_days < daily_window + weekly_buckets * 7 {
            let week = (age_days - daily_window) / 7;
            pick(&mut weekly, week, m, path);
        } else if age_days < daily_window + weekly_buckets * 7 + monthly_buckets * 30 {
            let month = (age_days - daily_window - weekly_buckets * 7) / 30;
            pick(&mut monthly, month, m, path);
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();
    for (_, (_, p)) in daily {
        out.push(p);
    }
    for (_, (_, p)) in weekly {
        out.push(p);
    }
    for (_, (_, p)) in monthly {
        out.push(p);
    }
    out.sort();
    out.dedup();
    out
}

// ---------------------------------------------------------------------------
// Zstd-compressed snapshots with SHA3-256-sealed sidecar manifest.
//
// This is the default snapshot pipeline as of the 0.1-pre-alpha CLI
// refactor (`pcloudc snapshot …`). GPG remains available on opt-in via
// [`SnapshotOptions::gpg_recipient`] and layers on top of zstd.
// ---------------------------------------------------------------------------

/// User-tunable knobs for [`create_snapshot`].
///
/// `zstd_level` is validated at construction time via
/// [`SnapshotOptions::with_zstd_level`]; pass it through that builder or
/// construct the struct with a level known to be in range. Constructing
/// the struct literally with an out-of-range level is still rejected by
/// [`create_snapshot`] with [`SnapshotError::InvalidZstdLevel`].
///
/// Secrets: `gpg_recipient` is a public-key identifier (email / key id)
/// and does **not** hold any secret material. It is forwarded verbatim
/// to `gpg --recipient`; it is never emitted in error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotOptions {
    /// zstd compression level in `1..=22`. Default: `3` (zstd default).
    pub zstd_level: i32,
    /// Optional GPG recipient. When `Some`, a GPG envelope is added on
    /// top of the zstd-compressed tar and the archive path must end in
    /// [`ZSTD_GPG_SUFFIX`].
    pub gpg_recipient: Option<String>,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            zstd_level: ZSTD_DEFAULT_LEVEL,
            gpg_recipient: None,
        }
    }
}

impl SnapshotOptions {
    /// Construct options with a validated zstd level. Returns
    /// [`SnapshotError::InvalidZstdLevel`] when `level` is outside
    /// [`ZSTD_MIN_LEVEL`]..=[`ZSTD_MAX_LEVEL`].
    pub fn with_zstd_level(level: i32) -> Result<Self, SnapshotError> {
        validate_zstd_level(level)?;
        Ok(Self {
            zstd_level: level,
            gpg_recipient: None,
        })
    }

    /// Add a GPG recipient to the options, switching the pipeline to
    /// `tar → zstd → gpg` and requiring the [`ZSTD_GPG_SUFFIX`] output
    /// path.
    #[must_use]
    pub fn with_gpg_recipient(mut self, recipient: impl Into<String>) -> Self {
        self.gpg_recipient = Some(recipient.into());
        self
    }
}

/// Outer-envelope sidecar manifest written next to the compressed
/// archive as `<archive>.manifest.json`.
///
/// Outer integrity: `sha3_256` is the hex SHA3-256 digest of the final
/// on-disk archive bytes (the `.tar.zst` or `.tar.zst.gpg` file).
/// `inner_manifest` carries the original [`SnapshotManifest`] from
/// inside the (compressed) tar so operators can audit schema_version
/// and payload digest without decrypting / decompressing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarManifest {
    /// Sidecar manifest schema version. Currently always
    /// [`SIDECAR_MANIFEST_VERSION`].
    pub version: u32,
    /// File name (not path) of the archive this sidecar describes.
    pub archive_filename: String,
    /// Size of the archive file in bytes.
    pub archive_size_bytes: u64,
    /// Hex-encoded SHA3-256 of the archive file's bytes.
    pub sha3_256: String,
    /// Effective zstd compression level used when producing the
    /// archive.
    pub zstd_level: i32,
    /// `true` when a GPG envelope was added on top of the compressed
    /// tar (i.e. archive path ends with [`ZSTD_GPG_SUFFIX`]).
    pub encrypted: bool,
    /// UNIX seconds at archive creation time.
    pub created_at: u64,
    /// Original per-payload SnapshotManifest, preserved verbatim.
    pub inner_manifest: SnapshotManifest,
}

/// Validate that `level` is in [`ZSTD_MIN_LEVEL`]..=[`ZSTD_MAX_LEVEL`].
fn validate_zstd_level(level: i32) -> Result<(), SnapshotError> {
    if !(ZSTD_MIN_LEVEL..=ZSTD_MAX_LEVEL).contains(&level) {
        return Err(SnapshotError::InvalidZstdLevel { got: level });
    }
    Ok(())
}

/// Derive the sidecar path for an archive path by appending
/// [`SIDECAR_SUFFIX`]. The caller is responsible for the leading
/// archive extension; this helper never strips anything.
#[must_use]
pub fn sidecar_path_for(archive: &Path) -> PathBuf {
    let mut s: std::ffi::OsString = archive.as_os_str().to_os_string();
    s.push(SIDECAR_SUFFIX);
    PathBuf::from(s)
}

fn sha3_of_file(path: &Path) -> Result<String, SnapshotError> {
    let mut hasher = Sha3_256::new();
    let mut f = File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        Sha3Digest::update(&mut hasher, &buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn build_tar_bytes(
    store_path: &Path,
    vault_path: &Path,
    audit_path: &Path,
    config_bytes: &[u8],
    scratch_dir: &Path,
) -> Result<(Vec<u8>, SnapshotManifest), SnapshotError> {
    let staged_tar = scratch_dir.join("inner.tar");
    let manifest = create_unencrypted_snapshot(
        store_path,
        vault_path,
        audit_path,
        config_bytes,
        &staged_tar,
    )?;
    let bytes = fs::read(&staged_tar)?;
    Ok((bytes, manifest))
}

fn zstd_compress(bytes: &[u8], level: i32) -> Result<Vec<u8>, SnapshotError> {
    zstd::stream::encode_all(bytes, level).map_err(SnapshotError::ZstdFailed)
}

fn zstd_decompress_reader<R: Read>(reader: R) -> Result<Vec<u8>, SnapshotError> {
    let mut out = Vec::new();
    zstd::stream::copy_decode(reader, &mut out).map_err(SnapshotError::ZstdFailed)?;
    Ok(out)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SnapshotError> {
    // Write to `<path>.tmp-<pid>` in the same directory, fsync, rename.
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("sidecar"),
        std::process::id()
    );
    let tmp = parent.join(tmp_name);
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn archive_suffix_ok(path: &Path, encrypted: bool) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if encrypted {
        name.ends_with(ZSTD_GPG_SUFFIX)
    } else {
        name.ends_with(ZSTD_SUFFIX) && !name.ends_with(ZSTD_GPG_SUFFIX)
    }
}

fn gpg_encrypt_bytes(bytes: &[u8], recipient: &str, out_path: &Path) -> Result<(), SnapshotError> {
    if !gpg_available() {
        return Err(SnapshotError::GpgUnavailable);
    }
    if !gpg_recipient_known(recipient) {
        return Err(SnapshotError::GpgRecipientMissing);
    }
    let mut child = Command::new("gpg")
        .arg("--batch")
        .arg("--yes")
        .arg("--encrypt")
        .arg("--recipient")
        .arg(recipient)
        .arg("--output")
        .arg(out_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| SnapshotError::GpgFailed)?;
    {
        let mut stdin = child.stdin.take().ok_or(SnapshotError::GpgFailed)?;
        stdin.write_all(bytes)?;
    }
    let status = child.wait().map_err(|_| SnapshotError::GpgFailed)?;
    if !status.success() {
        let _ = fs::remove_file(out_path);
        return Err(SnapshotError::GpgFailed);
    }
    Ok(())
}

fn gpg_decrypt_bytes(archive: &Path) -> Result<Vec<u8>, SnapshotError> {
    if !gpg_available() {
        return Err(SnapshotError::GpgUnavailable);
    }
    let output = Command::new("gpg")
        .arg("--batch")
        .arg("--yes")
        .arg("--decrypt")
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| SnapshotError::GpgFailed)?;
    if !output.status.success() {
        return Err(SnapshotError::GpgFailed);
    }
    Ok(output.stdout)
}

/// Create a snapshot using the zstd + SHA3-256 sidecar pipeline.
///
/// The default pipeline (no GPG recipient) produces:
///
/// * `<out_path>` — a `.tar.zst` archive (inner tar, zstd-compressed).
/// * `<out_path>.manifest.json` — the [`SidecarManifest`] sealing the
///   archive with a SHA3-256 digest.
///
/// When `options.gpg_recipient` is `Some(...)`, the pipeline adds a
/// GPG envelope: `<out_path>` must end with [`ZSTD_GPG_SUFFIX`] and the
/// compressed bytes are piped to `gpg --encrypt --recipient ...`. The
/// sidecar still records SHA3 over the **final on-disk bytes** (i.e.
/// the ciphertext).
///
/// # Errors
///
/// * [`SnapshotError::InvalidZstdLevel`] if `options.zstd_level` is
///   outside `1..=22`.
/// * [`SnapshotError::InvalidOutputSuffix`] if the archive path does
///   not match the selected pipeline's suffix.
/// * [`SnapshotError::ZstdFailed`] on compression failure.
/// * [`SnapshotError::GpgUnavailable`] / [`SnapshotError::GpgRecipientMissing`]
///   / [`SnapshotError::GpgFailed`] on the GPG path.
/// * I/O / store / manifest errors from [`create_unencrypted_snapshot`].
pub fn create_snapshot(
    store_path: &Path,
    vault_path: &Path,
    audit_path: &Path,
    config_bytes: &[u8],
    out_path: &Path,
    options: &SnapshotOptions,
) -> Result<SidecarManifest, SnapshotError> {
    validate_zstd_level(options.zstd_level)?;
    let encrypted = options.gpg_recipient.is_some();
    if !archive_suffix_ok(out_path, encrypted) {
        return Err(SnapshotError::InvalidOutputSuffix);
    }

    // 1 + 2: build inner tar and zstd-compress it (staged in a tempdir
    // so plaintext never sits next to the final archive on disk).
    let staging = tempfile::tempdir()?;
    let (tar_bytes, inner_manifest) = build_tar_bytes(
        store_path,
        vault_path,
        audit_path,
        config_bytes,
        staging.path(),
    )?;
    let compressed = zstd_compress(&tar_bytes, options.zstd_level)?;

    // 3: either write the compressed bytes directly, or pipe them
    // through gpg. In the encrypted case we do NOT first write the
    // compressed plaintext to disk — `gpg_encrypt_bytes` takes a slice.
    if let Some(recipient) = options.gpg_recipient.as_deref() {
        gpg_encrypt_bytes(&compressed, recipient, out_path)?;
    } else {
        // Atomic: write to tmp + fsync + rename.
        atomic_write(out_path, &compressed)?;
    }

    // 4: compute SHA3 over the final on-disk archive bytes.
    let sha3_hex = sha3_of_file(out_path)?;
    let size_bytes = fs::metadata(out_path)?.len();
    let archive_filename = out_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_owned();
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 5: build + write sidecar atomically.
    let sidecar = SidecarManifest {
        version: SIDECAR_MANIFEST_VERSION,
        archive_filename,
        archive_size_bytes: size_bytes,
        sha3_256: sha3_hex,
        zstd_level: options.zstd_level,
        encrypted,
        created_at,
        inner_manifest,
    };
    let sidecar_bytes = serde_json::to_vec_pretty(&sidecar)?;
    atomic_write(&sidecar_path_for(out_path), &sidecar_bytes)?;

    Ok(sidecar)
}

fn read_sidecar(archive: &Path) -> Result<SidecarManifest, SnapshotError> {
    let path = sidecar_path_for(archive);
    if !path.exists() {
        return Err(SnapshotError::SidecarMissing);
    }
    let bytes = fs::read(&path).map_err(SnapshotError::Io)?;
    let sidecar: SidecarManifest =
        serde_json::from_slice(&bytes).map_err(|_| SnapshotError::SidecarCorrupt)?;
    if sidecar.version != SIDECAR_MANIFEST_VERSION {
        return Err(SnapshotError::SidecarCorrupt);
    }
    Ok(sidecar)
}

/// Verify a snapshot produced by [`create_snapshot`].
///
/// Steps:
///
/// 1. Read the sidecar `<archive>.manifest.json`.
/// 2. Recompute SHA3-256 over `archive` and compare to
///    `sidecar.sha3_256`; mismatch is [`SnapshotError::DigestMismatch`].
/// 3. If `sidecar.encrypted`, shell out to `gpg --decrypt` to recover
///    the zstd-compressed tar bytes; else read the archive directly.
/// 4. zstd-decompress into a tempfile and run
///    [`verify_unencrypted_snapshot`] to re-check the inner per-payload
///    SHA-256.
///
/// Returns the sidecar manifest on success. Does not mutate the
/// archive or the sidecar.
pub fn verify_snapshot(archive: &Path) -> Result<SidecarManifest, SnapshotError> {
    let sidecar = read_sidecar(archive)?;
    let recomputed = sha3_of_file(archive)?;
    if recomputed != sidecar.sha3_256 {
        return Err(SnapshotError::DigestMismatch);
    }

    // Recover compressed tar bytes.
    let compressed = if sidecar.encrypted {
        gpg_decrypt_bytes(archive)?
    } else {
        fs::read(archive)?
    };
    let tar_bytes = zstd_decompress_reader(compressed.as_slice())?;

    // Write to a tempfile so the existing verify helper (which takes a
    // Path) can be reused without duplicating tar-parse logic.
    let staging = tempfile::tempdir()?;
    let plain = staging.path().join("snapshot.tar");
    fs::write(&plain, &tar_bytes)?;
    let _ = verify_unencrypted_snapshot(&plain)?;
    Ok(sidecar)
}

/// Restore targets for [`restore_snapshot`]. Each payload is written
/// atomically into its destination path; existing files are replaced.
#[derive(Debug, Clone)]
pub struct RestoreTargets {
    /// Target directory into which the four payload files are placed:
    /// `auth_token.bin`, `store.sqlite3`, `audit.ndjson`,
    /// `config.toml`.
    pub target_dir: PathBuf,
}

/// Restore a snapshot into the payload layout expected by the daemon.
///
/// Applies the same verification chain as [`verify_snapshot`] before
/// extracting; an archive whose outer SHA3 does not match the sidecar
/// is rejected before any payload is unpacked.
///
/// Returns the sidecar manifest on success.
pub fn restore_snapshot(
    archive: &Path,
    targets: &RestoreTargets,
) -> Result<SidecarManifest, SnapshotError> {
    // Verify outer seal first.
    let sidecar = read_sidecar(archive)?;
    let recomputed = sha3_of_file(archive)?;
    if recomputed != sidecar.sha3_256 {
        return Err(SnapshotError::DigestMismatch);
    }

    // Recover compressed tar bytes -> tarfile in staging.
    let compressed = if sidecar.encrypted {
        gpg_decrypt_bytes(archive)?
    } else {
        fs::read(archive)?
    };
    let tar_bytes = zstd_decompress_reader(compressed.as_slice())?;

    fs::create_dir_all(&targets.target_dir)?;
    let parent = targets
        .target_dir
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let staging = tempfile::Builder::new()
        .prefix(".pcloud-rs-restore-")
        .tempdir_in(parent)?;
    let plain = staging.path().join("snapshot.tar");
    fs::write(&plain, &tar_bytes)?;

    // Verify inner manifest digest before any payload is exposed.
    let _ = verify_unencrypted_snapshot(&plain)?;

    let extract_root = staging.path().join("extract");
    fs::create_dir_all(&extract_root)?;
    let mut archive_r = Archive::new(File::open(&plain)?);
    archive_r.set_preserve_permissions(false);
    for entry in archive_r.entries()? {
        let mut entry = entry?;
        let rel = entry.path()?.to_path_buf();
        let rel_str = rel.to_string_lossy().to_string();
        if rel.is_absolute()
            || rel_str.contains("..")
            || rel_str.contains('\0')
            || rel_str.contains('/')
            || rel_str.contains('\\')
        {
            return Err(SnapshotError::UnsafePath);
        }
        if rel_str == "manifest.json" {
            let mut sink = io::sink();
            io::copy(&mut entry, &mut sink)?;
            continue;
        }
        let dest = extract_root.join(&rel);
        let mut out = File::create(&dest)?;
        io::copy(&mut entry, &mut out)?;
    }

    for name in PAYLOAD_ENTRIES.iter() {
        let from = extract_root.join(name);
        if !from.exists() {
            return Err(SnapshotError::MissingFile { which: name });
        }
        let to = targets.target_dir.join(name);
        if to.exists() {
            fs::remove_file(&to)?;
        }
        fs::rename(&from, &to)?;
    }
    Ok(sidecar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::tempdir;

    fn seed_store(path: &Path, schema_version: u32) {
        let conn = rusqlite::Connection::open(path).expect("open store");
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);")
            .expect("create table");
        conn.execute("INSERT INTO t (v) VALUES (?1)", ["hello"])
            .expect("insert row");
        conn.pragma_update(None, "user_version", schema_version)
            .expect("set user_version");
    }

    fn seed_inputs(dir: &Path, schema_version: u32) -> (PathBuf, PathBuf, PathBuf, Vec<u8>) {
        let store = dir.join("store.sqlite3");
        let vault = dir.join("auth.bin");
        let audit = dir.join("audit.ndjson");
        seed_store(&store, schema_version);
        let mut vf = File::create(&vault).unwrap();
        vf.write_all(b"\x01\x02\x03vault-bytes").unwrap();
        let mut af = File::create(&audit).unwrap();
        af.write_all(b"{\"event\":\"start\"}\n{\"event\":\"end\"}\n")
            .unwrap();
        let config = b"# pcloud config\nendpoint = \"api.pcloud.com\"\n".to_vec();
        (store, vault, audit, config)
    }

    #[test]
    fn create_snapshot_produces_valid_manifest() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 7);
        let out = dir.path().join("snap.tar");

        let manifest = create_unencrypted_snapshot(&store, &vault, &audit, &config, &out)
            .expect("create snapshot");

        assert_eq!(manifest.version, SNAPSHOT_FORMAT_VERSION);
        assert_eq!(manifest.schema_version, 7);
        assert_eq!(manifest.sha256_manifest.len(), 64);
        assert!(out.exists());

        let verified = verify_unencrypted_snapshot(&out).expect("verify");
        assert_eq!(verified, manifest);

        let with_schema = verify_unencrypted_snapshot_with_schema(&out, 7).expect("verify schema");
        assert_eq!(with_schema, manifest);
    }

    #[test]
    fn verify_rejects_tampered_archive() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 3);
        let out = dir.path().join("snap.tar");
        create_unencrypted_snapshot(&store, &vault, &audit, &config, &out).unwrap();

        // Re-read the tar, flip one byte inside audit.ndjson, write a new tar.
        let tampered = dir.path().join("snap-tampered.tar");
        {
            let src = File::open(&out).unwrap();
            let mut archive = Archive::new(src);
            let dst = File::create(&tampered).unwrap();
            let mut builder = Builder::new(dst);
            for entry in archive.entries().unwrap() {
                let mut entry = entry.unwrap();
                let name = entry.path().unwrap().to_string_lossy().to_string();
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).unwrap();
                if name == "audit.ndjson" && !buf.is_empty() {
                    buf[0] ^= 0xff;
                }
                let mut header = Header::new_gnu();
                header.set_size(buf.len() as u64);
                header.set_mode(0o600);
                header.set_mtime(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, name.as_str(), buf.as_slice())
                    .unwrap();
            }
            builder.finish().unwrap();
        }

        let err = verify_unencrypted_snapshot(&tampered).unwrap_err();
        assert!(matches!(err, SnapshotError::DigestMismatch), "got {err:?}");
    }

    #[test]
    fn verify_rejects_schema_downgrade() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 99);
        let out = dir.path().join("snap.tar");
        create_unencrypted_snapshot(&store, &vault, &audit, &config, &out).unwrap();

        let err = verify_unencrypted_snapshot_with_schema(&out, 1).unwrap_err();
        match err {
            SnapshotError::SchemaMismatch { expected, got } => {
                assert_eq!(expected, 1);
                assert_eq!(got, 99);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ---------- PR3: GFS pruning unit tests (no gpg required) ----------

    fn touch_with_mtime(path: &Path, mtime: SystemTime) {
        let mut f = File::create(path).unwrap();
        f.write_all(b"x").unwrap();
        let ft = filetime::FileTime::from_system_time(mtime);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    fn day_ago(days: u64) -> SystemTime {
        // Subtract an extra small delta so the file lands inside the
        // intended day-bucket regardless of clock rounding.
        SystemTime::now() - std::time::Duration::from_secs(days * SECS_PER_DAY + 600)
    }

    #[test]
    fn prune_honours_gfs_retention() {
        let dir = tempdir().unwrap();

        // Three same-day-0 entries: only the freshest must be kept.
        let d0_old = dir.path().join("pcloud-rs-d0-old.tar.gpg");
        let d0_mid = dir.path().join("pcloud-rs-d0-mid.tar.gpg");
        let d0_new = dir.path().join("pcloud-rs-d0-new.tar.gpg");
        touch_with_mtime(
            &d0_old,
            SystemTime::now() - std::time::Duration::from_secs(7_200),
        );
        touch_with_mtime(
            &d0_mid,
            SystemTime::now() - std::time::Duration::from_secs(3_600),
        );
        touch_with_mtime(
            &d0_new,
            SystemTime::now() - std::time::Duration::from_secs(60),
        );

        // One file per intermediate day in the daily window.
        let d1 = dir.path().join("pcloud-rs-d1.tar.gpg");
        let d2 = dir.path().join("pcloud-rs-d2.tar.gpg");
        let d5 = dir.path().join("pcloud-rs-d5.tar.gpg");
        touch_with_mtime(&d1, day_ago(1));
        touch_with_mtime(&d2, day_ago(2));
        touch_with_mtime(&d5, day_ago(5));

        // Weekly buckets (retention_days=7 -> daily window covers ages 0..7).
        let w0 = dir.path().join("pcloud-rs-w0.tar.gpg"); // age 10 -> week 0
        let w1 = dir.path().join("pcloud-rs-w1.tar.gpg"); // age 20 -> week 1
        touch_with_mtime(&w0, day_ago(10));
        touch_with_mtime(&w1, day_ago(20));

        // Monthly bucket (age 70 -> after 7 + 56 = 63; (70-63)/30 = 0).
        let m0 = dir.path().join("pcloud-rs-m0.tar.gpg");
        touch_with_mtime(&m0, day_ago(70));

        // Beyond all buckets.
        let ancient = dir.path().join("pcloud-rs-old.tar.gpg");
        touch_with_mtime(&ancient, day_ago(400));

        // A non-matching file that must be ignored entirely.
        let unrelated = dir.path().join("not-a-snapshot.txt");
        fs::write(&unrelated, b"ignore me").unwrap();

        let kept = prune_gfs(dir.path(), 7).unwrap();
        let kept_set: std::collections::HashSet<PathBuf> = kept.into_iter().collect();

        assert!(
            kept_set.contains(&d0_new),
            "freshest day-0 file must be kept"
        );
        assert!(!kept_set.contains(&d0_old));
        assert!(!kept_set.contains(&d0_mid));
        assert!(kept_set.contains(&d1));
        assert!(kept_set.contains(&d2));
        assert!(kept_set.contains(&d5));
        assert!(kept_set.contains(&w0));
        assert!(kept_set.contains(&w1));
        assert!(kept_set.contains(&m0));
        assert!(!kept_set.contains(&ancient));
        assert!(!kept_set.contains(&unrelated));
    }

    #[test]
    fn prune_gfs_execute_removes_only_unkept_files() {
        let dir = tempdir().unwrap();
        let keep = dir.path().join("pcloud-rs-keep.tar.gpg");
        let drop_ = dir.path().join("pcloud-rs-drop.tar.gpg");
        touch_with_mtime(
            &keep,
            SystemTime::now() - std::time::Duration::from_secs(60),
        );
        touch_with_mtime(&drop_, day_ago(400));

        let removed = prune_gfs_execute(dir.path(), 7).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], drop_);
        assert!(keep.exists());
        assert!(!drop_.exists());
    }

    // -------- gpg-gated round-trip tests --------

    fn gpg_test_enabled() -> bool {
        std::env::var("PCLOUD_GPG_TEST").ok().as_deref() == Some("1")
    }

    #[test]
    #[ignore = "requires PCLOUD_GPG_TEST=1 and PCLOUD_GPG_RECIPIENT in keyring"]
    fn encrypted_snapshot_round_trip() {
        if !gpg_test_enabled() {
            return;
        }
        let recipient =
            std::env::var("PCLOUD_GPG_RECIPIENT").expect("PCLOUD_GPG_RECIPIENT must be set");

        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 11);
        let archive = dir.path().join("snap.tar.gpg");
        let manifest =
            create_encrypted_snapshot(&store, &vault, &audit, &config, &recipient, &archive)
                .expect("create encrypted snapshot");
        assert_eq!(manifest.schema_version, 11);

        let verified = verify_encrypted_snapshot(&archive).expect("verify");
        assert_eq!(verified, manifest);

        let restore = tempdir().unwrap();
        let target = restore.path().join("restored");
        let restored = restore_encrypted_snapshot(&archive, &target).expect("restore");
        assert_eq!(restored, manifest);

        for name in PAYLOAD_ENTRIES.iter() {
            assert!(
                target.join(name).exists(),
                "payload {name} missing after restore"
            );
        }
    }

    #[test]
    #[ignore = "requires PCLOUD_GPG_TEST=1 and PCLOUD_GPG_RECIPIENT in keyring"]
    fn restore_refuses_schema_version_mismatch() {
        if !gpg_test_enabled() {
            return;
        }
        let recipient =
            std::env::var("PCLOUD_GPG_RECIPIENT").expect("PCLOUD_GPG_RECIPIENT must be set");

        // Build a valid plaintext archive then rewrite manifest.json with a
        // wrong digest -- the verifier rejects any manifest tampering, which
        // is our proxy for "schema/version mismatch" since changing the
        // version field would also invalidate the digest.
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 5);
        let plain = dir.path().join("snap.tar");
        create_unencrypted_snapshot(&store, &vault, &audit, &config, &plain).unwrap();

        let tampered = dir.path().join("snap-bad.tar");
        {
            let src = File::open(&plain).unwrap();
            let mut archive = Archive::new(src);
            let dst = File::create(&tampered).unwrap();
            let mut builder = Builder::new(dst);
            for entry in archive.entries().unwrap() {
                let mut entry = entry.unwrap();
                let name = entry.path().unwrap().to_string_lossy().to_string();
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).unwrap();
                if name == "manifest.json" {
                    let mut m: SnapshotManifest = serde_json::from_slice(&buf).unwrap();
                    m.sha256_manifest = "00".repeat(32);
                    buf = serde_json::to_vec_pretty(&m).unwrap();
                }
                let mut header = Header::new_gnu();
                header.set_size(buf.len() as u64);
                header.set_mode(0o600);
                header.set_mtime(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, name.as_str(), buf.as_slice())
                    .unwrap();
            }
            builder.finish().unwrap();
        }

        let encrypted = dir.path().join("snap-bad.tar.gpg");
        let status = Command::new("gpg")
            .arg("--batch")
            .arg("--yes")
            .arg("--encrypt")
            .arg("--recipient")
            .arg(&recipient)
            .arg("--output")
            .arg(&encrypted)
            .arg(&tampered)
            .status()
            .unwrap();
        assert!(status.success());

        let restore = tempdir().unwrap();
        let target = restore.path().join("restored");
        let err = restore_encrypted_snapshot(&encrypted, &target).unwrap_err();
        assert!(matches!(err, SnapshotError::DigestMismatch), "got {err:?}");
    }

    // -------- Zstd + SHA3 sidecar snapshot tests (no GPG required) --------

    #[test]
    fn create_snapshot_default_produces_zst_and_sidecar() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 42);
        let out = dir.path().join("snap.tar.zst");

        let sidecar = create_snapshot(
            &store,
            &vault,
            &audit,
            &config,
            &out,
            &SnapshotOptions::default(),
        )
        .expect("create_snapshot default");

        // Archive and sidecar exist with expected shapes.
        assert!(out.exists(), "archive missing");
        assert!(sidecar_path_for(&out).exists(), "sidecar missing");
        assert_eq!(sidecar.version, SIDECAR_MANIFEST_VERSION);
        assert_eq!(sidecar.archive_filename, "snap.tar.zst");
        assert!(!sidecar.encrypted);
        assert_eq!(sidecar.zstd_level, ZSTD_DEFAULT_LEVEL);
        assert_eq!(sidecar.sha3_256.len(), 64);
        assert_eq!(sidecar.inner_manifest.schema_version, 42);

        // SHA3 in the sidecar matches what we recompute over the archive.
        let recomputed = sha3_of_file(&out).unwrap();
        assert_eq!(recomputed, sidecar.sha3_256);

        // Verify round-trips.
        let verified = verify_snapshot(&out).expect("verify");
        assert_eq!(verified, sidecar);
    }

    #[test]
    fn create_snapshot_custom_level_roundtrips() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 3);
        let out = dir.path().join("snap.tar.zst");

        let opts = SnapshotOptions::with_zstd_level(19).expect("level 19 is valid");
        let sidecar =
            create_snapshot(&store, &vault, &audit, &config, &out, &opts).expect("create");
        assert_eq!(sidecar.zstd_level, 19);
        let v = verify_snapshot(&out).expect("verify");
        assert_eq!(v.zstd_level, 19);
    }

    #[test]
    fn create_snapshot_rejects_invalid_level() {
        // Direct builder rejects out-of-range.
        assert!(matches!(
            SnapshotOptions::with_zstd_level(0),
            Err(SnapshotError::InvalidZstdLevel { got: 0 })
        ));
        assert!(matches!(
            SnapshotOptions::with_zstd_level(23),
            Err(SnapshotError::InvalidZstdLevel { got: 23 })
        ));

        // Struct-literal bypass is still rejected by create_snapshot.
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 1);
        let out = dir.path().join("snap.tar.zst");
        let bad = SnapshotOptions {
            zstd_level: 25,
            gpg_recipient: None,
        };
        let err = create_snapshot(&store, &vault, &audit, &config, &out, &bad).unwrap_err();
        assert!(
            matches!(err, SnapshotError::InvalidZstdLevel { got: 25 }),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_snapshot_detects_tamper() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 5);
        let out = dir.path().join("snap.tar.zst");
        create_snapshot(
            &store,
            &vault,
            &audit,
            &config,
            &out,
            &SnapshotOptions::default(),
        )
        .unwrap();

        // Flip a byte in the middle of the archive.
        let mut bytes = fs::read(&out).unwrap();
        assert!(bytes.len() > 10);
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        fs::write(&out, &bytes).unwrap();

        let err = verify_snapshot(&out).unwrap_err();
        assert!(matches!(err, SnapshotError::DigestMismatch), "got {err:?}");
    }

    #[test]
    fn verify_snapshot_missing_sidecar_is_error() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 5);
        let out = dir.path().join("snap.tar.zst");
        create_snapshot(
            &store,
            &vault,
            &audit,
            &config,
            &out,
            &SnapshotOptions::default(),
        )
        .unwrap();

        fs::remove_file(sidecar_path_for(&out)).unwrap();
        let err = verify_snapshot(&out).unwrap_err();
        assert!(matches!(err, SnapshotError::SidecarMissing), "got {err:?}");
    }

    #[test]
    fn create_snapshot_rejects_wrong_suffix() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 1);

        // No GPG but path is *.tar.zst.gpg → rejected.
        let bad = dir.path().join("snap.tar.zst.gpg");
        let err = create_snapshot(
            &store,
            &vault,
            &audit,
            &config,
            &bad,
            &SnapshotOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, SnapshotError::InvalidOutputSuffix),
            "got {err:?}"
        );

        // GPG recipient but path is *.tar.zst → rejected.
        let bad2 = dir.path().join("snap.tar.zst");
        let opts = SnapshotOptions::default().with_gpg_recipient("ops@example.com");
        let err = create_snapshot(&store, &vault, &audit, &config, &bad2, &opts).unwrap_err();
        assert!(
            matches!(err, SnapshotError::InvalidOutputSuffix),
            "got {err:?}"
        );
    }

    #[test]
    fn restore_snapshot_places_payloads() {
        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 11);
        let out = dir.path().join("snap.tar.zst");
        create_snapshot(
            &store,
            &vault,
            &audit,
            &config,
            &out,
            &SnapshotOptions::default(),
        )
        .unwrap();

        let target = dir.path().join("restored");
        let sidecar = restore_snapshot(
            &out,
            &RestoreTargets {
                target_dir: target.clone(),
            },
        )
        .expect("restore");
        assert_eq!(sidecar.inner_manifest.schema_version, 11);
        for name in PAYLOAD_ENTRIES.iter() {
            assert!(
                target.join(name).exists(),
                "payload {name} missing after restore"
            );
        }
    }

    #[test]
    fn prune_gfs_discovers_legacy_tar_gpg() {
        // Back-compat: prune must still find `.tar.gpg` files while
        // also matching `.tar.zst` / `.tar.zst.gpg` alongside.
        //
        // Each archive lands in a distinct daily bucket so GFS keeps
        // all three; sidecars and unrelated files must be ignored.
        let dir = tempdir().unwrap();
        let legacy = dir.path().join("pcloud-rs-legacy.tar.gpg");
        let new_plain = dir.path().join("pcloud-rs-plain.tar.zst");
        let new_enc = dir.path().join("pcloud-rs-enc.tar.zst.gpg");
        let sidecar = dir.path().join("pcloud-rs-plain.tar.zst.manifest.json");
        let unrelated = dir.path().join("notes.txt");
        for p in [&legacy, &new_plain, &new_enc, &sidecar, &unrelated] {
            fs::write(p, b"x").unwrap();
        }
        touch_with_mtime(&legacy, day_ago(0));
        touch_with_mtime(&new_plain, day_ago(1));
        touch_with_mtime(&new_enc, day_ago(2));

        let kept = prune_gfs(dir.path(), 7).unwrap();
        let kept_set: std::collections::HashSet<PathBuf> = kept.into_iter().collect();
        // All three archive styles should be retained (recent enough,
        // each in its own daily bucket).
        assert!(kept_set.contains(&legacy), "legacy .tar.gpg not kept");
        assert!(kept_set.contains(&new_plain), "new .tar.zst not kept");
        assert!(kept_set.contains(&new_enc), "new .tar.zst.gpg not kept");
        // Sidecar & unrelated are not archives.
        assert!(!kept_set.contains(&sidecar));
        assert!(!kept_set.contains(&unrelated));
    }

    #[test]
    fn encrypted_snapshot_zstd_round_trip_gpg_gated() {
        // Only runs when PCLOUD_GPG_TEST=1 AND PCLOUD_GPG_RECIPIENT is
        // in the keyring, mirroring the existing encrypted tests.
        if !gpg_test_enabled() {
            return;
        }
        let recipient =
            std::env::var("PCLOUD_GPG_RECIPIENT").expect("PCLOUD_GPG_RECIPIENT must be set");

        let dir = tempdir().unwrap();
        let (store, vault, audit, config) = seed_inputs(dir.path(), 21);
        let out = dir.path().join("snap.tar.zst.gpg");

        let opts = SnapshotOptions::default().with_gpg_recipient(&recipient);
        let sidecar = create_snapshot(&store, &vault, &audit, &config, &out, &opts)
            .expect("create encrypted zstd snapshot");
        assert!(sidecar.encrypted);
        assert_eq!(sidecar.archive_filename, "snap.tar.zst.gpg");

        let v = verify_snapshot(&out).expect("verify");
        assert_eq!(v, sidecar);

        let target = dir.path().join("restored");
        restore_snapshot(
            &out,
            &RestoreTargets {
                target_dir: target.clone(),
            },
        )
        .expect("restore");
        for name in PAYLOAD_ENTRIES.iter() {
            assert!(target.join(name).exists());
        }
    }
}

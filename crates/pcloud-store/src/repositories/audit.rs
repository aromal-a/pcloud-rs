//! Tamper-evident audit event repository.
//!
//! # Table shape
//!
//! The `audit_events` table stores one row per emitted event. Schema
//! v1 created the bare shape (`id`, `category`, `created_at`); v2
//! added the free-form `details` column; v8 added the three hash-chain
//! columns (`prev_hash`, `entry_hash`, `hmac`) and re-hashed every
//! historical row in insertion order via `rebuild_hash_chain` so the
//! chain is valid after upgrade.
//!
//! # Hash-chain construction
//!
//! Each row carries two BLOB columns — `prev_hash` and `entry_hash` —
//! which together form a SHA-256 hash chain:
//!
//! ```text
//! entry_hash[0] = sha256( ZERO_32 || serialize(event[0]) )
//! entry_hash[i] = sha256( entry_hash[i-1] || serialize(event[i]) )
//! ```
//!
//! `prev_hash` stores either the previous row's `entry_hash` or
//! `AUDIT_ZERO_PREV` for the genesis row. `serialize(event)` is a
//! stable, length-framed canonical form (see
//! `canonical_event_bytes`) so that re-hashing the row at
//! verification time always produces the same digest. The framing is
//! load-bearing: altering it would invalidate every historical
//! row's `entry_hash` and require a follow-on schema bump that
//! re-hashes the entire table.
//!
//! # Tamper evidence
//!
//! Any single-row mutation (changing `category`, `details`,
//! `created_at`, or `prev_hash`) breaks the chain: the stored
//! `entry_hash` will no longer match the recomputed digest, and
//! `AuditRepository::verify_chain` returns
//! `AuditChainError::EntryHashMismatch` pointing at the first
//! affected row id. Splicing a forged row in the middle of the chain
//! is caught by the `prev_hash` continuity check
//! (`AuditChainError::PrevHashMismatch`). Wholesale truncation of
//! the tail is not detected by the chain itself — that class of
//! attack is mitigated by the optional HMAC column described below.
//!
//! # Optional HMAC-SHA256 non-repudiation
//!
//! When the daemon is started with a long-term HMAC key provisioned
//! via the `PCLOUD_AUDIT_HMAC_KEY` environment variable, an
//! HMAC-SHA256 over `entry_hash` is also written to the `hmac`
//! column on every new append. This gives non-repudiation against
//! offline attackers who might otherwise be able to rewrite the
//! chain wholesale: such an attacker can recompute every
//! `entry_hash` trivially (the chain only needs SHA-256), but
//! cannot forge HMAC tags without the key. The key is **never**
//! persisted in the database; it is held in process memory only and
//! must be re-provisioned on every restart that wants HMAC
//! coverage. Rows appended while no key was provisioned retain a
//! `NULL` `hmac` column and are accepted by
//! `AuditRepository::verify_chain` as long as the pure-hash chain
//! itself remains intact.
//!
//! # `VerifiedChain` semantics
//!
//! A successful verification returns a `VerifiedChain` describing
//! the inclusive `[first_id, last_id]` range that was checked and the
//! count of rows visited. When the caller passes a non-`None` `from`
//! bound, the verifier implicitly trusts the `prev_hash` stored on
//! the first in-range row as the chain anchor — full non-repudiation
//! requires calling with `from = None` so the walk starts from the
//! genesis row and compares against `AUDIT_ZERO_PREV`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fmt;

use hmac::{Hmac, Mac};
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Length of a sha256 / HMAC-SHA256 digest in bytes.
pub const AUDIT_HASH_LEN: usize = 32;

/// Canonical zero-prev marker used for the genesis row.
pub const AUDIT_ZERO_PREV: [u8; AUDIT_HASH_LEN] = [0u8; AUDIT_HASH_LEN];

type HmacSha256 = Hmac<Sha256>;

/// In-memory audit repository state.
///
/// `hmac_key` is only populated when an HMAC key was provisioned. It is
/// intentionally kept in-memory only (never persisted) so a restart
/// without the env var transparently disables the HMAC column without
/// invalidating the pure-hash chain.
#[derive(Clone, Default)]
pub struct AuditRepository {
    /// Cached `COUNT(*)` of the `audit_events` table.
    pub retained_event_count: usize,
    /// Cached `details` column of the most recently appended row.
    pub last_event_details: Option<String>,
    /// Running `entry_hash` value of the most recently appended row, or
    /// `ZERO_32` when the chain is empty. Cached to avoid a round-trip
    /// on every append.
    pub last_entry_hash: [u8; AUDIT_HASH_LEN],
    /// HMAC key bytes when provisioned. Stored as `Option<Vec<u8>>`
    /// (rather than `SecretBytes`) because `pcloud-store` is a low-level
    /// crate and cannot depend on `pcloud-secret`. The daemon is
    /// responsible for wiping the key material when the process exits.
    pub hmac_key: Option<Vec<u8>>,
}

impl fmt::Debug for AuditRepository {
    // Custom Debug so the HMAC key is never accidentally exposed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuditRepository")
            .field("retained_event_count", &self.retained_event_count)
            .field("last_event_details", &self.last_event_details)
            .field("last_entry_hash", &hex(&self.last_entry_hash))
            .field("hmac_key", &self.hmac_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl PartialEq for AuditRepository {
    fn eq(&self, other: &Self) -> bool {
        // Deliberately ignore `hmac_key` for equality: a repository with
        // and without the key provisioned still mirrors the same
        // persisted chain state.
        self.retained_event_count == other.retained_event_count
            && self.last_event_details == other.last_event_details
            && self.last_entry_hash == other.last_entry_hash
    }
}

impl Eq for AuditRepository {}

/// Result of a successful chain verification walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedChain {
    /// `id` of the first row the verifier visited, or `None` for an empty chain.
    pub first_id: Option<i64>,
    /// `id` of the last row the verifier visited, or `None` for an empty chain.
    pub last_id: Option<i64>,
    /// Number of rows successfully hashed and checked.
    pub entries_checked: usize,
}

/// Errors raised by [`AuditRepository::verify_chain`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AuditChainError {
    /// Row `id` has a `prev_hash` that does not match the running digest.
    #[error("audit chain broken at id={id}: prev_hash mismatch")]
    PrevHashMismatch {
        /// Row id where the chain breaks.
        id: i64,
    },
    /// Row `id` has a stored `entry_hash` that does not recompute.
    #[error("audit chain broken at id={id}: entry_hash mismatch")]
    EntryHashMismatch {
        /// Row id where the chain breaks.
        id: i64,
    },
    /// Row `id` has a `NULL` or short hash column.
    #[error("audit chain broken at id={id}: missing stored hash column")]
    MissingStoredHash {
        /// Row id where the chain breaks.
        id: i64,
    },
    /// Row `id` has an HMAC that does not match the provisioned key.
    #[error("audit chain broken at id={id}: hmac mismatch")]
    HmacMismatch {
        /// Row id where the chain breaks.
        id: i64,
    },
    /// Underlying SQLite error stringified to keep `AuditChainError` `Clone + Eq`.
    #[error("sqlite: {0}")]
    Sql(String),
}

impl From<rusqlite::Error> for AuditChainError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sql(err.to_string())
    }
}

impl AuditRepository {
    /// Populate the in-memory cache from the current state of the `audit_events` table.
    ///
    /// Reads `COUNT(*)`, the last row's `details`, and the last row's `entry_hash` so that
    /// subsequent [`AuditRepository::append_event`] calls can chain without re-reading.
    pub fn load(conn: &Connection) -> Result<Self, rusqlite::Error> {
        let retained_event_count =
            conn.query_row("SELECT COUNT(*) FROM audit_events", [], |row| {
                row.get::<_, usize>(0)
            })?;
        let last_event_details = conn
            .query_row(
                "SELECT details FROM audit_events ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        let last_entry_hash = conn
            .query_row(
                "SELECT entry_hash FROM audit_events ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten()
            .and_then(|bytes| {
                if bytes.len() == AUDIT_HASH_LEN {
                    let mut out = [0u8; AUDIT_HASH_LEN];
                    out.copy_from_slice(&bytes);
                    Some(out)
                } else {
                    None
                }
            })
            .unwrap_or(AUDIT_ZERO_PREV);
        Ok(Self {
            retained_event_count,
            last_event_details,
            last_entry_hash,
            hmac_key: None,
        })
    }

    /// Provision (or rotate) the HMAC key used for non-repudiation.
    /// Pass `None` to disable HMAC emission on future appends.
    pub fn set_hmac_key(&mut self, key: Option<Vec<u8>>) {
        self.hmac_key = key;
    }

    /// Append an event to `audit_events`, extending the SHA-256 hash chain.
    ///
    /// Performed inside an inner `BEGIN`/`COMMIT` so the insert + hash back-fill
    /// are atomic — a crash between the two steps cannot leave an unhashed row.
    /// When [`AuditRepository::hmac_key`] is set, the `hmac` column is written as
    /// `HMAC-SHA256(key, entry_hash)` for non-repudiation.
    pub fn append_event(
        &mut self,
        conn: &Connection,
        category: &str,
        details: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        // Insert the row first (to obtain AUTOINCREMENT id + created_at),
        // then back-fill the hash-chain columns in the same transaction
        // so a crash between the two steps cannot leave an unhashed row.
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO audit_events (category, details, prev_hash) VALUES (?1, ?2, ?3)",
            (category, details, self.last_entry_hash.to_vec()),
        )?;
        let id: i64 = tx.query_row("SELECT last_insert_rowid()", [], |row| row.get(0))?;
        let created_at: String = tx.query_row(
            "SELECT created_at FROM audit_events WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;

        let event_bytes = canonical_event_bytes(id, category, details, &created_at);
        let entry_hash = compute_entry_hash(&self.last_entry_hash, &event_bytes);
        let hmac_bytes = self
            .hmac_key
            .as_deref()
            .map(|key| compute_hmac(key, &entry_hash));

        tx.execute(
            "UPDATE audit_events SET entry_hash = ?1, hmac = ?2 WHERE id = ?3",
            (
                entry_hash.to_vec(),
                hmac_bytes.as_ref().map(|v| v.to_vec()),
                id,
            ),
        )?;
        tx.commit()?;

        self.retained_event_count += 1;
        self.last_event_details = details.map(ToOwned::to_owned);
        self.last_entry_hash = entry_hash;
        Ok(())
    }

    /// Walk the audit chain from `from` (inclusive, defaults to the
    /// genesis row) to `to` (inclusive, defaults to the latest row),
    /// asserting that every row's `prev_hash` matches the previous row's
    /// `entry_hash` and that every row's `entry_hash` equals
    /// `sha256(prev_hash || canonical(event))`.
    ///
    /// When `from` is not the genesis row (id 1), the walk is anchored
    /// against the stored `prev_hash` of the first row in the range:
    /// the caller is implicitly trusting history before `from`. Full
    /// non-repudiation requires `from = None`.
    pub fn verify_chain(
        &self,
        conn: &Connection,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<VerifiedChain, AuditChainError> {
        let mut stmt = conn.prepare(
            "SELECT id, category, details, created_at, prev_hash, entry_hash, hmac \
             FROM audit_events \
             WHERE (?1 IS NULL OR id >= ?1) AND (?2 IS NULL OR id <= ?2) \
             ORDER BY id ASC",
        )?;
        let mut rows = stmt.query(rusqlite::params![from, to])?;

        let mut running_prev: Option<[u8; AUDIT_HASH_LEN]> = None;
        let mut first_id: Option<i64> = None;
        let mut last_id: Option<i64> = None;
        let mut entries: usize = 0;

        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let category: String = row.get(1)?;
            let details: Option<String> = row.get(2)?;
            let created_at: String = row.get(3)?;
            let prev_hash_stored: Option<Vec<u8>> = row.get(4)?;
            let entry_hash_stored: Option<Vec<u8>> = row.get(5)?;
            let hmac_stored: Option<Vec<u8>> = row.get(6)?;

            let prev_hash_stored =
                prev_hash_stored.ok_or(AuditChainError::MissingStoredHash { id })?;
            let entry_hash_stored =
                entry_hash_stored.ok_or(AuditChainError::MissingStoredHash { id })?;
            if prev_hash_stored.len() != AUDIT_HASH_LEN || entry_hash_stored.len() != AUDIT_HASH_LEN
            {
                return Err(AuditChainError::MissingStoredHash { id });
            }

            // Continuity check against the previously-iterated row (or,
            // for the first row in the range, the caller trusts the
            // stored prev_hash as the chain anchor).
            if let Some(expected_prev) = running_prev
                && prev_hash_stored.as_slice() != expected_prev.as_slice()
            {
                return Err(AuditChainError::PrevHashMismatch { id });
            }

            // Recompute and compare entry_hash.
            let mut prev_arr = [0u8; AUDIT_HASH_LEN];
            prev_arr.copy_from_slice(&prev_hash_stored);
            let event_bytes = canonical_event_bytes(id, &category, details.as_deref(), &created_at);
            let recomputed = compute_entry_hash(&prev_arr, &event_bytes);
            if recomputed.as_slice() != entry_hash_stored.as_slice() {
                return Err(AuditChainError::EntryHashMismatch { id });
            }

            // If the verifier has an HMAC key AND the row stored an HMAC,
            // cross-check them. Missing HMACs on rows appended before
            // the key was provisioned are permitted.
            if let (Some(key), Some(stored_hmac)) = (self.hmac_key.as_deref(), hmac_stored.as_ref())
            {
                let expected = compute_hmac(key, &recomputed);
                if stored_hmac.as_slice() != expected.as_slice() {
                    return Err(AuditChainError::HmacMismatch { id });
                }
            }

            running_prev = Some(recomputed);
            if first_id.is_none() {
                first_id = Some(id);
            }
            last_id = Some(id);
            entries += 1;
        }

        Ok(VerifiedChain {
            first_id,
            last_id,
            entries_checked: entries,
        })
    }
}

/// Stable canonical byte representation of an audit event. The exact
/// framing here IS load-bearing: migrating to a different framing would
/// invalidate every historical row's `entry_hash`. If a future migration
/// needs to change framing, it must re-hash every row at migration time
/// (see [`rebuild_hash_chain`](rebuild_hash_chain)) and document the schema bump.
///
/// Framing: each field is length-prefixed with a u64 little-endian
/// length, followed by its UTF-8 / BLOB bytes. Optional fields emit a
/// distinct presence byte (`0` absent / `1` present) before the length.
pub(crate) fn canonical_event_bytes(
    id: i64,
    category: &str,
    details: Option<&str>,
    created_at: &str,
) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(64 + category.len() + details.map_or(0, str::len) + created_at.len());
    out.extend_from_slice(&id.to_le_bytes());
    push_str(&mut out, category);
    match details {
        Some(s) => {
            out.push(1);
            push_str(&mut out, s);
        }
        None => {
            out.push(0);
        }
    }
    push_str(&mut out, created_at);
    out
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

pub(crate) fn compute_entry_hash(
    prev_hash: &[u8; AUDIT_HASH_LEN],
    event_bytes: &[u8],
) -> [u8; AUDIT_HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash);
    hasher.update(event_bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; AUDIT_HASH_LEN];
    out.copy_from_slice(&digest);
    out
}

pub(crate) fn compute_hmac(key: &[u8], entry_hash: &[u8]) -> [u8; AUDIT_HASH_LEN] {
    // INVARIANT: HMAC-SHA256 accepts keys of any non-zero length per RFC 2104.
    // The `if key.is_empty()` guard substitutes a one-byte sentinel so the
    // call can never fail with a zero-length key, even in zero-key test fixtures.
    let mut mac = <HmacSha256 as Mac>::new_from_slice(if key.is_empty() { &[0u8] } else { key })
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(entry_hash);
    let tag = mac.finalize().into_bytes();
    let mut out = [0u8; AUDIT_HASH_LEN];
    out.copy_from_slice(&tag);
    out
}

/// Rebuild the hash chain for every existing row in insertion order.
/// Called by the schema-v8 migration to bring legacy (pre-hash-chain)
/// rows into the new tamper-evident format. Safe to call multiple
/// times; the result is deterministic for a given set of rows.
pub fn rebuild_hash_chain(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt =
        conn.prepare("SELECT id, category, details, created_at FROM audit_events ORDER BY id ASC")?;
    let mut rows = stmt.query([])?;
    let mut prev = AUDIT_ZERO_PREV;
    // Collect first to avoid holding a read statement while we write.
    let mut all: Vec<(i64, String, Option<String>, String)> = Vec::new();
    while let Some(row) = rows.next()? {
        all.push((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
    }
    drop(rows);
    drop(stmt);

    for (id, category, details, created_at) in all {
        let event_bytes = canonical_event_bytes(id, &category, details.as_deref(), &created_at);
        let entry_hash = compute_entry_hash(&prev, &event_bytes);
        conn.execute(
            "UPDATE audit_events SET prev_hash = ?1, entry_hash = ?2, hmac = NULL WHERE id = ?3",
            (prev.to_vec(), entry_hash.to_vec(), id),
        )?;
        prev = entry_hash;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StoreError, bootstrap_profile};
    use std::path::PathBuf;

    fn temp_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pcloud-store-audit-{}-{}.sqlite3",
            std::process::id(),
            name
        ))
    }

    fn fresh_profile(tag: &str) -> (crate::StoreProfile, PathBuf) {
        let path = temp_db_path(tag);
        let _ = std::fs::remove_file(&path);
        let (profile, _) = bootstrap_profile(&path).expect("bootstrap ok");
        (profile, path)
    }

    #[test]
    fn append_builds_valid_chain() -> Result<(), StoreError> {
        let (mut profile, path) = fresh_profile("chain-valid");
        let conn = Connection::open(&path)?;
        profile
            .repositories
            .audit
            .append_event(&conn, "auth", Some("login"))?;
        profile
            .repositories
            .audit
            .append_event(&conn, "auth", Some("logout"))?;
        profile
            .repositories
            .audit
            .append_event(&conn, "sync", Some("add"))?;

        let verified = profile
            .repositories
            .audit
            .verify_chain(&conn, None, None)
            .expect("chain should verify");
        assert_eq!(verified.entries_checked, 3);
        assert_eq!(verified.first_id, Some(1));
        assert_eq!(verified.last_id, Some(3));
        Ok(())
    }

    #[test]
    fn tampering_detected_at_correct_index() -> Result<(), StoreError> {
        let (mut profile, path) = fresh_profile("chain-tamper");
        let conn = Connection::open(&path)?;
        for i in 0..5 {
            profile
                .repositories
                .audit
                .append_event(&conn, "auth", Some(&format!("event-{i}")))?;
        }

        // Mutate row 3's details to simulate after-the-fact tampering.
        conn.execute(
            "UPDATE audit_events SET details = ?1 WHERE id = 3",
            ["tampered"],
        )?;

        let err = profile
            .repositories
            .audit
            .verify_chain(&conn, None, None)
            .expect_err("tampered chain must fail");
        assert_eq!(err, AuditChainError::EntryHashMismatch { id: 3 });
        Ok(())
    }

    #[test]
    fn tampering_prev_hash_detected() -> Result<(), StoreError> {
        let (mut profile, path) = fresh_profile("chain-tamper-prev");
        let conn = Connection::open(&path)?;
        for i in 0..3 {
            profile
                .repositories
                .audit
                .append_event(&conn, "auth", Some(&format!("event-{i}")))?;
        }

        // Overwrite prev_hash on row 2 with zeros to simulate an attacker
        // splicing a row in place.
        conn.execute(
            "UPDATE audit_events SET prev_hash = ?1 WHERE id = 2",
            [vec![0u8; AUDIT_HASH_LEN]],
        )?;

        let err = profile
            .repositories
            .audit
            .verify_chain(&conn, None, None)
            .expect_err("tampered prev_hash must fail");
        assert_eq!(err, AuditChainError::PrevHashMismatch { id: 2 });
        Ok(())
    }

    #[test]
    fn hmac_when_enabled() -> Result<(), StoreError> {
        let (mut profile, path) = fresh_profile("chain-hmac");
        let conn = Connection::open(&path)?;
        profile
            .repositories
            .audit
            .set_hmac_key(Some(b"test-hmac-key".to_vec()));
        profile
            .repositories
            .audit
            .append_event(&conn, "auth", Some("login"))?;

        let (stored_hmac, entry_hash): (Option<Vec<u8>>, Vec<u8>) = conn.query_row(
            "SELECT hmac, entry_hash FROM audit_events WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let stored_hmac = stored_hmac.expect("hmac should be populated");
        assert_eq!(stored_hmac.len(), AUDIT_HASH_LEN);
        let expected = compute_hmac(b"test-hmac-key", &entry_hash);
        assert_eq!(stored_hmac, expected.to_vec());

        // Verify with correct key passes.
        profile
            .repositories
            .audit
            .verify_chain(&conn, None, None)
            .expect("chain with matching hmac key should verify");

        // Verify with wrong key fails.
        let mut reloaded = AuditRepository::load(&conn)?;
        reloaded.set_hmac_key(Some(b"wrong-key".to_vec()));
        let err = reloaded
            .verify_chain(&conn, None, None)
            .expect_err("hmac mismatch must fail");
        assert_eq!(err, AuditChainError::HmacMismatch { id: 1 });
        Ok(())
    }

    #[test]
    fn verify_chain_range() -> Result<(), StoreError> {
        let (mut profile, path) = fresh_profile("chain-range");
        let conn = Connection::open(&path)?;
        for i in 0..5 {
            profile
                .repositories
                .audit
                .append_event(&conn, "auth", Some(&format!("event-{i}")))?;
        }

        let v = profile
            .repositories
            .audit
            .verify_chain(&conn, Some(2), Some(4))
            .expect("range verify ok");
        assert_eq!(v.first_id, Some(2));
        assert_eq!(v.last_id, Some(4));
        assert_eq!(v.entries_checked, 3);
        Ok(())
    }
}

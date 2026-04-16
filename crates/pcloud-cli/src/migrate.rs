//! `pcloudc migrate-from-c` — one-shot migration helper that imports the
//! legacy C `pcloud-rs` client's on-disk state into the canonical XDG
//! layout consumed by the Rust daemon.
//!
//! **PLATFORM:** Linux primary (where the C `pcloud-rs` client historically
//! ran); macOS secondary.
//! **GATING:** `#[cfg(unix)]` — the C client never ran on Windows, so the
//! module, its tests, and the wired-up CLI command are all Unix-only.
//!
//! The migration is deliberately conservative:
//!
//! * the legacy `~/.pcloud` tree is **copied**, never moved, so a failed
//!   run leaves the C client bit-identical to its prior state;
//! * the legacy SQLite database is captured verbatim as a side-car
//!   (`imported-from-c.sqlite3`) under the Rust data directory, both as a
//!   forensic artifact and to let a human re-run the extractor offline if
//!   the heuristic misses anything;
//! * the Rust store is seeded via the public [`pcloud_store`] API so the
//!   resulting `store.sqlite3` is indistinguishable from one written by a
//!   fresh daemon (schema version, WAL mode, 0600 perms, etc.);
//! * the captured auth token is written to the Rust auth-token vault with
//!   the same `0600` file / `0700` parent-dir discipline the daemon
//!   enforces elsewhere, so migrating never downgrades the on-disk
//!   security posture;
//! * refuses to run on a populated Rust state directory unless the
//!   caller passes `--force-overwrite` — the default path is always
//!   idempotent and non-destructive.
//!
//! **Secret handling:** the auth token read from the legacy DB is held in
//! a [`SecretString`] for the lifetime of the migration, is never written
//! to stdout, stderr, or a log line, and is zeroized on drop. The
//! human-readable preview/report formatters only acknowledge the
//! presence of the token, never its value.

// Linux/macOS only. The C pcloud-rs never ran on Windows, so we gate the
// whole module at compile time rather than decorating every item.
#![cfg(unix)]
// The module doubles as a `#[path]`-included mini-library for the
// `tests/migrate_fixture.rs` integration test. That build sees a subset
// of the API surface (the test exercises `detect_with_targets` +
// `execute` + `render_preview` only); suppress the resulting dead-code
// warnings rather than sprinkling per-item `#[allow]` attributes.
#![allow(dead_code)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use pcloud_config::paths::PcloudDirs;
use pcloud_model::ids::SyncId;
use pcloud_model::sync::SyncType;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use pcloud_store::repositories::sync_graph::SyncRootRecord;
use pcloud_store::{bootstrap_profile, persist_profile};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

/// Errors surfaced by the migration planner / executor.
///
/// All variants are `Display`-safe to print directly. None of them carry a
/// secret — the legacy auth token is wrapped in [`SecretString`] on the
/// happy path and never reaches an error variant.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// The legacy home environment (`$HOME`) could not be resolved.
    #[error("cannot resolve $HOME to locate legacy ~/.pcloud")]
    HomeUnset,
    /// An I/O operation against the filesystem failed.
    #[error("filesystem error at {path}: {source}")]
    Io {
        /// Path that triggered the failure (never a secret).
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },
    /// The legacy SQLite database could not be opened or queried.
    #[error("legacy database error at {path}: {source}")]
    Sqlite {
        /// Path to the legacy database that failed to open/parse.
        path: PathBuf,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
    /// The Rust daemon already has persisted state.
    ///
    /// Emitted when `store.sqlite3` exists under
    /// [`PcloudDirs::data`] and the caller has not passed
    /// `--force-overwrite`. Refusing to clobber is the default and
    /// preferred behavior; the operator can remove the file or pass
    /// `--force-overwrite` if they really mean it.
    #[error(
        "Rust daemon state already present at {path}. Remove it first OR use \
         `--force-overwrite` (destructive)."
    )]
    RustStateAlreadyPresent {
        /// Path to the conflicting Rust `store.sqlite3`.
        path: PathBuf,
    },
    /// Failure inside `pcloud-store` while seeding the Rust DB.
    #[error("failed to seed Rust store: {0}")]
    Store(#[from] pcloud_store::StoreError),
    /// Failure discovering XDG-canonical directories.
    #[error("cannot resolve pcloud directories: {0}")]
    Dirs(#[from] pcloud_config::ConfigError),
}

/// One legacy sync root rehydrated from the C `syncfolder` table.
///
/// The legacy C schema stores `localpath`, a remote `folderid`, and a
/// `synctype` enum. The Rust store needs a `remote_path` string too; the
/// legacy `syncfolder` table does not carry it (only the transient
/// `syncfolderdelayed` table does), so we reconstruct a best-effort
/// placeholder (`/` + fallback marker) and let the operator fix up in
/// `pcloudc sync-list` afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncRootMigration {
    /// Legacy `syncfolder.id`.
    pub legacy_id: u64,
    /// Remote pCloud `folderid` the root was anchored at, if present.
    pub remote_folder_id: Option<u64>,
    /// Absolute local path on disk.
    pub local_path: String,
    /// Best-effort remote path. The C schema does not store this
    /// directly; a migrated row is always resolvable from the id above.
    pub remote_path: String,
    /// `psync_synctype_t` value (1 download-only, 2 upload-only,
    /// 3 full). Defaults to `Full` if missing / out of range.
    pub sync_type: SyncType,
}

/// Planned (or executed) migration from a legacy `~/.pcloud` home into
/// the Rust-side XDG layout. Construct via [`MigrationPlan::detect`];
/// inspect via [`MigrationPlan::render_preview`]; commit via
/// [`MigrationPlan::execute`].
#[derive(Debug)]
pub struct MigrationPlan {
    /// Legacy pCloud home (usually `~/.pcloud`).
    legacy_home: PathBuf,
    /// Target XDG config directory (usually `~/.config/pcloud-rs`).
    target_config: PathBuf,
    /// Target XDG data directory (usually `~/.local/share/pcloud-rs`).
    target_data: PathBuf,
    /// Sync roots extracted from the legacy database.
    sync_roots: Vec<SyncRootMigration>,
    /// `true` when an auth token row was found in the legacy `setting`
    /// table. The token itself is held in [`auth_token`](Self::auth_token)
    /// as a [`SecretString`] so we never accidentally log it.
    has_auth_token: bool,
    /// Legacy auth token, wrapped so it redacts under `Debug` and
    /// zeroizes on drop.
    auth_token: Option<SecretString>,
    /// Preferences lifted from the legacy `setting` table that we will
    /// copy verbatim into the Rust settings-kv scope. Only
    /// non-secret-bearing keys are carried here; `auth` / `pass` / token
    /// keys are filtered out on ingest.
    preferences: Vec<(String, String)>,
    /// Current run mode. `true` means "render a preview only"; `false`
    /// means "execute and mutate the target".
    dry_run: bool,
    /// Allow overwriting an existing Rust `store.sqlite3`.
    force_overwrite: bool,
}

/// Result of a successful [`MigrationPlan::execute`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MigrationReport {
    /// Side-car path the legacy DB was copied to.
    pub side_car_db: PathBuf,
    /// Path of the newly-seeded Rust store.
    pub seeded_store: PathBuf,
    /// Path of the auth-token vault that was populated (if any).
    pub vault_path: Option<PathBuf>,
    /// Number of sync roots seeded into the Rust store.
    pub sync_roots_seeded: usize,
    /// Number of preferences carried across.
    pub preferences_carried: usize,
    /// Whether an auth token was carried into the vault.
    pub auth_token_carried: bool,
}

/// Filename of the C client's SQLite database at the root of `~/.pcloud`.
/// This is the single fingerprint we use to decide "there is something to
/// migrate here".
const LEGACY_DB_FILENAME: &str = ".pclouddb";
/// Keys in the C `setting` table we intentionally do NOT carry over:
/// `pass` is a plaintext password echo the Rust daemon refuses to
/// persist, and `auth` is routed into the vault (not the settings kv).
const SECRET_SETTING_KEYS: &[&str] = &["auth", "pass"];

impl MigrationPlan {
    /// Detect a migratable legacy C state under `~/.pcloud` (or the path
    /// passed via `--from`). Returns `Ok(None)` when no legacy DB is
    /// found: "nothing to migrate" is not an error.
    ///
    /// # Errors
    ///
    /// Returns [`MigrateError::Io`] if the legacy DB exists but cannot be
    /// stat'd, [`MigrateError::Sqlite`] if it cannot be opened or queried,
    /// [`MigrateError::Dirs`] if the target XDG directories cannot be
    /// resolved.
    pub fn detect() -> Result<Option<Self>, MigrateError> {
        Self::detect_from(None, false, false)
    }

    /// Extended entry point: override the legacy home, toggle dry-run /
    /// force-overwrite.
    pub fn detect_from(
        from: Option<PathBuf>,
        dry_run: bool,
        force_overwrite: bool,
    ) -> Result<Option<Self>, MigrateError> {
        Self::detect_with_targets(from, dry_run, force_overwrite, None, None)
    }

    /// Full-control entry point used by tests — allows overriding both
    /// the target config and data directories so the migration can be
    /// exercised against a `TempDir` without touching the user's real
    /// XDG paths. The public wrappers always pass `None` here.
    pub fn detect_with_targets(
        from: Option<PathBuf>,
        dry_run: bool,
        force_overwrite: bool,
        target_config_override: Option<PathBuf>,
        target_data_override: Option<PathBuf>,
    ) -> Result<Option<Self>, MigrateError> {
        let legacy_home = match from {
            Some(p) => p,
            None => {
                let home = std::env::var_os("HOME").ok_or(MigrateError::HomeUnset)?;
                PathBuf::from(home).join(".pcloud")
            }
        };
        let db_path = legacy_home.join(LEGACY_DB_FILENAME);
        if !db_path.exists() {
            return Ok(None);
        }

        let (target_config, target_data) =
            if target_config_override.is_some() || target_data_override.is_some() {
                // Test path: use overrides verbatim when supplied, else fall
                // back to `discover()` for the other half. We never mix a
                // real-home data dir with a temp config dir implicitly.
                let dirs = PcloudDirs::discover()?;
                (
                    target_config_override.unwrap_or(dirs.config),
                    target_data_override.unwrap_or(dirs.data),
                )
            } else {
                let dirs = PcloudDirs::discover()?;
                (dirs.config, dirs.data)
            };

        // Extraction reads the legacy DB read-only so we never mutate
        // the C client's live state — even if it somehow re-opens under
        // us.
        let Extracted {
            sync_roots,
            auth_token,
            preferences,
        } = extract_from_db(&db_path)?;
        let has_auth_token = auth_token.is_some();

        Ok(Some(Self {
            legacy_home,
            target_config,
            target_data,
            sync_roots,
            has_auth_token,
            auth_token,
            preferences,
            dry_run,
            force_overwrite,
        }))
    }

    /// Path to the legacy home inspected by this plan.
    #[must_use]
    pub fn legacy_home(&self) -> &Path {
        &self.legacy_home
    }

    /// Planned side-car path. Computed, not persisted, so the value is
    /// stable across multiple calls.
    #[must_use]
    pub fn planned_side_car(&self) -> PathBuf {
        self.target_data.join("imported-from-c.sqlite3")
    }

    /// Planned seeded-store path.
    #[must_use]
    pub fn planned_store(&self) -> PathBuf {
        self.target_data.join("store.sqlite3")
    }

    /// Planned vault path. Matches
    /// `crates/pcloud-daemon/src/doctor.rs`'s canonical layout.
    #[must_use]
    pub fn planned_vault(&self) -> PathBuf {
        self.target_config.join("auth_token")
    }

    /// Human-readable dry-run output. Never includes the auth token
    /// value — only the fact that one was found.
    #[must_use]
    pub fn render_preview(&self) -> String {
        let mut out = String::new();
        out.push_str("migration plan (dry-run)\n");
        out.push_str("------------------------\n");
        out.push_str(&format!(
            "legacy home        : {}\n",
            self.legacy_home.display()
        ));
        out.push_str(&format!(
            "target config dir  : {}\n",
            self.target_config.display()
        ));
        out.push_str(&format!(
            "target data dir    : {}\n",
            self.target_data.display()
        ));
        out.push_str(&format!(
            "side-car db (copy) : {}\n",
            self.planned_side_car().display()
        ));
        out.push_str(&format!(
            "seeded Rust store  : {}\n",
            self.planned_store().display()
        ));
        out.push_str(&format!(
            "auth token vault   : {}\n",
            self.planned_vault().display()
        ));
        out.push_str(&format!(
            "auth token present : {}\n",
            if self.has_auth_token {
                "yes (value redacted)"
            } else {
                "no"
            }
        ));
        out.push_str(&format!("sync roots found   : {}\n", self.sync_roots.len()));
        for r in &self.sync_roots {
            out.push_str(&format!(
                "  - id={} folderid={} type={:?} local={}\n",
                r.legacy_id,
                r.remote_folder_id
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                r.sync_type,
                r.local_path,
            ));
        }
        out.push_str(&format!(
            "preferences found  : {} (secret keys filtered)\n",
            self.preferences.len()
        ));
        for (k, v) in &self.preferences {
            out.push_str(&format!("  - {k} = {v}\n"));
        }
        out
    }

    /// Commit the migration.
    ///
    /// Copies the legacy DB to the side-car path, seeds a fresh Rust
    /// store via [`pcloud_store`], writes the auth token (if any) to
    /// the vault with 0600 / 0700 permissions, and returns a
    /// [`MigrationReport`] describing what was done.
    ///
    /// # Errors
    ///
    /// - [`MigrateError::RustStateAlreadyPresent`] if a Rust
    ///   `store.sqlite3` already exists and `force_overwrite` is false.
    /// - [`MigrateError::Io`] on any filesystem operation failure.
    /// - [`MigrateError::Store`] if seeding the Rust store fails.
    pub fn execute(self) -> Result<MigrationReport, MigrateError> {
        let store_path = self.planned_store();
        if store_path.exists() && !self.force_overwrite {
            return Err(MigrateError::RustStateAlreadyPresent { path: store_path });
        }

        // 1. Ensure target dirs exist with secure modes.
        ensure_dir(&self.target_data, 0o700)?;
        ensure_dir(&self.target_config, 0o700)?;

        // 2. Copy legacy DB to side-car.
        let side_car = self.planned_side_car();
        let src_db = self.legacy_home.join(LEGACY_DB_FILENAME);
        fs::copy(&src_db, &side_car).map_err(|e| MigrateError::Io {
            path: side_car.clone(),
            source: e,
        })?;
        // Tighten to 0600 — legacy files may be world-readable.
        let _ = fs::set_permissions(&side_car, fs::Permissions::from_mode(0o600));

        // 3. If force_overwrite, wipe the old store.sqlite3 and its
        //    WAL/SHM sidecars so bootstrap_profile starts clean.
        if store_path.exists() && self.force_overwrite {
            for suffix in ["", "-wal", "-shm"] {
                let mut p = store_path.clone().into_os_string();
                p.push(suffix);
                let p = PathBuf::from(p);
                if p.exists() {
                    fs::remove_file(&p).map_err(|e| MigrateError::Io { path: p, source: e })?;
                }
            }
        }

        // 4. Seed a fresh Rust store via the public API.
        let (mut profile, _integrity) = bootstrap_profile(&store_path)?;
        profile.repositories.sync_graph.tracked_sync_roots = self
            .sync_roots
            .iter()
            .enumerate()
            .map(|(idx, s)| SyncRootRecord {
                // We intentionally do NOT reuse legacy_id — the C and
                // Rust id spaces are disjoint. Allocate 1-based.
                sync_id: SyncId::new((idx as u64) + 1),
                local_path: s.local_path.clone(),
                remote_path: s.remote_path.clone(),
                paused: false,
                sync_type: s.sync_type,
            })
            .collect();
        persist_profile(&profile)?;

        // 5. Carry preferences into the settings-kv scope.
        let handle = pcloud_store::StoreHandle::open(&store_path)?;
        let settings = handle.settings_kv();
        for (k, v) in &self.preferences {
            // Best-effort: stringified value. Settings are free-form on
            // the Rust side (same as the C client), so a string write
            // is always valid.
            let _ = settings.set_string(k, v);
        }
        drop(handle);

        // 6. Auth token → vault with 0600/0700.
        let vault_path = if let Some(ref tok) = self.auth_token {
            let vault = self.planned_vault();
            write_vault_token(&vault, tok)?;
            Some(vault)
        } else {
            None
        };

        Ok(MigrationReport {
            side_car_db: side_car,
            seeded_store: store_path,
            vault_path,
            sync_roots_seeded: self.sync_roots.len(),
            preferences_carried: self.preferences.len(),
            auth_token_carried: self.has_auth_token,
        })
    }
}

/// Internal extraction bundle — the three things we pull out of the
/// legacy DB in one read-only pass.
struct Extracted {
    sync_roots: Vec<SyncRootMigration>,
    auth_token: Option<SecretString>,
    preferences: Vec<(String, String)>,
}

fn extract_from_db(db_path: &Path) -> Result<Extracted, MigrateError> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| MigrateError::Sqlite {
        path: db_path.to_path_buf(),
        source: e,
    })?;

    // Sniff the setting table columns. Historically the C client used
    // `setting(id, value)`; defensive against downstream forks that may
    // have renamed columns.
    let settings = extract_settings(&conn, db_path)?;
    let sync_roots = extract_sync_roots(&conn, db_path)?;

    let mut auth_token: Option<SecretString> = None;
    let mut preferences: Vec<(String, String)> = Vec::new();
    for (k, v) in settings {
        if k == "auth" {
            // Hold in a SecretString so the token is zeroized on drop
            // and never rendered by `Debug`. We intentionally do not
            // propagate the raw `v` String further; assigning into a
            // SecretString moves it.
            auth_token = Some(SecretString::new(v));
            continue;
        }
        if SECRET_SETTING_KEYS.contains(&k.as_str()) {
            // `pass` is intentionally dropped on the floor — the Rust
            // daemon refuses to persist plaintext passwords. See
            // CLAUDE.md Security/Auth rules.
            continue;
        }
        preferences.push((k, v));
    }

    Ok(Extracted {
        sync_roots,
        auth_token,
        preferences,
    })
}

fn extract_settings(
    conn: &Connection,
    db_path: &Path,
) -> Result<Vec<(String, String)>, MigrateError> {
    let cols = pragma_columns(conn, "setting");
    if cols.is_empty() {
        return Ok(Vec::new());
    }
    // Resolve the id/value columns resiliently — default to C names.
    let id_col = if cols.iter().any(|c| c == "id") {
        "id"
    } else {
        "key"
    };
    let val_col = if cols.iter().any(|c| c == "value") {
        "value"
    } else {
        "val"
    };
    let sql = format!("SELECT {id_col}, {val_col} FROM setting");
    let mut stmt = conn.prepare(&sql).map_err(|e| MigrateError::Sqlite {
        path: db_path.to_path_buf(),
        source: e,
    })?;
    let rows = stmt
        .query_map([], |row| {
            let k: String = row.get(0)?;
            // Some legacy rows store integer values — coerce to string.
            let v: String = match row.get::<_, String>(1) {
                Ok(s) => s,
                Err(_) => row
                    .get::<_, i64>(1)
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            };
            Ok((k, v))
        })
        .map_err(|e| MigrateError::Sqlite {
            path: db_path.to_path_buf(),
            source: e,
        })?;
    let mut out = Vec::new();
    for r in rows {
        let pair = r.map_err(|e| MigrateError::Sqlite {
            path: db_path.to_path_buf(),
            source: e,
        })?;
        out.push(pair);
    }
    Ok(out)
}

fn extract_sync_roots(
    conn: &Connection,
    db_path: &Path,
) -> Result<Vec<SyncRootMigration>, MigrateError> {
    let cols = pragma_columns(conn, "syncfolder");
    if cols.is_empty() {
        return Ok(Vec::new());
    }
    let has_id = cols.iter().any(|c| c == "id");
    let has_folderid = cols.iter().any(|c| c == "folderid");
    let has_localpath = cols.iter().any(|c| c == "localpath");
    let has_synctype = cols.iter().any(|c| c == "synctype");
    let has_remotepath = cols.iter().any(|c| c == "remotepath");
    if !has_localpath {
        // Not a shape we recognize.
        return Ok(Vec::new());
    }
    let id_expr = if has_id { "id" } else { "rowid" };
    let folderid_expr = if has_folderid { "folderid" } else { "NULL" };
    let synctype_expr = if has_synctype { "synctype" } else { "3" };
    let remotepath_expr = if has_remotepath { "remotepath" } else { "''" };

    let sql = format!(
        "SELECT {id_expr}, {folderid_expr}, localpath, {synctype_expr}, {remotepath_expr} \
         FROM syncfolder"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| MigrateError::Sqlite {
        path: db_path.to_path_buf(),
        source: e,
    })?;
    let rows = stmt
        .query_map([], |row| {
            let legacy_id: i64 = row.get(0)?;
            let folderid: Option<i64> = row.get(1).ok();
            let localpath: String = row.get(2)?;
            let synctype: i64 = row.get(3).unwrap_or(3);
            let remotepath: String = row.get(4).unwrap_or_default();
            Ok((legacy_id, folderid, localpath, synctype, remotepath))
        })
        .map_err(|e| MigrateError::Sqlite {
            path: db_path.to_path_buf(),
            source: e,
        })?;
    let mut out = Vec::new();
    for r in rows {
        let (legacy_id, folderid, localpath, synctype, remotepath) =
            r.map_err(|e| MigrateError::Sqlite {
                path: db_path.to_path_buf(),
                source: e,
            })?;
        let sync_type = u8::try_from(synctype)
            .ok()
            .and_then(SyncType::from_u8)
            .unwrap_or(SyncType::Full);
        let remote_folder_id = folderid.and_then(|n| u64::try_from(n).ok());
        let remote_path = if remotepath.is_empty() {
            // Best-effort: the legacy C `syncfolder` table does not
            // persist the remote path; operators re-bind via
            // `pcloudc sync-list` after migration.
            match remote_folder_id {
                Some(id) => format!("folderid:{id}"),
                None => String::from("/"),
            }
        } else {
            remotepath
        };
        let legacy_id_u64 = u64::try_from(legacy_id).unwrap_or_default();
        out.push(SyncRootMigration {
            legacy_id: legacy_id_u64,
            remote_folder_id,
            local_path: localpath,
            remote_path,
            sync_type,
        });
    }
    Ok(out)
}

fn pragma_columns(conn: &Connection, table: &str) -> Vec<String> {
    // PRAGMA table_info does not support bound parameters in older
    // SQLite; use a safe literal. The table name is a hard-coded
    // constant in this module, so there is no injection surface.
    let sql = format!("PRAGMA table_info({table})");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

fn ensure_dir(path: &Path, mode: u32) -> Result<(), MigrateError> {
    fs::create_dir_all(path).map_err(|e| MigrateError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    // Best-effort tighten: set_permissions may silently fail on some
    // filesystems (e.g. NFS without root squash); the daemon still
    // re-asserts on first write.
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    Ok(())
}

fn write_vault_token(vault_path: &Path, token: &SecretString) -> Result<(), MigrateError> {
    if let Some(parent) = vault_path.parent() {
        ensure_dir(parent, 0o700)?;
    }
    // Write to a temporary then rename so an interrupted run never
    // leaves a half-written vault file behind.
    let tmp_path = vault_path.with_extension("tmp");
    // Clean stale tmp; ignore missing.
    let _ = fs::remove_file(&tmp_path);
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)
            .map_err(|e| MigrateError::Io {
                path: tmp_path.clone(),
                source: e,
            })?;
        f.write_all(token.expose_secret().as_bytes())
            .map_err(|e| MigrateError::Io {
                path: tmp_path.clone(),
                source: e,
            })?;
        f.sync_all().map_err(|e| MigrateError::Io {
            path: tmp_path.clone(),
            source: e,
        })?;
    }
    fs::rename(&tmp_path, vault_path).map_err(|e| MigrateError::Io {
        path: vault_path.to_path_buf(),
        source: e,
    })?;
    // Re-assert 0600 after rename.
    let _ = fs::set_permissions(vault_path, fs::Permissions::from_mode(0o600));
    Ok(())
}

/// Render a [`MigrationReport`] into a short human-readable text block.
#[must_use]
pub fn render_report(report: &MigrationReport) -> String {
    let mut s = String::new();
    s.push_str("migration complete\n");
    s.push_str("------------------\n");
    s.push_str(&format!(
        "side-car db       : {}\n",
        report.side_car_db.display()
    ));
    s.push_str(&format!(
        "seeded store      : {}\n",
        report.seeded_store.display()
    ));
    s.push_str(&format!(
        "vault             : {}\n",
        report
            .vault_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".into())
    ));
    s.push_str(&format!(
        "sync roots seeded : {}\n",
        report.sync_roots_seeded
    ));
    s.push_str(&format!(
        "preferences moved : {}\n",
        report.preferences_carried
    ));
    s.push_str(&format!(
        "auth token moved  : {}\n",
        if report.auth_token_carried {
            "yes"
        } else {
            "no"
        }
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::TempDir;

    fn seed_legacy_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE setting (id TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE syncfolder (
                 id INTEGER PRIMARY KEY,
                 folderid INTEGER,
                 localpath TEXT,
                 synctype INTEGER,
                 flags INTEGER,
                 inode INTEGER,
                 deviceid INTEGER
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO setting (id, value) VALUES ('auth', 'legacy-token-abc')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO setting (id, value) VALUES ('user', 'alice@example.com')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO setting (id, value) VALUES ('pass', 'should-be-dropped')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO setting (id, value) VALUES ('usessl', '1')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO syncfolder (id, folderid, localpath, synctype, flags, inode, deviceid) \
             VALUES (?1, ?2, ?3, ?4, 0, 0, 0)",
            params![1i64, 42i64, "/home/alice/Docs", 3i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO syncfolder (id, folderid, localpath, synctype, flags, inode, deviceid) \
             VALUES (?1, ?2, ?3, ?4, 0, 0, 0)",
            params![2i64, 43i64, "/home/alice/Photos", 1i64],
        )
        .unwrap();
    }

    #[test]
    fn detect_returns_none_when_no_legacy_db() {
        let tmp = TempDir::new().unwrap();
        let plan = MigrationPlan::detect_with_targets(
            Some(tmp.path().to_path_buf()),
            true,
            false,
            Some(tmp.path().join("cfg")),
            Some(tmp.path().join("data")),
        )
        .unwrap();
        assert!(plan.is_none());
    }

    #[test]
    fn detect_extracts_settings_and_sync_roots() {
        let tmp = TempDir::new().unwrap();
        let legacy_home = tmp.path().join("legacy");
        fs::create_dir_all(&legacy_home).unwrap();
        seed_legacy_db(&legacy_home.join(LEGACY_DB_FILENAME));

        let plan = MigrationPlan::detect_with_targets(
            Some(legacy_home),
            true,
            false,
            Some(tmp.path().join("cfg")),
            Some(tmp.path().join("data")),
        )
        .unwrap()
        .expect("plan");

        assert!(plan.has_auth_token);
        assert_eq!(plan.sync_roots.len(), 2);
        // `pass` must be filtered.
        assert!(
            plan.preferences
                .iter()
                .all(|(k, _)| k != "pass" && k != "auth")
        );
        // `user` and `usessl` survive.
        assert!(plan.preferences.iter().any(|(k, _)| k == "user"));
        assert!(plan.preferences.iter().any(|(k, _)| k == "usessl"));

        // Preview must not contain the raw token.
        let preview = plan.render_preview();
        assert!(!preview.contains("legacy-token-abc"));
        assert!(preview.contains("auth token present : yes"));
    }
}

// TODO(bd-sweep-unwrap): This file contains ~33 `.unwrap()` / `.expect()`
// call sites in non-test code paths. The most-reachable ones are in token
// read/write paths; converting them to `?` propagation is the priority.
// Full sweep deferred to a dedicated hardening pass.

//! **PLATFORM: all** (Linux/BSD/macOS/Windows-file-fallback).
//!
//! Secrets persisted to `<config>/auth_token` with mode 0600, parent 0700.
//! On Windows, the permission model is different (NTFS ACLs, not octal
//! POSIX bits); callers should use `DpapiVault` as primary on Windows and
//! reserve this as the explicit `PCLOUD_VAULT=file` opt-in.
//!
//! This file is the canonical home of what used to live in
//! `crate::auth_vault`. The free functions `load_token`, `store_token`,
//! and `clear_token` are kept public so existing call sites
//! (`bootstrap.rs`, `runtime.rs`, `refresh_loop.rs`) can continue to use
//! them verbatim while higher layers migrate to the [`PlatformVault`]
//! trait.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use zeroize::Zeroize;

use crate::auth_vault::AuthVaultError;

use super::{AuthToken, PlatformVault, Result as VaultResult};

#[cfg(unix)]
use pcloud_ipc::current_effective_uid;

/// File-backed implementation of [`PlatformVault`].
///
/// Wraps the path to the on-disk vault file. The underlying filesystem
/// operations are exactly those exposed by the legacy free functions;
/// this struct just gives them a trait-object home.
#[derive(Debug, Clone)]
pub struct FileVault {
    path: PathBuf,
}

impl FileVault {
    /// Construct a `FileVault` pointing at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path to the on-disk vault file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl PlatformVault for FileVault {
    fn load(&self) -> VaultResult<Option<AuthToken>> {
        load_token(&self.path)
    }

    fn store(&self, token: &AuthToken) -> VaultResult<()> {
        store_token(&self.path, token)
    }

    fn clear(&self) -> VaultResult<()> {
        clear_token(&self.path)
    }

    fn backend_name(&self) -> &'static str {
        "file"
    }
}

/// Load the persisted auth token from `path`, returning `Ok(None)` when
/// the vault file does not exist. Validates owner/mode on UNIX and
/// wraps the bytes in a [`SecretString`] so zeroization on drop applies.
pub fn load_token(path: &Path) -> std::result::Result<Option<SecretString>, AuthVaultError> {
    match validate_vault_file(path) {
        Ok(()) => {}
        Err(AuthVaultError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    }

    // Audit finding M4: read vault bytes into an owned buffer that we can
    // explicitly `zeroize()` before drop, instead of going through
    // `fs::read_to_string` which would leak two intermediate `String` copies
    // (raw + trimmed) of the token on the heap until they are reclaimed.
    let mut buf: Vec<u8> = Vec::new();
    match fs::File::open(path) {
        Ok(mut file) => {
            if let Err(err) = file.read_to_end(&mut buf) {
                buf.zeroize();
                return Err(AuthVaultError::Io(err));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(AuthVaultError::Io(err)),
    }

    // Determine the trimmed slice without allocating a second `String`.
    let start = buf
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(buf.len());
    let end = buf
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);

    if start >= end {
        buf.zeroize();
        return Ok(None);
    }

    // Copy only the trimmed token bytes into a `SecretString`, then zeroize
    // the original (untrimmed) buffer so the secret does not linger.
    let trimmed_bytes = buf[start..end].to_vec();
    buf.zeroize();

    match String::from_utf8(trimmed_bytes) {
        Ok(token) => Ok(Some(SecretString::new(token))),
        Err(err) => {
            // Scrub the invalid bytes before returning. `into_bytes()` yields
            // the original `Vec<u8>` so we can zeroize it in place.
            let mut bad = err.into_bytes();
            bad.zeroize();
            Err(AuthVaultError::InsecureMetadata(
                "vault file must contain valid utf-8 token bytes",
            ))
        }
    }
}

/// Persist `token` to `path`, creating the parent directory at mode
/// `0700` and writing the file atomically at mode `0600` on UNIX.
/// Replaces any previous value.
///
/// # Windows
///
/// This function returns [`AuthVaultError::UnsupportedPlatform`] on
/// Windows. The file-vault backend does not apply NTFS ACL restrictions,
/// so the token file would be readable by any process running as the same
/// user without any DACL gate. Use `DpapiVault` as the primary Windows
/// backend; the file vault is intentionally restricted to UNIX hosts.
/// Set `PCLOUD_VAULT=dpapi` or leave `PCLOUD_VAULT` unset on Windows.
pub fn store_token(path: &Path, token: &SecretString) -> std::result::Result<(), AuthVaultError> {
    // Audit-04 §2-opus L-3: refuse the file vault on Windows because we
    // cannot apply owner-only NTFS ACLs portably. DPAPI is the correct
    // backend on Windows.
    #[cfg(windows)]
    {
        let _ = (path, token);
        return Err(AuthVaultError::UnsupportedPlatform(
            "file vault is not supported on Windows; use PCLOUD_VAULT=dpapi".to_owned(),
        ));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }

    let tmp_path = path.with_extension("tmp");
    // Audit finding L3: open the tmp file with O_CREAT|O_EXCL and mode 0o600
    // atomically so a racing symlink or pre-existing attacker-controlled file
    // in the vault dir is rejected by the kernel rather than followed.
    // If a previous aborted write left a stale tmp file behind, clear it
    // first — the parent dir is already 0o700, so only the running user (or
    // root) can have created it.
    match fs::remove_file(&tmp_path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(AuthVaultError::Io(err)),
    }
    {
        #[cfg(unix)]
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)?;
        #[cfg(not(unix))]
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        // Belt-and-braces: re-apply 0o600 in case the umask on this platform
        // relaxed the initial mode through the `mode(...)` hint. `create_new`
        // guarantees we own the newly-created inode.
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(token.expose_secret().as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    sync_parent_directory(path)?;
    Ok(())
}

/// Remove any persisted auth token at `path`. Idempotent — returns
/// `Ok(())` if the file does not exist.
pub fn clear_token(path: &Path) -> std::result::Result<(), AuthVaultError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AuthVaultError::Io(err)),
    }
}

#[cfg(unix)]
fn validate_vault_file(path: &Path) -> std::result::Result<(), AuthVaultError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(AuthVaultError::InsecureMetadata(
            "vault path must be a regular file",
        ));
    }

    let current_uid = current_effective_uid();
    if metadata.uid() != current_uid {
        return Err(AuthVaultError::InsecureMetadata(
            "vault file must be owned by the current user",
        ));
    }

    if metadata.mode() & 0o077 != 0 {
        return Err(AuthVaultError::InsecureMetadata(
            "vault file must not grant group or other access",
        ));
    }

    // Harden the parent directory to 0700 on load, not just on creation.
    // A directory left at a relaxed mode by a previous install, package
    // upgrade, or manual operation would allow other local users to list the
    // vault file name even if the file itself is 0600. Re-applying 0700 here
    // is cheap and idempotent.
    //
    // audit-06 LOW security / pcloud-rs-ncx.80-b: if the parent is owned
    // by us, a chmod failure is a real security problem (we should be
    // able to tighten our own directory) and we escalate to
    // `InsecureMetadata`. If the parent is owned by someone else
    // (e.g. system-managed /etc), we can't tighten it and a warning is
    // the correct outcome.
    if let Some(parent) = path.parent() {
        match fs::set_permissions(parent, fs::Permissions::from_mode(0o700)) {
            Ok(()) => {}
            Err(err) => {
                let parent_meta = fs::symlink_metadata(parent).ok();
                let parent_owned_by_us = parent_meta
                    .as_ref()
                    .map(|m| m.uid() == current_uid)
                    .unwrap_or(false);
                if parent_owned_by_us {
                    log::error!(
                        "vault: failed to tighten parent dir perms on owner-matched {}: {err}",
                        parent.display()
                    );
                    return Err(AuthVaultError::InsecureMetadata(
                        "vault parent directory chmod to 0700 failed on owner-matched path",
                    ));
                }
                log::warn!(
                    "vault: could not tighten parent dir permissions on {}: {err} \
                     (parent not owned by current uid; leaving as-is)",
                    parent.display()
                );
            }
        }
    }

    Ok(())
}

#[cfg(not(unix))]
fn validate_vault_file(path: &Path) -> std::result::Result<(), AuthVaultError> {
    // On non-Unix targets we only validate that the path is a regular
    // file. NTFS ACL inspection belongs in `DpapiVault`; keeping this
    // fallback permissive is safe because non-Unix users must opt in via
    // `PCLOUD_VAULT=file`.
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(AuthVaultError::InsecureMetadata(
            "vault path must be a regular file",
        ));
    }
    Ok(())
}

fn sync_parent_directory(path: &Path) -> std::result::Result<(), AuthVaultError> {
    if let Some(parent) = path.parent() {
        let dir = fs::File::open(parent)?;
        dir.sync_all()?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    use pcloud_secret::ExposeSecret;

    use super::{AuthVaultError, load_token, store_token};

    fn temp_vault_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!(
                "pcloud-auth-vault-{label}-{}-{nonce}",
                std::process::id()
            ))
            .join("auth.token")
    }

    #[test]
    fn load_token_rejects_group_readable_file() {
        let path = temp_vault_path("insecure-mode");
        std::fs::create_dir_all(path.parent().expect("vault parent should exist"))
            .expect("vault parent dir should be created");
        std::fs::write(&path, "auth-token\n").expect("vault file should be written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("vault permissions should be relaxed");

        let err = load_token(&path).expect_err("insecure vault file should be rejected");

        assert!(matches!(
            err,
            AuthVaultError::InsecureMetadata("vault file must not grant group or other access")
        ));
    }

    #[test]
    fn load_token_trims_whitespace_and_wraps_in_secret_string() {
        // Audit finding M4 regression guard: load_token must not go through
        // a plaintext `String` and must tolerate trailing whitespace.
        let path = temp_vault_path("trim");
        std::fs::create_dir_all(path.parent().expect("vault parent should exist"))
            .expect("vault parent dir should be created");
        std::fs::write(&path, "  trimmed-token\n\n").expect("vault file should be written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("vault permissions should be tight");
        let loaded = load_token(&path)
            .expect("vault load should succeed")
            .expect("token should be present");
        assert_eq!(loaded.expose_secret(), "trimmed-token");
    }

    #[test]
    fn load_token_rejects_non_utf8_contents() {
        // Audit finding M4 regression guard: invalid UTF-8 must be reported
        // with a generic metadata error, and the raw bytes must not appear in
        // the error surface.
        let path = temp_vault_path("utf8");
        std::fs::create_dir_all(path.parent().expect("vault parent should exist"))
            .expect("vault parent dir should be created");
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).expect("vault file should be written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("vault permissions should be tight");
        let err = load_token(&path).expect_err("invalid utf-8 vault must be rejected");
        assert!(matches!(err, AuthVaultError::InsecureMetadata(_)));
    }

    #[test]
    fn store_token_refuses_to_follow_symlink_at_tmp_path() {
        // Audit finding L3 regression guard: the tmp file is opened with
        // O_CREAT|O_EXCL, so if an attacker plants a symlink at the tmp path
        // pointing at, say, /etc/passwd, the open must fail instead of being
        // followed. We simulate this by planting a regular stale file that is
        // NOT the tmp file (so the first remove clears it), then plant a
        // symlink and confirm open_new rejects it. We use a dangling symlink
        // so there is nothing to actually clobber.
        let path = temp_vault_path("symlink");
        let parent = path.parent().expect("vault parent should exist");
        std::fs::create_dir_all(parent).expect("vault parent dir should be created");
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .expect("parent mode should be tight");

        // Write an initial valid token first, then plant a symlink at the tmp
        // path that points to a non-writable target outside the dir. A second
        // store_token call must fail at the create_new step (because the
        // symlink already exists) rather than silently following it.
        let token = pcloud_secret::secret_string::SecretString::new("first");
        store_token(&path, &token).expect("initial store should succeed");

        let tmp_path = path.with_extension("tmp");
        // Plant a symlink where the tmp file will be written on the next store.
        let dangling_target = parent.join("does-not-exist");
        std::os::unix::fs::symlink(&dangling_target, &tmp_path)
            .expect("symlink plant should succeed");

        let token2 = pcloud_secret::secret_string::SecretString::new("second");
        let result = store_token(&path, &token2);
        // Expect failure: we removed the symlink first via fs::remove_file in
        // the updated store_token, BUT we only remove regular tmp-file debris.
        // `remove_file` on a symlink unlinks the link itself, which is what we
        // want — then create_new succeeds on a fresh inode. So this test
        // documents that behavior: we do NOT follow the symlink target. After
        // the call, the tmp path no longer exists and the real vault was
        // updated to "second".
        assert!(
            result.is_ok(),
            "store should not follow symlink: {:?}",
            result
        );
        let loaded = load_token(&path)
            .expect("reload should succeed")
            .expect("token should be present");
        assert_eq!(loaded.expose_secret(), "second");
        // The dangling target was never created.
        assert!(
            !dangling_target.exists(),
            "symlink target must not have been followed"
        );
    }

    #[test]
    fn store_token_writes_secure_file_and_loads_it() {
        let path = temp_vault_path("roundtrip");
        let token = pcloud_secret::secret_string::SecretString::new("auth-token");

        store_token(&path, &token).expect("vault token should store");
        let metadata = std::fs::metadata(&path).expect("vault file should exist");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

        let loaded = load_token(&path)
            .expect("vault token should load")
            .expect("vault token should be present");
        assert_eq!(loaded.expose_secret(), "auth-token");
    }

    #[test]
    fn file_vault_trait_roundtrip() {
        // Regression guard for the Phase 0 PlatformVault shim: the trait
        // impl must delegate exactly to load_token/store_token/clear_token.
        use super::super::PlatformVault;
        use super::FileVault;

        let path = temp_vault_path("trait");
        let vault = FileVault::new(path.clone());
        assert_eq!(vault.backend_name(), "file");
        assert!(
            vault
                .load()
                .expect("empty vault should load as None")
                .is_none()
        );

        let token = pcloud_secret::secret_string::SecretString::new("trait-token");
        vault.store(&token).expect("store should succeed");
        let loaded = vault
            .load()
            .expect("load should succeed")
            .expect("token should be present");
        assert_eq!(loaded.expose_secret(), "trait-token");

        vault.clear().expect("clear should succeed");
        assert!(
            vault
                .load()
                .expect("cleared vault should load as None")
                .is_none()
        );
        // clear is idempotent
        vault.clear().expect("double-clear should succeed");
    }
}

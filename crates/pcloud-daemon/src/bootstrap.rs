//! Daemon bootstrap: composes configuration, store, auth vault, protocol
//! clients, the filesystem shell, and backend services into a
//! `RuntimeShell`. Callers: the `pcloudd` binary (`main.rs`), embedder
//! crates (`pcloud-sdk`), and integration tests.
//!
//! Portable; no platform gating at this layer.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::env;
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use pcloud_auth::SessionManager;
use pcloud_cache::CacheShell;
use pcloud_config::{ConfigProfile, Environment, env::apply_env_overrides};
use pcloud_crypto::CryptoShell;
use pcloud_engine::EngineShell;
use pcloud_fs::FilesystemShell;
use pcloud_observability::ObservabilityShell;
use pcloud_p2p::P2pShell;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use pcloud_store::repositories::account::AccountRecord;
use pcloud_store::{bootstrap_profile, persist_profile};
use thiserror::Error;
use zeroize::Zeroize;

use crate::account_backend::AccountRuntime;
use crate::auth_backend::AuthRuntime;
use crate::auth_vault::{AuthVaultError, clear_token};
use crate::backup_backend::BackupRuntime;
use crate::crypto_backend::CryptoRuntime;
use crate::folder_backend::FolderRuntime;
use crate::notifications_backend::NotificationsRuntime;
use crate::public_link_backend::PublicLinkRuntime;
use crate::runtime::RuntimeControlState;
use crate::runtime::RuntimeShell;
use crate::shares_backend::SharesRuntime;
use crate::sync_backend::SyncRuntime;
use crate::transfer_backend::TransferRuntime;
use crate::transport_factory::TransportFactory;
use crate::vault::{PlatformVault, select_vault};

/// Errors surfaced while assembling a [`RuntimeShell`] during daemon
/// startup. Each variant carries the underlying cause verbatim so the
/// operator can distinguish config, filesystem, store, vault, and
/// credential-provisioning failures.
#[derive(Debug, Error)]
pub enum BootstrapError {
    /// Resolving the process current working directory failed (usually
    /// a deleted cwd or a permissions issue on the parent).
    #[error("failed to resolve current working directory: {0}")]
    CurrentDir(#[from] std::io::Error),
    /// Loading or validating the [`pcloud_config::ConfigProfile`] failed.
    #[error("config validation failed: {0}")]
    Config(#[from] pcloud_config::ConfigError),
    /// Creating or locking the runtime/state directories failed.
    #[error("failed to provision runtime directories: {0}")]
    Provision(std::io::Error),
    /// The SQLite-backed store failed to open, migrate, or pass
    /// integrity checks.
    #[error("store bootstrap failed: {0}")]
    Store(#[from] pcloud_store::StoreError),
    /// The auth-token vault could not be read, written, or validated.
    #[error("auth vault operation failed: {0}")]
    AuthVault(#[from] AuthVaultError),
    /// A credential file sourced from `$CREDENTIALS_DIRECTORY` or a
    /// `PCLOUDRS_*_FILE` override failed validation (bad mode, not a
    /// regular file, non-UTF8, etc.).
    #[error("credential bootstrap failed: {0}")]
    CredentialBootstrap(String),
    /// The configured [`pcloud_config::auth::VaultBackend`] is not
    /// available on the current host (e.g. `keychain` on Linux).
    #[error("vault backend selection failed: {0}")]
    VaultSelect(#[from] crate::vault::VaultSelectError),
}

#[derive(Debug, Default)]
pub(crate) struct BootstrapCredentials {
    pub(crate) token: Option<SecretString>,
    pub(crate) username: Option<String>,
    pub(crate) password: Option<SecretString>,
    pub(crate) two_factor_code: Option<SecretString>,
    pub(crate) recovery_code: Option<SecretString>,
    pub(crate) trust_device: bool,
}

fn system_credential_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("CREDENTIALS_DIRECTORY").map(|dir| PathBuf::from(dir).join(name))
}

fn env_credential_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

fn resolve_credential_path(var: &str, systemd_name: &str) -> Option<PathBuf> {
    env_credential_path(var).or_else(|| system_credential_path(systemd_name))
}

fn validate_secret_file(path: &Path) -> Result<(), BootstrapError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|err| BootstrapError::CredentialBootstrap(format!("{}: {err}", path.display())))?;
    if !meta.file_type().is_file() {
        return Err(BootstrapError::CredentialBootstrap(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    let mode = meta.permissions().mode();
    #[cfg(not(unix))]
    let mode: u32 = 0; // Windows permissions are ACL-based; the Unix-mode
                      // triangle mask below degrades to "always 0 → pass"
                      // (no rejection). Native ACL inspection is tracked
                      // under bd-xplat-windows.
    if mode & 0o077 != 0 {
        return Err(BootstrapError::CredentialBootstrap(format!(
            "{} must not grant group/other access (mode=0o{:o})",
            path.display(),
            mode & 0o777
        )));
    }
    Ok(())
}

fn read_secret_file(path: &Path) -> Result<Option<SecretString>, BootstrapError> {
    if !path.exists() {
        return Ok(None);
    }
    validate_secret_file(path)?;
    let mut file = fs::File::open(path)
        .map_err(|err| BootstrapError::CredentialBootstrap(format!("{}: {err}", path.display())))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|err| BootstrapError::CredentialBootstrap(format!("{}: {err}", path.display())))?;
    let text = match String::from_utf8(buf.clone()) {
        Ok(s) => s.trim().to_owned(),
        Err(_) => {
            buf.zeroize();
            return Err(BootstrapError::CredentialBootstrap(format!(
                "{} does not contain valid UTF-8",
                path.display()
            )));
        }
    };
    buf.zeroize();
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(SecretString::new(text)))
}

fn read_text_file(path: &Path) -> Result<Option<String>, BootstrapError> {
    Ok(read_secret_file(path)?.map(|s| s.expose_secret().to_owned()))
}

fn load_bootstrap_credentials() -> Result<BootstrapCredentials, BootstrapError> {
    let token = match resolve_credential_path("PCLOUDRS_TOKEN_FILE", "pcloud-rs-token") {
        Some(path) => read_secret_file(&path)?,
        None => None,
    };
    let username = match resolve_credential_path("PCLOUDRS_USERNAME_FILE", "pcloud-rs-username") {
        Some(path) => read_text_file(&path)?,
        None => None,
    };
    let password = match resolve_credential_path("PCLOUDRS_PASSWORD_FILE", "pcloud-rs-password") {
        Some(path) => read_secret_file(&path)?,
        None => None,
    };
    let two_factor_code =
        match resolve_credential_path("PCLOUDRS_TFA_CODE_FILE", "pcloud-rs-tfa-code") {
            Some(path) => read_secret_file(&path)?,
            None => None,
        };
    let recovery_code =
        match resolve_credential_path("PCLOUDRS_RECOVERY_CODE_FILE", "pcloud-rs-recovery-code") {
            Some(path) => read_secret_file(&path)?,
            None => None,
        };
    let trust_device = matches!(
        std::env::var("PCLOUDRS_TRUST_DEVICE").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    );
    Ok(BootstrapCredentials {
        token,
        username,
        password,
        two_factor_code,
        recovery_code,
        trust_device,
    })
}

fn desired_account_record(auth: &SessionManager) -> Option<AccountRecord> {
    let snapshot = auth.snapshot();
    match (snapshot.authenticated_user, snapshot.email.as_ref()) {
        (Some(user_id), Some(email)) => Some(AccountRecord {
            user_id,
            email: email.clone(),
            auth_token_present: snapshot.auth_token.is_some(),
        }),
        _ => None,
    }
}

fn sync_bootstrap_auth_state(
    config: &ConfigProfile,
    store: &mut pcloud_store::StoreProfile,
    auth: &SessionManager,
    vault: &dyn PlatformVault,
) -> Result<(), BootstrapError> {
    store.repositories.accounts.primary_account = desired_account_record(auth);
    if config.features.durable_auth_tokens_enabled {
        match auth.snapshot().auth_token.as_ref() {
            Some(token) => vault.store(token)?,
            None => vault.clear()?,
        }
    }
    persist_profile(store)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn apply_bootstrap_credentials(
    config: &ConfigProfile,
    store: &mut pcloud_store::StoreProfile,
    auth_runtime: &AuthRuntime,
    auth: &mut SessionManager,
    creds: BootstrapCredentials,
) -> Result<bool, BootstrapError> {
    // Back-compat shim: external callers (lib.rs tests) invoke this
    // without a vault handle. Construct a file-backed vault pointing at
    // the configured vault path so existing behavior (write-through via
    // the file vault) is preserved when the caller did not route
    // through `bootstrap_with_config`. Bootstrap-internal callers use
    // `apply_bootstrap_credentials_with_vault` below and pass their
    // already-selected vault.
    let vault: Box<dyn PlatformVault> = Box::new(crate::vault::FileVault::new(
        config.paths.auth_token_vault_path(),
    ));
    apply_bootstrap_credentials_with_vault(config, store, auth_runtime, auth, creds, vault.as_ref())
}

pub(crate) fn apply_bootstrap_credentials_with_vault(
    config: &ConfigProfile,
    store: &mut pcloud_store::StoreProfile,
    auth_runtime: &AuthRuntime,
    auth: &mut SessionManager,
    creds: BootstrapCredentials,
    vault: &dyn PlatformVault,
) -> Result<bool, BootstrapError> {
    let attempted = creds.token.is_some()
        || creds.username.is_some()
        || creds.password.is_some()
        || creds.two_factor_code.is_some()
        || creds.recovery_code.is_some();
    if !attempted {
        return Ok(false);
    }

    if let Some(token) = creds.token {
        auth_runtime
            .login_with_token(auth, token)
            .map_err(|err| BootstrapError::CredentialBootstrap(err.to_string()))?;
        sync_bootstrap_auth_state(config, store, auth, vault)?;
        return Ok(true);
    }

    let username = creds.username.ok_or_else(|| {
        BootstrapError::CredentialBootstrap(
            "username/password bootstrap requires PCLOUDRS_USERNAME_FILE".to_owned(),
        )
    })?;
    let password = creds.password.ok_or_else(|| {
        BootstrapError::CredentialBootstrap(
            "username/password bootstrap requires PCLOUDRS_PASSWORD_FILE".to_owned(),
        )
    })?;

    auth_runtime
        .login_with_password(auth, username, password)
        .map_err(|err| BootstrapError::CredentialBootstrap(err.to_string()))?;

    if auth.snapshot().pending_challenge.is_some() {
        let (code, recovery) = match (creds.recovery_code, creds.two_factor_code) {
            (Some(code), None) => (code, true),
            (None, Some(code)) => (code, false),
            (Some(_), Some(_)) => {
                return Err(BootstrapError::CredentialBootstrap(
                    "set either PCLOUDRS_TFA_CODE_FILE or PCLOUDRS_RECOVERY_CODE_FILE, not both"
                        .to_owned(),
                ))
            }
            (None, None) => {
                return Err(BootstrapError::CredentialBootstrap(
                    "two-factor authentication is required; provide PCLOUDRS_TFA_CODE_FILE or PCLOUDRS_RECOVERY_CODE_FILE"
                        .to_owned(),
                ))
            }
        };
        auth_runtime
            .submit_two_factor_code(auth, code, creds.trust_device, recovery)
            .map_err(|err| BootstrapError::CredentialBootstrap(err.to_string()))?;
    }

    sync_bootstrap_auth_state(config, store, auth, vault)?;
    Ok(true)
}

fn bootstrap_auth_from_env_credentials(
    config: &ConfigProfile,
    store: &mut pcloud_store::StoreProfile,
    auth_runtime: &AuthRuntime,
    auth: &mut SessionManager,
    vault: &dyn PlatformVault,
) -> Result<bool, BootstrapError> {
    apply_bootstrap_credentials_with_vault(
        config,
        store,
        auth_runtime,
        auth,
        load_bootstrap_credentials()?,
        vault,
    )
}

/// Bootstrap a [`RuntimeShell`] using environment-driven defaults.
///
/// Honors `PCLOUD_ROOT` for a single-tree override; otherwise uses the
/// platform's XDG-canonical directories. Applies env overrides,
/// validates the resulting config, then delegates to
/// [`bootstrap_with_config`].
///
/// # Startup sequence
///
/// The daemon boot sequence (see R19 manpage findings in
/// `docs/book/src/reference/daemon-startup.md`) runs, in order:
///
/// 1. Resolve the data root (`PCLOUD_ROOT` or XDG defaults).
/// 2. Build a [`ConfigProfile::secure_defaults`] baseline.
/// 3. Apply `PCLOUD_*` env overrides via `apply_env_overrides`.
/// 4. Validate config (rejects production plaintext transport).
/// 5. Provision runtime directories with owner-only permissions.
/// 6. Open the SQLite store and run schema migrations.
/// 7. Run an integrity check and persist the result.
/// 8. Wire the auth vault (opt-in durable token persistence).
/// 9. Construct the session manager and per-subsystem runtimes.
/// 10. Install signal handlers (`SIGTERM`/`SIGINT`/`SIGHUP`/`SIGPIPE`).
/// 11. Load systemd-credential bootstrap credentials, if present.
/// 12. Return the composed [`RuntimeShell`] ready to serve IPC.
///
/// # Errors
///
/// Returns a [`BootstrapError`] if path discovery, config validation,
/// store opening, vault wiring, or credential bootstrap fails. The
/// daemon must abort on any such error — partial state is never
/// exposed.
///
/// # Examples
///
/// ```no_run
/// # use pcloud_daemon::bootstrap_shell;
/// let shell = bootstrap_shell().expect("daemon bootstrap");
/// println!("{}", shell.summary());
/// ```
pub fn bootstrap_shell() -> Result<RuntimeShell, BootstrapError> {
    // **PLATFORM:** all. Default layout uses XDG-canonical directories
    // via `PcloudDirs::discover()` (Linux/BSD XDG, macOS `~/Library/*`,
    // Windows `%APPDATA%`/`%LOCALAPPDATA%`). `PCLOUD_ROOT=…` overrides
    // with a single rooted tree for multi-instance, testing, or
    // non-standard home layouts. pCloud accounts are user-scoped, not
    // session-scoped, so no random session ID is introduced.
    let mut config = match env::var_os("PCLOUD_ROOT") {
        Some(r) => {
            ConfigProfile::secure_defaults(std::path::PathBuf::from(r), Environment::Production)
        }
        None => {
            let dirs = pcloud_config::paths::PcloudDirs::discover()?;
            let mut p = ConfigProfile::secure_defaults(
                std::path::PathBuf::from("/"),
                Environment::Production,
            );
            p.paths = dirs.to_managed_paths();
            p
        }
    };
    config = apply_env_overrides(config)?;
    bootstrap_with_config(config)
}

/// Bootstrap a [`RuntimeShell`] from an explicit, already-validated
/// [`ConfigProfile`].
///
/// Provisions runtime directories, opens the store, wires the auth
/// vault (honoring the `durable_auth_tokens_enabled` feature gate),
/// constructs every per-subsystem runtime, and loads any
/// systemd-credential-provided bootstrap credentials before returning
/// the composed shell.
///
/// This is the explicit-config counterpart to [`bootstrap_shell`]; use
/// it when the caller already owns a validated [`ConfigProfile`] (e.g.
/// embedded tests, SDK consumers). The runtime-directory permissions,
/// vault ownership checks, and store integrity validation are applied
/// unchanged.
///
/// # Errors
///
/// Returns a [`BootstrapError`] on config-validation, directory
/// provisioning, store, vault, or credential bootstrap failure.
pub fn bootstrap_with_config(config: ConfigProfile) -> Result<RuntimeShell, BootstrapError> {
    // TODO(bd-1du.sec-sandbox): Apply Linux landlock + seccomp-BPF syscall
    // filtering immediately after directory provisioning so the daemon is
    // confined to pCloud-relevant syscalls for its lifetime. See audit-04
    // §2-opus M-3. The syscall allowlist should cover:
    //   read, write, open/openat, close, fstat, getdents64,
    //   socket (AF_UNIX, AF_INET, AF_INET6), connect, accept4,
    //   sendmsg, recvmsg, futex, epoll_*, timerfd_*, signalfd4,
    //   getrandom, mmap/munmap (PROT_READ|PROT_WRITE, no PROT_EXEC).
    // Landlock: restrict FS access to config_dir, state_dir, runtime_dir,
    // cache_dir, and explicitly-mounted FUSE mountpoints.
    // macOS/Windows equivalents (Sandbox.kext, AppContainer) can be added
    // once the Linux path is stable. Do not implement here without a
    // cargo-feature gate (`sandbox`) so the daemon remains functional on
    // kernels that pre-date landlock (< 5.13).
    config.validate()?;

    // ncx.59 (P3-E6): install the runtime-configurable IPC connection
    // caps from the validated profile before any accept loop starts.
    // `pcloud-ipc` defaults to the legacy 128/32 compile-time constants
    // when this is not called (e.g. tests that build a bare
    // `BoundIpcServer`), so this is purely additive.
    pcloud_ipc::set_ipc_connection_caps(
        config.limits.max_ipc_connections,
        config.limits.max_ipc_connections_per_peer,
    );

    for (path, mode) in [
        (&config.paths.config_dir, config.runtime.config_dir_mode),
        (&config.paths.state_dir, config.runtime.state_dir_mode),
        (&config.paths.runtime_dir, config.runtime.socket_dir_mode),
        (&config.paths.cache_dir, config.runtime.cache_dir_mode),
    ] {
        fs::create_dir_all(path).map_err(BootstrapError::Provision)?;
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(BootstrapError::Provision)?;
        #[cfg(not(unix))]
        let _ = mode; // ACL-based perms on Windows; bd-xplat-windows.
    }

    if config.extensions.plugins_enabled {
        fs::create_dir_all(&config.extensions.plugin_dir).map_err(BootstrapError::Provision)?;
        #[cfg(unix)]
        fs::set_permissions(
            &config.extensions.plugin_dir,
            fs::Permissions::from_mode(config.runtime.config_dir_mode),
        )
        .map_err(BootstrapError::Provision)?;
    }

    let store_path = config.paths.state_dir.join("store.sqlite3");
    let (mut store, integrity) = bootstrap_profile(&store_path)?;

    // Provision the audit-chain HMAC key from the environment (feature
    // flag `audit_hmac`). When the env var is absent, the chain remains
    // hash-only; this is by design so existing callers keep working
    // without any config churn.
    if let Some(raw) = std::env::var_os("PCLOUD_AUDIT_HMAC_KEY") {
        let key = raw.to_string_lossy().into_owned().into_bytes();
        if !key.is_empty() {
            store.repositories.audit.set_hmac_key(Some(key));
        }
    }

    let mut config = config;
    if let Some(enabled) = store.repositories.preferences.durable_auth_tokens_enabled {
        config.features.durable_auth_tokens_enabled = enabled;
    }
    if let Some(api_server) = store.repositories.preferences.api_server_binapi.as_deref() {
        match config.api.apply_api_server_hint(api_server) {
            Ok(()) => log::info!(
                "pcloudd bootstrap: applied api_server hint from preferences: {}",
                api_server
            ),
            Err(reason) => log::error!(
                "pcloudd bootstrap: rejecting api_server hint from preferences: {} — {}",
                api_server,
                reason
            ),
        }
    }
    let mut auth = SessionManager::new();
    let auth_runtime = AuthRuntime::from_config(&config);
    let account_runtime = AccountRuntime::from_config(&config);
    let backup_runtime = BackupRuntime::from_config(&config);
    let crypto_runtime = CryptoRuntime::from_config(&config);
    let folder_runtime = FolderRuntime::from_config(&config);
    let notifications_runtime = NotificationsRuntime::from_config(&config);
    let public_link_runtime = PublicLinkRuntime::from_config(&config);
    let shares_runtime = SharesRuntime::from_config(&config);
    let sync_runtime = SyncRuntime::from_config(&config);
    let transfer_runtime = TransferRuntime::from_config(&config);
    // Resilience: in production, transports produced by the factory are
    // wrapped in `ResilientTransport` with real `SystemClock` +
    // `ThreadSleepWaiter`. In development/test the factory returns the
    // bare transport so existing tests stay deterministic. This does not
    // touch per-backend transport wiring (FUSE/crypto/shares/backup/sync/
    // public-link/notifications remain unchanged).
    let transport_factory = TransportFactory::new(config.environment, config.resilience.clone());
    let token_vault_path = config.paths.auth_token_vault_path();

    // Select the auth-token vault backend based on `config.auth.backend`.
    // `Auto` picks the platform-native backend (macOS → Keychain,
    // Windows → DPAPI, Linux → Secret Service, BSD → File). Explicit
    // values are honoured verbatim; a platform mismatch (e.g. `keychain`
    // on Linux) produces a hard `BootstrapError::VaultSelect` rather
    // than silently degrading.
    let vault_selection = select_vault(config.auth.backend, &token_vault_path)?;
    if let Some(warn) = &vault_selection.warning {
        log::warn!("pcloudd bootstrap: {warn}");
    }
    log::info!(
        "pcloudd bootstrap: auth-token vault backend = {} (requested: {})",
        vault_selection.effective.as_str(),
        config.auth.backend.as_str()
    );
    let vault: Box<dyn PlatformVault> = vault_selection.vault;

    // The on-disk file vault's startup load is only meaningful when the
    // effective backend is `file`. For keychain/dpapi/secret-service,
    // the same token is loaded through the trait object below.
    let used_external_credentials = bootstrap_auth_from_env_credentials(
        &config,
        &mut store,
        &auth_runtime,
        &mut auth,
        vault.as_ref(),
    )?;

    if !used_external_credentials && config.features.durable_auth_tokens_enabled {
        match vault.load()? {
            Some(token) => {
                if auth_runtime.login_with_token(&mut auth, token).is_err() {
                    vault.clear()?;
                    // On the legacy file path also honour the historical
                    // cleanup so a stale token file is removed even when
                    // the active backend is not `file`.
                    let _ = clear_token(&token_vault_path);
                    store.repositories.accounts.primary_account = None;
                    persist_profile(&store)?;
                } else {
                    sync_bootstrap_auth_state(&config, &mut store, &auth, vault.as_ref())?;
                }
            }
            None => {
                // No durable token in the selected backend. Fall through;
                // later sync below still persists account metadata if the
                // in-memory session was populated by the env-credential
                // path above.
            }
        }
    } else if !used_external_credentials && auth.snapshot().auth_token.is_some() {
        sync_bootstrap_auth_state(&config, &mut store, &auth, vault.as_ref())?;
    }

    // Startup-resume scan for per-inode chunked-upload sidecars under the
    // FUSE staging root. This pass runs *before* any authenticated
    // transport is available, so it enumerates and logs only. The live
    // server reconcile (trim up / trim down / NotFound / Stalled) is
    // driven by [`mount_runtime::pcloud_shim_adapter_factory`] at mount
    // time, which does have a `ProtoUploadBackend` wired. See
    // `pcloud_fs::write_path::replay_upload_sidecars`.
    {
        use pcloud_fs::write_path::{ResumeOutcome, enumerate_upload_sidecars};
        let staging_root = config.paths.cache_dir.join("fuse-staging");
        match enumerate_upload_sidecars(&staging_root) {
            Ok(outcomes) if !outcomes.is_empty() => {
                log::info!(
                    "pcloud-daemon bootstrap: {} upload sidecar(s) awaiting server reconcile under {}",
                    outcomes.len(),
                    staging_root.display()
                );
                for o in outcomes {
                    match o {
                        ResumeOutcome::Resumed {
                            sidecar,
                            upload_id,
                            acked_offset,
                        } => log::info!(
                            "upload_resume: sidecar={} upload_id={} acked_offset={}",
                            sidecar.display(),
                            upload_id,
                            acked_offset
                        ),
                        ResumeOutcome::Unparseable { sidecar, reason } => log::warn!(
                            "upload_resume: unparseable sidecar={} reason={}",
                            sidecar.display(),
                            reason
                        ),
                        other => log::info!("upload_resume: {other:?}"),
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                log::error!(
                    "pcloud-daemon bootstrap: upload sidecar enumeration failed under {}: {e}",
                    staging_root.display()
                );
            }
        }
    }

    // Resume-after-restart scan (UPLOAD-WIRING-GAP row 94 step 5).
    //
    // Surfaces any previously-interrupted chunked uploads to the log so
    // an operator / upper-layer consumer can decide whether to resume
    // them. Actual re-issuing of the upload is the responsibility of
    // the caller that owns the `UploadStateMachine` (SDK
    // `EmbeddedDaemon::start_upload` or a future sync-engine queue),
    // because only the caller has the live payload bytes + auth token.
    {
        use pcloud_store::repositories::upload_resume::UploadResumeRepository;
        let store_conn = rusqlite::Connection::open(&store_path)
            .map_err(|err| BootstrapError::Provision(std::io::Error::other(err.to_string())))?;
        match UploadResumeRepository::list_all(&store_conn) {
            Ok(rows) if !rows.is_empty() => {
                log::info!(
                    "pcloud-daemon bootstrap: {} resumable upload(s) pending",
                    rows.len()
                );
                for row in rows {
                    log::info!(
                        "upload_resume: path={} uploadid={} offset={}/{}",
                        row.local_path,
                        row.upload_id,
                        row.offset,
                        row.total_size,
                    );
                }
            }
            Ok(_) => {}
            Err(err) => {
                log::error!("pcloud-daemon bootstrap: upload_resume scan failed: {err}");
            }
        }
    }

    // H14 PR4 — clone the sweeper config before `config` is moved into
    // the RuntimeShell so the sweeper shell can be initialised below.
    let integrity_sweeper_cfg = config.features.integrity_sweeper.clone();

    // Clone the audit-verifier config before `config` is moved so the
    // verifier shell can be initialised from validated settings.
    let audit_verifier_cfg = config.features.audit_verifier.clone();

    // IPC rate-limit policy — clone before `config` is moved so the
    // `SessionRateLimiter` can be constructed below. See
    // `pcloud_daemon::rate_limit` for the dispatcher integration.
    let rate_limit_policy = config.rate_limit.clone();

    // Session-refresh policy — derive from the `[auth]` config block
    // before `config` is moved into the RuntimeShell.
    let session_refresh_policy = crate::session_refresh::policy_from_config(&config.auth);

    // Tier-2 HA (`docs/enterprise/ha.md` §4.2). Opt-in: when
    // `[ha].enabled = true` the daemon tries a non-blocking
    // `flock(LOCK_EX | LOCK_NB)` on `<state_dir>/daemon.lease`. On
    // success we become primary; on contention the posture depends on
    // `[ha].mode` (`refuse` aborts bootstrap with a diagnostic naming
    // the primary; `passive` binds the IPC socket and rejects every
    // request with `Unavailable` until the lease is released).
    let ha = if config.ha.enabled {
        let lease_path = config
            .paths
            .state_dir
            .join(crate::ha_lease::LEASE_FILE_NAME);
        let instance_id = config.paths.state_dir.display().to_string();
        match crate::ha_lease::LeaseHolder::try_acquire(&lease_path, instance_id) {
            Ok(holder) => {
                log::info!(
                    "pcloud-daemon: Tier-2 HA primary — lease acquired at {}",
                    lease_path.display()
                );
                crate::ha_lease::HaRuntime::Primary { holder }
            }
            Err(crate::ha_lease::LeaseError::HeldBy { owner }) => match config.ha.mode {
                pcloud_config::ha::HaContendedMode::Refuse => {
                    let who = owner
                        .as_ref()
                        .map(|o| {
                            format!("{}/pid={} (instance={})", o.hostname, o.pid, o.instance_id)
                        })
                        .unwrap_or_else(|| "unknown".to_owned());
                    return Err(BootstrapError::Provision(std::io::Error::other(format!(
                        "Tier-2 HA lease already held by {who}; refusing to start (mode=refuse). \
                         Set `[ha].mode = \"passive\"` to start in passive mode and take over on release."
                    ))));
                }
                pcloud_config::ha::HaContendedMode::Passive => {
                    log::info!(
                        "pcloud-daemon: Tier-2 HA passive — lease held by {}; polling {}",
                        owner
                            .as_ref()
                            .map(|o| format!("{}/pid={}", o.hostname, o.pid))
                            .unwrap_or_else(|| "unknown".to_owned()),
                        lease_path.display()
                    );
                    crate::ha_lease::HaRuntime::Passive { lease_path }
                }
            },
            Err(e) => {
                return Err(BootstrapError::Provision(std::io::Error::other(format!(
                    "Tier-2 HA lease acquire failed: {e}"
                ))));
            }
        }
    } else {
        crate::ha_lease::HaRuntime::Disabled
    };

    let observability = ObservabilityShell::default();
    // SLO wiring (I15 hot-path call site #5, registration side).
    //
    // Install the daemon's `Arc<Slo>` into the process-wide
    // `pcloud_fs::slo_hook` registry so the Linux FUSE `read` shim can
    // feed `mount.read.latency.p99` into the same counters rendered by
    // `Method::GetSlo` / `/slo`. Subsequent bootstrap calls (tests that
    // re-bootstrap a runtime) silently no-op because `OnceLock` only
    // accepts the first registration.
    let _first_registration = pcloud_fs::slo_hook::set_slo_registry(observability.slo.clone());

    // Hoist the mount-pid state-dir handle *before* the struct literal
    // consumes `config`. Cloned cheaply (PathBuf): the mount-pid sidecar
    // lives at `<state_dir>/mount_pid`.
    let mount_state_dir: std::path::PathBuf = config.paths.state_dir.clone();

    Ok(RuntimeShell {
        config,
        store,
        integrity,
        auth,
        auth_runtime,
        account_runtime,
        backup_runtime,
        crypto_runtime,
        folder_runtime,
        notifications_runtime,
        public_link_runtime,
        shares_runtime,
        sync_runtime,
        transfer_runtime,
        engine: EngineShell::new(),
        cache: CacheShell::default(),
        filesystem: FilesystemShell::default(),
        crypto: CryptoShell::default(),
        observability,
        p2p: P2pShell::default(),
        control: RuntimeControlState::default(),
        ipc_owner_uid: None,
        pending_password_auth: None,
        mount_control: {
            // P1.4: orphan-mount detection on startup. The default
            // `MountControl` reads `PCLOUD_FORCE_UMOUNT` from the env;
            // if unset and orphans exist the scan logs a helpful error
            // pointing at `pcloudc mount --force-umount <path>`.
            let mut ctl = crate::mount_runtime::MountControl::default();
            ctl.set_state_dir(mount_state_dir);
            // bd-1du.4: stale `<state_dir>/mount_pid` cleanup. If the
            // previous daemon crashed while a mount was active, the
            // sidecar points at the mountpoint we need to surface as
            // an orphan candidate. `sweep_stale_pidfile` removes the
            // file when the recorded pid is dead and returns the
            // mountpoint; a live pid means a sibling daemon still owns
            // the mount and we should not touch it.
            match ctl.sweep_stale_pidfile() {
                Ok(crate::mount_runtime::StalePidfileOutcome::Absent) => {}
                Ok(crate::mount_runtime::StalePidfileOutcome::Live { pid, mountpoint }) => {
                    log::info!(
                        "pcloud-rs mount: another daemon (pid={pid}) appears to own mount at {}; \
                         skipping orphan scan for this path",
                        mountpoint.display()
                    );
                }
                Ok(crate::mount_runtime::StalePidfileOutcome::Cleaned { mountpoint }) => {
                    log::warn!(
                        "pcloud-rs mount: removed stale mount_pid for crashed daemon at {} (will orphan-scan)",
                        mountpoint.display()
                    );
                }
                Ok(crate::mount_runtime::StalePidfileOutcome::Corrupt) => {
                    log::warn!("pcloud-rs mount: removed corrupt mount_pid sidecar");
                }
                Err(e) => {
                    log::error!("pcloud-rs mount: mount_pid sweep failed: {e}");
                }
            }
            match ctl.check_orphans() {
                Ok(crate::mount_runtime::OrphanCheckOutcome::Clean) => {}
                Ok(crate::mount_runtime::OrphanCheckOutcome::Rejected(paths)) => {
                    log::error!(
                        "pcloud-rs: refusing to start mount service - orphan FUSE mounts detected: {paths:?}. \
                         Recover with: pcloudc mount --force-umount <path>  (or set PCLOUD_FORCE_UMOUNT=1)"
                    );
                }
                Ok(crate::mount_runtime::OrphanCheckOutcome::ForceUnmounted(results)) => {
                    for (path, res) in results {
                        match res {
                            Ok(()) => {
                                log::info!(
                                    "pcloud-rs: force-unmounted orphan at {}",
                                    path.display()
                                )
                            }
                            Err(e) => log::error!(
                                "pcloud-rs: force-unmount of {} failed: {e}",
                                path.display()
                            ),
                        }
                    }
                }
                Err(e) => {
                    log::error!("pcloud-rs: orphan-mount scan failed: {e}");
                }
            }
            ctl
        },
        transport_factory,
        session_supervisor: crate::session_lifecycle::SessionSupervisor::new(
            session_refresh_policy,
        ),
        // H14 PR4 — integrity sweeper. Built from the validated config
        // block (default: disabled). The worker thread is spawned on
        // demand by `RuntimeShell::bootstrap_integrity_sweeper`. Bead:
        // bd-1du.4.6.1.
        integrity_sweeper:
            crate::runtime::integrity_sweeper_service::IntegritySweeperShell::from_config(
                integrity_sweeper_cfg,
            )
            .map_err(BootstrapError::Provision)?,
        // Residency-enforcement region cache. Empty at startup; the three
        // enforcement call sites populate it opportunistically via the
        // `pcloud_backends::residency::resolve_or_insert_with` path. See
        // docs/enterprise/data-residency.md §4.1.
        residency_cache: pcloud_backends::residency::RegionCache::new(),
        ha,
        // IPC per-session rate limiter. Built from the validated
        // `[rate_limit]` config block. See
        // `pcloud_daemon::rate_limit::SessionRateLimiter` for the
        // admission rules and `docs/book/src/reference/config.md` for
        // the operator-facing tuning guide.
        rate_limiter: crate::rate_limit::PerPeerRateLimiter::new(&rate_limit_policy),
        // Operator-visible upload-session registry. Empty at bootstrap;
        // populated by `Request::UploadCreate`. In-memory only.
        upload_sessions: pcloud_backends::upload_sessions::SessionRegistry::new(),
        // Scheduled audit-chain verifier (I04 follow-up). Built from the
        // validated `[features.audit_verifier]` config block. Default is
        // enabled at 03:00 daily. The cron scheduler is started by the
        // daemon's main loop after the runtime shell is fully constructed.
        audit_verifier: crate::audit_verifier_service::AuditVerifierShell::from_config(
            audit_verifier_cfg,
        )
        .map_err(BootstrapError::Provision)?,
        // Background sync loop shared state. Initialized to `None`;
        // the caller spawns the loop thread after construction and
        // sets the shared handle on the shell.
        sync_loop_shared: None,
        config_path: None,
        // ncx.54: populated by `dispatch::dispatch_with_peer` for the
        // lifetime of a single IPC request; `None` at bootstrap.
        current_peer_pid: None,
    })
}
